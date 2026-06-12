# Design: modeld Phase 0 - Architecture Design & RFC

## Overview

modeld is a Content-Addressable Storage (CAS) infrastructure for AI models - the "containerd + git-lfs + nix store" for the AI model world. This design document covers Phase 0: the foundational architecture design that must be completed before any core implementation begins.

**Core Value Proposition**: Users don't need to modify any existing configuration to automatically save TB-level disk space.

**Phase 0 Principle**: "Don't write core business code, only do system design."

## Goals

1. Complete system architecture design before implementation
2. Document all 7 RFC specifications
3. Define SQLite schema v1
4. Establish comprehensive Windows compatibility strategy
5. Enable anyone to "completely describe the system" without referencing docs

## High-Level Design

### System Architecture

```
┌─────────────────────────────────────────────────────┐
│            AI Frontend Layer                         │
│   ComfyUI    Forge    A1111    InvokeAI    diffusers │
└──────────────┬───────────────────────┬───────────────┘
               │                       │
     HF Hook (Python shim)      Virtual Model FS
     ~/.cache/huggingface        /virtual/comfy/
               │                       │
               └──────────┬────────────┘
                           │
               ┌───────────▼────────────┐
               │         modeld          │
               │  ┌─────────────────┐   │
               │  │ Download Manager│   │
               │  │ CAS Storage     │   │
               │  │ Dedup Engine    │   │
               │  │ Metadata Index  │   │
               │  │ Ref Tracker     │   │
               │  │ Workflow Parser │   │
               │  └─────────────────┘   │
               └───────────┬────────────┘
                           │
               ┌───────────▼────────────┐
               │     Content Store       │
│  /store/blake3/ab/cd/   │
               │  (BLAKE3-addressed)     │
               └────────────────────────┘
```

### Component Diagram

modeld consists of six core components that work together to provide transparent, space-efficient AI model storage:

#### 1. CAS Storage Layer (Content-Addressable Storage)

**Purpose**: Immutable storage backend for all model files

**Key Features**:
- BLAKE3-based content addressing (256-bit hashes, 64 hex chars)
- Prefix sharding: First 2 hex chars create subdirectories (256 shards: `00-ff`)
- Immutability enforcement: Files are read-only (chmod 444 / Windows read-only attribute)
- Scalability: Handles 25M+ models before sub-sharding needed
- Quarantine mechanism: 30-day grace period before permanent deletion

**Storage Structure**:
```
cas/blake3/{prefix}/{full_blake3_hash}
Example: cas/blake3/ab/abcdef123...
```

**Performance Characteristics**:
- Hash computation: ≥2GB/s on NVMe (using parallel chunk processing)
- Lookup: O(1) direct path construction from hash
- Storage overhead: <0.001% even at 10M models

**Interactions**:
- **Receives**: Model files from Dedup Engine, Download Manager
- **Provides**: Content to Virtual FS Layer via hardlinks/symlinks
- **Queries**: Metadata Index for ref counts, quarantine status

---

#### 2. Metadata Index (SQLite Database)

**Purpose**: Tracks all metadata, relationships, and references for CAS objects

**Database Schema** (5 main tables):

**a) `models` table** - Central CAS registry:
- Primary key: `blake3_hash` (64-char hex string)
- Metadata: `size_bytes`, `format`, `arch`, `category`, `base_model`
- Lifecycle: `created_at`, `last_seen`, `quarantined_at`
- Indexed for fast lookups: hash, format, architecture, quarantine status

**b) `aliases` table** - Filesystem path mappings:
- Maps: `path` → `model_hash` (many paths can point to same model)
- Link type tracking: `alias_type` (hardlink | symlink | junction | reference_only)
- Frontend categorization: `frontend` (comfyui | forge | a1111 | hf_cache | user)
- Used for: Reference counting, virtual FS synchronization

**c) `refs` table** - Explicit references:
- Workflow dependencies: ComfyUI workflow.json → model hash
- User favorites: Manual "keep this model" markers
- Type classification: `ref_type` (lora | checkpoint | vae | controlnet)
- Validation: `last_checked` timestamp for stale detection

**d) `downloads` table** - HuggingFace tracking:
- Hash mapping: `sha256_hash` (HF) ↔ `model_hash` (BLAKE3)
- Download state: `status` (pending | downloading | hashing | done | failed)
- Source tracking: `source_url`, progress tracking
- Critical for: Deduplicating HF downloads across frameworks

**e) `wal_transactions` table** - Crash recovery:
- Transaction tracking: `tx_id` (UUID), `operation` (dedup | download | gc)
- State machine: `status` (pending | copied | committed | failed)
- Recovery metadata: JSON with operation-specific context
- Enables: Atomic operations, resumable transactions

**Performance Configuration**:
- WAL mode: Concurrent readers during writes
- Indexes: All foreign keys + frequently queried fields
- Expected query times: <1ms hash lookups, <5ms ref counts (even with 10K refs)

**Interactions**:
- **Queried by**: All components for metadata, ref counts, mappings
- **Updated by**: Dedup Engine (aliases), Download Manager (downloads), GC (quarantine)
- **Provides**: Source of truth for system state

---

#### 3. Dedup Engine (Deduplication & Migration)

**Purpose**: Safely migrate duplicate files to CAS with transactional guarantees

**Core Capabilities**:

**a) Duplicate Detection**:
- Group files by BLAKE3 hash
- Canonical path selection algorithm:
  1. CAS first (if already in CAS)
  2. Oldest mtime (likely original)
  3. Shortest path (simpler reference)
  4. Alphabetical (deterministic tiebreaker)

**b) Two-Phase Commit Protocol**:

**Phase A (Prepare)**:
1. Generate transaction ID (UUID)
2. Write WAL record (status='pending')
3. Copy canonical file → `tmp/cas_staging/{hash}.tmp`
4. Verify BLAKE3 hash matches
5. Update WAL (status='copied')
6. fsync WAL to disk

**Phase B (Commit)**:
7. Atomic rename: staging → `cas/blake3/{prefix}/{hash}`
8. For each duplicate:
   - Determine link strategy (hardlink > symlink > junction > reference-only)
   - Create link at original path → CAS
   - Record in aliases table
9. Quarantine replaced files (if ref_count=0)
10. Update WAL (status='committed')

**c) Crash Recovery**:
- On startup, scan WAL for incomplete transactions
- Resume from checkpoint (Phase A or B)
- Rollback corrupted transactions
- Ensures no data loss or partial states

**Dedup Modes**:
- **Interactive**: Confirm each group, show details
- **Dry-run**: Preview savings without changes
- **Auto**: Non-interactive, suitable for cron jobs
- **Report**: Analysis only, no modifications

**Platform-Specific Handling**:
- **Same volume**: Hardlink (zero overhead, works everywhere)
- **Cross-volume + privileges**: Symlink (Windows Developer Mode required)
- **Cross-volume + no privileges**: Reference-only (no space savings, Windows fallback)
- **Junction points**: Directory-level links (Windows, no privileges needed)

**Performance**:
- Temporary storage: Requires 2x space during Phase A
- Verification: Hash check at every stage
- Throughput: Limited by disk I/O (typically 100-300 MB/s sequential)

**Interactions**:
- **Reads from**: Metadata Index (models table)
- **Writes to**: CAS Storage, Metadata Index (aliases, wal_transactions), Quarantine
- **Triggers**: Virtual FS Layer refresh after completion

---

#### 4. Virtual FS Layer (AI Frontend Integration)

**Purpose**: Provide familiar directory structures for AI frontends using hardlinks/symlinks

**Directory Structure**:
```
virtual/
├── comfyui/
│   ├── checkpoints/ → symlinks to CAS
│   ├── loras/
│   ├── vae/
│   ├── embeddings/
│   ├── controlnet/
│   └── upscale_models/
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

**Category Detection Algorithm** (multi-layer):
1. **Filename patterns**: Regex matching (fast, ~90% accuracy)
   - `(?i)lora` → lora, `(?i)vae` → vae, `(?i)controlnet` → controlnet
2. **File size heuristics**: Range-based disambiguation
   - <100MB → embedding, 100-500MB → lora, 500MB-1GB → vae, >1GB → checkpoint
3. **Safetensors metadata**: JSON header parsing
   - Check `__metadata__` for architecture, network type
4. **Tensor shape analysis**: Deep inspection (slow, high accuracy)
   - Count parameters, analyze tensor dimensions
5. **Default fallback**: Conservative guess (checkpoint)

**Link Strategy Priority**:
1. **Hardlink** (same volume): Zero overhead, transparent
2. **Symlink** (cross-volume): Requires privileges on Windows
3. **Junction** (directories only): Windows fallback, no privileges
4. **Reference-only**: Database tracking only, no physical link

**Refresh Mechanism**:
- **Triggers**: After dedup, download, scan, manual sync
- **Algorithm**: Incremental sync (only changed models)
- **Validation**: Check existing links point to correct CAS objects
- **Cleanup**: Remove stale links (target no longer in CAS)

**User Integration**:
```bash
# Option 1: Direct path
export COMFYUI_MODEL_PATH=$MODELD_STORE/virtual/comfyui

# Option 2: Symlink
ln -s $MODELD_STORE/virtual/comfyui ~/.comfyui/models

# Option 3: Configuration file
# ComfyUI extra_model_paths.yaml
```

**Performance**:
- Hardlinks: Zero overhead (same inode)
- Symlinks: Minimal overhead (~1-2% pointer dereference)
- Refresh: <10 seconds for 1000 models

**Interactions**:
- **Reads from**: CAS Storage (via hardlinks/symlinks)
- **Queries**: Metadata Index for categories, aliases
- **Updates**: Aliases table when creating links
- **Consumed by**: AI frontends (ComfyUI, Forge, A1111)

---

#### 5. HF Interception Layer (Download Manager)

**Purpose**: Transparently deduplicate HuggingFace downloads across frameworks

**Two-Layer Strategy**:

**Layer 1: HF_HOME Environment Variable** (Primary, 95% coverage):
- Set `HF_HOME=$MODELD_STORE/hf_cache`
- HuggingFace libraries natively respect this variable
- Stable: Official API, won't break with updates
- No code modification required

**Layer 2: Python Monkeypatch** (Fallback, 5% edge cases):
- Hook `huggingface_hub.hf_hub_download()` when Layer 1 insufficient
- Installation methods:
  - Option A: Explicit import (`import modeld_hook`)
  - Option B: System-wide sitecustomize.py (automatic)
  - Option C: PYTHONSTARTUP environment variable (session-level)

**Fake HF Cache Structure**:
```
hf_cache/hub/
└── models--{org}--{model}/
    ├── blobs/
    │   └── {sha256} → symlink to ../../../cas/blake3/{prefix}/{blake3_hash}
    ├── snapshots/
    │   └── {revision}/
    │       └── {filename} → ../../blobs/{sha256}
    └── refs/
        └── main  # Contains revision hash
```

**SHA256 ↔ BLAKE3 Mapping**:
- HuggingFace uses SHA256 for content addressing
- modeld uses BLAKE3 (3-10x faster)
- `downloads` table stores both hashes for bidirectional lookup
- Extract SHA256 from: HTTP headers (X-Linked-Etag) > .huggingface.json > compute manually

**Download Deduplication Flow**:
1. Framework requests model (e.g., `pipeline.from_pretrained("org/model")`)
2. HF library checks: `$HF_HOME/hub/models--org--model/snapshots/{rev}/{file}`
3. If exists: Return symlink to CAS (cache hit)
4. If not exists:
   a. modeld intercepts download
   b. Query downloads table: WHERE sha256_hash = ? (check if already have it)
   c. If found: Return existing CAS object (hash mapping hit)
   d. If not found:
      - Download to `tmp/downloads/{uuid}.part`
      - Extract SHA256 from HF metadata
      - Compute BLAKE3 hash
      - Check if BLAKE3 already in CAS (might be from user scan)
      - If found: Delete temp, use existing
      - If not found: Move to CAS
      - Record mapping: (blake3, sha256) in downloads table
      - Create fake HF cache symlinks
   e. Return path to framework

**Compatibility**:
- Works with: diffusers, transformers, safetensors, ComfyUI, Forge
- Handles: Partial downloads (resumable), offline mode, private repos (auth tokens)
- Survives: Library updates (Layer 1 stable, Layer 2 may need updates)

**Performance**:
- Hash mapping lookup: <1ms database query
- Deduplication: Instant for known SHA256, full download for new models
- Space savings: 100% for duplicate HF downloads

**Interactions**:
- **Intercepts**: HuggingFace library download calls
- **Writes to**: CAS Storage, Metadata Index (downloads table)
- **Creates**: Fake HF cache structure (blobs/, snapshots/)
- **Consumed by**: AI frameworks expecting HF cache layout

---

#### 6. Reference Tracker & GC (Garbage Collection)

**Purpose**: Track model usage and safely reclaim space from unused models

**Reference Types**:

**a) Explicit References**:
- Workflow dependencies: ComfyUI workflow.json → model hash
- User favorites: Manual "keep this" markers
- Download records: Recently downloaded models
- Stored in: `refs` table

**b) Implicit References (Aliases)**:
- Filesystem links: Hardlinks/symlinks in virtual/ or user directories
- Original file locations: Files not yet deduplicated
- Stored in: `aliases` table

**Reference Counting Algorithm**:
```rust
ref_count = COUNT(refs WHERE model_hash = ?) 
          + COUNT(aliases WHERE model_hash = ?)

if ref_count > 0:
    protection_level = PROTECTED (cannot GC)
else:
    protection_level = QUARANTINE (eligible for GC)
```

**Protection Levels**:

1. **PROTECTED** (ref_count > 0):
   - Location: `cas/blake3/{prefix}/{hash}`
   - State: Active, in use
   - Actions: Cannot be garbage collected

2. **QUARANTINE** (ref_count = 0, age < TTL):
   - Location: `quarantine/{hash}.{timestamp}`
   - State: Soft-deleted, recoverable
   - TTL: 30 days (default, configurable)
   - Metadata: `.meta` JSON file with last aliases, refs, reason

3. **DELETED** (age ≥ TTL):
   - Location: Permanently removed
   - State: Unrecoverable (unless external backup)

**Safe GC Algorithm** (5 phases):

**Phase 1: Reference Validation**
- Recount all references (don't trust cached counts)
- Remove stale aliases (path no longer exists)
- Remove orphaned refs (workflow file deleted)

**Phase 2: Candidate Selection**
- Query models with ref_count = 0
- Exclude already quarantined
- Exclude recent activity (<7 days grace period)
- Sort by last_seen ASC (oldest first)

**Phase 3: User Confirmation** (interactive mode)
- Display each candidate with details
- Show last known locations and refs
- Prompt: Quarantine / Skip / Abort

**Phase 4: Quarantine Execution**
- Double-check ref_count still = 0 (safety)
- Generate quarantine metadata JSON
- Move file: CAS → `quarantine/{hash}.{timestamp}`
- Update database: quarantined_at = now()

**Phase 5: Expiration Processing**
- Query expired quarantine models (age > TTL)
- Triple-check ref_count = 0 (paranoid safety)
- Delete file and metadata
- Remove from database
- Report space freed

**GC Modes**:
- **Safe** (default): Interactive, requires confirmations
- **Dry-run**: Preview only, no changes
- **Auto**: Non-interactive, suitable for cron
- **Aggressive**: Skip quarantine (requires --force)

**GC Triggers**:
- **Manual**: `modeld gc` command
- **Automatic**: Disk usage threshold (90% full, configurable)
- **Scheduled**: Cron job (daily at 3am)

**Recovery**:
```bash
modeld quarantine list                    # Show quarantined models
modeld restore {hash}                     # Restore specific model
modeld restore {hash} --recreate-aliases  # Restore and recreate links
```

**Safety Invariants**:
1. Model with ref_count > 0 is NEVER quarantined
2. All models go through quarantine before permanent deletion
3. Reference validation happens atomically (no race conditions)
4. Stale refs cleaned up before GC
5. All operations logged and reported

**Performance**:
- Ref count calculation: <5ms per model (even with 10K refs)
- Validation phase: ~1-2 seconds for 10K models
- Quarantine operation: <100ms per model
- Expected GC frequency: Weekly to monthly

**Interactions**:
- **Reads from**: Metadata Index (models, aliases, refs tables)
- **Writes to**: Quarantine directory, Metadata Index (quarantined_at)
- **Validates**: All file paths in aliases and refs
- **Coordinates with**: Virtual FS Layer (after GC, refresh links)

### Component Interactions

The six core components interact through well-defined interfaces to provide transparent, space-efficient model storage:

#### Component Interaction Diagram

```
┌─────────────────────────────────────────────────────────────────┐
│                     AI Frontend Layer                            │
│   ComfyUI    Forge    A1111    InvokeAI    diffusers            │
└──────────┬─────────────────────────────────┬────────────────────┘
           │                                 │
           │                                 │
    ┌──────▼─────────┐              ┌───────▼────────┐
    │  Virtual FS    │              │ HF Interception│
    │    Layer       │              │     Layer      │
    │  (hardlinks/   │              │  (downloads)   │
    │   symlinks)    │              │                │
    └────────┬───────┘              └───────┬────────┘
             │                              │
             │        ┌─────────────┐       │
             └───────►│  Metadata   │◄──────┘
                      │   Index     │
                      │  (SQLite)   │
                      └──────┬──────┘
                             │
                   ┌─────────┼─────────┐
                   │         │         │
            ┌──────▼───┐  ┌──▼──────┐ │
            │  Dedup   │  │Reference│ │
            │  Engine  │  │Tracker &│ │
            │          │  │   GC    │ │
            └─────┬────┘  └───┬─────┘ │
                  │           │       │
                  └─────┬─────┘       │
                        │             │
                  ┌─────▼─────────────▼──┐
                  │   CAS Storage Layer   │
                  │  (Immutable Objects)  │
                  └───────────────────────┘
```

#### Key Interactions

**1. Initial Model Scan** (User → CAS):
```
User: modeld scan ~/models/
  │
  ├─> Dedup Engine:
  │   ├─> For each file: Compute BLAKE3 hash (or cache lookup)
  │   ├─> GROUP BY hash to find duplicates
  │   └─> Write to Metadata Index (models table)
  │
  └─> Metadata Index:
      └─> Store: hash, size, format, arch, paths
```

**2. Deduplication** (Dedup Engine ↔ CAS ↔ Virtual FS):
```
User: modeld dedup
  │
  ├─> Dedup Engine:
  │   ├─> Query Metadata Index for duplicate groups
  │   ├─> Select canonical path (oldest, shortest)
  │   ├─> Two-phase commit:
  │   │   ├─> Phase A: Copy canonical → tmp/cas_staging/
  │   │   ├─> Phase B: Rename to cas/blake3/{prefix}/{hash}
  │   │   └─> Create hardlinks/symlinks at original paths
  │   └─> Update Metadata Index (aliases table)
  │
  └─> Virtual FS Layer:
      └─> Refresh: Create links in virtual/{frontend}/ directories
```

**3. HuggingFace Download** (Framework → HF Layer → CAS):
```
Framework: pipeline.from_pretrained("org/model")
  │
  ├─> HF Interception Layer:
  │   ├─> Check Metadata Index: downloads table (SHA256 → BLAKE3)
  │   ├─> Cache hit? Return fake HF cache symlink to CAS
  │   ├─> Cache miss? Download to tmp/downloads/
  │   ├─> Compute hashes: SHA256 (from HF) + BLAKE3
  │   ├─> Check if BLAKE3 already in CAS (user scan?)
  │   ├─> Move to CAS or reuse existing
  │   └─> Record mapping: (sha256, blake3) in downloads table
  │
  └─> Create fake HF cache structure:
      └─> blobs/{sha256} → symlink to cas/blake3/{prefix}/{hash}
```

**4. Model Loading** (Frontend → Virtual FS → CAS):
```
ComfyUI: Load model from ~/ComfyUI/models/checkpoints/sdxl.safetensors
  │
  ├─> Virtual FS Layer:
  │   ├─> Path: virtual/comfyui/checkpoints/sdxl.safetensors
  │   ├─> Link type: hardlink or symlink
  │   └─> Target: cas/blake3/ab/abcdef123...
  │
  └─> CAS Storage Layer:
      └─> Read immutable file: cas/blake3/ab/abcdef123...
          (Framework sees regular file, zero transparency)
```

**5. Garbage Collection** (GC ↔ Metadata Index ↔ Quarantine):
```
Scheduled: modeld gc --auto
  │
  ├─> Reference Tracker & GC:
  │   ├─> Phase 1: Validate all references
  │   │   ├─> Check aliases (paths still exist?)
  │   │   ├─> Check refs (workflows still exist?)
  │   │   └─> Remove stale entries
  │   │
  │   ├─> Phase 2: Query Metadata Index
  │   │   └─> SELECT models WHERE ref_count = 0 AND age > 7 days
  │   │
  │   ├─> Phase 3: Quarantine candidates
  │   │   ├─> Move: cas/blake3/{hash} → quarantine/{hash}.{timestamp}
  │   │   ├─> Write metadata: quarantine/{hash}.{timestamp}.meta
  │   │   └─> Update Metadata Index: quarantined_at = now()
  │   │
  │   └─> Phase 4: Delete expired
  │       └─> Query: quarantined_at < (now - 30 days)
  │           └─> Permanent deletion
  │
  └─> Virtual FS Layer:
      └─> Cleanup: Remove stale links
```

**6. Reference Tracking** (Workflow Parser → Metadata Index):
```
ComfyUI saves workflow: ~/ComfyUI/user/workflows/portrait.json
  │
  ├─> Workflow Parser:
  │   ├─> Extract model dependencies from JSON
  │   ├─> For each model reference:
  │   │   ├─> Resolve path → BLAKE3 hash
  │   │   ├─> Type: checkpoint | lora | vae | controlnet
  │   │   └─> INSERT INTO refs (model_hash, ref_source, ref_type)
  │   │
  │   └─> Update last_checked timestamp
  │
  └─> Reference Tracker:
      └─> Mark model as PROTECTED (ref_count > 0)
```

#### Data Flow Characteristics

**Read-Heavy Workload** (99% of operations):
- Model loading: Direct file access via hardlinks/symlinks (no overhead)
- Hash lookups: O(1) database queries with indexes
- Metadata queries: Cached by SQLite WAL mode

**Write Operations** (1% of operations):
- Initial scan: Sequential hash computation (2GB/s throughput)
- Deduplication: Two-phase commit (atomic, resumable)
- GC: Batch operations (weekly/monthly frequency)

**Concurrency**:
- **Reads**: Unlimited concurrent access (immutable CAS + WAL mode)
- **Writes**: Serialized through SQLite transactions
- **No locks needed**: For read-only operations

**Failure Recovery**:
- **Dedup crashes**: WAL enables resume from Phase A or B
- **Download failures**: Resumable via HTTP Range requests
- **Database corruption**: WAL rollback + integrity checks
- **GC errors**: Triple-check ref_count before deletion

### Data Flow Diagrams

#### Flow 1: Initial Scan & Dedup

This flow handles the initial model discovery, hash computation, and duplicate detection phase.

**Overview**: User scans a directory tree to discover models, compute content hashes, and identify duplicates.

**Flow Diagram**:

```
┌─────────────────────────────────────────────────────┐
│ User runs: modeld scan /path/to/models              │
└───────────────────┬─────────────────────────────────┘
                    │
                    ▼
        ┌───────────────────────────┐
        │ Initialize Scan Session   │
        │ - Create progress tracker │
        │ - Open database connection│
        │ - Load hash cache         │
        └───────────┬───────────────┘
                    │
                    ▼
        ┌───────────────────────────────┐
        │ Walk Directory Tree           │
        │ (Recursive, follow symlinks=no)│
        └───────────┬───────────────────┘
                    │
                    ▼
        ┌───────────────────────────────┐
        │ For each entry:               │
        │ Check file type               │
        └───────────┬───────────────────┘
                    │
         ┌──────────┴──────────┐
         │                     │
    Directory               File
         │                     │
         │                     ▼
         │         ┌───────────────────────┐
         │         │ Check extension:      │
         │         │ .safetensors, .gguf,  │
         │         │ .ckpt, .pth, .bin     │
         │         └───────┬───────────────┘
         │                 │
         │        ┌────────┴─────────┐
         │        │                  │
         │    Not Model         Is Model
         │        │                  │
         │    [Skip]                 ▼
         │              ┌────────────────────────┐
         │              │ Get file metadata:     │
         │              │ - path                 │
         │              │ - size (bytes)         │
         │              │ - mtime (modified time)│
         │              └────────┬───────────────┘
         │                       │
         │                       ▼
         │              ┌────────────────────────┐
         │              │ Query hash cache:      │
         │              │ SELECT hash FROM       │
         │              │ hash_cache WHERE       │
         │              │   path = ? AND         │
         │              │   mtime = ? AND        │
         │              │   size = ?             │
         │              └────────┬───────────────┘
         │                       │
         │              ┌────────┴────────┐
         │              │                 │
         │         Cache Hit        Cache Miss
         │              │                 │
         │              ▼                 ▼
         │    ┌─────────────────┐  ┌──────────────────┐
         │    │ Reuse cached    │  │ File size check  │
         │    │ BLAKE3 hash     │  └────────┬─────────┘
         │    │ (instant)       │           │
         │    └────────┬────────┘  ┌────────┴─────────┐
         │             │           │                  │
         │             │      < 10MB              ≥ 10MB
         │             │           │                  │
         │             │           ▼                  ▼
         │             │  ┌────────────────┐  ┌──────────────────┐
         │             │  │ Direct read    │  │ mmap + parallel  │
         │             │  │ strategy:      │  │ strategy:        │
         │             │  │ - Read entire  │  │ - Memory map file│
         │             │  │ - Single hash  │  │ - Split 64MB     │
         │             │  │ - Fast (~5ms)  │  │ - Parallel hash  │
         │             │  └────────┬───────┘  │ - 8 threads      │
         │             │           │          └──────┬───────────┘
         │             │           │                 │
         │             │           └────────┬────────┘
         │             │                    │
         │             │                    ▼
         │             │        ┌───────────────────────┐
         │             │        │ Compute BLAKE3 hash   │
         │             │        │ (256-bit / 64 hex)    │
         │             │        └───────┬───────────────┘
         │             │                │
         │             │                ▼
         │             │        ┌───────────────────────┐
         │             │        │ Update hash cache:    │
         │             │        │ INSERT OR REPLACE     │
         │             │        │ (path,mtime,size,hash)│
         │             │        └───────┬───────────────┘
         │             │                │
         │             └────────────────┘
         │                      │
         │                      ▼
         │          ┌───────────────────────────┐
         │          │ Check if model exists:    │
         │          │ SELECT FROM models        │
         │          │ WHERE blake3_hash = ?     │
         │          └───────┬───────────────────┘
         │                  │
         │         ┌────────┴─────────┐
         │         │                  │
         │      Exists            New Model
         │         │                  │
         │         ▼                  ▼
         │  ┌──────────────┐   ┌─────────────────┐
         │  │ UPDATE       │   │ INSERT model:   │
         │  │ last_seen    │   │ - blake3_hash   │
         │  │ timestamp    │   │ - size_bytes    │
         │  └──────┬───────┘   │ - format        │
         │         │            │ - created_at    │
         │         │            └────────┬────────┘
         │         │                     │
         │         └──────────┬──────────┘
         │                    │
         │                    ▼
         │        ┌───────────────────────────┐
         │        │ INSERT OR UPDATE alias:   │
         │        │ - model_hash              │
         │        │ - path                    │
         │        │ - alias_type='original'   │
         │        │ - frontend='user'         │
         │        └───────┬───────────────────┘
         │                │
         │                ▼
         │        ┌───────────────────────────┐
         │        │ Update progress:          │
         │        │ - Increment file count    │
         │        │ - Add to total bytes      │
         │        │ - Display progress bar    │
         │        └───────┬───────────────────┘
         │                │
         └────────────────┘
                          │
                          ▼
              [Continue to next file]
                          │
                          │ (After all files processed)
                          │
                          ▼
              ┌───────────────────────────┐
              │ Analyze duplicates:       │
              │ SELECT blake3_hash,       │
              │   COUNT(*) as count,      │
              │   SUM(size) as total_size │
              │ FROM models               │
              │ GROUP BY blake3_hash      │
              │ HAVING count > 1          │
              └───────┬───────────────────┘
                      │
                      ▼
              ┌───────────────────────────┐
              │ Calculate metrics:        │
              │ - Total duplicate groups  │
              │ - Potential space savings │
              │ - Cache hit rate          │
              └───────┬───────────────────┘
                      │
                      ▼
              ┌───────────────────────────┐
              │ Display report:           │
              │                           │
              │ "Scanned 500 models       │
              │  Found 25 duplicate groups│
              │  Potential savings: 78 GB"│
              │                           │
              │ Cache hits: 450/500 (90%) │
              │ Scan time: 2m 15s         │
              └───────────────────────────┘
