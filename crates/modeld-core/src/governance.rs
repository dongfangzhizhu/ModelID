//! Governance domain API
//!
//! High-level wrappers around the raw `Database` operations for:
//! - Tag management
//! - Model notes
//! - Pin / favourite flags
//! - Provenance metadata
//! - Export / import of tag sets as JSON

use crate::db::Database;
use crate::hash::Blake3Hash;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────────────────────────────────────
// Provenance
// ─────────────────────────────────────────────────────────────────────────────

/// Structured provenance metadata for a model.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProvenanceInfo {
    pub source_type: String,
    pub hf_repo_id: Option<String>,
    pub revision: Option<String>,
    pub download_url: Option<String>,
    pub license: Option<String>,
    pub downloaded_at: Option<String>,
    pub original_filename: Option<String>,
    pub model_card_url: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Tags
// ─────────────────────────────────────────────────────────────────────────────

/// Attach `tag` to the model identified by `hash`.
pub fn add_tag(db: &mut Database, hash: &Blake3Hash, tag: &str) -> Result<()> {
    db.add_tag(hash, tag)
}

/// Remove `tag` from the model identified by `hash`.  No-op if not present.
pub fn remove_tag(db: &mut Database, hash: &Blake3Hash, tag: &str) -> Result<()> {
    db.remove_tag(hash, tag)
}

/// Return all tags attached to the given model, sorted alphabetically.
pub fn get_tags_for_model(db: &Database, hash: &Blake3Hash) -> Result<Vec<String>> {
    db.get_tags(hash)
}

/// Return all tags across all models with their usage counts,
/// ordered by count descending.
pub fn list_all_tags(db: &Database) -> Result<Vec<(String, i64)>> {
    db.list_all_tags()
}

// ─────────────────────────────────────────────────────────────────────────────
// Notes
// ─────────────────────────────────────────────────────────────────────────────

/// Set or replace the free-form note for a model.  Pass an empty string to
/// clear the note.
pub fn set_note(db: &mut Database, hash: &Blake3Hash, text: &str) -> Result<()> {
    let note = if text.is_empty() { None } else { Some(text) };
    db.set_model_note(hash, note)
}

/// Return the note for a model, or `None` if no note has been set.
pub fn get_note(db: &Database, hash: &Blake3Hash) -> Result<Option<String>> {
    let model = db.get_model(hash)?;
    Ok(model.and_then(|m| m.note))
}

// ─────────────────────────────────────────────────────────────────────────────
// Pin / Favourite
// ─────────────────────────────────────────────────────────────────────────────

/// Pin a model so it is protected from GC.
pub fn pin_model(db: &mut Database, hash: &Blake3Hash) -> Result<()> {
    db.pin_model(hash, true)
}

/// Remove the pin flag from a model.
pub fn unpin_model(db: &mut Database, hash: &Blake3Hash) -> Result<()> {
    db.pin_model(hash, false)
}

/// Return `true` when the model is currently pinned.
pub fn is_pinned(db: &Database, hash: &Blake3Hash) -> Result<bool> {
    Ok(db.get_model(hash)?.map(|m| m.pinned).unwrap_or(false))
}

/// Return all pinned model hashes.
pub fn list_pinned(db: &Database) -> Result<Vec<Blake3Hash>> {
    // Reuse list_models and filter in memory.  A dedicated SQL query would be
    // faster at scale, but this keeps the DB layer lean.
    let models = db.list_models(None)?;
    Ok(models.into_iter().filter(|m| m.pinned).map(|m| m.blake3_hash).collect())
}

/// Mark a model as a favourite.
pub fn favorite_model(db: &mut Database, hash: &Blake3Hash) -> Result<()> {
    db.favorite_model(hash, true)
}

/// Remove the favourite flag from a model.
pub fn unfavorite_model(db: &mut Database, hash: &Blake3Hash) -> Result<()> {
    db.favorite_model(hash, false)
}

// ─────────────────────────────────────────────────────────────────────────────
// Provenance
// ─────────────────────────────────────────────────────────────────────────────

/// Persist provenance metadata.  Fields left as `None` are not overwritten.
pub fn set_provenance(db: &mut Database, hash: &Blake3Hash, p: &ProvenanceInfo) -> Result<()> {
    db.set_model_provenance(
        hash,
        Some(p.source_type.as_str()),
        p.hf_repo_id.as_deref(),
        p.revision.as_deref(),
        p.download_url.as_deref(),
        p.license.as_deref(),
        None, // downloaded_by — not in ProvenanceInfo
        p.downloaded_at.as_deref(),
        p.original_filename.as_deref(),
        p.model_card_url.as_deref(),
    )
}

