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

// ─────────────────────────────────────────────────────────────────────────────
// Row-parsing helpers (avoid .unwrap() panics when reading DB data)
// ─────────────────────────────────────────────────────────────────────────────

/// Parse an RFC-3339 string into UTC DateTime.  Falls back to `Utc::now()` on
/// parse error so that a single malformed row does not crash the whole query.
fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

/// Parse a BLAKE3 hex string into `Blake3Hash`, returning a rusqlite error on
/// failure so query_map can propagate it gracefully.
fn parse_blake3(s: &str) -> std::result::Result<Blake3Hash, rusqlite::Error> {
    Blake3Hash::from_hex(s)
        .map_err(|e| rusqlite::Error::InvalidParameterName(format!("blake3: {}", e)))
}

/// Parse a `Frontend` discriminant, returning a rusqlite error on unknown values.
fn parse_frontend(s: &str) -> std::result::Result<Frontend, rusqlite::Error> {
    Frontend::from_db_value(s).ok_or_else(|| {
        rusqlite::Error::InvalidParameterName(format!("unknown frontend: {}", s))
    })
}

/// Parse an `AliasType` discriminant, returning a rusqlite error on unknown values.
fn parse_alias_type(s: &str) -> std::result::Result<AliasType, rusqlite::Error> {
    AliasType::from_db_value(s).ok_or_else(|| {
        rusqlite::Error::InvalidParameterName(format!("unknown alias_type: {}", s))
    })
}

/// Parse a `TransactionStatus` discriminant.
fn parse_tx_status(s: &str) -> std::result::Result<TransactionStatus, rusqlite::Error> {
    TransactionStatus::from_db_value(s).ok_or_else(|| {
        rusqlite::Error::InvalidParameterName(format!("unknown tx_status: {}", s))
    })
}

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

    pub fn from_db_value(s: &str) -> Option<Self> {
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

    pub fn from_db_value(s: &str) -> Option<Self> {
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
    RolledBack,
}

impl TransactionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TransactionStatus::Pending => "pending",
            TransactionStatus::Copied => "copied",
            TransactionStatus::Committed => "committed",
            TransactionStatus::Failed => "failed",
            TransactionStatus::RolledBack => "rolled_back",
        }
    }

    pub fn from_db_value(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(TransactionStatus::Pending),
            "copied" => Some(TransactionStatus::Copied),
            "committed" => Some(TransactionStatus::Committed),
            "failed" => Some(TransactionStatus::Failed),
            "rolled_back" => Some(TransactionStatus::RolledBack),
            _ => None,
        }
    }
}

/// WAL transaction record for crash recovery (v2 schema)
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
    // ── v2 extended fields ──────────────────────────────────────────────────
    /// Fine-grained operation type (e.g. "dedup", "hf_download_commit", …)
    pub op_type: Option<String>,
    /// JSON array of paths affected by this transaction
    pub affected_paths: Option<String>,
    /// JSON rollback instructions (opaque to the DB layer)
    pub rollback_plan: Option<String>,
    /// RFC-3339 timestamp when the transaction ended (committed/failed/rolled-back)
    pub end_time: Option<String>,
    /// Human-readable error detail (only set when status is Failed)
    pub error_message: Option<String>,
}

/// Database connection manager
pub struct Database {
    conn: Connection,
    /// Absolute path to the `.db` file (used during schema migration for backup).
    db_path: std::path::PathBuf,
}

impl Database {
    /// Current schema version.  Bump this whenever you add a migration below.
    #[allow(dead_code)]
    const SCHEMA_VERSION: i64 = 2;

