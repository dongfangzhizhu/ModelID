//! Shared scan-result ingestion — used by both the CLI `scan_command` and the
//! WebUI `trigger_scan` handler so that both paths produce identical CAS and
//! Database state (audit Wave 1).

use crate::cas::CasStore;
use crate::db::{AliasType, Database, Frontend};
use crate::hash::hash_file;
use crate::scanner::ScannedFile;
use anyhow::Result;
use std::path::Path;

/// One file that failed ingestion (CAS or DB write error), including a
/// human-readable reason.
#[derive(Debug, Clone)]
pub struct IngestError {
    pub path: String,
    pub reason: String,
}

/// Aggregate result returned by [`ingest_scan_results`].
#[derive(Debug, Clone, Default)]
pub struct IngestResult {
    /// Number of files successfully written to CAS and DB.
    pub processed_count: usize,
    /// Total bytes of the files in `processed_count`.
    pub bytes_processed: u64,
    /// Files that failed CAS write, hash verification, or DB write.
    pub errors: Vec<IngestError>,
}

/// Store `files` into CAS (crash-safe) and record them in `db`.
///
/// This is the single code path used by both `modeld-cli::scan_command` and
/// `modeld-webui::api::scan::trigger_scan`. Any divergence between CLI and
/// WebUI scan behaviour must be fixed here, not in either caller.
///
/// Steps per file:
/// 1. Re-verify the file's actual BLAKE3 hash matches `file.hash` (defends
///    against a stale/incorrect hash reaching this function — Req 1.9).
/// 2. `CasStore::store_crash_safe(&file.path, &file.hash, scan_id)`.
/// 3. `db.insert_or_update_model`, `db.insert_alias` (Original/User,
///    idempotent — skipped if an alias already exists at that path),
///    `db.upsert_path_index`.
///
/// A failure on any single file is recorded in `IngestResult::errors` and
/// processing continues with the next file (Req 1.4) — the batch never
/// aborts early.
pub fn ingest_scan_results(
    store_path: &Path,
    db: &mut Database,
    files: &[ScannedFile],
    scan_id: &str,
) -> Result<IngestResult> {
    let cas = CasStore::new(store_path);
    let mut result = IngestResult::default();

    for file in files {
        // Req 1.9: reject files whose actual hash does not match the declared hash.
        // Cache hits (`from_cache = true`) are trusted since they were verified
        // on a previous scan; freshly-hashed files are re-verified here as a
        // cheap defence against caller bugs / bit-flips between scan and ingest.
        if !file.from_cache {
            match hash_file(&file.path) {
                Ok(actual) if actual.as_hex() == file.hash.as_hex() => {}
                Ok(actual) => {
                    result.errors.push(IngestError {
                        path: file.path.display().to_string(),
                        reason: format!(
                            "hash mismatch: declared {}, actual {}",
                            file.hash.as_hex(),
                            actual.as_hex()
                        ),
                    });
                    continue;
                }
                Err(e) => {
                    result.errors.push(IngestError {
                        path: file.path.display().to_string(),
                        reason: format!("re-hash failed: {e}"),
                    });
                    continue;
                }
            }
        }

        if let Err(e) = cas.store_crash_safe(&file.path, &file.hash, scan_id) {
            result.errors.push(IngestError {
                path: file.path.display().to_string(),
                reason: format!("CAS store failed: {e}"),
            });
            continue;
        }

        if let Err(e) =
            db.insert_or_update_model(&file.hash, file.size as i64, None, None, None, None)
        {
            result.errors.push(IngestError {
                path: file.path.display().to_string(),
                reason: format!("DB insert_or_update_model failed: {e}"),
            });
            continue;
        }

        let path_str = file.path.to_string_lossy().to_string();
        if db.get_alias_by_path(&path_str).ok().flatten().is_none() {
            let _ = db.insert_alias(&file.hash, &path_str, Frontend::User, AliasType::Original);
        }

        if let Err(e) = db.upsert_path_index(
            &path_str,
            &file.hash,
            file.size as i64,
            file.mtime,
            file.inode,
            file.device_id,
        ) {
            result.errors.push(IngestError {
                path: path_str,
                reason: format!("DB upsert_path_index failed: {e}"),
            });
            continue;
        }

        result.processed_count += 1;
        result.bytes_processed += file.size;
    }

    Ok(result)
}

