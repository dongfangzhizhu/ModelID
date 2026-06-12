# RFC 0003: Reference Model

**Status**: Draft  
**Author**: modeld Architecture Team  
**Created**: 2024  
**Last Updated**: 2024

## Abstract

This RFC defines the reference tracking system for modeld that prevents accidental deletion of AI models by tracking both explicit references (workflow dependencies, user tags) and implicit references (filesystem aliases via hardlinks/symlinks). The design includes a reference counting algorithm, garbage collection protection levels (PROTECTED, QUARANTINE, DELETED), a 30-day quarantine mechanism with time-to-live (TTL), safe garbage collection procedures, and recovery mechanisms to restore accidentally deleted models.

## Motivation

Content-addressable storage systems face a critical challenge: determining when objects are safe to delete. Unlike traditional filesystems where files have clear ownership, CAS objects may be referenced by:

- **Multiple AI workflows** (ComfyUI, Forge, A1111 workflows using same model)
- **Multiple filesystem locations** (hardlinks/symlinks in different directories)
- **Downloaded but not yet integrated** (fresh HuggingFace downloads)
- **User favorites** (models user wants to keep regardless of usage)

Without proper reference tracking, modeld could:
1. **Delete models still in use** → Break user workflows (critical failure)
2. **Never delete anything** → Waste disk space (defeats purpose)
3. **Require manual management** → Poor user experience

This RFC provides a comprehensive reference model that ensures safety while enabling automatic cleanup.

## Problem Statement

Design a reference tracking system that:

