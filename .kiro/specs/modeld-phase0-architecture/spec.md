# Spec: modeld Phase 0 - Architecture Design & RFC

## Overview

**Feature Name**: modeld-phase0-architecture

**Type**: Foundation / Architecture Design

**Phase**: Phase 0 (of 6-phase project)

**Timeline**: 2-3 weeks

**Priority**: Critical (must complete before implementation)

## Problem Statement

modeld is an ambitious AI model management system - the "containerd + git-lfs + nix store" for the AI model world. Before writing any core implementation code, we need a comprehensive architecture design that:

1. **Defines the complete system architecture** - All components, their interactions, and data flows
2. **Addresses platform-specific challenges** - Especially Windows filesystem limitations
3. **Specifies data structures** - Database schema, file layouts, protocols
4. **Documents key algorithms** - Hashing, deduplication, crash recovery
5. **Establishes design principles** - Safety, performance, compatibility

**Why Phase 0 is Critical**:
- Complex cross-platform requirements (Windows/Linux/macOS)
- Multiple integration points (ComfyUI, Forge, A1111, HuggingFace)
- Safety-critical operations (file deduplication, data management)
- Performance requirements (TB-scale, ≥2GB/s hashing)
- Without clear design, implementation will be fragile and require rewrites

## Goals

### Primary Goals

1. **Complete 7 RFC Documents**:
   - RFC 0001: Storage Layout
   - RFC 0002: Hash Strategy
   - RFC 0003: Reference Model
   - RFC 0004: Deduplication Strategy
   - RFC 0005: Windows Compatibility (CRITICAL)
   - RFC 0006: Virtual FS
   - RFC 0007: HF Interception

2. **Define SQLite Schema v1**:
   - Complete database schema with all tables
   - Indexes for performance
   - Foreign keys and constraints
   - WAL mode configuration

3. **Establish Windows Compatibility Strategy**:
   - Address symlink privilege requirements
   - Handle cross-volume hardlink limitations
   - Define fallback strategies
   - Create user communication plan

4. **Document System Architecture**:
   - High-level component diagram
   - Data flow diagrams
   - CAS storage layout specification
   - Integration points with AI frontends

5. **Specify Key Algorithms**:
   - BLAKE3 hash computation
   - Two-phase commit protocol
   - Crash recovery mechanism
   - Reference counting and GC

### Secondary Goals

- Define technical stack (Rust crates, Python packages)
- Document repository structure
- Create glossary and reference materials
- Identify open questions for future phases

## Success Criteria

### Phase 0 is Complete When:

- [ ] All 7 RFCs documented and internally reviewed
- [ ] Windows compatibility strategy clear with no major unknowns
- [ ] SQLite schema v1 finalized and validated
- [ ] CAS layout spec documented with scalability analysis
- [ ] Transactional move protocol specified with crash recovery
- [ ] Hash strategy documented with performance targets
- [ ] Reference counting model detailed with GC algorithm
- [ ] Virtual FS design complete with frontend templates
- [ ] HF interception approach decided with compatibility matrix
- [ ] Team (or self) can "completely describe the system" without docs

### Quality Gates

**Completeness**: No major "TBD" or unresolved issues remain

**Clarity**: Technical reviewer can understand without asking questions

**Consistency**: No contradictions between RFCs or sections

**Implementability**: Sufficient detail for Phase 1 implementation to start

**Soundness**: Algorithms are correct, no race conditions, handles edge cases

## Scope

### In Scope (Phase 0)

✅ Architecture design and documentation
✅ RFC specification documents
✅ Database schema design
✅ Algorithm pseudocode
✅ Windows compatibility research
✅ Platform strategy (Windows/Linux/macOS)
✅ Technology stack selection
✅ Repository structure design
✅ Design rationale and alternatives

### Out of Scope (Phase 0)

❌ Rust implementation code
❌ Python package implementation
❌ CI/CD pipeline setup
❌ Performance benchmarks (only targets)
❌ User-facing documentation
❌ Test code implementation
❌ Build system configuration
❌ Docker containers
❌ Deployment automation

## Key Design Decisions

### 1. Content-Addressable Storage (CAS)

