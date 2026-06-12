//! Integration tests for modeld-core
//!
//! Tests the complete workflow: scan → store → retrieve → dedup → quarantine

use modeld_core::{
    db::{AliasType, Frontend},
    CasStore, Database, DedupEngine, DedupMode, QuarantineManager, Scanner,
};
use std::fs;
use tempfile::TempDir;

#[test]
fn test_complete_workflow() {
    // Setup: Create test directory with model files
    let test_dir = TempDir::new().unwrap();
    let model_dir = test_dir.path().join("models");
    fs::create_dir(&model_dir).unwrap();

    // Create test model files
    let model1 = model_dir.join("sdxl_base.safetensors");
    let model2 = model_dir.join("lora_style.safetensors");
    let model3 = model_dir.join("readme.txt"); // Should be ignored

    fs::write(&model1, b"fake model data 1").unwrap();
    fs::write(&model2, b"fake model data 2").unwrap();
    fs::write(&model3, b"not a model").unwrap();

    // Initialize storage
    let store_dir = test_dir.path().join("store");
    let cas = CasStore::new(&store_dir);
    cas.init().unwrap();

    let db_path = store_dir.join("modeld.db");
    let mut db = Database::open(&db_path).unwrap();

    // Step 1: Scan directory
    let scanner = Scanner::new();
    let results = scanner.scan(&model_dir, |_, _| {}).unwrap();

    // Verify scan found 2 model files (not readme.txt)
    assert_eq!(results.len(), 2);

    // Step 2: Store files in CAS and database
    for file in &results {
        // Store in CAS
        let cas_path = cas.store(&file.path, &file.hash).unwrap();
        assert!(cas_path.exists());

        // Verify file is read-only
        let metadata = fs::metadata(&cas_path).unwrap();
        assert!(metadata.permissions().readonly());

        // Record in database
        db.insert_or_update_model(
            &file.hash,
            file.size as i64,
            Some("safetensors"),
            None,
            None,
            None,
        )
        .unwrap();
    }

    // Step 3: Verify database state
    let total_models = db.count_models().unwrap();
    assert_eq!(total_models, 2);

    let total_size = db.total_size().unwrap();
    assert_eq!(total_size, 17 + 17); // Both files have same content length

    // Step 4: Retrieve models from CAS
    for file in &results {
        let cas_path = cas.get(&file.hash).unwrap();
        assert!(cas_path.exists());

        // Verify file can be read
        let content = fs::read_to_string(&cas_path).unwrap();
        assert!(content.starts_with("fake model data"));

        // Verify model in database
        let model = db.get_model(&file.hash).unwrap().unwrap();
        assert_eq!(model.blake3_hash.as_hex(), file.hash.as_hex());
        assert_eq!(model.format.as_deref(), Some("safetensors"));
    }
}

#[test]
fn test_duplicate_handling() {
    let test_dir = TempDir::new().unwrap();
    let store_dir = test_dir.path().join("store");

    // Create two files with identical content in same directory
    let models_dir = test_dir.path().join("models");
    fs::create_dir(&models_dir).unwrap();

    let file1 = models_dir.join("model1.safetensors");
    let file2 = models_dir.join("model2.safetensors");

    let content = b"identical model content";
    fs::write(&file1, content).unwrap();
    fs::write(&file2, content).unwrap();

    // Initialize
    let cas = CasStore::new(&store_dir);
    cas.init().unwrap();

    let db_path = store_dir.join("modeld.db");
    let mut db = Database::open(&db_path).unwrap();

    // Scan directory once
    let scanner = Scanner::new();
    let results = scanner.scan(&models_dir, |_, _| {}).unwrap();

    // Should find 2 files
    assert_eq!(results.len(), 2);

    // Both files should have the same hash (same content)
    assert_eq!(results[0].hash.as_hex(), results[1].hash.as_hex());

    // Store both (second should be no-op in CAS)
    for file in &results {
        cas.store(&file.path, &file.hash).unwrap();
        db.insert_or_update_model(
            &file.hash,
            file.size as i64,
            Some("safetensors"),
            None,
            None,
            None,
        )
        .unwrap();
    }

    // Only one model in database (same hash, updated by second insert)
    let total_models = db.count_models().unwrap();
    assert_eq!(total_models, 1);

    // Verify CAS contains only one copy
    let hash = &results[0].hash;
    let cas_path = cas.get(hash).unwrap();
    assert!(cas_path.exists());

    // Verify content is correct
    let stored_content = fs::read(&cas_path).unwrap();
    assert_eq!(stored_content, content);
}