/// Load the provenance metadata for a model, or `None` when none has been
/// recorded yet.
pub fn get_provenance(db: &Database, hash: &Blake3Hash) -> Result<Option<ProvenanceInfo>> {
    let Some(model) = db.get_model(hash)? else {
        return Ok(None);
    };
    // Only return Some when at least one "rich" provenance field has been set.
    // source_type defaults to 'local' for every model — that alone is not
    // meaningful provenance.
    let has_provenance = model.hf_repo_id.is_some()
        || model.download_url.is_some()
        || model.original_filename.is_some()
        || model.model_card_url.is_some()
        || model.revision.is_some()
        || model.license.is_some()
        || model.downloaded_at.is_some();

    if !has_provenance {
        return Ok(None);
    }
    Ok(Some(ProvenanceInfo {
        source_type: model.source_type.unwrap_or_default(),
        hf_repo_id: model.hf_repo_id,
        revision: model.revision,
        download_url: model.download_url,
        license: model.license,
        downloaded_at: model.downloaded_at.map(|d| d.to_rfc3339()),
        original_filename: model.original_filename,
        model_card_url: model.model_card_url,
    }))
}

// ─────────────────────────────────────────────────────────────────────────────
// Export / Import
// ─────────────────────────────────────────────────────────────────────────────

/// Wire format for a single model's tag list.
#[derive(Serialize, Deserialize)]
struct TagEntry {
    hash: String,
    tags: Vec<String>,
}

/// Serialise all tags to a JSON array of `{hash, tags}` objects.
///
/// Example output:
/// ```json
/// [{"hash":"aa...","tags":["lora","sd15"]}]
/// ```
pub fn export_tags_json(db: &Database) -> Result<String> {
    let models = db.list_models(None)?;
    let mut entries: Vec<TagEntry> = Vec::new();
    for model in &models {
        let tags = db.get_tags(&model.blake3_hash)?;
        if !tags.is_empty() {
            entries.push(TagEntry { hash: model.blake3_hash.as_hex().to_string(), tags });
        }
    }
    serde_json::to_string_pretty(&entries).context("failed to serialise tags to JSON")
}