1. **Prevents accidental deletion** of models currently in use
2. **Tracks explicit references** (workflow → model dependencies)
3. **Tracks implicit references** (filesystem aliases)
4. **Provides safety buffer** (quarantine period before permanent deletion)
5. **Enables safe garbage collection** (remove truly unused models)
6. **Supports recovery** (restore quarantined models if needed)
7. **Scales efficiently** (handles millions of references)
8. **Clear semantics** (users understand what will/won't be deleted)

## Proposed Design

### Reference Types

modeld tracks two distinct types of references:

#### 1. Explicit References

**Definition**: Direct, intentional references recorded by modeld when parsing workflows or user actions.

**Sources**:
- **ComfyUI workflows** (`workflow.json` files)
- **Forge scripts** (Python scripts referencing models)
- **A1111 configurations** (config files, scripts)
- **User tags/favorites** (explicit "keep this model" markers)
- **Download records** (recently downloaded models)

**Storage**: `refs` table in SQLite database

```sql
CREATE TABLE refs (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    model_hash   TEXT NOT NULL,
    ref_source   TEXT NOT NULL,  -- workflow file path or source identifier
    ref_type     TEXT,           -- lora | checkpoint | vae | controlnet | embedding
    last_checked TEXT DEFAULT (datetime('now')),
    
    FOREIGN KEY (model_hash) REFERENCES models(blake3_hash) ON DELETE CASCADE,
    UNIQUE (model_hash, ref_source, ref_type)
);
```

**Example**:
```
model_hash: abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890
ref_source: C:\ComfyUI\user\workflows\my_workflow.json
ref_type: checkpoint
last_checked: 2024-01-15T10:30:00Z
```

**Lifecycle**:
1. **Creation**: When workflow parsed or user marks favorite
2. **Validation**: Periodically check if source still exists
3. **Deletion**: When workflow deleted or user removes tag

#### 2. Implicit References (Aliases)

**Definition**: Filesystem-level references via hardlinks, symlinks, or file copies pointing to CAS objects.

**Sources**:
- **Hardlinks** (same-volume deduplication)
- **Symlinks** (cross-volume deduplication)
- **Junction points** (Windows directory links)
- **Reference-only** (file kept at original location, no link)
- **Virtual FS links** (modeld's virtual directories)

**Storage**: `aliases` table in SQLite database

```sql
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
```

**Example**:
```
model_hash: abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890
path: C:\ComfyUI\models\checkpoints\sdxl-base-1.0.safetensors
frontend: comfyui
alias_type: hardlink
created_at: 2024-01-15T09:00:00Z
```

**Lifecycle**:
1. **Creation**: When deduplication creates link or during initial scan
2. **Validation**: Check if path still exists and points to correct hash
3. **Deletion**: When file deleted or moved by user

### Reference Counting Algorithm

**Core Principle**: A model is considered "in use" if it has any explicit or implicit references.

#### Algorithm

```rust
/// Calculate total reference count for a model
fn get_ref_count(model_hash: &str, db: &Database) -> Result<RefCount> {
    // Count explicit references
    let explicit_refs = db.execute(
        "SELECT COUNT(*) FROM refs WHERE model_hash = ?",
        [model_hash]
    )?;
    
    // Count implicit references (aliases)
    let implicit_refs = db.execute(
        "SELECT COUNT(*) FROM aliases WHERE model_hash = ?",
        [model_hash]
    )?;
    
    Ok(RefCount {
        explicit: explicit_refs,
        implicit: implicit_refs,
        total: explicit_refs + implicit_refs,
    })
}

struct RefCount {
    explicit: usize,   // refs table entries
    implicit: usize,   // aliases table entries
    total: usize,      // sum of both
}
```

#### Detailed Breakdown

**Explicit Reference Counting**:
```sql
-- All workflows referencing this model
SELECT ref_source, ref_type, last_checked 
FROM refs 
WHERE model_hash = 'abcdef123...'
ORDER BY last_checked DESC;

-- Example results:
-- C:\ComfyUI\workflows\portrait.json | checkpoint | 2024-01-15
-- C:\ComfyUI\workflows\landscape.json | checkpoint | 2024-01-14
-- C:\Forge\scripts\batch_process.py | checkpoint | 2024-01-10
-- Total explicit refs: 3
```

**Implicit Reference Counting**:
```sql
-- All filesystem locations pointing to this model
SELECT path, alias_type, frontend 
FROM aliases 
WHERE model_hash = 'abcdef123...'
ORDER BY created_at DESC;

-- Example results:
-- C:\ComfyUI\models\checkpoints\sdxl.safetensors | hardlink | comfyui
-- C:\Forge\models\Stable-diffusion\sdxl.safetensors | symlink | forge
-- D:\A1111\models\Stable-diffusion\sdxl.safetensors | symlink | a1111
-- Total implicit refs: 3
```

**Total Reference Count**:
```
total_refs = explicit_refs + implicit_refs
           = 3 + 3
           = 6
```

**Protection Decision**:
```
if total_refs > 0:
    protection_level = PROTECTED
else:
    protection_level = QUARANTINE (eligible for deletion after TTL)
```

### GC Protection Levels

Models transition through three protection levels based on their reference count and time since becoming unreferenced.

```
┌──────────────────────────────────────────────────────────┐
│                    Protection Levels                      │
└──────────────────────────────────────────────────────────┘

Level 1: PROTECTED (ref_count > 0)
  ├─ Location: cas/blake3/{prefix}/{hash}
  ├─ State: Active, in use
  ├─ Actions:
  │  └─ Cannot be garbage collected
  │  └─ May have aliases in virtual/ directories
  │  └─ May be referenced by workflows
  └─ Transition: When ref_count drops to 0 → QUARANTINE

Level 2: QUARANTINE (ref_count = 0, age < TTL)
  ├─ Location: quarantine/{hash}.{timestamp}
  ├─ State: Soft-deleted, recoverable
  ├─ TTL: 30 days (default, configurable)
  ├─ Actions:
  │  └─ Moved from CAS to quarantine directory
  │  └─ Metadata preserved (.meta file)
  │  └─ User can restore with `modeld restore <hash>`
  │  └─ Counts against storage quota (encourages cleanup)
  └─ Transition: After TTL expires → DELETED

Level 3: DELETED (age ≥ TTL)
  ├─ Location: Permanently removed from filesystem
  ├─ State: Unrecoverable (unless backed up externally)
  ├─ Actions:
  │  └─ File deleted from quarantine/
  │  └─ Database record marked as deleted or removed
  │  └─ Space freed
  └─ Transition: None (terminal state)
```

#### State Diagram

```
┌─────────────┐
│  PROTECTED  │ ◄─┐
│ (ref_count  │   │
│     > 0)    │   │ New alias added
└──────┬──────┘   │ or workflow reference
       │          │
       │ All refs removed
       │ (ref_count = 0)
       ▼          │
┌──────────────┐  │
│  QUARANTINE  │  │
│ (ref_count=0,│  │
│  age < TTL)  │  │
└──────┬───────┘  │
       │          │
       │ TTL expired  User restores
       │ (30 days)    (modeld restore)
       ▼          │
┌──────────────┐  │
│   DELETED    │  │
│ (permanent)  │  │
└──────────────┘  │
                  │
      ┌───────────┘
      │ (back to PROTECTED)
```

#### Protection Level Determination

```rust
enum ProtectionLevel {
    Protected,
    Quarantine,
    Deleted,
}

fn determine_protection_level(
    model_hash: &str, 
    db: &Database,
    config: &GcConfig
) -> Result<ProtectionLevel> {
    let ref_count = get_ref_count(model_hash, db)?;
    
    // Level 1: PROTECTED (has references)
    if ref_count.total > 0 {
        return Ok(ProtectionLevel::Protected);
    }
    
    // Level 2/3: Check quarantine status
    if let Some(quarantine_info) = db.get_quarantine_info(model_hash)? {
        let age = Utc::now() - quarantine_info.quarantine_date;
        
        if age < config.quarantine_ttl {
            return Ok(ProtectionLevel::Quarantine);
        } else {
            return Ok(ProtectionLevel::Deleted);
        }
    }
    
    // Not yet quarantined, but ref_count = 0 → eligible for quarantine
    Ok(ProtectionLevel::Quarantine)
}

struct GcConfig {
    quarantine_ttl: Duration,  // Default: 30 days
}
```

#### Edge Cases

**Case 1: Stale Aliases**
- **Scenario**: Alias exists in database, but file was deleted by user
- **Detection**: Periodic validation checks if alias path exists
- **Action**: Remove stale alias from database, decrement ref_count

**Case 2: Orphaned Workflow References**
- **Scenario**: Workflow deleted, but refs table entry remains
- **Detection**: Periodic check if `ref_source` file exists
- **Action**: Remove orphaned reference, decrement ref_count

**Case 3: Temporary Downloads**
- **Scenario**: Model downloaded but not yet integrated
- **Protection**: Download creates explicit ref until user confirms deletion
- **Action**: User must explicitly delete or ref expires after grace period

### Quarantine Mechanism

#### Directory Structure

```
quarantine/
├── {hash}.{unix_timestamp}       # Quarantined CAS object (actual file)
└── {hash}.{unix_timestamp}.meta  # Quarantine metadata (JSON)
```

**Example**:
```
quarantine/
├── abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890.1705320600
└── abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890.1705320600.meta
```

**Naming Convention**:
- `{hash}`: Full 64-character BLAKE3 hash
- `{unix_timestamp}`: Unix epoch seconds when quarantined (for uniqueness and sorting)
- `.meta`: JSON metadata file with additional context

#### Metadata Format

```json
{
  "hash": "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
  "size_bytes": 6942694400,
  "format": "safetensors",
  "arch": "sdxl",
  "base_model": "sdxl-base",
  "quarantine_date": "2024-01-15T10:30:00Z",
  "deletion_date": "2024-02-14T10:30:00Z",
  "reason": "zero_refs",
  "last_aliases": [
    "C:\\ComfyUI\\models\\checkpoints\\sdxl-base-1.0.safetensors",
    "D:\\Forge\\models\\Stable-diffusion\\sdxl-base-1.0.safetensors"
  ],
  "last_refs": [
    "C:\\ComfyUI\\workflows\\old_workflow.json (deleted)"
  ],
  "user_note": ""
}
```

**Metadata Fields**:
- `hash`: Model identifier
- `size_bytes`: File size (for space accounting)
- `format`, `arch`, `base_model`: Model metadata (from models table)
- `quarantine_date`: When model was quarantined (ISO 8601 timestamp)
- `deletion_date`: When model will be permanently deleted (quarantine_date + TTL)
- `reason`: Why quarantined (`zero_refs` | `user_requested` | `gc_auto`)
- `last_aliases`: Filesystem paths that referenced this model (before deletion)
- `last_refs`: Workflow/explicit references (before removal)
- `user_note`: Optional user comment (for manual quarantine)

#### Quarantine Lifecycle

```
┌────────────────────────────────────────────────────────────┐
│                  Quarantine Lifecycle                       │
└────────────────────────────────────────────────────────────┘

Step 1: Trigger Condition
  ├─ GC detects: model has ref_count = 0
  ├─ Check: Not already in quarantine
  └─ Action: Initiate quarantine process

Step 2: Move to Quarantine
  ├─ Read model metadata from models table
  ├─ Query last_aliases and last_refs
  ├─ Generate quarantine metadata JSON
  ├─ Move file: cas/blake3/{prefix}/{hash} → quarantine/{hash}.{timestamp}
  ├─ Write metadata: quarantine/{hash}.{timestamp}.meta
  ├─ Update models table: set quarantined_at = now()
  └─ Log event: "Model {hash} quarantined"

Step 3: Grace Period (30 days default)
  ├─ Model file remains in quarantine/
  ├─ Counts against storage quota
  ├─ User can list: `modeld quarantine list`
  ├─ User can restore: `modeld restore <hash>`
  └─ User can purge early: `modeld gc --purge <hash>`

Step 4: Expiration Check (daily cron or manual)
  ├─ Query: SELECT * FROM models WHERE quarantined_at < (now - TTL)
  ├─ For each expired model:
  │  ├─ Verify ref_count still = 0 (safety check)
  │  ├─ Delete file: quarantine/{hash}.{timestamp}
  │  ├─ Delete metadata: quarantine/{hash}.{timestamp}.meta
  │  ├─ Remove from models table (or mark deleted)
  │  └─ Log event: "Model {hash} permanently deleted"
  └─ Report space freed

Step 5: Permanent Deletion
  ├─ File removed from filesystem
  ├─ Database record removed or marked deleted
  ├─ Unrecoverable (unless external backup exists)
  └─ Space freed and available
```

#### Database Integration

Add quarantine tracking to `models` table:

```sql
-- Add quarantine columns to models table
ALTER TABLE models ADD COLUMN quarantined_at TEXT DEFAULT NULL;
ALTER TABLE models ADD COLUMN quarantine_reason TEXT DEFAULT NULL;

-- Index for efficient expiration queries
CREATE INDEX idx_models_quarantined ON models(quarantined_at) 
WHERE quarantined_at IS NOT NULL;

-- Query for expired models
SELECT blake3_hash, size_bytes, quarantined_at
FROM models
WHERE quarantined_at IS NOT NULL
  AND datetime(quarantined_at, '+30 days') < datetime('now');
```

### GC Trigger Conditions

Garbage collection can be triggered automatically or manually:

#### 1. Manual Trigger

```bash
# Explicit user command
modeld gc
```

**Behavior**:
- User-initiated cleanup
- Shows confirmation prompts
- Reports space to be freed
- Requires user approval for quarantine

#### 2. Automatic Trigger (Disk Usage Threshold)

```bash
# Trigger when disk usage exceeds threshold
# Configured in modeld.toml
```

**Configuration**:
```toml
[gc]
auto_trigger = true
disk_threshold_percent = 90  # Trigger when 90% full
check_interval_hours = 24    # Check every 24 hours
```

**Behavior**:
```rust
fn check_auto_gc(config: &GcConfig) -> Result<bool> {
    let store_path = get_store_path();
    let disk_usage = get_disk_usage(&store_path)?;
    
    if disk_usage.percent_used >= config.disk_threshold_percent {
        log::warn!(
            "Disk usage at {}%, threshold is {}%. Triggering automatic GC.",
            disk_usage.percent_used,
            config.disk_threshold_percent
        );
        return Ok(true);
    }
    
    Ok(false)
}
```

#### 3. Scheduled Trigger (Cron)

```bash
# Scheduled via cron job (Unix) or Task Scheduler (Windows)
# Example: Daily at 3am
0 3 * * * modeld gc --auto
```

**Behavior**:
- Non-interactive mode
- Moves zero-ref models to quarantine
- Deletes expired quarantine models
- Logs results to file

#### GC Modes

modeld provides different GC modes to balance safety and automation:

```bash
# Mode 1: Safe (default)
modeld gc --safe
```

**Safe Mode Behavior**:
- Interactive confirmations for each action
- Only quarantines models with ref_count = 0
- Shows detailed information before quarantine
- Requires explicit user approval
- Maximum safety, minimal automation

```bash
# Mode 2: Dry-run
modeld gc --dry-run
```

**Dry-run Behavior**:
- No actual changes made
- Shows what would happen
- Reports potential space savings
- Useful for previewing cleanup
- Safe for testing

```bash
# Mode 3: Auto
modeld gc --auto
```

**Auto Mode Behavior**:
- Non-interactive
- Quarantines zero-ref models automatically
- Deletes expired quarantine models
- Suitable for cron jobs
- Logs all actions to file

```bash
# Mode 4: Aggressive (requires --force)
modeld gc --aggressive --force
```

**Aggressive Mode Behavior**:
- Skips quarantine for selected models
- Immediate permanent deletion
- Requires explicit confirmation for each model
- Use only when certain
- High risk, maximum cleanup

### Safe GC Algorithm

The safe garbage collection algorithm ensures models are never deleted while in use.

#### Algorithm Overview

```
┌────────────────────────────────────────────────────────────┐
│              Safe Garbage Collection Algorithm              │
└────────────────────────────────────────────────────────────┘

Phase 1: Reference Validation
  ├─ For each model in database:
  │  ├─ Recount references (don't trust cached counts)
  │  ├─ Validate aliases: check if paths still exist
  │  ├─ Validate refs: check if source files still exist
  │  └─ Update ref_count in database
  │
  └─ Remove stale references from database

Phase 2: Candidate Selection
  ├─ Query: SELECT * FROM models WHERE ref_count = 0
  ├─ Filter: Exclude already quarantined models
  ├─ Filter: Exclude models with recent activity (< 7 days)
  ├─ Sort: By last_seen ASC (oldest first)
  └─ Result: List of GC candidates

Phase 3: User Confirmation (Safe Mode)
  ├─ For each candidate:
  │  ├─ Display: hash, size, format, arch, last_seen
  │  ├─ Display: last known aliases and refs
  │  ├─ Prompt: "Quarantine this model? (y/N/skip/abort)"
  │  └─ Record decision
  │
  └─ Abort if user chooses abort

Phase 4: Quarantine Execution
  ├─ For each approved candidate:
  │  ├─ Double-check: ref_count still = 0
  │  ├─ Generate quarantine metadata
  │  ├─ Move file to quarantine/
  │  ├─ Write .meta file
  │  ├─ Update database: quarantined_at = now()
  │  └─ Log action
  │
  └─ Report: Total quarantined, space pending cleanup

Phase 5: Expiration Processing
  ├─ Query expired quarantined models
  ├─ For each expired model:
  │  ├─ Triple-check: ref_count still = 0 (paranoid safety)
  │  ├─ Delete file from quarantine/
  │  ├─ Delete .meta file
  │  ├─ Remove from database (or mark deleted)
  │  └─ Log deletion
  │
  └─ Report: Total deleted, space freed
```

#### Detailed Implementation

```rust
/// Safe GC main entry point
fn safe_gc(mode: GcMode, config: &GcConfig, db: &Database) -> Result<GcReport> {
    let mut report = GcReport::new();
    
    // Phase 1: Reference Validation
    validate_all_references(db, &mut report)?;
    
    // Phase 2: Candidate Selection
    let candidates = select_gc_candidates(db, config)?;
    
    if mode == GcMode::DryRun {
        report.dry_run = true;
        report.candidates = candidates.len();
        report.potential_space_freed = candidates.iter()
            .map(|c| c.size_bytes)
            .sum();
        return Ok(report);
    }
    
    // Phase 3: User Confirmation (if interactive)
    let approved = if mode.is_interactive() {
        get_user_approval(&candidates)?
    } else {
        candidates // Auto mode approves all
    };
    
    // Phase 4: Quarantine Execution
    for candidate in approved {
        quarantine_model(&candidate, db, config, &mut report)?;
    }
    
    // Phase 5: Expiration Processing
    process_expired_quarantine(db, config, &mut report)?;
    
    Ok(report)
}

/// Phase 1: Validate all references and remove stale ones
fn validate_all_references(db: &Database, report: &mut GcReport) -> Result<()> {
    // Validate aliases (implicit references)
    let aliases = db.query("SELECT * FROM aliases")?;
    for alias in aliases {
        if !Path::new(&alias.path).exists() {
            // Stale alias - path no longer exists
            db.execute("DELETE FROM aliases WHERE id = ?", [alias.id])?;
            report.stale_aliases_removed += 1;
            log::info!("Removed stale alias: {} (file not found)", alias.path);
        }
    }
    
    // Validate refs (explicit references)
    let refs = db.query("SELECT * FROM refs")?;
    for ref_entry in refs {
        if !Path::new(&ref_entry.ref_source).exists() {
            // Orphaned ref - source file deleted
            db.execute("DELETE FROM refs WHERE id = ?", [ref_entry.id])?;
            report.orphaned_refs_removed += 1;
            log::info!("Removed orphaned ref: {} (source not found)", 
                      ref_entry.ref_source);
        }
    }
    
    Ok(())
}

/// Phase 2: Select models eligible for GC
fn select_gc_candidates(db: &Database, config: &GcConfig) -> Result<Vec<GcCandidate>> {
    let grace_period = config.gc_grace_period; // e.g., 7 days
    let cutoff_date = Utc::now() - grace_period;
    
    let query = "
        SELECT m.blake3_hash, m.size_bytes, m.format, m.arch, 
               m.last_seen, m.quarantined_at
        FROM models m
        WHERE (
            SELECT COUNT(*) FROM refs WHERE model_hash = m.blake3_hash
        ) + (
            SELECT COUNT(*) FROM aliases WHERE model_hash = m.blake3_hash
        ) = 0
        AND m.quarantined_at IS NULL
        AND datetime(m.last_seen) < datetime(?)
        ORDER BY m.last_seen ASC
    ";
    
    let candidates = db.query(query, [cutoff_date.to_rfc3339()])?;
    Ok(candidates)
}

/// Phase 3: Get user approval (interactive mode)
fn get_user_approval(candidates: &[GcCandidate]) -> Result<Vec<GcCandidate>> {
    let mut approved = Vec::new();
    
    println!("\nFound {} models eligible for garbage collection:\n", 
             candidates.len());
    
    for (i, candidate) in candidates.iter().enumerate() {
        println!("Model {}/{}:", i + 1, candidates.len());
        println!("  Hash: {}...", &candidate.hash[..16]);
        println!("  Size: {}", format_bytes(candidate.size_bytes));
        println!("  Format: {}", candidate.format);
        println!("  Last seen: {}", candidate.last_seen);
        
        // Show last known locations
        let aliases = get_last_aliases(&candidate.hash)?;
        if !aliases.is_empty() {
            println!("  Last known locations:");
            for alias in aliases.iter().take(3) {
                println!("    - {}", alias);
            }
        }
        
        print!("\nQuarantine this model? (y/N/skip/abort): ");
        std::io::stdout().flush()?;
        
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        
        match input.trim().to_lowercase().as_str() {
            "y" | "yes" => approved.push(candidate.clone()),
            "n" | "no" | "" => continue,
            "skip" => continue,
            "abort" => break,
            _ => continue,
        }
    }
    
    Ok(approved)
}

/// Phase 4: Move model to quarantine
fn quarantine_model(
    candidate: &GcCandidate, 
    db: &Database,
    config: &GcConfig,
    report: &mut GcReport
) -> Result<()> {
    // Double-check ref_count (safety)
    let ref_count = get_ref_count(&candidate.hash, db)?;
    if ref_count.total > 0 {
        log::warn!("Skipping model {} - gained references during GC", 
                  &candidate.hash[..16]);
        report.skipped_gained_refs += 1;
        return Ok(());
    }
    
    let quarantine_dir = config.store_path.join("quarantine");
    std::fs::create_dir_all(&quarantine_dir)?;
    
    let timestamp = Utc::now().timestamp();
    let quarantine_file = quarantine_dir.join(
        format!("{}.{}", candidate.hash, timestamp)
    );
    let meta_file = quarantine_dir.join(
        format!("{}.{}.meta", candidate.hash, timestamp)
    );
    
    // Get current CAS path
    let cas_path = get_cas_path(&candidate.hash, &config.store_path);
    
    // Collect metadata
    let last_aliases = get_last_aliases(&candidate.hash)?;
    let last_refs = get_last_refs(&candidate.hash)?;
    
    let metadata = QuarantineMetadata {
        hash: candidate.hash.clone(),
        size_bytes: candidate.size_bytes,
        format: candidate.format.clone(),
        arch: candidate.arch.clone(),
        base_model: candidate.base_model.clone(),
        quarantine_date: Utc::now(),
        deletion_date: Utc::now() + config.quarantine_ttl,
        reason: "zero_refs".to_string(),
        last_aliases,
        last_refs,
        user_note: String::new(),
    };
    
    // Write metadata
    let metadata_json = serde_json::to_string_pretty(&metadata)?;
    std::fs::write(&meta_file, metadata_json)?;
    
    // Move file to quarantine
    std::fs::rename(&cas_path, &quarantine_file)?;
    
    // Update database
    db.execute(
        "UPDATE models SET quarantined_at = ?, quarantine_reason = ? 
         WHERE blake3_hash = ?",
        [Utc::now().to_rfc3339(), "zero_refs".to_string(), candidate.hash.clone()]
    )?;
    
    report.models_quarantined += 1;
    report.space_pending_cleanup += candidate.size_bytes;
    
    log::info!("Quarantined model: {} ({} bytes)", 
              &candidate.hash[..16], candidate.size_bytes);
    
    Ok(())
}

/// Phase 5: Process expired quarantine models
fn process_expired_quarantine(
    db: &Database,
    config: &GcConfig,
    report: &mut GcReport
) -> Result<()> {
    let expiration_cutoff = Utc::now() - config.quarantine_ttl;
    
    let expired_models = db.query(
        "SELECT blake3_hash, size_bytes, quarantined_at 
         FROM models 
         WHERE quarantined_at IS NOT NULL
           AND datetime(quarantined_at) < datetime(?)",
        [expiration_cutoff.to_rfc3339()]
    )?;
    
    for model in expired_models {
        // Triple-check ref_count (paranoid safety)
        let ref_count = get_ref_count(&model.hash, db)?;
        if ref_count.total > 0 {
            log::warn!("Skipping deletion of {} - has references!", 
                      &model.hash[..16]);
            // Model somehow gained refs while in quarantine - restore it
            restore_from_quarantine(&model.hash, db, config)?;
            report.skipped_gained_refs += 1;
            continue;
        }
        
        // Find quarantine files
        let quarantine_dir = config.store_path.join("quarantine");
        let pattern = format!("{}.*", model.hash);
        
        for entry in std::fs::read_dir(&quarantine_dir)? {
            let entry = entry?;
            let filename = entry.file_name().to_string_lossy().to_string();
            
            if filename.starts_with(&model.hash) {
                std::fs::remove_file(entry.path())?;
                log::info!("Deleted quarantine file: {}", filename);
            }
        }
        
        // Remove from database
        db.execute(
            "DELETE FROM models WHERE blake3_hash = ?",
            [&model.hash]
        )?;
        
        report.models_deleted += 1;
        report.space_freed += model.size_bytes;
        
        log::info!("Permanently deleted model: {} ({} bytes freed)", 
                  &model.hash[..16], model.size_bytes);
    }
    
    Ok(())
}

struct GcReport {
    dry_run: bool,
    candidates: usize,
    stale_aliases_removed: usize,
    orphaned_refs_removed: usize,
    models_quarantined: usize,
    models_deleted: usize,
    skipped_gained_refs: usize,
    space_pending_cleanup: u64,
    space_freed: u64,
    potential_space_freed: u64,
}
```

#### Safety Invariants

The GC algorithm maintains these critical safety invariants:

1. **No false deletions**: A model with ref_count > 0 is NEVER quarantined
2. **No data loss**: Models go through quarantine before permanent deletion
3. **No race conditions**: Reference validation happens atomically
4. **No orphaned references**: Stale refs are cleaned up before GC
5. **No silent failures**: All operations are logged and reported

#### Performance Considerations

```rust
// Optimize reference counting with a single query
fn get_ref_count_optimized(model_hash: &str, db: &Database) -> Result<RefCount> {
    let result = db.query_row(
        "SELECT 
            (SELECT COUNT(*) FROM refs WHERE model_hash = ?) as explicit_refs,
            (SELECT COUNT(*) FROM aliases WHERE model_hash = ?) as implicit_refs",
        [model_hash, model_hash]
    )?;
    
    Ok(RefCount {
        explicit: result.explicit_refs,
        implicit: result.implicit_refs,
        total: result.explicit_refs + result.implicit_refs,
    })
}

// Batch validation for efficiency
fn validate_references_batch(db: &Database) -> Result<()> {
    // Check all aliases in one pass
    db.execute("
        DELETE FROM aliases 
        WHERE NOT EXISTS (
            SELECT 1 FROM files 
            WHERE files.path = aliases.path
        )
    ")?;
    
    // Check all refs in one pass
    db.execute("
        DELETE FROM refs 
        WHERE NOT EXISTS (
            SELECT 1 FROM files 
            WHERE files.path = refs.ref_source
        )
    ")?;
    
    Ok(())
}
```

### Recovery Procedures

modeld provides comprehensive recovery mechanisms for accidentally deleted models.

#### Recovery from Quarantine

**Scenario**: User realizes a quarantined model is still needed.

```bash
# List quarantined models
modeld quarantine list

# Output:
# Hash                 Size    Quarantine Date  Days Left  Last Location
# abcdef1234567890...  6.5GB   2024-01-15       23         C:\ComfyUI\models\checkpoints\sdxl.safetensors
# fedcba9876543210...  2.1GB   2024-01-20       28         C:\Forge\models\loras\character.safetensors
```

```bash
# Restore a specific model
modeld restore abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890

# or shorter (first 16 chars)
modeld restore abcdef1234567890
```

**Restore Algorithm**:

```rust
fn restore_from_quarantine(
    hash_prefix: &str, 
    db: &Database, 
    config: &GcConfig
) -> Result<()> {
    // Find full hash (support prefix matching)
    let full_hash = resolve_hash_prefix(hash_prefix, db)?;
    
    // Check if in quarantine
    let model = db.query_row(
        "SELECT * FROM models WHERE blake3_hash = ? AND quarantined_at IS NOT NULL",
        [&full_hash]
    )?;
    
    if model.is_none() {
        return Err(Error::NotInQuarantine(full_hash));
    }
    
    let quarantine_dir = config.store_path.join("quarantine");
    let cas_dir = config.store_path.join("cas/blake3");
    
    // Find quarantine file (any timestamp)
    let pattern = format!("{}.*", full_hash);
    let quarantine_files: Vec<_> = std::fs::read_dir(&quarantine_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.starts_with(&full_hash) && !name.ends_with(".meta")
        })
        .collect();
    
    if quarantine_files.is_empty() {
        return Err(Error::QuarantineFileNotFound(full_hash));
    }
    
    let quarantine_file = &quarantine_files[0];
    
    // Read metadata
    let meta_path = format!("{}.meta", quarantine_file.path().display());
    let metadata: QuarantineMetadata = serde_json::from_str(
        &std::fs::read_to_string(&meta_path)?
    )?;
    
    // Compute target CAS path
    let prefix = &full_hash[..2];
    let cas_path = cas_dir.join(prefix).join(&full_hash);
    
    // Create directory if needed
    std::fs::create_dir_all(cas_path.parent().unwrap())?;
    
    // Move file back to CAS
    std::fs::rename(quarantine_file.path(), &cas_path)?;
    
    // Delete metadata
    std::fs::remove_file(&meta_path)?;
    
    // Update database
    db.execute(
        "UPDATE models SET quarantined_at = NULL, quarantine_reason = NULL 
         WHERE blake3_hash = ?",
        [&full_hash]
    )?;
    
    // Optionally recreate aliases
    println!("\nModel restored to CAS. Previous locations:");
    for alias in metadata.last_aliases {
        println!("  {}", alias);
    }
    
    print!("\nRecreate aliases at previous locations? (y/N): ");
    std::io::stdout().flush()?;
    
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    
    if input.trim().to_lowercase() == "y" {
        recreate_aliases(&full_hash, &metadata.last_aliases, db, config)?;
    }
    
    log::info!("Restored model {} from quarantine", &full_hash[..16]);
    println!("\n✓ Model successfully restored!");
    
    Ok(())
}

fn recreate_aliases(
    model_hash: &str,
    locations: &[String],
    db: &Database,
    config: &GcConfig
) -> Result<()> {
    let cas_path = get_cas_path(model_hash, &config.store_path);
    
    for location in locations {
        let location_path = Path::new(location);
        
        // Create parent directory if needed
        if let Some(parent) = location_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        
        // Determine link strategy
        let link_type = determine_link_type(&cas_path, location_path)?;
        
        match link_type {
            LinkType::Hardlink => {
                std::fs::hard_link(&cas_path, location_path)?;
                println!("  ✓ Hardlink: {}", location);
            }
            LinkType::Symlink => {
                #[cfg(unix)]
                std::os::unix::fs::symlink(&cas_path, location_path)?;
                #[cfg(windows)]
                std::os::windows::fs::symlink_file(&cas_path, location_path)?;
                println!("  ✓ Symlink: {}", location);
            }
            LinkType::Copy => {
                std::fs::copy(&cas_path, location_path)?;
                println!("  ✓ Copy: {}", location);
            }
            LinkType::Skip => {
                println!("  ⚠ Skipped: {} (cannot create link)", location);
                continue;
            }
        }
        
        // Add to aliases table
        db.execute(
            "INSERT INTO aliases (model_hash, path, alias_type, frontend) 
             VALUES (?, ?, ?, ?)",
            [model_hash, location, link_type.to_string(), detect_frontend(location)]
        )?;
    }
    
    Ok(())
}
```

#### Recovery After Permanent Deletion

**Scenario**: Model was permanently deleted from quarantine.

**Options**:

1. **Re-download from source** (if HuggingFace or known URL)
   ```bash
   # modeld can re-download if it remembers the source
   modeld download --hash abcdef1234567890
   
   # Or manually re-download
   # modeld will detect duplicate and deduplicate automatically
   ```

2. **Restore from external backup** (if user maintains backups)
   ```bash
   # Copy model file back to any location
   cp /backup/model.safetensors /path/to/models/
   
   # Re-scan to detect
   modeld scan /path/to/models/
   ```

3. **Unrecoverable** (if no source and no backup)
   - Model is permanently lost
   - User must re-acquire from original source

**Prevention Best Practices**:

- Enable automatic backups of critical models
- Tag important models as favorites (explicit refs)
- Regularly review quarantine list
- Use longer TTL for critical systems (e.g., 90 days)

#### Bulk Restore

```bash
# Restore all models quarantined in last N days
modeld restore --recent 7

# Restore by pattern
modeld restore --pattern "sdxl*"

# Restore all from quarantine
modeld restore --all
```

#### Quarantine Management Commands

```bash
# List quarantined models
modeld quarantine list [--sort-by date|size] [--filter arch=sdxl]

# View quarantine details
modeld quarantine info <hash>

# Extend TTL for specific model
modeld quarantine extend <hash> --days 30

# Purge expired models immediately (skip TTL)
modeld quarantine purge [--force]

# Export quarantine metadata
modeld quarantine export --json quarantine_backup.json
```

## Implementation Considerations

### Concurrency

**Problem**: Multiple processes might access the same model simultaneously.

**Solution**:
- SQLite WAL mode for concurrent readers
- Advisory file locks for writes
- Atomic operations where possible

```rust
fn safe_quarantine_with_lock(model_hash: &str, db: &Database) -> Result<()> {
    // Acquire exclusive lock on model record
    let lock = db.begin_exclusive_transaction()?;
    
    // Re-check ref_count after acquiring lock
    let ref_count = get_ref_count(model_hash, db)?;
    if ref_count.total > 0 {
        return Err(Error::ModelStillReferenced);
    }
    
    // Perform quarantine
    quarantine_model_inner(model_hash, db)?;
    
    lock.commit()?;
    Ok(())
}
```

### Performance Optimization

**Reference Counting Cache**:
```rust
// Cache ref_counts to avoid repeated DB queries
struct RefCountCache {
    cache: HashMap<String, (RefCount, Instant)>,
    ttl: Duration,
}

impl RefCountCache {
    fn get(&mut self, hash: &str, db: &Database) -> Result<RefCount> {
        if let Some((count, timestamp)) = self.cache.get(hash) {
            if timestamp.elapsed() < self.ttl {
                return Ok(*count);
            }
        }
        
        let count = get_ref_count(hash, db)?;
        self.cache.insert(hash.to_string(), (count, Instant::now()));
        Ok(count)
    }
}
```

### Error Handling

**Quarantine Failures**:
- If move fails: retry with copy + verify + delete
- If metadata write fails: rollback move
- If database update fails: rollback filesystem changes

**Validation Errors**:
- Log all stale references for audit
- Continue GC even if some validations fail
- Report errors at end

### Monitoring & Observability

**Metrics to Track**:
- Total models in each protection level
- Quarantine size (bytes)
- GC runs per day
- False positive rate (models restored from quarantine)
- Space freed per GC run

**Logging**:
```rust
log::info!("GC started: mode={:?}", mode);
log::info!("Validated {} aliases, {} refs", alias_count, ref_count);
log::info!("Removed {} stale aliases, {} orphaned refs", stale, orphaned);
log::info!("Quarantined {} models ({} bytes)", count, bytes);
log::info!("Deleted {} expired models ({} bytes freed)", count, bytes);
log::info!("GC completed in {:?}", duration);
```


## Alternatives Considered

### Alternative 1: No Quarantine (Direct Deletion)

**Approach**: Delete models immediately when ref_count drops to 0.

**Pros**:
- Simpler implementation
- Immediate space reclamation
- No quarantine overhead

**Cons**:
- High risk of accidental data loss
- No recovery mechanism
- Users would be afraid to use GC
- One mistake = permanent data loss

**Decision**: Rejected due to safety concerns.

### Alternative 2: Infinite Retention (No GC)

**Approach**: Never delete anything, keep all models forever.

**Pros**:
- Maximum safety
- No risk of data loss
- Simple implementation

**Cons**:
- Defeats purpose of modeld (space savings)
- Disk fills up indefinitely
- No automatic cleanup
- Users must manually manage space

**Decision**: Rejected as it doesn't solve the problem.

### Alternative 3: Recycle Bin Integration

**Approach**: Use OS recycle bin (Windows) or trash (Linux/macOS).

**Pros**:
- Familiar to users
- OS-provided recovery mechanism
- Integrates with existing workflows

**Cons**:
- Platform-specific implementations
- Limited metadata (can't store last_aliases, etc.)
- Recycle bin may auto-purge based on OS settings
- No programmatic control over TTL
- Large model files might bypass recycle bin (size limits)

**Decision**: Rejected due to platform inconsistencies and lack of control.

### Alternative 4: Reference Counting Only (No Explicit Tracking)

**Approach**: Only count filesystem links (hardlinks/symlinks), no database refs table.

**Pros**:
- Simpler model
- Fewer database tables
- Automatic via filesystem

**Cons**:
- Cannot track workflow dependencies
- Cannot support user favorites/tags
- No semantic information (why is this referenced?)
- Cannot detect stale references
- Loses information when links broken

**Decision**: Rejected as it doesn't handle workflow dependencies.

### Alternative 5: Mark-and-Sweep GC (Like Git)

**Approach**: Periodic scan to mark reachable objects, delete unreachable ones.

**Pros**:
- Similar to git's model
- Well-understood algorithm
- Can handle complex reference graphs

**Cons**:
- Expensive full scans required
- More complex implementation
- Harder to reason about (what's reachable?)
- No quarantine concept
- Less predictable behavior

**Decision**: Rejected due to complexity and lack of safety buffer.

## Security Considerations

### Privilege Escalation

**Risk**: Malicious workflow files could reference arbitrary paths to prevent GC.

**Mitigation**:
- Validate workflow file locations (must be in known frontend directories)
- Limit ref_source to trusted paths
- Sanitize all file paths

### Path Traversal

**Risk**: Malicious paths in aliases table could reference outside CAS.

**Mitigation**:
- Validate all paths before operations
- Use canonical paths (resolve symlinks)
- Check path is within expected directories

### Race Conditions

**Risk**: TOCTOU (Time-of-Check-Time-of-Use) bugs in ref counting.

**Mitigation**:
- Use database transactions
- Re-validate ref_count after acquiring locks
- Atomic operations where possible

### Denial of Service

**Risk**: Creating millions of refs to exhaust storage or database.

**Mitigation**:
- Limit refs per model (e.g., 1000 max)
- Periodic cleanup of orphaned refs
- Rate limiting on ref creation

## Future Enhancements

### Enhancement 1: Smart TTL

**Idea**: Adjust TTL based on model characteristics.

```rust
fn calculate_smart_ttl(model: &Model) -> Duration {
    let base_ttl = Duration::days(30);
    
    // Larger models get longer TTL (expensive to re-download)
    let size_factor = if model.size_bytes > 10_000_000_000 { 2.0 } else { 1.0 };
    
    // Recently used models get longer TTL
    let recency_factor = if model.last_seen > Utc::now() - Duration::days(7) {
        1.5
    } else {
        1.0
    };
    
    // Popular architectures get longer TTL
    let arch_factor = match model.arch.as_str() {
        "sdxl" | "flux" => 1.5,
        "sd1" => 1.2,
        _ => 1.0,
    };
    
    let multiplier = size_factor * recency_factor * arch_factor;
    Duration::from_secs((base_ttl.as_secs() as f64 * multiplier) as u64)
}
```

### Enhancement 2: Predictive GC

**Idea**: Predict which models will be needed soon based on usage patterns.

- Track model usage frequency
- Detect seasonal patterns (weekday vs weekend)
- Protect frequently used models from GC
- Pre-load likely-to-be-used models

### Enhancement 3: Tiered Storage

**Idea**: Move quarantined models to cheaper storage (slower disk, cloud).

- Quarantine tier 1: Fast SSD (first 7 days)
- Quarantine tier 2: Slow HDD (days 8-30)
- Quarantine tier 3: Cloud backup (optional, after 30 days)

### Enhancement 4: Workflow-Aware GC

**Idea**: Analyze workflow execution frequency to inform GC decisions.

- Track which workflows are run often
- Protect models used by frequently-run workflows
- Warn before GC if model used by recent workflow

### Enhancement 5: Undo History

**Idea**: Keep GC history for rollback.

```bash
# Show recent GC operations
modeld gc history

# Undo last GC operation
modeld gc undo

# Undo specific GC run
modeld gc undo --run-id abc123
```

### Enhancement 6: Reference Graphs Visualization

**Idea**: Visualize model dependencies.

```bash
# Show all references to a model
modeld refs show <hash>

# Show reference graph
modeld refs graph --output refs.dot

# Find unused models
modeld refs unused
```

## Testing Strategy

### Unit Tests

- `test_ref_counting()`: Verify ref_count calculation
- `test_protection_levels()`: Verify level transitions
- `test_quarantine_metadata()`: Verify JSON serialization
- `test_gc_safety()`: Verify no false deletions
- `test_restore()`: Verify recovery works

### Integration Tests

- `test_gc_workflow()`: Full GC cycle
- `test_concurrent_gc()`: Multiple GC runs simultaneously
- `test_stale_ref_cleanup()`: Orphaned reference removal
- `test_quarantine_expiration()`: TTL-based deletion

### Property-Based Tests

```rust
#[quickcheck]
fn prop_never_delete_referenced_model(refs: Vec<Ref>) -> bool {
    // Property: If a model has any references, it's PROTECTED
    let model_hash = "test_hash";
    db.insert_refs(refs);
    let protection = determine_protection_level(model_hash, &db, &config);
    
    if get_ref_count(model_hash, &db).total > 0 {
        matches!(protection, ProtectionLevel::Protected)
    } else {
        true // No refs = can be QUARANTINE or DELETED
    }
}

#[quickcheck]
fn prop_restore_preserves_content(model: Model) -> bool {
    // Property: Restoring a model preserves its content
    let original_hash = compute_hash(&model);
    quarantine_model(&model, &db, &config);
    restore_from_quarantine(&model.hash, &db, &config);
    let restored_hash = compute_hash(&get_model(&model.hash));
    
    original_hash == restored_hash
}
```

### Manual Test Cases

1. **Basic GC Flow**
   - Scan models
   - Remove all aliases
   - Run GC
   - Verify model in quarantine
   - Wait 30 days (or adjust TTL)
   - Verify permanent deletion

2. **Restore Flow**
   - Quarantine a model
   - List quarantine
   - Restore model
   - Verify back in CAS
   - Verify aliases recreated

3. **Edge Cases**
   - GC with stale aliases
   - GC with orphaned refs
   - Concurrent GC runs
   - Restore while GC running
   - Quarantine file corruption

## Rollout Plan

### Phase 1: Read-Only Monitoring (Weeks 1-2)

- Deploy ref tracking (no GC)
- Monitor ref_count accuracy
- Validate stale reference detection
- Collect metrics

### Phase 2: Manual GC Only (Weeks 3-4)

- Enable manual `modeld gc` command
- Require explicit confirmation for every action
- Monitor for false positives
- Gather user feedback

### Phase 3: Quarantine Testing (Weeks 5-6)

- Enable quarantine mechanism
- Test restore functionality
- Validate TTL expiration
- Verify metadata accuracy

### Phase 4: Automatic GC (Opt-In) (Weeks 7-8)

- Enable auto GC for willing users
- Monitor disk threshold triggers
- Track false positive rate
- Adjust TTL based on data

### Phase 5: General Availability (Week 9+)

- Enable auto GC by default
- Provide opt-out mechanism
- Monitor at scale
- Iterate based on production data

## Success Criteria

- **Safety**: Zero false deletions (ref_count > 0 models never GC'd)
- **Recovery**: 100% restore success rate from quarantine
- **Performance**: GC completes in <10 minutes for 10K models
- **User Satisfaction**: <1% of users disable auto GC
- **Space Savings**: Average 20-30% disk space reclaimed

## Appendix: Reference Counting Examples

### Example 1: Simple Case

```
Model: sdxl-base-1.0.safetensors
Hash: abcdef1234567890...

Aliases (3):
  - C:\ComfyUI\models\checkpoints\sdxl-base-1.0.safetensors (hardlink)
  - D:\Forge\models\Stable-diffusion\sdxl-base.safetensors (symlink)
  - E:\A1111\models\Stable-diffusion\sdxl.safetensors (symlink)

Refs (2):
  - C:\ComfyUI\workflows\portrait.json (checkpoint)
  - C:\ComfyUI\workflows\landscape.json (checkpoint)

Total ref_count: 3 + 2 = 5
Protection Level: PROTECTED
```

### Example 2: Zero References

```
Model: old-model-v1.ckpt
Hash: fedcba9876543210...

Aliases (0): [none]

Refs (0): [none]

Total ref_count: 0 + 0 = 0
Protection Level: QUARANTINE (eligible)
Action: Move to quarantine/ on next GC run
```

### Example 3: Stale Alias

```
Model: character-lora.safetensors
Hash: 123456789abcdef0...

Aliases (2):
  - C:\ComfyUI\models\loras\character.safetensors (deleted by user)
  - D:\Forge\models\loras\character.safetensors (exists)

Refs (1):
  - C:\ComfyUI\workflows\character_gen.json (lora)

After validation:
  Aliases (1): [stale alias removed]
  Refs (1): [still exists]
  Total ref_count: 1 + 1 = 2
  Protection Level: PROTECTED
```

## Glossary

- **Explicit Reference**: Database entry linking workflow → model
- **Implicit Reference**: Filesystem alias (hardlink/symlink) to CAS object
- **Reference Count**: Total of explicit + implicit references
- **Protection Level**: PROTECTED, QUARANTINE, or DELETED state
- **Quarantine**: Soft-delete state with recovery period
- **TTL**: Time-to-Live, grace period before permanent deletion (default 30 days)
- **GC**: Garbage Collection, automatic cleanup of unused models
- **Stale Alias**: Database entry for non-existent file
- **Orphaned Ref**: Reference to deleted workflow file
- **Safe GC**: GC mode that never deletes referenced models

---

**RFC Status**: Draft  
**Next Review**: After Phase 1 implementation  
**Approval Required**: Architecture team, Security team

