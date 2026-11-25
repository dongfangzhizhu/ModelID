//! C2 验收：通过真实 `modeld audit` 二进制查询审计日志，
//! 覆盖「上周谁删了什么」的时间范围 + 操作类型过滤。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn temp_store(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("modeld-audit-cli-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn modeld(store: &PathBuf, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_modeld"))
        .args(["--store"])
        .arg(store)
        .args(args)
        .env_remove("MODELD_STORE")
        .output()
        .expect("run modeld")
}

fn seed(store: &Path) {
    use chrono::{Duration, Utc};
    let logger = modeld_core::AuditLogger::new(store);
    logger
        .log(&modeld_core::AuditEntry {
            timestamp: Utc::now() - Duration::days(2),
            actor_ip: "192.168.1.5".into(),
            ..modeld_core::AuditEntry::success("unlink", "/models/recent.safetensors")
        })
        .unwrap();
    logger
        .log(&modeld_core::AuditEntry {
            timestamp: Utc::now() - Duration::days(9),
            ..modeld_core::AuditEntry::success("unlink", "/models/old.safetensors")
        })
        .unwrap();
}

#[test]
fn audit_query_who_deleted_what_last_week() {
    let store = temp_store("week");
    seed(&store);

    let out = modeld(&store, &["audit", "--since", "7d", "--op", "unlink", "--json"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let body = String::from_utf8_lossy(&out.stdout);
    let rows: Vec<serde_json::Value> = serde_json::from_str(&body).expect("JSON array");
    assert_eq!(rows.len(), 1, "{body}");
    assert_eq!(rows[0]["actor_ip"], "192.168.1.5", "{body}");
    assert_eq!(rows[0]["resource"], "/models/recent.safetensors", "{body}");
}

#[test]
fn audit_query_rejects_bad_time() {
    let store = temp_store("badtime");
    let out = modeld(&store, &["audit", "--since", "yesterday"]);
    assert_eq!(out.status.code(), Some(1), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn audit_query_empty_store_prints_message() {
    let store = temp_store("empty");
    let out = modeld(&store, &["audit"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("没有匹配的审计记录"));
}