#[test]
fn test_scan_nested_directories() {
    let test_dir = TempDir::new().unwrap();
    let root = test_dir.path().join("models");

    // Create nested structure
    fs::create_dir_all(root.join("checkpoints")).unwrap();
    fs::create_dir_all(root.join("loras")).unwrap();
    fs::create_dir_all(root.join("vae")).unwrap();

    // Create model files at different levels
    fs::write(root.join("root.safetensors"), b"root model").unwrap();
    fs::write(root.join("checkpoints/sdxl.safetensors"), b"checkpoint").unwrap();
    fs::write(root.join("loras/style.safetensors"), b"lora").unwrap();
    fs::write(root.join("vae/vae.safetensors"), b"vae model").unwrap();

    // Scan recursively
    let scanner = Scanner::new();
    let results = scanner.scan(&root, |_, _| {}).unwrap();

    // Should find all 4 model files
    assert_eq!(results.len(), 4);

    // Verify all files were discovered
    let paths: Vec<_> = results.iter().map(|r| r.path.to_str().unwrap()).collect();
    assert!(paths.iter().any(|p| p.contains("root.safetensors")));
    assert!(paths.iter().any(|p| p.contains("sdxl.safetensors")));
    assert!(paths.iter().any(|p| p.contains("style.safetensors")));
    assert!(paths.iter().any(|p| p.contains("vae.safetensors")));
}

#[test]
fn test_database_persistence() {
    let test_dir = TempDir::new().unwrap();
    let db_path = test_dir.path().join("test.db");

    // Create and populate database
    {
        let mut db = Database::open(&db_path).unwrap();

        let hash = modeld_core::Blake3Hash::from_hex(
            "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
        )
        .unwrap();

        db.insert_or_update_model(
            &hash,
            1024,
            Some("safetensors"),
            Some("sdxl"),
            Some("checkpoint"),
            None,
        )
        .unwrap();
    }

    // Reopen database and verify data persisted
    {
        let db = Database::open(&db_path).unwrap();

        let count = db.count_models().unwrap();
        assert_eq!(count, 1);

        let hash = modeld_core::Blake3Hash::from_hex(
            "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
        )
        .unwrap();

        let model = db.get_model(&hash).unwrap().unwrap();
        assert_eq!(model.format.as_deref(), Some("safetensors"));
        assert_eq!(model.arch.as_deref(), Some("sdxl"));
        assert_eq!(model.category.as_deref(), Some("checkpoint"));
    }
}

/// End-to-end dedup test: create duplicates, scan, dedup, verify links
#[test]
fn test_e2e_dedup_workflow() {
    let test_dir = TempDir::new().unwrap();
    let store_dir = test_dir.path().join("store");
    let models_dir = test_dir.path().join("models");
    fs::create_dir_all(&models_dir).unwrap();

    // Create duplicate model files with same content
    let content = b"this is a fake model file with substantial content for testing 1234567890";
    let file1 = models_dir.join("model_copy1.safetensors");
    let file2 = models_dir.join("model_copy2.safetensors");
    let file3 = models_dir.join("unique_model.safetensors");
    let unique_content = b"this is a completely different model 9876543210";

    fs::write(&file1, content).unwrap();
    fs::write(&file2, content).unwrap();
    fs::write(&file3, unique_content).unwrap();

    // Initialize store
    let cas = CasStore::new(&store_dir);
    cas.init().unwrap();

    let db_path = store_dir.join("modeld.db");
    let mut db = Database::open(&db_path).unwrap();

    // Scan and store
    let scanner = Scanner::new();
    let results = scanner.scan(&models_dir, |_, _| {}).unwrap();
    assert_eq!(results.len(), 3);

    for file in &results {
        cas.store(&file.path, &file.hash).unwrap();
        db.insert_or_update_model(&file.hash, file.size as i64, None, None, None, None)
            .unwrap();
        let path_str = file.path.to_string_lossy().to_string();
        if db.get_alias_by_path(&path_str).unwrap().is_none() {
            db.insert_alias(&file.hash, &path_str, Frontend::User, AliasType::Original)
                .unwrap();
        }
    }

    // Find duplicates
    let db2 = Database::open(&db_path).unwrap();
    let mut engine = DedupEngine::new(db2, store_dir.clone());
    let groups = engine.find_duplicates().unwrap();

    // Should find 1 duplicate group (file1 and file2)
    assert_eq!(groups.len(), 1, "Should find exactly 1 duplicate group");
    assert_eq!(groups[0].files.len(), 2, "Duplicate group should have 2 files");

    // Calculate savings
    let savings = engine.calculate_savings(&groups);
    assert_eq!(savings, content.len() as u64, "Savings should equal one copy size");

    // Execute dedup in dry-run first
    let dry_result = engine
        .execute_dedup_group(&groups[0], DedupMode::DryRun)
        .unwrap();
    assert_eq!(dry_result.links_created.len(), 0, "Dry run should not create links");

    // Execute actual dedup
    let result = engine
        .execute_dedup_group(&groups[0], DedupMode::Auto)
        .unwrap();
    assert!(
        !result.links_created.is_empty(),
        "Should have created at least one link"
    );

    // Verify CAS file exists
    let cas_path = engine.cas_path_for_hash(&groups[0].hash);
    assert!(cas_path.exists(), "CAS file should exist after dedup");
}

