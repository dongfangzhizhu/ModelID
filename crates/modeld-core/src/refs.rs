//! Model reference inspection (`modeld refs`)
//!
//! Provides three public entry-points:
//! - [`scan_workflow_refs`] — parse a ComfyUI workflow JSON and index its refs
//! - [`explain_refs`]       — why a specific hash cannot be deleted
//! - [`list_orphans`]       — hashes with no references at all

use crate::db::Database;
use crate::hash::Blake3Hash;
use crate::workflow::{build_model_lookup, index_workflow};
use anyhow::Result;
use std::path::Path;

/// Parse a ComfyUI workflow JSON at `path` and upsert its model references
/// into the database.
///
/// Returns the number of model references indexed (resolved + unresolved).
pub fn scan_workflow_refs(db: &mut Database, path: &Path) -> Result<usize> {
    let lookup = build_model_lookup(db)?;
    let (_wf_id, resolved, unresolved) = index_workflow(db, path, &lookup)?;
    Ok(resolved + unresolved)
}

/// Return a human-readable list of reasons why `hash` cannot be deleted.
///
/// Returns an empty `Vec` when the model is an orphan (safe to delete).
pub fn explain_refs(db: &Database, hash: &Blake3Hash) -> Result<Vec<String>> {
    let rows = db.get_gc_candidate_counts()?;

    let row = rows.iter().find(|(m, _, _)| m.blake3_hash.as_hex() == hash.as_hex());

    let Some((model, alias_count, workflow_ref_count)) = row else {
        return Ok(vec!["model not found in database".to_string()]);
    };

    let mut reasons = Vec::new();

    if model.pinned {
        reasons.push("pinned by user".to_string());
    }

    if *workflow_ref_count > 0 {
        reasons.push(format!(
            "referenced by {} workflow file(s)",
            workflow_ref_count
        ));
    }

    if *alias_count > 0 {
        reasons.push(format!("has {} alias(es) on disk", alias_count));
    }

    Ok(reasons)
}