**Decision**: Use BLAKE3-addressed immutable object store with prefix sharding

**Rationale**:
- Eliminates duplication by design (same hash = same content)
- Immutability enables safe sharing (hardlinks, concurrent access)
- Prefix sharding scales to millions of models
- BLAKE3 is 3-5x faster than SHA256

**Alternatives Considered**:
- Flat directory → Rejected: doesn't scale
- Date-based sharding → Rejected: uneven distribution
- SHA256 hashing → Rejected: slower

### 2. SQLite for Metadata

**Decision**: Use SQLite with WAL mode for all metadata storage

**Rationale**:
- Zero deployment dependencies (embedded database)
- Excellent performance for read-heavy workloads
- WAL mode enables concurrent readers
- FTS5 for future full-text search

**Alternatives Considered**:
- PostgreSQL → Rejected: requires server
- JSON files → Rejected: no ACID, poor query performance
- Custom binary format → Rejected: reinventing the wheel

### 3. Two-Phase Commit for Dedup

**Decision**: Implement WAL-based two-phase commit protocol

**Rationale**:
- Ensures atomicity (all or nothing)
- Enables crash recovery
- Prevents data loss during dedup operations
- Standard distributed systems pattern

**Alternatives Considered**:
- In-place dedup → Rejected: no recovery
- Copy-on-write → Rejected: filesystem-specific
- Three-phase → Rejected: unnecessary complexity

### 4. Multi-Tier Windows Strategy

**Decision**: Graceful degradation with multiple fallback strategies

**Rationale**:
- Not all users can/will enable Developer Mode
- Cross-volume scenarios are common
- Better to work partially than fail completely
- Clear communication builds trust

**Alternatives Considered**:
- Require Admin always → Rejected: poor UX
- Windows-only release → Rejected: want cross-platform
- Ignore Windows → Rejected: largest user base

### 5. HF_HOME + Optional Monkey-Patch

**Decision**: Primary strategy is HF_HOME env variable, monkey-patch as fallback

**Rationale**:
- HF_HOME is stable across HF library updates
- Monkey-patching is fragile but covers edge cases
- Two-layer approach maximizes compatibility

**Alternatives Considered**:
- Only monkey-patch → Rejected: breaks with HF updates
- HTTP proxy → Rejected: complex, breaks HTTPS
- Fork HF libraries → Rejected: unsustainable

## Architecture Overview

### System Components

