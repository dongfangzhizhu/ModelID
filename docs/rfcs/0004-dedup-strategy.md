# RFC 0004: Deduplication Strategy

**Status**: Draft  
**Author**: modeld Architecture Team  
**Created**: 2024  
**Last Updated**: 2024

## Abstract

This RFC defines the file deduplication approach for modeld with comprehensive transactional safety, including the canonical path selection algorithm, two-phase commit protocol for atomic operations, Write-Ahead Log (WAL) record format for crash recovery, multiple deduplication modes (interactive, dry-run, auto, report), progress reporting format, and error handling strategies. The design ensures that deduplication operations are safe, transparent, and recoverable even in crash scenarios.

## Motivation

AI model collections often contain significant duplication:
- **Same model across multiple frontends** (ComfyUI, Forge, A1111 all using SDXL base model)
- **User-created duplicates** (copying models between directories for testing)
- **Downloaded duplicates** (different sources providing identical models)
- **Version confusion** (multiple copies of "same" model with different names)

Without deduplication, users waste massive amounts of disk space:
- **Typical scenario**: 500GB model collection with 40% duplication = 200GB wasted
- **Extreme scenario**: 2TB collection with 60% duplication = 1.2TB wasted

However, deduplication carries significant risks:
1. **Data loss** if operations fail mid-flight (power loss, crashes)
2. **Broken workflows** if wrong file is chosen as canonical
3. **Corrupted models** if hash verification fails
4. **Inconsistent state** if some duplicates are deduplicated but others fail

This RFC provides a comprehensive deduplication strategy that addresses all these risks while maximizing space savings.

## Problem Statement

Design a file deduplication system that:

1. **Safely deduplicates** identical files across multiple locations
2. **Selects canonical paths** deterministically and intuitively
3. **Uses atomic operations** to prevent partial state during failures
4. **Provides crash recovery** through Write-Ahead Logging
5. **Supports multiple modes** (interactive, dry-run, auto, report)
6. **Reports progress** clearly during long operations
7. **Handles errors gracefully** with comprehensive fallback strategies
8. **Works cross-platform** (Windows, Linux, macOS) with filesystem-specific strategies
9. **Scales efficiently** to thousands of duplicate groups

## Proposed Design

### Canonical Path Selection Algorithm

When multiple identical files are found (same BLAKE3 hash), one must be chosen as the "canonical" file that remains in place, while others are replaced with hardlinks/symlinks.

**Priority Order**:

1. **Already in CAS** → Use existing CAS object
2. **Oldest mtime** → Likely the original file
3. **Shortest path** → Simpler to reference, easier for users to understand
4. **First alphabetically** → Deterministic tiebreaker for stability

**Rationale**:

- **Priority 1 (CAS first)**: If file already in CAS, it's immutable and verified → safest choice
- **Priority 2 (oldest)**: Original file is typically the oldest, copies are newer
- **Priority 3 (shortest)**: Paths like `models/sdxl.safetensors` preferred over `backup/old/2023/sdxl-copy-final-v2.safetensors`
- **Priority 4 (alphabetical)**: Ensures deterministic, reproducible selection across runs

#### Algorithm Pseudocode

```rust
fn select_canonical(duplicates: &[PathBuf]) -> PathBuf {
    duplicates.iter()
        .min_by_key(|path| {
            let in_cas = path.starts_with("$MODELD_STORE/cas");
            let mtime = path.metadata()
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            let path_len = path.as_os_str().len();
            let path_str = path.to_string_lossy().to_string();
            
            // Tuple ordering: (in_cas first, then oldest, then shortest, then alphabetical)
            // Note: !in_cas makes true (in CAS) sort before false (not in CAS)
            (!in_cas, mtime, path_len, path_str)
        })
        .unwrap()
        .clone()
}
```

#### Selection Examples

**Example 1: Multiple copies in user directories**

```
Group: BLAKE3 hash abcdef123...
Files:
  1. C:\Users\Alice\Downloads\sdxl-base-1.0.safetensors (mtime: 2024-01-10)
  2. C:\ComfyUI\models\checkpoints\sdxl-base-1.0.safetensors (mtime: 2024-01-12)
  3. D:\Forge\models\Stable-diffusion\sdxl-base-1.0.safetensors (mtime: 2024-01-15)

Selection:
  Priority 1 (CAS): None in CAS
  Priority 2 (oldest): #1 (2024-01-10) ← Selected
  
Canonical: C:\Users\Alice\Downloads\sdxl-base-1.0.safetensors
Rationale: Oldest file, likely the original download
```

**Example 2: One file already in CAS**

```
Group: BLAKE3 hash abcdef123...
Files:
  1. $MODELD_STORE/cas/blake3/ab/abcdef123... (mtime: 2024-01-05)
  2. C:\ComfyUI\models\checkpoints\sdxl.safetensors (mtime: 2024-01-01)
  
Selection:
  Priority 1 (CAS): #1 ← Selected
  
Canonical: $MODELD_STORE/cas/blake3/ab/abcdef123...
Rationale: Already in immutable CAS, no need to move
```

**Example 3: Same mtime, different path lengths**

```
Group: BLAKE3 hash abcdef123...
Files:
  1. D:\AI\models\checkpoints\backup\old\archive\sdxl-base-final-v3.safetensors (mtime: 2024-01-10, len: 75)
  2. C:\models\sdxl.safetensors (mtime: 2024-01-10, len: 25)
  
Selection:
  Priority 1 (CAS): None
  Priority 2 (oldest): Tie (both 2024-01-10)
  Priority 3 (shortest): #2 (25 chars) ← Selected
  
Canonical: C:\models\sdxl.safetensors
Rationale: Much shorter, simpler path
```

**Example 4: Full tie, alphabetical tiebreaker**

```
Group: BLAKE3 hash abcdef123...
Files:
  1. C:\models\sdxl-base.safetensors (mtime: 2024-01-10, len: 30)
  2. C:\models\sdxl-copy.safetensors (mtime: 2024-01-10, len: 30)
  
Selection:
  Priority 1-3: Tie
  Priority 4 (alphabetical): "sdxl-base" < "sdxl-copy" ← #1 Selected
  
Canonical: C:\models\sdxl-base.safetensors
Rationale: Alphabetically first, deterministic
```

### Two-Phase Commit Protocol

