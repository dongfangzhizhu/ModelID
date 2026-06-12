//! SQLite database layer for metadata management
//!
//! Implements the models table from Phase 0 schema with:
//! - WAL mode for concurrent access
//! - Indexes for performance
//! - CRUD operations

use crate::hash::Blake3Hash;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

/// Model metadata stored in database
#[derive(Debug, Clone)]
pub struct Model {
    pub id: i64,
    pub blake3_hash: Blake3Hash,
    pub size_bytes: i64,
    pub format: Option<String>,
    pub arch: Option<String>,
    pub category: Option<String>,
    pub base_model: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub quarantined_at: Option<DateTime<Utc>>,
}

/// Database connection manager
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open or create database at the given path
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("Failed to open database at {}", path.display()))?;

        // Enable WAL mode for concurrent access
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        let mut db = Self { conn };
        db.init_schema()?;

        Ok(db)
    }

    /// Initialize database schema
    fn init_schema(&mut self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS models (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                blake3_hash TEXT UNIQUE NOT NULL,
                size_bytes INTEGER NOT NULL,
                format TEXT,
                arch TEXT,
                category TEXT,
                base_model TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                last_seen TEXT NOT NULL DEFAULT (datetime('now')),
                quarantined_at TEXT DEFAULT NULL,
                
                CHECK (length(blake3_hash) = 64)
            );

            CREATE INDEX IF NOT EXISTS idx_models_hash ON models(blake3_hash);
            CREATE INDEX IF NOT EXISTS idx_models_format ON models(format);
            CREATE INDEX IF NOT EXISTS idx_models_arch ON models(arch);
            CREATE INDEX IF NOT EXISTS idx_models_category ON models(category);
            CREATE INDEX IF NOT EXISTS idx_models_quarantined ON models(quarantined_at);
            "#,
        )?;

        Ok(())
    }

    /// Insert a new model or update if exists
    pub fn insert_or_update_model(
        &mut self,
        hash: &Blake3Hash,
        size_bytes: i64,
        format: Option<&str>,
        arch: Option<&str>,
        category: Option<&str>,
        base_model: Option<&str>,
    ) -> Result<i64> {
        let now = Utc::now().to_rfc3339();

        self.conn.execute(
            r#"
            INSERT INTO models (blake3_hash, size_bytes, format, arch, category, base_model, created_at, last_seen)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
            ON CONFLICT(blake3_hash) DO UPDATE SET
                last_seen = ?7,
                size_bytes = ?2,
                format = COALESCE(?3, format),
                arch = COALESCE(?4, arch),
                category = COALESCE(?5, category),
                base_model = COALESCE(?6, base_model)
            "#,
            params![hash.as_hex(), size_bytes, format, arch, category, base_model, now],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Get model by hash
    pub fn get_model(&self, hash: &Blake3Hash) -> Result<Option<Model>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, blake3_hash, size_bytes, format, arch, category, base_model,
                   created_at, last_seen, quarantined_at
            FROM models
            WHERE blake3_hash = ?1
            "#,
        )?;

        let model = stmt
            .query_row(params![hash.as_hex()], |row| {
                Ok(Model {
                    id: row.get(0)?,
                    blake3_hash: Blake3Hash::from_hex(&row.get::<_, String>(1)?).unwrap(),
                    size_bytes: row.get(2)?,
                    format: row.get(3)?,
                    arch: row.get(4)?,
                    category: row.get(5)?,
                    base_model: row.get(6)?,
                    created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(7)?)
                        .unwrap()
                        .with_timezone(&Utc),
                    last_seen: DateTime::parse_from_rfc3339(&row.get::<_, String>(8)?)
                        .unwrap()
                        .with_timezone(&Utc),
                    quarantined_at: row
                        .get::<_, Option<String>>(9)?
                        .map(|s| DateTime::parse_from_rfc3339(&s).unwrap().with_timezone(&Utc)),
                })
            })
            .optional()?;

        Ok(model)
    }

    /// Get total count of models
    pub fn count_models(&self) -> Result<i64> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM models", [], |row| row.get(0))?;
        Ok(count)
    }

    /// Get total size of all models
    pub fn total_size(&self) -> Result<i64> {
        let size: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(size_bytes), 0) FROM models",
            [],
            |row| row.get(0),
        )?;
        Ok(size)
    }

    /// List all models
    pub fn list_models(&self, limit: Option<i64>) -> Result<Vec<Model>> {
        let sql = if let Some(limit) = limit {
            format!(
                "SELECT id, blake3_hash, size_bytes, format, arch, category, base_model,
                        created_at, last_seen, quarantined_at
                 FROM models
                 ORDER BY created_at DESC
                 LIMIT {}",
                limit
            )
        } else {
            "SELECT id, blake3_hash, size_bytes, format, arch, category, base_model,
                    created_at, last_seen, quarantined_at
             FROM models
             ORDER BY created_at DESC"
                .to_string()
        };

        let mut stmt = self.conn.prepare(&sql)?;
        let models = stmt
            .query_map([], |row| {
                Ok(Model {
                    id: row.get(0)?,
                    blake3_hash: Blake3Hash::from_hex(&row.get::<_, String>(1)?).unwrap(),
                    size_bytes: row.get(2)?,
                    format: row.get(3)?,
                    arch: row.get(4)?,
                    category: row.get(5)?,
                    base_model: row.get(6)?,
                    created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(7)?)
                        .unwrap()
                        .with_timezone(&Utc),
                    last_seen: DateTime::parse_from_rfc3339(&row.get::<_, String>(8)?)
                        .unwrap()
                        .with_timezone(&Utc),
                    quarantined_at: row
                        .get::<_, Option<String>>(9)?
                        .map(|s| DateTime::parse_from_rfc3339(&s).unwrap().with_timezone(&Utc)),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(models)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_db_init() {
        let temp_db = NamedTempFile::new().unwrap();
        let db = Database::open(temp_db.path()).unwrap();

        // Verify tables exist
        let table_count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='models'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(table_count, 1);
    }

    #[test]
    fn test_insert_and_retrieve() {
        let temp_db = NamedTempFile::new().unwrap();
        let mut db = Database::open(temp_db.path()).unwrap();

        let hash = Blake3Hash::from_hex(
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        )
        .unwrap();

        // Insert model
        db.insert_or_update_model(&hash, 1024, Some("safetensors"), Some("sdxl"), None, None)
            .unwrap();

        // Retrieve model
        let model = db.get_model(&hash).unwrap().unwrap();

        assert_eq!(model.blake3_hash.as_hex(), hash.as_hex());
        assert_eq!(model.size_bytes, 1024);
        assert_eq!(model.format.as_deref(), Some("safetensors"));
        assert_eq!(model.arch.as_deref(), Some("sdxl"));
    }

    #[test]
    fn test_update_existing() {
        let temp_db = NamedTempFile::new().unwrap();
        let mut db = Database::open(temp_db.path()).unwrap();

        let hash = Blake3Hash::from_hex(
            "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
        )
        .unwrap();

        // Insert first time
        db.insert_or_update_model(&hash, 2048, Some("gguf"), None, None, None)
            .unwrap();

        // Update with additional metadata
        db.insert_or_update_model(&hash, 2048, Some("gguf"), Some("llama"), Some("lora"), None)
            .unwrap();

        // Verify updated
        let model = db.get_model(&hash).unwrap().unwrap();
        assert_eq!(model.format.as_deref(), Some("gguf"));
        assert_eq!(model.arch.as_deref(), Some("llama"));
        assert_eq!(model.category.as_deref(), Some("lora"));
    }

    #[test]
    fn test_count_and_size() {
        let temp_db = NamedTempFile::new().unwrap();
        let mut db = Database::open(temp_db.path()).unwrap();

        // Insert multiple models
        for i in 0..5 {
            let hash_str = format!("{:064x}", i);
            let hash = Blake3Hash::from_hex(&hash_str).unwrap();
            db.insert_or_update_model(&hash, 1000 * (i as i64 + 1), None, None, None, None)
                .unwrap();
        }

        assert_eq!(db.count_models().unwrap(), 5);
        assert_eq!(db.total_size().unwrap(), 1000 + 2000 + 3000 + 4000 + 5000);
    }

    #[test]
    fn test_list_models() {
        let temp_db = NamedTempFile::new().unwrap();
        let mut db = Database::open(temp_db.path()).unwrap();

        // Insert models
        for i in 0..10 {
            let hash_str = format!("{:064x}", i);
            let hash = Blake3Hash::from_hex(&hash_str).unwrap();
            db.insert_or_update_model(&hash, 1024, None, None, None, None)
                .unwrap();
        }

        // List all
        let all_models = db.list_models(None).unwrap();
        assert_eq!(all_models.len(), 10);

        // List limited
        let limited = db.list_models(Some(5)).unwrap();
        assert_eq!(limited.len(), 5);
    }
}