    /// Open or create database at the given path
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("Failed to open database at {}", path.display()))?;

        // Enable WAL mode for concurrent access
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        // Wait up to 5 s when another connection holds a write lock (prevents
        // immediate "database is locked" errors when CLI and webui/proxy run
        // concurrently).
        conn.pragma_update(None, "busy_timeout", 5000i64)?;

        let mut db = Self { conn, db_path: path.to_path_buf() };
        db.init_schema()?;

        Ok(db)
    }

    /// Initialize / migrate database schema.
    ///
    /// Uses `PRAGMA user_version` as a monotonically-increasing schema version
    /// counter.
    ///
    /// | DB state          | Action                                        |
    /// |---|---|
    /// | user_version == 0 | Create all tables at the current v2 schema    |
    /// | user_version == 1 | Backup + run v1→v2 migration                  |
    /// | user_version >= 2 | Nothing to do                                 |
    fn init_schema(&mut self) -> Result<()> {
        // Temporarily disable FK enforcement so we can create tables in any order.
        self.conn.pragma_update(None, "foreign_keys", "OFF")?;

        let current_version: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap_or(0);

        if current_version == 0 {
            // ── Fresh database: create schema at v2 directly ─────────────────
            // The `wal_transactions` table is created WITHOUT the old
            // `CHECK (operation IN ('dedup','download','gc'))` constraint and
            // WITH the five v2 extra columns, so new databases never need
            // to go through the rename-copy migration path.
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
                id               INTEGER PRIMARY KEY AUTOINCREMENT,
                tx_id            TEXT    UNIQUE NOT NULL,
                operation        TEXT    NOT NULL,
                status           TEXT    NOT NULL,
                source_path      TEXT,
                target_hash      TEXT,
                metadata         TEXT,
                created_at       TEXT    NOT NULL DEFAULT (datetime('now')),
                updated_at       TEXT    NOT NULL DEFAULT (datetime('now')),
                -- v2 additions
                op_type          TEXT,
                affected_paths   TEXT,
                rollback_plan    TEXT,
                end_time         TEXT,
                error_message    TEXT,

                CHECK (status IN ('pending', 'copied', 'committed', 'failed', 'rolled_back'))
            );

            CREATE INDEX IF NOT EXISTS idx_wal_status    ON wal_transactions(status);
            CREATE INDEX IF NOT EXISTS idx_wal_created   ON wal_transactions(created_at);
            CREATE INDEX IF NOT EXISTS idx_wal_operation ON wal_transactions(operation);

            -- HuggingFace download records (Phase 3)
            CREATE TABLE IF NOT EXISTS downloads (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                model_hash TEXT,
                source_url TEXT NOT NULL,
                sha256_hash TEXT,
                repo_id TEXT,
                filename TEXT,
                revision TEXT DEFAULT 'main',
                status TEXT NOT NULL DEFAULT 'pending',
                bytes_total INTEGER DEFAULT 0,
                bytes_done INTEGER DEFAULT 0,
                started_at TEXT NOT NULL DEFAULT (datetime('now')),
                finished_at TEXT,
                error_message TEXT,

                FOREIGN KEY (model_hash) REFERENCES models(blake3_hash),
                CHECK (status IN ('pending', 'downloading', 'done', 'failed'))
            );

            CREATE INDEX IF NOT EXISTS idx_downloads_model_hash ON downloads(model_hash);
            CREATE INDEX IF NOT EXISTS idx_downloads_sha256 ON downloads(sha256_hash);
            CREATE INDEX IF NOT EXISTS idx_downloads_status ON downloads(status);
            CREATE INDEX IF NOT EXISTS idx_downloads_url ON downloads(source_url);

            -- HF SHA256 <-> BLAKE3 mapping table (Phase 3)
            CREATE TABLE IF NOT EXISTS hf_mappings (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                sha256_hash TEXT UNIQUE NOT NULL,
                blake3_hash TEXT NOT NULL,
                repo_id TEXT,
                filename TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),

                FOREIGN KEY (blake3_hash) REFERENCES models(blake3_hash),
                CHECK (length(sha256_hash) = 64),
                CHECK (length(blake3_hash) = 64)
            );

            CREATE INDEX IF NOT EXISTS idx_hf_mappings_sha256 ON hf_mappings(sha256_hash);
            CREATE INDEX IF NOT EXISTS idx_hf_mappings_blake3 ON hf_mappings(blake3_hash);

            -- Workflow files (Phase 4)
            CREATE TABLE IF NOT EXISTS workflows (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                path TEXT UNIQUE NOT NULL,
                -- BLAKE3 hash of the workflow JSON file itself
                file_hash TEXT,
                title TEXT,
                parsed_at TEXT NOT NULL DEFAULT (datetime('now')),
                last_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
                ref_count INTEGER NOT NULL DEFAULT 0
            );

            CREATE INDEX IF NOT EXISTS idx_workflows_path ON workflows(path);
            CREATE INDEX IF NOT EXISTS idx_workflows_file_hash ON workflows(file_hash);

            -- Workflow → Model dependency edges (Phase 4)
            CREATE TABLE IF NOT EXISTS workflow_refs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                workflow_id INTEGER NOT NULL,
                model_hash TEXT,              -- NULL if unresolved
                ref_type TEXT NOT NULL,       -- 'checkpoint','lora','vae','clip','controlnet','ipadapter','unet','unknown'
                model_name TEXT NOT NULL,     -- raw name from workflow JSON
                model_path TEXT,              -- resolved absolute path (if found)
                resolved INTEGER NOT NULL DEFAULT 0,  -- 1 = matched to CAS
                created_at TEXT NOT NULL DEFAULT (datetime('now')),

                FOREIGN KEY (workflow_id) REFERENCES workflows(id) ON DELETE CASCADE,
                FOREIGN KEY (model_hash) REFERENCES models(blake3_hash),
                CHECK (ref_type IN ('checkpoint','lora','vae','clip','controlnet','ipadapter','unet','unknown'))
            );

            CREATE INDEX IF NOT EXISTS idx_workflow_refs_workflow ON workflow_refs(workflow_id);
            CREATE INDEX IF NOT EXISTS idx_workflow_refs_model ON workflow_refs(model_hash);
            CREATE INDEX IF NOT EXISTS idx_workflow_refs_type ON workflow_refs(ref_type);
            "#,
        )?;
            // Brand-new database: stamp directly at v2 (no migration needed)
            self.conn.pragma_update(None, "user_version", 2i64)?;
        } else if current_version == 1 {
            // ── Migration v1 → v2 ────────────────────────────────────────────
            // Adds extended transaction metadata columns to `wal_transactions`
            // and removes the restrictive `CHECK (operation IN (...))` so that
            // new op types (StoreMigration, AliasRewrite, …) can be recorded.
            //
            // SQLite does not support ALTER TABLE DROP CONSTRAINT, so we use
            // the recommended "rename → recreate → copy → drop" approach.
            self.backup_before_migration(1)?;

            self.conn.execute_batch(
                r#"
                -- Step 1: rename old table
                ALTER TABLE wal_transactions RENAME TO wal_transactions_v1;

                -- Step 2: create new table without the restrictive CHECK and with new columns
                CREATE TABLE wal_transactions (
                    id               INTEGER PRIMARY KEY AUTOINCREMENT,
                    tx_id            TEXT    UNIQUE NOT NULL,
                    operation        TEXT    NOT NULL,
                    status           TEXT    NOT NULL,
                    source_path      TEXT,
                    target_hash      TEXT,
                    metadata         TEXT,
                    created_at       TEXT    NOT NULL DEFAULT (datetime('now')),
                    updated_at       TEXT    NOT NULL DEFAULT (datetime('now')),
                    -- v2 additions
                    op_type          TEXT,
                    affected_paths   TEXT,
                    rollback_plan    TEXT,
                    end_time         TEXT,
                    error_message    TEXT,

                    CHECK (status IN ('pending', 'copied', 'committed', 'failed', 'rolled_back'))
                );

                -- Step 3: copy existing rows (new columns default to NULL)
                INSERT INTO wal_transactions
                    (id, tx_id, operation, status, source_path, target_hash,
                     metadata, created_at, updated_at)
                SELECT  id, tx_id, operation, status, source_path, target_hash,
                        metadata, created_at, updated_at
                FROM wal_transactions_v1;

                -- Step 4: drop backup table
                DROP TABLE wal_transactions_v1;

                -- Recreate indexes on the new table
                CREATE INDEX IF NOT EXISTS idx_wal_status    ON wal_transactions(status);
                CREATE INDEX IF NOT EXISTS idx_wal_created   ON wal_transactions(created_at);
                CREATE INDEX IF NOT EXISTS idx_wal_operation ON wal_transactions(operation);
                "#,
            )?;

            self.conn.pragma_update(None, "user_version", 2i64)?;
        }
        // current_version >= 2: schema is already up to date

        // Re-enable FK enforcement
        self.conn.pragma_update(None, "foreign_keys", "ON")?;


        Ok(())
    }

    /// Copy the database file to `<store>/backups/modeld_backup_v<from_version>_<ts>.db`
    /// before running a migration.  The store root is derived as the parent of the
    /// directory that contains the `.db` file (`<store>/modeld.db` → `<store>`).
    ///
    /// Uses SQLite's `VACUUM INTO` to create a consistent snapshot of the live
    /// database without closing the connection (safe on all platforms).
    fn backup_before_migration(&self, from_version: u32) -> Result<()> {
        // Determine store root: the directory that contains the db file.
        let store_root = self
            .db_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Cannot determine store root from DB path"))?;

        let backups_dir = store_root.join("backups");
        std::fs::create_dir_all(&backups_dir)
            .with_context(|| format!("Failed to create backups dir: {}", backups_dir.display()))?;

        let ts = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
        let backup_name = format!("modeld_backup_v{}_{}.db", from_version, ts);
        let backup_path = backups_dir.join(&backup_name);

        // `VACUUM INTO` creates a compact, self-contained copy of the database
        // without requiring the file to be closed — safe on Windows and Unix.
        let backup_sql = format!(
            "VACUUM INTO '{}'",
            backup_path.to_string_lossy().replace('\'', "''")
        );
        self.conn
            .execute_batch(&backup_sql)
            .with_context(|| {
                format!(
                    "Failed to backup database to {}",
                    backup_path.display()
                )
            })?;

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
                let h: String = row.get(1)?;
                let created: String = row.get(7)?;
                let last: String = row.get(8)?;
                let qat: Option<String> = row.get(9)?;
                Ok(Model {
                    id: row.get(0)?,
                    blake3_hash: parse_blake3(&h)?,
                    size_bytes: row.get(2)?,
                    format: row.get(3)?,
                    arch: row.get(4)?,
                    category: row.get(5)?,
                    base_model: row.get(6)?,
                    created_at: parse_dt(&created),
                    last_seen: parse_dt(&last),
                    quarantined_at: qat.as_deref().map(parse_dt),
                })
            })
            .optional()?;

        Ok(model)
    }

    /// Get total count of models
    pub fn count_models(&self) -> Result<i64> {
        let count: i64 =
            self.conn.query_row("SELECT COUNT(*) FROM models", [], |row| row.get(0))?;
        Ok(count)
    }

    /// Get total size of all models
    pub fn total_size(&self) -> Result<i64> {
        let size: i64 =
            self.conn.query_row("SELECT COALESCE(SUM(size_bytes), 0) FROM models", [], |row| {
                row.get(0)
            })?;
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
                let h: String = row.get(1)?;
                let created: String = row.get(7)?;
                let last: String = row.get(8)?;
                let qat: Option<String> = row.get(9)?;
                Ok(Model {
                    id: row.get(0)?,
                    blake3_hash: parse_blake3(&h)?,
                    size_bytes: row.get(2)?,
                    format: row.get(3)?,
                    arch: row.get(4)?,
                    category: row.get(5)?,
                    base_model: row.get(6)?,
                    created_at: parse_dt(&created),
                    last_seen: parse_dt(&last),
                    quarantined_at: qat.as_deref().map(parse_dt),
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
            params![model_hash.as_hex(), path, frontend.as_str(), alias_type.as_str(), now],
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
                let h: String = row.get(1)?;
                let fe: String = row.get(3)?;
                let at: String = row.get(4)?;
                let dt: String = row.get(5)?;
                Ok(Alias {
                    id: row.get(0)?,
                    model_hash: parse_blake3(&h)?,
                    path: row.get(2)?,
                    frontend: parse_frontend(&fe)?,
                    alias_type: parse_alias_type(&at)?,
                    created_at: parse_dt(&dt),
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
                let h: String = row.get(1)?;
                let fe: String = row.get(3)?;
                let at: String = row.get(4)?;
                let dt: String = row.get(5)?;
                Ok(Alias {
                    id: row.get(0)?,
                    model_hash: parse_blake3(&h)?,
                    path: row.get(2)?,
                    frontend: parse_frontend(&fe)?,
                    alias_type: parse_alias_type(&at)?,
                    created_at: parse_dt(&dt),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(aliases)
    }

    /// Delete alias by path
    pub fn delete_alias(&mut self, path: &str) -> Result<()> {
        self.conn.execute("DELETE FROM aliases WHERE path = ?1", params![path])?;
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
                   metadata, created_at, updated_at,
                   op_type, affected_paths, rollback_plan, end_time, error_message
            FROM wal_transactions
            WHERE tx_id = ?1
            "#,
        )?;

        let tx = stmt
            .query_row(params![tx_id], |row| {
                let st: String = row.get(3)?;
                let c: String = row.get(7)?;
                let u: String = row.get(8)?;
                Ok(WalTransaction {
                    id: row.get(0)?,
                    tx_id: row.get(1)?,
                    operation: row.get(2)?,
                    status: parse_tx_status(&st)?,
                    source_path: row.get(4)?,
                    target_hash: row.get(5)?,
                    metadata: row.get(6)?,
                    created_at: parse_dt(&c),
                    updated_at: parse_dt(&u),
                    op_type: row.get(9)?,
                    affected_paths: row.get(10)?,
                    rollback_plan: row.get(11)?,
                    end_time: row.get(12)?,
                    error_message: row.get(13)?,
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
                   metadata, created_at, updated_at,
                   op_type, affected_paths, rollback_plan, end_time, error_message
            FROM wal_transactions
            WHERE status IN ('pending', 'copied')
            ORDER BY created_at ASC
            "#,
        )?;

        let transactions = stmt
            .query_map([], |row| {
                let st: String = row.get(3)?;
                let c: String = row.get(7)?;
                let u: String = row.get(8)?;
                Ok(WalTransaction {
                    id: row.get(0)?,
                    tx_id: row.get(1)?,
                    operation: row.get(2)?,
                    status: parse_tx_status(&st)?,
                    source_path: row.get(4)?,
                    target_hash: row.get(5)?,
                    metadata: row.get(6)?,
                    created_at: parse_dt(&c),
                    updated_at: parse_dt(&u),
                    op_type: row.get(9)?,
                    affected_paths: row.get(10)?,
                    rollback_plan: row.get(11)?,
                    end_time: row.get(12)?,
                    error_message: row.get(13)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(transactions)
    }

    /// Delete WAL transaction by tx_id
    pub fn delete_wal_transaction(&mut self, tx_id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM wal_transactions WHERE tx_id = ?1", params![tx_id])?;
        Ok(())
    }

    /// Update the v2 extended fields for a transaction.
    ///
    /// Pass `None` for any field you do not want to overwrite.
    pub fn update_wal_extended(
        &mut self,
        tx_id: &str,
        op_type: Option<&str>,
        affected_paths: Option<&str>,
        rollback_plan: Option<&str>,
        end_time: Option<&str>,
        error_message: Option<&str>,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            r#"
            UPDATE wal_transactions SET
                op_type        = COALESCE(?2, op_type),
                affected_paths = COALESCE(?3, affected_paths),
                rollback_plan  = COALESCE(?4, rollback_plan),
                end_time       = COALESCE(?5, end_time),
                error_message  = COALESCE(?6, error_message),
                updated_at     = ?7
            WHERE tx_id = ?1
            "#,
            params![tx_id, op_type, affected_paths, rollback_plan, end_time, error_message, now],
        )?;
        Ok(())
    }

    /// List all WAL transactions ordered by creation time (newest first).
    pub fn list_wal_transactions(&self) -> Result<Vec<WalTransaction>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, tx_id, operation, status, source_path, target_hash,
                   metadata, created_at, updated_at,
                   op_type, affected_paths, rollback_plan, end_time, error_message
            FROM wal_transactions
            ORDER BY created_at DESC
            "#,
        )?;

        let transactions = stmt
            .query_map([], |row| {
                let st: String = row.get(3)?;
                let c: String = row.get(7)?;
                let u: String = row.get(8)?;
                Ok(WalTransaction {
                    id: row.get(0)?,
                    tx_id: row.get(1)?,
                    operation: row.get(2)?,
                    status: parse_tx_status(&st)?,
                    source_path: row.get(4)?,
                    target_hash: row.get(5)?,
                    metadata: row.get(6)?,
                    created_at: parse_dt(&c),
                    updated_at: parse_dt(&u),
                    op_type: row.get(9)?,
                    affected_paths: row.get(10)?,
                    rollback_plan: row.get(11)?,
                    end_time: row.get(12)?,
                    error_message: row.get(13)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(transactions)
    }

    /// Return the current `PRAGMA user_version` (schema version).
    pub fn schema_version(&self) -> Result<i64> {
        Ok(self.conn.pragma_query_value(None, "user_version", |r| r.get(0))?)
    }

    // ─────────────────────────────────────────────────────────────────────────
    // GC helpers
    // ─────────────────────────────────────────────────────────────────────────

    /// Mark a model as quarantined (sets `quarantined_at` timestamp).
    pub fn quarantine_model(&mut self, hash: &Blake3Hash, at: DateTime<Utc>) -> Result<()> {
        self.conn.execute(
            "UPDATE models SET quarantined_at = ?1 WHERE blake3_hash = ?2",
            params![at.to_rfc3339(), hash.as_hex()],
        )?;
        Ok(())
    }

    /// Delete all aliases for a model (call after quarantine so aliases
    /// no longer point at a non-existent file).
    pub fn delete_aliases_for_model(&mut self, hash: &Blake3Hash) -> Result<usize> {
        let n = self.conn.execute(
            "DELETE FROM aliases WHERE model_hash = ?1",
            params![hash.as_hex()],
        )?;
        Ok(n)
    }

    /// Return every alias in the table (used by workflow lookup and fsck).
    pub fn list_all_aliases(&self) -> Result<Vec<Alias>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, model_hash, path, frontend, alias_type, created_at
            FROM aliases
            ORDER BY model_hash, created_at ASC
            "#,
        )?;
        let aliases = stmt
            .query_map([], |row| {
                let h: String = row.get(1)?;
                let fe: String = row.get(3)?;
                let at: String = row.get(4)?;
                let dt: String = row.get(5)?;
                Ok(Alias {
                    id: row.get(0)?,
                    model_hash: parse_blake3(&h)?,
                    path: row.get(2)?,
                    frontend: parse_frontend(&fe)?,
                    alias_type: parse_alias_type(&at)?,
                    created_at: parse_dt(&dt),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(aliases)
    }
} // impl Database (model/alias/WAL/GC block)

// ─────────────────────────────────────────────────────────────────────────────
// Batch query helpers (eliminate N+1 patterns — audit 3.5)
// ─────────────────────────────────────────────────────────────────────────────

impl Database {
    /// Return duplicate groups in a single JOIN query.
    ///
    /// Replaces the previous `list_models()` + per-model `get_aliases_for_model()`
    /// loop (N+1 queries) with a single SQL pass.  Groups with fewer than 2
    /// aliases are excluded by the HAVING clause.
    pub fn find_duplicate_groups(&self) -> Result<Vec<(Blake3Hash, i64, Vec<Alias>)>> {
        // Find all model hashes that have ≥ 2 aliases
        let mut hash_stmt = self.conn.prepare(
            r#"
            SELECT m.blake3_hash, m.size_bytes
            FROM models m
            WHERE (SELECT COUNT(*) FROM aliases a WHERE a.model_hash = m.blake3_hash) >= 2
            ORDER BY m.blake3_hash
            "#,
        )?;

        let model_rows: Vec<(Blake3Hash, i64)> = hash_stmt
            .query_map([], |row| {
                let h: String = row.get(0)?;
                Ok((parse_blake3(&h)?, row.get(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        // Fetch all aliases in one query and group in Rust
        let mut alias_stmt = self.conn.prepare(
            r#"
            SELECT a.model_hash, a.id, a.path, a.frontend, a.alias_type, a.created_at
            FROM aliases a
            WHERE a.model_hash IN (
                SELECT model_hash FROM aliases GROUP BY model_hash HAVING COUNT(*) >= 2
            )
            ORDER BY a.model_hash, a.created_at ASC
            "#,
        )?;

        let alias_rows: Vec<Alias> = alias_stmt
            .query_map([], |row| {
                let mh: String = row.get(0)?;
                let fe: String = row.get(3)?;
                let at: String = row.get(4)?;
                let dt: String = row.get(5)?;
                Ok(Alias {
                    id: row.get(1)?,
                    model_hash: parse_blake3(&mh)?,
                    path: row.get(2)?,
                    frontend: parse_frontend(&fe)?,
                    alias_type: parse_alias_type(&at)?,
                    created_at: parse_dt(&dt),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        // Group aliases by model hash
        use std::collections::HashMap;
        let mut by_hash: HashMap<String, Vec<Alias>> = HashMap::new();
        for alias in alias_rows {
            by_hash.entry(alias.model_hash.as_hex().to_string()).or_default().push(alias);
        }

        let groups = model_rows
            .into_iter()
            .filter_map(|(hash, size)| {
                by_hash
                    .remove(hash.as_hex())
                    .filter(|aliases| aliases.len() >= 2)
                    .map(|aliases| (hash, size, aliases))
            })
            .collect();

        Ok(groups)
    }

    /// Return GC candidate summary in a single GROUP-BY query.
    ///
    /// Each row contains: (model, alias_count, workflow_ref_count).
    /// Replaces the previous 3N-query loop in `gc.candidates()`.
    pub fn get_gc_candidate_counts(&self) -> Result<Vec<(Model, usize, usize)>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT
                m.id, m.blake3_hash, m.size_bytes, m.format, m.arch,
                m.category, m.base_model, m.created_at, m.last_seen, m.quarantined_at,
                COUNT(DISTINCT a.id)  AS alias_count,
                COUNT(DISTINCT wr.id) AS workflow_ref_count
            FROM models m
            LEFT JOIN aliases a  ON a.model_hash  = m.blake3_hash
            LEFT JOIN workflow_refs wr ON wr.model_hash = m.blake3_hash
            GROUP BY m.id
            ORDER BY m.created_at DESC
            "#,
        )?;

        let rows = stmt
            .query_map([], |row| {
                let h: String = row.get(1)?;
                let created: String = row.get(7)?;
                let last: String = row.get(8)?;
                let qat: Option<String> = row.get(9)?;
                let alias_count: i64 = row.get(10)?;
                let wf_count: i64 = row.get(11)?;
                Ok((
                    Model {
                        id: row.get(0)?,
                        blake3_hash: parse_blake3(&h)?,
                        size_bytes: row.get(2)?,
                        format: row.get(3)?,
                        arch: row.get(4)?,
                        category: row.get(5)?,
                        base_model: row.get(6)?,
                        created_at: parse_dt(&created),
                        last_seen: parse_dt(&last),
                        quarantined_at: qat.as_deref().map(parse_dt),
                    },
                    alias_count as usize,
                    wf_count as usize,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    /// Return all indexed (path, hash, size_bytes) pairs for incremental scan.
    ///
    /// The caller compares current file size against `size_bytes`; if they
    /// match the file is assumed unchanged and re-hashing is skipped.
    pub fn get_all_indexed_paths(&self) -> Result<std::collections::HashMap<String, (Blake3Hash, i64)>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT a.path, a.model_hash, m.size_bytes
            FROM aliases a
            JOIN models m ON m.blake3_hash = a.model_hash
            "#,
        )?;
        let mut map = std::collections::HashMap::new();
        let rows = stmt.query_map([], |row| {
            let path: String = row.get(0)?;
            let mh: String = row.get(1)?;
            let size: i64 = row.get(2)?;
            Ok((path, parse_blake3(&mh)?, size))
        })?;
        for row in rows {
            let (path, hash, size) = row?;
            map.insert(path, (hash, size));
        }
        Ok(map)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Download record types (Phase 3 — HF interception)
// ─────────────────────────────────────────────────────────────────────────────

/// Download status
#[derive(Debug, Clone, PartialEq)]
pub enum DownloadStatus {
    Pending,
    Downloading,
    Done,
    Failed,
}

impl DownloadStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            DownloadStatus::Pending => "pending",
            DownloadStatus::Downloading => "downloading",
            DownloadStatus::Done => "done",
            DownloadStatus::Failed => "failed",
        }
    }

    pub fn from_db_value(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(DownloadStatus::Pending),
            "downloading" => Some(DownloadStatus::Downloading),
            "done" => Some(DownloadStatus::Done),
            "failed" => Some(DownloadStatus::Failed),
            _ => None,
        }
    }
}

/// Download record from the downloads table
#[derive(Debug, Clone)]
pub struct Download {
    pub id: i64,
    pub model_hash: Option<String>,
    pub source_url: String,
    pub sha256_hash: Option<String>,
    pub repo_id: Option<String>,
    pub filename: Option<String>,
    pub revision: String,
    pub status: DownloadStatus,
    pub bytes_total: i64,
    pub bytes_done: i64,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error_message: Option<String>,
}

/// HF SHA256 ↔ BLAKE3 mapping record
#[derive(Debug, Clone)]
pub struct HfMapping {
    pub id: i64,
    pub sha256_hash: String,
    pub blake3_hash: String,
    pub repo_id: Option<String>,
    pub filename: Option<String>,
    pub created_at: DateTime<Utc>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Download CRUD
// ─────────────────────────────────────────────────────────────────────────────

impl Database {
    /// Insert a new download record
    pub fn insert_download(
        &mut self,
        source_url: &str,
        repo_id: Option<&str>,
        filename: Option<&str>,
        revision: Option<&str>,
    ) -> Result<i64> {
        let now = Utc::now().to_rfc3339();
        let rev = revision.unwrap_or("main");
        self.conn.execute(
            r#"
            INSERT INTO downloads (source_url, repo_id, filename, revision, status, started_at)
            VALUES (?1, ?2, ?3, ?4, 'pending', ?5)
            "#,
            params![source_url, repo_id, filename, rev, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Update download status and progress
    pub fn update_download_progress(
        &mut self,
        id: i64,
        status: DownloadStatus,
        bytes_done: i64,
        bytes_total: i64,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            r#"
            UPDATE downloads SET status = ?2, bytes_done = ?3, bytes_total = ?4,
                finished_at = CASE WHEN ?2 IN ('done','failed') THEN ?5 ELSE finished_at END
            WHERE id = ?1
            "#,
            params![id, status.as_str(), bytes_done, bytes_total, now],
        )?;
        Ok(())
    }

    /// Mark download as done with hash information
    pub fn complete_download(
        &mut self,
        id: i64,
        model_hash: &str,
        sha256_hash: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            r#"
            UPDATE downloads SET
                status = 'done',
                model_hash = ?2,
                sha256_hash = ?3,
                finished_at = ?4,
                bytes_done = bytes_total
            WHERE id = ?1
            "#,
            params![id, model_hash, sha256_hash, now],
        )?;
        Ok(())
    }

    /// Mark download as failed
    pub fn fail_download(&mut self, id: i64, error: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            r#"
            UPDATE downloads SET status = 'failed', error_message = ?2, finished_at = ?3
            WHERE id = ?1
            "#,
            params![id, error, now],
        )?;
        Ok(())
    }

    /// Look up a download by SHA256 hash (HF dedup check)
    pub fn get_download_by_sha256(&self, sha256: &str) -> Result<Option<Download>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, model_hash, source_url, sha256_hash, repo_id, filename, revision,
                   status, bytes_total, bytes_done, started_at, finished_at, error_message
            FROM downloads
            WHERE sha256_hash = ?1 AND status = 'done'
            ORDER BY finished_at DESC
            LIMIT 1
            "#,
        )?;
        parse_download_row(&mut stmt, params![sha256])
    }

    /// Get a download record by ID
    pub fn get_download(&self, id: i64) -> Result<Option<Download>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, model_hash, source_url, sha256_hash, repo_id, filename, revision,
                   status, bytes_total, bytes_done, started_at, finished_at, error_message
            FROM downloads WHERE id = ?1
            "#,
        )?;
        parse_download_row(&mut stmt, params![id])
    }

    /// List downloads by status
    pub fn list_downloads(&self, status: Option<DownloadStatus>) -> Result<Vec<Download>> {
        let sql = match &status {
            Some(_) => {
                "SELECT id, model_hash, source_url, sha256_hash, repo_id, filename, revision, \
                 status, bytes_total, bytes_done, started_at, finished_at, error_message \
                 FROM downloads WHERE status = ?1 ORDER BY started_at DESC"
            }
            None => {
                "SELECT id, model_hash, source_url, sha256_hash, repo_id, filename, revision, \
                 status, bytes_total, bytes_done, started_at, finished_at, error_message \
                 FROM downloads ORDER BY started_at DESC"
            }
        };

        let mut stmt = self.conn.prepare(sql)?;

        let mapper = |row: &rusqlite::Row<'_>| {
            let status_str: String = row.get(7)?;
            let started_str: String = row.get(10)?;
            let finished_str: Option<String> = row.get(11)?;
            Ok(Download {
                id: row.get(0)?,
                model_hash: row.get(1)?,
                source_url: row.get(2)?,
                sha256_hash: row.get(3)?,
                repo_id: row.get(4)?,
                filename: row.get(5)?,
                revision: row.get::<_, Option<String>>(6)?.unwrap_or_else(|| "main".into()),
                status: DownloadStatus::from_db_value(&status_str)
                    .unwrap_or(DownloadStatus::Pending),
                bytes_total: row.get(8)?,
                bytes_done: row.get(9)?,
                started_at: started_str.parse::<DateTime<Utc>>().unwrap_or_else(|_| Utc::now()),
                finished_at: finished_str.and_then(|s| s.parse::<DateTime<Utc>>().ok()),
                error_message: row.get(12)?,
            })
        };

        let rows: Vec<Download> = match &status {
            Some(s) => {
                stmt.query_map(params![s.as_str()], mapper)?.collect::<rusqlite::Result<_>>()?
            }
            None => stmt.query_map([], mapper)?.collect::<rusqlite::Result<_>>()?,
        };
        Ok(rows)
    }

    // ─────────────────────────────────────────────────────────────────────────
    // HF Mapping CRUD (SHA256 ↔ BLAKE3)
    // ─────────────────────────────────────────────────────────────────────────

    /// Insert or ignore a SHA256↔BLAKE3 mapping
    pub fn upsert_hf_mapping(
        &mut self,
        sha256: &str,
        blake3: &str,
        repo_id: Option<&str>,
        filename: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            r#"
            INSERT OR IGNORE INTO hf_mappings (sha256_hash, blake3_hash, repo_id, filename, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![sha256, blake3, repo_id, filename, now],
        )?;
        Ok(())
    }

    /// Look up BLAKE3 hash by SHA256 (HF → CAS resolution)
    pub fn get_blake3_by_sha256(&self, sha256: &str) -> Result<Option<HfMapping>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, sha256_hash, blake3_hash, repo_id, filename, created_at
            FROM hf_mappings WHERE sha256_hash = ?1
            "#,
        )?;
        let mapping = stmt
            .query_row(params![sha256], |row| {
                let created_str: String = row.get(5)?;
                Ok(HfMapping {
                    id: row.get(0)?,
                    sha256_hash: row.get(1)?,
                    blake3_hash: row.get(2)?,
                    repo_id: row.get(3)?,
                    filename: row.get(4)?,
                    created_at: created_str.parse::<DateTime<Utc>>().unwrap_or_else(|_| Utc::now()),
                })
            })
            .optional()?;
        Ok(mapping)
    }

    /// Look up all SHA256s for a given BLAKE3 hash
    pub fn get_sha256_by_blake3(&self, blake3: &str) -> Result<Vec<HfMapping>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, sha256_hash, blake3_hash, repo_id, filename, created_at
            FROM hf_mappings WHERE blake3_hash = ?1
            "#,
        )?;
        let rows: Vec<HfMapping> = stmt
            .query_map(params![blake3], |row| {
                let created_str: String = row.get(5)?;
                Ok(HfMapping {
                    id: row.get(0)?,
                    sha256_hash: row.get(1)?,
                    blake3_hash: row.get(2)?,
                    repo_id: row.get(3)?,
                    filename: row.get(4)?,
                    created_at: created_str.parse::<DateTime<Utc>>().unwrap_or_else(|_| Utc::now()),
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }
}

// Helper to parse a Download row from a prepared statement
fn parse_download_row(
    stmt: &mut rusqlite::Statement<'_>,
    params: impl rusqlite::Params,
) -> Result<Option<Download>> {
    let row = stmt
        .query_row(params, |row| {
            let status_str: String = row.get(7)?;
            let started_str: String = row.get(10)?;
            let finished_str: Option<String> = row.get(11)?;
            Ok(Download {
                id: row.get(0)?,
                model_hash: row.get(1)?,
                source_url: row.get(2)?,
                sha256_hash: row.get(3)?,
                repo_id: row.get(4)?,
                filename: row.get(5)?,
                revision: row.get::<_, Option<String>>(6)?.unwrap_or_else(|| "main".into()),
                status: DownloadStatus::from_db_value(&status_str)
                    .unwrap_or(DownloadStatus::Pending),
                bytes_total: row.get(8)?,
                bytes_done: row.get(9)?,
                started_at: started_str.parse::<DateTime<Utc>>().unwrap_or_else(|_| Utc::now()),
                finished_at: finished_str.and_then(|s| s.parse::<DateTime<Utc>>().ok()),
                error_message: row.get(12)?,
            })
        })
        .optional()?;
    Ok(row)
}

// ──────────────────────────────────────────────────────────────────────────
// Phase 4: Workflow & Reference Graph
// ──────────────────────────────────────────────────────────────────────────

/// A parsed ComfyUI (or other format) workflow file
#[derive(Debug, Clone)]
pub struct WorkflowRecord {
    pub id: i64,
    pub path: String,
    pub file_hash: Option<String>,
    pub title: Option<String>,
    pub parsed_at: String,
    pub last_seen_at: String,
    pub ref_count: i64,
}

/// A single model reference extracted from a workflow
#[derive(Debug, Clone)]
pub struct WorkflowRef {
    pub id: i64,
    pub workflow_id: i64,
    pub model_hash: Option<String>,
    pub ref_type: String,
    pub model_name: String,
    pub model_path: Option<String>,
    pub resolved: bool,
}

impl Database {
    /// Upsert a workflow record; returns the workflow ID.
    pub fn upsert_workflow(
        &self,
        path: &str,
        file_hash: Option<&str>,
        title: Option<&str>,
    ) -> Result<i64> {
        self.conn.execute(
            r#"INSERT INTO workflows (path, file_hash, title, parsed_at, last_seen_at)
               VALUES (?1, ?2, ?3, datetime('now'), datetime('now'))
               ON CONFLICT(path) DO UPDATE SET
                 file_hash    = excluded.file_hash,
                 title        = excluded.title,
                 parsed_at    = excluded.parsed_at,
                 last_seen_at = excluded.last_seen_at"#,
            rusqlite::params![path, file_hash, title],
        )?;
        Ok(self.conn.last_insert_rowid().max(self.conn.query_row(
            "SELECT id FROM workflows WHERE path = ?1",
            rusqlite::params![path],
            |r| r.get::<_, i64>(0),
        )?))
    }

    /// Delete all workflow_refs for a workflow before re-inserting (refresh).
    pub fn clear_workflow_refs(&self, workflow_id: i64) -> Result<()> {
        self.conn.execute(
            "DELETE FROM workflow_refs WHERE workflow_id = ?1",
            rusqlite::params![workflow_id],
        )?;
        Ok(())
    }

    /// Insert a single workflow → model reference edge.
    pub fn insert_workflow_ref(
        &self,
        workflow_id: i64,
        model_hash: Option<&str>,
        ref_type: &str,
        model_name: &str,
        model_path: Option<&str>,
        resolved: bool,
    ) -> Result<i64> {
        self.conn.execute(
            r#"INSERT INTO workflow_refs
               (workflow_id, model_hash, ref_type, model_name, model_path, resolved)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
            rusqlite::params![
                workflow_id,
                model_hash,
                ref_type,
                model_name,
                model_path,
                resolved as i64,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Update workflow ref_count to reflect current edge count
    pub fn update_workflow_ref_count(&self, workflow_id: i64) -> Result<()> {
        self.conn.execute(
            r#"UPDATE workflows SET ref_count = (
                SELECT COUNT(*) FROM workflow_refs WHERE workflow_id = ?1
               ) WHERE id = ?1"#,
            rusqlite::params![workflow_id],
        )?;
        Ok(())
    }

    /// Get all workflow_refs that reference a given model (by blake3 hash).
    pub fn get_workflows_for_model(&self, blake3: &str) -> Result<Vec<WorkflowRef>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT wr.id, wr.workflow_id, wr.model_hash, wr.ref_type,
                      wr.model_name, wr.model_path, wr.resolved
               FROM workflow_refs wr
               WHERE wr.model_hash = ?1"#,
        )?;
        let rows = stmt.query_map(rusqlite::params![blake3], |r| {
            Ok(WorkflowRef {
                id: r.get(0)?,
                workflow_id: r.get(1)?,
                model_hash: r.get(2)?,
                ref_type: r.get(3)?,
                model_name: r.get(4)?,
                model_path: r.get(5)?,
                resolved: r.get::<_, i64>(6)? != 0,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// Get all model refs for a given workflow path.
    pub fn get_refs_for_workflow(&self, path: &str) -> Result<Vec<WorkflowRef>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT wr.id, wr.workflow_id, wr.model_hash, wr.ref_type,
                      wr.model_name, wr.model_path, wr.resolved
               FROM workflow_refs wr
               JOIN workflows w ON wr.workflow_id = w.id
               WHERE w.path = ?1"#,
        )?;
        let rows = stmt.query_map(rusqlite::params![path], |r| {
            Ok(WorkflowRef {
                id: r.get(0)?,
                workflow_id: r.get(1)?,
                model_hash: r.get(2)?,
                ref_type: r.get(3)?,
                model_name: r.get(4)?,
                model_path: r.get(5)?,
                resolved: r.get::<_, i64>(6)? != 0,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// List all known workflows.
    pub fn list_workflows(&self) -> Result<Vec<WorkflowRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, file_hash, title, parsed_at, last_seen_at, ref_count FROM workflows ORDER BY last_seen_at DESC"
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(WorkflowRecord {
                id: r.get(0)?,
                path: r.get(1)?,
                file_hash: r.get(2)?,
                title: r.get(3)?,
                parsed_at: r.get(4)?,
                last_seen_at: r.get(5)?,
                ref_count: r.get(6)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// Return models that have zero workflow_refs (candidates for GC).
    pub fn orphan_models(&self) -> Result<Vec<Model>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT m.id, m.blake3_hash, m.size_bytes, m.format,
                      m.arch, m.category, m.base_model, m.created_at, m.last_seen, m.quarantined_at
               FROM models m
               WHERE NOT EXISTS (
                   SELECT 1 FROM workflow_refs wr WHERE wr.model_hash = m.blake3_hash
               )
               ORDER BY m.size_bytes DESC"#,
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Model {
                id: r.get(0)?,
                blake3_hash: Blake3Hash::from_hex(&r.get::<_, String>(1)?).unwrap(),
                size_bytes: r.get(2)?,
                format: r.get(3)?,
                arch: r.get(4)?,
                category: r.get(5)?,
                base_model: r.get(6)?,
                created_at: DateTime::parse_from_rfc3339(&r.get::<_, String>(7)?)
                    .unwrap()
                    .with_timezone(&Utc),
                last_seen: DateTime::parse_from_rfc3339(&r.get::<_, String>(8)?)
                    .unwrap()
                    .with_timezone(&Utc),
                quarantined_at: r
                    .get::<_, Option<String>>(9)?
                    .map(|s| DateTime::parse_from_rfc3339(&s).unwrap().with_timezone(&Utc)),
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// Return workflow_refs that could not be resolved to a CAS model.
    pub fn unresolved_refs(&self) -> Result<Vec<WorkflowRef>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT id, workflow_id, model_hash, ref_type, model_name, model_path, resolved
               FROM workflow_refs WHERE resolved = 0"#,
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(WorkflowRef {
                id: r.get(0)?,
                workflow_id: r.get(1)?,
                model_hash: r.get(2)?,
                ref_type: r.get(3)?,
                model_name: r.get(4)?,
                model_path: r.get(5)?,
                resolved: false,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// Get a workflow record by path.
    pub fn get_workflow(&self, path: &str) -> Result<Option<WorkflowRecord>> {
        let result = self.conn.query_row(
            "SELECT id, path, file_hash, title, parsed_at, last_seen_at, ref_count FROM workflows WHERE path = ?1",
            rusqlite::params![path],
            |r| {
                Ok(WorkflowRecord {
                    id: r.get(0)?,
                    path: r.get(1)?,
                    file_hash: r.get(2)?,
                    title: r.get(3)?,
                    parsed_at: r.get(4)?,
                    last_seen_at: r.get(5)?,
                    ref_count: r.get(6)?,
                })
            },
        );
        match result {
            Ok(r) => Ok(Some(r)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
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
        db.insert_or_update_model(&hash, 2048, Some("gguf"), None, None, None).unwrap();

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
            db.insert_or_update_model(&hash, 1024, None, None, None, None).unwrap();
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
        db.insert_or_update_model(&hash, 1024, Some("safetensors"), None, None, None).unwrap();

        // Insert alias
        db.insert_alias(&hash, "C:\\models\\sdxl.safetensors", Frontend::User, AliasType::Original)
            .unwrap();

        // Retrieve by path
        let alias = db.get_alias_by_path("C:\\models\\sdxl.safetensors").unwrap().unwrap();

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
        db.insert_or_update_model(&hash, 2048, None, None, None, None).unwrap();

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
        db.insert_or_update_model(&hash, 512, None, None, None, None).unwrap();
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
        db.update_wal_status(tx_id, TransactionStatus::Copied).unwrap();

        let tx = db.get_wal_transaction(tx_id).unwrap().unwrap();
        assert_eq!(tx.status, TransactionStatus::Copied);

        // Update to committed
        db.update_wal_status(tx_id, TransactionStatus::Committed).unwrap();

        let tx = db.get_wal_transaction(tx_id).unwrap().unwrap();
        assert_eq!(tx.status, TransactionStatus::Committed);
    }

    #[test]
    fn test_get_incomplete_wal_transactions() {
        let temp_db = NamedTempFile::new().unwrap();
        let mut db = Database::open(temp_db.path()).unwrap();

        // Insert various transactions
        db.insert_wal_transaction("tx1", "dedup", TransactionStatus::Pending, None, None, None)
            .unwrap();

        db.insert_wal_transaction("tx2", "dedup", TransactionStatus::Copied, None, None, None)
            .unwrap();

        db.insert_wal_transaction("tx3", "dedup", TransactionStatus::Committed, None, None, None)
            .unwrap();

        db.insert_wal_transaction("tx4", "download", TransactionStatus::Failed, None, None, None)
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
