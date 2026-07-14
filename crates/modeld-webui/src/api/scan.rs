//! Scan API handler — POST /api/v1/scan

use axum::{extract::State, response::Json};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::error::ApiResult;
use crate::state::{AppState, WsEvent};

/// Request body for `POST /api/v1/scan`.
///
/// Both the legacy single-path format (`path`) and the new multi-path format
/// (`paths`) are accepted for backward compatibility.
#[derive(Deserialize, Default)]
pub struct ScanRequest {
    /// A single directory to scan (legacy field).
    pub path: Option<String>,
    /// One or more directories to scan (new canonical field).
    pub paths: Option<Vec<String>>,
    pub force_rehash: Option<bool>,
    /// Follow symbolic links when traversing (default: false)
    pub follow_symlinks: Option<bool>,
    /// Glob patterns to exclude
    pub exclude_globs: Option<Vec<String>>,
}

#[derive(Serialize)]
pub struct ScanResponse {
    pub scan_id: String,
    pub message: String,
    /// Paths that will be scanned (resolved from request).
    pub scan_paths: Vec<String>,
}

pub async fn trigger_scan(
    State(state): State<AppState>,
    body: Option<Json<ScanRequest>>,
) -> ApiResult<Json<ScanResponse>> {
    let req = body.map(|b| b.0).unwrap_or_default();
    let scan_id = uuid::Uuid::new_v4().to_string();

    // Resolve scan paths: prefer `paths`, fall back to `path`, then store root.
    let mut scan_paths: Vec<std::path::PathBuf> = req
        .paths
        .as_deref()
        .map(|ps| ps.iter().map(std::path::PathBuf::from).collect())
        .or_else(|| req.path.as_deref().map(|p| vec![std::path::PathBuf::from(p)]))
        .unwrap_or_else(|| vec![state.store_path.clone()]);

    // De-duplicate
    scan_paths.dedup();

    let scan_paths_display: Vec<String> =
        scan_paths.iter().map(|p| p.display().to_string()).collect();

    let db_arc = state.db.clone();
    let store_path = state.store_path.clone();
    let event_tx = state.event_tx.clone();
    let sid = scan_id.clone();
    let force_rehash = req.force_rehash.unwrap_or(false);
    let follow_symlinks = req.follow_symlinks.unwrap_or(false);
    let exclude_globs = req.exclude_globs.unwrap_or_default();

    tokio::spawn(async move {
        let mut any_error = false;

        for scan_root in scan_paths {
            let root_display = scan_root.display().to_string();

            // Phase: walking — directory traversal is about to begin.
            let _ = event_tx.send(WsEvent::ScanProgress {
                scan_id: sid.clone(),
                phase: "walking".to_string(),
                current_path: Some(root_display.clone()),
                files_scanned: 0,
                files_total: 0,
                bytes_scanned: 0,
                bytes_total: 0,
            });

            let db_arc2 = db_arc.clone();
            let store_path2 = store_path.clone();
            let event_tx2 = event_tx.clone();
            let scan_root2 = scan_root.clone();
            let excl2 = exclude_globs.clone();
            let sid2 = sid.clone(); // moved into spawn_blocking; `sid` itself is reused across loop iterations

            let result = tokio::task::spawn_blocking(
                move || -> anyhow::Result<(modeld_core::IngestResult, WsEvent)> {
                    // Load preindexed cache for incremental scan
                    let preindexed = {
                        let db = db_arc2.lock().map_err(|_| anyhow::anyhow!("lock poisoned"))?;
                        db.get_all_indexed_paths().unwrap_or_default()
                    };

                    let store_canonical =
                        std::fs::canonicalize(&store_path2).unwrap_or(store_path2.clone());

                    let scan_opts = modeld_core::ScanOptions {
                        incremental: !force_rehash,
                        full: force_rehash,
                        follow_symlinks,
                        exclude_globs: excl2,
                    };

                    let scanner = modeld_core::Scanner::new()
                        .with_excluded_dirs(vec![store_canonical])
                        .with_preindexed(preindexed)
                        .with_scan_options(scan_opts);

                    // Count files first (still part of the walking phase) so that
                    // the hashing phase can report accurate totals.
                    let (total_files, total_bytes) =
                        scanner.count_files(&scan_root2).unwrap_or((0, 0));
                    let total = total_files as u64;

                    // Phase: hashing — traversal/counting complete, per-file BLAKE3
                    // hashing is about to start.
                    let _ = event_tx2.send(WsEvent::ScanProgress {
                        scan_id: sid2.clone(),
                        phase: "hashing".to_string(),
                        current_path: None,
                        files_scanned: 0,
                        files_total: total,
                        bytes_scanned: 0,
                        bytes_total: total_bytes,
                    });

                    let done_counter = Arc::new(AtomicU64::new(0));
                    let bytes_counter = Arc::new(AtomicU64::new(0));
                    let done_arc = done_counter.clone();
                    let bytes_arc = bytes_counter.clone();
                    let etx = event_tx2.clone();
                    let sid3 = sid2.clone();

                    let scanned = scanner.scan(&scan_root2, move |path, size| {
                        let done = done_arc.fetch_add(1, Ordering::Relaxed) + 1;
                        let bytes_done = bytes_arc.fetch_add(size, Ordering::Relaxed) + size;
                        let _ = etx.send(WsEvent::ScanProgress {
                            scan_id: sid3.clone(),
                            phase: "hashing".to_string(),
                            current_path: Some(path.display().to_string()),
                            files_scanned: done,
                            files_total: total,
                            bytes_scanned: bytes_done,
                            bytes_total: total_bytes,
                        });
                    })?;

                    // Read the final counter values after scan completes —  these
                    // reflect the number of files actually processed (for which
                    // the progress callback was invoked), which may be less than
                    // `total` if some files failed directory traversal/hashing.
                    let final_files_scanned = done_counter.load(Ordering::Relaxed);
                    let final_bytes_scanned = bytes_counter.load(Ordering::Relaxed);

                    let _scanned_count = scanned.len() as u64;
                    let _scanned_bytes: u64 = scanned.iter().map(|f| f.size).sum();

                    // Phase: indexing — about to write scan results to CAS/DB via
                    // the shared ingestion function so that CLI scans and WebUI
                    // scans produce identical CAS/DB state (audit Wave 1).
                    let indexing_event = WsEvent::ScanProgress {
                        scan_id: sid2.clone(),
                        phase: "indexing".to_string(),
                        current_path: None,
                        files_scanned: final_files_scanned,
                        files_total: total,
                        bytes_scanned: final_bytes_scanned,
                        bytes_total: total_bytes,
                    };
                    let _ = event_tx2.send(indexing_event.clone());

                    // `sid2` is the scan_id generated by `trigger_scan` and is
                    // passed through to `CasStore::store_crash_safe` as the tx_id
                    // (audit Wave 5.5).
                    let mut db = db_arc2.lock().map_err(|_| anyhow::anyhow!("lock poisoned"))?;
                    let ingest_result =
                        modeld_core::ingest_scan_results(&store_path2, &mut db, &scanned, &sid2)?;
                    Ok((ingest_result, indexing_event))
                },
            )
            .await;

            // Phase: done (success) or error (fatal scan/ingestion failure).
            match result {
                Ok(Ok((_ingest_result, indexing_event))) => {
                    // Clone the indexing event's counters to ensure perfect
                    // monotonicity (done phase uses the same counts as
                    // indexing phase - Requirement 3.3).
                    let done_event = if let WsEvent::ScanProgress {
                        files_scanned,
                        files_total,
                        bytes_scanned,
                        bytes_total,
                        ..
                    } = indexing_event
                    {
                        WsEvent::ScanProgress {
                            scan_id: sid.clone(),
                            phase: "done".to_string(),
                            current_path: None,
                            files_scanned,
                            files_total,
                            bytes_scanned,
                            bytes_total,
                        }
                    } else {
                        unreachable!("indexing_event must be ScanProgress")
                    };
                    let _ = event_tx.send(done_event);
                }
                Ok(Err(e)) => {
                    any_error = true;
                    let _ = event_tx.send(WsEvent::ScanProgress {
                        scan_id: sid.clone(),
                        phase: "error".to_string(),
                        current_path: Some(root_display.clone()),
                        files_scanned: 0,
                        files_total: 0,
                        bytes_scanned: 0,
                        bytes_total: 0,
                    });
                    let _ = event_tx.send(WsEvent::Error {
                        message: format!("scan failed for {}: {}", root_display, e),
                    });
                }
                Err(join_err) => {
                    any_error = true;
                    let _ = event_tx.send(WsEvent::ScanProgress {
                        scan_id: sid.clone(),
                        phase: "error".to_string(),
                        current_path: Some(root_display.clone()),
                        files_scanned: 0,
                        files_total: 0,
                        bytes_scanned: 0,
                        bytes_total: 0,
                    });
                    let _ = event_tx.send(WsEvent::Error {
                        message: format!("scan task panicked for {}: {}", root_display, join_err),
                    });
                }
            }
        }

        // Signal all-paths complete
        let _ = event_tx.send(WsEvent::OperationComplete {
            operation: format!("scan:{}", sid),
            success: !any_error,
            message: if any_error {
                "Scan completed with errors".to_string()
            } else {
                "Scan complete".to_string()
            },
        });
    });

    Ok(Json(ScanResponse {
        scan_id,
        message: "Scan started".to_string(),
        scan_paths: scan_paths_display,
    }))
}

