# RFC 0007: HuggingFace Interception

**Status**: Draft  
**Author**: modeld Architecture Team  
**Created**: 2024  
**Last Updated**: 2024

## Abstract

This RFC defines the HuggingFace ecosystem interception strategy for modeld, enabling transparent deduplication of model downloads from HuggingFace Hub across multiple AI frameworks (diffusers, transformers, ComfyUI, etc.) without code modifications. The design combines environment variable-based cache redirection (primary) with optional Python monkeypatching (fallback), fake HF cache layout emulation, SHA256↔BLAKE3 hash mapping, and comprehensive compatibility testing to ensure seamless integration.

## Motivation

HuggingFace Hub is the de facto standard for distributing AI models, with billions of downloads monthly. AI users frequently download the same models multiple times for different frameworks or projects:

**Common Duplication Scenarios**:
- User downloads `stabilityai/stable-diffusion-xl-base-1.0` for ComfyUI
- Later downloads the same model for Forge
- Downloads again for A1111 or diffusers scripts
- Each download: 6.94GB → Total: 20.82GB for 3 copies

**Challenges**:
1. **Framework diversity**: diffusers, transformers, ComfyUI, Forge each have their own download mechanisms
2. **No central cache**: Each framework maintains its own cache (~/.cache/huggingface/)
3. **Hash diversity**: HF uses SHA256, modeld uses BLAKE3 (need bidirectional mapping)
4. **Fragility**: Monkeypatching can break with library updates
5. **User experience**: Must work transparently (zero config changes)

**Goals**:
- Automatically deduplicate HF downloads across all frameworks
- Work without modifying user code
- Survive HuggingFace library updates
- Provide 3 installation options (flexibility)
- Support both online downloads and offline mode


## Problem Statement

Design a HuggingFace interception system that:

1. **Intercepts downloads** from all major HF-based frameworks:
   - `diffusers.pipeline.from_pretrained()`
   - `transformers.AutoModel.from_pretrained()`
   - `huggingface_hub.hf_hub_download()`
   - ComfyUI's built-in model manager
   - Forge's download system
   - A1111's extension downloader

2. **Maintains compatibility** across library versions:
   - Survives huggingface_hub 0.19 → 0.20+ updates
   - Works with diffusers 0.25+ API changes
   - Handles transformers 4.30+ breaking changes

3. **Maps hash algorithms** bidirectionally:
   - SHA256 (used by HuggingFace) ↔ BLAKE3 (used by modeld)
   - Deduplicates when either hash is known
   - Stores both hashes for future lookups

4. **Emulates HF cache** transparently:
   - Frameworks expect specific directory structures
   - Must provide symlinks to CAS objects
   - Snapshots, refs, and blobs directories

5. **Offers installation flexibility**:
   - Option A: Explicit import (user control)
   - Option B: System-wide sitecustomize.py (automatic)
   - Option C: Environment variable PYTHONSTARTUP (session-level)

6. **Handles edge cases**:
   - Partial downloads (resumable)
   - Corrupted downloads (verify hashes)
   - Offline mode (serve from CAS)
   - Private HF repos (auth tokens)


## Proposed Design

### Two-Layer Interception Strategy

modeld uses a **defense-in-depth** approach with two complementary layers:

**Layer 1: Environment Variable (Primary, Stable)**
- Set `HF_HOME` to modeld-managed directory
- HuggingFace libraries natively respect this variable
- No code modification required
- **Stability**: Official API, won't break with updates
- **Coverage**: ~95% of use cases

**Layer 2: Python Monkeypatch (Fallback, Targeted)**
- Hook `huggingface_hub.hf_hub_download()` for edge cases
- Only activates when Layer 1 insufficient
- **Fragility**: May break with major HF updates
- **Coverage**: Remaining ~5% edge cases

**Design Philosophy**: Prefer stable, officially supported methods (HF_HOME) over fragile hacks (monkeypatching).

---

### Layer 1: HF_HOME Environment Variable (Primary Strategy)

#### Background

HuggingFace libraries check the following environment variables in order:
1. `HF_HOME` (primary cache directory)
2. `XDG_CACHE_HOME` (Linux standard) + `/huggingface`
3. `~/.cache/huggingface/` (default fallback)

By setting `HF_HOME`, we redirect all HF downloads to a modeld-managed directory without any code changes.

#### Implementation

**Environment Setup**:

```bash
# Linux/macOS
export HF_HOME="$HOME/.local/share/modeld/hf_cache"

# Windows (PowerShell)
$env:HF_HOME = "$env:USERPROFILE\.modeld\hf_cache"

# Windows (CMD)
set HF_HOME=%USERPROFILE%\.modeld\hf_cache
```


**modeld Integration**:

```bash
# modeld automatically sets HF_HOME on init
modeld init
  → Creates: ~/.local/share/modeld/hf_cache/
  → Adds to shell profile: export HF_HOME="..."
  → Verifies with test download

# User activates in current shell
source ~/.bashrc  # or ~/.zshrc, ~/.profile

# Verify it works
echo $HF_HOME
python -c "from huggingface_hub import constants; print(constants.HF_HOME)"
```

**Persistence**:

modeld adds the following to shell profiles:

```bash
# ~/.bashrc, ~/.zshrc, or ~/.profile
# >>> modeld HuggingFace cache integration >>>
export HF_HOME="$HOME/.local/share/modeld/hf_cache"
# <<< modeld HuggingFace cache integration <<<
```

Windows (PowerShell Profile):
```powershell
# Microsoft.PowerShell_profile.ps1
# >>> modeld HuggingFace cache integration >>>
$env:HF_HOME = "$env:USERPROFILE\.modeld\hf_cache"
# <<< modeld HuggingFace cache integration >>>
```

**Advantages**:
- ✅ **Stable**: Official HuggingFace API, won't break
- ✅ **Universal**: Works with all HF libraries (diffusers, transformers, safetensors)
- ✅ **Simple**: Single environment variable
- ✅ **No dependencies**: No Python package needed
- ✅ **Transparent**: Frameworks see normal HF cache structure

**Limitations**:
- ⚠ Requires shell profile modification (one-time)
- ⚠ Doesn't work in sandboxed environments (Docker without env forwarding)
- ⚠ User must source profile or restart shell


---

### Fake HuggingFace Cache Layout

modeld emulates the official HuggingFace Hub cache structure so frameworks can load models transparently.

#### Official HF Cache Structure