/// Test crash recovery: simulate a pending WAL transaction
#[test]
fn test_crash_recovery_pending() {
    use modeld_core::db::TransactionStatus;

    let test_dir = TempDir::new().unwrap();
    let store_dir = test_dir.path().join("store");

    let db_path = store_dir.join("modeld.db");
    fs::create_dir_all(&store_dir).unwrap();

    // Create staging file to simulate interrupted Phase A
    let staging_dir = store_dir.join("tmp").join("cas_staging");
    fs::create_dir_all(&staging_dir).unwrap();
    let fake_hash = "aaaa111111111111111111111111111111111111111111111111111111111111";
    let staging_file = staging_dir.join(format!("{}.tmp", fake_hash));
    fs::write(&staging_file, "interrupted staging file content").unwrap();
    assert!(staging_file.exists());

    // Create database with pending WAL transaction
    let mut db = Database::open(&db_path).unwrap();
    db.insert_wal_transaction(
        "crash-recovery-test-tx-001",
        "dedup",
        TransactionStatus::Pending,
        Some("/original/model.safetensors"),
        Some(fake_hash),
        None,
    )
    .unwrap();

    // Verify transaction is present
    let incomplete = db.get_incomplete_wal_transactions().unwrap();
    assert_eq!(incomplete.len(), 1);

    // Create engine and run recovery
    let db2 = Database::open(&db_path).unwrap();
    let mut engine = DedupEngine::new(db2, store_dir.clone());
    let count = engine.recover_incomplete_transactions().unwrap();

    // Should have processed 1 transaction
    assert_eq!(count, 1);

    // Staging file should be cleaned up
    assert!(!staging_file.exists(), "Staging file should be removed during recovery");

    // No more incomplete transactions
    let db3 = Database::open(&db_path).unwrap();
    let remaining = db3.get_incomplete_wal_transactions().unwrap();
    assert_eq!(remaining.len(), 0, "No incomplete transactions should remain");
}

/// Test crash recovery: simulate a copied WAL transaction (Phase B didn't complete)
#[test]
fn test_crash_recovery_copied() {
    use modeld_core::db::TransactionStatus;

    let test_dir = TempDir::new().unwrap();
    let store_dir = test_dir.path().join("store");

    let db_path = store_dir.join("modeld.db");
    fs::create_dir_all(&store_dir).unwrap();

    // Create a real staging file with known content
    let content = b"model content for crash recovery test";
    let staging_dir = store_dir.join("tmp").join("cas_staging");
    fs::create_dir_all(&staging_dir).unwrap();

    // Hash the content
    let temp_file = tempfile::NamedTempFile::new().unwrap();
    fs::write(temp_file.path(), content).unwrap();
    let hash = modeld_core::hash_file(temp_file.path()).unwrap();
    let hash_str = hash.as_hex().to_string();

    let staging_file = staging_dir.join(format!("{}.tmp", hash_str));
    fs::write(&staging_file, content).unwrap();

    // Create database with 'copied' WAL transaction
    let mut db = Database::open(&db_path).unwrap();
    db.insert_or_update_model(&hash, content.len() as i64, None, None, None, None)
        .unwrap();
    db.insert_wal_transaction(
        "crash-recovery-copied-tx-001",
        "dedup",
        TransactionStatus::Copied,
        Some("/some/original/model.safetensors"),
        Some(&hash_str),
        None,
    )
    .unwrap();

    // Run recovery
    let db2 = Database::open(&db_path).unwrap();
    let mut engine = DedupEngine::new(db2, store_dir.clone());
    let count = engine.recover_incomplete_transactions().unwrap();

    // Should have processed 1 transaction
    assert_eq!(count, 1);

    // CAS file should now exist (Phase B completed)
    let cas_path = engine.cas_path_for_hash(&hash);
    assert!(cas_path.exists(), "CAS file should exist after recovery of 'copied' state");
}

