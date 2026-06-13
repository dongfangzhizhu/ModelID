//! Safe Garbage Collection (Phase 4)
//!
//! GC Protection Hierarchy:
//! 1. Hard protection: workflow_refs exists → never GC
//! 2. Soft protection: aliases exist → warn + confirm
//! 3. No protection: ref=0, alias=0 → quarantine (30-day TTL)
//! 4. Final deletion: quarantine TTL expired → physical delete

use crate::cas::CasStore;
use crate::db::{Database, Model};
use crate::quarantine::QuarantineManager;
use anyhow::Result;

/// A GC candidate with its protection status
#[derive(Debug, Clone)]
pub struct GcCandidate {
    pub model: Model,
    /// Number of workflow_refs pointing to this model
    pub workflow_ref_count: usize,
    /// Number of aliases (frontend symlinks/hardlinks)
    pub alias_count: usize,
    /// Whether this model's CAS file actually exists on disk
    pub cas_file_exists: bool,
    /// Estimated disk savings if removed (bytes)
    pub savings_bytes: i64,
}

impl GcCandidate {
    /// Level 1: Never GC — has workflow references
    pub fn is_hard_protected(&self) -> bool {
        self.workflow_ref_count > 0
    }

    /// Level 2: Soft warning — has aliases but no workflow refs
    pub fn is_soft_protected(&self) -> bool {
        self.workflow_ref_count == 0 && self.alias_count > 0
    }

    /// Level 3: Safe to quarantine — no refs, no aliases
    pub fn is_orphan(&self) -> bool {
        self.workflow_ref_count == 0 && self.alias_count == 0
    }
}

/// Result of a GC run
#[derive(Debug, Default)]
pub struct GcResult {
    /// Models moved to quarantine (hash prefix)
    pub quarantined: Vec<String>,
    /// Models skipped (hard protected)
    pub skipped_protected: Vec<String>,
    /// Models skipped (soft protected, user declined)
    pub skipped_soft: Vec<String>,
    /// Total bytes recovered by quarantine
    pub bytes_recovered: i64,
    /// Models already in quarantine that were cleaned up
    pub cleaned_quarantine: usize,
}

/// The GC engine
pub struct GcEngine<'a> {
    db: &'a Database,
    cas: CasStore,
    quarantine: QuarantineManager,
}

impl<'a> GcEngine<'a> {
    pub fn new(db: &'a Database, store_path: &std::path::Path) -> Self {
        Self {
            db,
            cas: CasStore::new(store_path),
            quarantine: QuarantineManager::new(store_path),
        }
    }

    /// List all GC candidates (models with their protection status).
    pub fn candidates(&self) -> Result<Vec<GcCandidate>> {
        let models = self.db.list_models(None)?;
        let mut candidates = Vec::new();

        for model in models {
            let hash_str = model.blake3_hash.as_hex().to_string();
            let workflow_refs = self.db.get_workflows_for_model(&hash_str)?;
            let aliases = self.db.get_aliases_for_model(&model.blake3_hash)?;
            let cas_file_exists = self.cas.contains(&model.blake3_hash);

            candidates.push(GcCandidate {
                savings_bytes: model.size_bytes,
                workflow_ref_count: workflow_refs.len(),
                alias_count: aliases.len(),
                cas_file_exists,
                model,
            });
        }

        Ok(candidates)
    }

    /// Preview GC — list what would happen without making changes.
    pub fn preview(&self) -> Result<GcPreview> {
        let candidates = self.candidates()?;
        let mut preview = GcPreview::default();

        for c in &candidates {
            let hash_prefix = &c.model.blake3_hash.as_hex()[..16];
            if c.is_hard_protected() {
                preview.hard_protected.push(hash_prefix.to_string());
            } else if c.is_soft_protected() {
                preview.soft_protected.push(GcPreviewItem {
                    hash_prefix: hash_prefix.to_string(),
                    name: c.model.format.clone(),
                    size_bytes: c.model.size_bytes,
                    alias_count: c.alias_count,
                });
            } else if c.is_orphan() {
                preview.would_quarantine.push(GcPreviewItem {
                    hash_prefix: hash_prefix.to_string(),
                    name: c.model.format.clone(),
                    size_bytes: c.model.size_bytes,
                    alias_count: 0,
                });
                preview.total_reclaimable_bytes += c.model.size_bytes;
            }
        }

        // Check expired quarantine entries
        let entries = self.quarantine.list().unwrap_or_default();
        let expired: Vec<_> = entries.iter().filter(|e| e.days_remaining.is_none()).collect();
        preview.expired_quarantine_count = expired.len();
        preview.expired_quarantine_bytes = expired.iter()
            .map(|e| e.meta.size_bytes as i64)
            .sum();

        Ok(preview)
    }

