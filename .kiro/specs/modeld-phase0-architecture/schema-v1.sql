-- modeld SQLite Schema v1
-- Phase 0 - Architecture Design & RFC
-- 
-- This schema defines the complete database structure for modeld's metadata storage,
-- including models registry, filesystem aliases, workflow references, download tracking,
-- and write-ahead logging for crash recovery.

-- =============================================================================
-- Table: models
-- Purpose: Central Content-Addressable Storage (CAS) registry
-- =============================================================================
CREATE TABLE models (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    blake3_hash       TEXT UNIQUE NOT NULL,       -- 64-character hex hash (content address)
    size_bytes        INTEGER NOT NULL,            -- File size in bytes
    format            TEXT,                        -- safetensors | gguf | ckpt | bin | pt
    arch              TEXT,                        -- sd1 | sdxl | flux | llm | unknown
    base_model        TEXT,                        -- sd-v1-5 | sdxl-base | ...
    created_at        TEXT DEFAULT (datetime('now')), -- First discovery timestamp
    last_seen         TEXT DEFAULT (datetime('now')), -- Last scan timestamp
    quarantined_at    TEXT DEFAULT NULL,           -- NULL = active, timestamp = quarantined
    quarantine_reason TEXT DEFAULT NULL,           -- zero_refs | user_requested | gc_auto
    
    CHECK (length(blake3_hash) = 64),
    CHECK (size_bytes > 0)
);

CREATE INDEX idx_models_hash ON models(blake3_hash);
CREATE INDEX idx_models_format ON models(format);
CREATE INDEX idx_models_arch ON models(arch);
CREATE INDEX idx_models_quarantined ON models(quarantined_at) WHERE quarantined_at IS NOT NULL;

-- =============================================================================
-- Table: aliases
-- Purpose: Multiple filesystem paths pointing to same content
-- =============================================================================
CREATE TABLE aliases (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    model_hash  TEXT NOT NULL,                   -- References models.blake3_hash
    path        TEXT NOT NULL UNIQUE,             -- Absolute filesystem path
    frontend    TEXT,                             -- comfyui | forge | a1111 | hf_cache | user
    alias_type  TEXT NOT NULL,                    -- hardlink | symlink | junction | copy | original
    created_at  TEXT DEFAULT (datetime('now')),
    
    FOREIGN KEY (model_hash) REFERENCES models(blake3_hash) ON DELETE CASCADE,
    CHECK (alias_type IN ('hardlink', 'symlink', 'junction', 'copy', 'original'))
);

CREATE INDEX idx_aliases_hash ON aliases(model_hash);
CREATE INDEX idx_aliases_frontend ON aliases(frontend);

-- =============================================================================
-- Table: refs
-- Purpose: Explicit workflow/usage references (prevents accidental deletion)
-- =============================================================================
CREATE TABLE refs (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    model_hash   TEXT NOT NULL,                    -- References models.blake3_hash
    ref_source   TEXT NOT NULL,                    -- Workflow file path or source identifier
    ref_type     TEXT,                             -- lora | checkpoint | vae | controlnet | embedding
    last_checked TEXT DEFAULT (datetime('now')),
    
    FOREIGN KEY (model_hash) REFERENCES models(blake3_hash) ON DELETE CASCADE,
    UNIQUE (model_hash, ref_source, ref_type)
);

CREATE INDEX idx_refs_hash ON refs(model_hash);
CREATE INDEX idx_refs_source ON refs(ref_source);

-- =============================================================================
-- Table: downloads
-- Purpose: HuggingFace download tracking and SHA256↔BLAKE3 mapping
-- =============================================================================
CREATE TABLE downloads (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    model_hash   TEXT,                             -- NULL during download, set on completion
    source_url   TEXT NOT NULL,                    -- HuggingFace download URL
    sha256_hash  TEXT,                             -- HF uses SHA256, maps to BLAKE3
    status       TEXT NOT NULL,                    -- pending | downloading | hashing | done | failed
    bytes_total  INTEGER,                          -- Expected file size
    bytes_done   INTEGER DEFAULT 0,                -- Downloaded bytes (for progress)
    started_at   TEXT DEFAULT (datetime('now')),
    finished_at  TEXT,                             -- Completion timestamp
    error_msg    TEXT,                             -- Error details if status=failed
    
    FOREIGN KEY (model_hash) REFERENCES models(blake3_hash),
    CHECK (status IN ('pending', 'downloading', 'hashing', 'done', 'failed')),
    CHECK (bytes_done >= 0),
    CHECK (bytes_done <= bytes_total OR bytes_total IS NULL)
);

CREATE INDEX idx_downloads_url ON downloads(source_url);
CREATE INDEX idx_downloads_status ON downloads(status);
CREATE INDEX idx_downloads_sha256 ON downloads(sha256_hash);

-- =============================================================================
-- Table: wal_transactions
-- Purpose: Write-Ahead Log for crash recovery (two-phase commit protocol)
-- =============================================================================
CREATE TABLE wal_transactions (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    tx_id        TEXT UNIQUE NOT NULL,             -- UUID transaction identifier
    operation    TEXT NOT NULL,                    -- dedup | download | gc
    status       TEXT NOT NULL,                    -- pending | copied | committed | failed
    source_path  TEXT,                             -- Source file path (operation-specific)
    target_hash  TEXT,                             -- Target BLAKE3 hash
    metadata     TEXT,                             -- JSON with operation-specific data
    created_at   TEXT DEFAULT (datetime('now')),
    updated_at   TEXT DEFAULT (datetime('now')),
    
    CHECK (status IN ('pending', 'copied', 'committed', 'failed')),
    CHECK (operation IN ('dedup', 'download', 'gc'))
);

CREATE INDEX idx_wal_status ON wal_transactions(status);
CREATE INDEX idx_wal_created ON wal_transactions(created_at);
CREATE INDEX idx_wal_operation ON wal_transactions(operation);

-- =============================================================================
-- Database Configuration
-- =============================================================================

-- Enable Write-Ahead Logging for concurrent readers during writes
PRAGMA journal_mode = WAL;

-- Synchronous mode: NORMAL (fsync at transaction commit, not every write)
PRAGMA synchronous = NORMAL;

-- Enable foreign key constraints (disabled by default in SQLite)
PRAGMA foreign_keys = ON;

-- =============================================================================
-- Schema Version Tracking (for future migrations)
-- =============================================================================
CREATE TABLE schema_version (
    version     INTEGER PRIMARY KEY,
    applied_at  TEXT DEFAULT (datetime('now'))
);

INSERT INTO schema_version (version) VALUES (1);

-- =============================================================================
-- End of Schema v1
-- =============================================================================