// ─────────────────────────────────────────────────────────────────────────────
// Property-based tests (audit Wave 3, Property 6)
// ─────────────────────────────────────────────────────────────────────────────
//
// `trigger_scan` is an axum handler that spawns its actual work onto
// `tokio::spawn` and reports progress purely through the `event_tx`
// broadcast channel on `AppState` — the HTTP response only carries the
// `scan_id`. To property-test the emitted event *sequence* we therefore:
//   1. build a real (but temp-dir-backed) `AppState`,
//   2. subscribe to `state.event_tx` *before* invoking the handler,
//   3. await the handler call (which returns immediately after spawning),
//   4. drain the broadcast receiver — with a timeout — until the terminal
//      `WsEvent::OperationComplete` for this scan is observed,
//   5. assert the ordering/consistency invariants over the collected
//      `ScanProgress` events.
//
// This exercises the handler's real async/spawn_blocking code path (no
// logic extraction / mocking), which is the most faithful way to test an
// axum handler that reports progress via a side channel rather than its
// return value.
#[cfg(test)]
mod property_tests {
    use super::*;
    use crate::state::AppState;
    use proptest::prelude::*;
    use std::time::Duration;
    use tempfile::TempDir;

    /// Numeric ordinal for each `ScanProgress` phase, used to assert that the
    /// sequence of phases observed for a single scan never regresses to an
    /// earlier phase (Requirements 3.2-3.6: walking -> hashing -> indexing ->
    /// done/error, no going back).
    fn phase_ordinal(phase: &str) -> u8 {
        match phase {
            "walking" => 0,
            "hashing" => 1,
            "indexing" => 2,
            "done" | "error" => 3,
            other => panic!("unexpected ScanProgress phase: {other}"),
        }
    }

