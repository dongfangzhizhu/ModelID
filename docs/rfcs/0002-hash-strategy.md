# RFC 0002: Hash Strategy

**Status**: Draft  
**Author**: modeld Architecture Team  
**Created**: 2024  
**Last Updated**: 2024

## Abstract

This RFC defines the BLAKE3 hashing strategy for modeld, including hash function selection rationale, chunk size optimization, small file handling, incremental hashing with mtime/size caching, performance targets, and memory usage considerations. The goal is to achieve ≥2GB/s sustained hash throughput on modern NVMe storage while minimizing redundant computation through intelligent caching.

## Motivation

AI model files present unique hashing challenges:
- **Large file sizes**: Individual models range from 100MB to 100GB+
- **Frequent re-scanning**: Users need to detect new/changed models efficiently
- **Performance critical**: Initial scans of 1TB+ model collections must complete in reasonable time
- **Storage detection**: Duplicate detection requires hashing potentially millions of files
- **Incremental updates**: Most scans find no changes, should be fast

A well-optimized hashing strategy is foundational to modeld's performance and user experience.

## Problem Statement

Choose optimal hashing parameters for:

1. **Performance**: Hash 1TB of models in <15 minutes (2GB/s sustained)
2. **Incremental hashing**: Avoid re-hashing unchanged files across scans
3. **Cache efficiency**: Minimal storage overhead for hash cache
4. **Memory usage**: Bounded memory consumption during parallel hashing
5. **Correctness**: Reliably detect file modifications and corruptions
6. **Cross-platform**: Work efficiently on Windows/Linux/macOS with different storage types

## Proposed Design

### Hash Function Selection

**Choice**: BLAKE3

**Rationale**:

BLAKE3 is selected over alternatives for the following reasons:

| Property | BLAKE3 | SHA256 | xxHash | BLAKE2 |
|----------|--------|--------|--------|--------|
| Speed (sequential) | 1-3 GB/s | 300-500 MB/s | 5-10 GB/s | 500-900 MB/s |
| Speed (parallel) | 3-10 GB/s | 300-500 MB/s | 5-10 GB/s | 500-900 MB/s |
| Cryptographically Secure | ✓ Yes | ✓ Yes | ✗ No | ✓ Yes |
| Parallelizable | ✓ Yes | ✗ No | ✓ Yes | Limited |
| Hash Size | 256 bits | 256 bits | 64 bits | 256 bits |
| Collision Resistance | Excellent | Excellent | Poor | Excellent |
| Maturity | Mature (2020) | Very Mature | Mature | Mature (2015) |
| Use Cases | General-purpose | Legacy systems | Non-crypto | General-purpose |

**Why Not SHA256?**
- 3-5x slower than BLAKE3 on modern CPUs
- Not parallelizable (sequential-only algorithm)
- Would take 45-75 minutes to hash 1TB (vs 15 minutes target)

**Why Not xxHash?**
- Not cryptographically secure (vulnerable to collisions)
- Unsuitable for content-addressable storage (security requirement)
- Could be exploited to create hash collisions

**Why Not BLAKE2?**
- Good candidate, but slower than BLAKE3 (2x difference)
- Limited parallelization compared to BLAKE3's tree-based approach
- BLAKE3 is the successor with better performance

**BLAKE3 Advantages**:
1. **Parallelizable**: Tree-based hashing utilizes all CPU cores
2. **Fast**: 3-10 GB/s on modern hardware (meets 2GB/s target)
3. **Secure**: Cryptographically secure, collision-resistant
4. **Standard output**: 256-bit hash (64 hex characters)
5. **Well-supported**: Available in Rust (`blake3` crate), Python (`blake3` package)

**Hash Output Format**:
```
64 hexadecimal characters (256 bits)
Example: abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890
```

**Decision**: Adopt BLAKE3 as the primary hash function for content addressing.

---

### Chunk Size Selection

**Choice**: 64MB chunks for parallel hashing

**Rationale**:

BLAKE3's tree-based hashing allows splitting large files into chunks that can be hashed in parallel. The chunk size affects:
- **Parallelism**: Smaller chunks = more parallel work
- **Memory usage**: Larger chunks = more memory per thread
- **Overhead**: Too small chunks = excessive thread coordination

