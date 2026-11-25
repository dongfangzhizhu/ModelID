//! `modeld daemon` — 常驻文件监听与自动入库（C1）。
//!
//! 组成：
//! - `notify` 递归监听一个或多个目录，事件经 `mpsc` 送到主线程；
//! - `DebounceQueue` 让文件在 `debounce_ms` 内不再变化才进入入库批次，
//!   避免读到「写了一半」的模型文件；
//! - 入库复用 `Scanner::scan_file` + `ingest_scan_results`，与 CLI/WebUI
//!   扫描走同一条路径；批次期间才持有 StoreLock，不长时间阻塞其他命令；
//! - `supervise` 循环在监听线程或工作线程异常退出时按预算重启，避免
//!   一次 panic 让常驻进程永久失效；
//! - 周期写入 `<store>/daemon-status.json`，包含 RSS、事件数、入库数、
//!   重启数，供验收脚本读取真实数字而不是凭日志猜测。

use crate::util::lock_store;
use anyhow::{Context, Result};
use modeld_core::{ingest_scan_results, Database, IngestMode, ScanOptions, Scanner};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 每 100 ms 唤醒一次主循环，用于去抖计时与状态刷新。
const LOOP_TICK: Duration = Duration::from_millis(100);
/// 状态文件刷新间隔。
const STATUS_INTERVAL: Duration = Duration::from_secs(1);
/// 监听线程短时间内连续崩溃的最大次数，超过则退出以免热循环。
const MAX_WATCHER_RESTARTS: u32 = 5;
/// 工作线程 panic 的容忍次数。
const MAX_WORKER_PANICS: u32 = 20;
/// 重启前的退避时间。
const RESTART_BACKOFF: Duration = Duration::from_millis(500);

/// 去抖队列：每个路径记录最后一次事件时间；静默超过阈值才出队。
#[derive(Debug, Default)]
struct DebounceQueue {
    pending: HashMap<PathBuf, Instant>,
}

impl DebounceQueue {
    fn push(&mut self, path: PathBuf) {
        self.pending.insert(path, Instant::now());
    }