/// Return all model hashes that have no hard references, no aliases, and are
/// not pinned — i.e. candidates for quarantine / deletion.
pub fn list_orphans(db: &Database) -> Result<Vec<Blake3Hash>> {
    let rows = db.get_gc_candidate_counts()?;
    let orphans = rows
        .into_iter()
        .filter(|(model, alias_count, workflow_ref_count)| {
            !model.pinned && *workflow_ref_count == 0 && *alias_count == 0
        })
        .map(|(model, _, _)| model.blake3_hash)
        .collect();
    Ok(orphans)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{AliasType, Frontend};
    use crate::gc::RefStatus;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn open_db() -> (Database, NamedTempFile) {
        let f = NamedTempFile::new().unwrap();
        let db = Database::open(f.path()).unwrap();
        (db, f)
    }

    fn make_hash(c: char) -> Blake3Hash {
        // Map each char to a unique 64-character hex string.
        let s = match c {
            c if c.is_ascii_hexdigit() => c.to_string().repeat(64),
            // Non-hex chars: use two-char alternating patterns (distinct from single-char repeats).
            'g' => "e0".repeat(32),
            'h' => "d1".repeat(32),
            'i' => "c2".repeat(32),
            'j' => "b3".repeat(32),
            'k' => "a4".repeat(32),
            _ => "0f".repeat(32),
        };
        Blake3Hash::from_hex(&s).unwrap()
    }

    fn write_workflow_json(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::with_suffix(".json").unwrap();
        write!(f, "{}", content).unwrap();
        f
    }

    // ── scan_workflow_refs ────────────────────────────────────────────────────

    #[test]
    fn test_scan_workflow_refs_counts_refs() {
        let (mut db, _f) = open_db();
        let h = make_hash('a');
        db.insert_or_update_model(&h, 1000, None, None, None, None).unwrap();

        let wf = write_workflow_json(
            r#"{
                "nodes": [
                    {
                        "type": "CheckpointLoaderSimple",
                        "inputs": {"ckpt_name": "model.safetensors"},
                        "widgets_values": []
                    },
                    {
                        "type": "LoraLoader",
                        "inputs": {"lora_name": "my_lora.safetensors"},
                        "widgets_values": []
                    }
                ]
            }"#,
        );

        let count = scan_workflow_refs(&mut db, wf.path()).unwrap();
        assert_eq!(count, 2);
    }

    // ── explain_refs ─────────────────────────────────────────────────────────

    #[test]
    fn test_explain_refs_orphan() {
        let (db, _f) = open_db();
        let mut db = db;
        let h = make_hash('b');
        db.insert_or_update_model(&h, 100, None, None, None, None).unwrap();

        let reasons = explain_refs(&db, &h).unwrap();
        assert!(reasons.is_empty(), "orphan should have no reasons: {:?}", reasons);
    }

    #[test]
    fn test_explain_refs_pinned() {
        let (mut db, _f) = open_db();
        let h = make_hash('c');
        db.insert_or_update_model(&h, 100, None, None, None, None).unwrap();
        db.pin_model(&h, true).unwrap();

        let reasons = explain_refs(&db, &h).unwrap();
        assert!(reasons.iter().any(|r| r.contains("pinned")));
    }

    #[test]
    fn test_explain_refs_has_alias() {
        let (mut db, _f) = open_db();
        let h = make_hash('d');
        db.insert_or_update_model(&h, 100, None, None, None, None).unwrap();
        db.insert_alias(&h, "/some/model.safetensors", Frontend::User, AliasType::Original)
            .unwrap();

        let reasons = explain_refs(&db, &h).unwrap();
        assert!(reasons.iter().any(|r| r.contains("alias")));
    }

    #[test]
    fn test_explain_refs_unknown_hash() {
        let (db, _f) = open_db();
        let h = make_hash('e');
        let reasons = explain_refs(&db, &h).unwrap();
        assert!(reasons.iter().any(|r| r.contains("not found")));
    }

    // ── list_orphans ──────────────────────────────────────────────────────────

    #[test]
    fn test_list_orphans_empty() {
        let (db, _f) = open_db();
        assert!(list_orphans(&db).unwrap().is_empty());
    }

    #[test]
    fn test_list_orphans_returns_unreferenced() {
        let (mut db, _f) = open_db();
        let orphan = make_hash('f');
        let aliased = make_hash('1');

        db.insert_or_update_model(&orphan, 100, None, None, None, None).unwrap();
        db.insert_or_update_model(&aliased, 200, None, None, None, None).unwrap();
        db.insert_alias(&aliased, "/path/model.safetensors", Frontend::User, AliasType::Original)
            .unwrap();

        let orphans = list_orphans(&db).unwrap();
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].as_hex(), orphan.as_hex());
    }

    #[test]
    fn test_list_orphans_pinned_not_returned() {
        let (mut db, _f) = open_db();
        let h = make_hash('g');
        db.insert_or_update_model(&h, 100, None, None, None, None).unwrap();
        db.pin_model(&h, true).unwrap();

        // Even though there are no aliases/workflow refs, pinned is not an orphan.
        let orphans = list_orphans(&db).unwrap();
        assert!(orphans.is_empty());
    }

    // ── classify_refs ─────────────────────────────────────────────────────────

    #[test]
    fn test_classify_refs_orphan() {
        let (mut db, _f) = open_db();
        let h = make_hash('h');
        db.insert_or_update_model(&h, 100, None, None, None, None).unwrap();
        assert_eq!(crate::gc::classify_refs(&db, &h), RefStatus::OrphanCandidate);
    }

    #[test]
    fn test_classify_refs_soft() {
        let (mut db, _f) = open_db();
        let h = make_hash('i');
        db.insert_or_update_model(&h, 100, None, None, None, None).unwrap();
        db.insert_alias(&h, "/path/m.safetensors", Frontend::User, AliasType::Original).unwrap();
        assert_eq!(crate::gc::classify_refs(&db, &h), RefStatus::SoftReference);
    }

    #[test]
    fn test_classify_refs_pinned() {
        let (mut db, _f) = open_db();
        let h = make_hash('j');
        db.insert_or_update_model(&h, 100, None, None, None, None).unwrap();
        db.pin_model(&h, true).unwrap();
        assert_eq!(crate::gc::classify_refs(&db, &h), RefStatus::Pinned);
    }

    #[test]
    fn test_classify_refs_unknown_hash() {
        let (db, _f) = open_db();
        let h = make_hash('k');
        assert_eq!(crate::gc::classify_refs(&db, &h), RefStatus::Unknown);
    }
}