```
~/.cache/huggingface/hub/
└── models--{org}--{model}/
    ├── .no_exist/
    │   └── {revision}           # Placeholder for incomplete downloads
    ├── blobs/
    │   ├── {sha256_hash_1}      # Actual file content
    │   ├── {sha256_hash_2}
    │   └── ...
    ├── refs/
    │   ├── main                 # Text file containing revision hash
    │   └── {other_branches}
    └── snapshots/
        └── {revision_hash}/
            ├── model.safetensors → ../../blobs/{sha256}
            ├── config.json       → ../../blobs/{sha256}
            ├── tokenizer.json    → ../../blobs/{sha256}
            └── ...
```

**Key Components**:
1. **blobs/**: Content-addressed storage (SHA256-named files)
2. **snapshots/**: Logical views (symlinks to blobs)
3. **refs/**: Branch pointers (text files with revision hashes)
4. **.no_exist/**: Incomplete download tracking

#### modeld's Emulated Cache Structure

```
$MODELD_STORE/hf_cache/hub/
└── models--{org}--{model}/
    ├── .no_exist/
    │   └── {revision}           # Managed by modeld
    ├── blobs/
    │   └── {sha256}             → symlink to ../../../cas/blake3/{prefix}/{blake3_hash}
    ├── refs/
    │   └── main                 # Managed by modeld
    └── snapshots/
        └── {revision}/
            └── {filename}       → ../../blobs/{sha256}
```

**Key Differences**:
- **blobs/** contain symlinks (not actual files)
- Symlinks point to modeld CAS: `cas/blake3/{prefix}/{blake3_hash}`
- modeld maintains SHA256↔BLAKE3 mapping in database
- Same logical structure, different physical storage


#### Example: Stable Diffusion XL

**Download Request**:
```python
from diffusers import DiffusionPipeline
pipeline = DiffusionPipeline.from_pretrained("stabilityai/stable-diffusion-xl-base-1.0")
```

**Resulting Cache Structure**:
```
$HF_HOME/hub/
└── models--stabilityai--stable-diffusion-xl-base-1.0/
    ├── blobs/
    │   ├── 31e35c80fc4829d14f90153f4c74cd59c90b779f6afe05a74cd6120b893f7e5b
    │   │   → ../../../../cas/blake3/7a/7a3d9f8e1c2b4a5f6e8d9c0b1a2e3d4f5g6h7i8j9k0l1m2n3o4p5q6r7s8t9u0v1w
    │   ├── 594b2fd5af80c522b2c851d5fc94e7c1b161df9dce0b1c0c6d70a1e0d9534d3e
    │   │   → ../../../../cas/blake3/4b/4b8c7e6d5f4a3c2b1e0d9c8b7a6f5e4d3c2b1a0f9e8d7c6b5a4f3e2d1c0b9a8f
    │   └── ...
    ├── refs/
    │   └── main                     # Contains: "2b5db9c4..."
    └── snapshots/
        └── 2b5db9c4dd522e00db3ddb43c923aa8714e97ed7/
            ├── model_index.json     → ../../blobs/31e35c...
            ├── text_encoder/
            │   └── model.safetensors → ../../../blobs/594b2...
            ├── unet/
            │   └── diffusion_pytorch_model.safetensors → ../../../blobs/7f8a2...
            └── ...
```

**How It Works**:
1. Framework requests: `stabilityai/stable-diffusion-xl-base-1.0`
2. HF library checks: `$HF_HOME/hub/models--stabilityai--stable-diffusion-xl-base-1.0/`
3. Finds `snapshots/{revision}/` with all expected files
4. Follows symlinks: `blobs/{sha256}` → `cas/blake3/{blake3_hash}`
5. Loads model from CAS transparently
6. **Framework has no idea** it's loading from deduplicated storage


---

### SHA256 ↔ BLAKE3 Hash Mapping Strategy

HuggingFace uses SHA256 for content addressing; modeld uses BLAKE3. We need bidirectional mapping to deduplicate regardless of which hash we know first.

#### Database Schema Extension

```sql
-- Extend downloads table to include SHA256 hash
ALTER TABLE downloads ADD COLUMN sha256_hash TEXT;

-- Create index for fast SHA256 lookups
CREATE INDEX idx_downloads_sha256 ON downloads(sha256_hash);

-- Example record
INSERT INTO downloads (
    model_hash,           -- BLAKE3 hash (primary key in modeld)
    source_url,           -- https://huggingface.co/...
    sha256_hash,          -- SHA256 hash (from HF metadata)
    status,               -- 'done'
    bytes_total,
    finished_at
) VALUES (
    '7a3d9f8e1c2b4a5f6e8d9c0b1a2e3d4f5g6h7i8j9k0l1m2n3o4p5q6r7s8t9u0v1w',  -- BLAKE3
    'https://huggingface.co/stabilityai/stable-diffusion-xl-base-1.0/resolve/main/unet/diffusion_pytorch_model.safetensors',
    '594b2fd5af80c522b2c851d5fc94e7c1b161df9dce0b1c0c6d70a1e0d9534d3e',  -- SHA256
    'done',
    4265380512,
    '2024-01-15T10:30:00Z'
);
```

#### Mapping Algorithm

**Scenario 1: SHA256 Known (Download Request)**

```
User: pipeline.from_pretrained("org/model")
  │
  └─> HF library: need file with SHA256 = abc123...
      │
      └─> modeld intercepts:
          1. Query: SELECT model_hash FROM downloads WHERE sha256_hash = 'abc123...'
          2. Found? → Return symlink to cas/blake3/{prefix}/{blake3_hash} ✓
          3. Not found? → Proceed to download
```


**Scenario 2: BLAKE3 Known (User Scanned Existing Models)**

```
User: modeld scan ~/models/
  │
  └─> modeld computes BLAKE3 = xyz789...
      │
      └─> Later, HF download requests file with SHA256 = abc123...
          │
          └─> modeld:
              1. Query: SELECT model_hash FROM downloads WHERE sha256_hash = 'abc123...'
              2. Not found → download to temp
              3. Compute BLAKE3 = xyz789...
              4. Query: SELECT * FROM models WHERE blake3_hash = 'xyz789...'
              5. Found! → File already in CAS
              6. DELETE temp file
              7. INSERT mapping: (blake3=xyz789, sha256=abc123)
              8. Return symlink to existing CAS object
```

**Scenario 3: Neither Hash Known (New Download)**

```
HF download request:
  │
  ├─> Download to: tmp/downloads/{uuid}.part
  ├─> On completion:
  │   ├─> Extract SHA256 from HF metadata (.huggingface.json or HTTP headers)
  │   ├─> Compute BLAKE3 hash of downloaded file
  │   ├─> Query: SELECT * FROM models WHERE blake3_hash = ?
  │   │   └─> Found? → DELETE temp, use existing CAS object
  │   │   └─> Not found? → Move to CAS: cas/blake3/{prefix}/{hash}
  │   └─> INSERT INTO downloads (model_hash, sha256_hash, ...)
  └─> Create fake HF cache symlinks
```

#### HF Metadata Extraction

HuggingFace provides SHA256 hashes in multiple places:

**1. HTTP Response Headers** (most reliable):
```
X-Repo-Commit: 2b5db9c4dd522e00db3ddb43c923aa8714e97ed7
X-Linked-Etag: "594b2fd5af80c522b2c851d5fc94e7c1b161df9dce0b1c0c6d70a1e0d9534d3e"
X-Linked-Size: 4265380512
```
The `X-Linked-Etag` contains the SHA256 hash (without quotes).

**2. `.huggingface.json` Files** (cached metadata):
```json
{
  "sha256": "594b2fd5af80c522b2c851d5fc94e7c1b161df9dce0b1c0c6d70a1e0d9534d3e",
  "size": 4265380512,
  "url": "https://huggingface.co/stabilityai/stable-diffusion-xl-base-1.0/resolve/2b5db9c4/unet/diffusion_pytorch_model.safetensors",
  "etag": "\"594b2fd5af80c522b2c851d5fc94e7c1b161df9dce0b1c0c6d70a1e0d9534d3e\""
}
```


**3. Model Card Metadata** (repo-level):
```yaml
# README.md or .safetensors metadata
---
library_name: diffusers
files:
  - filename: unet/diffusion_pytorch_model.safetensors
    sha256: 594b2fd5af80c522b2c851d5fc94e7c1b161df9dce0b1c0c6d70a1e0d9534d3e
---
```

**Extraction Priority**:
1. HTTP headers (most reliable, always present)
2. .huggingface.json (cached, may be stale)
3. Compute SHA256 ourselves (fallback if HF doesn't provide)

**Rust Implementation** (pseudocode):
```rust
fn extract_sha256(response: &Response, file_path: &Path) -> Result<String> {
    // Priority 1: HTTP headers
    if let Some(etag) = response.headers().get("x-linked-etag") {
        let sha256 = etag.to_str()?.trim_matches('"');
        if sha256.len() == 64 {  // Valid SHA256 hex
            return Ok(sha256.to_string());
        }
    }
    
    // Priority 2: .huggingface.json
    let metadata_file = file_path.with_extension("huggingface.json");
    if metadata_file.exists() {
        if let Ok(json) = serde_json::from_reader(File::open(&metadata_file)?) {
            if let Some(sha256) = json["sha256"].as_str() {
                return Ok(sha256.to_string());
            }
        }
    }
    
    // Priority 3: Compute ourselves
    log::warn!("HF SHA256 not provided, computing manually");
    compute_sha256(file_path)
}

fn compute_sha256(file_path: &Path) -> Result<String> {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    let mut file = File::open(file_path)?;
    std::io::copy(&mut file, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}
```


---

### Download Deduplication Flow

Complete end-to-end flow for HuggingFace download interception and deduplication.

#### Flow Diagram

```
┌─────────────────────────────────────────────────────────────────┐
│ User Code: pipeline.from_pretrained("org/model")                │
└────────────────────────┬────────────────────────────────────────┘
                         │
                         ▼
          ┌──────────────────────────────┐
          │ HuggingFace Library          │
          │ Checks: $HF_HOME/hub/        │
          │ models--org--model/          │
          │ snapshots/{rev}/{file}       │
          └──────────┬───────────────────┘
                     │
            ┌────────┴─────────┐
            │                  │
        EXISTS              NOT FOUND
            │                  │
            ▼                  ▼
    ┌───────────────┐   ┌──────────────────────┐
    │ Return path   │   │ HF library calls:    │
    │ (symlink to   │   │ hf_hub_download()    │
    │ CAS object)   │   └──────────┬───────────┘
    └───────────────┘              │
                                   ▼
                    ┌──────────────────────────────┐
                    │ modeld intercepts download   │
                    │ Check downloads table:       │
                    │ WHERE sha256_hash = ?        │
                    └──────────┬───────────────────┘
                               │
                      ┌────────┴─────────┐
                      │                  │
                  CACHE HIT          CACHE MISS
                      │                  │
                      ▼                  ▼
            ┌──────────────────┐   ┌─────────────────────┐
            │ Return symlink   │   │ Download to:        │
            │ to CAS object    │   │ tmp/downloads/      │
            │                  │   │ {uuid}.part         │
            └──────────────────┘   └──────┬──────────────┘
                                          │
                                          ▼
                              ┌────────────────────────┐
                              │ Download with:         │
                              │ - HTTP Range support   │
                              │ - Progress callback    │
                              │ - Resume on failure    │
                              └──────┬─────────────────┘
                                     │
                                     ▼
                          ┌────────────────────────────┐
                          │ Download complete          │
                          │ Extract SHA256 from HF     │
                          │ (headers or metadata)      │
                          └──────┬─────────────────────┘
                                 │
                                 ▼
                      ┌────────────────────────────────┐
                      │ Compute BLAKE3 hash            │
                      │ of downloaded file             │
                      └──────┬─────────────────────────┘
                             │
                             ▼
                  ┌────────────────────────────────────┐
                  │ Check: model with BLAKE3 exists?   │
                  │ SELECT FROM models                 │
                  │ WHERE blake3_hash = ?              │
                  └──────┬─────────────────────────────┘
                         │
                ┌────────┴─────────┐
                │                  │
            EXISTS              NOT EXISTS
                │                  │
                ▼                  ▼
    ┌──────────────────┐   ┌───────────────────────┐
    │ DELETE temp file │   │ Move to CAS:          │
    │ (already have it)│   │ cas/blake3/{prefix}/  │
    │                  │   │ {blake3_hash}         │
    └──────┬───────────┘   └──────┬────────────────┘
           │                      │
           └──────────┬───────────┘
                      │
                      ▼
          ┌────────────────────────────────┐
          │ INSERT INTO downloads:         │
          │ (model_hash, sha256_hash,      │
          │  source_url, status='done')    │
          └──────┬─────────────────────────┘
                 │
                 ▼
      ┌────────────────────────────────────┐
      │ Create fake HF cache structure:    │
      │ - blobs/{sha256} → CAS symlink     │
      │ - snapshots/{rev}/{file} → blob    │
      │ - refs/main = {revision}           │
      └──────┬─────────────────────────────┘
             │
             ▼
  ┌────────────────────────────────────────┐
  │ Return path to HF library              │
  │ HF library loads model                 │
  │ (transparent to user)                  │
  └────────────────────────────────────────┘
```


#### Pseudocode Implementation

```rust
async fn handle_hf_download(
    repo_id: &str,
    filename: &str,
    revision: Option<&str>,
    db: &Database,
    cas_store: &CasStore,
) -> Result<PathBuf> {
    // Step 1: Construct HF cache path
    let model_dir = format!("models--{}", repo_id.replace('/', "--"));
    let rev = revision.unwrap_or("main");
    let cache_path = hf_cache_dir()
        .join("hub")
        .join(&model_dir)
        .join("snapshots")
        .join(rev)
        .join(filename);
    
    // Step 2: Check if already in fake cache
    if cache_path.exists() {
        log::info!("HF cache hit: {}", cache_path.display());
        return Ok(cache_path);
    }
    
    // Step 3: Fetch HF metadata (get SHA256 if available)
    let metadata = fetch_hf_metadata(repo_id, filename, rev).await?;
    let sha256 = metadata.sha256.as_ref();
    
    // Step 4: Check downloads table for SHA256 match
    if let Some(sha256_hash) = sha256 {
        if let Some(download) = db.get_download_by_sha256(sha256_hash)? {
            log::info!("Download cache hit (SHA256): {}", sha256_hash);
            
            // Create fake HF cache symlinks
            create_hf_cache_structure(&model_dir, rev, filename, sha256_hash, &download.model_hash, cas_store)?;
            
            return Ok(cache_path);
        }
    }
    
    // Step 5: Download to temporary location
    let temp_path = download_to_temp(repo_id, filename, rev, &metadata).await?;
    
    // Step 6: Compute BLAKE3 hash
    let blake3_hash = compute_blake3_hash(&temp_path)?;
    
    // Step 7: Check if model already exists in CAS
    let cas_path = if db.model_exists(&blake3_hash)? {
        log::info!("Model already in CAS: {}", blake3_hash);
        std::fs::remove_file(&temp_path)?;  // Delete duplicate
        cas_store.get_path(&blake3_hash)
    } else {
        // Move to CAS
        log::info!("New model, adding to CAS: {}", blake3_hash);
        cas_store.add_file(&temp_path, &blake3_hash)?
    };
    
    // Step 8: Compute/verify SHA256
    let sha256_hash = if let Some(provided_sha256) = sha256 {
        // Verify HF-provided SHA256
        let computed_sha256 = compute_sha256(&cas_path)?;
        if computed_sha256 != *provided_sha256 {
            return Err(Error::HashMismatch {
                expected: provided_sha256.clone(),
                actual: computed_sha256,
            });
        }
        provided_sha256.clone()
    } else {
        // Compute SHA256 ourselves
        compute_sha256(&cas_path)?
    };
    
    // Step 9: Record download in database
    db.insert_download(&Download {
        model_hash: blake3_hash.clone(),
        source_url: format!("https://huggingface.co/{}/resolve/{}/{}", repo_id, rev, filename),
        sha256_hash: Some(sha256_hash.clone()),
        status: DownloadStatus::Done,
        bytes_total: metadata.size,
        finished_at: Some(Utc::now()),
        ..Default::default()
    })?;
    
    // Step 10: Create fake HF cache structure
    create_hf_cache_structure(&model_dir, rev, filename, &sha256_hash, &blake3_hash, cas_store)?;
    
    Ok(cache_path)
}
```


---

### Layer 2: Python Hook Implementation (Fallback)

For edge cases where `HF_HOME` is insufficient, modeld provides an optional Python monkeypatch hook.

#### Use Cases for Layer 2

**When HF_HOME Isn't Enough**:
1. **Hardcoded cache paths**: Some tools ignore `HF_HOME` and hardcode `~/.cache/huggingface/`
2. **Subprocess calls**: Child processes may not inherit `HF_HOME`
3. **Docker containers**: Environment variables not forwarded
4. **Legacy code**: Old scripts written before `HF_HOME` support

**Design Philosophy**: Layer 2 is a **targeted fallback**, not the primary solution.

#### Implementation

**Package Structure**:
```
modeld-hook/
├── modeld_hook/
│   ├── __init__.py       # Auto-activation on import
│   ├── intercept.py      # Core interception logic
│   ├── cache.py          # Fake cache management
│   └── utils.py          # Helper functions
├── pyproject.toml
├── README.md
└── tests/
    └── test_intercept.py
```

**Core Hook** (`modeld_hook/__init__.py`):
```python
"""
modeld-hook: HuggingFace download interception for modeld CAS

Automatically activates on import to intercept HF downloads.
"""
import sys
import logging
from pathlib import Path

__version__ = "0.1.0"

logger = logging.getLogger(__name__)


def activate():
    """
    Monkey-patch HuggingFace download functions to use modeld cache.
    
    This function intercepts:
    - huggingface_hub.hf_hub_download()
    - huggingface_hub.snapshot_download()
    - transformers file download functions
    """
    try:
        import huggingface_hub
        from . import intercept
        
        # Patch hf_hub_download
        original_download = huggingface_hub.hf_hub_download
        huggingface_hub.hf_hub_download = intercept.modeld_hf_hub_download(original_download)
        
        # Patch snapshot_download
        original_snapshot = huggingface_hub.snapshot_download
        huggingface_hub.snapshot_download = intercept.modeld_snapshot_download(original_snapshot)
        
        logger.info("modeld-hook activated: HuggingFace downloads will be deduplicated")
        
    except ImportError:
        logger.debug("huggingface_hub not installed, skipping activation")
        pass
    except Exception as e:
        logger.error(f"Failed to activate modeld-hook: {e}")


# Auto-activate on import
activate()
```


**Interception Logic** (`modeld_hook/intercept.py`):
```python
import functools
import subprocess
import json
from pathlib import Path
from typing import Optional, Callable


def modeld_hf_hub_download(original_func: Callable):
    """
    Wrapper for hf_hub_download() that checks modeld cache first.
    """
    @functools.wraps(original_func)
    def wrapper(
        repo_id: str,
        filename: str,
        revision: Optional[str] = None,
        **kwargs
    ):
        # Step 1: Check if modeld CLI is available
        try:
            subprocess.run(["modeld", "--version"], capture_output=True, check=True)
        except (FileNotFoundError, subprocess.CalledProcessError):
            # modeld not installed, fall back to original
            return original_func(repo_id, filename, revision=revision, **kwargs)
        
        # Step 2: Query modeld cache
        result = subprocess.run(
            ["modeld", "hf-check", repo_id, filename, "--revision", revision or "main"],
            capture_output=True,
            text=True
        )
        
        if result.returncode == 0:
            # Cache hit - return path
            cache_path = result.stdout.strip()
            logger.debug(f"modeld cache hit: {cache_path}")
            return cache_path
        
        # Step 3: Cache miss - download via modeld
        logger.debug(f"modeld cache miss, downloading: {repo_id}/{filename}")
        result = subprocess.run(
            ["modeld", "hf-download", repo_id, filename, "--revision", revision or "main"],
            capture_output=True,
            text=True,
            check=True
        )
        
        downloaded_path = result.stdout.strip()
        return downloaded_path
    
    return wrapper


def modeld_snapshot_download(original_func: Callable):
    """
    Wrapper for snapshot_download() that checks modeld cache first.
    """
    @functools.wraps(original_func)
    def wrapper(repo_id: str, revision: Optional[str] = None, **kwargs):
        # Similar logic to hf_hub_download, but for full repo snapshots
        # ... (implementation similar to above)
        pass
    
    return wrapper
```


#### Installation Methods (3 Options)

**Option A: Explicit Import (Recommended for Users)**

User adds a single import at the top of their script:

```python
import modeld_hook  # Activates automatically
from diffusers import DiffusionPipeline

# Rest of code unchanged
pipeline = DiffusionPipeline.from_pretrained("stabilityai/stable-diffusion-xl-base-1.0")
```

**Pros**:
- ✅ Explicit user control
- ✅ Easy to disable (remove import)
- ✅ Clear what's happening

**Cons**:
- ⚠ Requires code modification (minimal)
- ⚠ User must remember to add import

---

**Option B: System-Wide sitecustomize.py (Automatic)**

modeld installer creates `sitecustomize.py` in Python's site-packages:

```bash
# During modeld init
modeld init --enable-python-hook

# This creates/modifies sitecustomize.py:
# /usr/lib/python3.11/site-packages/sitecustomize.py (Linux)
# C:\Python311\Lib\site-packages\sitecustomize.py (Windows)
```

**sitecustomize.py**:
```python
# Auto-generated by modeld
import modeld_hook
```

**Pros**:
- ✅ Completely transparent (zero code changes)
- ✅ Works with all Python scripts
- ✅ Applies to Jupyter notebooks, CLI tools, everything

**Cons**:
- ⚠ System-wide effect (affects all Python)
- ⚠ Harder to debug ("why is my HF download different?")
- ⚠ Conflicts with other sitecustomize.py files
- ⚠ Requires write access to site-packages

---

**Option C: PYTHONSTARTUP Environment Variable (Session-Level)**

Set an environment variable that Python reads on startup:

```bash
# Add to ~/.bashrc or ~/.zshrc
export PYTHONSTARTUP="$HOME/.modeld/hook_init.py"

# ~/.modeld/hook_init.py contains:
import modeld_hook
```

**Pros**:
- ✅ Session-level control (only affects terminals with env var)
- ✅ No site-packages modification needed
- ✅ Easy to enable/disable

**Cons**:
- ⚠ Only works for interactive sessions (not scripts)
- ⚠ Doesn't affect systemd services, cron jobs, etc.
- ⚠ User must configure environment

---

**Recommendation Matrix**:

| User Type | Recommended Option | Rationale |
|-----------|-------------------|-----------|
| Power users | Option A (explicit import) | Full control, clear intent |
| Casual users | Option B (sitecustomize.py) | Zero-config, transparent |
| Developers | Option A | Easier debugging |
| System-wide deployment | Option B | Automatic for all users |
| Testing/development | Option C | Easy to toggle on/off |


---

## Compatibility Testing Matrix

To ensure modeld works across the HuggingFace ecosystem, comprehensive testing is required.

### Library Version Matrix

| Library | Versions to Test | Critical APIs |
|---------|-----------------|---------------|
| **huggingface_hub** | 0.19.4, 0.20.0, 0.21.0, latest | `hf_hub_download()`, `snapshot_download()`, `HF_HOME` detection |
| **diffusers** | 0.25.0, 0.26.0, latest | `DiffusionPipeline.from_pretrained()`, cache behavior |
| **transformers** | 4.30.0, 4.35.0, latest | `AutoModel.from_pretrained()`, `AutoTokenizer.from_pretrained()` |
| **safetensors** | 0.4.0, latest | File format parsing, metadata extraction |
| **accelerate** | 0.25.0, latest | Multi-GPU download handling |

### Framework Integration Matrix

| Framework | Integration Point | Test Scenario |
|-----------|------------------|---------------|
| **ComfyUI** | Built-in model manager | Download checkpoint via UI, verify dedup |
| **Forge** | Extension system | Install extension that downloads models |
| **A1111** | Model downloader | Download via extension, verify symlinks |
| **InvokeAI** | Model manager | Import from HF, verify cache usage |
| **diffusers CLI** | `diffusers-cli` commands | `diffusers-cli download`, check cache |

### Test Cases

#### TC1: Fresh Download
```python
# Precondition: Model not in cache
from diffusers import DiffusionPipeline

pipeline = DiffusionPipeline.from_pretrained("runwayml/stable-diffusion-v1-5")

# Expected:
# 1. modeld downloads to tmp/
# 2. Computes BLAKE3 + extracts SHA256
# 3. Moves to CAS
# 4. Creates fake HF cache symlinks
# 5. Returns path to framework
# 6. Framework loads successfully
```

#### TC2: Duplicate Detection (SHA256 Match)
```python
# Precondition: Model already in CAS with SHA256 mapping
from transformers import AutoModel

model = AutoModel.from_pretrained("bert-base-uncased")

# Expected:
# 1. HF requests download
# 2. modeld queries: downloads.sha256_hash = ?
# 3. Cache hit! Returns existing CAS path
# 4. NO download occurs
# 5. Framework loads from CAS
```


#### TC3: Duplicate Detection (BLAKE3 Match)
```python
# Precondition: User scanned local models, file has BLAKE3 hash but no SHA256 mapping
from diffusers import DiffusionPipeline

# HF tries to download a model that user already has locally
pipeline = DiffusionPipeline.from_pretrained("stabilityai/stable-diffusion-xl-base-1.0")

# Expected:
# 1. modeld downloads to tmp/ (SHA256 not in downloads table yet)
# 2. Computes BLAKE3 = abc123...
# 3. Queries: models.blake3_hash = abc123...
# 4. Cache hit! File already exists
# 5. DELETE temp file
# 6. INSERT SHA256 mapping
# 7. Create fake HF cache symlinks to existing CAS object
# 8. NO additional storage used
```

#### TC4: Cross-Framework Deduplication
```python
# Scenario: Same model used by diffusers and ComfyUI
import modeld_hook
from diffusers import DiffusionPipeline

# User 1: Downloads via diffusers
pipeline = DiffusionPipeline.from_pretrained("runwayml/stable-diffusion-v1-5")
# → Stored in CAS

# User 2 (later): Downloads same model via ComfyUI UI
# ComfyUI → HF library → modeld intercept → SHA256 match → Reuse CAS object
# → Zero additional storage
```

#### TC5: Resumable Downloads
```python
# Scenario: Download interrupted mid-way
from diffusers import DiffusionPipeline

# First attempt (interrupted at 50%)
try:
    pipeline = DiffusionPipeline.from_pretrained("stabilityai/stable-diffusion-xl-base-1.0")
except KeyboardInterrupt:
    pass

# Second attempt (resume from 50%)
pipeline = DiffusionPipeline.from_pretrained("stabilityai/stable-diffusion-xl-base-1.0")

# Expected:
# 1. modeld finds tmp/downloads/{uuid}.part
# 2. HTTP Range request: "Range: bytes=5368709120-"
# 3. Resume download from 50%
# 4. Complete and move to CAS
```

#### TC6: Offline Mode
```python
# Scenario: Model already in CAS, no internet connection
import modeld_hook
from diffusers import DiffusionPipeline

# Disconnect network
# ...

pipeline = DiffusionPipeline.from_pretrained(
    "runwayml/stable-diffusion-v1-5",
    local_files_only=True  # HF offline mode
)

# Expected:
# 1. HF checks local cache only
# 2. Finds fake HF cache structure (created by modeld)
# 3. Follows symlinks to CAS
# 4. Loads model successfully
# 5. NO network requests
```


### Platform-Specific Tests

#### Windows
- ✅ Symlink creation with Developer Mode
- ✅ Fallback to junctions for directories
- ⚠ exFAT drives (no symlink support)

#### Linux
- ✅ Standard symlink behavior
- ✅ Network mounts (NFS, SMB)
- ✅ Docker containers (HF_HOME forwarding)

#### macOS
- ✅ APFS symlinks
- ✅ External drives (HFS+, APFS)

### Performance Tests

| Test | Target | Measurement |
|------|--------|-------------|
| Cache hit latency | <100ms | Time from request to path return |
| SHA256 lookup | <10ms | Database query time |
| BLAKE3 computation (12GB) | <6s | Hash speed (2GB/s target) |
| Fake cache creation | <50ms | Symlink creation overhead |

---

## Alternatives Considered

### Alternative 1: HTTP Proxy

**Idea**: Run a local proxy server that intercepts HuggingFace API requests.

```
User code → HTTP request to huggingface.co
  → modeld proxy intercepts
  → Returns cached file or proxies to HF
```

**Pros**:
- Language-agnostic (works with any HTTP client)
- No Python dependency

**Cons**:
- **Complex**: Requires proxy server daemon
- **HTTPS issues**: Certificate validation, MITM concerns
- **Port conflicts**: Proxy port may be in use
- **Framework compatibility**: Some frameworks bypass system proxy

**Decision**: **Rejected** - Too complex, fragile, and has security implications.

---

### Alternative 2: LD_PRELOAD / DLL Injection

**Idea**: Use OS-level dynamic library preloading to hijack file system calls.

```bash
# Linux
LD_PRELOAD=/usr/lib/modeld_interpose.so python script.py

# Windows
# DLL injection into Python process
```

**Pros**:
- Extremely powerful (intercepts all file operations)
- Language-agnostic

**Cons**:
- **Platform-specific**: Different implementation for each OS
- **Fragile**: Breaks with Python version changes
- **Complex**: Requires C/C++ development
- **Security**: Rejected by sandboxed environments

**Decision**: **Rejected** - Too complex and platform-specific.

---


### Alternative 3: Custom PyPI Mirror

**Idea**: Host a custom PyPI mirror that serves patched versions of `huggingface_hub`.

```bash
pip install --index-url https://modeld.io/pypi huggingface_hub
```

**Pros**:
- Centralized patching
- Works transparently after installation

**Cons**:
- **Infrastructure**: Requires hosting and maintaining PyPI mirror
- **Updates**: Must patch every HF release
- **Trust**: Users must trust custom package source
- **Maintenance burden**: High ongoing cost

**Decision**: **Rejected** - Too much infrastructure and maintenance.

---

### Alternative 4: Fork HuggingFace Libraries

**Idea**: Maintain forks of `huggingface_hub`, `diffusers`, `transformers` with modeld integration.

```bash
pip install modeld-huggingface-hub
pip install modeld-diffusers
```

**Pros**:
- Full control over integration
- Can optimize for modeld use cases

**Cons**:
- **Massive maintenance burden**: Must track upstream changes
- **Fragmentation**: Users have to choose between official and modeld versions
- **Updates**: Lag behind official releases
- **Community**: Loses upstream bug fixes and features

**Decision**: **Rejected** - Unsustainable maintenance burden.

---

### Alternative 5: Patch HuggingFace Source on Install

**Idea**: When modeld installs, patch the HuggingFace libraries in-place.

```bash
modeld init --patch-huggingface
  → Modifies site-packages/huggingface_hub/*.py files
```

**Pros**:
- Works without import or environment changes
- Fast (no interprocess communication)

**Cons**:
- **Breaks on updates**: `pip install --upgrade huggingface_hub` undoes patches
- **Fragile**: Different patch for each version
- **Integrity**: May break package signatures/verification
- **Conflicts**: Multiple tools patching same files

**Decision**: **Rejected** - Too fragile and breaks with updates.

---

## Decision Summary

| Approach | Stability | Coverage | Complexity | Selected |
|----------|-----------|----------|------------|----------|
| **HF_HOME env var** | ✅ High | ✅ 95% | ✅ Low | **YES (Primary)** |
| **Python monkeypatch** | ⚠ Medium | ⚠ 5% | ⚠ Medium | **YES (Fallback)** |
| HTTP proxy | ⚠ Medium | ✅ High | ❌ High | ❌ NO |
| LD_PRELOAD | ❌ Low | ✅ High | ❌ Very High | ❌ NO |
| Custom PyPI mirror | ⚠ Medium | ✅ High | ❌ Very High | ❌ NO |
| Fork libraries | ✅ High | ✅ High | ❌ Extreme | ❌ NO |
| Patch on install | ❌ Low | ✅ High | ❌ High | ❌ NO |

**Final Design**: Two-layer approach (HF_HOME + optional Python hook) balances stability, coverage, and complexity.


---

## Implementation Considerations

### Rust Implementation

**modeld CLI Commands**:

```bash
# Initialize HF integration
modeld hf init
  → Creates fake HF cache structure
  → Sets HF_HOME in shell profile
  → Tests with sample download

# Check if file is in cache
modeld hf-check <repo_id> <filename> [--revision <rev>]
  → Returns: path if in cache, exit code 1 if not

# Download via modeld
modeld hf-download <repo_id> <filename> [--revision <rev>]
  → Downloads, deduplicates, returns path

# List cached HF models
modeld hf list
  → Shows all HF models in CAS with SHA256 mappings

# Verify HF cache integrity
modeld hf verify
  → Checks all symlinks point to valid CAS objects
  → Rebuilds broken symlinks
```

**Rust Crate Dependencies**:

```toml
[dependencies]
# HTTP client for HuggingFace API
reqwest = { version = "0.11", features = ["json", "stream"] }

# SHA256 computation (for HF compatibility)
sha2 = "0.10"

# Async runtime
tokio = { version = "1", features = ["full"] }

# JSON parsing (HF metadata)
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Progress bars
indicatif = "0.17"

# Resumable downloads
bytes = "1"
```

### Python Package Implementation

**Package Structure**:

```
modeld-hook/
├── modeld_hook/
│   ├── __init__.py       # Auto-activation
│   ├── intercept.py      # Monkeypatch logic
│   ├── cache.py          # Cache checking
│   ├── cli.py            # Subprocess calls to modeld
│   └── utils.py          # Helpers
├── tests/
│   ├── test_intercept.py
│   ├── test_cache.py
│   └── fixtures/
├── pyproject.toml
├── README.md
└── LICENSE
```

**pyproject.toml**:

```toml
[project]
name = "modeld-hook"
version = "0.1.0"
description = "HuggingFace download interception hook for modeld CAS"
authors = [{name = "modeld Team"}]
readme = "README.md"
requires-python = ">=3.8"
license = {text = "MIT"}

dependencies = [
    # No dependencies! Works via subprocess calls to modeld CLI
]

[project.optional-dependencies]
dev = [
    "pytest>=7.0",
    "huggingface_hub>=0.19",
    "diffusers>=0.25",
    "transformers>=4.30",
]

[project.urls]
Homepage = "https://github.com/modeld/modeld"
Documentation = "https://modeld.dev/docs"
Repository = "https://github.com/modeld/modeld-hook"

[build-system]
requires = ["setuptools>=61.0", "wheel"]
build-backend = "setuptools.build_meta"
```


### Error Handling

**Error Scenarios**:

| Error | Detection | Recovery |
|-------|-----------|----------|
| **modeld not installed** | `subprocess.run()` fails with FileNotFoundError | Fall back to original HF download |
| **Hash mismatch** | SHA256 verification fails | Delete file, retry download (max 3 attempts) |
| **Corrupted download** | BLAKE3 computation fails | Delete partial file, restart download |
| **Disk full** | OS error during CAS write | Show error, suggest cleanup, abort |
| **Symlink creation fails** | Windows permission error | Fall back to reference-only mode |
| **Network error** | HTTP timeout/connection refused | Retry with exponential backoff (3 attempts) |
| **HF auth required** | 401/403 from HuggingFace | Prompt user for HF token, store in keyring |

**Graceful Degradation**:

```python
def modeld_hf_hub_download(original_func):
    @functools.wraps(original_func)
    def wrapper(*args, **kwargs):
        try:
            # Try modeld interception
            return modeld_download(*args, **kwargs)
        except ModeldNotInstalledError:
            logger.debug("modeld not available, using standard HF download")
            return original_func(*args, **kwargs)
        except ModeldCacheError:
            logger.warning("modeld cache error, falling back to standard download")
            return original_func(*args, **kwargs)
        except Exception as e:
            logger.error(f"Unexpected modeld error: {e}, falling back")
            return original_func(*args, **kwargs)
    return wrapper
```

**Principle**: Never break user's workflow. If modeld fails, fall back to standard HF download.

---

## Security Considerations

### Threat: Malicious Model Injection

**Scenario**: Attacker replaces cached model with malicious weights.

**Mitigations**:
1. **Hash verification**: Always verify BLAKE3 and SHA256 before serving
2. **CAS immutability**: chmod 444 (read-only) on CAS objects
3. **Symlink validation**: Check symlink targets on access
4. **Quarantine**: Suspicious files moved to quarantine, not served

### Threat: Path Traversal

**Scenario**: Malicious repo_id like `../../../etc/passwd`.

**Mitigations**:
1. **Input validation**: Reject repo_id with `..` or absolute paths
2. **Canonical paths**: Use `Path.resolve()` to normalize
3. **Jail paths**: Ensure all operations stay within `$MODELD_STORE`

```rust
fn validate_repo_id(repo_id: &str) -> Result<()> {
    if repo_id.contains("..") || repo_id.starts_with('/') || repo_id.contains('\\') {
        return Err(Error::InvalidRepoId(repo_id.to_string()));
    }
    Ok(())
}
```

### Threat: HF Token Leakage

**Scenario**: User's HuggingFace authentication token stored insecurely.

**Mitigations**:
1. **Keyring storage**: Use OS keyring (Windows Credential Manager, macOS Keychain, Linux Secret Service)
2. **Environment fallback**: `HF_TOKEN` env var (user's responsibility)
3. **No logging**: Never log tokens in debug output
4. **Permissions**: Token file (if used) is chmod 600 (user-only)


---

## Monkeypatch vs HF_HOME Comparison

Detailed comparison of the two interception strategies.

### Stability Comparison

| Aspect | HF_HOME (Layer 1) | Monkeypatch (Layer 2) |
|--------|-------------------|----------------------|
| **API Stability** | ✅ Official HuggingFace API | ⚠ Internal function hooking |
| **Breakage Risk** | ✅ Low (env vars rarely change) | ⚠ High (functions can be renamed) |
| **Version Compatibility** | ✅ Works with all HF versions | ❌ May break with major updates |
| **Maintenance** | ✅ None required | ⚠ Must track HF releases |
| **Debugging** | ✅ Easy (standard behavior) | ⚠ Harder (hidden patching) |

**Example Breakage** (Monkeypatch):
```python
# HuggingFace 0.19
from huggingface_hub import hf_hub_download  # ✓ Works

# HuggingFace 0.20 (hypothetical)
from huggingface_hub import download_file  # Function renamed!
# → modeld patch breaks
```

### Coverage Comparison

| Use Case | HF_HOME | Monkeypatch |
|----------|---------|-------------|
| `diffusers.from_pretrained()` | ✅ Yes | ✅ Yes |
| `transformers.AutoModel` | ✅ Yes | ✅ Yes |
| `huggingface_hub.hf_hub_download()` | ✅ Yes | ✅ Yes |
| ComfyUI model manager | ✅ Yes | ⚠ Maybe |
| Subprocess downloads | ⚠ If env inherited | ❌ No |
| Docker containers | ⚠ If env forwarded | ❌ No |
| Hardcoded cache paths | ❌ No | ✅ Yes |

**Coverage Summary**:
- **HF_HOME**: 95% of real-world use cases
- **Monkeypatch**: Catches remaining 5% (hardcoded paths)

### Performance Comparison

| Operation | HF_HOME | Monkeypatch | Winner |
|-----------|---------|-------------|--------|
| Cache hit latency | ~5ms (path check) | ~50ms (subprocess call) | HF_HOME |
| Download overhead | ~0ms (native HF) | ~20ms (interception) | HF_HOME |
| Memory usage | 0 MB | ~10 MB (Python hook) | HF_HOME |
| CPU overhead | 0% | <1% | HF_HOME |

### User Experience Comparison

| Aspect | HF_HOME | Monkeypatch |
|--------|---------|-------------|
| **Setup Complexity** | ⚠ Modify shell profile | ✅ Just `pip install` + import |
| **Transparency** | ✅ Explicit env var | ⚠ Hidden patching |
| **Debuggability** | ✅ Easy | ⚠ Confusing |
| **Activation** | ⚠ Must restart shell | ✅ Immediate |
| **Deactivation** | ✅ Just unset env | ⚠ Must uninstall package |

### Recommended Strategy

**Primary (95% of users)**: HF_HOME
- Set during `modeld init`
- Stable, fast, transparent
- Covers all major frameworks

**Fallback (5% edge cases)**: Monkeypatch
- Optional `pip install modeld-hook`
- For hardcoded cache paths
- Explicit import or sitecustomize.py

**Never**: Rely solely on monkeypatch (too fragile).


---

## Open Questions (To Be Resolved in Phase 3 Implementation)

1. **HF SHA256 Availability**: Do all HuggingFace downloads provide SHA256 hashes?
   - **Action**: Survey popular models, test with various file types
   - **Fallback**: Compute SHA256 ourselves if not provided

2. **Private Repo Handling**: How to handle HuggingFace authentication tokens securely?
   - **Action**: Research OS keyring integration (Windows Credential Manager, macOS Keychain)
   - **Fallback**: Environment variable `HF_TOKEN` (user's responsibility)

3. **Snapshot Downloads**: How to efficiently handle `snapshot_download()` (entire repos)?
   - **Action**: Test with large repos (100+ files), measure performance
   - **Optimization**: Parallel file processing

4. **HF API Rate Limits**: Does deduplication trigger rate limits?
   - **Action**: Monitor during testing, implement backoff if needed
   - **Mitigation**: Cache HF metadata locally to reduce API calls

5. **Monkeypatch Fragility**: How often do HF libraries break monkeypatch?
   - **Action**: Set up CI to test against HF nightly builds
   - **Monitoring**: Alert on breakage, fix within 24 hours

6. **Large File Resumption**: Does HF library support HTTP Range requests reliably?
   - **Action**: Test with 20GB+ models, simulate network interruptions
   - **Fallback**: Implement our own resumable downloader if HF's is insufficient

7. **Metadata Parsing**: Are there edge cases in `.huggingface.json` format?
   - **Action**: Collect samples from diverse models (LLMs, diffusion, multimodal)
   - **Robustness**: Defensive parsing with schema validation

8. **Docker Integration**: Best practice for HF_HOME in containerized environments?
   - **Action**: Test with Docker Compose, Kubernetes
   - **Documentation**: Provide example Dockerfiles

---

## Summary

**HuggingFace Interception Strategy**:

1. **Primary Method**: Set `HF_HOME` environment variable to modeld-managed directory
   - ✅ Stable, official API
   - ✅ Works with 95% of frameworks
   - ⚠ Requires shell profile modification

2. **Fake HF Cache**: Emulate official HuggingFace cache structure
   - `blobs/{sha256}` → symlinks to `cas/blake3/{blake3_hash}`
   - Frameworks load transparently from deduplicated storage

3. **Hash Mapping**: Bidirectional SHA256 ↔ BLAKE3 mapping in database
   - Enables deduplication regardless of which hash is known first
   - Extract SHA256 from HF metadata (HTTP headers or .huggingface.json)

4. **Download Deduplication**: Check cache before downloading
   - SHA256 match → instant return (no download)
   - BLAKE3 match after download → delete duplicate, reuse CAS object

5. **Fallback Method**: Optional Python monkeypatch hook (`modeld-hook`)
   - ⚠ Fragile, may break with HF updates
   - ✅ Catches edge cases (hardcoded cache paths)
   - 3 installation options: explicit import, sitecustomize.py, PYTHONSTARTUP

6. **Compatibility**: Comprehensive testing matrix
   - huggingface_hub, diffusers, transformers (multiple versions)
   - ComfyUI, Forge, A1111, InvokeAI integration
   - Windows, Linux, macOS platform tests

**Design Philosophy**: Prefer stable, officially supported methods (HF_HOME) over fragile hacks (monkeypatching). Layer 1 (HF_HOME) handles 95% of use cases; Layer 2 (monkeypatch) is a targeted fallback for edge cases.

**Installation Options**:
- **Option A**: Explicit `import modeld_hook` (user control)
- **Option B**: System-wide `sitecustomize.py` (automatic)
- **Option C**: `PYTHONSTARTUP` environment variable (session-level)

**Expected Outcome**: Users download models once, all frameworks reuse deduplicated storage. Zero config changes to existing code (with HF_HOME), or single import line (with monkeypatch).

---

*RFC 0007 Status: Draft - Pending Phase 3 Implementation and Testing*