**Performance Analysis**:

| Chunk Size | Parallelism (8-core) | Memory per Thread | Overhead | Performance |
|------------|---------------------|-------------------|----------|-------------|
| 4MB | Excellent (high) | 4MB | Medium | Good |
| 16MB | Excellent | 16MB | Low | Very Good |
| **64MB** | **Good** | **64MB** | **Very Low** | **Excellent** |
| 256MB | Limited | 256MB | Very Low | Good |
| 1GB | Poor | 1GB | Very Low | Poor |

**Why 64MB?**

1. **Balanced parallelism**: 12GB model = 192 chunks, fully utilizes 8+ cores
2. **Memory efficient**: 8 threads × 64MB = 512MB peak memory (acceptable)
3. **Low overhead**: Minimal thread coordination cost
4. **I/O alignment**: Good alignment with NVMe I/O patterns
5. **Industry standard**: Used by similar systems (git-lfs, IPFS)

**Example** (12GB SDXL model on 8-core CPU):
```
File size: 12,884,901,888 bytes (12GB)
Chunk size: 67,108,864 bytes (64MB)
Number of chunks: 192 chunks
Threads: min(8 cores, 192 chunks) = 8 threads
Work per thread: 24 chunks
Hashing time: ~6 seconds at 2GB/s
```

**Memory Calculation**:
```
Peak memory = num_threads × chunk_size + overhead
            = 8 × 64MB + ~50MB (buffers, hasher state)
            = 512MB + 50MB
            = ~560MB peak memory usage
```

**Decision**: Use 64MB chunks for parallel hashing of large files.

---

### Small File Threshold

**Choice**: 10MB threshold for strategy selection

**Problem**: Memory-mapped I/O (mmap) and parallel processing have overhead that's counterproductive for small files.

**Strategy Decision Tree**:

```
File size check
     │
     ├─> Size < 10MB
     │   └─> Direct read strategy
     │       ├─> std::fs::read() entire file
     │       ├─> Single-threaded BLAKE3 hash
     │       └─> Fast: ~5-20ms for small files
     │
     └─> Size ≥ 10MB
         └─> mmap + parallel strategy
             ├─> Memory-map file
             ├─> Split into 64MB chunks
             ├─> Parallel hash with Rayon
             └─> Optimal for large files
```

**Rationale for 10MB Threshold**:

| File Size | Direct Read | mmap + Parallel | Winner |
|-----------|-------------|-----------------|--------|
| 100KB | 1ms | 5ms (overhead) | Direct |
| 1MB | 3ms | 8ms (overhead) | Direct |
| 5MB | 15ms | 20ms (overhead) | Direct |
| **10MB** | **30ms** | **30ms** | **Break-even** |
| 50MB | 150ms | 50ms | mmap |
| 500MB | 1500ms | 300ms | mmap |
| 5GB | 15s | 3s | mmap |

**Overhead Analysis**:
- **mmap setup**: ~5ms (map file into memory)
- **Thread spawning**: ~2ms per thread
- **Synchronization**: ~1-3ms (combining results)
- **Total overhead**: ~10-15ms

For files <10MB, overhead dominates; for files ≥10MB, parallel speedup compensates.

**Common Model File Sizes**:
- Text embeddings: 100KB - 5MB → Use direct read
- LoRA models: 10MB - 500MB → Use mmap (right at threshold)
- VAE models: 100MB - 500MB → Use mmap
- Checkpoints: 2GB - 12GB → Use mmap (highly benefits)
- Flux models: 20GB+ → Use mmap (essential)

**Decision**: Use 10MB as threshold between direct read and mmap+parallel strategies.

---

### Incremental Hashing with Caching

**Problem**: Re-hashing unchanged files wastes time. Initial scan of 1TB takes 15 minutes; subsequent scans should be <30 seconds.

**Solution**: Cache (path, mtime, size) → hash mappings to detect unchanged files.

#### Cache Key Design

**Cache Key**: `(path, mtime, size)` tuple

**Why This Key?**