    /// Execute safe GC: quarantine all zero-ref models.
    pub fn run_safe(&self) -> Result<GcResult> {
        let mut result = GcResult::default();
        let candidates = self.candidates()?;

        for c in &candidates {
            let hash_prefix = c.model.blake3_hash.as_hex()[..16].to_string();

            if c.is_hard_protected() {
                result.skipped_protected.push(hash_prefix);
                continue;
            }

            if c.is_soft_protected() {
                result.skipped_soft.push(hash_prefix);
                continue;
            }

            if c.is_orphan() && c.cas_file_exists {
                // Move to quarantine
                if let Some(cas_path) = self.cas.get(&c.model.blake3_hash) {
                    self.quarantine.init()?;
                    let hash_str = c.model.blake3_hash.as_hex().to_string();
                    match self.quarantine.quarantine(
                        &cas_path,
                        &hash_str,
                        "gc: no workflow refs, no aliases",
                        vec![],
                    ) {
                        Ok(_) => {
                            result.quarantined.push(hash_prefix);
                            result.bytes_recovered += c.model.size_bytes;
                        }
                        Err(e) => {
                            eprintln!("Warning: Failed to quarantine {}: {}", &hash_str[..16], e);
                        }
                    }
                }
            }
        }

        // Clean up expired quarantine entries
        result.cleaned_quarantine = self.quarantine.cleanup_expired()?;

        Ok(result)
    }

    /// Clean expired quarantine entries (final deletion).
    pub fn cleanup_quarantine(&self) -> Result<usize> {
        self.quarantine.cleanup_expired()
    }
}

/// Preview of what GC would do
#[derive(Debug, Default)]
pub struct GcPreview {
    pub hard_protected: Vec<String>,
    pub soft_protected: Vec<GcPreviewItem>,
    pub would_quarantine: Vec<GcPreviewItem>,
    pub total_reclaimable_bytes: i64,
    pub expired_quarantine_count: usize,
    pub expired_quarantine_bytes: i64,
}

#[derive(Debug, Clone)]
pub struct GcPreviewItem {
    pub hash_prefix: String,
    pub name: Option<String>,
    pub size_bytes: i64,
    pub alias_count: usize,
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::hash::Blake3Hash;
    use std::collections::HashMap;
    use std::io::Write;
    use tempfile::{NamedTempFile, TempDir};

    #[test]
    fn test_gc_candidate_protection() {
        let model = Model {
            id: 1,
            blake3_hash: Blake3Hash::from_hex(&"a".repeat(64)).unwrap(),
            size_bytes: 1000,
            format: None,
            arch: None,
            category: None,
            base_model: None,
            created_at: chrono::Utc::now(),
            last_seen: chrono::Utc::now(),
            quarantined_at: None,
        };

        let hard = GcCandidate {
            model: model.clone(),
            workflow_ref_count: 1,
            alias_count: 0,
            cas_file_exists: true,
            savings_bytes: 1000,
        };
        assert!(hard.is_hard_protected());
        assert!(!hard.is_soft_protected());
        assert!(!hard.is_orphan());

        let soft = GcCandidate {
            model: model.clone(),
            workflow_ref_count: 0,
            alias_count: 2,
            cas_file_exists: true,
            savings_bytes: 1000,
        };
        assert!(!soft.is_hard_protected());
        assert!(soft.is_soft_protected());
        assert!(!soft.is_orphan());

        let orphan = GcCandidate {
            model: model.clone(),
            workflow_ref_count: 0,
            alias_count: 0,
            cas_file_exists: true,
            savings_bytes: 1000,
        };
        assert!(!orphan.is_hard_protected());
        assert!(!orphan.is_soft_protected());
        assert!(orphan.is_orphan());
    }

    #[test]
    fn test_gc_preview_empty_store() {
        let tmp = TempDir::new().unwrap();
        let db_file = NamedTempFile::new().unwrap();
        let db = Database::open(db_file.path()).unwrap();

        let gc = GcEngine::new(&db, tmp.path());
        let preview = gc.preview().unwrap();

        assert_eq!(preview.would_quarantine.len(), 0);
        assert_eq!(preview.hard_protected.len(), 0);
    }

    #[test]
    fn test_gc_hard_protection_via_workflow_ref() {
        let tmp = TempDir::new().unwrap();
        let db_file = NamedTempFile::new().unwrap();
        let mut db = Database::open(db_file.path()).unwrap();

        // Add a model
        let hash = Blake3Hash::from_hex(&"a".repeat(64)).unwrap();
        db.insert_or_update_model(&hash, 1000, None, None, None, None).unwrap();

        // Add workflow that references the model
        let mut workflow_file = NamedTempFile::with_suffix(".json").unwrap();
        write!(workflow_file, r#"{{"nodes":[{{"type":"CheckpointLoaderSimple","inputs":{{"ckpt_name":"model.safetensors"}},"widgets_values":[]}}]}}"#).unwrap();

        let mut lookup = HashMap::new();
        lookup.insert("model.safetensors".to_string(), hash.clone());
        crate::workflow::index_workflow(&db, workflow_file.path(), &lookup).unwrap();

        // GC should hard-protect the model
        let gc = GcEngine::new(&db, tmp.path());
        let candidates = gc.candidates().unwrap();

        let candidate = candidates
            .iter()
            .find(|c| c.model.blake3_hash.as_hex() == hash.as_hex())
            .unwrap();
        assert!(candidate.is_hard_protected());
        assert_eq!(candidate.workflow_ref_count, 1);
    }
}
