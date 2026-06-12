# Phase 1: Core CAS Implementation

**Duration**: 3-4 weeks  
**Status**: Starting  
**Goal**: Implement basic CAS storage, BLAKE3 hashing, file scanning, and CLI

---

## Task Breakdown

### 1. Initialize Repository Structure ⏳
**Estimated**: 2 hours

- [ ] Create Cargo workspace with initial crates
- [ ] Set up project structure (see RFC repo layout)
- [ ] Configure rustfmt, clippy
- [ ] Add .gitignore
- [ ] Create basic README.md

**Deliverable**: Working Rust workspace that compiles

---

### 2. Implement BLAKE3 Hashing Module ⏳
**Estimated**: 1 day

- [ ] Small file strategy (<10MB direct read)
- [ ] Large file strategy (mmap + 64MB chunks)
- [ ] Parallel hashing with rayon
- [ ] Hash caching (path, mtime, size → hash)
- [ ] Unit tests
- [ ] Basic benchmarks

**Deliverable**: `modeld-core/src/hash.rs` with all strategies

---

### 3. Build CAS Storage Layer ⏳
**Estimated**: 1 day

- [ ] Directory initialization (cas/blake3/{prefix}/)
- [ ] Prefix sharding implementation
- [ ] File storage (copy to CAS with immutability)
- [ ] File retrieval (construct path from hash)
- [ ] Path utilities
- [ ] Unit tests

**Deliverable**: `modeld-core/src/cas.rs` with storage operations

---

### 4. Create SQLite Database Layer ⏳
**Estimated**: 1.5 days

- [ ] Schema creation (models table to start)
- [ ] WAL mode configuration
- [ ] CRUD operations for models
- [ ] Connection management
- [ ] Migration system (basic)
- [ ] Unit tests

**Deliverable**: `modeld-core/src/db.rs` with database operations

---

### 5. Develop File Scanner ⏳
**Estimated**: 1.5 days

- [ ] Recursive directory traversal (walkdir)
- [ ] File extension filtering (.safetensors, .gguf, .ckpt, .pth, .bin)
- [ ] Hash computation with cache
- [ ] Progress reporting
- [ ] Database insertion
- [ ] Integration tests

**Deliverable**: `modeld-core/src/scanner.rs` with scanning logic

---

### 6. Build CLI Interface ⏳
**Estimated**: 2 days

- [ ] CLI framework setup (clap)
- [ ] `modeld init` - Initialize store
- [ ] `modeld scan <path>` - Scan directory
- [ ] `modeld status` - Show statistics
- [ ] `modeld hash <file>` - Compute hash
- [ ] Progress bars (indicatif)
- [ ] Error handling and user messages

**Deliverable**: `modeld-cli/src/main.rs` with working commands

---

### 7. Integration Testing & Validation ⏳
**Estimated**: 1.5 days

- [ ] End-to-end test: init → scan → status
- [ ] Test with real model files
- [ ] Cross-platform smoke tests
- [ ] Performance validation (hash speed)
- [ ] Documentation updates

**Deliverable**: Working Phase 1 MVP

---

## Success Criteria

- ✅ Can initialize a modeld store
- ✅ Can scan a directory and compute BLAKE3 hashes
- ✅ Hash cache works (mtime/size based)
- ✅ Models tracked in SQLite database
- ✅ CAS storage organized correctly (prefix sharding)
- ✅ CLI provides clear feedback and progress
- ✅ Hash performance ≥500MB/s (lower target for Phase 1, optimize later)

---

## Out of Scope (Phase 2+)

- Deduplication (Phase 2)
- Virtual filesystem (Phase 3)
- HuggingFace interception (Phase 4)
- Garbage collection (Phase 5)

---

## Dependencies

From Phase 0 design:
- RFC 0001: Storage Layout
- RFC 0002: Hash Strategy
- Database Schema: models table
- Repository Structure definition

---

## Next Steps

1. Create Cargo workspace
2. Implement hash module first (most critical for performance)
3. Build CAS layer
4. Add database
5. Implement scanner
6. Wire up CLI
7. Test and validate

Let's start coding! 🚀