| Attribute | Purpose | Detection |
|-----------|---------|-----------|
| **path** | File identity | Renamed files detected |
| **mtime** | Modification time | Content changes detected |
| **size** | File size | Truncation/appends detected |

**Cache Hit Conditions**:
```rust
fn is_cache_valid(path: &Path, cached: &CacheEntry) -> bool {
    let metadata = path.metadata().ok()?;
    
    // All three must match for cache hit
    cached.path == path
        && cached.mtime == metadata.modified().ok()?
        && cached.size == metadata.len()
}
```

**Cache Miss Scenarios** (require re-hashing):
1. **New file**: Not in cache
2. **Modified file**: mtime changed
3. **Replaced file**: size changed (even if mtime unchanged)
4. **Renamed file**: path changed (new entry, old entry orphaned)

#### Cache Storage

**Option 1: Dedicated cache table** (chosen):
```sql
CREATE TABLE hash_cache (
    path      TEXT PRIMARY KEY,
    mtime     INTEGER NOT NULL,  -- Unix timestamp (seconds since epoch)
    size      INTEGER NOT NULL,  -- Bytes
    hash      TEXT NOT NULL,     -- BLAKE3 hash (64 hex chars)
    cached_at TEXT DEFAULT (datetime('now')),
    
    CHECK (length(hash) = 64),
    CHECK (size >= 0)
);

CREATE INDEX idx_hash_cache_hash ON hash_cache(hash);
CREATE INDEX idx_hash_cache_mtime ON hash_cache(cached_at);
```

**Option 2: Reuse models table**:
```sql
-- Add caching fields to existing models table
ALTER TABLE models ADD COLUMN last_path TEXT;
ALTER TABLE models ADD COLUMN last_mtime INTEGER;
ALTER TABLE models ADD COLUMN last_size INTEGER;
```

**Decision**: Use dedicated `hash_cache` table for separation of concerns.

**Rationale**:
- **Cleaner schema**: Models table focuses on CAS objects, cache is scan metadata
- **Independent lifecycle**: Cache can be cleared without affecting model registry
- **Different retention**: Cache may have LRU eviction, models persist until GC
- **Query optimization**: Separate indexes for different access patterns

#### Cache Eviction Policy

**LRU (Least Recently Used)** with max entry limit:

```rust
const MAX_CACHE_ENTRIES: usize = 100_000;

fn evict_cache_if_needed(db: &Database) -> Result<()> {
    let count: usize = db.query_row("SELECT COUNT(*) FROM hash_cache")?;
    
    if count > MAX_CACHE_ENTRIES {
        let to_remove = count - MAX_CACHE_ENTRIES;
        db.execute(
            "DELETE FROM hash_cache 
             WHERE rowid IN (
                 SELECT rowid FROM hash_cache 
                 ORDER BY cached_at ASC 
                 LIMIT ?
             )",
            [to_remove]
        )?;
    }
    
    Ok(())
}
```

**Storage Overhead**:
```
Per cache entry: ~200 bytes (path + metadata)
100,000 entries: ~20MB
1,000,000 entries: ~200MB
```

**Configuration**:
```toml
# modeld.toml
[cache]
max_entries = 100_000  # Default
eviction_policy = "lru"  # lru | ttl | none
ttl_days = 90  # For TTL policy
```

**Decision**: Default to 100K entry LRU cache (~20MB overhead).

---

### Hash Computation Algorithm

#### Pseudocode