```

**Step-by-Step Documentation**:

**Step 1: Initialize Scan Session**
- Creates progress tracking structures
- Opens database connection with WAL mode enabled
- Loads hash cache into memory (recent entries, LRU)
- Initializes file counters and byte counters
- Sets up thread pool for parallel hashing (default: num_cpus threads)

**Step 2: Walk Directory Tree**
- Uses `walkdir` crate (Rust) for recursive traversal
- Respects `.gitignore` patterns (optional, configurable)
- Does NOT follow symlinks (prevents infinite loops)
- Filters hidden files/directories (starting with `.`)
- Handles permission errors gracefully (log and continue)

**Step 3: File Type Check**
- Identifies model files by extension:
  - `.safetensors` (modern format, preferred)
  - `.gguf` (GGML Unified Format, LLaMA models)
  - `.ckpt` (legacy checkpoint format)
  - `.pth` (PyTorch tensor file)
  - `.bin` (binary format, transformers)
- Skips non-model files (logs, configs, text files)

**Step 4: Get File Metadata**
- Reads filesystem metadata via `std::fs::metadata()`
- Extracts:
  - **path**: Full absolute path
  - **size**: File size in bytes
  - **mtime**: Last modified timestamp (Unix epoch or system-specific)

**Step 5: Query Hash Cache**
- Checks database for existing hash record
- Cache key: `(path, mtime, size)` tuple
- **Cache hit**: All three match → reuse hash (instant, no I/O)
- **Cache miss**: Any mismatch → must rehash file

**Step 6: Hash Computation Strategy Selection**
- **Small files (<10MB)**:
  - Direct read strategy: `std::fs::read()`
  - Single-threaded BLAKE3 hash
  - Fast: ~5-20ms per file
  - Low memory: entire file fits in RAM
- **Large files (≥10MB)**:
  - Memory-mapped I/O: `mmap2` crate
  - Split into 64MB chunks
  - Parallel hashing via Rayon thread pool
  - Optimal for multi-GB files (2-3 GB/s throughput)

**Step 7: BLAKE3 Hash Computation**
- Computes 256-bit cryptographic hash
- Output format: 64 hexadecimal characters
- Example: `7a3d9f8e1c2b4a5f6e8d9c0b1a2e3d4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1e2f`
- Deterministic: same file → same hash (always)

**Step 8: Update Hash Cache**
- Writes cache record to database
- `INSERT OR REPLACE INTO hash_cache (path, mtime, size, hash, cached_at)`
- Enables fast incremental rescans (reuse hash if file unchanged)
- Cache eviction: LRU policy when > 100K entries

**Step 9: Check Model Existence**
- Queries models table: `WHERE blake3_hash = ?`
- **Exists**: Model already known (maybe from previous scan)
- **New**: First time seeing this content hash

**Step 10: Upsert Model Record**
- **If exists**: `UPDATE models SET last_seen = now()`
- **If new**: `INSERT INTO models (blake3_hash, size_bytes, format, created_at)`
- Metadata extraction (optional):
  - `format`: safetensors | gguf | ckpt
  - `arch`: SDXL | SD15 | Flux | LLaMA (from filename patterns)
  - `category`: checkpoint | lora | vae (heuristics)

**Step 11: Record Alias**
- Creates alias record linking path to hash
- `INSERT OR REPLACE INTO aliases (model_hash, path, alias_type, frontend)`
- `alias_type = 'original'` (not a deduplicated link yet)
- `frontend = 'user'` (user-managed file, not in virtual/)

**Step 12: Update Progress Display**
- Increments counters: files processed, bytes processed
- Updates progress bar: `[████████░░] 80% (400/500 files)`
- Shows speed: `Processing at 2.5 GB/s | ETA: 1m 30s`
- Cache hit rate: `Cache hits: 360/400 (90%)`

**Step 13: Duplicate Analysis**
- After all files processed, queries for duplicates
- Groups by `blake3_hash`, counts occurrences
- Calculates potential space savings: `SUM(size) - MIN(size)` per group

**Step 14: Final Report**
- Displays summary:
  - Total models scanned
  - Duplicate groups found
  - Potential space savings (GB)
  - Cache effectiveness (hit rate %)
  - Total scan time
- Suggests next action: `modeld dedup` to reclaim space

**Error Handling**:

| Error Condition | Handling Strategy | Impact |
|----------------|-------------------|---------|
| Permission denied | Log warning, skip file | Scan continues, file excluded |
| File disappeared | Log info, skip | Scan continues (file deleted during scan) |
| Corrupted file | Log error, mark as corrupted | Scan continues, file not added to database |
| Hash cache DB error | Disable cache, continue | Slower scan (no cache), but functional |
| Disk full (can't write DB) | Abort scan, rollback transaction | User must free space |
| Out of memory | Reduce thread pool, retry | Slower but completes |

**Decision Points**:

1. **Extension check**: Is this a model file? (Yes → process, No → skip)
2. **Cache lookup**: Hash cached? (Yes → reuse, No → compute)
3. **File size**: <10MB or ≥10MB? (Determines hashing strategy)
4. **Model existence**: Already in DB? (Yes → update timestamp, No → insert)
5. **Continue scan**: More files? (Yes → next file, No → analyze duplicates)

#### Flow 2: Deduplication Execution

This flow handles the safe, transactional process of replacing duplicate files with hardlinks/symlinks to CAS storage.

**Overview**: After scan identifies duplicates, user runs dedup to replace duplicates with space-efficient links to a single canonical copy.

**Flow Diagram**:

```
┌─────────────────────────────────────────────────────┐
│ User runs: modeld dedup [--interactive|--auto]      │
└───────────────────┬─────────────────────────────────┘
                    │
                    ▼
        ┌───────────────────────────┐
        │ Query duplicate groups:   │
        │ SELECT blake3_hash,       │
        │   COUNT(*) as copies,     │
        │   GROUP_CONCAT(path)      │
        │ FROM aliases              │
        │ GROUP BY model_hash       │
        │ HAVING copies > 1         │
        └───────────┬───────────────┘
                    │
                    ▼
        ┌───────────────────────────┐
        │ For each duplicate group: │
        │ (Iterate sequentially)    │
        └───────────┬───────────────┘
                    │
                    ▼
        ┌───────────────────────────────┐
        │ Select Canonical Path:        │
        │ Priority order:               │
        │ 1. Already in CAS?            │
        │ 2. Oldest mtime               │
        │ 3. Shortest path              │
        │ 4. Alphabetically first       │
        └───────────┬───────────────────┘
                    │
                    ▼
        ┌────────────────────────────────┐
        │ Interactive mode?              │
        └───────┬────────────────────────┘
                │
       ┌────────┴─────────┐
       │                  │
   Yes (--interactive)  No (--auto)
       │                  │
       ▼                  │
┌──────────────────┐      │
│ Display group:   │      │
│ - Canonical path │      │
│ - Duplicate paths│      │
│ - Space savings  │      │
│                  │      │
│ Prompt user:     │      │
│ [Y/n/skip/abort] │      │
└───────┬──────────┘      │
        │                 │
   ┌────┴───────┐         │
   │            │         │
  Skip       Abort        │
   │            │         │
[Next]    [Exit]          │
           │              │
           └────────────┬─┘
                        │
                        ▼
            ┌───────────────────────────┐
            │ PHASE A: PREPARE          │
            │ (Write-Ahead, Safe)       │
            └───────────┬───────────────┘
                        │
                        ▼
            ┌───────────────────────────┐
            │ Step A1: Generate tx_id   │
            │ (UUID: e.g., 550e8400...) │
            └───────────┬───────────────┘
                        │
                        ▼
            ┌─────────────────────────────────┐
            │ Step A2: Write WAL record       │
            │ INSERT INTO wal_transactions:   │
            │ - tx_id                         │
            │ - operation='dedup'             │
            │ - status='pending'              │
            │ - source_path (canonical)       │
            │ - target_hash (BLAKE3)          │
            │ - metadata (JSON: dup paths)    │
            └───────────┬─────────────────────┘
                        │
                        ▼
            ┌─────────────────────────────────┐
            │ Step A3: Copy to staging        │
            │ Source: canonical_path          │
            │ Dest: tmp/cas_staging/{hash}.tmp│
            │ Method: Buffered I/O (64KB)     │
            └───────────┬─────────────────────┘
                        │
                        ▼ (Error: I/O failure?)
                        │
              ┌─────────┴──────────┐
              │                    │
          Success              Failure
              │                    │
              │                    ▼
              │        ┌───────────────────────┐
              │        │ Rollback: Delete tmp  │
              │        │ UPDATE WAL:           │
              │        │   status='failed'     │
              │        │ Log error             │
              │        │ [Skip to next group]  │
              │        └───────────────────────┘
              │
              ▼
    ┌───────────────────────────────┐
    │ Step A4: Verify hash          │
    │ Compute BLAKE3 of staging file│
    │ Compare: computed == expected │
    └───────────┬───────────────────┘
                │
                ▼ (Error: Hash mismatch?)
                │
       ┌────────┴─────────┐
       │                  │
   Match             Mismatch
       │                  │
       │                  ▼
       │      ┌───────────────────────┐
       │      │ Rollback: Delete tmp  │
       │      │ UPDATE WAL:           │
       │      │   status='failed',    │
       │      │   error='hash_mismatch'│
       │      │ Log corruption warning│
       │      │ [Skip to next group]  │
       │      └───────────────────────┘
       │
       ▼
┌───────────────────────────────┐
│ Step A5: Update WAL           │
│ UPDATE wal_transactions       │
│ SET status='copied',          │
│     updated_at=now()          │
│ WHERE tx_id=?                 │
└───────────┬───────────────────┘
            │
            ▼
┌───────────────────────────────┐
│ Step A6: fsync WAL            │
│ Force WAL write to disk       │
│ (Ensures crash recovery sees  │
│  'copied' status)             │
└───────────┬───────────────────┘
            │
            ▼
┌───────────────────────────────┐
│ PHASE B: COMMIT               │
│ (Make changes visible)        │
└───────────┬───────────────────┘
            │
            ▼
┌─────────────────────────────────────┐
│ Step B1: Atomic rename to CAS       │
│ Source: tmp/cas_staging/{hash}.tmp  │
│ Dest: cas/blake3/{prefix}/{hash}    │
│                                     │
│ Same filesystem? → rename() atomic  │
│ Cross-filesystem? → copy+verify+del │
└───────────┬─────────────────────────┘
            │
            ▼ (Error: Rename fails?)
            │
   ┌────────┴─────────┐
   │                  │
Success           Failure
   │                  │
   │                  ▼
   │      ┌──────────────────────┐
   │      │ Retry rename (3x)    │
   │      │ If still fails:      │
   │      │ - Keep in staging    │
   │      │ - Mark WAL failed    │
   │      │ - Manual recovery    │
   │      └──────────────────────┘
   │
   ▼
┌───────────────────────────────────┐
│ Step B2: For each duplicate path: │
│ (Process sequentially)            │
└───────────┬───────────────────────┘
            │
            ▼
┌───────────────────────────────────┐
│ Determine link strategy:          │
│ - Check volume IDs (same/cross)   │
│ - Check privileges (Windows)      │
│ - Choose: hardlink > symlink >    │
│           junction > ref-only     │
└───────────┬───────────────────────┘
            │
            ▼
┌───────────────────────────────────┐
│ Strategy decision:                │
└───┬───────────────────────────────┘
    │
    ├──> Same volume?
    │    └─> Create hardlink
    │        (Zero overhead, transparent)
    │
    ├──> Cross volume + symlink privilege?
    │    └─> Create symlink
    │        (Requires Windows Developer Mode)
    │
    ├──> Cross volume + no privilege?
    │    └─> Reference-only mode
    │        (No space savings, DB tracking only)
    │
    └──> Directory (Windows)?
         └─> Create junction point
             (No privileges needed)
    │
    ▼
┌─────────────────────────────────────┐
│ Execute link creation:              │
│                                     │
│ Windows:                            │
│ - Hardlink: CreateHardLink()       │
│ - Symlink: CreateSymbolicLink()    │
│ - Junction: DeviceIoControl()      │
│                                     │
│ Unix:                               │
│ - Hardlink: std::fs::hard_link()   │
│ - Symlink: std::os::unix::fs::symlink()│
└───────────┬─────────────────────────┘
            │
            ▼ (Error: Link creation fails?)
            │
   ┌────────┴─────────┐
   │                  │
Success           Failure
   │                  │
   │                  ▼
   │      ┌──────────────────────────┐
   │      │ Log error (non-fatal):   │
   │      │ "Failed to link {path}"  │
   │      │ Reason: {error}          │
   │      │                          │
   │      │ Keep original file       │
   │      │ Mark in aliases:         │
   │      │   link_failed=true       │
   │      │                          │
   │      │ [Continue to next path]  │
   │      └──────────────────────────┘
   │
   ▼
┌─────────────────────────────────────┐
│ Step B3: Delete/Quarantine original│
│ If link creation succeeded:        │
│                                     │
│ Check ref_count(original_path):    │
│ - Count aliases pointing to path   │
│ - Count explicit refs (workflows)  │
└───────────┬─────────────────────────┘
            │
   ┌────────┴─────────┐
   │                  │
ref_count > 0    ref_count = 0
   │                  │
[Keep file]           ▼
   │      ┌───────────────────────────┐
   │      │ Move to quarantine:       │
   │      │ Source: original_path     │
   │      │ Dest: quarantine/{hash}.  │
   │      │       {timestamp}         │
   │      │                           │
   │      │ Create metadata:          │
   │      │ {hash}.{timestamp}.meta   │
   │      │ (JSON: paths, reason)     │
   │      └───────┬───────────────────┘
   │              │
   └──────────────┘
                  │
                  ▼
        ┌─────────────────────────────┐
        │ Step B4: Update aliases     │
        │ UPDATE aliases SET:         │
        │ - alias_type='hardlink'|    │
        │               'symlink'     │
        │ - updated_at=now()          │
        │ WHERE path=? AND            │
        │       model_hash=?          │
        └───────────┬─────────────────┘
                    │
                    ▼
        ┌─────────────────────────────┐
        │ Step B5: Calculate savings  │
        │ space_saved = file_size *   │
        │   (num_duplicates - 1)      │
        └───────────┬─────────────────┘
                    │
                    ▼
        ┌─────────────────────────────┐
        │ Step B6: Update WAL         │
        │ UPDATE wal_transactions     │
        │ SET status='committed',     │
        │     updated_at=now(),       │
        │     metadata=json_set(      │
        │       '$.space_saved',      │
        │       {space_saved})        │
        │ WHERE tx_id=?               │
        └───────────┬─────────────────┘
                    │
                    ▼
        ┌─────────────────────────────┐
        │ Step B7: Report progress    │
        │ Display:                    │
        │ "✓ Group {n}/{total}        │
        │  Saved {size} GB            │
        │  ({num} duplicates removed)"│
        └───────────┬─────────────────┘
                    │
                    ▼
        ┌─────────────────────────────┐
        │ Optionally: Clean up WAL    │
        │ DELETE FROM wal_transactions│
        │ WHERE status='committed'    │
        │   AND age > 7 days          │
        │ (Or keep for audit trail)   │
        └───────────┬─────────────────┘
                    │
                    ▼
            [Next duplicate group]
                    │
                    │ (After all groups)
                    │
                    ▼
        ┌─────────────────────────────┐
        │ Final Summary:              │
        │                             │
        │ "Deduplication complete:    │
        │  - 22/25 groups processed   │
        │  - 74.4 GB space saved      │
        │  - 3 groups skipped         │
        │    (no privileges)          │
        │                             │
        │  Total time: 5m 32s"        │
        └─────────────────────────────┘
```

**Step-by-Step Documentation**:

**Phase A: Prepare (Write-Ahead)**

**Step A1: Generate Transaction ID**
- Creates unique UUID for this deduplication operation
- Example: `550e8400-e29b-41d4-a716-446655440000`
- Used to track operation in WAL and enable recovery

**Step A2: Write WAL Record**
- Inserts transaction record with `status='pending'`
- Records:
  - `tx_id`: Transaction identifier
  - `operation='dedup'`: Operation type
  - `source_path`: Canonical file path
  - `target_hash`: BLAKE3 hash (64 hex chars)
  - `metadata`: JSON with duplicate paths, expected space savings
- Purpose: If crash occurs, recovery can detect incomplete transaction

**Step A3: Copy to Staging**
- Copies canonical file to temporary staging area
- Destination: `tmp/cas_staging/{blake3_hash}.tmp`
- Uses buffered I/O (64KB blocks) for efficiency
- Preserves timestamps where possible (forensics)
- **Critical**: Original files remain untouched until Phase B

**Step A4: Verify Hash**
- Recomputes BLAKE3 hash of staging file
- Compares: `computed_hash == target_hash`
- **If mismatch**: Source file corrupted or changed during copy
  - Delete staging file
  - Mark transaction failed
  - Log corruption warning
  - Skip this group (don't deduplicate corrupted data)
- **If match**: Proceed to Phase B

**Step A5: Update WAL Status**
- Updates transaction record: `status='copied'`
- Indicates Phase A completed successfully
- Crash recovery can resume from Phase B

**Step A6: fsync WAL**
- Forces WAL write to physical disk
- Ensures crash recovery will see `'copied'` status
- Without fsync: power loss might lose WAL update → safe rollback

**Phase B: Commit (Make Changes Visible)**

**Step B1: Atomic Rename to CAS**
- Moves staging file to final CAS location
- Source: `tmp/cas_staging/{hash}.tmp`
- Destination: `cas/blake3/{prefix}/{hash}`
- **Same filesystem**: `rename()` is atomic (POSIX guarantee)
- **Cross-filesystem**: Fallback to copy + verify + delete (slower)
- **If fails**: Retry 3 times, then mark for manual recovery

**Step B2-B3: Link Creation Strategy**

| Scenario | Link Type | Requirements | Space Savings | Notes |
|----------|-----------|--------------|---------------|-------|
| Same volume (Unix/Linux) | Hardlink | None | 100% | Zero overhead, transparent |
| Same volume (Windows NTFS) | Hardlink | None | 100% | Same inode, atomic |
| Cross-volume (Unix) | Symlink | Standard privilege | 100% | Pointer to CAS path |
| Cross-volume (Windows with privilege) | Symlink | Developer Mode | 100% | Requires `SeCreateSymbolicLinkPrivilege` |
| Cross-volume (Windows no privilege) | Reference-only | None | 0% | Database tracking only, no link |
| Directory (Windows) | Junction | None | N/A | NTFS reparse point |

**Step B4: Update Aliases Table**
- Records link in database
- `UPDATE aliases SET alias_type='hardlink'|'symlink', updated_at=now()`
- Enables ref counting for GC
- Tracks which paths are deduplicated vs original

**Step B5: Quarantine Original Files**
- Checks `ref_count` for each replaced file
- **If ref_count > 0**: Keep original (still referenced)
- **If ref_count = 0**: Move to quarantine
  - Destination: `quarantine/{hash}.{unix_timestamp}`
  - Creates metadata file: `{hash}.{timestamp}.meta` (JSON)
  - Grace period: 30 days (configurable)
  - Enables recovery if deduplication was mistake

**Step B6: Calculate Space Savings**
- Formula: `space_saved = file_size × (num_duplicates - 1)`
- Example: 3 copies of 6.94GB → saved 13.88GB
- Accumulates total savings across all groups

**Step B7: Update WAL and Report**
- Marks transaction committed: `status='committed'`
- Records final space savings in metadata JSON
- Displays progress: `"✓ Group 5/25: Saved 13.88 GB"`

**Error Handling**:

| Error Condition | Phase | Handling Strategy | Recovery |
|----------------|-------|-------------------|----------|
| Copy fails (disk full) | A3 | Rollback, mark failed, skip group | No data loss |
| Hash mismatch | A4 | Rollback, log corruption, skip | Preserves original |
| Crash during Phase A | A1-A6 | On restart: detect `pending`, resume or rollback | Safe, no permanent changes |
| Rename fails | B1 | Retry 3x, then manual recovery | Staging file preserved |
| Link creation fails | B2 | Log error, keep original, continue | Non-fatal, group partially deduplicated |
| Crash during Phase B | B1-B7 | On restart: detect `copied`, resume from B1 | Resumable, no data loss |
| Quarantine move fails | B5 | Log error, keep original | Non-critical, space not freed |

**Decision Points**:

1. **Mode selection**: Interactive or auto? (Interactive → prompt user per group)
2. **User confirmation**: (Interactive only) Proceed with this group? (Y/n/skip/abort)
3. **Hash verification**: Does computed hash match expected? (Mismatch → rollback)
4. **Link strategy**: Same volume? Privileges available? (Determines link type)
5. **Ref count check**: Original file still referenced? (>0 → keep, =0 → quarantine)
6. **Continue dedup**: More groups to process? (Yes → next group, No → final summary)

#### Flow 3: HuggingFace Download Interception

This flow handles transparent interception of HuggingFace model downloads, deduplication across frameworks, and SHA256↔BLAKE3 hash mapping.

**Overview**: When AI frameworks download models from HuggingFace Hub, modeld intercepts the download, checks for existing content (by SHA256 or BLAKE3 hash), and creates fake HF cache structures pointing to deduplicated CAS storage.

**Flow Diagram**:

```
┌─────────────────────────────────────────────────────────┐
│ User Python code:                                        │
│ pipeline = DiffusionPipeline.from_pretrained(           │
│     "stabilityai/stable-diffusion-xl-base-1.0"          │
│ )                                                        │
└───────────────────┬─────────────────────────────────────┘
                    │
                    ▼
        ┌───────────────────────────────┐
        │ HuggingFace Library           │
        │ (diffusers, transformers)     │
        │                               │
        │ Calls: hf_hub_download()      │
        │ or: snapshot_download()       │
        └───────────┬───────────────────┘
                    │
                    ▼
        ┌───────────────────────────────┐
        │ Check: HF_HOME env variable   │
        │ Value: $MODELD_STORE/hf_cache │
        └───────────┬───────────────────┘
                    │
                    ▼
        ┌─────────────────────────────────────┐
        │ Construct expected cache path:      │
        │ $HF_HOME/hub/                       │
        │   models--{org}--{model}/           │
        │   snapshots/{revision}/{filename}   │
        └───────────┬─────────────────────────┘
                    │
                    ▼
        ┌───────────────────────────┐
        │ Check: Path exists?       │
        └───────┬───────────────────┘
                │
       ┌────────┴─────────┐
       │                  │
    Exists            Not Found
       │                  │
       │                  ▼
       │      ┌───────────────────────────┐
       │      │ DOWNLOAD INITIATION       │
       │      │ (modeld intercepts)       │
       │      └───────────┬───────────────┘
       │                  │
       │                  ▼
       │      ┌───────────────────────────────┐
       │      │ Fetch HF metadata:            │
       │      │ GET {repo}/resolve/{rev}/{file}│
       │      │ (HEAD request, no download)   │
       │      └───────────┬───────────────────┘
       │                  │
       │                  ▼
       │      ┌───────────────────────────────┐
       │      │ Extract SHA256 from:          │
       │      │ Priority 1: X-Linked-Etag     │
       │      │   header (most reliable)      │
       │      │ Priority 2: .huggingface.json │
       │      │ Priority 3: Compute ourselves │
       │      └───────────┬───────────────────┘
       │                  │
       │                  ▼ (SHA256 extracted)
       │                  │
       │      ┌────────────────────────────────┐
       │      │ Query downloads table:         │
       │      │ SELECT model_hash, status      │
       │      │ FROM downloads                 │
       │      │ WHERE sha256_hash = ?          │
       │      └────────┬───────────────────────┘
       │               │
       │      ┌────────┴─────────┐
       │      │                  │
       │  Found                Not Found
       │      │                  │
       │      ▼                  ▼
       │ ┌────────────────┐  ┌──────────────────────────┐
       │ │ CACHE HIT!     │  │ DOWNLOAD REQUIRED        │
       │ │ (SHA256 known) │  │ (New model)              │
       │ └────────┬───────┘  └──────────┬───────────────┘
       │          │                     │
       │          │                     ▼
       │          │         ┌──────────────────────────────┐
       │          │         │ Download to temp:            │
       │          │         │ tmp/downloads/{uuid}.part    │
       │          │         │                              │
       │          │         │ Features:                    │
       │          │         │ - HTTP Range support (resume)│
       │          │         │ - Progress callback          │
       │          │         │ - Auth token (if private)    │
       │          │         └──────────┬───────────────────┘
       │          │                    │
       │          │                    ▼ (Error: Network failure?)
       │          │                    │
       │          │           ┌────────┴─────────┐
       │          │           │                  │
       │          │       Success            Failure
       │          │           │                  │
       │          │           │                  ▼
       │          │           │      ┌───────────────────────┐
       │          │           │      │ Retry with backoff:   │
       │          │           │      │ - 3 retries           │
       │          │           │      │ - Exponential delay   │
       │          │           │      │ - Resume from partial │
       │          │           │      │   (HTTP Range)        │
       │          │           │      │                       │
       │          │           │      │ Still fails?          │
       │          │           │      │ → Mark download failed│
       │          │           │      │ → Return error to user│
       │          │           │      └───────────────────────┘
       │          │           │
       │          │           ▼
       │          │ ┌──────────────────────────────┐
       │          │ │ Download complete            │
       │          │ │ File at: tmp/downloads/      │
       │          │ │          {uuid}.part         │
       │          │ └──────────┬───────────────────┘
       │          │            │
       │          │            ▼
       │          │ ┌──────────────────────────────┐
       │          │ │ Verify SHA256 (if provided): │
       │          │ │ Compute SHA256 of download   │
       │          │ │ Compare: computed == expected│
       │          │ └──────────┬───────────────────┘
       │          │            │
       │          │   ┌────────┴─────────┐
       │          │   │                  │
       │          │ Match            Mismatch
       │          │   │                  │
       │          │   │                  ▼
       │          │   │      ┌───────────────────────┐
       │          │   │      │ ERROR: Corrupted      │
       │          │   │      │ Delete temp file      │
       │          │   │      │ Report to HF (corrupt)│
       │          │   │      │ Return error to user  │
       │          │   │      └───────────────────────┘
       │          │   │
       │          │   ▼
       │          │ ┌──────────────────────────────┐
       │          │ │ Compute BLAKE3 hash:         │
       │          │ │ blake3({uuid}.part)          │
       │          │ └──────────┬───────────────────┘
       │          │            │
       │          │            ▼
       │          │ ┌──────────────────────────────┐
       │          │ │ Query models table:          │
       │          │ │ SELECT * FROM models         │
       │          │ │ WHERE blake3_hash = ?        │
       │          │ └──────┬───────────────────────┘
       │          │        │
       │          │ ┌──────┴─────────┐
       │          │ │                │
       │          │ Found          Not Found
       │          │ │                │
       │          │ ▼                ▼
       │          │ ┌──────────┐  ┌──────────────────┐
       │          │ │ DEDUP!   │  │ NEW MODEL        │
       │          │ │ (BLAKE3  │  │ Add to CAS       │
       │          │ │ exists)  │  │                  │
       │          │ └────┬─────┘  └────────┬─────────┘
       │          │      │                 │
       │          │      ▼                 ▼
       │          │ ┌────────────┐  ┌──────────────────┐
       │          │ │ Delete temp│  │ Move to CAS:     │
       │          │ │ file       │  │ tmp → cas/blake3/│
       │          │ │ (already   │  │ {prefix}/{hash}  │
       │          │ │ have it!)  │  │                  │
       │          │ └────┬───────┘  │ INSERT models:   │
       │          │      │          │ - blake3_hash    │
       │          │      │          │ - size_bytes     │
       │          │      │          │ - format         │
       │          │      │          │ - created_at     │
       │          │      │          └────────┬─────────┘
       │          │      │                   │
       │          └──────┴───────────────────┘
       │                            │
       │                            ▼
       │                ┌────────────────────────────┐
       │                │ Record download mapping:   │
       │                │ INSERT INTO downloads:     │
       │                │ - model_hash (BLAKE3)      │
       │                │ - sha256_hash              │
       │                │ - source_url               │
       │                │ - status='done'            │
       │                │ - bytes_total              │
       │                │ - finished_at              │
       │                └────────┬───────────────────┘
       │                         │
       │                         ▼
       │             ┌────────────────────────────────┐
       │             │ CREATE FAKE HF CACHE STRUCTURE │
       │             │ (Make modeld transparent)      │
       │             └────────┬───────────────────────┘
       │                      │
       │                      ▼
       │          ┌──────────────────────────────────────┐
       │          │ Step 1: Create model directory       │
       │          │ $HF_HOME/hub/                        │
       │          │   models--{org}--{model}/            │
       │          └──────────┬───────────────────────────┘
       │                     │
       │                     ▼
       │          ┌──────────────────────────────────────┐
       │          │ Step 2: Create refs/                 │
       │          │ Write: refs/main                     │
       │          │ Content: {revision_hash}             │
       │          │ (Text file with commit hash)         │
       │          └──────────┬───────────────────────────┘
       │                     │
       │                     ▼
       │          ┌──────────────────────────────────────┐
       │          │ Step 3: Create blob symlink          │
       │          │ Target: blobs/{sha256}               │
       │          │ Points to: ../../../../cas/blake3/   │
       │          │            {prefix}/{blake3_hash}    │
       │          │                                      │
       │          │ Windows: symlink or junction         │
       │          │ Unix: symlink                        │
       │          └──────────┬───────────────────────────┘
       │                     │
       │                     ▼
       │          ┌──────────────────────────────────────┐
       │          │ Step 4: Create snapshot symlink      │
       │          │ Target: snapshots/{revision}/        │
       │          │         {filename}                   │
       │          │ Points to: ../../blobs/{sha256}      │
       │          │                                      │
       │          │ (Relative symlink within HF cache)   │
       │          └──────────┬───────────────────────────┘
       │                     │
       │                     ▼
       │          ┌──────────────────────────────────────┐
       │          │ Step 5: Write .huggingface.json      │
       │          │ Location: blobs/{sha256}.huggingface │
       │          │ Content (JSON):                      │
       │          │ {                                    │
       │          │   "sha256": "{sha256_hash}",         │
       │          │   "size": {bytes},                   │
       │          │   "url": "{hf_url}",                 │
       │          │   "etag": "\"{sha256}\""             │
       │          │ }                                    │
       │          └──────────┬───────────────────────────┘
       │                     │
       └─────────────────────┘
                             │
                             ▼
                 ┌────────────────────────────┐
                 │ Return path to HF library: │
                 │ snapshots/{rev}/{filename} │
                 └────────┬───────────────────┘
                          │
                          ▼
                 ┌────────────────────────────┐
                 │ HF library loads model:    │
                 │ - Follows snapshot symlink │
                 │ - Follows blob symlink     │
                 │ - Reads from CAS           │
                 │                            │
                 │ (Transparent to framework) │
                 └────────┬───────────────────┘
                          │
                          ▼
                 ┌────────────────────────────┐
                 │ Model loaded successfully  │
                 │                            │
                 │ User code continues:       │
                 │ image = pipeline(prompt)   │
                 └────────────────────────────┘