    /// Run `trigger_scan` against a fresh temp-dir-backed `AppState` scanning
    /// `scan_root`, and collect every `WsEvent` broadcast for this scan up to
    /// (and including) the terminating `OperationComplete` event.
    ///
    /// Returns `(scan_id, events)`.
    fn run_scan_and_collect_events(scan_root: &std::path::Path) -> (String, Vec<WsEvent>) {
        let rt = tokio::runtime::Runtime::new().expect("build tokio runtime");

        rt.block_on(async move {
            let store_dir = TempDir::new().expect("create store temp dir");
            let db_path = store_dir.path().join("modeld.db");
            let db =
                modeld_core::Database::open(&db_path).expect("open test database");

            let (state, _initial_rx) = AppState::new(db, store_dir.path().to_path_buf(), None, None);

            // Subscribe BEFORE invoking the handler so we don't miss the
            // very first "walking" event, which is sent synchronously inside
            // the spawned task shortly after the handler returns.
            let mut rx = state.event_tx.subscribe();

            let req = ScanRequest {
                path: Some(scan_root.display().to_string()),
                paths: None,
                force_rehash: Some(true),
                follow_symlinks: None,
                exclude_globs: None,
            };

            let Json(resp) = trigger_scan(State(state.clone()), Some(Json(req)))
                .await
                .expect("trigger_scan handler must succeed");
            let scan_id = resp.scan_id;

            let mut events = Vec::new();
            loop {
                let ev = tokio::time::timeout(Duration::from_secs(30), rx.recv())
                    .await
                    .expect("timed out waiting for a scan event")
                    .expect("event_tx channel closed unexpectedly");

                let is_terminal = matches!(&ev, WsEvent::OperationComplete { operation, .. } if operation == &format!("scan:{scan_id}"));
                events.push(ev);
                if is_terminal {
                    break;
                }
            }

            (scan_id, events)
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20))]