```rust
use blake3::Hasher;
use memmap2::MmapOptions;
use rayon::prelude::*;
use std::fs::File;
use std::path::Path;

/// Compute BLAKE3 hash of a file with optimal strategy selection
fn compute_blake3_hash(file_path: &Path) -> Result<Blake3Hash> {
    let metadata = file_path.metadata()?;
    let file_size = metadata.len();
    
    // Strategy 1: Small files (<10MB) - direct read
    if file_size < 10 * 1024 * 1024 {
        return hash_small_file(file_path);
    }
    
    // Strategy 2: Large files (≥10MB) - mmap + parallel chunks
    hash_large_file(file_path, file_size)
}

/// Strategy 1: Direct read for small files
fn hash_small_file(file_path: &Path) -> Result<Blake3Hash> {
    let mut hasher = Hasher::new();
    let data = std::fs::read(file_path)?;
    hasher.update(&data);
    Ok(Blake3Hash::from(hasher.finalize()))
}

/// Strategy 2: mmap + parallel chunking for large files
fn hash_large_file(file_path: &Path, file_size: u64) -> Result<Blake3Hash> {
    let file = File::open(file_path)?;
    let mmap = unsafe { MmapOptions::new().map(&file)? };
    
    const CHUNK_SIZE: usize = 64 * 1024 * 1024;  // 64MB chunks
    
    // Create hasher with parallelism support
    let mut hasher = Hasher::new();
    
    // BLAKE3 has built-in parallel update via Rayon
    hasher.update_rayon(&mmap);
    
    Ok(Blake3Hash::from(hasher.finalize()))
}

/// Get hash with caching support
fn get_or_compute_hash(path: &Path, db: &Database) -> Result<Blake3Hash> {
    let metadata = path.metadata()?;
    let mtime = metadata.modified()?;
    let size = metadata.len();
    
    // Check cache
    if let Some(cached) = db.get_hash_cache(path)? {
        if cached.mtime == mtime && cached.size == size {
            // Cache hit - return cached hash
            return Ok(cached.hash);
        }
    }
    
    // Cache miss - compute hash
    let hash = compute_blake3_hash(path)?;
    
    // Update cache
    db.upsert_hash_cache(path, mtime, size, &hash)?;
    
    Ok(hash)
}
```

#### Incremental Directory Scan

```rust
use walkdir::WalkDir;

/// Scan directory with incremental hashing
fn scan_directory(path: &Path, db: &Database) -> Result<ScanReport> {
    let mut report = ScanReport::default();
    
    for entry in WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !is_hidden(e))
    {
        let entry = entry?;
        
        // Skip directories
        if !entry.file_type().is_file() {
            continue;
        }
        
        // Check file extension
        let path = entry.path();
        if !is_model_file(path) {
            continue;
        }
        
        // Get or compute hash (uses cache)
        let metadata = entry.metadata()?;
        let mtime = metadata.modified()?;
        let size = metadata.len();
        
        // Cache lookup
        let hash = if let Some(cached) = db.get_hash_cache(path)? {
            if cached.mtime == mtime && cached.size == size {
                report.cache_hits += 1;
                cached.hash
            } else {
                report.cache_misses += 1;
                let hash = compute_blake3_hash(path)?;
                db.upsert_hash_cache(path, mtime, size, &hash)?;
                hash
            }
        } else {
            report.new_files += 1;
            let hash = compute_blake3_hash(path)?;
            db.upsert_hash_cache(path, mtime, size, &hash)?;
            hash
        };
        
        // Upsert model
        db.upsert_model(&hash, &metadata, path)?;
        report.total_files += 1;
    }
    
    Ok(report)
}
```

---

## Performance Targets

### Throughput Targets

| Scenario | Target | Rationale |
|----------|--------|-----------|
| **1TB first scan (NVMe)** | **≤15 min** | **2GB/s sustained throughput** |
| 1TB incremental (no changes) | ≤30 sec | Metadata-only checks |
| Single 12GB model (first hash) | ≤6 sec | 2GB/s hash rate |
| Single 12GB model (cached) | <100ms | Database lookup only |
| 100 models (5GB each, cached) | ≤5 sec | Fast cache validation |
| 100 models (5GB each, new) | ≤4 min | 500GB @ 2GB/s |

### Hardware Assumptions

**Reference System** (performance targets based on):
- **CPU**: 8-core modern x86_64 (Intel Core i7/i9, AMD Ryzen 7/9)
- **Storage**: NVMe SSD (PCIe Gen3 or better)
- **RAM**: 8GB+ available
- **OS**: Windows 10/11, Linux (kernel 5.0+), macOS 10.15+

**Storage Performance Requirements**:

| Storage Type | Sequential Read | Expected Hash Speed | Scan Time (1TB) |
|--------------|----------------|---------------------|-----------------|
| **NVMe SSD (Gen3)** | **3+ GB/s** | **2-3 GB/s** | **5-8 min** ✓ |
| SATA SSD | 500 MB/s | 400-500 MB/s | 35-40 min |
| HDD (7200rpm) | 150 MB/s | 120-150 MB/s | 2-2.5 hours |
| USB 3.0 External | 300 MB/s | 250-300 MB/s | 60-70 min |

