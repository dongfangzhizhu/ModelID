//! Integration tests for modeld-core
//!
//! Tests the complete workflow: scan → store → retrieve

use modeld_core::{CasStore, Database, Scanner};
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
        db.insert_or_update_model(&file.hash, file.size as i64, Some("safetensors"), None, None, None)
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
        db.insert_or_update_model(&file.hash, file.size as i64, Some("safetensors"), None, None, None)
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

        db.insert_or_update_model(&hash, 1024, Some("safetensors"), Some("sdxl"), Some("checkpoint"), None)
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