        /// Property 6: Scan lifecycle emits phases in valid order with
        /// consistent counters.
        ///
        /// For any scan of a directory containing N model files (arbitrary
        /// count and sizes), the sequence of `WsEvent::ScanProgress` events
        /// broadcast during the scan:
        /// 1. Starts with "walking" and ends (for this success path) with
        ///    "done", with phase values transitioning only in the order
        ///    walking -> hashing -> indexing -> done (never regressing to an
        ///    earlier phase).
        /// 2. Every event for this scan shares the same `scan_id`.
        /// 3. `files_scanned`/`bytes_scanned` are monotonically
        ///    non-decreasing across the sequence and never exceed
        ///    `files_total`/`bytes_total` once the total is known
        ///    (non-zero).
        /// 4. The final "done" event has `files_scanned == files_total == N`
        ///    and `bytes_scanned == bytes_total` equal to the sum of the
        ///    scanned files' sizes.
        ///
        /// **Validates: Requirements 3.2, 3.3, 3.4, 3.5, 3.6**
        #[test]
        fn prop_scan_lifecycle_emits_phases_in_order_with_consistent_counters(
            file_sizes in prop::collection::vec(1usize..4096, 0..6)
        ) {
            let scan_root = TempDir::new().unwrap();

            let mut total_bytes: u64 = 0;
            for (i, size) in file_sizes.iter().enumerate() {
                let path = scan_root.path().join(format!("model_{i}.safetensors"));
                let content = vec![(i as u8).wrapping_add(1); *size];
                std::fs::write(&path, &content).unwrap();
                total_bytes += *size as u64;
            }
            let expected_files = file_sizes.len() as u64;

            let (scan_id, events) = run_scan_and_collect_events(scan_root.path());

            // Isolate ScanProgress events for this scan (OperationComplete /
            // Error events, if any, are inspected separately below).
            let scan_progress_events: Vec<_> = events
                .iter()
                .filter_map(|e| match e {
                    WsEvent::ScanProgress {
                        scan_id: sid,
                        phase,
                        files_scanned,
                        files_total,
                        bytes_scanned,
                        bytes_total,
                        ..
                    } => Some((sid.clone(), phase.clone(), *files_scanned, *files_total, *bytes_scanned, *bytes_total)),
                    _ => None,
                })
                .collect();

            prop_assert!(!scan_progress_events.is_empty(), "expected at least one ScanProgress event");

            // (1) starts with "walking".
            prop_assert_eq!(scan_progress_events[0].1.as_str(), "walking");

            // (1) ends with "done" on this success path (all input files are
            // valid, readable, freshly-written — no ingestion errors
            // expected).
            let last = scan_progress_events.last().unwrap();
            prop_assert_eq!(last.1.as_str(), "done");

            // (2) every ScanProgress event shares this scan's scan_id.
            for (sid, _, _, _, _, _) in &scan_progress_events {
                prop_assert_eq!(sid, &scan_id);
            }

            // (1) phase never regresses to an earlier phase, and
            // (3) files_scanned/bytes_scanned are monotonically
            // non-decreasing and never exceed the total once it is known
            // (i.e. once files_total/bytes_total is non-zero, or once N==0
            // in which case totals of 0 trivially bound scanned counts of 0).
            let mut prev_ordinal = phase_ordinal(&scan_progress_events[0].1);
            let mut prev_files_scanned = 0u64;
            let mut prev_bytes_scanned = 0u64;
            for (_, phase, files_scanned, files_total, bytes_scanned, bytes_total) in &scan_progress_events {
                let ordinal = phase_ordinal(phase);
                prop_assert!(
                    ordinal >= prev_ordinal,
                    "phase regressed: {} (ordinal {}) came after ordinal {}",
                    phase, ordinal, prev_ordinal
                );
                prev_ordinal = ordinal;

                prop_assert!(
                    *files_scanned >= prev_files_scanned,
                    "files_scanned decreased: {} < {}", files_scanned, prev_files_scanned
                );
                prop_assert!(
                    *bytes_scanned >= prev_bytes_scanned,
                    "bytes_scanned decreased: {} < {}", bytes_scanned, prev_bytes_scanned
                );
                prev_files_scanned = *files_scanned;
                prev_bytes_scanned = *bytes_scanned;

                // Totals are only meaningful once known; a total of 0 with a
                // scanned count of 0 is consistent (N == 0 case).
                prop_assert!(
                    *files_scanned <= *files_total,
                    "files_scanned {} exceeds files_total {} in phase {}", files_scanned, files_total, phase
                );
                prop_assert!(
                    *bytes_scanned <= *bytes_total,
                    "bytes_scanned {} exceeds bytes_total {} in phase {}", bytes_scanned, bytes_total, phase
                );
            }

            // (4) the final "done" event has files_scanned == files_total == N
            // and bytes_scanned == bytes_total == sum of scanned file sizes.
            let (_, _, done_scanned, done_total, done_bytes_scanned, done_bytes_total) = last;
            prop_assert_eq!(*done_scanned, expected_files);
            prop_assert_eq!(*done_total, expected_files);
            prop_assert_eq!(*done_bytes_scanned, total_bytes);
            prop_assert_eq!(*done_bytes_total, total_bytes);
        }
    }
}