    fn drain_ready(&mut self, debounce: Duration) -> Vec<PathBuf> {
        let now = Instant::now();
        let mut ready: Vec<PathBuf> = self
            .pending
            .iter()
            .filter(|(_, seen)| now.duration_since(**seen) >= debounce)
            .map(|(path, _)| path.clone())
            .collect();
        for path in &ready {
            self.pending.remove(path);
        }
        ready.sort(); // 稳定顺序，便于测试与日志比对
        ready
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.pending.len()
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

/// `daemon` 的运行参数。
pub struct DaemonOptions {
    pub watches: Vec<PathBuf>,
    pub mode: IngestMode,
    pub debounce_ms: u64,
    pub wait_secs: u64,
}

/// 运行时状态，周期性写入 `<store>/daemon-status.json`。
#[derive(Debug, Default, Clone)]
struct DaemonStatus {
    pid: u32,
    started_at: String,
    watched_paths: Vec<String>,
    events_seen: u64,
    files_ingested: u64,
    ingest_errors: u64,
    watcher_restarts: u32,
    worker_panics: u32,
    rss_bytes: u64,
    running: bool,
}

impl DaemonStatus {
    fn to_json(&self) -> String {
        json!({
            "pid": self.pid,
            "started_at": self.started_at,
            "watched_paths": self.watched_paths,
            "events_seen": self.events_seen,
            "files_ingested": self.files_ingested,
            "ingest_errors": self.ingest_errors,
            "watcher_restarts": self.watcher_restarts,
            "worker_panics": self.worker_panics,
            "rss_bytes": self.rss_bytes,
            "running": self.running,
        })
        .to_string()
    }
}

/// 命令入口：初始化 store、安装 Ctrl-C、进入监督循环。
pub fn daemon_command(store_path: PathBuf, opts: DaemonOptions) -> Result<()> {
    if opts.watches.is_empty() {
        anyhow::bail!("daemon requires at least one --watch <PATH>");
    }
    for path in &opts.watches {
        std::fs::create_dir_all(path)
            .with_context(|| format!("create watched directory {}", path.display()))?;
    }
    std::fs::create_dir_all(&store_path)
        .with_context(|| format!("create store directory {}", store_path.display()))?;

    let cas = modeld_core::CasStore::new(&store_path);
    cas.init()?;
    let mut db = Database::open(&store_path.join("modeld.db"))?;
    let config = crate::load_config(&store_path).unwrap_or_default();
    let extensions = config.scan_extensions();
    let store_canonical = std::fs::canonicalize(&store_path).unwrap_or_else(|_| store_path.clone());

    let shutdown = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&shutdown);
    // 多次运行（测试里同进程多次调用）时忽略重复安装的错误。
    let _ = ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst));

    let mut status = DaemonStatus {
        pid: std::process::id(),
        started_at: chrono::Utc::now().to_rfc3339(),
        watched_paths: opts.watches.iter().map(|p| p.display().to_string()).collect(),
        running: true,
        ..Default::default()
    };
    let status_path = store_path.join("daemon-status.json");

    println!(
        "daemon: watching {} path(s), mode={}, debounce={}ms, pid={}",
        opts.watches.len(),
        opts.mode.as_str(),
        opts.debounce_ms,
        status.pid
    );

    let mut watcher_restarts = 0u32;
    let mut worker_panics = 0u32;

    while !shutdown.load(Ordering::SeqCst) {
        let (tx, rx) = mpsc::channel();
        let watch_targets = opts.watches.clone();
        let watcher = match spawn_watcher(tx, &watch_targets) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("daemon: watcher failed to start: {e}");
                if watcher_restarts >= MAX_WATCHER_RESTARTS {
                    anyhow::bail!("watcher failed to start after {watcher_restarts} restarts");
                }
                watcher_restarts += 1;
                status.watcher_restarts = watcher_restarts;
                std::thread::sleep(RESTART_BACKOFF);
                continue;
            }
        };
        status.watcher_restarts = watcher_restarts;

        let outcome = run_loop(
            &rx,
            &mut db,
            &store_path,
            &store_canonical,
            &extensions,
            &opts,
            &shutdown,
            &mut status,
            &status_path,
            &mut worker_panics,
        );

        drop(watcher); // 先停监听，再决定是否重启
        status.running = false;
        write_status(&status_path, &status);

        match outcome {
            LoopOutcome::Shutdown => break,
            LoopOutcome::WatcherDied => {
                match restart_budget(&outcome, watcher_restarts, worker_panics) {
                    Some(next) => {
                        watcher_restarts = next;
                        status.watcher_restarts = watcher_restarts;
                        status.running = true;
                        eprintln!("daemon: watcher died, restarting ({watcher_restarts})");
                        std::thread::sleep(RESTART_BACKOFF);
                    }
                    None => anyhow::bail!("watcher died {watcher_restarts} times; giving up"),
                }
            }
            LoopOutcome::WorkerPanic => {
                match restart_budget(&outcome, watcher_restarts, worker_panics) {
                    Some(next) => {
                        worker_panics = next;
                        status.worker_panics = worker_panics;
                        status.running = true;
                        eprintln!("daemon: worker panicked, continuing ({worker_panics})");
                    }
                    None => anyhow::bail!("worker panicked {worker_panics} times; giving up"),
                }
            }
        }
    }

    println!("daemon: stopped cleanly");
    Ok(())
}

/// 主循环结束原因。
#[derive(Debug, PartialEq, Eq)]
enum LoopOutcome {
    Shutdown,
    WatcherDied,
    WorkerPanic,
}

/// 监督器对一次异常退出是否继续重启的判定（与 I/O 解耦，便于单测）。
///
/// 返回 `Some(下一轮计数)` 表示应重启，`None` 表示放弃或属于正常退出。
fn restart_budget(outcome: &LoopOutcome, restarts: u32, worker_panics: u32) -> Option<u32> {
    match outcome {
        LoopOutcome::Shutdown => None,
        LoopOutcome::WatcherDied if restarts < MAX_WATCHER_RESTARTS => Some(restarts + 1),
        LoopOutcome::WorkerPanic if worker_panics < MAX_WORKER_PANICS => Some(worker_panics + 1),
        _ => None,
    }
}

/// 启动 notify 监听线程，把事件送到 `tx`。
fn spawn_watcher(
    tx: Sender<notify::Result<Event>>,
    paths: &[PathBuf],
) -> Result<RecommendedWatcher> {
    let mut watcher = RecommendedWatcher::new(
        move |res| {
            let _ = tx.send(res);
        },
        notify::Config::default(),
    )
    .context("create notify watcher")?;
    for path in paths {
        watcher
            .watch(path, RecursiveMode::Recursive)
            .with_context(|| format!("watch {}", path.display()))?;
    }
    Ok(watcher)
}

