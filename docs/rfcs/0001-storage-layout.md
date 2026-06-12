# RFC 0001: CAS Storage Layout

**Status**: Draft  
**Author**: modeld Architecture Team  
**Created**: 2024  
**Last Updated**: 2024

## Abstract

This RFC defines the Content-Addressable Storage (CAS) layout for modeld, including directory structure, file organization, sharding strategy, immutability enforcement, and scalability considerations. The design must support millions of AI models while maintaining compatibility with future OCI standards and chunk-level deduplication capabilities.

## Motivation

AI model storage faces unique challenges:
- **Scale**: Individual models range from 100MB to 100GB+
- **Duplication**: Users often have identical models across multiple AI frontends
- **Safety**: Accidental modification or deletion can cause workflow failures
- **Performance**: Fast lookup and retrieval are critical for user experience
- **Future-proofing**: Must accommodate future features like chunk-level dedup and OCI compatibility

A well-designed storage layout is foundational to all modeld features.

## Problem Statement

Design a CAS storage layout that:

1. **Scales to millions of models** without filesystem performance degradation
2. **Supports future chunk-level deduplication** (Phase 6) without breaking changes
3. **Remains compatible with OCI artifact standards** for ecosystem interoperability
4. **Works across platforms** (Windows/Linux/macOS) with filesystem limitations
5. **Enforces immutability** to prevent accidental data corruption
6. **Enables efficient garbage collection** with safe deletion mechanisms
7. **Provides clear organization** that aids debugging and manual inspection

## Proposed Design

### Directory Structure

```
$MODELD_STORE/
├── cas/
│   └── blake3/
│       ├── 00/
│       │   ├── 0012ab3c4d5e6f7890abcdef1234567890abcdef1234567890abcdef12345678
│       │   └── 00f1e2d3c4b5a6978695847362514039281706543219876054321abcdef01234
│       ├── 01/
│       ├── ab/
│       │   └── abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890
│       ├── cd/
│       │   └── cdef567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef
│       └── ff/
│
├── virtual/
│   ├── comfyui/
│   │   ├── checkpoints/
│   │   │   ├── sd-v1-5-pruned.safetensors → ../../../cas/blake3/ab/abc...
│   │   │   └── sdxl-base-1.0.safetensors → ../../../cas/blake3/cd/cde...
│   │   ├── loras/
│   │   │   ├── character-lora-v1.safetensors → ../../../cas/blake3/12/123...
│   │   │   └── style-lora-sdxl.safetensors → ../../../cas/blake3/34/345...
│   │   ├── vae/
│   │   ├── embeddings/
│   │   └── controlnet/
│   ├── forge/
│   │   ├── models/
│   │   └── embeddings/
│   └── a1111/
│       ├── models/
│       └── embeddings/
│
├── hf_cache/
│   └── hub/
│       └── models--stabilityai--stable-diffusion-xl-base-1.0/
│           ├── blobs/
│           │   ├── abc123...sha256hash → ../../../../cas/blake3/ab/abc...
│           │   └── def456...sha256hash → ../../../../cas/blake3/cd/cde...
│           └── snapshots/
│               └── main/
│                   ├── model.safetensors → ../../blobs/abc123...
│                   ├── config.json → ../../blobs/def456...
│                   └── README.md → ../../blobs/789ghi...
│
├── tmp/
│   ├── downloads/
│   │   ├── abcdef123456.part  (actively downloading)
│   │   └── abcdef123456.lock  (download lock file)
│   └── cas_staging/
│       ├── abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890.tmp
│       └── fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321.tmp
│
├── quarantine/
│   ├── abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890.1705320600
│   └── abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890.1705320600.meta
│
├── wal/
│   ├── transactions.log
│   └── transactions.log-wal
│
└── modeld.db  (SQLite metadata database)
```

### Prefix Sharding Strategy


**Sharding Approach**: Use the first 2 hexadecimal characters of the BLAKE3 hash as the shard directory name.

**Rationale**:
- **Avoids single-directory bottleneck**: Filesystem performance degrades with millions of entries in one directory
- **Even distribution**: BLAKE3 produces uniformly distributed hashes, ensuring balanced shards
- **Optimal shard count**: 256 shards (16² = 256 two-character hex combinations: 00-ff)
- **Proven pattern**: Used by Git (2-char prefix), Docker, and other CAS systems

**Scalability Analysis**:

| Total Models | Models per Shard | Filesystem Performance | Status |
|--------------|------------------|------------------------|--------|
| 10,000 | ~39 | Excellent | ✓ |
| 100,000 | ~390 | Excellent | ✓ |
| 1,000,000 | ~3,906 | Good | ✓ |
| 10,000,000 | ~39,062 | Acceptable | ✓ |
| 100,000,000 | ~390,625 | Degraded | ⚠ Needs sub-sharding |

**Filesystem Limits** (directory entry count before performance degradation):
- **NTFS** (Windows): ~10,000-100,000 files per directory for good performance
- **ext4** (Linux): ~10 million entries (hard limit), but performance degrades around 100,000
- **APFS** (macOS): Similar to ext4, practical limit around 100,000 for good performance
- **btrfs** (Linux): Excellent scalability, handles millions well

**Conclusion**: With 256 shards, modeld can efficiently handle up to **25 million models** (97,656 per shard) before considering sub-sharding. This exceeds the requirements for the foreseeable future.

### Hash as Filename

**Design Decision**: The full BLAKE3 hash (64 hexadecimal characters) is used as the filename within each shard directory.

**Format**:
```
cas/blake3/{prefix}/{full_hash}
         ^^        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
         |         |
         |         Full 64-character BLAKE3 hash (256 bits)
         |
         First 2 characters (shard prefix)

Example:
cas/blake3/ab/abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890
           ^^  ^^^^
           |   |
           |   Shard prefix repeated
           Shard directory
```

**Why No Metadata in Filename?**