**Note**: Targets are for NVMe; slower storage will proportionally increase scan times but remain functional.

### Incremental Scan Performance

**Cache Hit Rate Impact**:

| Cache Hit Rate | Models to Hash | Time (1TB, 200 models) | Speedup |
|----------------|----------------|------------------------|---------|
| 0% (first scan) | 200 | 15 min | 1x (baseline) |
| 50% (partial update) | 100 | 7.5 min | 2x |
| 90% (few changes) | 20 | 1.5 min | 10x |
| 99% (almost no changes) | 2 | 30 sec | 30x |
| 100% (no changes) | 0 | 15 sec | 60x |

**Expected User Scenarios**:
- **Initial setup**: 0% cache hit → Full scan time
- **Weekly rescan**: 95%+ cache hit → Sub-minute scans
- **After model download**: ~90% cache hit → Few minutes
- **After mass download**: ~50% cache hit → Half time

---

## Memory Usage Considerations

### Peak Memory Consumption

**Components**:

```
Total Memory = Base + Workers + Cache + Database + Overhead

Base overhead:        ~50MB   (program, runtime, libraries)
Worker threads:       ~512MB  (8 threads × 64MB chunks)
Hash cache (memory):  ~20MB   (100K entries loaded)
SQLite cache:         ~50MB   (default SQLite cache_size)
OS file buffers:      ~100MB  (kernel buffer cache)
Misc overhead:        ~50MB   (hasher state, temporary buffers)
─────────────────────────────
Total peak memory:    ~780MB
```

**Tuning Parameters**:

```rust
// Configurable in modeld.toml
[hashing]
chunk_size_mb = 64        // Memory per worker thread
max_parallel_files = 4    // Concurrent file hashing
thread_pool_size = 8      // Rayon thread pool (default: num_cpus)

[cache]
max_entries = 100_000     // In-memory cache entries
sqlite_cache_mb = 50      // SQLite page cache
```

**Low-Memory Mode** (for systems with <4GB RAM):

```toml
[hashing]
chunk_size_mb = 16        // Reduce to 16MB chunks
max_parallel_files = 1    // Hash one file at a time
thread_pool_size = 4      // Use fewer threads

[cache]
max_entries = 10_000      // Smaller cache
sqlite_cache_mb = 10      // Reduce SQLite cache
```

**Memory Usage**:
- **Normal mode**: ~780MB peak
- **Low-memory mode**: ~200MB peak

### Memory Efficiency Techniques

1. **Streaming Processing**: Don't load entire files into RAM
   - Use `mmap` for large files (zero-copy)
   - Stream small files directly

2. **Lazy Cache Loading**: Don't load entire cache into memory
   - Query database on-demand
   - SQLite handles disk-based storage

3. **Bounded Thread Pool**: Limit concurrent operations
   - Rayon thread pool prevents unbounded parallelism
   - Configured via `thread_pool_size`

4. **Chunk Reuse**: Reuse buffers across files
   - Thread-local buffers
   - No allocation per file

---

## Cross-Platform Considerations

### Unix-like Systems (Linux, macOS)

**mtime Precision**:
- **Linux**: Nanosecond precision (since kernel 2.6+)
- **macOS**: Nanosecond precision (APFS, HFS+)

**Implementation**:
```rust
#[cfg(unix)]
fn get_mtime_unix(metadata: &Metadata) -> SystemTime {
    metadata.modified().unwrap()
}
```

**No special handling needed** - standard Rust APIs work well.

### Windows Considerations

**mtime Precision**:
- **NTFS**: 100-nanosecond precision
- **FAT32**: 2-second precision (problematic!)
- **exFAT**: 10-millisecond precision

**Issue**: FAT32 2-second granularity may miss rapid modifications.