// ─────────────────────────────────────────────────────────────────────────────
// Property-based tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::Blake3Hash;
    use proptest::prelude::*;
    use std::fs;
    use tempfile::TempDir;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20))]

        /// Property 1: Successful ingestion writes both CAS and DB records.
        ///
        /// For any non-empty batch of `ScannedFile` values whose declared
        /// BLAKE3 hash matches their actual file content, calling
        /// `ingest_scan_results` results in: (a) a CAS object at
        /// `path_for_hash(file.hash)` for every file, (b) a DB model record
        /// for every file's hash, (c) an alias at every file's path, and
        /// (d) a path_index entry at every file's path. It also asserts
        /// `processed_count` equals the number of input files with zero
        /// errors.
        ///
        /// **Validates: Requirements 1.2, 1.3, 1.8**
        #[test]
        fn prop_successful_batch_writes_cas_and_db(
            contents in prop::collection::vec(prop::collection::vec(any::<u8>(), 1..2048), 1..8)
        ) {
            let tmp = TempDir::new().unwrap();
            let models_dir = tmp.path().join("models");
            fs::create_dir_all(&models_dir).unwrap();

            let store_dir = tmp.path().join("store");
            let cas = CasStore::new(&store_dir);
            cas.init().unwrap();

            let db_path = store_dir.join("modeld.db");
            let mut db = Database::open(&db_path).unwrap();

            // Build a batch of valid ScannedFile entries: each file is written
            // to disk with real content, and its declared hash is the actual
            // BLAKE3 hash of that content (i.e. no simulated hash mismatch or
            // I/O failure).
            let mut files = Vec::new();
            for (i, content) in contents.iter().enumerate() {
                let path = models_dir.join(format!("model_{i}.safetensors"));
                fs::write(&path, content).unwrap();
                let hash = hash_file(&path).unwrap();
                files.push(ScannedFile {
                    path,
                    size: content.len() as u64,
                    hash,
                    from_cache: false,
                    mtime: 0,
                    inode: 0,
                    device_id: 0,
                });
            }

            let result =
                ingest_scan_results(&store_dir, &mut db, &files, "prop-test-scan-id").unwrap();

            // No errors, and every file counted as processed.
            prop_assert!(result.errors.is_empty());
            prop_assert_eq!(result.processed_count, files.len());
            let expected_bytes: u64 = files.iter().map(|f| f.size).sum();
            prop_assert_eq!(result.bytes_processed, expected_bytes);

            let indexed_paths = db.get_all_indexed_paths().unwrap();

            for file in &files {
                // (a) CAS object exists for the file's hash.
                prop_assert!(
                    cas.path_for_hash(&file.hash).exists(),
                    "expected CAS object for hash {}",
                    file.hash.as_hex()
                );

                // (b) DB model record exists for the file's hash.
                prop_assert!(
                    db.get_model(&file.hash).unwrap().is_some(),
                    "expected DB model record for hash {}",
                    file.hash.as_hex()
                );

                let path_str = file.path.to_string_lossy().to_string();

                // (c) Alias exists at the file's path.
                prop_assert!(
                    db.get_alias_by_path(&path_str).unwrap().is_some(),
                    "expected alias at path {}",
                    path_str
                );

                // (d) path_index entry exists at the file's path.
                prop_assert!(
                    indexed_paths.contains_key(&path_str),
                    "expected path_index entry at path {}",
                    path_str
                );
            }
        }

        /// Property 2: Partial batch failure does not abort remaining files.
        ///
        /// For any batch of `ScannedFile` values containing a mix of valid
        /// files and files that fail (here: a missing/unreadable source path,
        /// which fails the hash re-verification step since the file cannot be
        /// read), `ingest_scan_results` must still process every valid file
        /// to completion (CAS + DB written) and record every failing file in
        /// `errors`, regardless of the failing files' position in the batch.
        /// The function must return `Ok(IngestResult)` rather than propagating
        /// an error for such per-file failures.
        ///
        /// **Validates: Requirements 1.4**
        #[test]
        fn prop_partial_failure_does_not_abort_batch(
            // `None` entries simulate a file that fails ingestion (missing
            // source path on disk); `Some(bytes)` entries are valid files.
            // Interleaved order (mix of both, in arbitrary positions) is
            // exactly what this property is about.
            entries in prop::collection::vec(
                prop::option::of(prop::collection::vec(any::<u8>(), 1..1024)),
                2..10,
            )
                .prop_filter(
                    "need at least one valid and one failing entry",
                    |v| v.iter().any(|e| e.is_some()) && v.iter().any(|e| e.is_none()),
                )
        ) {
            let tmp = TempDir::new().unwrap();
            let models_dir = tmp.path().join("models");
            fs::create_dir_all(&models_dir).unwrap();

            let store_dir = tmp.path().join("store");
            let cas = CasStore::new(&store_dir);
            cas.init().unwrap();

            let db_path = store_dir.join("modeld.db");
            let mut db = Database::open(&db_path).unwrap();

            let mut files = Vec::new();
            // Track which files are expected to succeed vs fail, by index.
            let mut expect_valid: Vec<bool> = Vec::new();

            for (i, entry) in entries.iter().enumerate() {
                match entry {
                    Some(content) => {
                        let path = models_dir.join(format!("model_{i}.safetensors"));
                        fs::write(&path, content).unwrap();
                        let hash = hash_file(&path).unwrap();
                        files.push(ScannedFile {
                            path,
                            size: content.len() as u64,
                            hash,
                            from_cache: false,
                            mtime: 0,
                            inode: 0,
                            device_id: 0,
                        });
                        expect_valid.push(true);
                    }
                    None => {
                        // Simulate a missing/unreadable file: the path does
                        // NOT exist on disk, so the hash re-verification step
                        // in `ingest_scan_results` fails and the file must be
                        // recorded in `errors` without aborting the batch.
                        let path = models_dir.join(format!("missing_{i}.safetensors"));
                        // A syntactically valid but arbitrary declared hash —
                        // its correctness doesn't matter since the file can't
                        // even be read.
                        let hash = Blake3Hash::from_hex(&format!("{:064x}", i + 1)).unwrap();
                        files.push(ScannedFile {
                            path,
                            size: 1,
                            hash,
                            from_cache: false,
                            mtime: 0,
                            inode: 0,
                            device_id: 0,
                        });
                        expect_valid.push(false);
                    }
                }
            }

            let valid_count = expect_valid.iter().filter(|v| **v).count();
            let invalid_count = expect_valid.len() - valid_count;

            // The batch must still succeed at the outer `Result` level even
            // though some individual files fail.
            let result =
                ingest_scan_results(&store_dir, &mut db, &files, "prop-partial-fail-scan-id")
                    .expect("ingest_scan_results must return Ok even with per-file failures");

            prop_assert_eq!(result.processed_count, valid_count);
            prop_assert_eq!(result.errors.len(), invalid_count);

            let indexed_paths = db.get_all_indexed_paths().unwrap();

            for (file, was_valid) in files.iter().zip(expect_valid.iter()) {
                let path_str = file.path.to_string_lossy().to_string();
                if *was_valid {
                    // Every valid file must be fully processed regardless of
                    // where the failing files sit in the batch.
                    prop_assert!(
                        cas.path_for_hash(&file.hash).exists(),
                        "expected CAS object for valid file {}",
                        path_str
                    );
                    prop_assert!(
                        db.get_model(&file.hash).unwrap().is_some(),
                        "expected DB model record for valid file {}",
                        path_str
                    );
                    prop_assert!(
                        db.get_alias_by_path(&path_str).unwrap().is_some(),
                        "expected alias for valid file {}",
                        path_str
                    );
                    prop_assert!(
                        indexed_paths.contains_key(&path_str),
                        "expected path_index entry for valid file {}",
                        path_str
                    );
                    prop_assert!(
                        !result.errors.iter().any(|e| e.path == path_str),
                        "valid file {} must not appear in errors",
                        path_str
                    );
                } else {
                    // Failing files must not leave any CAS/DB side effects
                    // and must be recorded in `errors`.
                    prop_assert!(
                        !cas.path_for_hash(&file.hash).exists(),
                        "failing file {} must not produce a CAS object",
                        path_str
                    );
                    prop_assert!(
                        db.get_model(&file.hash).unwrap().is_none(),
                        "failing file {} must not produce a DB model record",
                        path_str
                    );
                    prop_assert!(
                        result.errors.iter().any(|e| e.path == path_str),
                        "failing file {} must be recorded in errors",
                        path_str
                    );
                }
            }
        }

        /// Property 3: Ingestion result counters are consistent with input.
        ///
        /// For any batch of `ScannedFile` values (arbitrary size, arbitrary
        /// mix of valid files and files that fail ingestion), every input
        /// file is accounted for exactly once: either as a processed file
        /// (`processed_count`) or as a recorded error (`errors`), with no
        /// file silently dropped or double-counted. Concretely:
        /// `processed_count + errors.len() == files.len()`, and
        /// `processed_count` equals the number of files with no entry in
        /// `errors` while `bytes_processed` equals the sum of `size` for
        /// exactly those successfully processed files.
        ///
        /// **Validates: Requirements 1.5**
        #[test]
        fn prop_result_counts_consistent_with_input(
            // `None` entries simulate a file that fails ingestion (missing
            // source path on disk); `Some(bytes)` entries are valid files.
            // Unlike Property 2's generator, no mix is required here — an
            // all-valid, all-failing, or empty batch must all satisfy the
            // counting invariant just as well as a mixed batch.
            entries in prop::collection::vec(
                prop::option::of(prop::collection::vec(any::<u8>(), 1..1024)),
                0..12,
            )
        ) {
            let tmp = TempDir::new().unwrap();
            let models_dir = tmp.path().join("models");
            fs::create_dir_all(&models_dir).unwrap();

            let store_dir = tmp.path().join("store");
            let cas = CasStore::new(&store_dir);
            cas.init().unwrap();

            let db_path = store_dir.join("modeld.db");
            let mut db = Database::open(&db_path).unwrap();

            let mut files = Vec::new();
            let mut expect_valid: Vec<bool> = Vec::new();

            for (i, entry) in entries.iter().enumerate() {
                match entry {
                    Some(content) => {
                        let path = models_dir.join(format!("model_{i}.safetensors"));
                        fs::write(&path, content).unwrap();
                        let hash = hash_file(&path).unwrap();
                        files.push(ScannedFile {
                            path,
                            size: content.len() as u64,
                            hash,
                            from_cache: false,
                            mtime: 0,
                            inode: 0,
                            device_id: 0,
                        });
                        expect_valid.push(true);
                    }
                    None => {
                        // Simulate a missing/unreadable file: the path does
                        // NOT exist on disk, so the hash re-verification step
                        // in `ingest_scan_results` fails.
                        let path = models_dir.join(format!("missing_{i}.safetensors"));
                        let hash = Blake3Hash::from_hex(&format!("{:064x}", i + 1)).unwrap();
                        files.push(ScannedFile {
                            path,
                            size: 1,
                            hash,
                            from_cache: false,
                            mtime: 0,
                            inode: 0,
                            device_id: 0,
                        });
                        expect_valid.push(false);
                    }
                }
            }

            let valid_count = expect_valid.iter().filter(|v| **v).count();
            let expected_bytes: u64 = files
                .iter()
                .zip(expect_valid.iter())
                .filter(|(_, valid)| **valid)
                .map(|(f, _)| f.size)
                .sum();

            let result =
                ingest_scan_results(&store_dir, &mut db, &files, "prop-count-consistency-scan-id")
                    .expect("ingest_scan_results must return Ok even with per-file failures");

            // Core invariant: every input file is accounted for exactly once,
            // either as processed or as an error — none silently dropped or
            // double-counted.
            prop_assert_eq!(result.processed_count + result.errors.len(), files.len());

            // Counters must also match the expected valid/invalid split and
            // byte total, not just sum to the right total.
            prop_assert_eq!(result.processed_count, valid_count);
            prop_assert_eq!(result.errors.len(), files.len() - valid_count);
            prop_assert_eq!(result.bytes_processed, expected_bytes);

            // No path appears in both "processed" (i.e. absent from errors)
            // and "errors" — each file's outcome is unambiguous.
            let error_paths: std::collections::HashSet<&str> =
                result.errors.iter().map(|e| e.path.as_str()).collect();
            prop_assert_eq!(error_paths.len(), result.errors.len(), "errors must not contain duplicate paths");
        }

        /// Property 4: Hash-mismatched files are rejected without side effects.
        ///
        /// Unlike Property 2's "missing/unreadable file" failure case, this
        /// property targets a file that DOES exist and IS readable, but whose
        /// declared `hash` does not match the actual BLAKE3 hash of its
        /// on-disk content — simulating tampering/corruption between scan
        /// and ingest (Req 1.9). For any such file mixed into an otherwise
        /// valid batch, `ingest_scan_results` must:
        /// - reject the file and record it in `errors` with its path and a
        ///   reason,
        /// - NOT create a CAS object at `path_for_hash` of either the
        ///   declared hash or the file's real content hash,
        /// - NOT create a DB model record for the declared hash,
        /// - still fully process every other valid file in the batch.
        ///
        /// **Validates: Requirements 1.9**
        #[test]
        fn prop_hash_mismatch_file_rejected_without_side_effects(
            valid_contents in prop::collection::vec(prop::collection::vec(any::<u8>(), 1..1024), 0..6),
            tampered_content in prop::collection::vec(any::<u8>(), 1..1024),
            decoy_content in prop::collection::vec(any::<u8>(), 1..1024),
            mismatch_position in 0usize..6,
        ) {
            // Ensure the "declared" hash (computed from decoy_content) is
            // actually different from the real on-disk content's hash.
            prop_assume!(tampered_content != decoy_content);

            let tmp = TempDir::new().unwrap();
            let models_dir = tmp.path().join("models");
            fs::create_dir_all(&models_dir).unwrap();

            let store_dir = tmp.path().join("store");
            let cas = CasStore::new(&store_dir);
            cas.init().unwrap();

            let db_path = store_dir.join("modeld.db");
            let mut db = Database::open(&db_path).unwrap();

            // The tampered file exists on disk with `tampered_content`, but
            // its declared `hash` field is the BLAKE3 hash of
            // `decoy_content` — a hash that does NOT match what's actually
            // on disk (simulating tampering/corruption between scan and
            // ingest).
            let tampered_path = models_dir.join("tampered_file.safetensors");
            fs::write(&tampered_path, &tampered_content).unwrap();
            let real_hash = hash_file(&tampered_path).unwrap();

            let decoy_path = models_dir.join("__decoy_source.bin");
            fs::write(&decoy_path, &decoy_content).unwrap();
            let declared_hash = hash_file(&decoy_path).unwrap();
            fs::remove_file(&decoy_path).unwrap();

            let tampered_file = ScannedFile {
                path: tampered_path.clone(),
                size: tampered_content.len() as u64,
                hash: declared_hash.clone(),
                from_cache: false,
                mtime: 0,
                inode: 0,
                device_id: 0,
            };

            // Build the batch: `valid_contents.len()` genuinely valid files,
            // plus the hash-mismatched file inserted at `mismatch_position`
            // (clamped into range) so the mismatch can land anywhere in the
            // batch, not just at the start or end.
            let mut files = Vec::new();
            let insert_at = mismatch_position.min(valid_contents.len());

            for (i, content) in valid_contents.iter().enumerate() {
                if i == insert_at {
                    files.push(tampered_file.clone());
                }
                let path = models_dir.join(format!("model_{i}.safetensors"));
                fs::write(&path, content).unwrap();
                let hash = hash_file(&path).unwrap();
                files.push(ScannedFile {
                    path,
                    size: content.len() as u64,
                    hash,
                    from_cache: false,
                    mtime: 0,
                    inode: 0,
                    device_id: 0,
                });
            }
            if insert_at == valid_contents.len() {
                files.push(tampered_file);
            }

            let valid_count = valid_contents.len();

            let result = ingest_scan_results(&store_dir, &mut db, &files, "prop-hash-mismatch-scan-id")
                .expect("ingest_scan_results must return Ok even with a hash-mismatched file");

            // Every genuinely valid file is still processed normally.
            prop_assert_eq!(result.processed_count, valid_count);
            prop_assert_eq!(result.errors.len(), 1);

            let tampered_path_str = tampered_path.to_string_lossy().to_string();

            // The mismatched file is recorded in errors with its path and a reason.
            let err = result
                .errors
                .iter()
                .find(|e| e.path == tampered_path_str)
                .expect("tampered file must be recorded in errors with its path");
            prop_assert!(!err.reason.is_empty(), "error reason must not be empty");

            // No CAS object exists for the file's real on-disk content hash,
            // nor for the declared (wrong) hash.
            prop_assert!(
                !cas.path_for_hash(&real_hash).exists(),
                "no CAS object must exist for the real content hash of the tampered file"
            );
            prop_assert!(
                !cas.path_for_hash(&declared_hash).exists(),
                "no CAS object must exist for the declared (mismatched) hash"
            );

            // No DB model record exists for either hash, and no alias/path_index
            // entry exists at the tampered file's path.
            prop_assert!(db.get_model(&real_hash).unwrap().is_none());
            prop_assert!(db.get_model(&declared_hash).unwrap().is_none());
            prop_assert!(db.get_alias_by_path(&tampered_path_str).unwrap().is_none());
            let indexed_paths = db.get_all_indexed_paths().unwrap();
            prop_assert!(!indexed_paths.contains_key(&tampered_path_str));

            // Every other valid file in the batch is still fully processed
            // (no interruption caused by the mismatched file).
            for file in files.iter().filter(|f| f.path != tampered_path) {
                let path_str = file.path.to_string_lossy().to_string();
                prop_assert!(
                    cas.path_for_hash(&file.hash).exists(),
                    "expected CAS object for valid file {}",
                    path_str
                );
                prop_assert!(
                    db.get_model(&file.hash).unwrap().is_some(),
                    "expected DB model record for valid file {}",
                    path_str
                );
                prop_assert!(
                    db.get_alias_by_path(&path_str).unwrap().is_some(),
                    "expected alias for valid file {}",
                    path_str
                );
                prop_assert!(
                    indexed_paths.contains_key(&path_str),
                    "expected path_index entry for valid file {}",
                    path_str
                );
            }
        }
    }
}