Alternatives considered:
1. **Include size**: `hash.size` → Rejected: size changes don't change hash, adds complexity
2. **Include format**: `hash.safetensors` → Rejected: format is metadata, belongs in database
3. **Include timestamp**: `hash.timestamp` → Rejected: content-addressing means timestamp is irrelevant

**Decision**: Keep filenames pure content addresses. All metadata lives in SQLite `models` table.

**Benefits**:
- **Simplicity**: Fewer edge cases, easier debugging
- **Content-addressability**: Guaranteed deduplication (same content = same path)
- **Schema flexibility**: Metadata changes don't require filesystem changes
- **Atomic lookups**: Direct path construction from hash (no directory scanning)

### Immutability Enforcement

**Problem**: CAS objects must never be modified after creation to maintain integrity.

**Enforcement Mechanisms**:

#### 1. Filesystem Permissions

**Unix/Linux/macOS**:
```bash
# After writing to CAS
chmod 444 cas/blake3/ab/abcdef123...
# Sets: -r--r--r-- (read-only for owner, group, others)
```

**Windows**:
```rust
// Pseudocode
use std::fs::File;
use std::os::windows::fs::FileExt;

fn make_readonly_windows(path: &Path) -> io::Result<()> {
    let file = File::open(path)?;
    file.set_attributes(FILE_ATTRIBUTE_READONLY)?;
    Ok(())
}
```

#### 2. Application-Level Checks

Before any write operation:
```rust
fn ensure_immutable(path: &Path) -> Result<(), Error> {
    if path.starts_with("$MODELD_STORE/cas/") {
        return Err(Error::CasImmutabilityViolation(
            "CAS objects are immutable and cannot be modified"
        ));
    }
    Ok(())
}
```

#### 3. Directory Permissions

**Unix/Linux/macOS**:
```bash
# CAS directory: allow read + execute (list), deny write
chmod 555 cas/blake3/ab/
# Sets: dr-xr-xr-x
```

This prevents:
- Creating new files in CAS directories (only modeld daemon can do this)
- Deleting existing CAS objects
- Renaming CAS objects

**Windows**:
Windows doesn't have Unix-style directory permissions. Alternative approach:
- Rely on file-level read-only attribute
- Application-level validation in modeld daemon
- Optional: Use NTFS ACLs to restrict write access to SYSTEM account only

#### 4. Hash Verification on Read

**Integrity Check** (optional, performance trade-off):
```rust
fn verify_cas_integrity(hash: &Blake3Hash, path: &Path) -> Result<bool, Error> {
    let computed_hash = compute_blake3_hash(path)?;
    Ok(computed_hash == *hash)
}
```

**When to verify**:
- **On demand**: `modeld verify` command
- **During GC**: Before deletion
- **After corruption reports**: User-triggered
- **Not on every read**: Too slow for model loading

### Temporary Staging Areas


**Purpose**: Isolate in-progress operations from the immutable CAS.

#### tmp/downloads/

**Use Case**: Active file downloads from HuggingFace or other sources.

**Structure**:
```
tmp/downloads/
├── {partial_hash}.part     # Partially downloaded file
├── {partial_hash}.lock     # Lock file (prevents concurrent downloads)
└── {partial_hash}.meta     # Download metadata (JSON)
```

**Metadata Example** (`{hash}.meta`):
```json
{
  "source_url": "https://huggingface.co/stabilityai/stable-diffusion-xl-base-1.0/resolve/main/model.safetensors",
  "expected_sha256": "abc123...",
  "expected_size": 6942694400,
  "downloaded_bytes": 3471347200,
  "started_at": "2024-01-15T10:30:00Z",
  "last_progress": "2024-01-15T10:35:00Z"
}
```

**Lifecycle**:
1. Download starts → Create `.part` and `.lock` files
2. Download progresses → Update `.part` file
3. Download completes → Compute BLAKE3 hash
4. Hash verified → Move to `tmp/cas_staging/`
5. `.lock` deleted → Allow cleanup

#### tmp/cas_staging/

**Use Case**: Pre-commit verification area for Two-Phase Commit protocol.

**Structure**:
```
tmp/cas_staging/
└── {full_blake3_hash}.tmp
```

**Lifecycle** (see RFC 0004 for detailed protocol):
1. **Phase A (Prepare)**: Copy canonical file to `{hash}.tmp`
2. **Verification**: Compute hash of `.tmp` file, ensure it matches
3. **Phase B (Commit)**: Atomic rename `.tmp` → `cas/blake3/{prefix}/{hash}`
4. **Cleanup**: On crash, WAL recovery handles orphaned `.tmp` files

**Cleanup Policy**:
- **Orphaned .part files**: Delete after 7 days of inactivity
- **Orphaned .tmp files**: WAL recovery on startup (resume or rollback)
- **Completed downloads**: Delete `.meta` and `.lock` immediately after move to CAS

### Quarantine Directory


**Purpose**: Soft-delete mechanism to prevent accidental data loss during garbage collection.

**Structure**:
```
quarantine/
├── {hash}.{unix_timestamp}       # Quarantined CAS object
└── {hash}.{unix_timestamp}.meta  # Quarantine metadata (JSON)
```

**Example**:
```
quarantine/
├── abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890.1705320600
└── abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890.1705320600.meta
```

**Metadata Format**:
```json
{
  "hash": "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
  "size_bytes": 6942694400,
  "format": "safetensors",
  "arch": "sdxl",
  "quarantine_date": "2024-01-15T10:30:00Z",
  "deletion_date": "2024-02-14T10:30:00Z",
  "reason": "zero_refs",
  "last_aliases": [
    "/home/user/.comfyui/models/checkpoints/sdxl-base.safetensors",
    "/home/user/.forge/models/Stable-diffusion/sdxl-base.safetensors"
  ],
  "last_refs": []
}
```