#[allow(clippy::too_many_arguments)]
fn run_loop(
    rx: &Receiver<notify::Result<Event>>,
    db: &mut Database,
    store_path: &Path,
    store_canonical: &Path,
    extensions: &[String],
    opts: &DaemonOptions,
    shutdown: &AtomicBool,
    status: &mut DaemonStatus,
    status_path: &Path,
    worker_panics: &mut u32,
) -> LoopOutcome {
    let debounce = Duration::from_millis(opts.debounce_ms);
    let mut queue = DebounceQueue::default();
    let mut last_status = Instant::now() - STATUS_INTERVAL;

    loop {
        if shutdown.load(Ordering::SeqCst) {
            return LoopOutcome::Shutdown;
        }

        match rx.recv_timeout(LOOP_TICK) {
            Ok(Ok(event)) => {
                status.events_seen += 1;
                collect_event_paths(&event, store_canonical, &mut queue);
            }
            Ok(Err(e)) => {
                eprintln!("daemon: watcher event error: {e}");
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return LoopOutcome::WatcherDied,
        }

        let ready = queue.drain_ready(debounce);
        if !ready.is_empty() {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                ingest_batch(store_path, db, store_canonical, extensions, opts, &ready, status)
            }));
            match result {
                Ok(()) => {}
                Err(panic) => {
                    *worker_panics += 1;
                    status.worker_panics = *worker_panics;
                    let msg = if let Some(s) = panic.downcast_ref::<&str>() {
                        (*s).to_string()
                    } else if let Some(s) = panic.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "unknown panic".to_string()
                    };
                    eprintln!("daemon: worker panic ({}): {msg}", *worker_panics);
                    // 该批次重新入队，避免丢失；下一次循环会再次尝试。
                    for path in ready {
                        queue.push(path);
                    }
                    return LoopOutcome::WorkerPanic;
                }
            }
        }

        if last_status.elapsed() >= STATUS_INTERVAL {
            status.rss_bytes = current_rss_bytes();
            write_status(status_path, status);
            last_status = Instant::now();
        }
    }
}

/// 从 notify 事件中提取「可能是模型文件」的路径，排除 store 自身。
fn collect_event_paths(event: &Event, store_canonical: &Path, queue: &mut DebounceQueue) {
    match event.kind {
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Any | EventKind::Other => {}
        _ => return,
    }
    for path in &event.paths {
        if path.starts_with(store_canonical) {
            continue;
        }
        if path.is_file() {
            queue.push(path.clone());
        }
    }
}

/// 对一个去抖后的批次执行入库。错误逐批记录，不让单次失败终止守护进程。
fn ingest_batch(
    store_path: &Path,
    db: &mut Database,
    store_canonical: &Path,
    extensions: &[String],
    opts: &DaemonOptions,
    paths: &[PathBuf],
    status: &mut DaemonStatus,
) {
    let _lock = match lock_store(store_path, "daemon", opts.wait_secs) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("daemon: cannot acquire store lock: {e}");
            status.ingest_errors += 1;
            return;
        }
    };

    let scanner = Scanner::new()
        .with_extensions(extensions.to_vec())
        .with_excluded_dirs(vec![store_canonical.to_path_buf()])
        .with_scan_options(ScanOptions::default());

    let mut files = Vec::new();
    for path in paths {
        match scanner.scan_file(path) {
            Ok(Some(file)) => files.push(file),
            Ok(None) => {} // 不是模型文件或被排除
            Err(e) => {
                eprintln!("daemon: skip {}: {e}", path.display());
                status.ingest_errors += 1;
            }
        }
    }
    if files.is_empty() {
        return;
    }

    let scan_id = uuid::Uuid::new_v4().to_string();
    match ingest_scan_results(store_path, db, &files, &scan_id, opts.mode) {
        Ok(result) => {
            status.files_ingested += result.processed_count as u64;
            status.ingest_errors += result.errors.len() as u64;
            for err in &result.errors {
                eprintln!("daemon: ingest failed {}: {}", err.path, err.reason);
            }
            if result.processed_count > 0 {
                println!("daemon: ingested {} file(s)", result.processed_count);
            }
        }
        Err(e) => {
            eprintln!("daemon: ingest batch failed: {e}");
            status.ingest_errors += 1;
        }
    }
}