```

**Step-by-Step Documentation**:

**Phase 1: Download Request Interception**

**Step 1: User Code Execution**
- User runs Python code: `pipeline.from_pretrained("org/model")`
- Framework (diffusers, transformers) needs to load model
- Internally calls `huggingface_hub.hf_hub_download()`

**Step 2: HF_HOME Check**
- HuggingFace library checks environment: `os.getenv("HF_HOME")`
- modeld has set: `HF_HOME=$MODELD_STORE/hf_cache`
- Library will use this as cache root (instead of `~/.cache/huggingface/`)

**Step 3: Cache Path Construction**
- HF library constructs expected path:
  - Format: `{HF_HOME}/hub/models--{org}--{model}/snapshots/{revision}/{filename}`
  - Example: `$MODELD_STORE/hf_cache/hub/models--stabilityai--stable-diffusion-xl-base-1.0/snapshots/2b5db9c4/unet/diffusion_pytorch_model.safetensors`

**Step 4: Path Existence Check**
- HF library checks if path exists
- **If exists**: Return path immediately (cache hit)
- **If not exists**: Trigger download

**Phase 2: Download Execution**

**Step 5: Fetch HF Metadata**
- modeld intercepts download request
- Sends HTTP HEAD request to HuggingFace:
  - URL: `https://huggingface.co/{org}/{model}/resolve/{revision}/{filename}`
  - Auth: If private repo, include token from `HF_TOKEN` env
- Response headers contain:
  - `X-Linked-Etag`: SHA256 hash (in quotes)
  - `X-Linked-Size`: File size in bytes
  - `X-Repo-Commit`: Revision hash

**Step 6: Extract SHA256 Hash**
- **Priority 1**: HTTP header `X-Linked-Etag`
  - Most reliable, always present
  - Example: `"594b2fd5af80c522b2c851d5fc94e7c1b161df9dce0b1c0c6d70a1e0d9534d3e"`
  - Strip quotes: `594b2fd5...`
- **Priority 2**: Cached `.huggingface.json` file
  - If file was previously downloaded
  - Contains: `{"sha256": "594b2fd5...", ...}`
- **Priority 3**: Compute after download
  - Fallback if HF doesn't provide hash
  - Slower but ensures correctness

**Step 7: Query Downloads Table**
- Checks if model already downloaded:
  ```sql
  SELECT model_hash, status FROM downloads WHERE sha256_hash = ?
  ```
- **If found and status='done'**: Cache hit! (Go to Step 13)
- **If not found**: Download required (Continue to Step 8)

**Step 8: Download to Temporary Location**
- Downloads file to: `tmp/downloads/{uuid}.part`
  - UUID prevents collisions for concurrent downloads
  - `.part` suffix indicates incomplete download
- Download features:
  - **HTTP Range requests**: Supports resume if interrupted
  - **Progress callback**: Reports download progress to user
  - **Auth tokens**: Includes `Authorization: Bearer {HF_TOKEN}` for private repos
  - **Retry logic**: 3 retries with exponential backoff (1s, 2s, 4s)