**Lifecycle**:
1. **GC triggers**: Model has `ref_count = 0`
2. **Move to quarantine**: `cas/blake3/{prefix}/{hash}` → `quarantine/{hash}.{timestamp}`
3. **Grace period**: Default 30 days (configurable)
4. **Expiration check**: Daily cron or on-demand `modeld gc --purge`
5. **Permanent deletion**: After grace period expires

**Recovery**:
```bash
# List quarantined models
modeld quarantine list

# Restore a specific model
modeld restore abcdef123...

# Restore and recreate aliases
modeld restore abcdef123... --recreate-aliases
```

**Design Rationale**:
- **Prevents accidents**: User can recover from mistaken deletions
- **Audit trail**: Metadata shows why model was quarantined
- **Configurable**: TTL can be adjusted per user preference
- **Disk space**: Quarantine counts against storage quota (encourages cleanup)

### Virtual Directories


**Purpose**: Provide familiar directory structures for AI frontends via hardlinks/symlinks.

**Structure**:
```
virtual/
├── comfyui/
│   ├── checkpoints/     # SDXL, SD1.5, Flux checkpoints
│   ├── loras/           # LoRA models
│   ├── vae/             # VAE models
│   ├── embeddings/      # Textual inversions
│   ├── controlnet/      # ControlNet models
│   ├── upscale_models/  # Upscalers (ESRGAN, etc.)
│   └── clip/            # CLIP models
├── forge/
│   └── models/
│       ├── Stable-diffusion/
│       ├── Lora/
│       └── VAE/
└── a1111/
    └── models/
        ├── Stable-diffusion/
        ├── Lora/
        └── VAE/
```

**Link Strategy** (see RFC 0005 for Windows compatibility details):
1. **Same volume**: Hardlink (zero overhead)
2. **Cross volume + privileges**: Symlink
3. **Cross volume + no privileges**: Reference-only (Windows fallback)

**Example Links**:
```bash
# Hardlink (preferred)
virtual/comfyui/checkpoints/sdxl-base-1.0.safetensors
  → cas/blake3/ab/abcdef123...
  (same inode, zero additional disk space)

# Symlink (cross-volume)
virtual/forge/models/Stable-diffusion/sdxl-base-1.0.safetensors
  → ../../../cas/blake3/ab/abcdef123...
  (pointer, minimal disk space)
```

**Category Detection** (see RFC 0006 for full algorithm):
- Analyze filename patterns (`lora`, `vae`, `controlnet` keywords)
- Check file metadata (safetensors header, model architecture)
- Use heuristics (file size, tensor shapes)
- Default to `checkpoints/` if uncertain

### HuggingFace Cache Directory


**Purpose**: Mimic HuggingFace Hub cache layout to transparently intercept downloads.

**Structure** (following HuggingFace conventions):
```
hf_cache/hub/
└── models--{org}--{model}/
    ├── blobs/
    │   ├── {sha256_hash_1} → ../../../../cas/blake3/{prefix1}/{blake3_hash_1}
    │   └── {sha256_hash_2} → ../../../../cas/blake3/{prefix2}/{blake3_hash_2}
    ├── snapshots/
    │   └── {commit_hash}/
    │       ├── model.safetensors → ../../blobs/{sha256_hash_1}
    │       ├── config.json → ../../blobs/{sha256_hash_2}
    │       └── README.md → ../../blobs/{sha256_hash_3}
    └── refs/
        └── main → ../snapshots/{commit_hash}
```

**Example** (Stable Diffusion XL):
```
hf_cache/hub/models--stabilityai--stable-diffusion-xl-base-1.0/
├── blobs/
│   └── abc123def456...sha256 → ../../../../cas/blake3/ab/abcdef1234567890...blake3
├── snapshots/
│   └── 76d28af79c56629411e7c6a40c9a47a6eb38084d/
│       ├── model.safetensors → ../../blobs/abc123def456...
│       └── config.json → ../../blobs/789ghi012jkl...
└── refs/
    └── main → ../snapshots/76d28af79c56629411e7c6a40c9a47a6eb38084d
```

**Hash Mapping** (SHA256 ↔ BLAKE3):

HuggingFace uses SHA256, modeld uses BLAKE3. Mapping is stored in the database:

```sql
-- In downloads table
CREATE TABLE downloads (
    id INTEGER PRIMARY KEY,
    model_hash TEXT,        -- BLAKE3 hash (primary)
    sha256_hash TEXT,       -- HuggingFace SHA256
    source_url TEXT NOT NULL,
    ...
    FOREIGN KEY (model_hash) REFERENCES models(blake3_hash)
);

CREATE INDEX idx_downloads_sha256 ON downloads(sha256_hash);
```

**Lookup Flow**:
1. HuggingFace library requests `blobs/{sha256}`
2. modeld looks up SHA256 in `downloads` table
3. Finds corresponding BLAKE3 hash
4. Returns symlink to `cas/blake3/{prefix}/{blake3}`

**Design Rationale**:
- **Transparency**: AI frameworks see standard HF cache layout
- **Zero config**: Set `HF_HOME=$MODELD_STORE/hf_cache`
- **Deduplication**: Multiple HF models with same files share CAS objects
- **Compatibility**: Works with all HuggingFace-based tools


### WAL Directory

**Purpose**: Transaction logs for crash recovery (see RFC 0004 for detailed protocol).

**Structure**:
```
wal/
├── transactions.log       # Main WAL file
└── transactions.log-wal   # SQLite WAL file (if using WAL mode)
```

**Contents** (stored in `wal_transactions` SQLite table):
- Transaction ID (UUID)
- Operation type (dedup, download, gc)
- Status (pending, copied, committed, failed)
- Source path, target hash
- Operation metadata (JSON)
- Timestamps (created_at, updated_at)

