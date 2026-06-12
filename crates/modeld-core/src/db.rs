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

/// Alias type for filesystem links
#[derive(Debug, Clone, PartialEq)]
pub enum AliasType {
    Hardlink,
    Symlink,
    Junction,
    ReferenceOnly,
    Original,
}

impl AliasType {
    pub fn as_str(&self) -> &'static str {
        match self {
            AliasType::Hardlink => "hardlink",
            AliasType::Symlink => "symlink",
            AliasType::Junction => "junction",
            AliasType::ReferenceOnly => "reference_only",
            AliasType::Original => "original",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "hardlink" => Some(AliasType::Hardlink),
            "symlink" => Some(AliasType::Symlink),
            "junction" => Some(AliasType::Junction),
            "reference_only" => Some(AliasType::ReferenceOnly),
            "original" => Some(AliasType::Original),
            _ => None,
        }
    }
}

/// Frontend type for alias categorization
#[derive(Debug, Clone, PartialEq)]
pub enum Frontend {
    ComfyUI,
    Forge,
    A1111,
    HfCache,
    User,
}

impl Frontend {
    pub fn as_str(&self) -> &'static str {
        match self {
            Frontend::ComfyUI => "comfyui",
            Frontend::Forge => "forge",
            Frontend::A1111 => "a1111",
            Frontend::HfCache => "hf_cache",
            Frontend::User => "user",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "comfyui" => Some(Frontend::ComfyUI),
            "forge" => Some(Frontend::Forge),
            "a1111" => Some(Frontend::A1111),
            "hf_cache" => Some(Frontend::HfCache),
            "user" => Some(Frontend::User),
            _ => None,
        }
    }
}

/// Alias record - maps filesystem paths to model hashes
#[derive(Debug, Clone)]
pub struct Alias {
    pub id: i64,
    pub model_hash: Blake3Hash,
    pub path: String,
    pub frontend: Frontend,
    pub alias_type: AliasType,
    pub created_at: DateTime<Utc>,
}

/// WAL transaction status
#[derive(Debug, Clone, PartialEq)]
pub enum TransactionStatus {
    Pending,
    Copied,
    Committed,
    Failed,
}

impl TransactionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TransactionStatus::Pending => "pending",
            TransactionStatus::Copied => "copied",
            TransactionStatus::Committed => "committed",
            TransactionStatus::Failed => "failed",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(TransactionStatus::Pending),
            "copied" => Some(TransactionStatus::Copied),
            "committed" => Some(TransactionStatus::Committed),
            "failed" => Some(TransactionStatus::Failed),
            _ => None,
        }
    }
}

/// WAL transaction record for crash recovery
#[derive(Debug, Clone)]
pub struct WalTransaction {
    pub id: i64,
    pub tx_id: String,
    pub operation: String,
    pub status: TransactionStatus,
    pub source_path: Option<String>,
    pub target_hash: Option<String>,
    pub metadata: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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

            CREATE TABLE IF NOT EXISTS aliases (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                model_hash TEXT NOT NULL,
                path TEXT UNIQUE NOT NULL,
                frontend TEXT NOT NULL,
                alias_type TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                
                FOREIGN KEY (model_hash) REFERENCES models(blake3_hash),
                CHECK (alias_type IN ('hardlink', 'symlink', 'junction', 'reference_only', 'original')),
                CHECK (frontend IN ('comfyui', 'forge', 'a1111', 'hf_cache', 'user'))
            );

            CREATE INDEX IF NOT EXISTS idx_aliases_model_hash ON aliases(model_hash);
            CREATE INDEX IF NOT EXISTS idx_aliases_path ON aliases(path);
            CREATE INDEX IF NOT EXISTS idx_aliases_frontend ON aliases(frontend);
            CREATE INDEX IF NOT EXISTS idx_aliases_type ON aliases(alias_type);