**Error Handling: Network Failures**
- **Connection timeout**: Retry with longer timeout
- **HTTP 5xx errors**: Retry (server temporary error)
- **HTTP 404**: Abort (model doesn't exist)
- **HTTP 401/403**: Abort (auth failed, check HF_TOKEN)
- **Partial download**: Resume from byte offset (HTTP Range: bytes={offset}-)

**Step 9: Verify SHA256 (if provided)**
- If HF provided SHA256 in metadata:
  - Compute SHA256 of downloaded file
  - Compare: `computed_sha256 == expected_sha256`
  - **If mismatch**: Download corrupted
    - Delete temp file
    - Report to HF (corrupted blob)
    - Return error to user
  - **If match**: Proceed

**Step 10: Compute BLAKE3 Hash**
- Computes BLAKE3 hash of downloaded file
- Uses same strategy as scan: mmap + parallel chunks
- Result: 64-character hex string

**Step 11: Check if BLAKE3 Exists in CAS**
- Queries models table:
  ```sql
  SELECT * FROM models WHERE blake3_hash = ?
  ```
- **If found**: Deduplication! (File already in CAS from user scan or previous download)
  - Delete temp file (no need to store duplicate)
  - Reuse existing CAS object
- **If not found**: New model, add to CAS

**Step 12: Move to CAS (if new model)**
- Moves file from temp to CAS:
  - Source: `tmp/downloads/{uuid}.part`
  - Destination: `cas/blake3/{prefix}/{blake3_hash}`
- Uses atomic rename if same filesystem
- Inserts model record:
  ```sql
  INSERT INTO models (blake3_hash, size_bytes, format, created_at) VALUES (?, ?, ?, ?)
  ```

**Phase 3: Hash Mapping and Cache Structure**

**Step 13: Record Download Mapping**
- Inserts download record linking SHA256 ↔ BLAKE3:
  ```sql
  INSERT INTO downloads (model_hash, sha256_hash, source_url, status, bytes_total, finished_at)
  VALUES (?, ?, ?, 'done', ?, now())
  ```
- Enables future cache hits for same SHA256
- Bidirectional mapping: can find model by either hash

**Step 14: Create Fake HF Cache Structure**

**Step 14a: Create Model Directory**
- Creates: `$HF_HOME/hub/models--{org}--{model}/`
- Creates subdirectories: `blobs/`, `refs/`, `snapshots/{revision}/`

**Step 14b: Create refs/main**
- Writes text file: `refs/main`
- Content: `{revision_hash}` (e.g., `2b5db9c4dd522e00db3ddb43c923aa8714e97ed7`)
- HF library reads this to determine latest revision

**Step 14c: Create Blob Symlink**
- Target: `blobs/{sha256_hash}`
- Points to: `../../../../cas/blake3/{prefix}/{blake3_hash}`
- **Windows**: Uses symlink (if privilege) or junction (for directories)
- **Unix**: Standard symlink
- Example:
  ```
  blobs/594b2fd5... → ../../../../cas/blake3/7a/7a3d9f8e...
  ```

**Step 14d: Create Snapshot Symlink**
- Target: `snapshots/{revision}/{filename}`
- Points to: `../../blobs/{sha256_hash}` (relative within HF cache)
- Example:
  ```
  snapshots/2b5db9c4/unet/diffusion_pytorch_model.safetensors → ../../blobs/594b2fd5...
  ```
- This allows HF library to navigate: snapshot → blob → CAS

**Step 14e: Write .huggingface.json**
- Creates metadata file: `blobs/{sha256_hash}.huggingface.json`
- Content:
  ```json
  {
    "sha256": "594b2fd5af80c522b2c851d5fc94e7c1b161df9dce0b1c0c6d70a1e0d9534d3e",
    "size": 4265380512,
    "url": "https://huggingface.co/stabilityai/stable-diffusion-xl-base-1.0/resolve/2b5db9c4/unet/diffusion_pytorch_model.safetensors",
    "etag": "\"594b2fd5af80c522b2c851d5fc94e7c1b161df9dce0b1c0c6d70a1e0d9534d3e\""
  }
  ```
- Used by HF library for metadata, offline mode

**Phase 4: Return to Framework**

**Step 15: Return Path to HF Library**
- Returns path: `snapshots/{revision}/{filename}`
- HF library follows symlink chain:
  1. `snapshots/{rev}/{file}` → `blobs/{sha256}`
  2. `blobs/{sha256}` → `cas/blake3/{prefix}/{blake3_hash}`
- Framework loads model from CAS (transparent)

**Step 16: Model Loading**
- Framework reads file via symlinks
- Parses model format (safetensors, PyTorch, GGUF)
- Loads tensors into memory
- User code continues execution
- **Zero awareness**: Framework thinks it loaded from normal HF cache

**Error Handling**:

| Error Condition | Phase | Handling Strategy | User Impact |
|----------------|-------|-------------------|-------------|
| Network timeout | 2 | Retry 3x with backoff, then abort | Download fails, error message |
| SHA256 mismatch | 2 | Delete temp, report corruption | Download fails, suggests retry |
| Disk full (temp) | 2 | Abort, cleanup partial | Download fails, "disk full" error |
| Disk full (CAS) | 2 | Abort, cleanup temp | Download fails, "disk full" error |
| Symlink creation fails | 3 | Retry with junction (Windows) or copy | Works but slower (copy vs link) |
| Permission denied (HF cache) | 3 | Abort, suggest directory permissions | Download fails, permission error |
| Corrupted CAS object | 4 | Redownload, replace CAS object | Automatic recovery, logged |
| HF API rate limit | 2 | Exponential backoff, respect Retry-After | Delayed download, eventual success |

**Decision Points**:

1. **Path exists?**: Does snapshot path exist? (Yes → cache hit, No → download)
2. **SHA256 known?**: Found in downloads table? (Yes → use existing CAS, No → download)
3. **BLAKE3 exists?**: Found in models table? (Yes → dedup, No → add to CAS)
4. **SHA256 provided by HF?**: Header present? (Yes → verify, No → trust download)
5. **Symlink privilege?**: Can create symlink? (Yes → symlink, No → junction/copy)
6. **Network failure?**: Download interrupted? (Retry → resume with Range, Abort after 3 failures)

**Performance Characteristics**:

| Scenario | Time | Network | Disk I/O | Notes |
|----------|------|---------|----------|-------|
| Cache hit (SHA256 known) | <100ms | 0 | Metadata query only | Instant return |
| Cache hit (BLAKE3 exists) | ~5s | Full download | Compute hash, delete temp | Dedup detected post-download |
| New model (6.94GB, 100 Mbps) | ~10min | Full download | Write to CAS | First time download |
| New model (6.94GB, 1 Gbps) | ~1min | Full download | Write to CAS | Fast network |
| Resumable download (50% done) | ~5min | 50% download | Append to temp | HTTP Range request |

## Low-Level Design

### CAS Storage Layout Specification

The Content-Addressable Storage (CAS) layer is the foundation of modeld's architecture, providing immutable, deduplicable storage for all model files.

#### Complete Directory Tree

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
│   │   └── models/
│   │       ├── Stable-diffusion/
│   │       ├── Lora/
│   │       └── VAE/
│   └── a1111/
│       └── models/
│           ├── Stable-diffusion/
│           ├── Lora/
│           └── VAE/
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
└── modeld.db  (SQLite metadata)
```

#### Prefix Sharding Strategy

**Design**: First 2 hexadecimal characters (00-ff) create subdirectories

**Rationale**:
- **Even distribution**: BLAKE3 produces uniformly distributed hashes
- **256 shards**: Prevents single-directory bottleneck
- **Proven pattern**: Used by Git, Docker, and other CAS systems
- **Optimal balance**: Between too few (performance) and too many (complexity)

**Scalability Analysis**:

| Total Models | Models per Shard | Performance | Status |
|--------------|------------------|-------------|--------|
| 10,000 | ~39 | Excellent | ✓ |
| 100,000 | ~390 | Excellent | ✓ |
| 1,000,000 | ~3,906 | Good | ✓ |
| 10,000,000 | ~39,062 | Acceptable | ✓ |
| 25,000,000 | ~97,656 | Good (near limit) | ✓ |
| 100,000,000 | ~390,625 | Degraded | ⚠ Sub-sharding needed |

**Filesystem Limits**:
- **NTFS** (Windows): 10K-100K files per directory for good performance
- **ext4** (Linux): ~10M entries hard limit, degrades around 100K
- **APFS** (macOS): Similar to ext4, practical limit ~100K
- **btrfs** (Linux): Excellent scalability, handles millions well

**Conclusion**: With 256 shards, modeld efficiently handles **25 million models** before considering sub-sharding.

#### Hash as Filename

**Format**: Full 64-character BLAKE3 hash used as filename

```
cas/blake3/{prefix}/{full_hash}
         ^^        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
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

**Design Decision**: Pure content addresses, no metadata in filename

**Rejected Alternatives**:
- `hash.size` → Size changes don't change hash, adds complexity
- `hash.safetensors` → Format is metadata, belongs in database
- `hash.timestamp` → Content-addressing means timestamp irrelevant

**Benefits**:
- **Simplicity**: Fewer edge cases, easier debugging
- **Content-addressability**: Same content = same path (guaranteed dedup)
- **Schema flexibility**: Metadata changes don't require filesystem changes
- **Atomic lookups**: Direct path construction from hash (no directory scanning)

#### File Naming Conventions

**CAS Objects** (immutable):
- Format: `{64-char-hex-blake3-hash}`
- Example: `abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890`
- Validation: Exactly 64 characters, [0-9a-f] only
- Permissions: 444 (r--r--r--) on Unix, Read-only on Windows

**Temporary Staging**:
- Format: `{64-char-hash}.tmp`
- Location: `tmp/cas_staging/`
- Purpose: Two-phase commit preparation
- Cleanup: Removed after successful commit or on crash recovery

**Quarantine Files**:
- Format: `{64-char-hash}.{unix-timestamp}`
- Example: `abcdef123...1705320600`
- Metadata: `{64-char-hash}.{unix-timestamp}.meta` (JSON)
- Retention: 30 days default (configurable)

**Download In-Progress**:
- Format: `{partial-hash}.part`
- Lock file: `{partial-hash}.lock`
- Metadata: `{partial-hash}.meta` (JSON with progress)
- Location: `tmp/downloads/`

### Immutability Enforcement

modeld enforces immutability at multiple layers to ensure CAS objects are never modified after creation.

#### Layer 1: Filesystem Permissions

**Unix/Linux/macOS**:
```bash
# After writing to CAS
chmod 444 cas/blake3/ab/abcdef123...
# Results in: -r--r--r-- (read-only for owner, group, others)

# Directory permissions
chmod 555 cas/blake3/ab/
# Results in: dr-xr-xr-x (read + execute, no write)
```

**Effect**:
- Owner cannot modify file
- Owner cannot delete file (directory is read-only)
- Only root/admin can bypass (system-level protection)

**Windows**:
```rust
// Pseudocode for Windows read-only
use std::fs::File;
use std::os::windows::fs::MetadataExt;

fn make_readonly_windows(path: &Path) -> io::Result<()> {
    let file = File::open(path)?;
    let mut permissions = file.metadata()?.permissions();
    permissions.set_readonly(true);
    file.set_permissions(permissions)?;
    Ok(())
}
```

**Windows Challenges**:
- NTFS doesn't have Unix-style directory permissions
- Read-only attribute can be bypassed more easily than Unix permissions
- Rely more on application-level validation

**Alternative (Advanced)**: NTFS ACLs
```powershell
# Set CAS directory ACL to deny write access to all users
icacls "cas\blake3" /deny Everyone:(W,D) /T
```

#### Layer 2: Application-Level Checks

**Pre-Write Validation**:
```rust
fn ensure_immutable(path: &Path) -> Result<(), Error> {
    if path.starts_with("$MODELD_STORE/cas/") {
        return Err(Error::CasImmutabilityViolation(
            "CAS objects are immutable and cannot be modified. \
             If you need to update a model, it will create a new CAS object with a different hash."
        ));
    }
    Ok(())
}

// Check before any write operation
fn write_file(path: &Path, data: &[u8]) -> Result<()> {
    ensure_immutable(path)?;  // Fail if writing to CAS
    std::fs::write(path, data)?;
    Ok(())
}
```

**Benefits**:
- Platform-independent (works on all OSes)
- Clear error messages to users
- Prevents accidental modeld bugs from corrupting CAS
- Works even if filesystem permissions bypassed

#### Layer 3: Hash Verification on Read

**Integrity Checks** (optional, performance trade-off):
```rust
fn verify_cas_integrity(hash: &Blake3Hash, path: &Path) -> Result<bool, Error> {
    let computed_hash = compute_blake3_hash(path)?;
    Ok(computed_hash == *hash)
}
```

**When to Verify**:
- ✓ **On demand**: `modeld verify` command (user-triggered)
- ✓ **During GC**: Before deleting (ensure not corrupted)
- ✓ **After corruption reports**: User-flagged issues
- ✓ **After dedup commit**: Phase B verification
- ✗ **Not on every read**: Too slow for model loading (2-6 seconds per model)

**Verification Modes**:
```bash
# Verify specific model
modeld verify {hash}

# Verify all models (slow, use sparingly)
modeld verify --all

# Verify quarantined models only
modeld verify --quarantine

# Fast check (file exists + size matches)
modeld verify --quick
```

#### Layer 4: Write-Once Semantics

**CAS Path Construction**:
```rust
impl CasStore {
    fn add_file(&self, source: &Path, hash: &Blake3Hash) -> Result<PathBuf> {
        let cas_path = self.get_path(hash);
        
        // Check if already exists
        if cas_path.exists() {
            // Verify hash matches (detect collisions)
            let existing_hash = compute_blake3_hash(&cas_path)?;
            if existing_hash == *hash {
                // Already have this exact content, no-op
                return Ok(cas_path);
            } else {
                // Hash collision (extremely rare, ~2^128 probability)
                return Err(Error::HashCollision {
                    expected: hash.clone(),
                    existing: existing_hash,
                });
            }
        }
        
        // Copy to staging
        let staging = self.tmp_dir.join(format!("{}.tmp", hash));
        std::fs::copy(source, &staging)?;
        
        // Verify hash
        let computed = compute_blake3_hash(&staging)?;
        if computed != *hash {
            std::fs::remove_file(&staging)?;
            return Err(Error::HashMismatch);
        }
        
        // Atomic rename (write-once)
        std::fs::rename(&staging, &cas_path)?;
        
        // Make read-only
        make_readonly(&cas_path)?;
        
        Ok(cas_path)
    }
}
```

**Key Property**: Once a CAS path exists, it will never be overwritten (write-once semantics).

#### Layer 5: Database Consistency

**Foreign Key Constraints**:
```sql
-- aliases table references models table
FOREIGN KEY (model_hash) REFERENCES models(blake3_hash) ON DELETE CASCADE

-- refs table references models table  
FOREIGN KEY (model_hash) REFERENCES models(blake3_hash) ON DELETE CASCADE
```

**Check Constraints**:
```sql
-- Hash must be exactly 64 hex characters
CHECK (length(blake3_hash) = 64)

-- Size must be positive
CHECK (size_bytes > 0)

-- Status must be valid enum value
CHECK (status IN ('pending', 'copied', 'committed', 'failed'))
```

**Benefits**:
- Prevents orphaned references (aliases without parent model)
- Validates data at database level (defense in depth)
- Self-documenting (schema shows valid states)
- Works even if application has bugs

#### Immutability Benefits

**1. Safe Hardlinks**:
- Multiple directory entries can point to same inode
- No risk of one alias modifying content seen by others
- Zero-overhead file sharing

**2. Deduplication Confidence**:
- Same hash = same content (guaranteed)
- Can safely replace file with link
- No need for re-verification after dedup

**3. Concurrent Access**:
- No locks needed for reads (file never changes)
- Multiple frameworks can load same model simultaneously
- SQLite WAL mode allows concurrent readers during writes

**4. Crash Recovery**:
- CAS objects are either complete or don't exist (atomic)
- No partial writes or corrupted states
- Simple rollback: delete incomplete staging files

**5. Reproducibility**:
- Hash uniquely identifies content
- Same model will always have same hash
- Easy to verify integrity across systems

#### Immutability Trade-offs

**Storage Overhead**:
- ✗ Cannot modify models in-place (need to create new CAS object)
- ✗ Old versions not automatically deleted (need explicit GC)
- ✓ Mitigated by: Quarantine mechanism with configurable TTL

**Update Workflow**:
- ✗ User cannot "edit" a model file directly
- ✓ Workflow: Download new version → New hash → Old version GC'd if unused
- ✓ Transparent to AI frameworks (just see new file)

**Complexity**:
- ✗ More complex than traditional filesystem (CAS + metadata index)
- ✓ Mitigated by: Clear abstractions, comprehensive error handling
- ✓ Benefit: Guarantees safety that simple FS cannot provide

**Summary**: Immutability is a core design principle that enables safe deduplication, concurrent access, and crash recovery. The slight complexity cost is justified by the significant safety and performance benefits.

**Directory Structure:**

```
$MODELD_STORE/
├── cas/
│   └── blake3/
│       ├── ab/
│       │   └── abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890
│       ├── cd/
│       │   └── cdef5678...
│       └── ...
│
├── virtual/
│   ├── comfyui/
│   │   ├── checkpoints/
│   │   │   └── v1-5-pruned.safetensors → ../../cas/blake3/ab/abc...
│   │   └── loras/
│   │       └── character.safetensors → ../../cas/blake3/cd/cde...
│   ├── forge/
│   └── a1111/
│
├── hf_cache/
│   └── hub/
│       └── models--org--model/
│           ├── blobs/
│           │   └── {sha256} → ../../../cas/blake3/ab/abc...
│           └── snapshots/
│               └── {revision}/
│                   └── model.safetensors → ../../../blobs/{sha256}
│
├── tmp/
│   ├── downloads/
│   │   └── {hash}.part  (downloading)
│   └── cas_staging/
│       └── {hash}.tmp  (pre-commit)
│
├── quarantine/
│   └── {hash}.{timestamp}  (ref_count=0, pending deletion)
│
├── wal/
│   └── transactions.log  (crash recovery)
│
└── modeld.db  (SQLite metadata)
```

**Design Decisions:**

1. **Prefix Sharding**: First 2 chars of hash → subdirectory
   - Avoids single directory with millions of files
   - Typical limit: 10,000-100,000 files per directory
   - 256 shards = up to 25M models before sub-sharding

2. **Immutability**: All CAS objects are read-only
   - Prevents accidental modification
   - Enables safe hardlink sharing

3. **Quarantine**: Grace period before deletion
   - ref_count drops to 0 → move to quarantine/
   - Default TTL: 30 days
   - Prevents accidental data loss

### Database Schema (SQLite)

```sql
-- Models: Central CAS registry
CREATE TABLE models (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    blake3_hash       TEXT UNIQUE NOT NULL,
    size_bytes        INTEGER NOT NULL,
    format            TEXT,           -- safetensors | gguf | ckpt | bin | pt
    arch              TEXT,           -- sd1 | sdxl | flux | llm | unknown
    base_model        TEXT,           -- sd-v1-5 | sdxl-base | ...
    created_at        TEXT DEFAULT (datetime('now')),
    last_seen         TEXT DEFAULT (datetime('now')),
    quarantined_at    TEXT DEFAULT NULL,
    quarantine_reason TEXT DEFAULT NULL,
    
    CHECK (length(blake3_hash) = 64),
    CHECK (size_bytes > 0)
);

CREATE INDEX idx_models_hash ON models(blake3_hash);
CREATE INDEX idx_models_format ON models(format);
CREATE INDEX idx_models_arch ON models(arch);
CREATE INDEX idx_models_quarantined ON models(quarantined_at) WHERE quarantined_at IS NOT NULL;

-- Aliases: Multiple paths pointing to same content
CREATE TABLE aliases (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    model_hash  TEXT NOT NULL,
    path        TEXT NOT NULL UNIQUE,
    frontend    TEXT,           -- comfyui | forge | a1111 | hf_cache | user
    alias_type  TEXT NOT NULL,  -- hardlink | symlink | junction | copy | original
    created_at  TEXT DEFAULT (datetime('now')),
    
    FOREIGN KEY (model_hash) REFERENCES models(blake3_hash) ON DELETE CASCADE,
    CHECK (alias_type IN ('hardlink', 'symlink', 'junction', 'copy', 'original'))
);

CREATE INDEX idx_aliases_hash ON aliases(model_hash);
CREATE INDEX idx_aliases_frontend ON aliases(frontend);

-- Refs: Workflow/usage references (prevents accidental deletion)
CREATE TABLE refs (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    model_hash   TEXT NOT NULL,
    ref_source   TEXT NOT NULL,  -- workflow file path or source identifier
    ref_type     TEXT,           -- lora | checkpoint | vae | controlnet | embedding
    last_checked TEXT DEFAULT (datetime('now')),
    
    FOREIGN KEY (model_hash) REFERENCES models(blake3_hash) ON DELETE CASCADE,
    UNIQUE (model_hash, ref_source, ref_type)
);

CREATE INDEX idx_refs_hash ON refs(model_hash);
CREATE INDEX idx_refs_source ON refs(ref_source);

-- Downloads: HF download tracking
CREATE TABLE downloads (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    model_hash   TEXT,
    source_url   TEXT NOT NULL,
    sha256_hash  TEXT,           -- HF uses sha256, need mapping to blake3
    status       TEXT NOT NULL,  -- pending | downloading | hashing | done | failed
    bytes_total  INTEGER,
    bytes_done   INTEGER DEFAULT 0,
    started_at   TEXT DEFAULT (datetime('now')),
    finished_at  TEXT,
    error_msg    TEXT,
    
    FOREIGN KEY (model_hash) REFERENCES models(blake3_hash),
    CHECK (status IN ('pending', 'downloading', 'hashing', 'done', 'failed')),
    CHECK (bytes_done >= 0),
    CHECK (bytes_done <= bytes_total OR bytes_total IS NULL)
);

CREATE INDEX idx_downloads_url ON downloads(source_url);
CREATE INDEX idx_downloads_status ON downloads(status);
CREATE INDEX idx_downloads_sha256 ON downloads(sha256_hash);

-- WAL: Transaction log for crash recovery
CREATE TABLE wal_transactions (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    tx_id        TEXT UNIQUE NOT NULL,
    operation    TEXT NOT NULL,  -- dedup | download | gc
    status       TEXT NOT NULL,  -- pending | copied | committed | failed
    source_path  TEXT,
    target_hash  TEXT,
    metadata     TEXT,           -- JSON with operation-specific data
    created_at   TEXT DEFAULT (datetime('now')),
    updated_at   TEXT DEFAULT (datetime('now')),
    
    CHECK (status IN ('pending', 'copied', 'committed', 'failed')),
    CHECK (operation IN ('dedup', 'download', 'gc'))
);

CREATE INDEX idx_wal_status ON wal_transactions(status);
CREATE INDEX idx_wal_created ON wal_transactions(created_at);
CREATE INDEX idx_wal_operation ON wal_transactions(operation);

-- Enable WAL mode for better concurrency
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;
```

**Schema Design Rationale:**

#### 1. Why SQLite?

**Advantages**:
- **Zero configuration**: No server setup, single file database
- **ACID transactions**: Full transactional guarantees
- **Cross-platform**: Works identically on Windows, Linux, macOS
- **Embedded**: No separate daemon process needed
- **Performance**: Fast for read-heavy workloads (up to 100K reads/sec)
- **Reliability**: Proven, battle-tested in billions of deployments
- **Simple backup**: Copy single .db file

**Alternatives Considered**:
- **PostgreSQL**: Rejected - requires server setup, overkill for local storage
- **JSON files**: Rejected - no ACID guarantees, poor query performance
- **Custom binary format**: Rejected - reinventing the wheel, no SQL flexibility

**Decision**: SQLite is optimal for modeld's use case (single-user, local metadata storage).

---

#### 2. Table Design Decisions

**models Table** (Central Registry):
- **blake3_hash as TEXT**: SQLite doesn't have native binary type, TEXT is efficient for hex strings
- **UNIQUE constraint on hash**: Prevents duplicate entries, enables fast lookups
- **format/arch/base_model**: Metadata for UI/filtering, not critical for CAS operation
- **quarantined_at/quarantine_reason**: Support safe garbage collection with grace period
- **Timestamps**: created_at tracks first discovery, last_seen enables stale detection

**aliases Table** (Filesystem Mapping):
- **path UNIQUE**: Each filesystem path can only point to one model
- **alias_type enum**: Tracks link strategy (hardlink vs symlink vs junction) for debugging
- **frontend field**: Groups aliases by AI framework, useful for selective operations
- **CASCADE delete**: When model deleted, all aliases removed automatically

**refs Table** (Explicit References):
- **UNIQUE(model_hash, ref_source, ref_type)**: One model can be referenced by same workflow multiple times with different roles
- **ref_type enum**: Distinguishes lora vs checkpoint vs vae references
- **last_checked**: Enables periodic validation of stale workflow files

**downloads Table** (HuggingFace Tracking):
- **sha256_hash**: Critical for HF deduplication (maps HF SHA256 → modeld BLAKE3)
- **status enum**: Tracks download lifecycle for resumable downloads
- **bytes_done/bytes_total**: Progress tracking for UI
- **error_msg**: Debugging failed downloads
- **Foreign key to models**: Optional (NULL during download, set on completion)

**wal_transactions Table** (Crash Recovery):
- **tx_id UUID**: Globally unique transaction identifier
- **operation enum**: Different recovery logic for dedup vs download vs gc
- **status enum**: State machine (pending → copied → committed)
- **metadata JSON**: Flexible storage for operation-specific context (duplicate group, link strategy, etc.)
- **updated_at**: Tracks status transitions for forensics

---

#### 3. Index Strategy

**Performance Principles**:
- Index frequently queried columns (hash lookups, status filtering)
- Avoid over-indexing (each index slows INSERT/UPDATE)
- Use partial indexes for sparse data (quarantined models)

**Index Justification**:

| Index | Justification | Expected Usage |
|-------|---------------|----------------|
| `idx_models_hash` | O(1) hash lookup instead of O(n) table scan | Every file operation |
| `idx_models_format` | Filter by format (e.g., "show all safetensors") | UI queries |
| `idx_models_arch` | Filter by architecture (e.g., "SDXL models only") | UI queries |
| `idx_models_quarantined` | Efficient expired quarantine queries | Daily GC runs |
| `idx_aliases_hash` | List all paths for a model (ref counting) | Dedup, GC |
| `idx_aliases_frontend` | Frontend-specific operations ("scan ComfyUI") | Selective scans |
| `idx_refs_hash` | Ref count calculation | Every GC decision |
| `idx_refs_source` | Validate workflow file still exists | Periodic cleanup |
| `idx_downloads_url` | Deduplicate repeated download requests | HF downloads |
| `idx_downloads_status` | Query failed/pending downloads | Recovery |
| `idx_downloads_sha256` | Critical: SHA256 → BLAKE3 mapping | Every HF download |
| `idx_wal_status` | Find incomplete transactions on startup | Crash recovery |
| `idx_wal_created` | Cleanup old committed transactions | Periodic WAL pruning |
| `idx_wal_operation` | Group transactions by type | Debugging |

**Partial Index** (`idx_models_quarantined`):
- Only indexes rows WHERE quarantined_at IS NOT NULL
- Saves space (most models not quarantined)
- Faster queries (smaller index)

---

#### 4. Foreign Key Relationships

**Referential Integrity**:
```
models (blake3_hash) ← aliases (model_hash)
models (blake3_hash) ← refs (model_hash)
models (blake3_hash) ← downloads (model_hash)
```

**CASCADE DELETE Strategy**:
- **ON DELETE CASCADE**: When model deleted, all aliases and refs deleted automatically
- **Rationale**: Aliases without parent model are meaningless (orphaned symlinks)
- **Safety**: GC will never delete model with refs (ref_count > 0 check)

**Why NOT cascade for downloads?**
- Downloads table is historical record (audit trail)
- Failed downloads may not have model_hash (download failed before hashing)
- Foreign key is optional (NULL allowed during download)

---

#### 5. Check Constraints (Data Validation)

**Principle**: Prevent invalid data at database level, not just application code.

| Constraint | Purpose | Example Invalid Data Prevented |
|------------|---------|-------------------------------|
| `length(blake3_hash) = 64` | BLAKE3 is always 64 hex chars | Truncated hash: "abcdef123" |
| `size_bytes > 0` | Files cannot be zero bytes | Empty file records |
| `alias_type IN (...)` | Only valid link types | Typo: "hardlnk" |
| `status IN (...)` | State machine invariant | Invalid state: "processing" |
| `bytes_done <= bytes_total` | Progress cannot exceed 100% | Bug: bytes_done=200, bytes_total=100 |
| `operation IN (...)` | Only known operations | Typo: "dedupe" |

**Why at DB level?**
- Multiple clients may access database (CLI, daemon, Python hook)
- Defense in depth (even if application bug, DB rejects invalid data)
- Self-documenting (schema shows valid values)

---

#### 6. WAL Mode Configuration

**PRAGMA journal_mode = WAL;**
- **Default SQLite mode**: DELETE journal (exclusive write lock)
- **WAL mode**: Write-Ahead Logging, concurrent readers during writes
- **Benefit**: Read queries don't block during INSERT/UPDATE
- **Use case**: UI can query metadata while daemon is deduplicating
- **Trade-off**: Slightly larger disk usage (WAL file + database file)

**PRAGMA synchronous = NORMAL;**
- **Default**: FULL (fsync after every write)
- **NORMAL**: fsync at critical points (transaction commit)
- **Benefit**: 2-3x faster writes without sacrificing durability
- **Safety**: Transaction committed → data durable (even if crash)
- **Why not OFF**: Would risk database corruption on power loss

**PRAGMA foreign_keys = ON;**
- **SQLite quirk**: Foreign keys disabled by default for backwards compatibility
- **Explicit enable**: Required for CASCADE DELETE to work
- **Safety**: Prevents orphaned records
- **Performance**: Minimal overhead (indexes already exist)

---

#### 7. Timestamp Usage

**ISO 8601 Format** (TEXT):
```sql
datetime('now')  -- Returns: "2024-01-15 10:30:45"
```

**Why TEXT instead of INTEGER (Unix epoch)?**
- **Human-readable**: Can read timestamps directly in SQLite viewer
- **SQLite date functions**: `datetime()`, `date()`, `time()` work with TEXT
- **ISO 8601 sorting**: Lexicographic sort = chronological sort
- **Timezone handling**: Store UTC, display local (application responsibility)

**Timestamp Fields**:
- **created_at**: Immutable, tracks first discovery
- **last_seen**: Updated on every scan, detects stale models
- **quarantined_at**: NULL = active, non-NULL = quarantined
- **finished_at**: NULL = pending, non-NULL = completed
- **updated_at**: Tracks state transitions in WAL

---

#### 8. JSON Metadata Field

**wal_transactions.metadata** (TEXT):
```json
{
  "duplicate_group": ["path1", "path2", "path3"],
  "canonical_path": "path1",
  "file_size": 6942694400,
  "link_strategy": {"path2": "symlink", "path3": "hardlink"}
}
```

**Why JSON?**
- **Flexibility**: Different operations need different metadata
- **Schema evolution**: Can add new fields without ALTER TABLE
- **SQLite support**: JSON functions (json_extract) for queries
- **Debugging**: Human-readable, self-documenting

**Example Queries**:
```sql
-- Find all dedup transactions for a specific file
SELECT * FROM wal_transactions 
WHERE operation = 'dedup' 
  AND json_extract(metadata, '$.canonical_path') = 'C:\models\sdxl.safetensors';

-- Count pending transactions by operation
SELECT operation, COUNT(*) 
FROM wal_transactions 
WHERE status = 'pending' 
GROUP BY operation;
```

---

#### 9. Performance Targets

**Expected Database Size**:
- 1 million models → ~500 MB database
- 10 million models → ~5 GB database
- SQLite handles databases up to 281 TB (theoretical limit)

**Query Performance Benchmarks** (on NVMe SSD):
- Hash lookup (indexed): <1ms
- Ref count calculation: <5ms (even with 10K refs)
- Full table scan (1M rows): ~100ms
- Transaction commit: <10ms

**Scalability Limits**:
- SQLite concurrent readers: Unlimited
- SQLite concurrent writers: 1 (WAL mode allows readers during write)
- For modeld use case (single user): More than sufficient

---

#### 10. Schema Versioning

**Future Migrations**:
- Schema v1 (Phase 0): Initial design
- Schema v2 (Phase 3): May add chunk-level deduplication tables
- Schema v3 (Phase 6): May add OCI artifact metadata

**Migration Strategy**:
```sql
-- Store schema version
CREATE TABLE schema_version (
    version INTEGER PRIMARY KEY,
    applied_at TEXT DEFAULT (datetime('now'))
);

INSERT INTO schema_version (version) VALUES (1);
```

**Backwards Compatibility**:
- Additive changes only (new columns, new tables)
- Never break existing queries
- Provide migration path (v1 → v2 → v3)

---

**Summary**: The SQLite schema is designed for simplicity, correctness, and performance. Foreign keys enforce integrity, indexes optimize queries, WAL mode enables concurrency, and check constraints prevent invalid data. This foundation supports all modeld operations (scan, dedup, download, GC) with transactional safety and crash recovery.

### BLAKE3 Hash Algorithm

**Hash Computation Strategy:**

```rust
// Pseudocode for hash computation
fn compute_blake3_hash(file_path: &Path) -> Result<Blake3Hash> {
    let file_size = file_path.metadata()?.len();
    
    // Strategy 1: Small files (<10MB) - direct read
    if file_size < 10 * 1024 * 1024 {
        let mut hasher = blake3::Hasher::new();
        let data = std::fs::read(file_path)?;
        hasher.update(&data);
        return Ok(hasher.finalize());
    }
    
    // Strategy 2: Large files (≥10MB) - mmap + parallel chunks
    let file = File::open(file_path)?;
    let mmap = unsafe { MmapOptions::new().map(&file)? };
    
    let mut hasher = blake3::Hasher::new();
    
    // Parallel chunk hashing using Rayon
    const CHUNK_SIZE: usize = 64 * 1024 * 1024;  // 64MB chunks
    let chunks: Vec<_> = mmap.chunks(CHUNK_SIZE).collect();
    
    hasher.update_rayon(&chunks);  // blake3 parallel update
    
    Ok(hasher.finalize())
}

// Caching strategy
struct HashCache {
    path: PathBuf,
    mtime: SystemTime,
    size: u64,
    hash: Blake3Hash,
}

fn get_or_compute_hash(path: &Path, db: &Database) -> Result<Blake3Hash> {
    let metadata = path.metadata()?;
    let mtime = metadata.modified()?;
    let size = metadata.len();
    
    // Check cache
    if let Some(cached) = db.get_hash_cache(path)? {
        if cached.mtime == mtime && cached.size == size {
            return Ok(cached.hash);  // Cache hit!
        }
    }
    
    // Cache miss - compute and store
    let hash = compute_blake3_hash(path)?;
    db.set_hash_cache(path, mtime, size, &hash)?;
    Ok(hash)
}
```

**Design Parameters:**

- **Chunk Size**: 64MB (balances parallelism vs memory)
- **Small File Threshold**: 10MB (avoid mmap overhead)
- **Cache Key**: (path, mtime, size) tuple
- **Performance Target**: ≥2GB/s on NVMe SSD

### Transactional Move Protocol

**Two-Phase Commit for File Deduplication:**

```
┌─────────────────────────────────────────────────┐
│ Phase A: Prepare (Write-ahead)                  │
├─────────────────────────────────────────────────┤
│                                                 │
│ 1. Generate tx_id = uuid()                     │
│ 2. INSERT INTO wal_transactions                 │
│    (tx_id, operation='dedup',                   │
│     status='pending', source_path, target_hash) │
│                                                 │
│ 3. Copy file: source → tmp/cas_staging/{hash}  │
│    - Preserve timestamps if possible            │
│    - Use buffered I/O for large files           │
│                                                 │
│ 4. Verify: blake3(tmp file) == target_hash     │
│    - If mismatch: ROLLBACK, mark tx failed      │
│                                                 │
│ 5. UPDATE wal_transactions                      │
│    SET status='copied' WHERE tx_id=?            │
│                                                 │
│ 6. fsync(wal_transactions)                      │
│    - Ensure WAL is durable before Phase B       │
│                                                 │
└─────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────┐
│ Phase B: Commit (Make changes visible)          │
├─────────────────────────────────────────────────┤
│                                                 │
│ 7. Atomic rename:                               │
│    tmp/cas_staging/{hash}                       │
│    → cas/blake3/{prefix}/{hash}                 │
│    - Same filesystem: rename() is atomic        │
│    - Cross-filesystem: fallback to copy+verify  │
│                                                 │
│ 8. For each duplicate path:                     │
│    a. Determine link strategy:                  │
│       - Same volume → hardlink                  │
│       - Cross volume + permissions → symlink    │
│       - Cross volume + no perms → junction/ref  │
│    b. Create link: path → cas/blake3/{hash}     │
│    c. INSERT INTO aliases (model_hash, path,    │
│                            alias_type, frontend)│
│                                                 │
│ 9. If ref_count(old_file) == 0:                 │
│    - Move to quarantine/{hash}.{timestamp}     │
│    - Schedule deletion (default 30 days)        │
│                                                 │
│ 10. UPDATE wal_transactions                     │
│     SET status='committed' WHERE tx_id=?        │
│                                                 │
│ 11. Optionally: DELETE FROM wal_transactions    │
│     WHERE tx_id=? AND status='committed'        │
│     (or keep for audit trail)                   │
│                                                 │
└─────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────┐
│ Crash Recovery (on daemon startup)              │
├─────────────────────────────────────────────────┤
│                                                 │
│ SELECT * FROM wal_transactions                  │
│ WHERE status IN ('pending', 'copied')           │
│                                                 │
│ For each incomplete transaction:                │
│                                                 │
│ IF status = 'pending':                          │
│   - Check if tmp file exists                    │
│   - Resume from step 3 OR rollback              │
│                                                 │
│ IF status = 'copied':                           │
│   - tmp file should exist and be verified       │
│   - Resume from step 7 (Phase B)                │
│   - If tmp file corrupt: rollback               │
│                                                 │
│ Rollback procedure:                             │
│   - Delete tmp/cas_staging/{hash} if exists     │
│   - UPDATE status='failed', error_msg=...       │
│   - Log error for manual review                 │
│                                                 │
└─────────────────────────────────────────────────┘
```

**Critical Design Invariants:**

1. **Atomicity**: Either all duplicates are deduplicated, or none are
2. **Durability**: WAL fsync ensures crash recovery
3. **Consistency**: Hash verification at every stage
4. **Isolation**: One transaction per file group

### Windows Compatibility Strategy

**Platform-Specific Challenges:**

| Challenge | Details | Impact |
|-----------|---------|--------|
| Symlink Privileges | Requires Developer Mode or Admin | High - affects 90% of users |
| Cross-Volume Hardlink | Not supported by NTFS | High - common multi-disk setups |
| Junction Points | Directory-only, different semantics | Medium - can be workaround |
| File System Differences | NTFS vs ReFS vs exFAT | Low - focus on NTFS |

**Link Strategy Decision Tree:**

```
Start: Need to create link from source → CAS target
  │
  ├─> Check: Same volume?
  │   │
  │   YES ─> Create NTFS Hardlink
  │          └─> Success: DONE ✓
  │          └─> Fail: Fall through to next strategy
  │
  NO (cross-volume)
  │
  ├─> Check: Have symlink privilege?
  │   │   (Test: CreateSymbolicLinkW() → check error)
  │   │
  │   YES ─> Create NTFS Symlink
  │          └─> Success: DONE ✓
  │          └─> Fail: Fall through
  │
  NO (no privilege)
  │
  ├─> Check: Is it a directory?
  │   │
  │   YES ─> Create Junction Point
  │          └─> Success: DONE ✓
  │          └─> Fail: Fall through
  │
  NO (file, not directory)
  │
  └─> Fallback: Reference-Only Mode
      ├─> Keep file at original location
      ├─> Add alias with type='copy'
      ├─> Increment ref count in CAS
      ├─> Show warning: "Could not dedup (no privileges)"
      └─> DONE (no space saved for this file)
```

**Privilege Detection (Windows):**

```rust
// Pseudocode for Windows privilege check
fn has_symlink_privilege() -> bool {
    // Try to create a test symlink in temp directory
    let temp = std::env::temp_dir();
    let test_target = temp.join("modeld_test_target.txt");
    let test_link = temp.join("modeld_test_link.txt");
    
    // Create target file
    std::fs::write(&test_target, "test").ok()?;
    
    // Attempt symlink creation
    let result = std::os::windows::fs::symlink_file(&test_target, &test_link);
    
    // Cleanup
    let _ = std::fs::remove_file(&test_link);
    let _ = std::fs::remove_file(&test_target);
    
    result.is_ok()
}

// Cache result to avoid repeated checks
static HAS_SYMLINK_PRIV: OnceCell<bool> = OnceCell::new();

fn get_link_capability() -> LinkCapability {
    let has_symlink = HAS_SYMLINK_PRIV.get_or_init(|| has_symlink_privilege());
    
    if *has_symlink {
        LinkCapability::Full  // Can do hardlink + symlink
    } else {
        LinkCapability::Limited  // Hardlink only (same volume)
    }
}
```

**Cross-Volume Strategy Matrix:**

| Scenario | User Has Privileges | Solution |
|----------|-------------------|----------|
| Same volume | N/A | Hardlink (always works) |
| Cross volume | Yes (Dev Mode) | Symlink |
| Cross volume | No | Reference-only + Warning |

**User Communication:**

```
# Initial scan
$ modeld scan D:\models E:\more_models

Scanning...
⚠ Warning: No symlink privilege detected
  Cross-volume deduplication will be limited.
  
  To enable full deduplication:
  1. Enable Windows Developer Mode
  2. Or run as Administrator
  
  Learn more: https://modeld.dev/docs/windows-setup

Found 50 duplicates (120 GB potential savings)
├─ Same volume: 30 groups (80 GB) ✓ Can deduplicate
└─ Cross volume: 20 groups (40 GB) ⚠ Limited by privileges
```

## Algorithm Specifications

This section provides comprehensive pseudocode for all critical algorithms in modeld, enabling implementation without ambiguity. Each algorithm includes detailed steps, error handling, and platform-specific considerations.

### 1. BLAKE3 Hash Computation Algorithm

**Purpose**: Compute BLAKE3 hash of files with optimal strategy selection based on file size.

**Strategy Selection**: Small files (<10MB) use direct read; large files (≥10MB) use memory-mapped parallel processing.

#### Algorithm Pseudocode

```rust
use blake3::Hasher;
use memmap2::MmapOptions;
use rayon::prelude::*;
use std::fs::File;
use std::path::Path;

const SMALL_FILE_THRESHOLD: u64 = 10 * 1024 * 1024;  // 10MB
const CHUNK_SIZE: usize = 64 * 1024 * 1024;          // 64MB

/// Main entry point: Compute BLAKE3 hash with optimal strategy
fn compute_blake3_hash(file_path: &Path) -> Result<Blake3Hash, HashError> {
    let metadata = file_path.metadata()
        .map_err(|e| HashError::IoError(e))?;
    let file_size = metadata.len();
    
    // Log hash operation
    log::debug!("Hashing file: {} ({} bytes)", file_path.display(), file_size);
    
    // Strategy selection based on file size
    if file_size < SMALL_FILE_THRESHOLD {
        hash_small_file(file_path, file_size)
    } else {
        hash_large_file(file_path, file_size)
    }
}

/// Strategy 1: Direct read for small files (<10MB)
fn hash_small_file(file_path: &Path, file_size: u64) -> Result<Blake3Hash, HashError> {
    let start = Instant::now();
    
    // Read entire file into memory
    let data = std::fs::read(file_path)
        .map_err(|e| HashError::IoError(e))?;
    
    // Single-threaded hash computation
    let mut hasher = Hasher::new();
    hasher.update(&data);
    let hash = hasher.finalize();
    
    let elapsed = start.elapsed();
    log::debug!("Small file hashed: {} bytes in {:?}", file_size, elapsed);
    
    Ok(Blake3Hash::from(hash))
}

/// Strategy 2: Memory-mapped + parallel for large files (≥10MB)
fn hash_large_file(file_path: &Path, file_size: u64) -> Result<Blake3Hash, HashError> {
    let start = Instant::now();
    
    // Open file and create memory map
    let file = File::open(file_path)
        .map_err(|e| HashError::IoError(e))?;
    
    let mmap = unsafe { 
        MmapOptions::new()
            .map(&file)
            .map_err(|e| HashError::MmapError(e))?
    };
    
    // BLAKE3 supports parallel hashing via Rayon
    let mut hasher = Hasher::new();
    hasher.update_rayon(&mmap);  // Parallel processing of 64MB chunks
    let hash = hasher.finalize();
    
    let elapsed = start.elapsed();
    let throughput = file_size as f64 / elapsed.as_secs_f64();
    log::debug!("Large file hashed: {} bytes in {:?} ({:.2} GB/s)", 
               file_size, elapsed, throughput / 1_000_000_000.0);
    
    Ok(Blake3Hash::from(hash))
}

/// Error types for hash computation
enum HashError {
    IoError(std::io::Error),
    MmapError(std::io::Error),
    FileNotFound(PathBuf),
}
```

#### Hash Caching Algorithm

**Purpose**: Avoid re-hashing unchanged files by caching (path, mtime, size) → hash mappings.

```rust
/// Get hash with caching support
fn get_or_compute_hash(
    path: &Path, 
    db: &Database
) -> Result<Blake3Hash, HashError> {
    let metadata = path.metadata()
        .map_err(|e| HashError::IoError(e))?;
    let mtime = metadata.modified()
        .map_err(|e| HashError::IoError(e))?;
    let size = metadata.len();
    
    // Step 1: Check cache for existing hash
    if let Some(cached) = db.query_hash_cache(path)? {
        if cached.mtime == mtime && cached.size == size {
            // Cache hit - return cached hash
            log::debug!("Cache hit: {}", path.display());
            return Ok(cached.hash);
        } else {
            log::debug!("Cache miss (stale): {} (mtime or size changed)", 
                       path.display());
        }
    } else {
        log::debug!("Cache miss (new file): {}", path.display());
    }
    
    // Step 2: Cache miss - compute hash
    let hash = compute_blake3_hash(path)?;
    
    // Step 3: Update cache
    db.upsert_hash_cache(path, mtime, size, &hash)?;
    
    Ok(hash)
}

/// Cache validation (check if cache entry is still valid)
fn is_cache_valid(path: &Path, cached: &CacheEntry) -> Result<bool, std::io::Error> {
    let metadata = path.metadata()?;
    let current_mtime = metadata.modified()?;
    let current_size = metadata.len();
    
    // Platform-specific handling
    #[cfg(windows)]
    {
        let fs_type = get_filesystem_type(path)?;
        if fs_type == "FAT32" {
            // FAT32: 2-second mtime granularity tolerance
            let mtime_diff = current_mtime.duration_since(cached.mtime)?;
            let mtime_matches = mtime_diff.as_secs() <= 2;
            return Ok(mtime_matches && current_size == cached.size);
        }
    }
    
    // Standard comparison (NTFS, ext4, APFS, etc.)
    Ok(current_mtime == cached.mtime && current_size == cached.size)
}
```

### 2. Hash Caching Algorithm

**Purpose**: Efficiently cache hash computations to avoid redundant work on unchanged files.

```rust
/// Cache entry structure
struct CacheEntry {
    path: PathBuf,
    mtime: SystemTime,
    size: u64,
    hash: Blake3Hash,
    cached_at: DateTime<Utc>,
}

/// Upsert hash cache entry
fn upsert_hash_cache(
    db: &Database,
    path: &Path,
    mtime: SystemTime,
    size: u64,
    hash: &Blake3Hash
) -> Result<(), DbError> {
    let path_str = path.to_string_lossy();
    let mtime_secs = mtime.duration_since(UNIX_EPOCH)?.as_secs() as i64;
    
    db.execute(
        "INSERT OR REPLACE INTO hash_cache 
         (path, mtime, size, hash, cached_at) 
         VALUES (?, ?, ?, ?, datetime('now'))",
        params![path_str, mtime_secs, size as i64, hash.to_string()]
    )?;
    
    // Evict old entries if cache too large
    evict_cache_if_needed(db)?;
    
    Ok(())
}

/// LRU cache eviction
fn evict_cache_if_needed(db: &Database) -> Result<(), DbError> {
    const MAX_CACHE_ENTRIES: usize = 100_000;
    
    let count: usize = db.query_row(
        "SELECT COUNT(*) FROM hash_cache",
        [],
        |row| row.get(0)
    )?;
    
    if count > MAX_CACHE_ENTRIES {
        let to_remove = count - MAX_CACHE_ENTRIES;
        log::info!("Evicting {} old cache entries", to_remove);
        
        db.execute(
            "DELETE FROM hash_cache 
             WHERE rowid IN (
                 SELECT rowid FROM hash_cache 
                 ORDER BY cached_at ASC 
                 LIMIT ?
             )",
            params![to_remove]
        )?;
    }
    
    Ok(())
}
```

### 3. Transactional Move Protocol (Two-Phase Commit)

**Purpose**: Atomically move files to CAS with crash recovery guarantees.

#### Phase A: Prepare (Write-Ahead)

```rust
/// Phase A: Copy file to staging and verify
fn dedup_phase_a(
    canonical_path: &Path,
    target_hash: &Blake3Hash,
    db: &Database,
    config: &Config
) -> Result<TransactionId, DedupError> {
    // Step 1: Generate unique transaction ID
    let tx_id = Uuid::new_v4();
    log::info!("Starting dedup transaction: {}", tx_id);
    
    // Step 2: Write WAL record (status='pending')
    db.execute(
        "INSERT INTO wal_transactions 
         (tx_id, operation, status, source_path, target_hash, metadata, created_at)
         VALUES (?, 'dedup', 'pending', ?, ?, ?, datetime('now'))",
        params![
            tx_id.to_string(),
            canonical_path.to_string_lossy(),
            target_hash.to_string(),
            json!({"file_size": canonical_path.metadata()?.len()}).to_string()
        ]
    )?;
    
    // Step 3: Copy canonical file to staging
    let staging_path = config.store_path
        .join("tmp/cas_staging")
        .join(format!("{}.tmp", target_hash));
    
    std::fs::create_dir_all(staging_path.parent().unwrap())?;
    
    log::debug!("Copying: {} -> {}", 
               canonical_path.display(), staging_path.display());
    
    std::fs::copy(canonical_path, &staging_path)?;
    
    // Step 4: Verify hash of staged file
    let computed_hash = compute_blake3_hash(&staging_path)?;
    
    if &computed_hash != target_hash {
        // Hash mismatch - rollback
        log::error!("Hash mismatch! Expected: {}, Got: {}", 
                   target_hash, computed_hash);
        std::fs::remove_file(&staging_path)?;
        
        db.execute(
            "UPDATE wal_transactions 
             SET status='failed', 
                 metadata=json_set(metadata, '$.error', 'hash_mismatch'),
                 updated_at=datetime('now')
             WHERE tx_id=?",
            params![tx_id.to_string()]
        )?;
        
        return Err(DedupError::HashMismatch);
    }
    
    // Step 5: Update WAL (status='copied')
    db.execute(
        "UPDATE wal_transactions 
         SET status='copied', 
             metadata=json_set(metadata, '$.phase_a_completed_at', datetime('now')),
             updated_at=datetime('now')
         WHERE tx_id=?",
        params![tx_id.to_string()]
    )?;
    
    // Step 6: fsync WAL to ensure durability
    db.execute("PRAGMA wal_checkpoint(FULL)", [])?;
    
    log::info!("Phase A completed: {}", tx_id);
    Ok(tx_id)
}
```

#### Phase B: Commit (Make Changes Visible)

```rust
/// Phase B: Atomic rename and create links
fn dedup_phase_b(
    tx_id: TransactionId,
    duplicate_paths: &[PathBuf],
    target_hash: &Blake3Hash,
    db: &Database,
    config: &Config,
    capability: &LinkCapability
) -> Result<(), DedupError> {
    log::info!("Starting Phase B: {}", tx_id);
    
    // Update WAL metadata
    db.execute(
        "UPDATE wal_transactions 
         SET metadata=json_set(metadata, '$.phase_b_started_at', datetime('now'))
         WHERE tx_id=?",
        params![tx_id.to_string()]
    )?;
    
    // Step 7: Atomic rename to CAS
    let staging_path = config.store_path
        .join("tmp/cas_staging")
        .join(format!("{}.tmp", target_hash));
    
    let cas_path = get_cas_path(target_hash, &config.store_path);
    std::fs::create_dir_all(cas_path.parent().unwrap())?;
    
    log::debug!("Atomic rename: {} -> {}", 
               staging_path.display(), cas_path.display());
    
    // Atomic rename (same filesystem) or copy+verify (cross-filesystem)
    if is_same_volume(&staging_path, &cas_path) {
        std::fs::rename(&staging_path, &cas_path)?;
    } else {
        // Cross-filesystem fallback
        std::fs::copy(&staging_path, &cas_path)?;
        let verify_hash = compute_blake3_hash(&cas_path)?;
        if &verify_hash != target_hash {
            std::fs::remove_file(&cas_path)?;
            return Err(DedupError::HashMismatch);
        }
        std::fs::remove_file(&staging_path)?;
    }
    
    // Make CAS file read-only (immutability)
    set_readonly(&cas_path, true)?;
    
    // Step 8: Create links for each duplicate
    for dup_path in duplicate_paths {
        let link_result = create_link(dup_path, &cas_path, capability)?;
        
        // Record in aliases table
        db.execute(
            "INSERT INTO aliases 
             (model_hash, path, frontend, alias_type, created_at)
             VALUES (?, ?, 'user', ?, datetime('now'))",
            params![
                target_hash.to_string(),
                dup_path.to_string_lossy(),
                format!("{:?}", link_result.link_type())
            ]
        )?;
        
        log::info!("Created link: {} -> CAS ({:?})", 
                  dup_path.display(), link_result.link_type());
    }
    
    // Step 9: Quarantine original files if not referenced
    // (handled by separate GC process)
    
    // Step 10: Mark transaction committed
    db.execute(
        "UPDATE wal_transactions 
         SET status='committed', updated_at=datetime('now')
         WHERE tx_id=?",
        params![tx_id.to_string()]
    )?;
    
    log::info!("Phase B completed: {}", tx_id);
    Ok(())
}
```

### 4. Crash Recovery Algorithm

**Purpose**: Recover incomplete transactions on daemon startup or manual recovery.

```rust
/// Run on startup or manual recovery command
fn recover_wal_transactions(
    db: &Database, 
    config: &Config
) -> Result<RecoveryReport, RecoveryError> {
    let mut report = RecoveryReport::default();
    
    log::info!("Starting WAL recovery...");
    
    // Query all incomplete transactions
    let incomplete = db.prepare(
        "SELECT tx_id, operation, status, source_path, target_hash, metadata, created_at
         FROM wal_transactions 
         WHERE status IN ('pending', 'copied')
         ORDER BY created_at ASC"
    )?;
    
    let transactions: Vec<WalTransaction> = incomplete
        .query_map([], |row| {
            Ok(WalTransaction {
                tx_id: row.get(0)?,
                operation: row.get(1)?,
                status: row.get(2)?,
                source_path: row.get::<_, String>(3)?.into(),
                target_hash: row.get(4)?,
                metadata: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    
    log::info!("Found {} incomplete transactions", transactions.len());
    
    // Recover each transaction
    for tx in transactions {
        log::info!("Recovering transaction: {} (status: {})", 
                  tx.tx_id, tx.status);
        
        match tx.status.as_str() {
            "pending" => recover_pending_transaction(&tx, db, config, &mut report)?,
            "copied" => recover_copied_transaction(&tx, db, config, &mut report)?,
            _ => unreachable!("Invalid status in query"),
        }
    }
    
    log::info!("WAL recovery complete: resumed={}, rolled_back={}, data_loss={}", 
              report.resumed_transactions, 
              report.rolled_back_transactions,
              report.data_loss_detected);
    
    Ok(report)
}

/// Recover transaction in 'pending' state (Phase A incomplete)
fn recover_pending_transaction(
    tx: &WalTransaction,
    db: &Database,
    config: &Config,
    report: &mut RecoveryReport
) -> Result<(), RecoveryError> {
    let staging_path = config.store_path
        .join("tmp/cas_staging")
        .join(format!("{}.tmp", tx.target_hash));
    
    if staging_path.exists() {
        // Staging file exists - verify and resume
        log::info!("Found staging file, verifying hash");
        
        let computed_hash = compute_blake3_hash(&staging_path)?;
        
        if computed_hash.to_string() == tx.target_hash {
            // Hash matches - mark as copied and resume Phase B
            log::info!("Staging file verified, resuming Phase B");
            
            db.execute(
                "UPDATE wal_transactions 
                 SET status='copied', updated_at=datetime('now')
                 WHERE tx_id=?",
                params![&tx.tx_id]
            )?;
            
            // Resume Phase B (would need duplicate_paths from metadata)
            report.resumed_transactions += 1;
        } else {
            // Hash mismatch - corrupted, rollback
            log::error!("Staging file corrupted (hash mismatch), rolling back");
            std::fs::remove_file(&staging_path)?;
            rollback_transaction(&tx.tx_id, db)?;
            report.rolled_back_transactions += 1;
        }
    } else {
        // No staging file - check if CAS already has it
        let cas_path = get_cas_path(&tx.target_hash, &config.store_path);
        
        if cas_path.exists() {
            // CAS file exists - verify and mark committed
            let computed_hash = compute_blake3_hash(&cas_path)?;
            
            if computed_hash.to_string() == tx.target_hash {
                log::info!("CAS file exists and verified, marking committed");
                db.execute(
                    "UPDATE wal_transactions 
                     SET status='committed', updated_at=datetime('now')
                     WHERE tx_id=?",
                    params![&tx.tx_id]
                )?;
                report.fixed_wal_inconsistency += 1;
            } else {
                log::error!("CAS file corrupted, rolling back");
                rollback_transaction(&tx.tx_id, db)?;
                report.rolled_back_transactions += 1;
            }
        } else {
            // Nothing exists - safe to rollback
            log::info!("No staging or CAS file found, rolling back cleanly");
            rollback_transaction(&tx.tx_id, db)?;
            report.rolled_back_transactions += 1;
        }
    }
    
    Ok(())
}

/// Recover transaction in 'copied' state (Phase B incomplete)
fn recover_copied_transaction(
    tx: &WalTransaction,
    db: &Database,
    config: &Config,
    report: &mut RecoveryReport
) -> Result<(), RecoveryError> {
    let staging_path = config.store_path
        .join("tmp/cas_staging")
        .join(format!("{}.tmp", tx.target_hash));
    let cas_path = get_cas_path(&tx.target_hash, &config.store_path);
    
    if cas_path.exists() {
        // CAS file already exists - Phase B might be complete
        log::info!("CAS file exists, verifying and completing Phase B");
        
        let computed_hash = compute_blake3_hash(&cas_path)?;
        
        if computed_hash.to_string() == tx.target_hash {
            // Hash valid - mark committed (links may need recreation)
            db.execute(
                "UPDATE wal_transactions 
                 SET status='committed', updated_at=datetime('now')
                 WHERE tx_id=?",
                params![&tx.tx_id]
            )?;
            
            // Clean up staging file if exists
            if staging_path.exists() {
                std::fs::remove_file(&staging_path)?;
            }
            
            report.resumed_transactions += 1;
        } else {
            // CAS file corrupted - rollback
            log::error!("CAS file corrupted, rolling back");
            std::fs::remove_file(&cas_path)?;
            rollback_transaction(&tx.tx_id, db)?;
            report.rolled_back_transactions += 1;
        }
    } else if staging_path.exists() {
        // Staging exists but CAS doesn't - retry rename
        log::info!("Staging file exists, retrying atomic rename to CAS");
        
        let computed_hash = compute_blake3_hash(&staging_path)?;
        
        if computed_hash.to_string() == tx.target_hash {
            // Rename to CAS
            std::fs::create_dir_all(cas_path.parent().unwrap())?;
            std::fs::rename(&staging_path, &cas_path)?;
            
            // Mark committed
            db.execute(
                "UPDATE wal_transactions 
                 SET status='committed', updated_at=datetime('now')
                 WHERE tx_id=?",
                params![&tx.tx_id]
            )?;
            
            report.resumed_transactions += 1;
        } else {
            log::error!("Staging file corrupted during recovery, rolling back");
            std::fs::remove_file(&staging_path)?;
            rollback_transaction(&tx.tx_id, db)?;
            report.rolled_back_transactions += 1;
        }
    } else {
        // Neither exists - data loss
        log::error!("Both staging and CAS files missing, data loss occurred");
        rollback_transaction(&tx.tx_id, db)?;
        report.data_loss_detected += 1;
    }
    
    Ok(())
}

/// Mark transaction as failed
fn rollback_transaction(tx_id: &str, db: &Database) -> Result<(), DbError> {
    db.execute(
        "UPDATE wal_transactions 
         SET status='failed', 
             metadata=json_set(metadata, '$.error', 'rolled_back_on_recovery'),
             updated_at=datetime('now')
         WHERE tx_id=?",
        params![tx_id]
    )?;
    Ok(())
}

struct RecoveryReport {
    resumed_transactions: usize,
    rolled_back_transactions: usize,
    fixed_wal_inconsistency: usize,
    data_loss_detected: usize,
}
```

### 5. Reference Counting Algorithm

**Purpose**: Calculate total reference count for a model to determine GC protection level.

```rust
/// Reference count structure
struct RefCount {
    explicit: usize,   // refs table entries (workflows, user favorites)
    implicit: usize,   // aliases table entries (filesystem links)
    total: usize,      // sum of both
}

/// Calculate total reference count for a model
fn get_ref_count(
    model_hash: &str, 
    db: &Database
) -> Result<RefCount, DbError> {
    // Optimized query: count both types in single round-trip
    let result = db.query_row(
        "SELECT 
            (SELECT COUNT(*) FROM refs WHERE model_hash = ?) as explicit_refs,
            (SELECT COUNT(*) FROM aliases WHERE model_hash = ?) as implicit_refs",
        params![model_hash, model_hash],
        |row| {
            Ok((
                row.get::<_, usize>(0)?,
                row.get::<_, usize>(1)?
            ))
        }
    )?;
    
    let (explicit, implicit) = result;
    let total = explicit + implicit;
    
    log::debug!("Ref count for {}: explicit={}, implicit={}, total={}", 
               &model_hash[..16], explicit, implicit, total);
    
    Ok(RefCount {
        explicit,
        implicit,
        total,
    })
}

/// Determine protection level based on reference count
enum ProtectionLevel {
    Protected,    // ref_count > 0, cannot GC
    Quarantine,   // ref_count = 0, eligible for GC
    Deleted,      // past TTL, permanent deletion
}

fn determine_protection_level(
    model_hash: &str, 
    db: &Database,
    config: &GcConfig
) -> Result<ProtectionLevel, DbError> {
    let ref_count = get_ref_count(model_hash, db)?;
    
    // Level 1: PROTECTED (has references)
    if ref_count.total > 0 {
        return Ok(ProtectionLevel::Protected);
    }
    
    // Level 2/3: Check quarantine status
    let quarantine_info = db.query_row_optional(
        "SELECT quarantined_at FROM models WHERE blake3_hash = ?",
        params![model_hash],
        |row| row.get::<_, String>(0)
    )?;
    
    if let Some(quarantine_date_str) = quarantine_info {
        let quarantine_date = DateTime::parse_from_rfc3339(&quarantine_date_str)?;
        let age = Utc::now() - quarantine_date;
        
        if age < config.quarantine_ttl {
            return Ok(ProtectionLevel::Quarantine);
        } else {
            return Ok(ProtectionLevel::Deleted);
        }
    }
    
    // Not yet quarantined, but ref_count = 0 → eligible for quarantine
    Ok(ProtectionLevel::Quarantine)
}

/// Validate and clean up stale references
fn validate_references(db: &Database) -> Result<ValidationReport, DbError> {
    let mut report = ValidationReport::default();
    
    // Validate aliases (implicit references)
    let aliases = db.prepare(
        "SELECT id, model_hash, path FROM aliases"
    )?.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
    })?.collect::<Result<Vec<_>, _>>()?;
    
    for (id, hash, path) in aliases {
        if !Path::new(&path).exists() {
            // Stale alias - path no longer exists
            db.execute("DELETE FROM aliases WHERE id = ?", params![id])?;
            report.stale_aliases_removed += 1;
            log::info!("Removed stale alias: {} (file not found)", path);
        }
    }
    
    // Validate refs (explicit references)
    let refs = db.prepare(
        "SELECT id, model_hash, ref_source FROM refs"
    )?.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
    })?.collect::<Result<Vec<_>, _>>()?;
    
    for (id, hash, ref_source) in refs {
        if !Path::new(&ref_source).exists() {
            // Orphaned ref - source file deleted
            db.execute("DELETE FROM refs WHERE id = ?", params![id])?;
            report.orphaned_refs_removed += 1;
            log::info!("Removed orphaned ref: {} (source not found)", ref_source);
        }
    }
    
    log::info!("Reference validation complete: stale_aliases={}, orphaned_refs={}", 
              report.stale_aliases_removed, report.orphaned_refs_removed);
    
    Ok(report)
}

struct ValidationReport {
    stale_aliases_removed: usize,
    orphaned_refs_removed: usize,
}
```

### 6. Canonical Path Selection Algorithm

**Purpose**: Deterministically select the "best" file to keep when multiple identical files exist.

**Priority Order**:
1. Already in CAS (immutable, verified)
2. Oldest mtime (likely the original)
3. Shortest path (simpler to reference)
4. Alphabetically first (deterministic tiebreaker)

```rust
/// Select canonical path from duplicate group
fn select_canonical(duplicates: &[PathBuf]) -> Result<PathBuf, SelectionError> {
    if duplicates.is_empty() {
        return Err(SelectionError::EmptyGroup);
    }
    
    if duplicates.len() == 1 {
        return Ok(duplicates[0].clone());
    }
    
    log::debug!("Selecting canonical from {} duplicates", duplicates.len());
    
    // Collect metadata for each path
    let mut candidates: Vec<CandidateInfo> = duplicates
        .iter()
        .filter_map(|path| {
            let metadata = path.metadata().ok()?;
            let mtime = metadata.modified().ok()?;
            let in_cas = path.to_string_lossy().contains("/cas/blake3/") || 
                        path.to_string_lossy().contains("\\cas\\blake3\\");
            let path_len = path.as_os_str().len();
            let path_str = path.to_string_lossy().to_string();
            
            Some(CandidateInfo {
                path: path.clone(),
                in_cas,
                mtime,
                path_len,
                path_str,
            })
        })
        .collect();
    
    if candidates.is_empty() {
        return Err(SelectionError::NoValidCandidates);
    }
    
    // Sort by priority:
    // 1. in_cas (true first, so !in_cas for ascending)
    // 2. mtime (oldest first)
    // 3. path_len (shortest first)
    // 4. path_str (alphabetically first)
    candidates.sort_by_key(|c| {
        (!c.in_cas, c.mtime, c.path_len, c.path_str.clone())
    });
    
    let canonical = &candidates[0];
    
    log::info!("Selected canonical: {} (in_cas={}, mtime={:?}, len={})", 
              canonical.path.display(), 
              canonical.in_cas,
              canonical.mtime,
              canonical.path_len);
    
    Ok(canonical.path.clone())
}

struct CandidateInfo {
    path: PathBuf,
    in_cas: bool,
    mtime: SystemTime,
    path_len: usize,
    path_str: String,
}

enum SelectionError {
    EmptyGroup,
    NoValidCandidates,
}
```

**Selection Examples**:

```rust
// Example 1: Multiple user copies
let duplicates = vec![
    PathBuf::from("C:\\Users\\Alice\\Downloads\\sdxl-base-1.0.safetensors"),      // mtime: 2024-01-10
    PathBuf::from("C:\\ComfyUI\\models\\checkpoints\\sdxl-base-1.0.safetensors"),  // mtime: 2024-01-12
    PathBuf::from("D:\\Forge\\models\\Stable-diffusion\\sdxl-base-1.0.safetensors"), // mtime: 2024-01-15
];
// Selected: C:\\Users\\Alice\\Downloads\\... (oldest mtime)

// Example 2: One already in CAS
let duplicates = vec![
    PathBuf::from("C:\\modeld\\cas\\blake3\\ab\\abcdef123..."),  // in CAS
    PathBuf::from("C:\\ComfyUI\\models\\checkpoints\\sdxl.safetensors"),  // mtime: 2024-01-01 (older!)
];
// Selected: C:\\modeld\\cas\\blake3\\ab\\abcdef123... (CAS takes priority)

// Example 3: Same mtime, different lengths
let duplicates = vec![
    PathBuf::from("D:\\AI\\models\\checkpoints\\backup\\old\\archive\\sdxl-base-final-v3.safetensors"),  // len: 75
    PathBuf::from("C:\\models\\sdxl.safetensors"),  // len: 25
];
// Selected: C:\\models\\sdxl.safetensors (shorter path)

// Example 4: Full tie, alphabetical
let duplicates = vec![
    PathBuf::from("C:\\models\\sdxl-copy.safetensors"),
    PathBuf::from("C:\\models\\sdxl-base.safetensors"),  // alphabetically first
];
// Selected: C:\\models\\sdxl-base.safetensors (alphabetical)
```

### 7. Link Creation Algorithm (Windows-Specific)

**Purpose**: Create filesystem links with multi-tier fallback strategy for Windows compatibility.

```rust
/// Link types
enum LinkType {
    Hardlink,       // Same volume, zero overhead
    Symlink,        // Cross volume, requires privilege
    Junction,       // Directory-only, no privilege
    ReferenceOnly,  // Database tracking only, no physical link
}

/// Link capability detection (cached at startup)
struct LinkCapability {
    has_symlink_privilege: bool,
    filesystem_type: String,
}

/// Main entry point: Create link with fallback strategy
fn create_link(
    source: &Path,     // Where link should be created
    target: &Path,     // CAS object to link to
    capability: &LinkCapability
) -> Result<LinkResult, LinkError> {
    log::debug!("Creating link: {} -> {}", source.display(), target.display());
    
    // Step 1: Check filesystem type
    let fs_type = get_filesystem_type(source)
        .unwrap_or_else(|_| "UNKNOWN".to_string());
    
    if fs_type == "FAT32" || fs_type == "exFAT" {
        // No linking support on FAT32/exFAT
        log::warn!("Filesystem {} does not support links: {}", 
                  fs_type, source.display());
        return create_reference_only(source, target);
    }
    
    // Step 2: Check if same volume
    if is_same_volume(source, target) {
        // Same volume - try hardlink
        log::debug!("Same volume detected, attempting hardlink");
        return create_hardlink(source, target);
    }
    
    // Step 3: Cross-volume - check privilege
    if capability.has_symlink_privilege {
        // Have privilege - try symlink
        log::debug!("Cross-volume with privilege, attempting symlink");
        return create_symlink(source, target);
    }
    
    // Step 4: No privilege - reference-only mode
    log::warn!("Cross-volume without privilege: {} (no space savings)", 
              source.display());
    return create_reference_only(source, target);
}

/// Create hardlink (same volume)
fn create_hardlink(source: &Path, target: &Path) -> Result<LinkResult, LinkError> {
    // Ensure parent directory exists
    if let Some(parent) = source.parent() {
        std::fs::create_dir_all(parent)?;
    }
    
    // Create hardlink
    match std::fs::hard_link(target, source) {
        Ok(_) => {
            log::info!("Created hardlink: {} -> {}", 
                      source.display(), target.display());
            Ok(LinkResult::Success(LinkType::Hardlink))
        },
        Err(e) => {
            log::error!("Hardlink failed: {} (error: {})", source.display(), e);
            Err(LinkError::HardlinkFailed(e))
        }
    }
}

/// Create symlink (cross volume, requires privilege)
#[cfg(windows)]
fn create_symlink(source: &Path, target: &Path) -> Result<LinkResult, LinkError> {
    use std::os::windows::fs::symlink_file;
    
    // Ensure parent directory exists
    if let Some(parent) = source.parent() {
        std::fs::create_dir_all(parent)?;
    }
    
    // Create symlink
    match symlink_file(target, source) {
        Ok(_) => {
            log::info!("Created symlink: {} -> {}", 
                      source.display(), target.display());
            Ok(LinkResult::Success(LinkType::Symlink))
        },
        Err(e) => {
            log::error!("Symlink failed: {} (error: {})", source.display(), e);
            
            // Check error code
            if let Some(1314) = e.raw_os_error() {
                // ERROR_PRIVILEGE_NOT_HELD
                log::warn!("Privilege check was incorrect, falling back to reference-only");
            }
            
            Err(LinkError::SymlinkFailed(e))
        }
    }
}

/// Reference-only mode (no physical link)
fn create_reference_only(source: &Path, target: &Path) -> Result<LinkResult, LinkError> {
    // File remains at original location
    // Only record in aliases table with type='reference_only'
    log::info!("Reference-only mode: file stays at {}", source.display());
    
    Ok(LinkResult::Success(LinkType::ReferenceOnly))
}

/// Detect symlink privilege (test at startup)
fn detect_symlink_privilege() -> bool {
    let temp_dir = std::env::temp_dir();
    let test_target = temp_dir.join("modeld_test_target.txt");
    let test_link = temp_dir.join("modeld_test_link.txt");
    
    // Create target
    if std::fs::write(&test_target, "test").is_err() {
        return false;
    }
    
    // Try symlink
    #[cfg(windows)]
    let result = std::os::windows::fs::symlink_file(&test_target, &test_link);
    
    #[cfg(not(windows))]
    let result = std::os::unix::fs::symlink(&test_target, &test_link);
    
    // Cleanup
    let _ = std::fs::remove_file(&test_link);
    let _ = std::fs::remove_file(&test_target);
    
    result.is_ok()
}

/// Check if two paths are on the same volume
#[cfg(windows)]
fn is_same_volume(path1: &Path, path2: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use winapi::um::fileapi::GetVolumePathNameW;
    
    let get_volume = |path: &Path| -> Option<String> {
        let wide_path: Vec<u16> = path.as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let mut volume_buf = vec![0u16; 260];
        
        unsafe {
            let result = GetVolumePathNameW(
                wide_path.as_ptr(),
                volume_buf.as_mut_ptr(),
                volume_buf.len() as u32
            );
            
            if result != 0 {
                let len = volume_buf.iter().position(|&c| c == 0).unwrap_or(0);
                let volume_str = String::from_utf16_lossy(&volume_buf[..len]);
                return Some(volume_str);
            }
        }
        
        None
    };
    
    match (get_volume(path1), get_volume(path2)) {
        (Some(vol1), Some(vol2)) => vol1 == vol2,
        _ => false,  // Assume different volumes on error (conservative)
    }
}

/// Get filesystem type
#[cfg(windows)]
fn get_filesystem_type(path: &Path) -> std::io::Result<String> {
    use std::os::windows::ffi::OsStrExt;
    use winapi::um::fileapi::GetVolumeInformationW;
    
    let volume_path = get_volume_path(path)?;
    let wide_volume: Vec<u16> = volume_path.as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    
    let mut fs_name_buf = vec![0u16; 32];
    
    unsafe {
        let result = GetVolumeInformationW(
            wide_volume.as_ptr(),
            std::ptr::null_mut(), 0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            fs_name_buf.as_mut_ptr(),
            fs_name_buf.len() as u32
        );
        
        if result != 0 {
            let len = fs_name_buf.iter().position(|&c| c == 0).unwrap_or(0);
            return Ok(String::from_utf16_lossy(&fs_name_buf[..len]));
        }
    }
    
    Err(std::io::Error::last_os_error())
}

enum LinkResult {
    Success(LinkType),
    Failed(String),
}

enum LinkError {
    HardlinkFailed(std::io::Error),
    SymlinkFailed(std::io::Error),
    IoError(std::io::Error),
}
```

---

## RFC Documents

### RFC 0001: Storage Layout

**Status**: Draft

**Problem Statement:**


Define a CAS storage layout that:
- Scales to millions of models
- Supports future chunk-level deduplication
- Remains compatible with OCI artifact standards
- Works across Windows/Linux/macOS

**Proposed Design:**

```
$MODELD_STORE/
├── cas/
│   └── blake3/
│       ├── {prefix}/        # 2-char hex prefix (256 buckets)
│       │   └── {full_hash}  # Full 64-char blake3 hash
│       └── ...
├── virtual/{frontend}/      # Hardlink/symlink facades
├── tmp/                     # Staging area
├── quarantine/              # Soft-deleted objects
├── wal/                     # Transaction logs
└── modeld.db                # Metadata SQLite
```

**Rationale:**

1. **Prefix Sharding**: Avoids OS limits on entries per directory
   - NTFS: ~10M files per dir before slowdown
   - ext4: ~10M files (directory size limit)
   - 256 shards = 2.5M models per shard = 640M total capacity

2. **Content Addressing**: Full hash as filename
   - No metadata in filename (keeps it simple)
   - All metadata in SQLite (flexible schema evolution)

3. **Immutability**: Read-only CAS objects
   - chmod 444 (Unix) / FILE_ATTRIBUTE_READONLY (Windows)
   - Enables safe concurrent access

4. **Future OCI Compatibility**:
   - Reserve `cas/sha256/` for OCI blobs (future)
   - Keep `cas/blake3/` for modeld-native objects
   - Mapping table: sha256 ↔ blake3 (when both exist)

**Chunk Boundary Reservation:**

For future Phase 6 (chunk-level dedup):
- Each model can be split into chunks
- Chunk hash stored in separate table
- CAS layout unchanged (chunks stored same way as full files)

**Alternatives Considered:**

1. **Single flat directory** → Rejected: doesn't scale
2. **Date-based sharding** → Rejected: uneven distribution
3. **Full path hierarchy** → Rejected: complex migrations

**Decision**: Adopt prefix-sharded layout as specified.

---

### RFC 0002: Hash Strategy

**Status**: Draft

**Problem Statement:**

Choose optimal hashing parameters for:
- Performance: Hash 1TB in <15 minutes
- Incremental hashing: Avoid re-hashing unchanged files
- Cache efficiency: Minimal storage overhead

**Proposed Design:**

**Hash Function**: BLAKE3
- Faster than SHA256 (3-5x on modern CPUs)
- Parallelizable (utilizes multi-core)
- Cryptographically secure (collision-resistant)
- Output: 256-bit (64 hex chars)

**Chunk Size**: 64MB
- Balance: parallelism vs memory usage
- Memory overhead: ~64MB per worker thread
- Rayon thread pool: min(num_cpus, 8) threads

**Small File Threshold**: 10MB
- Files <10MB: direct read (avoid mmap overhead)
- Files ≥10MB: mmap + parallel chunks

**Cache Strategy**:

```sql
-- Cache table (optional, can use models table)
CREATE TABLE hash_cache (
    path      TEXT PRIMARY KEY,
    mtime     INTEGER NOT NULL,  -- Unix timestamp
    size      INTEGER NOT NULL,
    hash      TEXT NOT NULL,
    cached_at TEXT DEFAULT (datetime('now'))
);

-- Eviction policy: LRU, max 100K entries
-- Storage: ~10MB for 100K entries
```

Cache Key: `(path, mtime, size)`
- If any changes → cache miss, recompute
- Catches file modifications, renames, replacements

**Incremental Scan**:

```rust
fn scan_directory(path: &Path, db: &Database) -> Result<()> {
    for entry in WalkDir::new(path) {
        let metadata = entry.metadata()?;
        let current_mtime = metadata.modified()?;
        let current_size = metadata.len();
        
        if let Some(cached) = db.get_hash_cache(&entry.path())? {
            if cached.mtime == current_mtime && cached.size == current_size {
                // Cache hit - skip hashing
                continue;
            }
        }
        
        // Cache miss - compute hash
        let hash = compute_blake3_hash(&entry.path())?;
        db.upsert_model(&hash, &metadata)?;
        db.update_hash_cache(&entry.path(), current_mtime, current_size, &hash)?;
    }
}
```

**Performance Targets**:

| Scenario | Target | Rationale |
|----------|--------|-----------|
| 1TB first scan (NVMe) | ≤15 min | 2GB/s sustained throughput |
| 1TB incremental (no changes) | ≤30 sec | Metadata-only checks |
| Single 12GB model | ≤6 sec | 2GB/s hash rate |

**Alternatives Considered**:

1. **SHA256** → Rejected: 3-5x slower
2. **xxHash** → Rejected: not cryptographically secure
3. **BLAKE2** → Considered: but BLAKE3 is newer and faster

**Decision**: Adopt BLAKE3 with 64MB chunks and mtime/size caching.

---

### RFC 0003: Reference Model

**Status**: Draft

**Problem Statement:**

Prevent accidental deletion of models that are:
- Used by ComfyUI workflows
- Referenced by other AI frontends
- Downloaded but not yet integrated

**Proposed Design:**

**Reference Types**:

1. **Explicit References** (`refs` table)
   - ComfyUI workflow → models
   - Forge scripts → models
   - User tags/favorites

2. **Implicit References** (`aliases` table)
   - Any hardlink/symlink pointing to CAS object
   - File exists in virtual/ directories

**Reference Counting**:

```rust
fn get_ref_count(model_hash: &str, db: &Database) -> Result<usize> {
    let explicit_refs = db.execute(
        "SELECT COUNT(*) FROM refs WHERE model_hash = ?",
        [model_hash]
    )?;
    
    let implicit_refs = db.execute(
        "SELECT COUNT(*) FROM aliases WHERE model_hash = ?",
        [model_hash]
    )?;
    
    Ok(explicit_refs + implicit_refs)
}
```

**GC Protection Levels**:

```
Level 1: PROTECTED (ref_count > 0)
  ├─> Explicit ref exists → Cannot GC
  └─> Alias exists → Warn before GC

Level 2: QUARANTINE (ref_count = 0, age < TTL)
  └─> Moved to quarantine/ directory
      └─> Recoverable via: modeld restore <hash>

Level 3: DELETED (age ≥ TTL)
  └─> Permanently deleted from quarantine/
```

**GC Trigger Conditions**:

1. **Manual**: `modeld gc` command
2. **Automatic**: When disk usage > 90%
3. **Scheduled**: Cron job (optional)

**Safe GC Algorithm**:

```
modeld gc [--safe | --aggressive]

--safe (default):
  1. SELECT models WHERE ref_count = 0
  2. For each model:
     - Check aliases: any still valid?
     - Check refs: any orphaned workflows?
     - If truly unused:
       → Move to quarantine/
       → Set quarantine_date = now()

--aggressive (requires --force):
  1. Skip quarantine
  2. Delete immediately
  3. Require confirmation for each model
```

**Quarantine Mechanism**:

```
quarantine/
├── {hash}.{unix_timestamp}  # Original CAS object
└── {hash}.{unix_timestamp}.meta  # JSON metadata

Metadata JSON:
{
  "hash": "abcdef...",
  "size_bytes": 12000000000,
  "quarantine_date": "2024-01-15T10:30:00Z",
  "deletion_date": "2024-02-14T10:30:00Z",  # +30 days
  "reason": "zero_refs",
  "last_aliases": [
    "/path/to/comfyui/models/checkpoint.safetensors"
  ]
}
```

**Recovery**:

```bash
# List quarantined models
modeld quarantine list

# Restore a model
modeld restore <hash>
  → Moves back from quarantine/ to cas/
  → Optionally recreate aliases
```

**Alternatives Considered**:

1. **No quarantine** → Rejected: too risky
2. **Recycle bin integration** → Rejected: platform-specific
3. **Infinite retention** → Rejected: waste space

**Decision**: Adopt ref counting + 30-day quarantine.

---

### RFC 0004: Deduplication Strategy

**Status**: Draft

**Problem Statement:**

Safely deduplicate identical files across:
- Multiple AI frontends (ComfyUI, Forge, A1111)
- Same frontend (user duplicated models)
- Crash scenarios (power loss during dedup)

**Proposed Design:**

**Canonical Path Selection**:

Priority order:
1. Already in CAS → use it
2. Oldest mtime → likely the original
3. Shortest path → simpler to reference
4. First alphabetically → deterministic tiebreaker

```rust
fn select_canonical(duplicates: &[PathBuf]) -> PathBuf {
    duplicates.iter()
        .min_by_key(|p| {
            let in_cas = p.starts_with("$MODELD_STORE/cas");
            let mtime = p.metadata().modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let path_len = p.as_os_str().len();
            let path_str = p.to_string_lossy();
            
            // Tuple ordering: (in_cas first, then oldest, then shortest, then alphabetical)
            (!in_cas, mtime, path_len, path_str)
        })
        .unwrap()
        .clone()
}
```

**Transactional Dedup**:

See "Transactional Move Protocol" section above for full details.

Key invariants:
- Hash verified at every stage
- WAL records all state transitions
- Atomic operations where possible (rename vs copy)
- Crash recovery on next startup

**Dedup Modes**:

```bash
# Interactive: confirm each group
modeld dedup

# Dry-run: show what would happen
modeld dedup --dry-run

# Automatic: no confirmations
modeld dedup --auto

# Report only: no changes
modeld dedup --report
```

**Progress Reporting**:

```
Deduplicating...
[████████████████████░░░░] 80% (40/50 groups)

Group 15/50: v1-5-pruned.safetensors (×3, 12.4 GB)
  Canonical: D:/ComfyUI/models/checkpoints/v1-5-pruned.safetensors
  Duplicates:
    ├─ D:/Forge/models/Stable-diffusion/v1-5-pruned.safetensors
    └─ D:/A1111/models/Stable-diffusion/v1-5-pruned.safetensors
  
  ✓ Moved to CAS: cas/blake3/ab/abc123...
  ✓ Created hardlink: D:/ComfyUI/models/checkpoints/v1-5-pruned.safetensors
  ✓ Created hardlink: D:/Forge/models/Stable-diffusion/v1-5-pruned.safetensors
  ✓ Created hardlink: D:/A1111/models/Stable-diffusion/v1-5-pruned.safetensors
  
  Saved: 24.8 GB
```

**Error Handling**:


| Error | Recovery Strategy |
|-------|------------------|
| Hash mismatch | Abort transaction, log error, continue to next group |
| Insufficient space | Abort all, show required space, suggest cleanup |
| Permission denied | Skip file, show warning, offer suggestions |
| Crash during copy | Resume from WAL on restart |
| Crash after commit | Safe (changes already visible) |

**Alternatives Considered**:

1. **In-place dedup** → Rejected: risky, no recovery
2. **Copy-on-write (CoW)** → Rejected: filesystem-specific
3. **Three-phase commit** → Rejected: overkill, performance cost

**Decision**: Adopt two-phase commit with WAL recovery.

---

### RFC 0005: Windows Compatibility

**Status**: Draft (CRITICAL)

**Problem Statement:**

Windows has unique filesystem limitations:
- Symlinks require Developer Mode or Admin privileges
- Hardlinks cannot cross volume boundaries
- Different semantics for junctions vs symlinks
- Varying filesystem support (NTFS, ReFS, exFAT)

**Proposed Design:**

See "Windows Compatibility Strategy" section above for full details.

**Key Design Decisions**:

1. **Privilege Detection**: Test at runtime, cache result
2. **Graceful Degradation**: Multiple fallback strategies
3. **User Communication**: Clear warnings and setup instructions
4. **Reference-Only Mode**: Last resort for unprivileged scenarios

**Setup Recommendations**:

**Option A: Enable Developer Mode** (Recommended)

```
Settings → Update & Security → For developers → Developer Mode: ON
```
- Enables symlink creation without Admin
- One-time setup per machine
- No security risks for personal machines

**Option B: Run as Administrator**
- Works but requires UAC prompt
- Not recommended for regular use

**Option C: Limited Mode**
- Use modeld without full dedup
- Still get duplicate detection reports
- Still get same-volume deduplication

**Junction Point Limitations**:

Windows junctions are directory-only:
```
✓ Can create: D:\virtual\comfyui\ → C:\modeld\cas\
✗ Cannot create: D:\model.safetensors → C:\cas\abc123...
```

For virtual directories (Phase 2), junctions CAN work:
```
Virtual structure:
D:\ComfyUI\models\checkpoints\  (junction point)
  → C:\modeld\virtual\comfyui\checkpoints\
    → Contains hardlinks to CAS objects
```

**Cross-Volume Strategy**:

```rust
fn create_link(source: &Path, target: &Path) -> Result<LinkType> {
    let source_vol = get_volume(source)?;
    let target_vol = get_volume(target)?;
    
    if source_vol == target_vol {
        // Same volume - hardlink always works
        std::fs::hard_link(target, source)?;
        return Ok(LinkType::Hardlink);
    }
    
    // Cross volume - try symlink
    if has_symlink_privilege() {
        match std::os::windows::fs::symlink_file(target, source) {
            Ok(_) => return Ok(LinkType::Symlink),
            Err(e) => log::warn!("Symlink failed: {}", e),
        }
    }
    
    // No privilege - reference-only mode
    log::warn!("Cannot create cross-volume link (no privilege)");
    return Ok(LinkType::ReferenceOnly);
}
```

**Testing Matrix**:

| Scenario | Expected Behavior |
|----------|------------------|
| Same volume, NTFS | Hardlink |
| Cross volume, Dev Mode ON | Symlink |
| Cross volume, Dev Mode OFF | Reference-only + Warning |
| ReFS (same volume) | Hardlink (supported) |
| exFAT | Reference-only (no hardlinks) |

**Documentation Requirements**:

- Installation guide with Windows-specific steps
- Troubleshooting section for privilege issues
- Comparison table: Full vs Limited mode
- Screenshots of Developer Mode setup

**Alternatives Considered**:

1. **Require Admin always** → Rejected: bad UX
2. **Windows-only release** → Rejected: want cross-platform
3. **Ignore Windows completely** → Rejected: large user base

**Decision**: Adopt multi-tier strategy with clear communication.

---

### RFC 0006: Virtual FS

**Status**: Draft

**Problem Statement**:

AI frontends expect models at specific paths:
- ComfyUI: `ComfyUI/models/checkpoints/`
- Forge: `webui/models/Stable-diffusion/`
- A1111: `stable-diffusion-webui/models/Stable-diffusion/`

How to make CAS objects appear at these paths?

**Proposed Design:**

**Virtual Directory Structure**:

```
$MODELD_STORE/virtual/
├── comfyui/
│   ├── checkpoints/
│   │   └── v1-5-pruned.safetensors → ../../cas/blake3/ab/abc...
│   ├── loras/
│   │   └── character.safetensors → ../../cas/blake3/cd/cde...
│   ├── vae/
│   └── embeddings/
│
├── forge/
│   ├── Stable-diffusion/
│   └── Lora/
│
└── a1111/
    ├── Stable-diffusion/
    └── Lora/
```

**Mounting Strategy**:

```bash
# User registers a frontend
modeld link comfyui --model-dir D:/ComfyUI/models

# modeld creates:
1. Virtual structure in $MODELD_STORE/virtual/comfyui/
2. For each model in DB:
   - Determine category (checkpoint, lora, vae, etc.)
   - Create link: virtual/comfyui/{category}/{name} → cas/blake3/{hash}
3. Create junction/symlink:
   D:/ComfyUI/models/ → $MODELD_STORE/virtual/comfyui/
```

**Category Detection**:

```rust
fn detect_model_category(metadata: &ModelMetadata) -> Category {
    // From safetensors metadata
    if let Some(arch) = &metadata.arch {
        match arch.as_str() {
            "stable-diffusion-v1" | "stable-diffusion-v2" | "sdxl" => Category::Checkpoint,
            "vae" => Category::VAE,
            _ => Category::Unknown,
        }
    }
    
    // From file size heuristics
    if metadata.size_bytes < 500_000_000 {  // <500MB
        Category::LoRA  // Likely a LoRA
    } else if metadata.size_bytes > 2_000_000_000 {  // >2GB
        Category::Checkpoint  // Likely full model
    } else {
        Category::Unknown
    }
}
```

**Link Refresh**:

```bash
# After scanning new models
modeld link refresh comfyui

# Scans virtual/ directory
# Adds missing links for new models
# Removes broken links for deleted models
```

**Frontend Templates**:

```toml
# Built-in templates in modeld config
[frontends.comfyui]
categories = [
  "checkpoints",
  "loras",
  "vae",
  "embeddings",
  "controlnet",
  "upscale_models"
]

[frontends.forge]
categories = [
  "Stable-diffusion",
  "Lora",
  "VAE",
  "embeddings"
]

# User can define custom
[frontends.my_custom]
base_path = "/custom/models"
categories = ["main", "extra"]
```

**Alternatives Considered**:

1. **FUSE filesystem** → Rejected: requires kernel module
2. **In-place replacement** → Rejected: risky for user data
3. **Config file injection** → Rejected: frontend-specific

**Decision**: Adopt virtual/ directory with link mirroring.

---

### RFC 0007: HF Interception

**Status**: Draft

**Problem Statement:**

Intercept HuggingFace downloads from:
- `diffusers.pipeline.from_pretrained()`
- `transformers.AutoModel.from_pretrained()`
- `huggingface_hub.hf_hub_download()`
- ComfyUI's built-in downloader

Without modifying user code or breaking updates.

**Proposed Design:**

**Two-Layer Approach**:

**Layer 1: Environment Variable** (Primary)

```bash
export HF_HOME="$MODELD_STORE/hf_cache"
```

- HuggingFace libraries check `HF_HOME` first
- modeld maintains a fake cache directory structure
- Transparent to all frameworks

**Fake Cache Layout**:

```
$MODELD_STORE/hf_cache/hub/
└── models--{org}--{model}/
    ├── .no_exist/
    │   └── {revision}  (placeholder)
    ├── blobs/
    │   └── {sha256}    → symlink to cas/blake3/{blake3_hash}
    ├── refs/
    │   └── main        → points to revision
    └── snapshots/
        └── {revision}/
            ├── model.safetensors  → ../../blobs/{sha256}
            ├── config.json        → ../../blobs/{sha256}
            └── ...
```

**SHA256 ↔ BLAKE3 Mapping**:

```sql
-- Extend downloads table
ALTER TABLE downloads ADD COLUMN sha256_hash TEXT;
CREATE INDEX idx_downloads_sha256 ON downloads(sha256_hash);

-- When HF provides sha256 in headers/metadata:
1. Check: SELECT model_hash FROM downloads WHERE sha256_hash = ?
2. If exists → return path to CAS object (cache hit!)
3. If not → download, compute both hashes, store mapping
```

**Layer 2: Python Hook** (Fallback)

For frameworks that don't respect `HF_HOME`:

```python
# modeld_hook/__init__.py
import sys
from pathlib import Path

def activate():
    """Monkey-patch HuggingFace functions"""
    try:
        import huggingface_hub
        original_download = huggingface_hub.hf_hub_download
        
        def modeld_download(*args, **kwargs):
            # Intercept download request
            url = construct_url(*args, **kwargs)
            
            # Check modeld cache
            if cached_path := check_modeld_cache(url):
                return cached_path
            
            # Download via modeld
            return download_via_modeld(url, *args, **kwargs)
        
        huggingface_hub.hf_hub_download = modeld_download
    except ImportError:
        pass  # HF not installed, skip

# Auto-activate on import
activate()
```

**Installation**:

```bash
pip install modeld-hook

# Option A: Explicit import in scripts
import modeld_hook  # activates automatically

# Option B: sitecustomize.py (system-wide)
echo "import modeld_hook" > $(python -c "import site; print(site.getsitepackages()[0])")/sitecustomize.py

# Option C: Environment variable
export PYTHONSTARTUP=/path/to/modeld_hook_init.py
```

**Download Deduplication Flow**:

```
User: model = pipeline.from_pretrained("runwayml/stable-diffusion-v1-5")
  │
  ├─> HF library checks: HF_HOME/hub/models--runwayml--stable-diffusion-v1-5/
  │   └─> snapshots/{revision}/model.safetensors exists?
  │
  NO ─> HF library calls: hf_hub_download(...)
  │
  ├─> modeld intercepts (if hook installed)
  │
  ├─> Check downloads table: URL exists?
  │   └─> Yes: return symlink to CAS object ✓
  │
  NO ─> Download to tmp/downloads/{uuid}.part
  │   ├─> HTTP Range support (resumable)
  │   ├─> Progress callback
  │   └─> On completion:
  │       ├─> Compute BLAKE3 hash
  │       ├─> Extract SHA256 from HF headers (if available)
  │       ├─> Move to CAS: cas/blake3/{prefix}/{hash}
  │       ├─> INSERT INTO models/downloads tables
  │       └─> Create fake HF cache structure
  │
  └─> Return path to HF library
      └─> HF library loads model transparently
```

**HF Metadata Extraction**:

HF provides metadata in `.huggingface.json` files:

```json
{
  "sha256": "abc123...",
  "size": 4265380512,
  "url": "https://huggingface.co/...",
  "etag": "\"def456...\""
}
```

modeld parses this to:
- Map sha256 → blake3
- Track download source
- Verify integrity

**Compatibility Testing**:

Must verify with:
- diffusers (latest)
- transformers (latest)
- huggingface_hub (0.20+)
- ComfyUI (latest)
- Forge (latest)

**Alternatives Considered**:

1. **HTTP proxy** → Rejected: complex, breaks HTTPS
2. **LD_PRELOAD hijacking** → Rejected: platform-specific, fragile
3. **Patch HF source** → Rejected: breaks on updates

**Decision**: Adopt HF_HOME + optional monkey-patch.

---

## Technical Stack Details

### Rust Crates

This section documents all Rust dependencies with justification for each choice, feature flags explanation, and version constraints.

#### Core Dependencies

**Async Runtime:**

```toml
tokio = { version = "1.35", features = ["full"] }
```

- **Justification**: De facto standard async runtime in Rust ecosystem
- **Features**: `full` includes all tokio features (fs, io-util, process, sync, time, rt-multi-thread)
- **Version Constraint**: `1.35` minimum for stable async file operations
- **Why not alternatives**: 
  - async-std: Smaller ecosystem, less active development
  - smol: Minimal but lacks ecosystem maturity
- **Usage**: File I/O operations, daemon IPC server, concurrent downloads

**Database:**

```toml
rusqlite = { version = "0.30", features = ["bundled", "column_decltype", "backup"] }
```

- **Justification**: Zero-configuration embedded database, no server required
- **Features**:
  - `bundled`: Statically links SQLite (no system dependency)
  - `column_decltype`: Get column types for metadata validation
  - `backup`: Enable online backup API for database exports
- **Version Constraint**: `0.30` minimum for SQLite 3.44+ (improved WAL performance)
- **Why not alternatives**:
  - sled: Less mature, no SQL (complex queries harder)
  - redb: Newer, smaller ecosystem
  - PostgreSQL/MySQL: Requires server setup (violates zero-config goal)
- **Usage**: Models registry, aliases tracking, refs counting, WAL transactions

**Hashing:**

```toml
blake3 = { version = "1.5", features = ["rayon", "mmap"] }
```

- **Justification**: Fastest cryptographic hash (3-5x faster than SHA256)
- **Features**:
  - `rayon`: Parallel hashing for large files (utilizes multi-core)
  - `mmap`: Memory-mapped file support for efficient large file hashing
- **Version Constraint**: `1.5` minimum for stable parallel API
- **Why not alternatives**:
  - sha2: 3-5x slower, no parallel hashing
  - xxHash: Not cryptographically secure (collision attacks possible)
  - BLAKE2: Older, slower than BLAKE3
- **Performance**: Target ≥2GB/s on NVMe SSD with modern CPU
- **Usage**: Content addressing, deduplication detection, integrity verification

**File System Operations:**

```toml
walkdir = "2.4"
notify = { version = "6.1", features = ["serde"] }
memmap2 = "0.9"
```

**walkdir**:
- **Justification**: Efficient recursive directory traversal
- **Version Constraint**: `2.4` minimum for stable API
- **Why not alternatives**: std::fs::read_dir (no recursion, manual stack management)
- **Usage**: Initial scan, model discovery

**notify**:
- **Justification**: Cross-platform file system watcher for daemon mode
- **Features**: `serde` for event serialization (IPC communication)
- **Version Constraint**: `6.1` minimum for stable event API
- **Platform Support**: Uses inotify (Linux), FSEvents (macOS), ReadDirectoryChangesW (Windows)
- **Usage**: Daemon file watching, automatic dedup triggers

**memmap2**:
- **Justification**: Memory-mapped file I/O for large file hashing
- **Version Constraint**: `0.9` minimum for stable API
- **Why not alternatives**: std::fs::read (loads entire file into memory)
- **Memory Efficiency**: Maps file into virtual memory (OS handles paging)
- **Usage**: BLAKE3 hashing of files ≥10MB

**Command-Line Interface:**

```toml
clap = { version = "4.4", features = ["derive", "env", "wrap_help"] }
```

- **Justification**: Most mature CLI framework with excellent UX
- **Features**:
  - `derive`: Proc macro API (less boilerplate)
  - `env`: Environment variable support (HF_HOME, MODELD_STORE)
  - `wrap_help`: Auto-wrap help text for terminal width
- **Version Constraint**: `4.4` minimum for stable derive API v4
- **Why not alternatives**:
  - structopt: Deprecated, merged into clap
  - argh: Minimal but lacks validation features
- **Usage**: `modeld scan`, `modeld dedup`, `modeld gc` commands

**Serialization:**

```toml
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
toml = "0.8"
```

**serde**:
- **Justification**: Standard serialization framework (universal ecosystem support)
- **Features**: `derive` for automatic trait implementation
- **Version Constraint**: `1.0` (stable API, no breaking changes)
- **Usage**: Configuration files, metadata export, IPC messages

**serde_json**:
- **Justification**: JSON format for workflow parsing, metadata export
- **Version Constraint**: `1.0` (stable API)
- **Usage**: ComfyUI workflow.json parsing, quarantine metadata

**toml**:
- **Justification**: Human-friendly config format
- **Version Constraint**: `0.8` minimum for TOML 1.0 spec compliance
- **Usage**: `modeld.toml` configuration file

**Error Handling:**

```toml
anyhow = "1.0"
thiserror = "1.0"
```

**anyhow**:
- **Justification**: Flexible error handling for application code
- **Version Constraint**: `1.0` (stable API)
- **Usage**: CLI commands, main application logic

**thiserror**:
- **Justification**: Derive macro for custom error types in libraries
- **Version Constraint**: `1.0` (stable API)
- **Usage**: `modeld-core` library error types

**Strategy**: 
- Libraries use `thiserror` (explicit error types)
- Applications use `anyhow` (dynamic error chains)

**Parallelism:**

```toml
rayon = "1.8"
```

- **Justification**: Data parallelism for CPU-bound operations
- **Version Constraint**: `1.8` minimum for stable scoped threads API
- **Why not alternatives**:
  - tokio tasks: Designed for async I/O, not CPU parallelism
  - std::thread: Manual thread management (complex)
- **Work Stealing**: Automatic load balancing across CPU cores
- **Usage**: Parallel BLAKE3 hashing, concurrent duplicate analysis

**Logging:**

```toml
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
```

**tracing**:
- **Justification**: Structured logging with async support
- **Version Constraint**: `0.1` (stable API)
- **Why not alternatives**:
  - log: Older, no structured events
  - env_logger: Less flexible

**tracing-subscriber**:
- **Features**:
  - `env-filter`: `RUST_LOG=debug` environment variable support
  - `json`: Structured JSON logs (easier parsing)
- **Usage**: Daemon logging, debug traces, performance profiling

**Utilities:**

```toml
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1.6", features = ["v4", "serde"] }
indicatif = "0.17"
```

**chrono**:
- **Justification**: Date/time handling for timestamps
- **Features**: `serde` for timestamp serialization
- **Version Constraint**: `0.4` (stable API)
- **Usage**: Model created_at, last_seen, quarantine_date

**uuid**:
- **Justification**: Transaction IDs for WAL recovery
- **Features**: `v4` (random UUIDs), `serde` (serialization)
- **Version Constraint**: `1.6` minimum for stable v7 UUID support (future)
- **Usage**: WAL transaction IDs

**indicatif**:
- **Justification**: Progress bars for scan/dedup operations
- **Version Constraint**: `0.17` minimum for multi-progress support
- **Usage**: Scan progress, hash progress, dedup progress

#### Platform-Specific Dependencies

**Windows:**

```toml
[target.'cfg(windows)'.dependencies]
windows = { version = "0.52", features = [
    "Win32_Storage_FileSystem",
    "Win32_Foundation",
    "Win32_Security"
] }
```

- **Justification**: Windows API bindings for privilege detection and link creation
- **Features**:
  - `Win32_Storage_FileSystem`: CreateHardLink, CreateSymbolicLink, junction points
  - `Win32_Foundation`: Basic types (HANDLE, BOOL, error codes)
  - `Win32_Security`: Privilege checks (SE_CREATE_SYMBOLIC_LINK_NAME)
- **Version Constraint**: `0.52` minimum for stable API
- **Why not alternatives**:
  - winapi: Deprecated, unmaintained
  - windows-sys: Lower-level, less ergonomic
- **Usage**: Link strategy decision tree, symlink privilege detection

**Unix (Linux/macOS):**

```toml
[target.'cfg(unix)'.dependencies]
nix = { version = "0.27", features = ["fs"] }
libc = "0.2"
```

**nix**:
- **Justification**: Safe Unix system call wrappers
- **Features**: `fs` for filesystem operations (chmod, fadvise)
- **Version Constraint**: `0.27` minimum for stable API
- **Usage**: Set file read-only (chmod 444), posix_fadvise (sequential read hints)

**libc**:
- **Justification**: Direct libc bindings for low-level operations
- **Version Constraint**: `0.2` (stable API)
- **Usage**: Fallback for operations not in nix

#### Development Dependencies

```toml
[dev-dependencies]
criterion = { version = "0.5", features = ["html_reports"] }
tempfile = "3.8"
proptest = "1.4"
rstest = "0.18"
```

**criterion**:
- **Justification**: Statistical benchmarking framework
- **Features**: `html_reports` for visualization
- **Usage**: Hash performance benchmarks, database query benchmarks

**tempfile**:
- **Justification**: Temporary directories for integration tests
- **Usage**: Test CAS storage, test database, test file operations

**proptest**:
- **Justification**: Property-based testing (generative testing)
- **Usage**: Test hash algorithm properties, test dedup invariants

**rstest**:
- **Justification**: Fixture-based testing (less boilerplate)
- **Usage**: Parametrized tests, test fixtures

### Python Package Specification

Complete `pyproject.toml` for `modeld-hook` package:

```toml
[build-system]
requires = ["setuptools>=68.0", "wheel"]
build-backend = "setuptools.build_meta"

[project]
name = "modeld-hook"
version = "0.1.0"
description = "HuggingFace download interception hook for modeld CAS"
readme = "README.md"
requires-python = ">=3.8"
license = { text = "MIT" }
authors = [
    { name = "modeld contributors" }
]
keywords = ["ai", "models", "storage", "deduplication", "huggingface"]
classifiers = [
    "Development Status :: 3 - Alpha",
    "Intended Audience :: Developers",
    "License :: OSI Approved :: MIT License",
    "Programming Language :: Python :: 3",
    "Programming Language :: Python :: 3.8",
    "Programming Language :: Python :: 3.9",
    "Programming Language :: Python :: 3.10",
    "Programming Language :: Python :: 3.11",
    "Programming Language :: Python :: 3.12",
    "Topic :: Scientific/Engineering :: Artificial Intelligence",
]

dependencies = [
    # HuggingFace Hub integration
    "huggingface-hub>=0.20.0,<1.0",
    
    # HTTP client for downloads
    "requests>=2.31.0,<3.0",
    
    # JSON processing
    "python-json-logger>=2.0.0,<3.0",
]

[project.optional-dependencies]
dev = [
    # Testing
    "pytest>=7.4.0",
    "pytest-cov>=4.1.0",
    "pytest-mock>=3.12.0",
    
    # Code quality
    "black>=23.0.0",
    "ruff>=0.1.0",
    "mypy>=1.7.0",
    
    # Type stubs
    "types-requests>=2.31.0",
]

[project.urls]
Homepage = "https://github.com/modeld/modeld"
Documentation = "https://modeld.dev/docs"
Repository = "https://github.com/modeld/modeld"
Issues = "https://github.com/modeld/modeld/issues"

[project.scripts]
modeld-hook = "modeld_hook.cli:main"

[tool.setuptools]
packages = ["modeld_hook"]

[tool.setuptools.package-data]
modeld_hook = ["py.typed"]

[tool.black]
line-length = 100
target-version = ["py38", "py39", "py310", "py311", "py312"]

[tool.ruff]
line-length = 100
target-version = "py38"

[tool.ruff.lint]
select = ["E", "F", "W", "I", "N", "UP", "ANN", "B", "A", "C4", "DTZ", "T10", "RET", "SIM"]
ignore = ["ANN101", "ANN102"]

[tool.mypy]
python_version = "3.8"
strict = true
warn_return_any = true
warn_unused_configs = true
disallow_untyped_defs = true

[tool.pytest.ini_options]
minversion = "7.0"
testpaths = ["tests"]
addopts = "-ra -q --strict-markers --cov=modeld_hook"
```

**Dependency Justification:**

**huggingface-hub** (`>=0.20.0,<1.0`):
- **Purpose**: Official HF library for model downloads
- **Version Constraint**: 
  - Minimum `0.20.0`: Introduced stable cache layout API
  - Maximum `<1.0`: Pin to v0 to avoid breaking changes
- **Why this version**: Stable cache_dir API for interception

**requests** (`>=2.31.0,<3.0`):
- **Purpose**: HTTP client for direct downloads (bypass HF SDK)
- **Version Constraint**:
  - Minimum `2.31.0`: Security fixes (CVE-2023-32681)
  - Maximum `<3.0`: Major version pin
- **Why needed**: Custom download logic with progress hooks

**python-json-logger** (`>=2.0.0,<3.0`):
- **Purpose**: Structured JSON logging for daemon integration
- **Version Constraint**: Pin to v2 (stable API)
- **Why needed**: Log events sent to modeld daemon via IPC

**Development Dependencies:**

- **pytest**: Standard testing framework
- **pytest-cov**: Code coverage reporting
- **pytest-mock**: Mock HF downloads in tests
- **black**: Code formatter (PEP 8 compliance)
- **ruff**: Fast linter (replaces flake8, isort, pyupgrade)
- **mypy**: Static type checking
- **types-requests**: Type stubs for requests library

**Python Version Constraint**: `>=3.8`
- **Rationale**: 
  - Python 3.8: Introduced in Oct 2019 (widely available)
  - Type hints support (PEP 585)
  - Assignment expressions (walrus operator)
  - f-string = specifier
- **Compatibility**: Works with ComfyUI (Python 3.8+), Forge (3.10+), A1111 (3.10+)

### Major Technology Choices Summary

| Component | Technology | Alternatives Considered | Decision Rationale |
|-----------|-----------|------------------------|-------------------|
| **Core Language** | Rust | C++, Go | Performance, memory safety, no GC pauses |
| **Database** | SQLite | PostgreSQL, sled, redb | Zero-config, embedded, mature, SQL support |
| **Hash Function** | BLAKE3 | SHA256, xxHash, BLAKE2 | 3-5x faster, parallel, cryptographically secure |
| **Async Runtime** | Tokio | async-std, smol | Ecosystem maturity, feature completeness |
| **Python Hook** | Monkeypatch + HF_HOME | Custom PyPI mirror | Simplicity, no network infrastructure |
| **CLI Framework** | Clap v4 | structopt, argh | Derive API, validation, help generation |
| **Serialization** | Serde | Manual parsing | Standard, zero-copy, derives |
| **Parallelism** | Rayon | Tokio tasks, threads | Data parallelism, work stealing |
| **Logging** | Tracing | log, env_logger | Structured, async-aware |

### Version Constraint Strategy

**Stability Tiers:**

1. **Locked Major Versions** (breaking changes expected):
   - `clap = "4.x"` (v4 derive API)
   - `rusqlite = "0.30"` (tracks SQLite versions)
   - `blake3 = "1.x"` (stable hash output)

2. **Pinned Minor Versions** (minimum required):
   - `tokio = "1.35"` (need async fs fixes)
   - `rayon = "1.8"` (need scoped threads)
   - `windows = "0.52"` (stable API)

3. **Flexible Ranges** (low breakage risk):
   - `serde = "1.0"` (semantic versioning guarantee)
   - `anyhow = "1.0"` (stable API)
   - `chrono = "0.4"` (mature, stable)

**Update Policy:**

- **Security patches**: Update immediately (CVEs)
- **Minor versions**: Update quarterly (bug fixes, features)
- **Major versions**: Review breaking changes, update if beneficial

### Platform-Specific Considerations

**Windows:**

- **Compiler**: MSVC toolchain required (Windows API compatibility)
- **Runtime**: No additional DLLs (static linking)
- **Symlink**: Windows 10 1703+ recommended (Developer Mode)
- **File System**: NTFS required (hardlink support)

**Linux:**

- **Compiler**: GCC 7+ or Clang 10+
- **Kernel**: 4.0+ (modern FS features)
- **File Systems**: ext4, btrfs, xfs supported
- **Dependencies**: None (static linking via `bundled` feature)

**macOS:**

- **Compiler**: Xcode Command Line Tools
- **OS Version**: macOS 10.15+ (Catalina)
- **File System**: APFS, HFS+ supported
- **Architecture**: x86_64, aarch64 (Apple Silicon)

### Build Configuration

**Cargo.toml (workspace root):**

```toml
[workspace]
members = [
    "crates/modeld-core",
    "crates/modeld-daemon",
    "crates/modeld-cli",
    "crates/modeld-scanner",
    "crates/modeld-metadata",
]
resolver = "2"

[workspace.package]
version = "0.1.0"
edition = "2021"
rust-version = "1.70"
license = "MIT"
repository = "https://github.com/modeld/modeld"

[workspace.dependencies]
# Shared versions (DRY principle)
tokio = { version = "1.35", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
anyhow = "1.0"

[profile.release]
opt-level = 3          # Maximum optimization
lto = "thin"           # Link-time optimization (faster builds than "fat")
codegen-units = 1      # Better optimization (slower builds)
strip = true           # Strip symbols (smaller binary)
panic = "abort"        # Smaller binary (no unwind tables)

[profile.dev]
opt-level = 0          # No optimization (fast builds)
debug = true           # Full debug info

[profile.bench]
inherits = "release"
debug = true           # Debug info for profiling
```

**Rationale:**

- **Rust Edition 2021**: Latest stable edition (async improvements)
- **MSRV 1.70**: Minimum Supported Rust Version (released Jun 2023)
- **LTO "thin"**: Balance compile time vs performance (20% faster than "fat" LTO)
- **codegen-units = 1**: Better optimization for release builds
- **strip = true**: Reduce binary size (~30% smaller)

### Dependency Audit

**Security:**

```bash
# Audit dependencies for known vulnerabilities
cargo audit

# Update advisory database
cargo audit --update
```

**License Compliance:**

All dependencies use permissive licenses:
- MIT: Most dependencies
- Apache-2.0: Some Rust crates (compatible with MIT)
- BSD: Some libraries

**No copyleft licenses** (GPL, LGPL) - safe for commercial use.

---

---

## Repository Structure

This section documents the complete modeld repository layout including all Rust crates, Python packages, documentation, tests, and CI/CD configuration.

### Overview

The modeld repository is organized as a Rust workspace with multiple crates, a Python package for HuggingFace interception, comprehensive documentation including RFCs, and multi-platform testing infrastructure.

**Repository Root**: `modeld/`

### Complete Directory Tree

```
modeld/
├── Cargo.toml                  # Workspace root manifest (see Workspace Configuration)
├── Cargo.lock                  # Dependency lock file (committed to repo)
├── README.md                   # Project overview, quick start, installation
├── LICENSE                     # MIT License
├── .gitignore                  # Git ignore patterns
├── .gitattributes              # Git line ending configuration
├── DEVELOPMENT_PLAN.md         # Phase roadmap and architecture overview
│
├── crates/                     # Rust workspace crates (6 crates)
│   │
│   ├── modeld-core/            # Core library (business logic)
│   │   ├── Cargo.toml          # Dependencies: blake3, rusqlite, rayon, anyhow, thiserror
│   │   ├── src/
│   │   │   ├── lib.rs          # Public API exports
│   │   │   ├── cas.rs          # CAS storage layer implementation
│   │   │   ├── db.rs           # SQLite operations (models, aliases, refs, downloads, WAL)
│   │   │   ├── dedup.rs        # Deduplication engine (two-phase commit protocol)
│   │   │   ├── hash.rs         # BLAKE3 hashing (parallel, mmap, cache)
│   │   │   ├── refs.rs         # Reference tracker & GC logic
│   │   │   ├── virtual_fs.rs   # Virtual FS layer (link creation, category detection)
│   │   │   ├── types.rs        # Common types (ModelHash, Alias, RefType, etc.)
│   │   │   └── error.rs        # Custom error types (using thiserror)
│   │   ├── tests/              # Unit tests
│   │   │   ├── cas_tests.rs
│   │   │   ├── hash_tests.rs
│   │   │   └── dedup_tests.rs
│   │   └── benches/            # Benchmarks (using criterion)
│   │       ├── hash_bench.rs
│   │       └── db_bench.rs
│   │
│   ├── modeld-daemon/          # Background daemon service (future phase)
│   │   ├── Cargo.toml          # Dependencies: modeld-core, tokio, notify, tracing
│   │   ├── src/
│   │   │   ├── main.rs         # Daemon entry point
│   │   │   ├── server.rs       # IPC server (Unix socket / named pipe)
│   │   │   ├── watch.rs        # File system watcher (notify crate)
│   │   │   └── config.rs       # Daemon configuration
│   │   └── tests/
│   │       └── daemon_tests.rs
│   │
│   ├── modeld-cli/             # Command-line interface (primary user interface)
│   │   ├── Cargo.toml          # Dependencies: modeld-core, clap, indicatif, colored
│   │   ├── src/
│   │   │   ├── main.rs         # CLI entry point (argument parsing)
│   │   │   ├── commands/       # CLI command implementations
│   │   │   │   ├── mod.rs      # Command module exports
│   │   │   │   ├── scan.rs     # `modeld scan` - directory scanning
│   │   │   │   ├── dedup.rs    # `modeld dedup` - deduplication execution
│   │   │   │   ├── gc.rs       # `modeld gc` - garbage collection
│   │   │   │   ├── status.rs   # `modeld status` - show CAS statistics
│   │   │   │   ├── sync.rs     # `modeld sync` - refresh virtual FS links
│   │   │   │   ├── quarantine.rs # `modeld quarantine list/restore`
│   │   │   │   └── init.rs     # `modeld init` - initialize store
│   │   │   ├── output.rs       # Output formatting (tables, progress bars)
│   │   │   └── config.rs       # Config file loading (modeld.toml)
│   │   └── tests/
│   │       └── cli_tests.rs
│   │
│   ├── modeld-scanner/         # File system scanning library
│   │   ├── Cargo.toml          # Dependencies: walkdir, memmap2, blake3, rayon
│   │   ├── src/
│   │   │   ├── lib.rs          # Scanner API exports
│   │   │   ├── scanner.rs      # Recursive directory walker
│   │   │   ├── progress.rs     # Scan progress tracking
│   │   │   ├── filter.rs       # File extension filtering (.safetensors, .gguf, etc.)
│   │   │   └── cache.rs        # Hash cache (mtime + size → BLAKE3)
│   │   └── tests/
│   │       └── scanner_tests.rs
│   │
│   ├── modeld-metadata/        # Model format metadata parsers
│   │   ├── Cargo.toml          # Dependencies: serde, serde_json, safetensors
│   │   ├── src/
│   │   │   ├── lib.rs          # Parser API exports
│   │   │   ├── safetensors.rs  # Safetensors header parser (JSON metadata)
│   │   │   ├── gguf.rs         # GGUF format parser
│   │   │   ├── ckpt.rs         # PyTorch checkpoint parser (pickle)
│   │   │   ├── category.rs     # Model category detection (checkpoint/lora/vae)
│   │   │   └── types.rs        # Metadata types (Arch, Format, Category)
│   │   └── tests/
│   │       ├── safetensors_tests.rs
│   │       └── category_tests.rs
│   │
│   └── modeld-proxy/           # HuggingFace HTTP proxy (Phase 5, future)
│       ├── Cargo.toml          # Dependencies: modeld-core, axum, tokio
│       ├── src/
│       │   ├── lib.rs
│       │   ├── server.rs       # HTTP server (intercept HF downloads)
│       │   └── routes.rs       # Proxy routes
│       └── tests/
│           └── proxy_tests.rs
│
├── python/                     # Python package for HuggingFace interception
│   ├── pyproject.toml          # PEP 621 project metadata (see Python Package section)
│   ├── README.md               # Installation and usage instructions
│   ├── setup.py                # Legacy setup script (optional, for compatibility)
│   │
│   ├── modeld_hook/            # Main package
│   │   ├── __init__.py         # Package exports, version
│   │   ├── intercept.py        # Monkey-patch HuggingFace Hub download functions
│   │   ├── cache.py            # Fake HF cache structure management
│   │   ├── mapping.py          # SHA256 ↔ BLAKE3 hash mapping
│   │   ├── client.py           # IPC client to communicate with modeld daemon
│   │   └── utils.py            # Helper functions (logging, config)
│   │
│   ├── tests/                  # Python unit tests (pytest)
│   │   ├── __init__.py
│   │   ├── test_hook.py        # Test monkey-patching
│   │   ├── test_cache.py       # Test fake cache structure
│   │   ├── test_mapping.py     # Test hash mapping
│   │   └── conftest.py         # Pytest fixtures
│   │
│   └── examples/               # Usage examples
│       ├── basic_usage.py      # Explicit import example
│       ├── sitecustomize.py    # System-wide installation example
│       └── diffusers_example.py # Integration with diffusers library
│
├── docs/                       # Documentation (markdown)
│   │
│   ├── rfcs/                   # Request for Comments (design specifications)
│   │   ├── README.md           # RFC index, how to read RFCs
│   │   ├── 0001-storage-layout.md     # CAS directory structure design
│   │   ├── 0002-hash-strategy.md      # BLAKE3 hashing strategy
│   │   ├── 0003-ref-model.md          # Reference tracking & GC
│   │   ├── 0004-dedup-strategy.md     # Deduplication two-phase commit
│   │   ├── 0005-windows-compat.md     # Windows compatibility (CRITICAL)
│   │   ├── 0006-virtual-fs.md         # Virtual FS for AI frontends
│   │   └── 0007-hf-interception.md    # HuggingFace interception approach
│   │
│   ├── architecture.md         # High-level architecture overview (Phase 0 summary)
│   ├── api.md                  # Public API documentation (modeld-core library)
│   ├── cli-reference.md        # Complete CLI command reference
│   │
│   ├── user-guide/             # End-user documentation
│   │   ├── installation.md     # Installation instructions (all platforms)
│   │   ├── quick-start.md      # Quick start tutorial
│   │   ├── scanning.md         # How to scan model directories
│   │   ├── deduplication.md    # How to deduplicate models
│   │   ├── garbage-collection.md # How to use GC safely
│   │   ├── virtual-fs.md       # How to integrate with AI frontends
│   │   └── configuration.md    # modeld.toml configuration reference
│   │
│   ├── windows-setup.md        # Windows-specific setup (Developer Mode, symlinks)
│   ├── troubleshooting.md      # Common issues and solutions
│   ├── faq.md                  # Frequently Asked Questions
│   └── CHANGELOG.md            # Version history and release notes
│
├── tests/                      # Integration and end-to-end tests
│   │
│   ├── integration/            # Rust integration tests
│   │   ├── test_scan.rs        # Test directory scanning workflow
│   │   ├── test_dedup.rs       # Test deduplication execution
│   │   ├── test_crash_recovery.rs # Test WAL recovery after crashes
│   │   ├── test_gc.rs          # Test garbage collection
│   │   ├── test_virtual_fs.rs  # Test virtual FS link creation
│   │   └── test_windows.rs     # Windows-specific link strategy tests
│   │
│   ├── fixtures/               # Test data fixtures
│   │   ├── test_models/        # Sample model files for testing
│   │   │   ├── sdxl.safetensors  # Example checkpoint (small, fake data)
│   │   │   ├── lora.safetensors  # Example LoRA
│   │   │   └── vae.safetensors   # Example VAE
│   │   ├── workflows/          # Sample ComfyUI workflows
│   │   │   └── example_workflow.json
│   │   └── configs/            # Test configuration files
│   │       └── test_modeld.toml
│   │
│   └── e2e/                    # End-to-end scenario tests (Python + Rust)
│       ├── test_hf_download.py # Test HF interception with real downloads
│       ├── test_comfyui_integration.py # Test ComfyUI integration
│       └── test_full_workflow.sh # Bash script for complete workflow test
│
├── .github/                    # GitHub-specific configuration
│   │
│   ├── workflows/              # GitHub Actions CI/CD pipelines
│   │   ├── ci.yml              # Main CI pipeline (build, test, lint)
│   │   ├── release.yml         # Release automation (tag → build → publish)
│   │   ├── test-windows.yml    # Windows-specific testing (3 link scenarios)
│   │   ├── test-linux.yml      # Linux testing (Ubuntu, ext4/btrfs)
│   │   ├── test-macos.yml      # macOS testing (APFS compatibility)
│   │   └── benchmark.yml       # Performance benchmarks (hash speed, DB perf)
│   │
│   ├── ISSUE_TEMPLATE/         # GitHub issue templates
│   │   ├── bug_report.md       # Bug report template
│   │   ├── feature_request.md  # Feature request template
│   │   └── windows_issue.md    # Windows-specific issue template
│   │
│   ├── PULL_REQUEST_TEMPLATE.md # PR template
│   └── CODEOWNERS              # Code ownership for review assignments
│
├── scripts/                    # Build and development scripts
│   ├── build-release.sh        # Cross-platform release build script
│   ├── install-dev.sh          # Dev environment setup (Rust, Python)
│   ├── run-benchmarks.sh       # Execute all benchmarks
│   ├── generate-fixtures.py    # Generate test model fixtures
│   └── check-windows-privs.ps1 # PowerShell script to check Windows privileges
│
├── config/                     # Configuration file examples
│   ├── modeld.toml.example     # Example modeld configuration
│   └── sitecustomize.py.example # Example system-wide Python hook
│
├── .cargo/                     # Cargo configuration
│   └── config.toml             # Cargo build configuration (target-specific)
│
├── .gitignore                  # Git ignore patterns
├── .gitattributes              # Git attributes (line endings, diffs)
├── rustfmt.toml                # Rust code formatting rules
├── clippy.toml                 # Clippy linter configuration
└── deny.toml                   # cargo-deny security/license checks
```

### Rust Crate Descriptions

#### 1. **modeld-core** (Core Library)

**Purpose**: Central business logic library containing all core functionality for CAS storage, deduplication, hashing, and metadata management.

**Public API**: Provides the foundational API used by CLI, daemon, and other tools.

**Key Modules**:
- `cas.rs`: CAS storage implementation (create, read, delete, quarantine operations)
- `db.rs`: SQLite database operations (schema, queries, transactions)
- `dedup.rs`: Deduplication engine (two-phase commit, canonical path selection)
- `hash.rs`: BLAKE3 hashing (parallel, mmap, incremental cache)
- `refs.rs`: Reference tracker and garbage collection logic
- `virtual_fs.rs`: Virtual filesystem layer (link creation, category detection)
- `types.rs`: Shared data types (ModelHash, Alias, RefType, etc.)
- `error.rs`: Custom error types using thiserror

**Dependencies**: blake3, rusqlite, rayon, anyhow, thiserror, chrono, uuid

**Testing**: Unit tests in `tests/`, benchmarks in `benches/`

---

#### 2. **modeld-daemon** (Background Service)

**Purpose**: Optional background daemon for file watching and automatic deduplication triggers (future phase).

**Functionality**:
- IPC server (Unix socket on Linux/macOS, named pipe on Windows)
- File system watcher using `notify` crate (inotify/FSEvents/ReadDirectoryChanges)
- Automatic dedup triggers when new models detected
- Status monitoring and reporting

**Dependencies**: modeld-core, tokio (async runtime), notify (file watching), tracing (logging)

**Status**: Planned for Phase 3+

---

#### 3. **modeld-cli** (Command-Line Interface)

**Purpose**: Primary user-facing tool for all modeld operations.

**Commands**:
- `modeld init`: Initialize CAS store (create directories, database)
- `modeld scan <path>`: Scan directory for models, compute hashes
- `modeld dedup [--mode=interactive|dry-run|auto]`: Execute deduplication
- `modeld gc [--mode=safe|dry-run|auto]`: Garbage collection
- `modeld status`: Show CAS statistics (models, space used, duplicates)
- `modeld sync`: Refresh virtual FS links
- `modeld quarantine list`: Show quarantined models
- `modeld quarantine restore <hash>`: Restore quarantined model

**Dependencies**: modeld-core, clap (CLI parsing), indicatif (progress bars), colored (terminal colors)

**Configuration**: Reads `modeld.toml` from `~/.config/modeld/` or `$MODELD_CONFIG`

---

#### 4. **modeld-scanner** (File System Scanner)

**Purpose**: Library for recursive directory traversal, file filtering, and hash computation.

**Functionality**:
- Recursive directory walking using `walkdir`
- File extension filtering (`.safetensors`, `.gguf`, `.ckpt`, `.pth`, `.bin`)
- Hash cache (mtime + size → BLAKE3, avoids recomputation)
- Parallel hash computation using `rayon`
- Progress tracking and reporting

**Dependencies**: walkdir, memmap2, blake3, rayon

**Usage**: Used by `modeld-cli` for `scan` command, potentially by `modeld-daemon` for watching

---

#### 5. **modeld-metadata** (Format Parsers)

**Purpose**: Parse model file formats to extract metadata (architecture, category, base model).

**Supported Formats**:
- **Safetensors**: JSON header parsing (most common AI model format)
- **GGUF**: Binary format parser (LLM quantized models)
- **CKPT**: PyTorch checkpoint (pickle format, limited metadata)

**Category Detection**: Heuristics to classify models (checkpoint, lora, vae, controlnet, embedding, upscaler)

**Dependencies**: serde, serde_json, safetensors (official parser)

**Usage**: Used by `modeld-core` for virtual FS category sorting

---

#### 6. **modeld-proxy** (HuggingFace HTTP Proxy)

**Purpose**: Optional HTTP proxy server to intercept HuggingFace downloads without Python hooks (future phase).

**Functionality**:
- HTTP proxy listening on localhost (e.g., `127.0.0.1:8765`)
- Intercept requests to `huggingface.co/*/resolve/*`
- Download to CAS if not present
- Return existing CAS file if already downloaded
- SHA256 ↔ BLAKE3 mapping

**Dependencies**: modeld-core, axum (HTTP server), tokio (async runtime)

**Status**: Planned for Phase 5+ (alternative to Python hook)

---

### Python Package Structure

#### **modeld-hook** Package

**Purpose**: HuggingFace download interception to deduplicate downloads across frameworks (ComfyUI, diffusers, transformers).

**Installation Methods**:
1. **Explicit import**: `import modeld_hook` in scripts
2. **System-wide**: Copy `sitecustomize.py` to Python site-packages
3. **Session-level**: Set `PYTHONSTARTUP` environment variable

**Modules**:

**`intercept.py`**: 
- Monkey-patches `huggingface_hub.hf_hub_download()` and `snapshot_download()`
- Checks if file already in CAS (via SHA256 → BLAKE3 mapping)
- If found, creates fake HF cache symlink to CAS
- If not found, downloads normally, hashes, moves to CAS

**`cache.py`**:
- Creates fake HuggingFace cache structure (`blobs/`, `snapshots/`, `refs/`)
- Manages symlinks: `blobs/{sha256}` → `cas/blake3/{prefix}/{hash}`
- Handles revision tracking (git-like refs)

**`mapping.py`**:
- Queries modeld SQLite database for SHA256 ↔ BLAKE3 mappings
- Inserts new mappings after downloads
- Handles concurrency (multiple processes downloading)

**`client.py`**:
- IPC client to communicate with modeld daemon (if running)
- Falls back to direct database access if daemon not available

**`utils.py`**:
- Logging configuration
- Config file reading (`modeld.toml`)
- Helper functions

**Dependencies**: `huggingface-hub>=0.20.0`, `requests>=2.31.0`, `python-json-logger>=2.0.0`

**Testing**: Pytest-based tests in `tests/` directory

---

### Documentation Structure

#### **RFCs (Request for Comments)**

Location: `docs/rfcs/`

**Purpose**: Detailed design specifications for major architectural decisions.

**Format**: Markdown files with consistent structure:
- **Status**: Draft / Accepted / Implemented / Deprecated
- **Motivation**: Why this design is needed
- **Proposed Design**: Detailed specification
- **Alternatives Considered**: What was rejected and why
- **Implementation Notes**: Phase, complexity, dependencies

**Index**: `docs/rfcs/README.md` provides RFC overview and reading order

---

#### **User Guide**

Location: `docs/user-guide/`

**Purpose**: End-user documentation for installation, configuration, and usage.

**Target Audience**: AI practitioners using ComfyUI/Forge/A1111 (not necessarily developers)

**Structure**:
- Step-by-step tutorials with screenshots
- Common use cases and workflows
- Troubleshooting guides
- Platform-specific instructions (Windows vs Linux vs macOS)

---

#### **API Documentation**

Location: `docs/api.md`

**Purpose**: Developer documentation for `modeld-core` library API.

**Generated From**: Rust doc comments (`cargo doc`)

**Contents**: Public structs, traits, functions, and usage examples

---

### Test Structure

#### **Unit Tests**

Location: Within each crate's `tests/` directory or inline with `#[cfg(test)]`

**Scope**: Test individual functions, modules in isolation

**Framework**: Rust built-in test framework + `rstest` for fixtures

**Coverage Goal**: ≥80% code coverage for core logic

---

#### **Integration Tests**

Location: `tests/integration/`

**Scope**: Test interactions between crates, end-to-end workflows

**Scenarios**:
- Complete scan → dedup → virtual FS workflow
- Crash recovery (kill process mid-dedup, verify WAL recovery)
- Multi-platform link strategy tests (especially Windows)
- GC safety (verify no accidental deletion)

**Framework**: Rust integration test framework + `tempfile` for test environments

---

#### **End-to-End Tests**

Location: `tests/e2e/`

**Scope**: Full system tests including Python hook and AI frameworks

**Scenarios**:
- HuggingFace download interception (real HTTP download)
- ComfyUI workflow execution with virtual FS models
- Cross-framework deduplication (download in diffusers, use in ComfyUI)

**Framework**: Pytest (Python) + Bash scripts + Docker containers (optional)

---

#### **Test Fixtures**

Location: `tests/fixtures/`

**Purpose**: Sample data for repeatable tests

**Contents**:
- Small fake model files (safetensors, gguf) with valid headers
- ComfyUI workflow JSON files
- Configuration file examples

**Generation**: `scripts/generate-fixtures.py` creates test data

---

### CI/CD Configuration

#### **GitHub Actions Workflows**

Location: `.github/workflows/`

**CI Pipeline** (`ci.yml`):
- Triggers: Push to main, pull requests
- Jobs:
  - **Build**: Compile all crates (`cargo build --workspace`)
  - **Test**: Run unit + integration tests (`cargo test --workspace`)
  - **Lint**: Clippy lints (`cargo clippy --all-targets`)
  - **Format**: Check code formatting (`cargo fmt --check`)
  - **Security**: `cargo-deny` for dependency auditing
- Matrix: Rust stable + nightly, OS (Ubuntu, Windows, macOS)

**Release Pipeline** (`release.yml`):
- Trigger: Git tag push (`v*`)
- Jobs:
  - **Build Binaries**: Cross-compile for Windows, Linux, macOS (x64 + ARM)
  - **Create Release**: GitHub release with binaries
  - **Publish Crates**: `cargo publish` to crates.io
  - **Publish Python**: `twine upload` to PyPI

**Platform-Specific Tests**:
- `test-windows.yml`: Test all 3 Windows link scenarios (same volume, cross volume + privs, no privs)
- `test-linux.yml`: Test on Ubuntu, ext4 and btrfs filesystems
- `test-macos.yml`: Test on macOS with APFS

**Benchmarks** (`benchmark.yml`):
- Trigger: Weekly schedule + manual dispatch
- Jobs: Run criterion benchmarks, generate HTML reports
- Artifacts: Upload reports to GitHub Pages

---

### Workspace Configuration

**Root `Cargo.toml`**:

```toml
[workspace]
members = [
    "crates/modeld-core",
    "crates/modeld-daemon",
    "crates/modeld-cli",
    "crates/modeld-scanner",
    "crates/modeld-metadata",
    "crates/modeld-proxy",
]

[workspace.package]
version = "0.1.0"
authors = ["modeld contributors"]
edition = "2021"
license = "MIT"
repository = "https://github.com/your-org/modeld"

[workspace.dependencies]
# Shared dependencies with consistent versions
blake3 = { version = "1.5", features = ["rayon", "mmap"] }
rusqlite = { version = "0.30", features = ["bundled"] }
tokio = { version = "1.35", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
anyhow = "1.0"
thiserror = "1.0"

[profile.release]
opt-level = 3           # Maximum optimization
lto = "thin"            # Link-time optimization (thin = faster build)
codegen-units = 1       # Better optimization (slower build)
strip = true            # Strip debug symbols
panic = "abort"         # Smaller binary size
```

---

### Build and Development Scripts

Location: `scripts/`

**`build-release.sh`**:
- Cross-platform release builds (x86_64 + aarch64)
- Uses `cargo build --release --target <triple>`
- Creates tarball/zip archives with binaries + docs

**`install-dev.sh`**:
- Installs Rust toolchain (rustup)
- Installs Python dependencies (pip install -e python/)
- Installs development tools (cargo-watch, cargo-deny, clippy)

**`run-benchmarks.sh`**:
- Executes all criterion benchmarks
- Generates comparison reports
- Archives results with timestamps

**`generate-fixtures.py`**:
- Generates fake model files with valid headers
- Creates test ComfyUI workflows
- Populates `tests/fixtures/` directory

**`check-windows-privs.ps1`** (PowerShell):
- Checks if Developer Mode enabled
- Tests symlink creation privileges
- Provides setup instructions if privileges missing

---

### Configuration Files

#### **`modeld.toml.example`**

Location: `config/modeld.toml.example`

**Purpose**: Example configuration file (users copy to `~/.config/modeld/modeld.toml`)

**Structure**:
```toml
[store]
path = "~/.local/share/modeld"  # CAS storage location
quarantine_ttl_days = 30        # Quarantine period before deletion

[scan]
extensions = [".safetensors", ".gguf", ".ckpt", ".pth", ".bin"]
exclude_dirs = ["tmp", "cache", ".git"]
hash_cache_enabled = true

[dedup]
default_mode = "interactive"    # interactive | dry-run | auto

[gc]
default_mode = "safe"           # safe | dry-run | auto | aggressive
auto_trigger_threshold_gb = 10  # Free space threshold for auto GC

[virtual_fs]
enabled = true
path = "~/.local/share/modeld/virtual"
frontends = ["comfyui", "forge", "a1111"]

[hf_interception]
enabled = true
hf_home = "~/.local/share/modeld/hf_cache"

[logging]
level = "info"                  # trace | debug | info | warn | error
file = "~/.local/share/modeld/modeld.log"
```

---

### Summary

The modeld repository is structured as:

1. **Rust Workspace** (6 crates): Core logic + CLI + daemon + specialized libraries
2. **Python Package** (modeld-hook): HuggingFace interception
3. **Documentation** (RFCs + user guides + API docs): Comprehensive design and usage docs
4. **Tests** (unit + integration + e2e): Multi-layer testing strategy
5. **CI/CD** (GitHub Actions): Automated build, test, release pipelines
6. **Scripts** (development tools): Build automation, fixture generation, privilege checks

**Repository Goals**:
- **Modularity**: Each crate has a single responsibility
- **Cross-platform**: Windows, Linux, macOS support with platform-specific tests
- **Developer-friendly**: Comprehensive docs, examples, clear structure
- **Production-ready**: CI/CD, security audits, benchmarks, release automation

**Next Steps** (Post-Phase 0):
- Create initial repository structure (`cargo new --lib` for crates)
- Set up CI/CD pipelines (`.github/workflows/`)
- Generate test fixtures (`scripts/generate-fixtures.py`)
- Begin Phase 1 implementation (modeld-scanner + modeld-core)

---

## Success Criteria (Phase 0)

- [x] All 7 RFCs documented in this design
- [x] Windows compatibility strategy clearly defined
- [x] SQLite schema v1 finalized
- [x] CAS layout specified with rationale
- [x] Transactional move protocol documented
- [x] Hash strategy with performance targets
- [x] Reference counting model specified
- [x] Virtual FS design complete
- [x] HF interception approach decided
- [x] Technical stack documented
- [x] Repository structure defined

**Review Checklist**:

1. Can someone read this doc and understand the entire system?
2. Are all edge cases for Windows addressed?
3. Are crash recovery scenarios covered?
4. Is the hash caching strategy clear?
5. Are performance targets realistic?
6. Is the reference model sound (no accidental deletions)?
7. Is the HF interception approach stable?

---

## Next Steps (Post Phase 0)

After completing this design document:

1. **RFC Review**: Get feedback on each RFC
2. **Prototype Key Components**:
   - BLAKE3 hashing performance test
   - SQLite schema migration system
   - Windows privilege detection
3. **Begin Phase 1**: Core Scanner MVP implementation
4. **Set up CI/CD**: Multi-platform testing
5. **Documentation**: User-facing setup guides

---

## Open Questions

1. **Chunk size optimization**: Need benchmarks across different hardware
2. **Quarantine TTL**: Is 30 days the right default?
3. **Virtual FS performance**: Does link overhead matter for model loading?
4. **HF interception stability**: Will monkey-patching survive HF updates?
5. **macOS compatibility**: Are there APFS-specific issues?

---

## Glossary

### Core Terminology

**CAS (Content-Addressable Storage)**: A storage system where data is indexed and retrieved using a cryptographic hash of its content, rather than a file path or name. modeld uses BLAKE3 hashes as content addresses.

**BLAKE3**: A cryptographic hash function producing 256-bit (64 hex character) hashes, chosen for its speed (3-10 GB/s) and parallelizability. Successor to BLAKE2, optimized for modern CPUs.

**SHA256**: A 256-bit cryptographic hash function used by HuggingFace Hub for content addressing. modeld maintains SHA256↔BLAKE3 mappings for deduplication of HF downloads.

**Hardlink**: A filesystem feature where multiple directory entries point to the same inode (file data). Hardlinks are transparent, zero-overhead, but cannot span volumes. Used by modeld for same-volume deduplication.

**Symlink (Symbolic Link)**: A special file that contains a reference path to another file or directory. On Windows, requires SeCreateSymbolicLinkPrivilege (Developer Mode). Used for cross-volume deduplication.

**Junction Point**: A Windows-specific directory symlink (NTFS reparse point) that doesn't require special privileges. Directory-only, absolute paths only. Used for virtual FS directory mounting.

**Prefix Sharding**: A technique to distribute files across subdirectories using the first N characters of the hash. modeld uses 2-char prefixes (00-ff) creating 256 shards to avoid single-directory bottlenecks.

**WAL (Write-Ahead Log)**: A crash recovery mechanism where operations are logged before execution. modeld uses WAL for atomic deduplication (two-phase commit) and SQLite WAL mode for database concurrency.

**Two-Phase Commit**: An atomic transaction protocol with Prepare (Phase A) and Commit (Phase B) stages. Used by modeld to ensure deduplication operations are crash-safe.

**Quarantine**: A soft-delete mechanism where models with zero references are moved to a grace period directory (30 days default) before permanent deletion, allowing recovery from mistakes.

**Protection Levels**: The GC safety states of a model:
- **PROTECTED**: ref_count > 0, cannot be deleted
- **QUARANTINE**: ref_count = 0, soft-deleted with TTL
- **DELETED**: Permanently removed after TTL expires

### Reference & Deduplication

**Explicit Reference**: A recorded dependency from a workflow, user favorite, or download record to a model. Stored in the `refs` table.

**Implicit Reference (Alias)**: A filesystem-level reference via hardlinks, symlinks, or original file locations. Stored in the `aliases` table.

**Reference Counting**: The total number of explicit and implicit references to a model: `ref_count = COUNT(refs) + COUNT(aliases)`. Models with ref_count > 0 are protected from GC.

**Canonical Path**: The "primary" file selected from a duplicate group during deduplication. Selection priority: (1) Already in CAS, (2) Oldest mtime, (3) Shortest path, (4) Alphabetically first.

**Deduplication Modes**:
- **Interactive**: Prompts for confirmation before each duplicate group
- **Dry-run**: Shows potential savings without making changes
- **Auto**: Non-interactive, suitable for scheduled tasks
- **Report**: Analysis only, generates savings report

### HuggingFace Integration

**HF_HOME**: Environment variable specifying the HuggingFace cache directory. modeld sets this to redirect all HF downloads to its managed cache.

**Fake HF Cache**: A directory structure emulating the official HuggingFace Hub cache layout (`blobs/`, `snapshots/`, `refs/`) with symlinks pointing to modeld CAS objects.

**SHA256↔BLAKE3 Mapping**: A bidirectional hash mapping stored in the `downloads` table, enabling deduplication when either HuggingFace's SHA256 or modeld's BLAKE3 is known.

**Monkeypatch**: Runtime modification of library functions (e.g., `huggingface_hub.hf_hub_download()`) to intercept downloads. Layer 2 fallback when HF_HOME is insufficient.

### Virtual Filesystem

**Virtual FS Layer**: A directory structure (`$MODELD_STORE/virtual/`) providing familiar frontend-specific layouts (comfyui/, forge/, a1111/) with hardlinks/symlinks to CAS objects.

**Category Detection**: Multi-layer algorithm to classify models by type (checkpoint, lora, vae, controlnet, embedding, upscale, clip) using filename patterns, size heuristics, safetensors metadata, and tensor analysis.

**Link Refresh**: Incremental synchronization process that updates virtual FS links after models are added, removed, or recategorized in CAS.

### Storage & Performance

**mmap (Memory-Mapped File I/O)**: A technique where a file is mapped into virtual memory, allowing efficient access to large files. modeld uses mmap for files ≥10MB during parallel hashing.

**Chunk Size**: The unit size for parallel hash computation. modeld uses 64MB chunks to balance parallelism (8 threads on 8-core CPU) with memory usage (~512MB peak).

**Hash Cache**: An mtime/size-based cache mapping file paths to BLAKE3 hashes, avoiding redundant hash computation during scans. Stored in the `hash_cache` table.

**Incremental Hashing**: Strategy where unchanged files (same mtime + size) reuse cached hashes instead of re-computing, enabling sub-minute rescans of TB-scale collections.

### Windows Compatibility

**Developer Mode**: A Windows 10/11 feature (Settings → For Developers) granting standard users SeCreateSymbolicLinkPrivilege for symlink creation without Administrator elevation.

**SeCreateSymbolicLinkPrivilege**: Windows security privilege required to create symbolic links. Normally granted only to Administrators, but enabled for standard users via Developer Mode.

**Cross-Volume Limitation**: Windows NTFS restriction preventing hardlinks from spanning volumes (e.g., C:\ to D:\). Requires symlinks (with privileges) or reference-only fallback.

**Reference-Only Mode**: A degraded deduplication mode where modeld tracks files in the database but cannot create physical links due to privilege or filesystem limitations. No space savings.

### Database Schema

**models table**: Central registry of all CAS objects, keyed by `blake3_hash`, with metadata (size, format, arch, category), lifecycle timestamps, and quarantine status.

**aliases table**: Filesystem path mappings to models, recording link type (hardlink/symlink/junction/reference_only) and frontend categorization.

**refs table**: Explicit references from workflows, user favorites, or manual keeps to models, with type classification (lora/checkpoint/vae/controlnet).

**downloads table**: HuggingFace download tracking with SHA256↔BLAKE3 mapping, source URLs, download status, and progress information.

**wal_transactions table**: Write-ahead log for crash recovery of long-running operations (dedup, download, gc) with transaction status (pending/copied/committed/failed).

### AI Frontends & Formats

**ComfyUI**: Node-based AI workflow editor with directory structure: `models/checkpoints/`, `models/loras/`, `models/vae/`, etc.

**Forge**: WebUI fork based on A1111 with similar structure: `models/Stable-diffusion/`, `models/Lora/`, `models/VAE/`.

**A1111 (Automatic1111)**: Popular Stable Diffusion WebUI with structure: `models/Stable-diffusion/`, `models/Lora/`, `models/VAE/`.

**Safetensors**: A safe, fast tensor file format with JSON metadata header. Enables efficient loading and metadata parsing without executing code.

**gguf**: GPT-Generated Unified Format for large language models (LLAMA, Mistral, etc.). Binary format with metadata.

**LoRA (Low-Rank Adaptation)**: A technique for fine-tuning large models with small parameter sets (10-500MB). Commonly used for style/character adaptations.

**VAE (Variational Autoencoder)**: The encoder/decoder component of diffusion models (300-800MB). Can be swapped for different aesthetic styles.

**ControlNet**: Spatial conditioning model for guided generation (e.g., canny edges, pose). Typically 1-2GB.

### Miscellaneous

**NVMe**: Non-Volatile Memory Express, a high-speed SSD interface standard. modeld's 2GB/s hash target assumes NVMe Gen3+ storage.

**NTFS (New Technology File System)**: Primary Windows filesystem supporting hardlinks, symlinks (with privileges), and junctions. Default for C:\ drive.

**ReFS (Resilient File System)**: Modern Windows filesystem for servers/storage with data integrity checksums. Similar link support to NTFS.

**exFAT**: Extended File Allocation Table, cross-platform filesystem for USB drives. No support for hardlinks, symlinks, or junctions.

**FAT32**: Legacy filesystem with 4GB file size limit. No advanced features. Not recommended for modeld (most AI models exceed 4GB).

**OCI (Open Container Initiative)**: Industry standard for container images and artifacts. Future modeld compatibility planned via SHA256 blob storage.

---

*Design Document Version: 1.0*
*Author: modeld project*
*Date: 2024*
*Status: Draft - Phase 0 Architecture Design*