/// Import tags from a JSON string produced by [`export_tags_json`].
///
/// Silently skips entries whose hash is not present in the database.
/// Returns the total number of tag associations that were inserted (duplicates
/// already in the DB are ignored).
pub fn import_tags_json(db: &mut Database, json: &str) -> Result<usize> {
    let entries: Vec<TagEntry> = serde_json::from_str(json).context("failed to parse tags JSON")?;

    let mut count = 0usize;
    for entry in &entries {
        let hash = Blake3Hash::from_hex(&entry.hash)
            .with_context(|| format!("invalid hash in import: {}", entry.hash))?;

        // Skip hashes not in the DB rather than failing the whole import
        if db.get_model(&hash)?.is_none() {
            continue;
        }

        for tag in &entry.tags {
            db.add_tag(&hash, tag)?;
            count += 1;
        }
    }
    Ok(count)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use tempfile::NamedTempFile;

    fn make_hash(c: char) -> Blake3Hash {
        Blake3Hash::from_hex(&c.to_string().repeat(64)).unwrap()
    }

    fn open_db() -> (Database, NamedTempFile) {
        let f = NamedTempFile::new().unwrap();
        let db = Database::open(f.path()).unwrap();
        (db, f)
    }

    fn register(db: &mut Database, hash: &Blake3Hash) {
        db.insert_or_update_model(hash, 100, None, None, None, None).unwrap();
    }

    // ── Tags ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_add_remove_get_tags() {
        let (mut db, _f) = open_db();
        let h = make_hash('a');
        register(&mut db, &h);

        add_tag(&mut db, &h, "foo").unwrap();
        add_tag(&mut db, &h, "bar").unwrap();
        add_tag(&mut db, &h, "foo").unwrap(); // duplicate — must not error

        let tags = get_tags_for_model(&db, &h).unwrap();
        assert_eq!(tags, vec!["bar", "foo"]);

        remove_tag(&mut db, &h, "bar").unwrap();
        let tags = get_tags_for_model(&db, &h).unwrap();
        assert_eq!(tags, vec!["foo"]);
    }

    #[test]
    fn test_list_all_tags_counts() {
        let (mut db, _f) = open_db();
        let h1 = make_hash('1');
        let h2 = make_hash('2');
        register(&mut db, &h1);
        register(&mut db, &h2);

        add_tag(&mut db, &h1, "shared").unwrap();
        add_tag(&mut db, &h2, "shared").unwrap();
        add_tag(&mut db, &h1, "unique").unwrap();

        let all = list_all_tags(&db).unwrap();
        let shared = all.iter().find(|(t, _)| t == "shared").unwrap();
        assert_eq!(shared.1, 2);
        let unique = all.iter().find(|(t, _)| t == "unique").unwrap();
        assert_eq!(unique.1, 1);
    }

    // ── Notes ────────────────────────────────────────────────────────────────

    #[test]
    fn test_set_get_note() {
        let (mut db, _f) = open_db();
        let h = make_hash('b');
        register(&mut db, &h);

        assert_eq!(get_note(&db, &h).unwrap(), None);

        set_note(&mut db, &h, "hello world").unwrap();
        assert_eq!(get_note(&db, &h).unwrap(), Some("hello world".to_string()));

        // Clear note with empty string
        set_note(&mut db, &h, "").unwrap();
        assert_eq!(get_note(&db, &h).unwrap(), None);
    }

    // ── Pin / Favourite ──────────────────────────────────────────────────────

    #[test]
    fn test_pin_unpin_list() {
        let (mut db, _f) = open_db();
        let h1 = make_hash('c');
        let h2 = make_hash('d');
        register(&mut db, &h1);
        register(&mut db, &h2);

        assert!(!is_pinned(&db, &h1).unwrap());
        pin_model(&mut db, &h1).unwrap();
        assert!(is_pinned(&db, &h1).unwrap());
        assert!(!is_pinned(&db, &h2).unwrap());

        let pinned = list_pinned(&db).unwrap();
        assert_eq!(pinned.len(), 1);
        assert_eq!(pinned[0].as_hex(), h1.as_hex());

        unpin_model(&mut db, &h1).unwrap();
        assert!(!is_pinned(&db, &h1).unwrap());
        assert!(list_pinned(&db).unwrap().is_empty());
    }

    #[test]
    fn test_favorite_unfavorite() {
        let (mut db, _f) = open_db();
        let h = make_hash('e');
        register(&mut db, &h);

        favorite_model(&mut db, &h).unwrap();
        let m = db.get_model(&h).unwrap().unwrap();
        assert!(m.favorited);

        unfavorite_model(&mut db, &h).unwrap();
        let m = db.get_model(&h).unwrap().unwrap();
        assert!(!m.favorited);
    }

    // ── Provenance ───────────────────────────────────────────────────────────

    #[test]
    fn test_set_get_provenance() {
        let (mut db, _f) = open_db();
        let h = make_hash('f');
        register(&mut db, &h);

        assert!(get_provenance(&db, &h).unwrap().is_none());

        let p = ProvenanceInfo {
            source_type: "huggingface".to_string(),
            hf_repo_id: Some("org/repo".to_string()),
            revision: Some("main".to_string()),
            download_url: Some("https://example.com".to_string()),
            license: Some("apache-2.0".to_string()),
            downloaded_at: None,
            original_filename: Some("model.safetensors".to_string()),
            model_card_url: Some("https://hf.co/org/repo".to_string()),
        };

        set_provenance(&mut db, &h, &p).unwrap();

        let loaded = get_provenance(&db, &h).unwrap().unwrap();
        assert_eq!(loaded.source_type, "huggingface");
        assert_eq!(loaded.hf_repo_id.as_deref(), Some("org/repo"));
        assert_eq!(loaded.license.as_deref(), Some("apache-2.0"));
    }

    // ── Export / Import ──────────────────────────────────────────────────────

    #[test]
    fn test_export_import_tags_roundtrip() {
        let (mut db, _f) = open_db();
        let h1 = make_hash('7');
        let h2 = make_hash('8');
        register(&mut db, &h1);
        register(&mut db, &h2);

        add_tag(&mut db, &h1, "lora").unwrap();
        add_tag(&mut db, &h1, "sd15").unwrap();
        add_tag(&mut db, &h2, "vae").unwrap();

        let json = export_tags_json(&db).unwrap();
        assert!(json.contains("lora"));
        assert!(json.contains("vae"));

        // Import into a fresh DB
        let (mut db2, _f2) = open_db();
        register(&mut db2, &h1);
        // h2 intentionally omitted — should be skipped gracefully

        let imported = import_tags_json(&mut db2, &json).unwrap();
        // 2 tags for h1, h2 skipped
        assert_eq!(imported, 2);

        let tags = get_tags_for_model(&db2, &h1).unwrap();
        assert!(tags.contains(&"lora".to_string()));
        assert!(tags.contains(&"sd15".to_string()));
    }

    #[test]
    fn test_import_invalid_json_errors() {
        let (mut db, _f) = open_db();
        let result = import_tags_json(&mut db, "not json");
        assert!(result.is_err());
    }

    #[test]
    fn test_export_empty_db() {
        let (db, _f) = open_db();
        let json = export_tags_json(&db).unwrap();
        assert_eq!(json.trim(), "[]");
    }
}