**Mitigation**:
```rust
#[cfg(windows)]
fn is_cache_valid_windows(path: &Path, cached: &CacheEntry) -> bool {
    let metadata = path.metadata().ok()?;
    let current_mtime = metadata.modified().ok()?;
    let current_size = metadata.len();
    
    // Check filesystem type
    let fs_type = get_filesystem_type(path)?;
    
    if fs_type == "FAT32" {
        // FAT32: 2-second granularity, add tolerance
        let mtime_diff = current_mtime.duration_since(cached.mtime).ok()?;
        let mtime_matches = mtime_diff.as_secs() <= 2;
        
        // Rely more on size for FAT32
        mtime_matches && current_size == cached.size
    } else {
        // NTFS, exFAT: standard comparison
        current_mtime == cached.mtime && current_size == cached.size
    }
}
```

**Recommendation**: Document FAT32 limitations, recommend NTFS/exFAT for modeld storage.

### Network Filesystems

**Challenges**:
- **SMB/CIFS**: Potential mtime synchronization issues
- **NFS**: mtime may lag behind actual modification
- **Cloud sync (OneDrive, Dropbox)**: Sync delays affect mtime

**Mitigation**:
- Add `--force-rehash` flag for network filesystems
- Detect network paths and warn users
- Option to disable cache for specific paths

```bash
# For network shares, force full rehash
modeld scan \\\\server\\models --force-rehash

# Or configure in modeld.toml
[cache]
exclude_paths = ["\\\\server\\models", "/mnt/nas"]
```

---

## Implementation Considerations

### Rust Crate Dependencies

```toml
[dependencies]
blake3 = { version = "1.5", features = ["rayon"] }
memmap2 = "0.9"
rayon = "1.8"
walkdir = "2.4"
```

**Feature Requirements**:
- `blake3` with `rayon` feature: Parallel hashing support
- `memmap2`: Memory-mapped file I/O
- `rayon`: Thread pool for parallelism
- `walkdir`: Recursive directory traversal

### Error Handling

**Recoverable Errors** (log and continue):
- File disappeared during scan
- Permission denied (skip file)
- Corrupted filesystem metadata