**Recovery on Startup**:
```rust
fn recover_wal_transactions(db: &Database) -> Result<()> {
    let incomplete = db.query(
        "SELECT * FROM wal_transactions WHERE status IN ('pending', 'copied')"
    )?;
    
    for tx in incomplete {
        match tx.status {
            Status::Pending => {
                // Check if tmp file exists, resume or rollback
                if tmp_file_exists(&tx.target_hash) {
                    resume_phase_a(&tx)?;
                } else {
                    rollback_transaction(&tx)?;
                }
            },
            Status::Copied => {
                // tmp file verified, proceed to Phase B
                resume_phase_b(&tx)?;
            },
            _ => unreachable!(),
        }
    }
    
    Ok(())
}
```

## OCI Compatibility Planning

**Goal**: Enable future compatibility with OCI (Open Container Initiative) artifact standards.

### OCI Artifact Structure

OCI stores blobs using SHA256:
```
blobs/
└── sha256/
    ├── abc123.../
    └── def456.../
```

### modeld Integration Strategy

**Phase 0-5** (Current design):
```
cas/blake3/        # modeld native storage (BLAKE3)
```

**Phase 6+** (OCI compatibility):
```
cas/
├── blake3/        # modeld native (existing)
└── sha256/        # OCI-compatible blobs (new)
    └── {sha256}/  # Full SHA256 hash as filename
```


**Hash Mapping Table** (when both hashes exist):
```sql
CREATE TABLE hash_mappings (
    blake3_hash TEXT NOT NULL,
    sha256_hash TEXT NOT NULL,
    mapping_type TEXT NOT NULL,  -- 'computed' | 'provided' | 'oci'
    verified BOOLEAN DEFAULT FALSE,
    created_at TEXT DEFAULT (datetime('now')),
    
    PRIMARY KEY (blake3_hash, sha256_hash),
    FOREIGN KEY (blake3_hash) REFERENCES models(blake3_hash)
);

CREATE INDEX idx_hash_map_sha256 ON hash_mappings(sha256_hash);
CREATE INDEX idx_hash_map_blake3 ON hash_mappings(blake3_hash);
```

**Mapping Sources**:
1. **Provided by HuggingFace**: SHA256 in download metadata → compute BLAKE3 on arrival
2. **Computed**: BLAKE3 already exists → optionally compute SHA256 for OCI export
3. **OCI Import**: SHA256 artifact imported → compute BLAKE3, create mapping

**Design Benefits**:
- **Backward compatible**: Existing BLAKE3-only storage works as-is
- **Forward compatible**: Can add SHA256 blobs without breaking changes
- **Flexible**: Supports both hash types simultaneously
- **Efficient**: Only compute mappings when needed (lazy evaluation)

**OCI Export** (future feature):
```bash
# Export modeld model to OCI artifact
modeld export oci --model abcdef123... --output model.tar

# Import OCI artifact to modeld
modeld import oci --artifact model.tar
```

## Chunk-Level Deduplication Planning (Phase 6)

**Goal**: Support chunk-based deduplication (e.g., LoRA merged with base model).

### Current Design (Whole-File CAS)

```
cas/blake3/{prefix}/{full_file_hash}
```

A 6GB SDXL model and a 6GB SDXL+LoRA model are stored separately (12GB total).

### Future Design (Chunk-Level CAS)

**Chunk Storage** (same structure as whole files):
```
cas/blake3/
├── 12/
│   └── 123abc...  (Chunk 1 of base model)
├── 34/
│   └── 345def...  (Chunk 2 of base model)
└── 56/
    └── 567ghi...  (Modified chunk in LoRA-merged model)
```

**Chunk Metadata Table**:
```sql
CREATE TABLE chunks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    chunk_hash TEXT UNIQUE NOT NULL,
    size_bytes INTEGER NOT NULL,
    created_at TEXT DEFAULT (datetime('now')),
    
    FOREIGN KEY (chunk_hash) REFERENCES models(blake3_hash)  -- Chunks are just CAS objects
);

CREATE TABLE model_chunks (
    model_hash TEXT NOT NULL,
    chunk_index INTEGER NOT NULL,
    chunk_hash TEXT NOT NULL,
    chunk_offset INTEGER NOT NULL,
    chunk_size INTEGER NOT NULL,
    
    PRIMARY KEY (model_hash, chunk_index),
    FOREIGN KEY (model_hash) REFERENCES models(blake3_hash),
    FOREIGN KEY (chunk_hash) REFERENCES chunks(chunk_hash)
);

CREATE INDEX idx_model_chunks_model ON model_chunks(model_hash);
CREATE INDEX idx_model_chunks_chunk ON model_chunks(chunk_hash);
```

**Chunk Boundaries** (for safetensors):
- Align on tensor boundaries (not arbitrary byte offsets)
- Preserve tensor atomicity
- Enable efficient reassembly


**Benefit**:
- Base SDXL model (6GB) stored once
- LoRA-merged model shares 98% of chunks with base (only ~120MB different)
- Total storage: 6GB + 120MB = 6.12GB (vs 12GB without chunking)

**Compatibility with Current Design**:
- Chunks stored in same `cas/blake3/` structure
- Whole-file models remain whole-file (no forced chunking)
- Chunk-level dedup is opt-in (user can trigger on specific models)
- No breaking changes to directory layout

## Scalability Analysis

### Capacity Calculations

**Assumptions**:
- Average model size: 5GB
- BLAKE3 hash: 64 hex chars
- Metadata per model: ~500 bytes (SQLite)
- Shard count: 256 (2-char hex prefix)

**Storage Overhead**:

| Total Models | Total Data | CAS Objects | Metadata DB | Overhead |
|--------------|-----------|-------------|-------------|----------|
| 1,000 | 5 TB | 1,000 files | ~500 KB | <0.001% |
| 10,000 | 50 TB | 10,000 files | ~5 MB | <0.001% |
| 100,000 | 500 TB | 100,000 files | ~50 MB | <0.001% |
| 1,000,000 | 5 PB | 1M files | ~500 MB | <0.001% |
| 10,000,000 | 50 PB | 10M files | ~5 GB | <0.001% |