            CREATE TABLE IF NOT EXISTS wal_transactions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tx_id TEXT UNIQUE NOT NULL,
                operation TEXT NOT NULL,
                status TEXT NOT NULL,
                source_path TEXT,
                target_hash TEXT,
                metadata TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                
                CHECK (status IN ('pending', 'copied', 'committed', 'failed')),
                CHECK (operation IN ('dedup', 'download', 'gc'))
            );

            CREATE INDEX IF NOT EXISTS idx_wal_status ON wal_transactions(status);
            CREATE INDEX IF NOT EXISTS idx_wal_created ON wal_transactions(created_at);
            CREATE INDEX IF NOT EXISTS idx_wal_operation ON wal_transactions(operation);
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

    /// Insert a new alias
    pub fn insert_alias(
        &mut self,
        model_hash: &Blake3Hash,
        path: &str,
        frontend: Frontend,
        alias_type: AliasType,
    ) -> Result<i64> {
        let now = Utc::now().to_rfc3339();

        self.conn.execute(
            r#"
            INSERT INTO aliases (model_hash, path, frontend, alias_type, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![
                model_hash.as_hex(),
                path,
                frontend.as_str(),
                alias_type.as_str(),
                now
            ],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Get alias by path
    pub fn get_alias_by_path(&self, path: &str) -> Result<Option<Alias>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, model_hash, path, frontend, alias_type, created_at
            FROM aliases
            WHERE path = ?1
            "#,
        )?;

        let alias = stmt
            .query_row(params![path], |row| {
                Ok(Alias {
                    id: row.get(0)?,
                    model_hash: Blake3Hash::from_hex(&row.get::<_, String>(1)?).unwrap(),
                    path: row.get(2)?,
                    frontend: Frontend::from_str(&row.get::<_, String>(3)?).unwrap(),
                    alias_type: AliasType::from_str(&row.get::<_, String>(4)?).unwrap(),
                    created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(5)?)
                        .unwrap()
                        .with_timezone(&Utc),
                })
            })
            .optional()?;

        Ok(alias)
    }

    /// Get all aliases for a model
    pub fn get_aliases_for_model(&self, model_hash: &Blake3Hash) -> Result<Vec<Alias>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, model_hash, path, frontend, alias_type, created_at
            FROM aliases
            WHERE model_hash = ?1
            ORDER BY created_at ASC
            "#,
        )?;

        let aliases = stmt
            .query_map(params![model_hash.as_hex()], |row| {
                Ok(Alias {
                    id: row.get(0)?,
                    model_hash: Blake3Hash::from_hex(&row.get::<_, String>(1)?).unwrap(),
                    path: row.get(2)?,
                    frontend: Frontend::from_str(&row.get::<_, String>(3)?).unwrap(),
                    alias_type: AliasType::from_str(&row.get::<_, String>(4)?).unwrap(),
                    created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(5)?)
                        .unwrap()
                        .with_timezone(&Utc),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(aliases)
    }

    /// Delete alias by path
    pub fn delete_alias(&mut self, path: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM aliases WHERE path = ?1", params![path])?;
        Ok(())
    }

    /// Count aliases for a model (reference counting)
    pub fn count_aliases(&self, model_hash: &Blake3Hash) -> Result<i64> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM aliases WHERE model_hash = ?1",
            params![model_hash.as_hex()],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Insert a new WAL transaction
    pub fn insert_wal_transaction(
        &mut self,
        tx_id: &str,
        operation: &str,
        status: TransactionStatus,
        source_path: Option<&str>,
        target_hash: Option<&str>,
        metadata: Option<&str>,
    ) -> Result<i64> {
        let now = Utc::now().to_rfc3339();

        self.conn.execute(
            r#"
            INSERT INTO wal_transactions 
            (tx_id, operation, status, source_path, target_hash, metadata, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
            "#,
            params![tx_id, operation, status.as_str(), source_path, target_hash, metadata, now],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Update WAL transaction status
    pub fn update_wal_status(&mut self, tx_id: &str, status: TransactionStatus) -> Result<()> {
        let now = Utc::now().to_rfc3339();

        self.conn.execute(
            "UPDATE wal_transactions SET status = ?1, updated_at = ?2 WHERE tx_id = ?3",
            params![status.as_str(), now, tx_id],
        )?;

        Ok(())
    }

    /// Get WAL transaction by tx_id
    pub fn get_wal_transaction(&self, tx_id: &str) -> Result<Option<WalTransaction>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, tx_id, operation, status, source_path, target_hash, 
                   metadata, created_at, updated_at
            FROM wal_transactions
            WHERE tx_id = ?1
            "#,
        )?;

        let tx = stmt
            .query_row(params![tx_id], |row| {
                Ok(WalTransaction {
                    id: row.get(0)?,
                    tx_id: row.get(1)?,
                    operation: row.get(2)?,
                    status: TransactionStatus::from_str(&row.get::<_, String>(3)?).unwrap(),
                    source_path: row.get(4)?,
                    target_hash: row.get(5)?,
                    metadata: row.get(6)?,
                    created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(7)?)
                        .unwrap()
                        .with_timezone(&Utc),
                    updated_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(8)?)
                        .unwrap()
                        .with_timezone(&Utc),
                })
            })
            .optional()?;

        Ok(tx)
    }

    /// Get all incomplete WAL transactions (pending or copied)
    pub fn get_incomplete_wal_transactions(&self) -> Result<Vec<WalTransaction>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, tx_id, operation, status, source_path, target_hash, 
                   metadata, created_at, updated_at
            FROM wal_transactions
            WHERE status IN ('pending', 'copied')
            ORDER BY created_at ASC
            "#,
        )?;

        let transactions = stmt
            .query_map([], |row| {
                Ok(WalTransaction {
                    id: row.get(0)?,
                    tx_id: row.get(1)?,
                    operation: row.get(2)?,
                    status: TransactionStatus::from_str(&row.get::<_, String>(3)?).unwrap(),
                    source_path: row.get(4)?,
                    target_hash: row.get(5)?,
                    metadata: row.get(6)?,
                    created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(7)?)
                        .unwrap()
                        .with_timezone(&Utc),
                    updated_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(8)?)
                        .unwrap()
                        .with_timezone(&Utc),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(transactions)
    }

    /// Delete WAL transaction by tx_id
    pub fn delete_wal_transaction(&mut self, tx_id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM wal_transactions WHERE tx_id = ?1", params![tx_id])?;
        Ok(())
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

    #[test]
    fn test_insert_and_retrieve_alias() {
        let temp_db = NamedTempFile::new().unwrap();
        let mut db = Database::open(temp_db.path()).unwrap();

        let hash = Blake3Hash::from_hex(
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        )
        .unwrap();

        // Insert model first
        db.insert_or_update_model(&hash, 1024, Some("safetensors"), None, None, None)
            .unwrap();

        // Insert alias
        db.insert_alias(
            &hash,
            "C:\\models\\sdxl.safetensors",
            Frontend::User,
            AliasType::Original,
        )
        .unwrap();

        // Retrieve by path
        let alias = db
            .get_alias_by_path("C:\\models\\sdxl.safetensors")
            .unwrap()
            .unwrap();

        assert_eq!(alias.model_hash.as_hex(), hash.as_hex());
        assert_eq!(alias.path, "C:\\models\\sdxl.safetensors");
        assert_eq!(alias.frontend, Frontend::User);
        assert_eq!(alias.alias_type, AliasType::Original);
    }

    #[test]
    fn test_get_aliases_for_model() {
        let temp_db = NamedTempFile::new().unwrap();
        let mut db = Database::open(temp_db.path()).unwrap();

        let hash = Blake3Hash::from_hex(
            "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
        )
        .unwrap();

        // Insert model
        db.insert_or_update_model(&hash, 2048, None, None, None, None)
            .unwrap();

        // Insert multiple aliases
        db.insert_alias(
            &hash,
            "C:\\ComfyUI\\models\\sdxl.safetensors",
            Frontend::ComfyUI,
            AliasType::Hardlink,
        )
        .unwrap();

        db.insert_alias(
            &hash,
            "D:\\Forge\\models\\sdxl.safetensors",
            Frontend::Forge,
            AliasType::Symlink,
        )
        .unwrap();

        // Retrieve all aliases
        let aliases = db.get_aliases_for_model(&hash).unwrap();
        assert_eq!(aliases.len(), 2);

        // Count aliases (reference counting)
        let count = db.count_aliases(&hash).unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_delete_alias() {
        let temp_db = NamedTempFile::new().unwrap();
        let mut db = Database::open(temp_db.path()).unwrap();

        let hash = Blake3Hash::from_hex(
            "fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321",
        )
        .unwrap();

        // Insert model and alias
        db.insert_or_update_model(&hash, 512, None, None, None, None)
            .unwrap();
        db.insert_alias(&hash, "C:\\test.safetensors", Frontend::User, AliasType::Original)
            .unwrap();

        // Verify exists
        assert!(db.get_alias_by_path("C:\\test.safetensors").unwrap().is_some());

        // Delete
        db.delete_alias("C:\\test.safetensors").unwrap();

        // Verify deleted
        assert!(db.get_alias_by_path("C:\\test.safetensors").unwrap().is_none());
    }

    #[test]
    fn test_wal_transaction_lifecycle() {
        let temp_db = NamedTempFile::new().unwrap();
        let mut db = Database::open(temp_db.path()).unwrap();

        let tx_id = "550e8400-e29b-41d4-a716-446655440000";
        let hash = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";

        // Insert transaction
        db.insert_wal_transaction(
            tx_id,
            "dedup",
            TransactionStatus::Pending,
            Some("C:\\source.safetensors"),
            Some(hash),
            Some(r#"{"test": "metadata"}"#),
        )
        .unwrap();

        // Retrieve
        let tx = db.get_wal_transaction(tx_id).unwrap().unwrap();
        assert_eq!(tx.tx_id, tx_id);
        assert_eq!(tx.operation, "dedup");
        assert_eq!(tx.status, TransactionStatus::Pending);
        assert_eq!(tx.source_path.as_deref(), Some("C:\\source.safetensors"));
        assert_eq!(tx.target_hash.as_deref(), Some(hash));

        // Update status
        db.update_wal_status(tx_id, TransactionStatus::Copied)
            .unwrap();

        let tx = db.get_wal_transaction(tx_id).unwrap().unwrap();
        assert_eq!(tx.status, TransactionStatus::Copied);

        // Update to committed
        db.update_wal_status(tx_id, TransactionStatus::Committed)
            .unwrap();

        let tx = db.get_wal_transaction(tx_id).unwrap().unwrap();
        assert_eq!(tx.status, TransactionStatus::Committed);
    }

    #[test]
    fn test_get_incomplete_wal_transactions() {
        let temp_db = NamedTempFile::new().unwrap();
        let mut db = Database::open(temp_db.path()).unwrap();

        // Insert various transactions
        db.insert_wal_transaction(
            "tx1",
            "dedup",
            TransactionStatus::Pending,
            None,
            None,
            None,
        )
        .unwrap();

        db.insert_wal_transaction(
            "tx2",
            "dedup",
            TransactionStatus::Copied,
            None,
            None,
            None,
        )
        .unwrap();

        db.insert_wal_transaction(
            "tx3",
            "dedup",
            TransactionStatus::Committed,
            None,
            None,
            None,
        )
        .unwrap();

        db.insert_wal_transaction(
            "tx4",
            "download",
            TransactionStatus::Failed,
            None,
            None,
            None,
        )
        .unwrap();

        // Query incomplete (only pending and copied)
        let incomplete = db.get_incomplete_wal_transactions().unwrap();
        assert_eq!(incomplete.len(), 2);

        let statuses: Vec<_> = incomplete.iter().map(|tx| &tx.status).collect();
        assert!(statuses.contains(&&TransactionStatus::Pending));
        assert!(statuses.contains(&&TransactionStatus::Copied));
    }

    #[test]
    fn test_delete_wal_transaction() {
        let temp_db = NamedTempFile::new().unwrap();
        let mut db = Database::open(temp_db.path()).unwrap();

        let tx_id = "delete-test-tx";

        // Insert
        db.insert_wal_transaction(tx_id, "gc", TransactionStatus::Committed, None, None, None)
            .unwrap();

        // Verify exists
        assert!(db.get_wal_transaction(tx_id).unwrap().is_some());

        // Delete
        db.delete_wal_transaction(tx_id).unwrap();

        // Verify deleted
        assert!(db.get_wal_transaction(tx_id).unwrap().is_none());
    }

    #[test]
    fn test_foreign_key_constraint() {
        let temp_db = NamedTempFile::new().unwrap();
        let mut db = Database::open(temp_db.path()).unwrap();

        let hash = Blake3Hash::from_hex(
            "1111111111111111111111111111111111111111111111111111111111111111",
        )
        .unwrap();

        // Try to insert alias without model (should fail due to foreign key)
        let result = db.insert_alias(&hash, "C:\\test.txt", Frontend::User, AliasType::Original);

        assert!(result.is_err());
    }
}