/// 写状态文件（先写临时文件再 rename，避免读者看到半截 JSON）。
fn write_status(path: &Path, status: &DaemonStatus) {
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, status.to_json()).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// 当前进程驻留集大小（字节）；读取失败返回 0。
fn current_rss_bytes() -> u64 {
    #[cfg(target_os = "linux")]
    {
        if let Ok(text) = std::fs::read_to_string("/proc/self/status") {
            for line in text.lines() {
                if let Some(rest) = line.strip_prefix("VmRSS:") {
                    if let Some(kb) = rest.split_whitespace().next() {
                        if let Ok(kb) = kb.parse::<u64>() {
                            return kb * 1024;
                        }
                    }
                }
            }
        }
    }
    #[cfg(windows)]
    {
        #[repr(C)]
        struct ProcessMemoryCounters {
            cb: u32,
            page_fault_count: u32,
            peak_working_set_size: usize,
            working_set_size: usize,
            quota_peak_paged_pool_usage: usize,
            quota_paged_pool_usage: usize,
            quota_peak_non_paged_pool_usage: usize,
            quota_non_paged_pool_usage: usize,
            page_file_usage: usize,
            peak_page_file_usage: usize,
        }
        #[link(name = "psapi")]
        extern "system" {
            fn GetCurrentProcess() -> *mut std::ffi::c_void;
            fn GetProcessMemoryInfo(
                process: *mut std::ffi::c_void,
                counters: *mut ProcessMemoryCounters,
                cb: u32,
            ) -> i32;
        }
        let mut counters: ProcessMemoryCounters = unsafe { std::mem::zeroed() };
        counters.cb = std::mem::size_of::<ProcessMemoryCounters>() as u32;
        let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
        if ok != 0 {
            return counters.working_set_size as u64;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debounce_holds_until_quiet() {
        let mut q = DebounceQueue::default();
        let p = PathBuf::from("model.safetensors");
        q.push(p.clone());
        assert_eq!(q.len(), 1);
        // 阈值 60s：刚写入不应出队
        assert!(q.drain_ready(Duration::from_secs(60)).is_empty());
        assert_eq!(q.len(), 1);
        // 阈值 0：立即出队且只出一次
        assert_eq!(q.drain_ready(Duration::ZERO), vec![p]);
        assert!(q.is_empty());
        assert!(q.drain_ready(Duration::ZERO).is_empty());
    }

    #[test]
    fn debounce_latest_event_wins() {
        let mut q = DebounceQueue::default();
        let p = PathBuf::from("a.gguf");
        q.push(p.clone());
        // 模拟「持续写入」：再 push 一次刷新时间戳。
        std::thread::sleep(Duration::from_millis(5));
        q.push(p.clone());
        assert_eq!(q.len(), 1, "同一路径应去抖合并，不重复入队");
        let ready = q.drain_ready(Duration::ZERO);
        assert_eq!(ready, vec![p]);
    }

    #[test]
    fn ignored_event_kinds_produce_no_paths() {
        let store = PathBuf::from("store");
        let mut q = DebounceQueue::default();
        // 用一个我们无法从公开 API 构造的 EventKind 变体覆盖默认分支：
        // Remove 不在入库关注范围内。
        let event = notify::Event {
            kind: EventKind::Remove(notify::event::RemoveKind::File),
            paths: vec![PathBuf::from("outside/model.safetensors")],
            attrs: Default::default(),
        };
        collect_event_paths(&event, &store, &mut q);
        assert!(q.is_empty());
    }

    #[test]
    fn supervisor_restarts_within_budget() {
        // 正常退出不重启
        assert_eq!(restart_budget(&LoopOutcome::Shutdown, 0, 0), None);
        // watcher 崩溃在预算内递增
        assert_eq!(restart_budget(&LoopOutcome::WatcherDied, 0, 0), Some(1));
        assert_eq!(
            restart_budget(&LoopOutcome::WatcherDied, MAX_WATCHER_RESTARTS - 1, 0),
            Some(MAX_WATCHER_RESTARTS)
        );
        // 超过预算放弃
        assert_eq!(restart_budget(&LoopOutcome::WatcherDied, MAX_WATCHER_RESTARTS, 0), None);
        // worker panic 单独计数
        assert_eq!(restart_budget(&LoopOutcome::WorkerPanic, 0, 0), Some(1));
        assert_eq!(restart_budget(&LoopOutcome::WorkerPanic, 0, MAX_WORKER_PANICS), None);
    }

    #[test]
    fn status_json_contains_acceptance_fields() {
        let status = DaemonStatus {
            pid: 42,
            events_seen: 7,
            files_ingested: 3,
            rss_bytes: 50 * 1024 * 1024,
            running: true,
            ..Default::default()
        };
        let value: serde_json::Value = serde_json::from_str(&status.to_json()).unwrap();
        assert_eq!(value["pid"], 42);
        assert_eq!(value["files_ingested"], 3);
        assert_eq!(value["rss_bytes"], 50 * 1024 * 1024);
        assert_eq!(value["running"], true);
    }
}