**Conclusion**: Metadata overhead is negligible (<0.001% even at 10M models).

### Filesystem Performance

**Directory Entry Lookup Complexity**:
- **Flat directory** (no sharding): O(n) linear scan for large n
- **Sharded directory** (256 buckets): O(n/256) per shard
- **Hash-based lookup**: O(1) direct path construction

**With 256 Shards**:

| Total Models | Avg per Shard | Lookup Time | Status |
|--------------|---------------|-------------|--------|
| 10,000 | 39 | <1ms | ✓ Excellent |
| 100,000 | 390 | <5ms | ✓ Excellent |
| 1,000,000 | 3,906 | <10ms | ✓ Good |
| 10,000,000 | 39,062 | ~50ms | ✓ Acceptable |
| 100,000,000 | 390,625 | ~500ms | ⚠ Degraded |

**Mitigation for >25M models** (if ever needed):
- Implement **sub-sharding**: `cas/blake3/{char1}/{char2}/` (65,536 shards)
- Example: `cas/blake3/a/b/abcdef123...`
- This would support 2.5 billion models (65,536 × 39,062 per shard)

### Database Scalability

**SQLite Limits**:
- Max database size: 281 TB (with 64KB page size)
- Max rows per table: 2⁶⁴ (18 quintillion)
- Practical limit: ~10-100 million rows with good performance

**Index Performance** (with proper indexes):

| Table | Row Count | Query Type | Time |
|-------|-----------|------------|------|
| models | 1M | Hash lookup | <1ms |
| models | 10M | Hash lookup | ~5ms |
| aliases | 5M | Path lookup | <5ms |
| refs | 10M | Model refs | ~10ms |

**Optimization Strategies**:
1. **Indexes on all foreign keys** (already in schema)
2. **WAL mode** (concurrent reads during writes)
3. **VACUUM periodically** (reclaim space)
4. **ANALYZE** (update query planner statistics)


**Conclusion**: SQLite is sufficient for 10M+ models with proper indexing.

### Concurrent Access

**Read Concurrency**:
- **SQLite WAL mode**: Multiple readers, single writer (excellent for read-heavy workload)
- **CAS immutability**: No locks needed for read operations (files never change)
- **Virtual FS**: Hardlinks/symlinks are filesystem primitives (OS handles concurrency)

**Write Concurrency**:
- **WAL transactions**: Serialized writes to database
- **CAS writes**: One transaction per file (isolation via tmp/ staging)
- **Lock-free reads**: Readers not blocked by writers

**Expected Workload**:
- **Reads**: 99% of operations (model loading, scanning, querying)
- **Writes**: 1% of operations (initial scan, downloads, dedups)

**Conclusion**: Design is optimized for the expected read-heavy workload.

## Design Alternatives Considered

### Alternative 1: Flat Directory (No Sharding)

**Structure**:
```
cas/blake3/
├── abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890
├── bcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890a
└── ... (millions of files)
```

**Pros**:
- Simpler implementation
- No prefix calculation

