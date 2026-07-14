//! Internationalization (i18n) for modeld's command-line output.
//!
//! Scope: this module localizes the human-facing text printed by the
//! `modeld` CLI (`println!`/`eprintln!`/`anyhow::bail!`) and the proxy
//! server's startup banner. It deliberately does NOT translate:
//!
//! - `--json` machine-readable output
//! - the HTTP API JSON bodies / error strings (clients may parse them)
//! - clap's derived `--help` text (the `clap` derive macros require literal
//!   strings at compile time; runtime translation of help would need a large
//!   refactor for little gain)
//!
//! Language selection priority (first match wins):
//! 1. `MODELD_LANG` environment variable  (explicit override, e.g. `en`/`zh`)
//! 2. `LC_ALL` / `LC_MESSAGES` / `LANG`     (POSIX/MSYS conventions)
//! 3. the host OS user locale               (`sys_locale::get_locale()`)
//! 4. fall back to English (`Lang::En`)
//!
//! See `docs/i18n.md` for the user-facing description of this behavior.

use std::sync::OnceLock;

/// A supported user-interface language.
///
/// Only two values are exposed today. `detect()` collapses every system
/// locale to one of these: any Chinese locale (`zh*`) → [`Lang::Zh`],
/// everything else → [`Lang::En`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    /// English (the default for any non-Chinese locale).
    En,
    /// Simplified/Traditional Chinese (covers all `zh*` locales).
    Zh,
}

impl Lang {
    /// The BCP-47-ish short code (`"en"` / `"zh"`). Used for logging/debugging.
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Zh => "zh",
        }
    }
}

/// Cached result of [`detect`]. Computed once on first use; the env / locale
/// are not expected to change during a single CLI invocation.
static LANG: OnceLock<Lang> = OnceLock::new();

/// Detect the active UI language using the priority order documented above.
///
/// Cheap and idempotent: the result is cached after the first call.
pub fn detect() -> Lang {
    *LANG.get_or_init(detect_uncached)
}

fn detect_uncached() -> Lang {
    // 1. Explicit override via MODELD_LANG.
    if let Some(l) = std::env::var("MODELD_LANG").ok().as_deref().and_then(parse_lang_tag) {
        return l;
    }
    // 2. POSIX/MSYS locale variables, most-specific first.
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Some(l) = std::env::var(var).ok().as_deref().and_then(parse_lang_tag) {
            return l;
        }
    }
    // 3. Host OS user locale (GetUserDefaultLocaleName on Windows, etc.).
    if let Some(loc) = sys_locale::get_locale() {
        if let Some(l) = parse_lang_tag(&loc) {
            return l;
        }
    }
    // 4. Fallback.
    Lang::En
}

/// Map a locale tag (or `MODELD_LANG` value) to a [`Lang`].
///
/// Accepts values like `zh`, `zh-CN`, `zh_CN.UTF-8`, `chinese`, `en`,
/// `en-US`, `czech`, … Anything Chinese → [`Lang::Zh`]; an explicit `en` →
/// [`Lang::En`]; otherwise `None` (so that e.g. `LC_MESSAGES=fr_FR` lets the
/// lookup continue down the chain instead of forcing English).
fn parse_lang_tag(tag: &str) -> Option<Lang> {
    let lower = tag.trim().to_ascii_lowercase();
    if lower.is_empty() || lower == "c" || lower == "posix" {
        return None;
    }
    // Strip codeset/modifier suffixes: "zh_CN.UTF-8" -> "zh_cn".
    let base = lower.split(['.', '_', '-']).next().unwrap_or("");
    match base {
        "zh" | "chinese" | "ch" | "cmn" | "zho" => Some(Lang::Zh),
        "en" | "english" => Some(Lang::En),
        _ => None,
    }
}

/// Look up a fixed (non-parameterized) message for the active language.
///
/// On an unknown `key` the key itself is returned (internally leaked, since the
/// set of keys is small and fixed for a given CLI invocation). That makes
/// missing translations obvious during development without panicking in
/// production.
pub fn t(key: &str) -> &'static str {
    let localized = match detect() {
        Lang::En => en(key),
        Lang::Zh => zh(key).or_else(|| en(key)),
    };
    match localized {
        Some(s) => s,
        // Unknown key: surface the key verbatim so a missing translation is
        // immediately visible. Leaked because `t` returns `&'static str`; the
        // key set is tiny and the process is short-lived.
        None => leak_str(key),
    }
}

fn leak_str(s: &str) -> &'static str {
    // Box::leak the string's bytes so it lives for `'static`. Each unique key
    // is leaked at most once in practice; for a CLI this is a non-issue.
    let boxed: Box<str> = Box::from(s);
    Box::leak(boxed)
}

/// Look up a message and substitute `{name}` placeholders.
///
/// `args` is a slice of `(name, value)` pairs; each `{name}` occurrence in the
/// localized template is replaced with the value's `Display` form. Unknown
/// placeholders are left as-is.
///
/// Example:
/// ```ignore
/// tf("scan.found", &[("count", &file_count), ("gb", &gb_str)])
/// ```
pub fn tf(key: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let template = t(key);
    fill(template, args)
}

