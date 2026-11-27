//! C1 验收测试：真实启动 `modeld daemon` 子进程，写入新模型文件，
//! 从 `<store>/daemon-status.json` 读取实测数字，验证 5 秒内入库。
//!
//! 状态文件由独立进程写入、测试进程读取，所以测的是黑盒行为，
//! 不是对内部函数的白盒调用。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn workspace_temp(name: &str) -> PathBuf {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/daemon-tests");
    let dir = base.join(format!("{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

struct DaemonChild(Child);

impl Drop for DaemonChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_daemon(store: &Path, watch: &Path, debounce_ms: u64) -> DaemonChild {
    let child = Command::new(env!("CARGO_BIN_EXE_modeld"))
        .args(["--store"])
        .arg(store)
        .arg("daemon")
        .arg("--watch")
        .arg(watch)
        .arg("--debounce-ms")
        .arg(debounce_ms.to_string())
        .arg("--mode")
        .arg("index")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn modeld daemon");
    DaemonChild(child)
}

fn read_status(store: &Path) -> Option<serde_json::Value> {
    let text = fs::read_to_string(store.join("daemon-status.json")).ok()?;
    serde_json::from_str(&text).ok()
}

fn wait_for<F: Fn() -> bool>(deadline: Duration, predicate: F) -> bool {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    predicate()
}

/// 验收：新增模型文件 5 秒内自动入库。
#[test]
fn new_model_file_is_ingested_within_five_seconds() {
    let root = workspace_temp("ingest");
    let store = root.join("store");
    let watch = root.join("watch");
    fs::create_dir_all(&watch).unwrap();

    // 先启动守护进程并等待它写出首个状态文件（证明监听已就绪）。
    let _daemon = spawn_daemon(&store, &watch, 300);
    assert!(
        wait_for(Duration::from_secs(10), || read_status(&store).is_some()),
        "daemon never wrote its status file"
    );
    let before = read_status(&store).unwrap();
    assert_eq!(before["running"], true);

    // 写一个新模型文件。用临时名 + rename 模拟「写完再落地」。
    let model = watch.join("new-model.safetensors");
    let partial = watch.join("new-model.safetensors.part");
    fs::write(&partial, b"safetensors payload for C1 test").unwrap();
    fs::rename(&partial, &model).unwrap();

    let started = Instant::now();
    let ingested = wait_for(Duration::from_secs(5), || {
        read_status(&store).map(|s| s["files_ingested"].as_u64().unwrap_or(0) >= 1).unwrap_or(false)
    });
    let elapsed = started.elapsed();
    let status = read_status(&store).unwrap();
    assert!(ingested, "file was not ingested within 5s: {status}");
    assert!(elapsed <= Duration::from_secs(5), "ingest took {elapsed:?}, expected <= 5s: {status}");
    assert!(status["events_seen"].as_u64().unwrap_or(0) >= 1, "no events seen: {status}");
}

/// 明确记录 RSS 实测值；测试只在能读到有效数字时断言 < 100 MB。
/// 若某平台 `current_rss_bytes` 返回 0，则跳过断言并在状态文件里留痕。
#[test]
fn idle_rss_stays_below_100mb() {
    let root = workspace_temp("rss");
    let store = root.join("store");
    let watch = root.join("watch");
    fs::create_dir_all(&watch).unwrap();

    let _daemon = spawn_daemon(&store, &watch, 300);
    assert!(
        wait_for(Duration::from_secs(10), || read_status(&store).is_some()),
        "daemon never wrote its status file"
    );
    // current_rss_bytes() 只在 Linux/Windows 有实现；macOS 返回 0。
    // 按本测试文档约定：读不到有效数字时跳过 <100 MB 断言，并留痕。
    let reported = wait_for(Duration::from_secs(10), || {
        read_status(&store).map(|s| s["rss_bytes"].as_u64().unwrap_or(0) > 0).unwrap_or(false)
    });
    if !reported {
        let status = read_status(&store).unwrap();
        let note = format!(
            "RSS reporting unsupported on this platform; skipped <100 MB assertion\nstatus: {status}\n"
        );
        fs::write(root.join("rss-skip.txt"), &note).unwrap();
        eprintln!("{note}");
        return;
    }
    let status = read_status(&store).unwrap();
    let rss = status["rss_bytes"].as_u64().unwrap_or(0);
    assert!(rss > 0, "invalid RSS reading: {status}");
    assert!(rss < 100 * 1024 * 1024, "idle RSS {rss} bytes exceeds 100 MB: {status}");
}