**Cons**:
- **Severe performance degradation** at scale (NTFS, ext4 slow down around 100K files)
- **Difficult debugging** (can't easily browse millions of files)
- **Filesystem limits** (some systems hard-limit directory entries)

**Decision**: **Rejected** due to scalability concerns.

### Alternative 2: Date-Based Sharding

**Structure**:
```
cas/blake3/
├── 2024-01-15/
│   └── abcdef123...
├── 2024-01-16/
│   └── bcdef456...
└── ...
```

**Pros**:
- Easy to find recently added models
- Natural cleanup (delete old directories)

**Cons**:
- **Uneven distribution** (heavy download days create hotspots)
- **Temporal coupling** (same model downloaded on different days = different paths)
- **Breaks content-addressing** (path depends on download time, not content)
- **GC complexity** (can't easily determine if model is still referenced)

**Decision**: **Rejected** - violates content-addressability principle.

### Alternative 3: Hierarchical Path (Full Hash)

**Structure**:
```
cas/blake3/
├── ab/
│   ├── cd/
│   │   ├── ef/
│   │   │   └── abcdef1234567890...
│   │   └── ...
│   └── ...
└── ...
```

**Pros**:
- Even deeper sharding (16³ = 4,096 shards at 3 levels)
- Scales to billions of models

**Cons**:
- **Over-engineered** for expected scale (1-10M models)
- **Deeper path traversal** (slower lookups)
- **More complex implementation** (multiple directory levels)
- **Harder debugging** (4,096 directories to navigate)

**Decision**: **Rejected** - 2-char prefix sufficient for foreseeable scale.

### Alternative 4: Embedded Metadata in Filename

**Structure**:
```
cas/blake3/ab/
└── abcdef123...{size=6GB,format=safetensors,arch=sdxl}.cas
```

**Pros**:
- Self-documenting filenames
- No database needed for basic info

**Cons**:
- **Breaks content-addressability** (metadata changes → path changes?)
- **Filename length limits** (Windows 260-char path limit)
- **Rigid schema** (adding new metadata requires filename changes)
- **Redundant** (metadata should live in database)

**Decision**: **Rejected** - keep filenames pure content addresses.


### Alternative 5: Single Unified Directory (No cas/, virtual/, tmp/ separation)

**Structure**:
```
$MODELD_STORE/
├── objects/
│   ├── ab/abcdef123...  (CAS)
│   ├── ab/abcdef123...@comfyui_checkpoint  (Alias)
│   └── ab/abcdef123...@forge_lora  (Alias)
```

**Pros**:
- Simpler top-level structure
- All objects in one place

**Cons**:
- **Confusing organization** (hard to distinguish CAS from aliases)
- **Collision risk** (alias suffixes could conflict)
- **Harder cleanup** (can't delete entire tmp/ directory)
- **Difficult debugging** (mixed concerns in one directory)

**Decision**: **Rejected** - clear separation of concerns is more maintainable.

## Implementation Considerations

### Directory Creation

**On Initialization**:
```rust
fn init_storage_layout(store_path: &Path) -> Result<()> {
    let dirs = [
        "cas/blake3",
        "virtual/comfyui/checkpoints",
        "virtual/comfyui/loras",
        "virtual/comfyui/vae",
        "virtual/comfyui/embeddings",
        "virtual/comfyui/controlnet",
        "virtual/comfyui/upscale_models",
        "virtual/comfyui/clip",
        "virtual/forge/models/Stable-diffusion",
        "virtual/forge/models/Lora",
        "virtual/forge/models/VAE",
        "virtual/a1111/models/Stable-diffusion",
        "virtual/a1111/models/Lora",
        "virtual/a1111/models/VAE",
        "hf_cache/hub",
        "tmp/downloads",
        "tmp/cas_staging",
        "quarantine",
        "wal",
    ];
    
    for dir in dirs {
        let full_path = store_path.join(dir);
        fs::create_dir_all(&full_path)?;
    }
    
    // Set CAS directory to read-only (Unix)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let cas_dir = store_path.join("cas/blake3");
        let mut perms = fs::metadata(&cas_dir)?.permissions();
        perms.set_mode(0o555);  // dr-xr-xr-x
        fs::set_permissions(&cas_dir, perms)?;
    }
    
    Ok(())
}
```

### Lazy Shard Creation

**Don't pre-create all 256 shard directories**:
```rust
fn get_cas_path(hash: &Blake3Hash) -> PathBuf {
    let prefix = &hash.to_hex()[0..2];
    let shard_dir = Path::new("cas/blake3").join(prefix);
    
    // Create shard directory on-demand
    if !shard_dir.exists() {
        fs::create_dir_all(&shard_dir).expect("Failed to create shard directory");
    }
    
    shard_dir.join(hash.to_hex())
}
```

**Rationale**:
- Reduces initial setup time
- Only creates directories as needed
- Still results in even distribution (hash determines shard)


### Path Construction

**Efficient Hash-to-Path Conversion**:
```rust
impl Blake3Hash {
    fn to_cas_path(&self, store_path: &Path) -> PathBuf {
        let hex = self.to_hex();
        let prefix = &hex[0..2];
        store_path
            .join("cas/blake3")
            .join(prefix)
            .join(&hex)
    }
}

// Usage
let hash = Blake3Hash::from_hex("abcdef1234567890...")?;
let path = hash.to_cas_path(&store_path);
// Result: $MODELD_STORE/cas/blake3/ab/abcdef1234567890...
```

**No Directory Scanning Required**:
- Path is deterministically computed from hash
- O(1) lookup (no filesystem traversal)
- Works even if file doesn't exist (for pre-flight checks)

### Cleanup Strategies

**Orphaned tmp/ Files**:
```rust
fn cleanup_tmp_directory(store_path: &Path) -> Result<()> {
    let now = SystemTime::now();
    let threshold = Duration::from_secs(7 * 24 * 60 * 60);  // 7 days
    
    for entry in fs::read_dir(store_path.join("tmp/downloads"))? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        let age = now.duration_since(metadata.modified()?)?;
        
        if age > threshold {
            fs::remove_file(entry.path())?;
            println!("Deleted stale download: {:?}", entry.path());
        }
    }
    
    Ok(())
}
```

**Expired Quarantine Objects**:
```rust
fn purge_expired_quarantine(store_path: &Path, ttl: Duration) -> Result<()> {
    let now = SystemTime::now();
    
    for entry in fs::read_dir(store_path.join("quarantine"))? {
        let entry = entry?;
        if !entry.path().extension().map_or(false, |e| e == "meta") {
            continue;  // Only process .meta files
        }
        
        let meta: QuarantineMeta = serde_json::from_str(&fs::read_to_string(&entry.path())?)?;
        let deletion_date = meta.deletion_date.parse::<DateTime<Utc>>()?;
        
        if deletion_date.timestamp() < now.duration_since(UNIX_EPOCH)?.as_secs() as i64 {
            // Delete both the object and metadata
            let object_path = entry.path().with_extension("");
            fs::remove_file(&object_path)?;
            fs::remove_file(&entry.path())?;
            println!("Purged quarantined object: {}", meta.hash);
        }
    }
    
    Ok(())
}
```

## Migration Path

### From Existing Storage

**User has models in**:
- `~/.comfyui/models/checkpoints/`
- `~/stable-diffusion-webui/models/Stable-diffusion/`
- `~/forge/models/Stable-diffusion/`

**Migration Process**:
```bash
# Step 1: Scan existing directories
modeld scan ~/.comfyui/models/
modeld scan ~/stable-diffusion-webui/models/
modeld scan ~/forge/models/

# Step 2: Identify duplicates
modeld dedup --report
# Output: "Found 50 duplicates (120GB potential savings)"

# Step 3: Execute deduplication
modeld dedup --auto

# Step 4: Verify
modeld verify
```

**Result**:
- Original files replaced with hardlinks/symlinks to CAS
- CAS objects in `$MODELD_STORE/cas/blake3/`
- Virtual directories populated
- Zero configuration changes for AI frontends


### Backward Compatibility

**Rollback Strategy** (if user wants to uninstall modeld):
```bash
modeld uninstall --restore

# Actions:
# 1. For each alias with type='hardlink':
#    - If CAS object still exists: keep hardlink (zero data loss)
# 2. For each alias with type='symlink':
#    - Copy CAS object → original path
#    - Delete symlink
# 3. Delete $MODELD_STORE
# 4. Restore original directory structure
```

**Safety**:
- CAS immutability ensures no data corruption
- Quarantine provides recovery for deleted models
- Database tracks all original paths

## Security Considerations

### Immutability Enforcement

**Problem**: What if malicious software modifies CAS objects?

**Mitigations**:
1. **File permissions**: Read-only for all users except modeld daemon
2. **Hash verification**: `modeld verify` detects tampering
3. **Cryptographic hashes**: BLAKE3 makes collision attacks computationally infeasible
4. **Audit trail**: Database tracks all CAS operations

### Symlink Attacks

**Problem**: Symlink to `/etc/passwd` → modeld follows → security breach?

**Mitigations**:
1. **Validate targets**: Ensure symlink targets are within `$MODELD_STORE`
2. **No privileged operations**: modeld never runs as root/admin (except during setup)
3. **Canonical paths**: Resolve symlinks before operations

```rust
fn is_safe_symlink(link_path: &Path, store_path: &Path) -> Result<bool> {
    let target = fs::read_link(link_path)?;
    let canonical_target = target.canonicalize()?;
    Ok(canonical_target.starts_with(store_path))
}
```

### Quarantine Access

**Problem**: Sensitive model in quarantine → data leak?

**Mitigation**:
- Quarantine has same permissions as CAS (read-only)
- No special permissions granted
- User explicitly restores (not automatic)

## Testing Strategy

### Unit Tests

**Directory Creation**:
```rust
#[test]
fn test_storage_layout_initialization() {
    let temp_dir = TempDir::new().unwrap();
    init_storage_layout(temp_dir.path()).unwrap();
    
    assert!(temp_dir.path().join("cas/blake3").exists());
    assert!(temp_dir.path().join("virtual/comfyui/checkpoints").exists());
    assert!(temp_dir.path().join("tmp/downloads").exists());
    assert!(temp_dir.path().join("quarantine").exists());
}
```

**Path Construction**:
```rust
#[test]
fn test_hash_to_cas_path() {
    let hash = Blake3Hash::from_hex(
        "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
    ).unwrap();
    let store_path = Path::new("/modeld_store");
    let cas_path = hash.to_cas_path(store_path);
    
    assert_eq!(
        cas_path,
        Path::new("/modeld_store/cas/blake3/ab/abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890")
    );
}
```

**Shard Distribution**:
```rust
#[test]
fn test_shard_distribution() {
    let mut shard_counts = HashMap::new();
    
    for _ in 0..10_000 {
        let hash = Blake3Hash::random();
        let prefix = &hash.to_hex()[0..2];
        *shard_counts.entry(prefix.to_string()).or_insert(0) += 1;
    }
    
    // Verify roughly even distribution (each shard ~39 files on average)
    for count in shard_counts.values() {
        assert!(*count > 10 && *count < 100, "Uneven distribution detected");
    }
}
```


### Integration Tests

**CAS Write and Read**:
```rust
#[test]
fn test_cas_write_read_cycle() {
    let temp_dir = TempDir::new().unwrap();
    init_storage_layout(temp_dir.path()).unwrap();
    
    // Write a model to CAS
    let model_data = b"fake safetensors data";
    let hash = blake3::hash(model_data);
    let cas_path = Blake3Hash::from(hash).to_cas_path(temp_dir.path());
    
    fs::write(&cas_path, model_data).unwrap();
    
    // Set read-only
    make_readonly(&cas_path).unwrap();
    
    // Verify we can read but not write
    let read_data = fs::read(&cas_path).unwrap();
    assert_eq!(read_data, model_data);
    
    let write_result = fs::write(&cas_path, b"tampered");
    assert!(write_result.is_err(), "Should not be able to write to read-only CAS file");
}
```

**Quarantine Lifecycle**:
```rust
#[test]
fn test_quarantine_lifecycle() {
    let temp_dir = TempDir::new().unwrap();
    init_storage_layout(temp_dir.path()).unwrap();
    
    // Create a CAS object
    let hash = blake3::hash(b"test data");
    let cas_path = Blake3Hash::from(hash).to_cas_path(temp_dir.path());
    fs::write(&cas_path, b"test data").unwrap();
    
    // Move to quarantine
    let quarantine_path = temp_dir.path().join(format!("quarantine/{}.{}", hash.to_hex(), 1705320600));
    fs::rename(&cas_path, &quarantine_path).unwrap();
    
    assert!(!cas_path.exists());
    assert!(quarantine_path.exists());
    
    // Restore from quarantine
    fs::rename(&quarantine_path, &cas_path).unwrap();
    
    assert!(cas_path.exists());
    assert!(!quarantine_path.exists());
}
```

### Performance Tests

**Shard Lookup Performance**:
```rust
#[bench]
fn bench_cas_path_construction(b: &mut Bencher) {
    let hash = Blake3Hash::random();
    let store_path = Path::new("/modeld_store");
    
    b.iter(|| {
        black_box(hash.to_cas_path(store_path));
    });
}
```

**Expected Result**: <100ns per path construction (pure string operations).

### Platform-Specific Tests

**Windows Symlink Privileges**:
```rust
#[cfg(windows)]
#[test]
fn test_windows_link_capability() {
    let capability = get_link_capability();
    println!("Link capability: {:?}", capability);
    
    match capability {
        LinkCapability::Full => {
            // Test symlink creation
            let temp_dir = TempDir::new().unwrap();
            let target = temp_dir.path().join("target.txt");
            let link = temp_dir.path().join("link.txt");
            
            fs::write(&target, "test").unwrap();
            std::os::windows::fs::symlink_file(&target, &link).unwrap();
            
            assert!(link.exists());
        },
        LinkCapability::Limited => {
            println!("Warning: No symlink privileges, cross-volume dedup limited");
        }
    }
}
```

## Documentation

### User-Facing Documentation

**Setup Guide**: `docs/setup.md`
- Initial configuration
- Storage location selection
- Windows Developer Mode setup (for symlinks)

**Storage Management**: `docs/storage.md`
- Understanding the CAS layout
- Disk space management
- Quarantine recovery

**Troubleshooting**: `docs/troubleshooting.md`
- Corrupted CAS objects
- Permission issues
- Quarantine restoration

### Developer Documentation

**Architecture Overview**: `docs/architecture.md`
- High-level design
- Component interactions
- Data flows

**API Reference**: `docs/api.md`
- Rust API for storage operations
- Database schema
- Extension points


## Open Questions

### Q1: Should we pre-create all 256 shard directories?

**Options**:
1. **Lazy creation** (current proposal): Create on-demand
2. **Pre-creation**: Create all 256 during `modeld init`

**Trade-offs**:
- Lazy: Faster init, slightly more complex code
- Pre-creation: Simpler code, shows directory structure immediately

**Decision**: **Lazy creation** (recommended) - Minimal impact, cleaner initial state.

### Q2: What is the optimal quarantine TTL?

**Options**:
1. 7 days (aggressive cleanup)
2. 30 days (current proposal, balanced)
3. 90 days (conservative)
4. User-configurable

**Trade-offs**:
- Shorter TTL: Less wasted space, higher risk of accidental loss
- Longer TTL: Safer, more wasted space

**Decision**: **30 days default, user-configurable** via `modeld config set quarantine_ttl 60`.

### Q3: Should CAS objects have file extensions?

**Current**: `cas/blake3/ab/abcdef123...` (no extension)

**Alternative**: `cas/blake3/ab/abcdef123....safetensors` (with extension)

**Trade-offs**:
- No extension: Pure content-addressing, simpler
- With extension: Slightly easier debugging, file managers show previews

**Decision**: **No extension** (current) - Keep CAS pure, metadata in database.

### Q4: How to handle CAS corruption?

**Scenarios**:
1. Bit rot (storage media failure)
2. Accidental modification (despite read-only)
3. Malicious tampering

**Detection**: `modeld verify` command rehashes all CAS objects

**Recovery Options**:
1. **Download again** (if source URL known)
2. **Restore from backup** (if user has backups)
3. **Mark as corrupted** (quarantine + alert user)

**Decision**: Mark as corrupted, provide recovery options in UI. Consider future integration with ZFS/btrfs checksums.

## Future Work

### Phase 1-5: Implement Current Design
- Core CAS operations
- Deduplication engine
- Virtual FS layer
- HuggingFace interception

### Phase 6: Chunk-Level Deduplication
- Safetensors tensor-level chunking
- Chunk storage in same CAS layout
- Reassembly on-demand

### Phase 7: OCI Compatibility
- SHA256 blob support
- OCI artifact import/export
- Registry integration

### Phase 8: Advanced Features
- Automatic backup to remote storage
- Peer-to-peer model sharing
- BitTorrent-style distribution

## References

- Git Object Storage: https://git-scm.com/book/en/v2/Git-Internals-Git-Objects
- Docker Image Layers: https://docs.docker.com/storage/storagedriver/
- OCI Artifact Spec: https://github.com/opencontainers/image-spec
- BLAKE3 Hash Function: https://github.com/BLAKE3-team/BLAKE3-specs

## Appendix: Full Example

### Scenario: User downloads SDXL model via ComfyUI

**Initial State**:
```
(Empty $MODELD_STORE)
```

**User Action**:
```python
# ComfyUI workflow loads model
model_path = "models/checkpoints/sdxl-base-1.0.safetensors"
```

**modeld Intercepts**:

1. **Check if model exists in CAS**:
   ```sql
   SELECT * FROM models WHERE blake3_hash = 'abcdef123...';
   -- Result: Not found
   ```

2. **Download to tmp/**:
   ```
   tmp/downloads/abcdef123....part
   tmp/downloads/abcdef123....lock
   ```

3. **Download completes, compute hash**:
   ```
   BLAKE3(downloaded_file) = abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890
   ```

4. **Move to CAS staging**:
   ```
   tmp/cas_staging/abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890.tmp
   ```

5. **Verify hash, atomic rename**:
   ```
   cas/blake3/ab/abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890
   ```

6. **Create virtual link**:
   ```
   virtual/comfyui/checkpoints/sdxl-base-1.0.safetensors
     → ../../../cas/blake3/ab/abcdef123...
   ```

7. **Update database**:
   ```sql
   INSERT INTO models (blake3_hash, size_bytes, format, arch)
   VALUES ('abcdef123...', 6942694400, 'safetensors', 'sdxl');
   
   INSERT INTO aliases (model_hash, path, frontend, alias_type)
   VALUES ('abcdef123...', 'virtual/comfyui/checkpoints/sdxl-base-1.0.safetensors', 'comfyui', 'hardlink');
   ```

**Final State**:
```
$MODELD_STORE/
├── cas/blake3/ab/
│   └── abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890  (6.47GB)
├── virtual/comfyui/checkpoints/
│   └── sdxl-base-1.0.safetensors → ../../../cas/blake3/ab/abcdef123...  (0 bytes, hardlink)
└── modeld.db  (contains models and aliases entries)

Total disk space: 6.47GB (CAS object only, hardlink has zero overhead)
```

**When user later downloads same model for Forge**:
```
modeld scan ~/forge/models/Stable-diffusion/
# Detects duplicate (same hash)

modeld dedup
# Replaces Forge model with hardlink to existing CAS object
# Total disk space: Still 6.47GB (no duplication)
```

---

## Decision

**Status**: **APPROVED** (Pending Review)

**Rationale**:
- Proven pattern (Git, Docker, OCI use similar approaches)
- Scales to millions of models
- Platform-compatible (Windows/Linux/macOS)
- Future-proof (chunk dedup, OCI support)
- Low overhead (<0.001% metadata)

**Next Steps**:
1. Review by team
2. Implementation in Phase 1 (`modeld-core` crate)
3. Create storage initialization code
4. Write comprehensive tests

---

*RFC 0001 Version: 1.0*  
*Last Updated: 2024*  
*Status: Draft*

