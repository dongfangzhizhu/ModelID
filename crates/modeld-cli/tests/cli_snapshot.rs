//! Snapshot tests for the command-line surface.
//!
//! These are the B2 safety net: the CLI was split from a 4153-line `main.rs` into
//! a clap tree, a helper module and fourteen command modules, and the only thing
//! that makes that refactor safe is that the observable behaviour did not move.
//! Every assertion here is something a user or a script can depend on — the
//! command list, the documented exit codes, the shape of `--json`, and the exit
//! status for a bad command line.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A throwaway store directory, unique per test.
fn temp_store(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("modeld-cli-{}-{}", std::process::id(), tag));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp store");
    dir
}

fn modeld(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_modeld"))
        .args(args)
        // Keep the test away from the developer's real configuration.
        .env_remove("MODELD_STORE")
        .output()
        .expect("run modeld")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("exit code")
}

#[test]
fn help_lists_every_command() {
    let out = modeld(&["--help"]);
    assert_eq!(code(&out), 0);
    let text = stdout(&out);
    for command in [
        "scan", "dedup", "gc", "status", "stats", "list", "info", "verify", "serve", "doctor",
        "store", "config", "hf", "tx", "db", "tag", "note", "pin", "favorite", "refs",
    ] {
        assert!(text.contains(command), "help is missing `{command}`:\n{text}");
    }
}

#[test]
fn help_documents_the_exit_codes() {
    let text = stdout(&modeld(&["--help"]));
    for expected in [
        "0  success",
        "1  the operation ran and failed",
        "2  invalid command line",
        "3  the operation ran but some items failed",
    ] {
        assert!(text.contains(expected), "missing exit code line `{expected}`");
    }
}

#[test]
fn version_comes_from_the_package() {
    let out = modeld(&["--version"]);
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn store_flag_works_before_and_after_the_subcommand() {
    let store = temp_store("flag-pos");
    let after = stdout(&modeld(&["store", "locate", "--store", store.to_str().unwrap()]));
    let before = stdout(&modeld(&["--store", store.to_str().unwrap(), "store", "locate"]));
    assert_eq!(after.trim(), store.to_str().unwrap());
    assert_eq!(before.trim(), after.trim());
}

#[test]
fn an_unknown_subcommand_is_a_usage_error() {
    let out = modeld(&["definitely-not-a-command"]);
    assert_eq!(code(&out), 2, "clap must exit 2 for a bad command line");
    assert!(!stderr(&out).is_empty());
}

#[test]
fn tx_list_json_is_pure_json() {
    let store = temp_store("tx-json");
    let init = modeld(&["store", "init", "--store", store.to_str().unwrap()]);
    assert_eq!(code(&init), 0, "init failed: {}", stderr(&init));

    let out = modeld(&["tx", "list", "--json", "--store", store.to_str().unwrap()]);
    assert_eq!(code(&out), 0);
    let text = stdout(&out).trim().to_string();
    // A fresh store has no transactions: the only thing on stdout may be the
    // empty array, never a banner or a progress line.
    assert_eq!(text, "[]", "stdout was not pure JSON: {text:?}");
}

#[test]
fn every_json_command_emits_parseable_json() {
    let store = temp_store("json-purity");
    assert_eq!(code(&modeld(&["store", "init", "--store", store.to_str().unwrap()])), 0);
    let dir = store.to_str().unwrap().to_string();

    // Each of these has a --json flag; with it set, stdout must parse as JSON
    // and nothing else may be mixed in.
    let cases: &[&[&str]] = &[
        &["tx", "list", "--json"],
        &["pin", "list", "--json"],
        &["refs", "orphans", "--json"],
        &["refs", "graph", "--json"],
        &["verify", "--json"],
        &["dupes", "--json"],
        &["list", "--json"],
    ];
    for case in cases {
        let mut args: Vec<&str> = case.to_vec();
        args.push("--store");
        args.push(&dir);
        let out = modeld(&args);
        assert_eq!(code(&out), 0, "{case:?} failed: {}", stderr(&out));
        let text = stdout(&out);
        serde_json::from_str::<serde_json::Value>(&text)
            .unwrap_or_else(|e| panic!("{case:?} stdout is not JSON ({e}): {text:?}"));
    }
}

#[test]
fn config_list_reports_the_core_keys() {
    let store = temp_store("config-list");
    assert_eq!(code(&modeld(&["store", "init", "--store", store.to_str().unwrap()])), 0);
    let text = stdout(&modeld(&["config", "list", "--store", store.to_str().unwrap()]));
    for key in ["store.path", "serve.port", "scan.extensions"] {
        assert!(text.contains(key), "config list is missing `{key}`:\n{text}");
    }
}

#[test]
fn verify_on_a_fresh_store_is_clean() {
    let store = temp_store("verify");
    assert_eq!(code(&modeld(&["store", "init", "--store", store.to_str().unwrap()])), 0);
    let out = modeld(&["verify", "--store", store.to_str().unwrap()]);
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).contains("verify.clean") || stdout(&out).contains("✓"));
}

#[test]
fn status_needs_an_initialised_store() {
    let store = temp_store("status-missing");
    let out = modeld(&["status", "--store", store.to_str().unwrap()]);
    assert_ne!(code(&out), 0, "status must fail on a store without a database");
    assert!(!stderr(&out).is_empty());
}

#[test]
fn a_store_path_is_not_silently_created_by_read_commands() {
    let store = temp_store("no-create");
    let target = store.join("does-not-exist");
    let out = modeld(&["status", "--store", target.to_str().unwrap()]);
    assert_ne!(code(&out), 0);
    assert!(!Path::new(&target).join("modeld.db").exists());
}