/// Test quarantine integration
#[test]
fn test_quarantine_integration() {
    let test_dir = TempDir::new().unwrap();
    let store_dir = test_dir.path().join("store");
    fs::create_dir_all(&store_dir).unwrap();

    let qm = QuarantineManager::new(&store_dir);
    qm.init().unwrap();

    // Create a "model" to quarantine
    let model_file = test_dir.path().join("old_model.safetensors");
    let content = b"old model content that needs quarantine";
    fs::write(&model_file, content).unwrap();

    let fake_hash = "bbbb222222222222222222222222222222222222222222222222222222222222";
    let references = vec![
        "/comfyui/models/old_model.safetensors".to_string(),
        "/forge/models/old_model.safetensors".to_string(),
    ];

    // Quarantine the file
    let quarantine_path = qm
        .quarantine(&model_file, fake_hash, "replaced by dedup", references.clone())
        .unwrap();

    // Original should be gone
    assert!(!model_file.exists());
    // Quarantine path should exist
    assert!(quarantine_path.exists());

    // List should show 1 entry
    let entries = qm.list().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].meta.blake3_hash, fake_hash);
    assert_eq!(entries[0].meta.references.len(), 2);
    assert!(entries[0].days_remaining.is_some());
    assert!(entries[0].days_remaining.unwrap() >= 29);

    // Stats
    let stats = qm.stats().unwrap();
    assert_eq!(stats.total_files, 1);
    assert_eq!(stats.expired_files, 0);

    // Restore the file
    let restored = qm.restore(&quarantine_path).unwrap();
    assert_eq!(restored, model_file);
    assert!(model_file.exists());

    // Verify content
    let restored_content = fs::read(&model_file).unwrap();
    assert_eq!(restored_content, content);

    // Quarantine should be empty now
    let entries = qm.list().unwrap();
    assert_eq!(entries.len(), 0);
}

/// Test that duplicate detection works correctly with aliases
#[test]
fn test_dedup_engine_finds_duplicates_via_aliases() {
    let test_dir = TempDir::new().unwrap();
    let store_dir = test_dir.path().join("store");
    fs::create_dir_all(&store_dir).unwrap();

    let db_path = store_dir.join("modeld.db");
    let mut db = Database::open(&db_path).unwrap();

    let content = b"same model content abc123";
    let temp_file = tempfile::NamedTempFile::new().unwrap();
    fs::write(temp_file.path(), content).unwrap();
    let hash = modeld_core::hash_file(temp_file.path()).unwrap();

    // Insert model
    db.insert_or_update_model(&hash, content.len() as i64, None, None, None, None)
        .unwrap();

    // Create real temp files for the aliases to point to
    let alias_file1 = test_dir.path().join("alias1.safetensors");
    let alias_file2 = test_dir.path().join("alias2.safetensors");
    fs::write(&alias_file1, content).unwrap();
    fs::write(&alias_file2, content).unwrap();

    // Insert two aliases (simulating two copies found during scan)
    db.insert_alias(
        &hash,
        &alias_file1.to_string_lossy(),
        Frontend::User,
        AliasType::Original,
    )
    .unwrap();
    db.insert_alias(
        &hash,
        &alias_file2.to_string_lossy(),
        Frontend::ComfyUI,
        AliasType::Hardlink,
    )
    .unwrap();

    // Engine should find this as a duplicate group
    let db2 = Database::open(&db_path).unwrap();
    let engine = DedupEngine::new(db2, store_dir.clone());
    let groups = engine.find_duplicates().unwrap();

    assert_eq!(groups.len(), 1, "Should find 1 duplicate group");
    assert_eq!(groups[0].files.len(), 2, "Group should have 2 files");
    assert_eq!(groups[0].hash.as_hex(), hash.as_hex());
}