```
┌─────────────────────────────────────────────────────┐
│            AI Frontend Layer                         │
│   ComfyUI    Forge    A1111    InvokeAI    diffusers │
└──────────────┬───────────────────────┬───────────────┘
               │                       │
     HF Hook (Python)            Virtual Model FS
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

### Core Components

1. **CAS Storage Layer**: Immutable content-addressed object store
2. **Virtual FS Layer**: Hardlink/symlink facade for AI frontends
3. **Metadata Index**: SQLite database for models, aliases, refs, downloads
4. **Dedup Engine**: Duplicate detection and transactional migration
5. **HF Interception**: Python hook for HuggingFace download intercept
6. **Workflow Parser**: ComfyUI workflow dependency extraction (future)

## Technical Stack

### Core Technology

- **Language**: Rust (core daemon, CLI) + Python (HF hook)
- **Async Runtime**: Tokio
- **Database**: SQLite 3 with rusqlite
- **Hash Function**: BLAKE3
- **CLI Framework**: clap
- **Config Format**: TOML
- **File Monitoring**: notify crate
- **IPC**: Unix socket / Named pipe

### Why Rust?

- Zero-cost abstractions for performance
- Memory safety without GC pauses
- Excellent async ecosystem (Tokio)
- mmap support for efficient file hashing
- Strong type system prevents bugs
- Cross-platform support

### Why Python (for HF Hook)?

- Must integrate with Python AI ecosystem
- HuggingFace libraries are Python-native
- Cannot use Rust for ecosystem interception
- Minimal performance impact (intercept only)

## Database Schema Summary

### Core Tables

**models**: CAS central registry
- blake3_hash (PK)
- size_bytes, format, arch
- created_at, last_seen

**aliases**: Multiple paths → same hash
- model_hash (FK → models)
- path, frontend, alias_type
- Created via dedup or virtual FS

**refs**: Workflow references
- model_hash (FK → models)
- ref_source (workflow file path)
- ref_type (lora, checkpoint, vae)
- Prevents accidental deletion

**downloads**: HF download tracking
- source_url, sha256_hash, model_hash
- status, bytes_total, bytes_done
- Download deduplication

**wal_transactions**: Crash recovery
- tx_id, operation, status
- source_path, target_hash, metadata
- Two-phase commit protocol

## Performance Targets

| Metric | Target | Rationale |
|--------|--------|-----------|
| Hash throughput | ≥2GB/s | Match NVMe SSD bandwidth |
| First scan (1TB) | ≤15 min | Acceptable initial setup time |
| Incremental scan | ≤30 sec | Fast re-scan after changes |
| Duplicate detection | ≤1 sec | Already-scanned data, DB query only |
| Memory usage (scan) | ≤200MB | Efficient for TB-scale operations |
| Database operations | O(log n) | Indexed lookups, scales to millions |

## Risks & Mitigations

### Critical Risks

**Risk 1: Windows Symlink Limitations**
- **Impact**: High - affects 90% of Windows users
- **Mitigation**: Multi-tier strategy with graceful degradation
- **Status**: Addressed in RFC 0005

**Risk 2: Cross-Volume Hardlink Prohibition**
- **Impact**: Medium - common multi-disk setups
- **Mitigation**: Reference-only mode as fallback
- **Status**: Addressed in RFC 0005

**Risk 3: HF Monkey-Patch Fragility**
- **Impact**: Medium - could break with HF updates
- **Mitigation**: HF_HOME primary, monkey-patch fallback
- **Status**: Addressed in RFC 0007

### Medium Risks

**Risk 4: Performance Targets Unrealistic**
- **Impact**: Medium - affects UX
- **Mitigation**: Based on BLAKE3 benchmarks, validate in Phase 1
- **Status**: Accepted, needs validation

**Risk 5: Database Schema Evolution**
- **Impact**: Low - can migrate
- **Mitigation**: Use JSON fields for extensibility
- **Status**: Accepted

## Open Questions

Questions to be resolved during/after Phase 0:

1. **Chunk size optimization**: Is 64MB optimal for all hardware?
   → Answer: Benchmark in Phase 1

2. **Quarantine TTL**: Is 30 days the right default?
   → Answer: Make configurable, 30 days reasonable start

3. **Virtual FS overhead**: Does symlink indirection slow model loading?
   → Answer: Measure in Phase 2 integration tests

4. **HF SHA256 availability**: Do all downloads provide SHA256?
   → Answer: Research during Phase 3 implementation

5. **macOS APFS specifics**: Any APFS-specific issues?
   → Answer: Test on macOS in Phase 1

## Next Steps (Post Phase 0)

After completing Phase 0 design:

1. **RFC Review**: Internal or community review of all RFCs
2. **Prototype Key Components**:
   - BLAKE3 hash performance test
   - SQLite schema with sample data
   - Windows privilege detection test
3. **Begin Phase 1**: Core Scanner MVP implementation
4. **Setup CI/CD**: Multi-platform testing (Windows, Linux, macOS)
5. **User Documentation**: Setup guides, troubleshooting

## Document Organization

This spec consists of three documents:

1. **spec.md** (this file): High-level overview and summary
2. **design.md**: Complete technical design with all 7 RFCs
3. **requirements.md**: Detailed functional and non-functional requirements
4. **tasks.md**: Breakdown of implementation tasks

## References

- Project Plan: `modeld-project-plan.md`
- BLAKE3 Spec: https://github.com/BLAKE3-team/BLAKE3-specs
- HuggingFace Hub: https://huggingface.co/docs/huggingface_hub
- SQLite WAL: https://www.sqlite.org/wal.html
- Windows Symlinks: https://learn.microsoft.com/en-us/windows/win32/fileio/symbolic-links

---

*Spec Version: 1.0*
*Phase: Phase 0 - Architecture Design & RFC*
*Status: Ready for Implementation*
*Date: 2024*