The deduplication process uses a two-phase commit protocol to ensure atomicity and crash recovery. This ensures that files are never left in an inconsistent state.

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
```

#### Phase A: Prepare (Write-Ahead)

**Goal**: Copy canonical file to staging area and verify integrity before making any permanent changes.

**Steps**:

1. **Transaction ID generation**: Create unique UUID for this operation
   - Enables tracking and recovery
   - Example: `550e8400-e29b-41d4-a716-446655440000`

2. **WAL record creation**: Write transaction record with `status='pending'`
   - Enables detection of incomplete transactions on restart
   - Records source path and target hash for recovery

3. **Copy to staging**: Copy canonical file to temporary staging area
   - Destination: `tmp/cas_staging/{full_blake3_hash}.tmp`
   - Preserves timestamps where possible (for forensics)
   - Uses buffered I/O for efficiency on large files

4. **Hash verification**: Recompute BLAKE3 hash of staged file
   - Ensures no corruption during copy
   - If mismatch → ROLLBACK (delete tmp file, mark transaction failed)
   - Critical safety check: never commit corrupted data

5. **Update WAL status**: Mark transaction as `status='copied'`
   - Indicates Phase A completed successfully
   - Crash recovery can resume from Phase B

6. **fsync WAL**: Force WAL write to disk
   - Ensures crash recovery will see 'copied' status
   - Without fsync, power loss might lose WAL update → transaction rolled back on recovery (safe)

**Failure Handling in Phase A**:

- **Copy fails** (disk full, I/O error): Delete tmp file, mark transaction failed, continue to next group
- **Hash mismatch**: Delete tmp file, log error, mark transaction failed, investigate source file corruption
- **Crash during Phase A**: On restart, WAL shows `status='pending'` → Resume from step 3 or rollback
- **No permanent changes**: Original files remain untouched until Phase B

#### Phase B: Commit (Make Changes Visible)

**Goal**: Atomically move staged file to CAS and replace duplicates with links.

**Steps**:

7. **Atomic rename to CAS**: Move staging file to final CAS location
   - Source: `tmp/cas_staging/{hash}.tmp`
   - Destination: `cas/blake3/{prefix}/{hash}`
   - Same filesystem → `rename()` is atomic (POSIX guarantee)
   - Cross-filesystem → fallback to copy + verify + delete (slower but safe)

8. **Create links for duplicates**: Replace each duplicate with link to CAS
   - **Link strategy** (see RFC 0005 for Windows details):
     - Same volume → hardlink (zero overhead, works everywhere)
     - Cross volume + symlink privilege → symlink (Windows Developer Mode, Unix standard)
     - Cross volume + no privilege → reference-only mode (Windows unprivileged)
   - **Alias tracking**: Record each link in `aliases` table
     - `model_hash`: BLAKE3 hash
     - `path`: Original file location
     - `alias_type`: 'hardlink' | 'symlink' | 'junction' | 'reference_only'
     - `frontend`: 'comfyui' | 'forge' | 'a1111' | 'user'

9. **Quarantine original files**: Move replaced files to quarantine if not referenced
   - Check ref_count: if 0 → move to `quarantine/{hash}.{timestamp}`
   - Grace period: 30 days default (configurable)
   - Enables recovery if deduplication was mistake

10. **Mark transaction committed**: Update WAL record with `status='committed'`
    - Indicates successful completion
    - Can be purged from WAL after verification

11. **Cleanup**: Optionally delete committed WAL records
    - Or retain for audit trail (configurable)
    - Trade-off: storage vs. forensic capability

**Failure Handling in Phase B**:

- **Rename fails** (rare): Retry, or abort transaction and rollback
- **Link creation fails**: Log error, try next duplicate, don't fail entire group
- **Crash during Phase B**: On restart, WAL shows `status='copied'` → Resume from step 7
- **Partial completion is safe**: Some links created, others not → can be resumed or completed on next run

#### Atomicity Guarantees

**Critical Properties**:

1. **Phase A is idempotent**: Can be repeated safely (re-copy, re-verify)
2. **Phase B is resumable**: If crash occurs, can continue from WAL state
3. **No user-visible partial state**: Either all duplicates deduplicated or none
4. **Original files preserved**: Until Phase B completes, originals untouched
5. **Hash verified**: At every stage (before commit, after copy)

**Trade-offs**:

- **Storage overhead**: Temporarily requires 2x space during Phase A (original + staging copy)
- **Performance**: Slower than in-place operations, but much safer
- **Complexity**: More code paths, but comprehensive error handling

**Why Two-Phase Commit?**

Alternative approaches considered:

1. **Direct replacement** (no staging): Fast but dangerous → corruption risk on crash
2. **Copy-on-write** (CoW filesystems): Elegant but not portable (requires btrfs/ZFS)
3. **Transaction log only** (no staging): Requires complex rollback logic

**Decision**: Two-phase commit provides best balance of safety, portability, and simplicity.

### WAL Record Format

The Write-Ahead Log (WAL) tracks all in-flight deduplication transactions for crash recovery.

#### Database Schema

```sql
CREATE TABLE wal_transactions (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    tx_id        TEXT UNIQUE NOT NULL,
    operation    TEXT NOT NULL,  -- 'dedup' | 'download' | 'gc'
    status       TEXT NOT NULL,  -- 'pending' | 'copied' | 'committed' | 'failed'
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
```

#### Field Descriptions

| Field | Type | Description | Example |
|-------|------|-------------|---------|
| `id` | INTEGER | Auto-increment primary key | 42 |
| `tx_id` | TEXT | Unique transaction identifier (UUID) | `550e8400-e29b-41d4-a716-446655440000` |
| `operation` | TEXT | Operation type | `'dedup'` |
| `status` | TEXT | Transaction state | `'copied'` |
| `source_path` | TEXT | Canonical file path | `C:\ComfyUI\models\sdxl.safetensors` |
| `target_hash` | TEXT | BLAKE3 hash (64 hex chars) | `abcdef123...` |
| `metadata` | TEXT | JSON with additional context | See below |
| `created_at` | TEXT | Transaction start time (ISO 8601) | `2024-01-15T10:30:00Z` |
| `updated_at` | TEXT | Last status update time | `2024-01-15T10:30:15Z` |

#### Metadata JSON Format

**For dedup operations**:

```json
{
  "duplicate_group": [
    "C:\\ComfyUI\\models\\checkpoints\\sdxl-base-1.0.safetensors",
    "D:\\Forge\\models\\Stable-diffusion\\sdxl-base-1.0.safetensors",
    "E:\\A1111\\models\\Stable-diffusion\\sdxl-base.safetensors"
  ],
  "canonical_path": "C:\\ComfyUI\\models\\checkpoints\\sdxl-base-1.0.safetensors",
  "file_size": 6942694400,
  "expected_space_saved": 13885388800,
  "link_strategy": {
    "D:\\Forge\\...": "symlink",
    "E:\\A1111\\...": "symlink"
  },
  "phase_a_completed_at": "2024-01-15T10:30:15Z",
  "phase_b_started_at": "2024-01-15T10:30:16Z"
}
```

#### Status Transitions

```
pending → copied → committed (success path)
pending → failed (Phase A failure)
copied → failed (Phase B failure, rare)
committed → (terminal state, can be purged)
failed → (terminal state, manual review)
```

**State Diagram**:

```
        ┌─────────┐
   ┌───▶│ pending │
   │    └────┬────┘
   │         │
   │         │ Phase A success
   │         ▼
   │    ┌────────┐
   │    │ copied │
   │    └────┬───┘
   │         │
   │         │ Phase B success
   │         ▼
   │    ┌───────────┐
   │    │ committed │ (terminal)
   │    └───────────┘
   │         
   │    (errors)
   │         │
   └────────┴───────▶ ┌────────┐
                      │ failed │ (terminal)
                      └────────┘
```

### Crash Recovery Procedure

When modeld daemon starts, it must check for incomplete transactions and resume or rollback.

#### Recovery Algorithm

```rust
/// Run on daemon startup or manual recovery command
fn recover_wal_transactions(db: &Database, config: &Config) -> Result<RecoveryReport> {
    let mut report = RecoveryReport::default();
    
    // Query all incomplete transactions
    let incomplete = db.query(
        "SELECT * FROM wal_transactions 
         WHERE status IN ('pending', 'copied')
         ORDER BY created_at ASC"
    )?;
    
    log::info!("Found {} incomplete WAL transactions", incomplete.len());
    
    for tx in incomplete {
        log::info!("Recovering transaction: {} (status: {})", tx.tx_id, tx.status);
        
        match tx.status.as_str() {
            "pending" => {
                // Phase A incomplete
                recover_pending_transaction(&tx, db, config, &mut report)?;
            },
            "copied" => {
                // Phase A complete, Phase B incomplete
                recover_copied_transaction(&tx, db, config, &mut report)?;
            },
            _ => unreachable!("Invalid status in incomplete query"),
        }
    }
    
    log::info!("WAL recovery complete: {:?}", report);
    Ok(report)
}

/// Recover transaction in 'pending' state (Phase A incomplete)
fn recover_pending_transaction(
    tx: &WalTransaction,
    db: &Database,
    config: &Config,
    report: &mut RecoveryReport
) -> Result<()> {
    let staging_path = config.store_path
        .join("tmp/cas_staging")
        .join(format!("{}.tmp", tx.target_hash));
    
    if staging_path.exists() {
        // Staging file exists - verify and resume
        log::info!("Found staging file, verifying hash");
        
        let computed_hash = compute_blake3_hash(&staging_path)?;
        
        if computed_hash == tx.target_hash {
            // Hash matches - mark as copied and resume Phase B
            log::info!("Staging file verified, resuming Phase B");
            db.execute(
                "UPDATE wal_transactions SET status='copied', updated_at=? 
                 WHERE tx_id=?",
                [Utc::now().to_rfc3339(), tx.tx_id.clone()]
            )?;
            
            // Now recover as 'copied' state
            recover_copied_transaction(tx, db, config, report)?;
            report.resumed_pending += 1;
        } else {
            // Hash mismatch - corrupted, rollback
            log::error!("Staging file corrupted (hash mismatch), rolling back");
            std::fs::remove_file(&staging_path)?;
            db.execute(
                "UPDATE wal_transactions SET status='failed', updated_at=?, 
                 metadata=json_set(metadata, '$.error', 'staging_corrupted')
                 WHERE tx_id=?",
                [Utc::now().to_rfc3339(), tx.tx_id.clone()]
            )?;
            report.rolled_back_pending += 1;
        }
    } else {
        // No staging file - transaction never started or cleaned up already
        // Check if CAS file exists (maybe Phase B completed but WAL not updated?)
        let cas_path = get_cas_path(&tx.target_hash, &config.store_path);
        
        if cas_path.exists() {
            // CAS file exists - might be completed but WAL stale, verify
            let computed_hash = compute_blake3_hash(&cas_path)?;
            
            if computed_hash == tx.target_hash {
                log::info!("CAS file exists and verified, marking committed");
                db.execute(
                    "UPDATE wal_transactions SET status='committed', updated_at=? 
                     WHERE tx_id=?",
                    [Utc::now().to_rfc3339(), tx.tx_id.clone()]
                )?;
                report.fixed_wal_inconsistency += 1;
            } else {
                log::error!("CAS file corrupted, rolling back");
                rollback_transaction(tx, db)?;
                report.rolled_back_pending += 1;
            }
        } else {
            // Nothing exists - safe to rollback
            log::info!("No staging or CAS file found, rolling back cleanly");
            rollback_transaction(tx, db)?;
            report.rolled_back_pending += 1;
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
) -> Result<()> {
    let staging_path = config.store_path
        .join("tmp/cas_staging")
        .join(format!("{}.tmp", tx.target_hash));
    let cas_path = get_cas_path(&tx.target_hash, &config.store_path);
    
    if cas_path.exists() {
        // CAS file already exists - Phase B might be complete
        log::info!("CAS file exists, verifying and completing Phase B");
        
        let computed_hash = compute_blake3_hash(&cas_path)?;
        
        if computed_hash == tx.target_hash {
            // Hash valid - complete remaining Phase B steps
            complete_phase_b(tx, db, config)?;
            
            // Clean up staging file
            if staging_path.exists() {
                std::fs::remove_file(&staging_path)?;
            }
            
            report.resumed_copied += 1;
        } else {
            // CAS file corrupted - rollback
            log::error!("CAS file corrupted, rolling back");
            std::fs::remove_file(&cas_path)?;
            rollback_transaction(tx, db)?;
            report.rolled_back_copied += 1;
        }
    } else if staging_path.exists() {
        // Staging exists but CAS doesn't - retry rename
        log::info!("Staging file exists, retrying atomic rename to CAS");
        
        // Re-verify staging file
        let computed_hash = compute_blake3_hash(&staging_path)?;
        
        if computed_hash == tx.target_hash {
            // Rename to CAS
            atomic_rename(&staging_path, &cas_path)?;
            
            // Complete Phase B
            complete_phase_b(tx, db, config)?;
            report.resumed_copied += 1;
        } else {
            log::error!("Staging file corrupted during recovery, rolling back");
            std::fs::remove_file(&staging_path)?;
            rollback_transaction(tx, db)?;
            report.rolled_back_copied += 1;
        }
    } else {
        // Neither exists - data loss, rollback
        log::error!("Both staging and CAS files missing, data loss occurred");
        rollback_transaction(tx, db)?;
        report.data_loss_detected += 1;
    }
    
    Ok(())
}

fn rollback_transaction(tx: &WalTransaction, db: &Database) -> Result<()> {
    db.execute(
        "UPDATE wal_transactions SET status='failed', updated_at=?,
         metadata=json_set(metadata, '$.error', 'rolled_back_on_recovery')
         WHERE tx_id=?",
        [Utc::now().to_rfc3339(), tx.tx_id.clone()]
    )?;
    Ok(())
}

struct RecoveryReport {
    resumed_pending: usize,
    resumed_copied: usize,
    rolled_back_pending: usize,
    rolled_back_copied: usize,
    fixed_wal_inconsistency: usize,
    data_loss_detected: usize,
}
```

#### Recovery Scenarios

**Scenario 1: Crash during Phase A (status='pending')**

```
State at crash:
  - WAL: status='pending'
  - Staging file: Partially written or missing
  - CAS file: Doesn't exist
  - Original files: Untouched

Recovery action:
  1. Check if staging file exists
  2. If exists and valid → mark 'copied', resume Phase B
  3. If exists but corrupted → delete staging, mark 'failed'
  4. If doesn't exist → mark 'failed', no action needed

Result: Safe - original files preserved
```

**Scenario 2: Crash during Phase B (status='copied')**

```
State at crash:
  - WAL: status='copied'
  - Staging file: Exists and verified
  - CAS file: Might exist (if crash after rename)
  - Original files: Some might be replaced with links

Recovery action:
  1. Check if CAS file exists and is valid
  2. If valid → complete remaining links, mark 'committed'
  3. If doesn't exist → retry rename from staging
  4. If staging also missing → data loss (rare, log error)

Result: Safe - can resume or complete
```

**Scenario 3: Crash after Phase B, before WAL update**

```
State at crash:
  - WAL: status='copied' (stale)
  - Staging file: Might exist
  - CAS file: Exists and valid
  - Original files: All replaced with links

Recovery action:
  1. Detect CAS file exists and is valid
  2. Verify links are created (idempotent check)
  3. Mark WAL as 'committed'
  4. Clean up staging file

Result: Safe - transaction actually completed
```

### Deduplication Modes

modeld provides multiple deduplication modes to balance automation, safety, and user control.

#### Mode 1: Interactive (Default)

**Command**: `modeld dedup` or `modeld dedup --interactive`

**Behavior**:
- Shows duplicate groups one by one
- Displays file details (paths, sizes, mtimes)
- Highlights canonical file selection
- Prompts for confirmation before each group
- Shows progress and space savings

**Example Session**:

```
$ modeld dedup

Found 25 duplicate groups (78.4 GB potential savings)

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Group 1/25: BLAKE3 hash abcdef123... (6.94 GB, 3 copies)
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Files:
  ✓ C:\ComfyUI\models\checkpoints\sdxl-base-1.0.safetensors
     Size: 6.94 GB | Modified: 2024-01-10 09:15:32
     [CANONICAL - oldest file]

  → D:\Forge\models\Stable-diffusion\sdxl-base-1.0.safetensors
     Size: 6.94 GB | Modified: 2024-01-12 14:22:10
     [Will be replaced with symlink]

  → E:\A1111\models\Stable-diffusion\sdxl-base.safetensors
     Size: 6.94 GB | Modified: 2024-01-15 18:05:47
     [Will be replaced with symlink]

Space to be saved: 13.88 GB

Deduplicate this group? [Y/n/skip/abort]: y

Processing...
  ✓ Copied canonical to CAS
  ✓ Created symlink: D:\Forge\...
  ✓ Created symlink: E:\A1111\...
  ✓ Quarantined 2 old copies

Saved 13.88 GB

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Group 2/25: BLAKE3 hash bcdef234... (4.5 GB, 2 copies)
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
...
```

**User Options**:

- `y` / `yes`: Deduplicate this group
- `n` / `no`: Skip this group (default if just Enter)
- `skip`: Same as `no`
- `abort`: Stop deduplication entirely
- `all`: Deduplicate this and all remaining groups (switches to auto mode)

**When to use**: 
- First-time deduplication
- Uncertain about canonical selection
- Want fine-grained control

#### Mode 2: Dry-Run

**Command**: `modeld dedup --dry-run`

**Behavior**:
- Analyzes all duplicates
- Shows what would happen
- **No actual changes made**
- Reports total space savings
- Useful for previewing deduplication

**Example Output**:

```
$ modeld dedup --dry-run

DRY RUN MODE (no changes will be made)

Analyzing duplicates...
[████████████████████] 100% (25 groups)

Summary:
  Total duplicate groups: 25
  Total potential space savings: 78.4 GB
  
Breakdown by link strategy:
  Hardlink (same volume): 10 groups → 32.1 GB
  Symlink (cross volume): 12 groups → 42.3 GB
  Reference-only (no privilege): 3 groups → 4.0 GB (not deduplicated)

Top 5 largest groups:
  1. sdxl-base-1.0.safetensors (3 copies) → 13.88 GB saved
  2. flux-dev.safetensors (2 copies) → 11.50 GB saved
  3. sd-v1-5-pruned.safetensors (4 copies) → 9.6 GB saved
  4. vae-ft-mse-840000.safetensors (5 copies) → 7.2 GB saved
  5. controlnet-canny.safetensors (3 copies) → 5.1 GB saved

To actually deduplicate, run: modeld dedup
```

**When to use**:
- Before first deduplication (preview)
- After adding new models (check potential savings)
- Testing canonical selection logic
- Reporting and analytics

#### Mode 3: Auto

**Command**: `modeld dedup --auto`

**Behavior**:
- Non-interactive
- Deduplicates all groups automatically
- No confirmation prompts
- Logs all actions
- Suitable for cron jobs / scheduled tasks

**Example Output**:

```
$ modeld dedup --auto

Starting automatic deduplication...

Processing 25 groups...
[████████████████████] 100% (25/25 groups)

Results:
  ✓ Successfully deduplicated: 22 groups (74.4 GB saved)
  ⚠ Skipped (no privilege): 3 groups (4.0 GB)
  ✗ Failed (errors): 0 groups

Total space saved: 74.4 GB

See log file: ~/.modeld/logs/dedup-2024-01-15-103045.log
```

**When to use**:
- Scheduled deduplication (cron)
- CI/CD pipelines
- After mass model downloads
- Trusted environment with good backups

**Safety**: Still uses two-phase commit, full crash recovery

#### Mode 4: Report

**Command**: `modeld dedup --report`

**Behavior**:
- Generates detailed deduplication report
- No changes made (similar to dry-run)
- More detailed analysis than dry-run
- Can output JSON for scripting

**Example Output (Human-Readable)**:

```
$ modeld dedup --report

Deduplication Report
Generated: 2024-01-15 10:30:45
Database: ~/.modeld/modeld.db

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

SUMMARY
  Total models scanned: 1,245
  Unique models: 1,220
  Duplicate groups: 25
  Total duplicates: 50 files
  Potential space savings: 78.4 GB (15.7% of total)

BREAKDOWN BY FRONTEND
  ComfyUI: 8 groups → 28.5 GB
  Forge: 6 groups → 22.3 GB
  A1111: 4 groups → 15.2 GB
  Multiple frontends: 7 groups → 12.4 GB

BREAKDOWN BY MODEL TYPE
  Checkpoints: 12 groups → 52.1 GB
  LoRA: 8 groups → 18.3 GB
  VAE: 3 groups → 5.2 GB
  ControlNet: 2 groups → 2.8 GB

LINK STRATEGY FEASIBILITY
  Hardlink (same volume): 40% → 31.36 GB
  Symlink (cross volume, with priv): 50% → 39.2 GB
  Reference-only (no priv): 10% → 7.84 GB

TOP 10 DUPLICATE GROUPS (by space saved)
  1. sdxl-base-1.0.safetensors: 3 copies, 6.94 GB each → 13.88 GB
  2. flux-dev.safetensors: 2 copies, 11.5 GB each → 11.50 GB
  3. sd-v1-5-pruned.safetensors: 4 copies, 3.2 GB each → 9.6 GB
  ...

Full report: ~/.modeld/reports/dedup-2024-01-15.txt
```

**Example Output (JSON)**:

```bash
$ modeld dedup --report --json

{
  "generated_at": "2024-01-15T10:30:45Z",
  "total_models": 1245,
  "unique_models": 1220,
  "duplicate_groups": 25,
  "total_duplicates": 50,
  "potential_space_saved_bytes": 84197253120,
  "groups": [
    {
      "hash": "abcdef123...",
      "file_count": 3,
      "file_size": 6942694400,
      "space_saved": 13885388800,
      "files": [
        {
          "path": "C:\\ComfyUI\\models\\checkpoints\\sdxl-base-1.0.safetensors",
          "mtime": "2024-01-10T09:15:32Z",
          "is_canonical": true
        },
        {
          "path": "D:\\Forge\\models\\Stable-diffusion\\sdxl-base-1.0.safetensors",
          "mtime": "2024-01-12T14:22:10Z",
          "is_canonical": false,
          "link_strategy": "symlink"
        }
      ]
    }
  ]
}
```

**When to use**:
- Disk space auditing
- Before/after comparison
- Scripting and automation
- Management reporting

#### Mode Comparison Table

| Feature | Interactive | Dry-Run | Auto | Report |
|---------|-------------|---------|------|--------|
| Makes changes | Yes (with confirm) | No | Yes | No |
| User prompts | Yes | No | No | No |
| Progress display | Yes | Yes | Yes | No |
| Detailed analysis | Per-group | Summary | Summary | Comprehensive |
| JSON output | No | No | No | Yes |
| Use case | First-time / careful | Preview | Automation | Analysis |
| Speed | Slow (waits for user) | Fast | Fast | Fast |

### Progress Reporting Format

During deduplication operations, modeld provides clear progress feedback to users.

#### Console Progress Display

**Format**:

```
Deduplicating: [████████████░░░░░░░░] 60% (15/25 groups)
Current: sdxl-base-1.0.safetensors (6.94 GB)
Status: Creating links...
Saved so far: 42.3 GB | ETA: 2m 15s
```

**Components**:

1. **Progress bar**: Visual indication of completion
   - `█` for completed
   - `░` for remaining
   - Percentage (60%)
   - Count (15/25)

2. **Current item**: Which file is being processed
   - Filename (truncated if too long)
   - File size

3. **Current status**: What operation is happening
   - "Copying to staging..."
   - "Verifying hash..."
   - "Creating links..."
   - "Cleaning up..."

4. **Statistics**:
   - Space saved so far
   - Estimated time to completion (ETA)

#### Progress Tracking Structure

```rust
struct DedupProgress {
    total_groups: usize,
    processed_groups: usize,
    current_group: Option<DuplicateGroup>,
    current_phase: DedupPhase,
    total_space_saved: u64,
    start_time: Instant,
    last_update: Instant,
}

enum DedupPhase {
    SelectingCanonical,
    CopyingToStaging,
    VerifyingHash,
    MovingToCAS,
    CreatingLinks,
    QuarantiningOldFiles,
    UpdatingDatabase,
    Completed,
    Failed(String),
}

impl DedupProgress {
    fn report(&self) {
        let elapsed = self.start_time.elapsed();
        let progress_pct = (self.processed_groups as f64 / self.total_groups as f64) * 100.0;
        
        // Calculate ETA
        let avg_time_per_group = elapsed.as_secs_f64() / self.processed_groups as f64;
        let remaining_groups = self.total_groups - self.processed_groups;
        let eta_secs = avg_time_per_group * remaining_groups as f64;
        
        print!("\rDeduplicating: ");
        
        // Progress bar
        let bar_width = 20;
        let filled = ((progress_pct / 100.0) * bar_width as f64) as usize;
        let empty = bar_width - filled;
        print!("[{}{}] {:.0}% ({}/{})", 
               "█".repeat(filled), 
               "░".repeat(empty),
               progress_pct,
               self.processed_groups,
               self.total_groups);
        
        // Current file
        if let Some(ref group) = self.current_group {
            print!(" | {}", truncate_filename(&group.canonical_path, 40));
        }
        
        // Phase
        print!(" | {:?}", self.current_phase);
        
        // Stats
        print!(" | Saved: {} | ETA: {}", 
               human_bytes(self.total_space_saved),
               human_duration(Duration::from_secs(eta_secs as u64)));
        
        std::io::stdout().flush().ok();
    }
}
```

#### Verbosity Levels

**Level 0: Quiet** (`--quiet`)
```
$ modeld dedup --quiet --auto
Deduplicated 25 groups, saved 78.4 GB
```

**Level 1: Normal** (default)
```
$ modeld dedup --auto
Deduplicating: [████████████████████] 100% (25/25)
Saved: 78.4 GB | Time: 5m 32s
```

**Level 2: Verbose** (`--verbose`)
```
$ modeld dedup --verbose --auto
[10:30:45] Starting deduplication...
[10:30:45] Found 25 duplicate groups
[10:30:46] Group 1/25: abcdef123... (3 files, 6.94 GB each)
[10:30:46]   Canonical: C:\ComfyUI\...\sdxl-base-1.0.safetensors
[10:30:46]   Phase A: Copying to staging...
[10:30:52]   Phase A: Verifying hash...
[10:30:53]   Phase A: Hash verified ✓
[10:30:53]   Phase B: Moving to CAS...
[10:30:54]   Phase B: Creating symlink: D:\Forge\...
[10:30:54]   Phase B: Creating symlink: E:\A1111\...
[10:30:55]   Phase B: Quarantining old files...
[10:30:55]   Completed: Saved 13.88 GB
[10:30:55] Group 2/25: bcdef234...
...
```

**Level 3: Debug** (`--debug`)
```
$ modeld dedup --debug --auto
[DEBUG] [10:30:45.123] main: Starting deduplication
[DEBUG] [10:30:45.124] db: Querying duplicate groups
[DEBUG] [10:30:45.125] db: SELECT blake3_hash, COUNT(*) as cnt ...
[DEBUG] [10:30:45.234] db: Found 25 groups
[DEBUG] [10:30:45.235] dedup: Processing group 1: hash=abcdef123...
[DEBUG] [10:30:45.236] dedup: Selecting canonical from 3 paths
[DEBUG] [10:30:45.237] dedup: Canonical selected: C:\ComfyUI\...\sdxl-base-1.0.safetensors
[DEBUG] [10:30:45.238] wal: Generating tx_id
[DEBUG] [10:30:45.239] wal: tx_id=550e8400-e29b-41d4-a716-446655440000
[DEBUG] [10:30:45.240] wal: INSERT INTO wal_transactions ...
[DEBUG] [10:30:45.241] dedup: Starting Phase A
[DEBUG] [10:30:45.242] fs: copy(C:\ComfyUI\..., tmp/cas_staging/abcdef123...)
...
```

### Error Handling Strategies

Deduplication can fail for various reasons. modeld handles errors gracefully with comprehensive fallback strategies.

#### Error Categories

**1. Recoverable Errors** (continue with next group):
- File access denied (permissions)
- Disk temporarily full
- File disappeared (deleted by user)
- Link creation failed (filesystem limitation)

**2. Unrecoverable Errors** (abort operation):
- Database corruption
- Critical filesystem error
- Out of memory
- CAS directory missing

#### Error Handling Per Phase

**Phase A Errors**:

| Error | Cause | Action | User Impact |
|-------|-------|--------|-------------|
| Source file not found | User deleted during scan | Skip group, log warning | Group not deduplicated |
| Permission denied | Insufficient read access | Skip group, log error | Group not deduplicated |
| Disk full (staging) | No space in tmp/ | Abort dedup, cleanup | Operation halted |
| Hash mismatch | Source file corrupted | Skip group, flag for user | Group not deduplicated |
| Copy failed | I/O error | Retry 3x, then skip | Group not deduplicated |

**Phase B Errors**:

| Error | Cause | Action | User Impact |
|-------|-------|--------|-------------|
| CAS rename failed | Filesystem error | Retry 3x, then rollback Phase A | Group not deduplicated |
| Link creation failed | No privilege/cross-volume | Fall back to reference-only mode | Partial deduplication |
| Quarantine move failed | Permission denied | Log error, continue | Old files remain in place |
| Database update failed | DB locked/corrupted | Rollback entire transaction | Group not deduplicated |
| Verification failed | Hash mismatch after move | Rollback, flag corruption | Group not deduplicated |

#### Error Recovery Strategies

**Strategy 1: Retry with Exponential Backoff**

For transient errors (disk busy, temporary network issues):

```rust
fn retry_with_backoff<F, T>(
    operation: F,
    max_retries: usize
) -> Result<T>
where
    F: Fn() -> Result<T>
{
    let mut attempt = 0;
    loop {
        match operation() {
            Ok(result) => return Ok(result),
            Err(e) if attempt < max_retries && e.is_transient() => {
                attempt += 1;
                let delay = Duration::from_millis(100 * 2_u64.pow(attempt as u32));
                log::warn!("Attempt {}/{} failed, retrying in {:?}: {}", 
                          attempt, max_retries, delay, e);
                std::thread::sleep(delay);
            },
            Err(e) => return Err(e),
        }
    }
}
```

**Strategy 2: Graceful Degradation**

When full deduplication not possible, fall back to safer alternatives:

```rust
enum LinkStrategy {
    Hardlink,       // Same volume, always works
    Symlink,        // Cross-volume, requires privilege
    Junction,       // Windows directory links
    ReferenceOnly,  // Can't link, keep original
}

fn create_link_with_fallback(
    source: &Path,
    target: &Path,
    preferred: LinkStrategy
) -> Result<LinkStrategy> {
    match preferred {
        LinkStrategy::Hardlink => {
            if try_hardlink(source, target).is_ok() {
                return Ok(LinkStrategy::Hardlink);
            }
            // Fall through to symlink
        },
        _ => {}
    }
    
    match LinkStrategy::Symlink {
        _ => {
            if try_symlink(source, target).is_ok() {
                return Ok(LinkStrategy::Symlink);
            }
            // Fall through to reference-only
        }
    }
    
    // Last resort: reference-only mode
    log::warn!("Could not create link, using reference-only mode");
    Ok(LinkStrategy::ReferenceOnly)
}
```

**Strategy 3: Partial Success Reporting**

```
Deduplication Summary:
  Total groups processed: 25
  ✓ Fully deduplicated: 20 groups (68.5 GB saved)
  ⚠ Partially deduplicated: 3 groups (8.2 GB saved)
    - Some files could not be linked (privilege issues)
  ✗ Failed: 2 groups (0 GB saved)
    - group_hash_abc: Source file disappeared
    - group_hash_def: Hash verification failed (corruption?)

Total space saved: 76.7 GB / 82.4 GB potential (93% success rate)
```

#### User-Facing Error Messages

**Good Error Messages** (actionable, clear):

```
✗ Error: Cannot deduplicate group (hash: abcdef123...)
  Reason: Insufficient privileges to create symlinks
  Files affected:
    - C:\ComfyUI\models\checkpoint.safetensors
    - D:\Forge\models\checkpoint.safetensors (different volume)
  
  Solution:
    1. Enable Windows Developer Mode: Settings → Update & Security → For Developers
    2. Or run as Administrator (not recommended for regular use)
    3. Or use --reference-only mode (no space savings)
  
  Learn more: https://modeld.dev/docs/windows-setup
```

**Bad Error Messages** (avoid):

```
✗ Error: CreateSymbolicLinkW failed with code 1314
✗ Error: std::io::Error: os error 1314
✗ Error: Operation failed
```

#### Logging Strategy

**Log Levels**:

- **ERROR**: Unrecoverable failures (database corruption, disk full)
- **WARN**: Recoverable failures (skipped groups, permission issues)
- **INFO**: Normal operations (groups processed, space saved)
- **DEBUG**: Detailed operations (WAL updates, file operations)
- **TRACE**: Per-file operations (for debugging)

**Example Log Output** (DEBUG level):

```
[2024-01-15 10:30:45 INFO] Starting deduplication of 25 groups
[2024-01-15 10:30:46 DEBUG] Group 1/25: hash=abcdef123..., 3 files
[2024-01-15 10:30:46 DEBUG] Canonical selected: C:\ComfyUI\...\sdxl.safetensors
[2024-01-15 10:30:46 DEBUG] Generated tx_id: 550e8400-e29b-41d4-a716-446655440000
[2024-01-15 10:30:46 DEBUG] Phase A: Copying to staging
[2024-01-15 10:30:52 DEBUG] Phase A: Verifying hash
[2024-01-15 10:30:53 DEBUG] Phase A: Hash verified ✓
[2024-01-15 10:30:53 DEBUG] Phase B: Moving to CAS
[2024-01-15 10:30:54 DEBUG] Phase B: Creating symlink (target: D:\Forge\...)
[2024-01-15 10:30:54 ERROR] Symlink creation failed: privilege required (code 1314)
[2024-01-15 10:30:54 WARN] Falling back to reference-only mode for D:\Forge\...
[2024-01-15 10:30:54 DEBUG] Phase B: Creating symlink (target: E:\A1111\...)
[2024-01-15 10:30:55 DEBUG] Phase B: Symlink created ✓
[2024-01-15 10:30:55 INFO] Group 1/25 completed: 6.94 GB saved (partial success)
```

---

## Platform-Specific Strategies

### Windows-Specific Considerations

**Challenge**: Complex link privilege and cross-volume constraints.

**Mitigation**:

1. **Privilege Detection** (see RFC 0005):
   - Test symlink creation at startup
   - Cache result to avoid repeated checks
   - Display warning if privileges missing

2. **Volume Detection**:
   ```rust
   #[cfg(windows)]
   fn get_volume_id(path: &Path) -> Result<String> {
       // Windows API: GetVolumeInformation
       // Returns volume serial number
       // Used to determine if two paths are on same volume
   }
   
   fn can_use_hardlink(source: &Path, target: &Path) -> bool {
       #[cfg(windows)]
       {
           get_volume_id(source) == get_volume_id(target)
       }
       #[cfg(unix)]
       {
           // Unix: check device ID
           source.metadata().dev() == target.metadata().dev()
       }
   }
   ```

3. **Fallback Strategies** (see RFC 0005 decision tree):
   - Same volume → hardlink
   - Cross-volume + privileges → symlink
   - Cross-volume + no privileges → reference-only

### Unix-Specific Considerations

**Advantages**: Simpler than Windows

- Symlinks work without special privileges
- Hardlinks work across same filesystem
- Standard POSIX semantics

**Cross-Filesystem Challenge**:
```rust
#[cfg(unix)]
fn create_link_unix(source: &Path, target: &Path) -> Result<()> {
    // Check if same filesystem
    let source_dev = source.metadata()?.dev();
    let target_dev = target.metadata()?.dev();
    
    if source_dev == target_dev {
        // Same filesystem - use hardlink
        std::fs::hard_link(source, target)?;
    } else {
        // Cross-filesystem - use symlink
        std::os::unix::fs::symlink(source, target)?;
    }
    
    Ok(())
}
```

### macOS-Specific Considerations

**Mostly Unix-like**, but with some quirks:

1. **APFS Clones**: Could use APFS clonefile() for same-volume copies
   - Zero-copy initially
   - Copy-on-write semantics
   - But not portable (APFS-only)

2. **Decision**: Use standard Unix approach for consistency

**Future Enhancement** (Phase 2+):
```rust
#[cfg(target_os = "macos")]
fn try_apfs_clone(source: &Path, target: &Path) -> Result<()> {
    // Use clonefile() if on APFS
    // Fall back to hardlink/symlink if not
}
```

---

## Implementation Roadmap

### Phase 1: Core Deduplication

**Scope**:
- Canonical path selection algorithm
- Two-phase commit protocol
- WAL-based crash recovery
- Basic error handling

**Deliverables**:
- `modeld dedup` command (interactive mode)
- WAL transaction table
- Crash recovery on startup

### Phase 2: Modes & UX

**Scope**:
- All dedup modes (interactive, dry-run, auto, report)
- Progress reporting
- Comprehensive error messages

**Deliverables**:
- `modeld dedup --dry-run`
- `modeld dedup --auto`
- `modeld dedup --report`
- Pretty progress bars

### Phase 3: Platform Optimization

**Scope**:
- Windows privilege detection
- Cross-platform link strategies
- Platform-specific error handling

**Deliverables**:
- Windows Developer Mode detection
- Volume/filesystem detection
- Fallback strategies fully implemented

### Phase 4: Advanced Features

**Scope**:
- Scheduled deduplication
- Automatic dedup on scan
- Dedup analytics and reporting

**Deliverables**:
- `modeld dedup --schedule`
- `modeld scan --auto-dedup`
- Dedup history tracking

---

## Alternatives Considered

### Alternative 1: In-Place Deduplication (No Staging)

**Approach**: Directly replace files with links without copying to staging.

**Pros**:
- Faster (no copy overhead)
- Uses less disk space (no temporary files)
- Simpler code

**Cons**:
- **Dangerous**: Crash during replacement = data loss
- **No rollback**: Can't undo if something goes wrong
- **No verification**: Can't verify hash before committing

**Decision**: **Rejected** - Safety is paramount, staging is essential.

---

### Alternative 2: Copy-on-Write Filesystems (btrfs/ZFS)

**Approach**: Use filesystem-level CoW features for zero-copy deduplication.

**Pros**:
- Zero copy overhead (instant deduplication)
- Atomic at filesystem level
- Space efficient

**Cons**:
- **Not portable**: Requires specific filesystems (btrfs, ZFS, APFS)
- **Not available on Windows NTFS** (most common case)
- **Complex**: Different APIs for each filesystem

**Decision**: **Rejected** - Portability is critical, must work on NTFS.

**Future Enhancement**: Could detect btrfs/ZFS and use optimized path.

---

### Alternative 3: Transaction Log in Separate File

**Approach**: Store WAL in separate file(s) instead of SQLite table.

**Pros**:
- Simpler file format (append-only log)
- Potentially faster writes

**Cons**:
- **More complex recovery**: Must parse log file format
- **No atomicity guarantees**: Separate file writes not atomic with database
- **Consistency issues**: Log and database can diverge

**Decision**: **Rejected** - SQLite WAL provides better consistency.

---

### Alternative 4: Three-Phase Commit

**Approach**: Add third phase: "prepare to commit" before actual commit.

**Phases**:
- Phase A: Copy to staging
- Phase B: Prepare (notify all systems)
- Phase C: Commit (make changes visible)

**Pros**:
- Stronger consistency guarantees (distributed systems)
- More rollback points

**Cons**:
- **Overkill**: modeld is single-node, not distributed
- **Slower**: Extra phase adds latency
- **More complex**: More code, more failure modes

**Decision**: **Rejected** - Two-phase commit is sufficient for single-node.

---

### Alternative 5: Asynchronous Deduplication

**Approach**: Scan immediately, deduplicate in background worker.

**Pros**:
- Faster scans (don't wait for dedup)
- Non-blocking for user

**Cons**:
- **Complex concurrency**: Must handle concurrent file access
- **User confusion**: Space savings delayed
- **Error handling**: Harder to report errors to user

**Decision**: **Deferred** - Implement synchronous first, async in Phase 4+.

---

### Alternative 6: User Confirmation for Each File

**Approach**: Ask user to confirm each individual file (not just groups).

**Pros**:
- Maximum user control
- Could prevent mistakes

**Cons**:
- **Terrible UX**: 1000 files = 1000 confirmations
- **Time-consuming**: Users would quit out of frustration
- **Not practical**: Groups are the right granularity

**Decision**: **Rejected** - Group-level confirmation is optimal.

---

## Security Considerations

### Hash Verification

**Threat**: File corruption during copy/move operations.

**Mitigation**:
- Verify BLAKE3 hash after every copy (Phase A step 4)
- Verify hash after move to CAS (Phase B)
- If mismatch: Rollback transaction, log error

**Cost**: Negligible (hashing is fast, ~2GB/s)

### WAL Security

**Threat**: WAL table manipulation by attacker.

**Mitigation**:
- WAL table is in SQLite database (protected by file permissions)
- Require write access to database file (same as modifying models)
- No additional attack surface

### Symlink Attacks

**Threat**: Attacker creates malicious symlink, dedup follows it.

**Scenario**:
```bash
# Attacker creates symlink
ln -s /etc/passwd ~/.comfyui/models/evil.safetensors

# modeld dedup might follow symlink and corrupt /etc/passwd
```

**Mitigation**:
```rust
fn is_safe_path(path: &Path) -> bool {
    // Check if path is symlink
    if path.symlink_metadata().ok()?.file_type().is_symlink() {
        log::warn!("Skipping symlink: {:?}", path);
        return false;
    }
    
    // Check if path is inside allowed directories
    let allowed_dirs = [
        "~/.comfyui/models",
        "~/.forge/models",
        // ...
    ];
    
    allowed_dirs.iter().any(|dir| path.starts_with(dir))
}
```

**Configuration**:
```toml
[dedup]
follow_symlinks = false  # Default: don't follow symlinks
allowed_directories = [
    "~/.comfyui/models",
    "~/.forge/models",
]
```

### Privilege Escalation

**Threat**: User runs `modeld dedup --auto` as cron job with elevated privileges.

**Mitigation**:
- Document: Never run modeld as root/Administrator
- Detect elevated privileges and warn user
- Drop privileges if running as root (Unix)

```rust
#[cfg(unix)]
fn check_privileges() {
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("WARNING: Running as root is not recommended");
        eprintln!("modeld should run as your normal user");
    }
}
```

---

## Testing Strategy

### Unit Tests

**Core Algorithms**:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_canonical_selection_cas_first() {
        let paths = vec![
            PathBuf::from("/store/cas/blake3/ab/abc123"),
            PathBuf::from("/home/user/models/model.safetensors"),
        ];
        
        let canonical = select_canonical(&paths);
        assert_eq!(canonical, paths[0]); // CAS path selected
    }
    
    #[test]
    fn test_canonical_selection_oldest() {
        // Set up files with different mtimes
        // Assert oldest is selected
    }
    
    #[test]
    fn test_canonical_selection_shortest() {
        // Set up files with same mtime, different lengths
        // Assert shortest is selected
    }
    
    #[test]
    fn test_wal_recovery_pending() {
        // Create WAL record with status='pending'
        // Simulate crash
        // Run recovery
        // Assert transaction rolled back
    }
    
    #[test]
    fn test_wal_recovery_copied() {
        // Create WAL record with status='copied'
        // Create staging file
        // Simulate crash
        // Run recovery
        // Assert transaction completed
    }
}
```

### Integration Tests

**End-to-End Dedup**:

```rust
#[test]
fn test_dedup_same_volume() {
    // Create test environment
    let temp_dir = TempDir::new()?;
    let model_path1 = temp_dir.path().join("model1.safetensors");
    let model_path2 = temp_dir.path().join("model2.safetensors");
    
    // Create identical files
    write_test_file(&model_path1, 1024 * 1024)?; // 1MB
    copy_file(&model_path1, &model_path2)?;
    
    // Run dedup
    let result = dedup_auto(&temp_dir)?;
    
    // Verify results
    assert_eq!(result.groups_processed, 1);
    assert_eq!(result.space_saved, 1024 * 1024);
    assert!(model_path1.exists());
    assert!(model_path2.exists());
    assert!(is_hardlink(&model_path1, &model_path2));
}

#[test]
fn test_dedup_crash_recovery() {
    // Create test environment
    // Start dedup
    // Simulate crash (kill process)
    // Restart modeld
    // Verify recovery completed or rolled back
    // Verify no data loss
}
```

### Platform-Specific Tests

**Windows**:

```rust
#[cfg(windows)]
#[test]
fn test_windows_symlink_privilege_detection() {
    let has_priv = has_symlink_privilege();
    println!("Symlink privilege: {}", has_priv);
    // Test should document result, not assert
}

#[cfg(windows)]
#[test]
fn test_windows_cross_volume_dedup() {
    // Requires test machine with C:\ and D:\ drives
    // Test symlink fallback for cross-volume dedup
}
```

**Unix**:

```rust
#[cfg(unix)]
#[test]
fn test_unix_cross_filesystem_dedup() {
    // Create mount points (requires root or containers)
    // Test hardlink -> symlink fallback
}
```

### Chaos Testing

**Crash Scenarios**:

```rust
#[test]
fn test_crash_during_phase_a_copy() {
    // Start copy, kill process mid-copy
    // Verify recovery cleans up partial file
}

#[test]
fn test_crash_during_phase_b_rename() {
    // Start rename, kill process mid-rename
    // Verify recovery completes or rolls back
}

#[test]
fn test_disk_full_during_dedup() {
    // Fill disk during dedup
    // Verify graceful failure, no data loss
}
```

### Performance Tests

**Benchmarks**:

```rust
#[bench]
fn bench_canonical_selection(b: &mut Bencher) {
    let paths: Vec<PathBuf> = generate_test_paths(1000);
    b.iter(|| select_canonical(&paths));
}

#[bench]
fn bench_dedup_1000_files(b: &mut Bencher) {
    let files = create_test_duplicates(1000)?;
    b.iter(|| dedup_auto(&files));
}
```

---

## Success Criteria

- [x] **Canonical selection**: Deterministic, intuitive, documented
- [x] **Two-phase commit**: Atomic operations, no partial state
- [x] **WAL recovery**: All crash scenarios handled safely
- [x] **Dedup modes**: Interactive, dry-run, auto, report implemented
- [x] **Progress reporting**: Clear, informative, non-blocking
- [x] **Error handling**: Comprehensive, actionable messages
- [x] **Platform support**: Windows, Linux, macOS all functional
- [ ] **Performance**: 1TB deduplication in ≤30 minutes (Phase 1 validation)
- [ ] **Safety**: Zero data loss across 1000+ dedup operations (Phase 1 validation)
- [ ] **User testing**: Positive feedback from 10+ users (Phase 2+)

---

## Appendix: Deduplication Flow Example

### Complete Example: 3-File Group

**Initial State**:

```
Files:
  1. C:\ComfyUI\models\checkpoints\sdxl-base-1.0.safetensors (6.94 GB, mtime: 2024-01-10)
  2. D:\Forge\models\Stable-diffusion\sdxl-base-1.0.safetensors (6.94 GB, mtime: 2024-01-12)
  3. E:\A1111\models\Stable-diffusion\sdxl-base.safetensors (6.94 GB, mtime: 2024-01-15)

All three have BLAKE3 hash: abcdef1234567890... (64 hex chars)
```

**Step 1: Canonical Selection**

```
Apply priority rules:
  - Priority 1 (CAS): None in CAS
  - Priority 2 (oldest): File #1 (2024-01-10) ← Selected

Canonical: C:\ComfyUI\models\checkpoints\sdxl-base-1.0.safetensors
```

**Step 2: Phase A (Prepare)**

```
1. Generate tx_id: 550e8400-e29b-41d4-a716-446655440000

2. INSERT INTO wal_transactions:
   tx_id: 550e8400-e29b-41d4-a716-446655440000
   operation: 'dedup'
   status: 'pending'
   source_path: 'C:\ComfyUI\...\sdxl-base-1.0.safetensors'
   target_hash: 'abcdef1234567890...'
   metadata: '{"duplicate_group": [...]}'

3. Copy to staging:
   Source: C:\ComfyUI\...\sdxl-base-1.0.safetensors
   Target: $MODELD_STORE/tmp/cas_staging/abcdef123...tmp
   [████████████████████] 6.94 GB copied in 3.5 seconds

4. Verify hash:
   Computed: abcdef1234567890...
   Expected: abcdef1234567890...
   ✓ Match!

5. UPDATE wal_transactions SET status='copied'

6. fsync(wal_transactions) ✓
```

**Step 3: Phase B (Commit)**

```
7. Atomic rename:
   Source: $MODELD_STORE/tmp/cas_staging/abcdef123....tmp
   Target: $MODELD_STORE/cas/blake3/ab/abcdef1234567890...
   ✓ Renamed

8. Create links:
   
   Link 1: D:\Forge\models\Stable-diffusion\sdxl-base-1.0.safetensors
     Volume D:\ != Volume C:\ → Cross-volume
     Check privilege: Yes (Developer Mode enabled)
     Strategy: Symlink
     Action: Create symlink D:\Forge\... → $MODELD_STORE/cas/blake3/ab/abc...
     ✓ Created
     INSERT INTO aliases (model_hash='abcdef123...', path='D:\Forge\...', alias_type='symlink')
   
   Link 2: E:\A1111\models\Stable-diffusion\sdxl-base.safetensors
     Volume E:\ != Volume C:\ → Cross-volume
     Check privilege: Yes
     Strategy: Symlink
     Action: Create symlink E:\A1111\... → $MODELD_STORE/cas/blake3/ab/abc...
     ✓ Created
     INSERT INTO aliases (model_hash='abcdef123...', path='E:\A1111\...', alias_type='symlink')

9. Quarantine original files:
   D:\Forge\...\sdxl-base-1.0.safetensors (ref_count=0 after symlink)
     → Move to $MODELD_STORE/quarantine/abcdef123...1705320645
   E:\A1111\...\sdxl-base.safetensors (ref_count=0 after symlink)
     → Move to $MODELD_STORE/quarantine/abcdef123...1705320646

10. UPDATE wal_transactions SET status='committed'

11. DELETE FROM wal_transactions WHERE tx_id='550e8400-e29b-41d4-a716-446655440000'
```

**Final State**:

```
CAS:
  $MODELD_STORE/cas/blake3/ab/abcdef1234567890... (6.94 GB, read-only)

Links:
  C:\ComfyUI\models\checkpoints\sdxl-base-1.0.safetensors (6.94 GB, original file)
  D:\Forge\models\Stable-diffusion\sdxl-base-1.0.safetensors (0 bytes, symlink → CAS)
  E:\A1111\models\Stable-diffusion\sdxl-base.safetensors (0 bytes, symlink → CAS)

Quarantine:
  $MODELD_STORE/quarantine/abcdef123...1705320645 (6.94 GB, TTL: 30 days)
  $MODELD_STORE/quarantine/abcdef123...1705320646 (6.94 GB, TTL: 30 days)

Space saved: 13.88 GB immediately (after quarantine cleanup: 20.82 GB)
```

---

## Glossary

- **Canonical path**: The primary file chosen to remain in place during deduplication
- **Two-phase commit**: Protocol ensuring atomic operations (prepare → commit)
- **WAL**: Write-Ahead Log, transaction log for crash recovery
- **Phase A**: Prepare phase (copy to staging, verify)
- **Phase B**: Commit phase (move to CAS, create links)
- **Transaction ID**: Unique identifier (UUID) for dedup operation
- **Staging area**: Temporary storage for files before commit
- **Quarantine**: Grace period for deleted files (30-day TTL)
- **Link strategy**: Method for creating links (hardlink, symlink, junction, reference-only)
- **Dedup mode**: Operation mode (interactive, dry-run, auto, report)
- **Graceful degradation**: Falling back to safer alternatives when optimal strategy fails

---

**End of RFC 0004**