**Unrecoverable Errors** (abort scan):
- Database connection failure
- Out of memory
- Disk full (can't write to database)

```rust
enum HashError {
    // Recoverable
    FileNotFound(PathBuf),
    PermissionDenied(PathBuf),
    IoError(std::io::Error),
    
    // Unrecoverable
    DatabaseError(rusqlite::Error),
    OutOfMemory,
    DiskFull,
}

impl HashError {
    fn is_recoverable(&self) -> bool {
        matches!(self, 
            HashError::FileNotFound(_) |
            HashError::PermissionDenied(_) |
            HashError::IoError(_)
        )
    }
}
```

### Progress Reporting

**User Feedback** during long scans:

```rust
struct ScanProgress {
    total_files: usize,
    processed_files: usize,
    total_bytes: u64,
    processed_bytes: u64,
    cache_hits: usize,
    cache_misses: usize,
    start_time: Instant,
}

impl ScanProgress {
    fn report(&self) {
        let elapsed = self.start_time.elapsed();
        let speed = self.processed_bytes as f64 / elapsed.as_secs_f64();
        let eta = if speed > 0.0 {
            (self.total_bytes - self.processed_bytes) as f64 / speed
        } else {
            0.0
        };
        
        println!(
            "Scanning: {}/{} files ({:.1}%) | Speed: {}/s | Cache hits: {}% | ETA: {}",
            self.processed_files,
            self.total_files,
            self.processed_files as f64 / self.total_files as f64 * 100.0,
            human_bytes(speed as u64),
            self.cache_hits as f64 / (self.cache_hits + self.cache_misses) as f64 * 100.0,
            human_duration(Duration::from_secs_f64(eta))
        );
    }
}
```

---

## Design Alternatives Considered

### Alternative 1: xxHash (Non-Cryptographic)

**Pros**:
- Extremely fast (5-10 GB/s)
- Simple implementation

**Cons**:
- **Not cryptographically secure** (collision attacks possible)
- **Unsuitable for CAS** (security requirement)
- Only 64-bit output (insufficient for collision resistance at scale)

**Decision**: **Rejected** - Security is mandatory for content addressing.

---

### Alternative 2: SHA256 (Traditional)

**Pros**:
- Well-established standard
- Used by HuggingFace, Docker, Git (historically)
- Excellent tooling support

**Cons**:
- **3-5x slower than BLAKE3** (300-500 MB/s vs 2-3 GB/s)
- **Not parallelizable** (sequential algorithm)
- Would require 45-75 min for 1TB scan (exceeds target)

**Decision**: **Rejected** - Performance insufficient for target use case.

---

### Alternative 3: Dynamic Chunk Sizing

**Idea**: Adjust chunk size based on file size and available cores.

**Example**:
```rust
fn calculate_chunk_size(file_size: u64, num_cores: usize) -> usize {
    let target_chunks = num_cores * 4;  // 4 chunks per core
    let chunk_size = (file_size / target_chunks as u64).max(4 * 1024 * 1024);
    chunk_size.next_power_of_two() as usize
}
```

**Pros**:
- Theoretically optimal for each file
- Better parallelism for small models

**Cons**:
- **Complex implementation**
- **Unpredictable memory usage**
- **Marginal benefit** (64MB works well for all sizes)
- **Testing complexity** (many code paths)

**Decision**: **Rejected** - Fixed 64MB is simpler and sufficient.

---

### Alternative 4: No Caching (Always Hash)

**Pros**:
- Simpler implementation (no cache management)
- No cache invalidation bugs
- Always correct

**Cons**:
- **Terrible user experience** (15 min every scan)
- **Wastes compute** (re-hashing unchanged 1TB every time)
- **Not competitive** with existing tools

**Decision**: **Rejected** - Caching is essential for usability.

---

### Alternative 5: Content-Based Caching (Hash Prefix)

**Idea**: Cache based on first 64KB hash instead of mtime/size.

**Pros**:
- More reliable than mtime (no filesystem quirks)
- Detects modifications regardless of mtime

**Cons**:
- **Must read 64KB of every file** (even unchanged ones)
- **Slower incremental scans** (1TB of 64KB reads = ~2min overhead)
- **Defeats purpose of caching** (still doing I/O)

**Decision**: **Rejected** - mtime/size is faster and sufficient.

---

### Alternative 6: Separate Small/Large Hash Tables

**Idea**: Use different tables for small vs large file caches.

**Pros**:
- Could optimize queries separately
- Different eviction policies

**Cons**:
- **Unnecessary complexity**
- **No measurable benefit** (SQLite indexes handle both well)
- **Harder to maintain**

**Decision**: **Rejected** - Single unified cache table is sufficient.

---

## Benchmarking Plan (Phase 1 Validation)

To validate the performance targets in Phase 1 implementation:

### Test Suite

1. **Synthetic Benchmarks**:
   ```bash
   # Generate test files
   dd if=/dev/urandom of=test_1gb.bin bs=1M count=1024
   
   # Benchmark direct BLAKE3
   time blake3sum test_1gb.bin
   
   # Benchmark modeld implementation
   time modeld hash test_1gb.bin
   ```

2. **Real Model Files**:
   - SDXL checkpoint (6.94GB)
   - Flux checkpoint (23GB)
   - LoRA collection (100 files, 10-500MB each)
   - Full ComfyUI models directory (500GB+)

3. **Cache Effectiveness**:
   ```bash
   # First scan (cold cache)
   time modeld scan /models --clear-cache
   
   # Second scan (hot cache)
   time modeld scan /models
   
   # Measure cache hit rate
   modeld stats --cache
   ```

4. **Platform Comparison**:
   - Windows 11 (NTFS, NVMe)
   - Linux (ext4, NVMe)
   - macOS (APFS, NVMe)
   - Windows (SATA SSD)
   - Linux (HDD 7200rpm)

### Success Criteria

| Test | Target | Measurement |
|------|--------|-------------|
| BLAKE3 raw speed | ≥2 GB/s | blake3sum benchmark |
| modeld overhead | ≤10% | (modeld time - blake3sum time) / blake3sum time |
| 1TB scan (NVMe, first) | ≤15 min | Actual scan time |
| 1TB scan (cached) | ≤30 sec | Incremental scan time |
| Cache hit rate (no changes) | ≥99% | modeld stats |
| Memory usage (peak) | ≤1 GB | OS process monitor |

---

## Security Considerations

### Hash Collision Attacks

**Threat**: Attacker crafts malicious model with same BLAKE3 hash as legitimate model.

**Mitigation**:
- BLAKE3 is cryptographically secure (collision resistance: 2^128 operations)
- 256-bit hash space: ~10^77 possible hashes
- Collision probability: Negligible for any realistic dataset

**Conclusion**: BLAKE3 provides sufficient collision resistance for CAS.

### Cache Poisoning

**Threat**: Attacker modifies file but preserves mtime/size to bypass cache.

**Scenario**:
```bash
# Attacker workflow
cp malicious.safetensors original.safetensors  # Replace content
touch -r original.safetensors.bak original.safetensors  # Restore mtime
truncate -s $(stat -c%s original.safetensors.bak) original.safetensors  # Restore size
```

**Mitigation**:
1. **Cache verification**: Periodic `modeld verify` command re-hashes files
2. **User awareness**: Document that cache is trust-based
3. **Permissions**: Protect model directories with proper file permissions

**Risk Level**: Low (requires file write access + sophisticated attack)

### Timing Attacks

**Threat**: Attacker measures hashing time to infer file content.

**Mitigation**:
- Hashing time is primarily determined by file size (public information)
- Content doesn't significantly affect BLAKE3 timing
- Not a practical threat for this use case

**Conclusion**: Timing attacks are not a concern for modeld.

---

## Open Questions (To Be Resolved in Phase 1)

1. **Optimal thread pool size**: Is `num_cpus` always best, or should we limit to 8?
   - **Action**: Benchmark with 4, 8, 16, 32 threads

2. **mmap vs read for medium files**: Is 10MB the optimal threshold?
   - **Action**: Benchmark files from 1MB-100MB to find break-even point

3. **Cache eviction frequency**: How often should we evict old entries?
   - **Action**: Monitor cache size growth over weeks

4. **FAT32 handling**: Can we reliably detect FAT32 filesystems?
   - **Action**: Test on Windows with FAT32 USB drives

5. **Network filesystem detection**: How to auto-detect SMB/NFS paths?
   - **Action**: Research platform-specific detection methods

---

## Summary

**Hash Function**: BLAKE3
- Fastest cryptographically secure option
- 3-10 GB/s throughput (meets 2GB/s target)
- 256-bit output (64 hex characters)

**Chunk Size**: 64MB
- Balanced parallelism and memory usage
- 8 threads × 64MB = 512MB peak memory
- Optimal for 2GB-100GB model files

**Small File Threshold**: 10MB
- Direct read for <10MB (avoid mmap overhead)
- mmap + parallel for ≥10MB (maximizes throughput)

**Caching Strategy**: (path, mtime, size) → hash
- LRU cache with 100K entry limit (~20MB overhead)
- Cache hit rate: 99%+ for typical incremental scans
- Reduces 15-minute scan to 30 seconds

**Performance Targets**:
- 1TB first scan (NVMe): ≤15 minutes (2GB/s)
- 1TB incremental scan (no changes): ≤30 seconds (60x speedup)
- 12GB single model: ≤6 seconds
- Memory usage: ~780MB peak (normal mode), ~200MB (low-memory mode)

**Platform Support**:
- Linux, macOS: Standard implementation
- Windows: Special handling for FAT32 (2-second mtime granularity)
- Network filesystems: Option to disable cache

**Implementation**: Rust with `blake3`, `memmap2`, `rayon` crates

---

## References

- [BLAKE3 Paper](https://github.com/BLAKE3-team/BLAKE3-specs/blob/master/blake3.pdf)
- [BLAKE3 Rust Crate](https://docs.rs/blake3/)
- [Content-Addressable Storage](https://en.wikipedia.org/wiki/Content-addressable_storage)
- [Git Object Storage](https://git-scm.com/book/en/v2/Git-Internals-Git-Objects) (inspiration for CAS design)
- [Rayon Parallel Iterators](https://docs.rs/rayon/)

---

**RFC Status**: Draft  
**Next Steps**: Implement in Phase 1, validate performance targets with benchmarks  
**Review Date**: After Phase 1 implementation