fn fill(template: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    if !template.contains('{') {
        return template.to_string();
    }
    let mut out = template.to_string();
    for (name, value) in args {
        let needle = format!("{{{name}}}");
        if out.contains(&needle) {
            out = out.replace(&needle, &value.to_string());
        }
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// English message table
// ─────────────────────────────────────────────────────────────────────────────

fn en(key: &str) -> Option<&'static str> {
    Some(match key {
        // ── common / errors ──
        "store.not_initialized" => "Store not initialized. Run 'modeld init' first.",
        "store.not_initialized_exit" => "Store not initialized. Run `modeld init` first.",
        "error.prefix" => "Error:",
        "error.file_not_found" => "File not found: {path}",
        "error.workflow_dir_missing" => "Workflow directory does not exist: {path}",
        "error.workflow_file_missing" => "File not found: {path}",
        "error.model_not_found" => "model not found: {hash}",
        "error.size_empty" => "size cannot be empty",
        "error.size_invalid" => "invalid size: {input}",
        "error.size_unit" => "unsupported size unit: {unit}",

        // ── init ──
        "init.at" => "Initializing modeld store at: {path}",
        "init.success" => "✓ Store initialized successfully",
        "init.next_steps" => "Next steps:",
        "init.step1" => "1. modeld scan <directory>",
        "init.step2" => "2. modeld status",
        "init.step3" => "3. modeld dedup --dry-run",

        // ── scan ──
        "scan.scanning" => "Scanning: {path}",
        "scan.no_files" => "No model files found",
        "scan.found" => "Found {count} model files ({gb} GB)",
        "scan.processing" => "Processing files...",
        "scan.complete" => "✓ Scan complete",
        "scan.processed" => "  Processed: {count}",
        "scan.total_size" => "  Total size: {gb} GB",
        "scan.errors_header" => "  Errors: {count}",
        "scan.error_line" => "    {path}: {reason}",
        "scan.progress_msg" => "{name} ({mb} MB)",
        "progress.done" => "Done",
        "progress.processing" => "Processing {prefix} ({current}/{total})",

        // ── status ──
        "status.header" => "modeld Store Status",
        "status.store_path" => "  Store path: {path}",
        "status.total_models" => "  Total models: {count}",
        "status.total_size" => "  Total size: {gb} GB",
        "status.recent" => "Recent models:",
        "status.recent_line" => "  {hash} - {mb} MB",
        "status.quarantine" => "Quarantine:",
        "status.q_files" => "  Files: {count}",
        "status.q_size" => "  Size: {mb} MB",
        "status.q_expired" => "  {count} expired (run 'modeld quarantine cleanup')",

        // ── stats ──
        "stats.header" => "modeld Statistics",
        "stats.store_path" => "  Store path: {path}",
        "stats.models" => "  Models: {count}",
        "stats.aliases" => "  Aliases: {count}",
        "stats.indexed_size" => "  Indexed size: {gb} GB",
        "stats.dup_groups" => "  Duplicate groups: {count}",
        "stats.savings" => "  Potential savings: {gb} GB",

        // ── dupes ──
        "dupes.header" => "Duplicate Models",
        "dupes.none" => "No duplicate files found",
        "dupes.group_line" => "{hash}  {mb} MB  {count} files",
        "dupes.file_line" => "  - {path}",
        "dupes.savings" => "Potential savings: {gb} GB",

        // ── list ──
        "list.header" => "Indexed Models",
        "list.none" => "No models indexed yet",
        "list.line" => "{hash}  {mb} MB  {format}",

        // ── info ──
        "info.header" => "Model Info",
        "info.hash" => "  Hash: {hash}",
        "info.size" => "  Size: {mb} MB",
        "info.format" => "  Format: {value}",
        "info.category" => "  Category: {value}",
        "info.last_seen" => "  Last seen: {value}",
        "info.cas_path" => "  CAS path: {path}",
        "info.aliases" => "Aliases:",
        "info.aliases_none" => "  none",
        "info.alias_line" => "  - {path} [{frontend} / {kind}]",

        // ── hash ──
        "hash.computing" => "Computing BLAKE3 hash: {path}",
        "hash.results" => "Results:",
        "hash.value" => "  Hash: {hash}",
        "hash.prefix" => "  Prefix: {prefix}",
        "hash.size" => "  Size: {mb} MB",

        // ── dedup ──
        "dedup.header" => "modeld Deduplication",
        "dedup.store" => "  Store: {path}",
        "dedup.mode" => "  Mode:  {mode}",
        "dedup.mode.dry_run" => "DRY RUN (no changes will be made)",
        "dedup.mode.report" => "REPORT (analysis only)",
        "dedup.mode.auto" => "AUTO",
        "dedup.mode.interactive" => "INTERACTIVE",
        "dedup.no_dupes" => "✓ No duplicate files found",
        "dedup.groups_found" => "→ {count} duplicate groups found",
        "dedup.potential_savings" => "→ Potential space savings: {gb} GB",
        "dedup.groups_header" => "Duplicate groups:",
        "dedup.group_header" => "  Group {i} ({mb} MB per copy, {count} copies):",
        "dedup.file_line" => "    {path}",
        "dedup.would_save" => "    → Would save {mb} MB",
        "dedup.total_potential" => "  ✓ Total potential savings: {gb} GB",
        "dedup.run_without_dry_run" => "  Run without --dry-run to apply deduplication",
        "dedup.executing" => "Executing deduplication...",
        "dedup.complete" => "Deduplication Complete",
        "dedup.groups_processed" => "  Groups processed: {count}",
        "dedup.groups_succeeded" => "  Groups succeeded: {count}",
        "dedup.groups_failed" => "  Groups failed:    {count}",
        "dedup.files_dedup" => "  Files deduplicated: {count}",
        "dedup.space_saved" => "  Space saved: {gb} GB",

        // ── quarantine ──
        "quarantine.list.none" => "No quarantined files",
        "quarantine.list.header" => "Quarantined Files",
        "quarantine.days_remaining" => "{count} days remaining",
        "quarantine.expired" => "EXPIRED",
        "quarantine.entry_header" => "\n  {path} ({status})",
        "quarantine.original" => "    Original: {path}",
        "quarantine.hash" => "    Hash:     {hash}",
        "quarantine.size" => "    Size:     {mb} MB",
        "quarantine.reason" => "    Reason:   {reason}",
        "quarantine.quarantined_at" => "    Quarantined: {value}",
        "quarantine.total" => "  Total: {count} files",
        "quarantine.size_line" => "  Size:  {mb} MB",
        "quarantine.cleanup_expired_hint" => "\n  {count} expired files - run 'modeld quarantine cleanup'",
        "quarantine.cleanup.start" => "Cleaning up expired quarantine entries...",
        "quarantine.cleanup.none" => "✓ No expired entries to clean up",
        "quarantine.cleanup.done" => "✓ Removed {count} expired quarantine entries",

        // ── hf ──
        "hf.check.hit" => "✓ Cache hit: {repo}/{file}@{rev}",
        "hf.check.path" => "  Path: {path}",
        "hf.check.miss" => "✗ Not cached: {repo}/{file}@{rev}",
        "hf.dl.cached" => "Cache hit (skipped download)",
        "hf.dl.downloaded" => "Downloaded",
        "hf.dl.success" => "✓ {status}",
        "hf.dl.repo" => "  Repo:     {repo}",
        "hf.dl.file" => "  File:     {file}",
        "hf.dl.blake3" => "  BLAKE3:   {hash}",
        "hf.dl.size" => "  Size:     {value}",
        "hf.dl.cas_path" => "  CAS path: {path}",
        "hf.setup.header" => "modeld HuggingFace Cache Setup",
        "hf.setup.dir" => "HF cache directory: {path}",
        "hf.setup.activate" => "To activate, add to your shell profile:",
        "hf.setup.windows_ps" => "  Windows (PowerShell):",
        "hf.setup.windows_cmd" => "  Windows (CMD):",
        "hf.setup.set_ps" => "  $env:HF_HOME = \"{path}\"",
        "hf.setup.set_cmd" => "  set HF_HOME={path}",
        "hf.setup.unix" => "  Linux/macOS: ~/.bashrc or ~/.zshrc:",
        "hf.setup.set_unix" => "  export HF_HOME=\"{path}\"",
        "hf.setup.hook_header" => "Or install the Python hook for automatic interception:",
        "hf.setup.hook_pip" => "  pip install modeld-hook",
        "hf.setup.hook_use" => "  # Then add to your script:",
        "hf.setup.hook_import" => "  import modeld_hook  # Auto-activates on import",
        "hf.status.header" => "HuggingFace Cache Status",
        "hf.status.not_initialized" => "HF cache not initialized. Run: modeld hf-setup",
        "hf.status.hf_home" => "  HF_HOME:     {path}",
        "hf.status.repos" => "  Repos:       {count}",
        "hf.status.blobs" => "  Blobs:       {count}",
        "hf.status.cached_repos" => "Cached repos:",
        "hf.status.repo_line" => "  • {repo}",
        "hf.status.downloads" => "  Downloads:   {total} total, {done} completed",

        // ── workflow ──
        "wf.scan.scanning" => "Scanning workflows in: {path}",
        "wf.scan.no_files" => "No workflow JSON files found.",
        "wf.scan.found" => "  Found {count} workflow files",
        "wf.scan.lookup" => "  Model lookup: {count} entries in CAS index",
        "wf.scan.complete" => "Workflow scan complete:",
        "wf.scan.workflows" => "  Workflows indexed:  {count}",
        "wf.scan.resolved" => "  Refs resolved:      {count}",
        "wf.scan.unresolved" => "  Refs unresolved:    {count}",
        "wf.scan.errors" => "  Parse errors:       {count}",
        "wf.warn" => "  WARN {path}: {error}",
        "wf.deps.workflow" => "Workflow: {path}",
        "wf.deps.title" => "  Title: {title}",
        "wf.deps.no_refs" => "No model references found.",
        "wf.deps.header" => "Model Dependencies:",
        "wf.deps.total" => "  Total: {count} refs ({resolved} resolved, {unresolved} unresolved)",
        "wf.deps.type.checkpoint" => "Checkpoints",
        "wf.deps.type.lora" => "LoRAs",
        "wf.deps.type.vae" => "VAEs",
        "wf.deps.type.clip" => "CLIP Models",
        "wf.deps.type.controlnet" => "ControlNets",
        "wf.deps.type.ipadapter" => "IPAdapters",
        "wf.deps.type.unet" => "UNets",
        "wf.deps.type.upscale_model" => "Upscale Models",

        // ── orphans ──
        "orphans.none" =>
            "No orphan models found. All models are referenced by at least one workflow.",
        "orphans.header" => "Orphan Models (no workflow references):",
        "orphans.col_hash" => "Hash",
        "orphans.col_size" => "Size",
        "orphans.col_format" => "Format",
        "orphans.summary" => "  {count} orphans, {size} reclaimable",
        "orphans.tip" =>
            "Tip: Run `modeld gc --preview` to see GC plan, or `modeld gc` to quarantine these models.",

        // ── gc ──
        "gc.preview.header" => "GC Preview (no changes will be made):",
        "gc.preview.hard" => "  {count} models hard-protected (referenced by workflows)",
        "gc.preview.soft_header" =>
            "  {count} models soft-protected (have aliases but no workflow refs):",
        "gc.preview.soft_line" => "    {prefix}... ({aliases} aliases, {size})",
        "gc.preview.quarantine_header" => "  {count} models would be quarantined:",
        "gc.preview.quarantine_line" => "    {prefix}... ({size})",
        "gc.preview.reclaimable" => "  Total reclaimable: {size}",
        "gc.preview.nothing" => "  Nothing to quarantine.",
        "gc.preview.expired" =>
            "  {count} expired quarantine entries ({size}) could be deleted permanently.",
        "gc.preview.run" => "Run `modeld gc` (without --preview) to execute.",
        "gc.running" => "Running safe GC...",
        "gc.nothing_quarantined" => "Nothing quarantined — store is clean.",
        "gc.quarantined" => "  ✓ Quarantined {count} models ({size})",
        "gc.skipped_hard" => "  • Skipped {count} hard-protected models",
        "gc.skipped_soft" => "  ⚠ Skipped {count} soft-protected models (use --force to override)",
        "gc.cleanup_header" => "Cleaning expired quarantine entries...",
        "gc.cleanup_done" => "  ✓ Permanently deleted {count} expired quarantine entries",
        "gc.cleanup_none" => "  No expired quarantine entries found.",

        // ── proxy ──
        "proxy.listening" => "modeld proxy listening on {bind} (store: {store})",
        "proxy.shutdown" => "shutdown requested, stopping proxy server",
        "proxy.discover.scanning" => "Scanning LAN for modeld proxy servers...",
        "proxy.discover.service" => "  (mDNS service: _modeld._tcp.local.)",
        "proxy.discover.none" => "No modeld proxy servers found on the local network.",
        "proxy.discover.make_sure" => "Make sure:",
        "proxy.discover.tip_server" => "  • A server is running: modeld proxy start",
        "proxy.discover.tip_mdns" => "  • mDNS is published (avahi-publish / dns-sd / Bonjour)",
        "proxy.discover.tip_subnet" => "  • The machine is on the same network/subnet",
        "proxy.discover.tip_firewall" => "  • mDNS traffic is allowed (UDP 5353)",
        "proxy.discover.found" => "✓ {count} server(s) found:",
        "proxy.discover.line" => "  • {url}",
        "proxy.status.header" => "modeld Proxy Status",
        "proxy.status.url" => "  URL:           {url}",
        "proxy.status.status" => "  Status:        {value}",
        "proxy.status.version" => "  Version:       {value}",
        "proxy.status.uptime" => "  Uptime:        {value}s",
        "proxy.status.models" => "  Models:        {count}",
        "proxy.status.total_size" => "  Total size:    {value}",
        "proxy.status.recent_header" => "Recent models (showing first 5):",
        "proxy.status.recent_line" => "  {hash}  {size}  {format}",
        "proxy.status.more" => "  ... and {count} more",
        "proxy.status.no_models" => "No models in store.",
        "proxy.status.list_failed" => "  ⚠ list_models failed: {error}",
        "proxy.status.reach_failed" => "failed to reach proxy at {url}: {error}",

        // ── internal warnings (gc / dedup / quarantine / discovery) ──
        "warn.gc_quarantine_failed" => "Warning: Failed to quarantine {hash}: {error}",
        "warn.dedup_link_failed" => "Warning: failed to create link for {path}: {error}",
        "warn.dedup_group_failed" => "Warning: dedup failed for group {prefix}: {error}",
        "warn.recover_rolled_back" => "Recovery: rolled back pending transaction {tx}",
        "warn.recover_move_failed" => "Recovery: failed to move staging to CAS for tx {tx}: {error}",
        "warn.recover_phase_b_done" => "Recovery: completed Phase B for tx {tx}",
        "warn.recover_processed" => "Recovery: processed {count} incomplete transactions",
        "warn.quarantine_cleanup_failed" =>
            "Warning: failed to clean up expired quarantine entry {path}: {error}",
        "warn.mdns_announce" =>
            "modeld mDNS: service modeld.{stype} on :{port} [TXT: {txt}]",
        "warn.mdns_announce_hint" =>
            "  (publication via host mDNS daemon: avahi-publish / dns-sd / Bonjour)",

        _ => return None,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Chinese message table — returns Option so the caller can fall back to English
// for any key not yet translated (defensive against drift during edits).
// ─────────────────────────────────────────────────────────────────────────────

fn zh(key: &str) -> Option<&'static str> {
    Some(match key {
        // ── common / errors ──
        "store.not_initialized" => "存储未初始化。请先运行 'modeld init'。",
        "store.not_initialized_exit" => "存储未初始化。请先运行 `modeld init`。",
        "error.prefix" => "错误：",
        "error.file_not_found" => "找不到文件：{path}",
        "error.workflow_dir_missing" => "工作流目录不存在：{path}",
        "error.workflow_file_missing" => "找不到文件：{path}",
        "error.model_not_found" => "未找到模型：{hash}",
        "error.size_empty" => "大小不能为空",
        "error.size_invalid" => "无效的大小：{input}",
        "error.size_unit" => "不支持的大小单位：{unit}",

        // ── init ──
        "init.at" => "正在初始化 modeld 存储于：{path}",
        "init.success" => "✓ 存储初始化成功",
        "init.next_steps" => "后续步骤：",
        "init.step1" => "1. modeld scan <目录>",
        "init.step2" => "2. modeld status",
        "init.step3" => "3. modeld dedup --dry-run",

        // ── scan ──
        "scan.scanning" => "正在扫描：{path}",
        "scan.no_files" => "未找到模型文件",
        "scan.found" => "找到 {count} 个模型文件（{gb} GB）",
        "scan.processing" => "正在处理文件…",
        "scan.complete" => "✓ 扫描完成",
        "scan.processed" => "  已处理：{count}",
        "scan.total_size" => "  总大小：{gb} GB",
        "scan.errors_header" => "  错误：{count}",
        "scan.error_line" => "    {path}：{reason}",
        "scan.progress_msg" => "{name}（{mb} MB）",
        "progress.done" => "完成",
        "progress.processing" => "正在处理 {prefix}（{current}/{total}）",

        // ── status ──
        "status.header" => "modeld 存储状态",
        "status.store_path" => "  存储路径：{path}",
        "status.total_models" => "  模型总数：{count}",
        "status.total_size" => "  总大小：{gb} GB",
        "status.recent" => "最近的模型：",
        "status.recent_line" => "  {hash} - {mb} MB",
        "status.quarantine" => "隔离区：",
        "status.q_files" => "  文件数：{count}",
        "status.q_size" => "  大小：{mb} MB",
        "status.q_expired" => "  {count} 个已过期（运行 'modeld quarantine cleanup'）",

        // ── stats ──
        "stats.header" => "modeld 统计信息",
        "stats.store_path" => "  存储路径：{path}",
        "stats.models" => "  模型：{count}",
        "stats.aliases" => "  别名：{count}",
        "stats.indexed_size" => "  索引大小：{gb} GB",
        "stats.dup_groups" => "  重复分组：{count}",
        "stats.savings" => "  预计节省：{gb} GB",

        // ── dupes ──
        "dupes.header" => "重复模型",
        "dupes.none" => "未发现重复文件",
        "dupes.group_line" => "{hash}  {mb} MB  {count} 个文件",
        "dupes.file_line" => "  - {path}",
        "dupes.savings" => "预计节省：{gb} GB",

        // ── list ──
        "list.header" => "已索引模型",
        "list.none" => "尚未索引任何模型",
        "list.line" => "{hash}  {mb} MB  {format}",

        // ── info ──
        "info.header" => "模型信息",
        "info.hash" => "  哈希：{hash}",
        "info.size" => "  大小：{mb} MB",
        "info.format" => "  格式：{value}",
        "info.category" => "  类别：{value}",
        "info.last_seen" => "  最后出现：{value}",
        "info.cas_path" => "  CAS 路径：{path}",
        "info.aliases" => "别名：",
        "info.aliases_none" => "  无",
        "info.alias_line" => "  - {path} [{frontend} / {kind}]",

        // ── hash ──
        "hash.computing" => "正在计算 BLAKE3 哈希：{path}",
        "hash.results" => "结果：",
        "hash.value" => "  哈希：{hash}",
        "hash.prefix" => "  前缀：{prefix}",
        "hash.size" => "  大小：{mb} MB",

        // ── dedup ──
        "dedup.header" => "modeld 去重",
        "dedup.store" => "  存储：{path}",
        "dedup.mode" => "  模式：{mode}",
        "dedup.mode.dry_run" => "预演（不会做任何更改）",
        "dedup.mode.report" => "报告（仅分析）",
        "dedup.mode.auto" => "自动",
        "dedup.mode.interactive" => "交互",
        "dedup.no_dupes" => "✓ 未发现重复文件",
        "dedup.groups_found" => "→ 发现 {count} 个重复分组",
        "dedup.potential_savings" => "→ 预计可节省空间：{gb} GB",
        "dedup.groups_header" => "重复分组：",
        "dedup.group_header" => "  分组 {i}（每份 {mb} MB，共 {count} 份）：",
        "dedup.file_line" => "    {path}",
        "dedup.would_save" => "    → 将节省 {mb} MB",
        "dedup.total_potential" => "  ✓ 总计预计节省：{gb} GB",
        "dedup.run_without_dry_run" => "  去掉 --dry-run 运行以应用去重",
        "dedup.executing" => "正在执行去重…",
        "dedup.complete" => "去重完成",
        "dedup.groups_processed" => "  处理分组数：{count}",
        "dedup.groups_succeeded" => "  成功分组数：{count}",
        "dedup.groups_failed" => "  失败分组数：{count}",
        "dedup.files_dedup" => "  已去重文件数：{count}",
        "dedup.space_saved" => "  已节省空间：{gb} GB",

        // ── quarantine ──
        "quarantine.list.none" => "没有已隔离的文件",
        "quarantine.list.header" => "已隔离文件",
        "quarantine.days_remaining" => "剩余 {count} 天",
        "quarantine.expired" => "已过期",
        "quarantine.entry_header" => "\n  {path}（{status}）",
        "quarantine.original" => "    原路径：{path}",
        "quarantine.hash" => "    哈希：  {hash}",
        "quarantine.size" => "    大小：  {mb} MB",
        "quarantine.reason" => "    原因：  {reason}",
        "quarantine.quarantined_at" => "    隔离时间：{value}",
        "quarantine.total" => "  总计：{count} 个文件",
        "quarantine.size_line" => "  大小：{mb} MB",
        "quarantine.cleanup_expired_hint" => {
            "\n  {count} 个已过期文件 - 运行 'modeld quarantine cleanup'"
        }
        "quarantine.cleanup.start" => "正在清理已过期的隔离条目…",
        "quarantine.cleanup.none" => "✓ 没有需要清理的过期条目",
        "quarantine.cleanup.done" => "✓ 已移除 {count} 个过期的隔离条目",

        // ── hf ──
        "hf.check.hit" => "✓ 缓存命中：{repo}/{file}@{rev}",
        "hf.check.path" => "  路径：{path}",
        "hf.check.miss" => "✗ 未缓存：{repo}/{file}@{rev}",
        "hf.dl.cached" => "缓存命中（已跳过下载）",
        "hf.dl.downloaded" => "已下载",
        "hf.dl.success" => "✓ {status}",
        "hf.dl.repo" => "  仓库：  {repo}",
        "hf.dl.file" => "  文件：  {file}",
        "hf.dl.blake3" => "  BLAKE3：{hash}",
        "hf.dl.size" => "  大小：  {value}",
        "hf.dl.cas_path" => "  CAS 路径：{path}",
        "hf.setup.header" => "modeld HuggingFace 缓存设置",
        "hf.setup.dir" => "HF 缓存目录：{path}",
        "hf.setup.activate" => "如需启用，请添加到你的 shell 配置：",
        "hf.setup.windows_ps" => "  Windows（PowerShell）：",
        "hf.setup.windows_cmd" => "  Windows（CMD）：",
        "hf.setup.set_ps" => "  $env:HF_HOME = \"{path}\"",
        "hf.setup.set_cmd" => "  set HF_HOME={path}",
        "hf.setup.unix" => "  Linux/macOS：~/.bashrc 或 ~/.zshrc：",
        "hf.setup.set_unix" => "  export HF_HOME=\"{path}\"",
        "hf.setup.hook_header" => "或安装 Python hook 以实现自动拦截：",
        "hf.setup.hook_pip" => "  pip install modeld-hook",
        "hf.setup.hook_use" => "  # 然后在脚本中加入：",
        "hf.setup.hook_import" => "  import modeld_hook  # 导入即自动启用",
        "hf.status.header" => "HuggingFace 缓存状态",
        "hf.status.not_initialized" => "HF 缓存未初始化。运行：modeld hf-setup",
        "hf.status.hf_home" => "  HF_HOME：{path}",
        "hf.status.repos" => "  仓库数：  {count}",
        "hf.status.blobs" => "  blob 数：{count}",
        "hf.status.cached_repos" => "已缓存的仓库：",
        "hf.status.repo_line" => "  • {repo}",
        "hf.status.downloads" => "  下载：    共 {total} 个，已完成 {done} 个",

        // ── workflow ──
        "wf.scan.scanning" => "正在扫描工作流目录：{path}",
        "wf.scan.no_files" => "未找到工作流 JSON 文件。",
        "wf.scan.found" => "  找到 {count} 个工作流文件",
        "wf.scan.lookup" => "  模型查找表：CAS 索引中共 {count} 条",
        "wf.scan.complete" => "工作流扫描完成：",
        "wf.scan.workflows" => "  已索引工作流：{count}",
        "wf.scan.resolved" => "  已解析引用：  {count}",
        "wf.scan.unresolved" => "  未解析引用：  {count}",
        "wf.scan.errors" => "  解析错误：      {count}",
        "wf.warn" => "  警告 {path}：{error}",
        "wf.deps.workflow" => "工作流：{path}",
        "wf.deps.title" => "  标题：{title}",
        "wf.deps.no_refs" => "未找到模型引用。",
        "wf.deps.header" => "模型依赖：",
        "wf.deps.total" => "  总计：{count} 个引用（{resolved} 已解析，{unresolved} 未解析）",
        "wf.deps.type.checkpoint" => "Checkpoints（大模型）",
        "wf.deps.type.lora" => "LoRA",
        "wf.deps.type.vae" => "VAE",
        "wf.deps.type.clip" => "CLIP 模型",
        "wf.deps.type.controlnet" => "ControlNet",
        "wf.deps.type.ipadapter" => "IPAdapter",
        "wf.deps.type.unet" => "UNet",
        "wf.deps.type.upscale_model" => "放大模型",

        // ── orphans ──
        "orphans.none" => "未发现孤立模型。所有模型都至少被一个工作流引用。",
        "orphans.header" => "孤立模型（无工作流引用）：",
        "orphans.col_hash" => "哈希",
        "orphans.col_size" => "大小",
        "orphans.col_format" => "格式",
        "orphans.summary" => "  {count} 个孤立模型，可回收 {size}",
        "orphans.tip" => {
            "提示：运行 `modeld gc --preview` 查看 GC 计划，或运行 `modeld gc` 隔离这些模型。"
        }

        // ── gc ──
        "gc.preview.header" => "GC 预览（不会做任何更改）：",
        "gc.preview.hard" => "  {count} 个模型被硬保护（被工作流引用）",
        "gc.preview.soft_header" => "  {count} 个模型被软保护（有别名但无工作流引用）：",
        "gc.preview.soft_line" => "    {prefix}...（{aliases} 个别名，{size}）",
        "gc.preview.quarantine_header" => "  {count} 个模型将被隔离：",
        "gc.preview.quarantine_line" => "    {prefix}...（{size}）",
        "gc.preview.reclaimable" => "  总计可回收：{size}",
        "gc.preview.nothing" => "  没有需要隔离的内容。",
        "gc.preview.expired" => "  {count} 个过期的隔离条目（{size}）可被永久删除。",
        "gc.preview.run" => "运行 `modeld gc`（不带 --preview）以执行。",
        "gc.running" => "正在运行安全 GC…",
        "gc.nothing_quarantined" => "无需隔离 —— 存储已干净。",
        "gc.quarantined" => "  ✓ 已隔离 {count} 个模型（{size}）",
        "gc.skipped_hard" => "  • 跳过 {count} 个硬保护模型",
        "gc.skipped_soft" => "  ⚠ 跳过 {count} 个软保护模型（使用 --force 覆盖）",
        "gc.cleanup_header" => "正在清理过期的隔离条目…",
        "gc.cleanup_done" => "  ✓ 已永久删除 {count} 个过期隔离条目",
        "gc.cleanup_none" => "  未发现过期的隔离条目。",

        // ── proxy ──
        "proxy.listening" => "modeld 代理正在监听 {bind}（存储：{store}）",
        "proxy.shutdown" => "收到关闭请求，正在停止代理服务器",
        "proxy.discover.scanning" => "正在局域网内扫描 modeld 代理服务器…",
        "proxy.discover.service" => "  （mDNS 服务：_modeld._tcp.local.）",
        "proxy.discover.none" => "在本地网络上未找到 modeld 代理服务器。",
        "proxy.discover.make_sure" => "请确认：",
        "proxy.discover.tip_server" => "  • 已运行服务器：modeld proxy start",
        "proxy.discover.tip_mdns" => "  • mDNS 已发布（avahi-publish / dns-sd / Bonjour）",
        "proxy.discover.tip_subnet" => "  • 该机器在同一网络/子网内",
        "proxy.discover.tip_firewall" => "  • 已放行 mDNS 流量（UDP 5353）",
        "proxy.discover.found" => "✓ 找到 {count} 台服务器：",
        "proxy.discover.line" => "  • {url}",
        "proxy.status.header" => "modeld 代理状态",
        "proxy.status.url" => "  URL：      {url}",
        "proxy.status.status" => "  状态：     {value}",
        "proxy.status.version" => "  版本：     {value}",
        "proxy.status.uptime" => "  运行时长： {value} 秒",
        "proxy.status.models" => "  模型：     {count}",
        "proxy.status.total_size" => "  总大小：   {value}",
        "proxy.status.recent_header" => "最近的模型（显示前 5 个）：",
        "proxy.status.recent_line" => "  {hash}  {size}  {format}",
        "proxy.status.more" => "  ... 还有 {count} 个",
        "proxy.status.no_models" => "存储中没有模型。",
        "proxy.status.list_failed" => "  ⚠ list_models 失败：{error}",
        "proxy.status.reach_failed" => "无法连接 {url} 处的代理：{error}",

        // ── internal warnings ──
        "warn.gc_quarantine_failed" => "警告：隔离 {hash} 失败：{error}",
        "warn.dedup_link_failed" => "警告：为 {path} 创建链接失败：{error}",
        "warn.dedup_group_failed" => "警告：分组 {prefix} 的去重失败：{error}",
        "warn.recover_rolled_back" => "恢复：已回滚挂起的事务 {tx}",
        "warn.recover_move_failed" => "恢复：事务 {tx} 的暂存文件移至 CAS 失败：{error}",
        "warn.recover_phase_b_done" => "恢复：事务 {tx} 的阶段 B 已完成",
        "warn.recover_processed" => "恢复：已处理 {count} 个未完成的事务",
        "warn.quarantine_cleanup_failed" => "警告：清理过期隔离条目 {path} 失败：{error}",
        "warn.mdns_announce" => "modeld mDNS：服务 modeld.{stype} 在 :{port} 上 [TXT: {txt}]",
        "warn.mdns_announce_hint" => {
            "  （通过宿主 mDNS 守护进程发布：avahi-publish / dns-sd / Bonjour）"
        }

        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_lang_tag_handles_common_forms() {
        assert_eq!(parse_lang_tag("zh"), Some(Lang::Zh));
        assert_eq!(parse_lang_tag("zh-CN"), Some(Lang::Zh));
        assert_eq!(parse_lang_tag("zh_CN.UTF-8"), Some(Lang::Zh));
        assert_eq!(parse_lang_tag("Chinese"), Some(Lang::Zh));
        assert_eq!(parse_lang_tag("en"), Some(Lang::En));
        assert_eq!(parse_lang_tag("en-US"), Some(Lang::En));
        assert_eq!(parse_lang_tag("English"), Some(Lang::En));
        // Empty / neutral locales don't force a language.
        assert_eq!(parse_lang_tag(""), None);
        assert_eq!(parse_lang_tag("C"), None);
        assert_eq!(parse_lang_tag("POSIX"), None);
        // Unrelated locales don't force English (let lookup continue).
        assert_eq!(parse_lang_tag("fr_FR.UTF-8"), None);
    }

    #[test]
    fn fill_substitutes_named_placeholders() {
        let n = 42;
        let name = "model.safetensors";
        let out = fill("Found {count} files: {name}", &[("count", &n), ("name", &name)]);
        assert_eq!(out, "Found 42 files: model.safetensors");
    }

    #[test]
    fn fill_leaves_unknown_placeholders_intact() {
        let out = fill("Hello {who}", &[("name", &"x")]);
        assert_eq!(out, "Hello {who}");
    }

    #[test]
    fn fill_returns_template_unchanged_without_braces() {
        let out = fill("no placeholders", &[("a", &1)]);
        assert_eq!(out, "no placeholders");
    }

    #[test]
    fn en_table_returns_none_for_unknown() {
        assert_eq!(en("__nonexistent_key__"), None);
    }

    #[test]
    fn zh_table_falls_back_to_none_for_unknown() {
        assert_eq!(zh("__nonexistent_key__"), None);
    }

    #[test]
    fn t_surfaces_unknown_key_verbatim() {
        // An unknown key should be returned as-is (not panic, not empty).
        // Note: detect() may return En or Zh depending on the test host, but
        // either way the key is unknown in both tables.
        assert_eq!(t("__nonexistent_key_xyz__"), "__nonexistent_key_xyz__");
    }

    #[test]
    fn every_zh_key_exists_in_en() {
        // A key must resolve to a real English string (not None) for the
        // fallback in `t()` to be safe. We test by sampling a handful of
        // keys that appear in both tables.
        for key in [
            "init.success",
            "scan.found",
            "dedup.mode.dry_run",
            "proxy.listening",
            "gc.quarantined",
            "hf.dl.success",
            "warn.gc_quarantine_failed",
            "warn.dedup_link_failed",
            "warn.dedup_group_failed",
            "warn.recover_rolled_back",
            "warn.recover_move_failed",
            "warn.recover_phase_b_done",
            "warn.recover_processed",
            "warn.quarantine_cleanup_failed",
            "warn.mdns_announce",
            "warn.mdns_announce_hint",
        ] {
            assert!(en(key).is_some(), "EN missing for key {key}");
            assert!(zh(key).is_some(), "ZH missing for key {key}");
        }
    }
}
