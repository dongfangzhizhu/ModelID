# Requirements: modeld Phase 0 - Architecture Design & RFC

## Overview

This document specifies the requirements for Phase 0 of the modeld project - the foundational architecture design phase that must be completed before any core implementation begins.

**Phase 0 Principle**: "Don't write core business code, only do system design."

## Functional Requirements

### FR1: RFC Documentation

**FR1.1**: Create RFC 0001 - Storage Layout
- **Description**: Document the CAS directory structure design
- **Acceptance Criteria**:
  - Prefix sharding strategy explained (2-char hex buckets)
  - Future OCI compatibility considerations documented
  - Chunk boundary planning for future Phase 6
  - Scalability analysis (up to millions of models)
  - Directory structure diagram included

**FR1.2**: Create RFC 0002 - Hash Strategy
- **Description**: Document BLAKE3 hash computation strategy
- **Acceptance Criteria**:
  - Hash function selection rationale (BLAKE3 vs alternatives)
  - Chunk size determination (64MB) with justification
  - Small file threshold (10MB) strategy
  - Incremental hashing approach (mtime + size caching)
  - Performance targets specified (≥2GB/s on NVMe)

**FR1.3**: Create RFC 0003 - Reference Model
- **Description**: Document the workflow → model reference tracking system
- **Acceptance Criteria**:
  - Explicit vs implicit reference types defined
  - Reference counting algorithm specified
  - GC trigger conditions documented
  - Quarantine mechanism (30-day grace period) detailed
  - Protection levels explained (PROTECTED → QUARANTINE → DELETED)

**FR1.4**: Create RFC 0004 - Deduplication Strategy
- **Description**: Document identical file deduplication approach
- **Acceptance Criteria**:
  - Canonical path selection algorithm defined
  - Two-phase commit protocol documented
  - WAL-based crash recovery mechanism specified
  - Dedup modes explained (interactive, dry-run, auto, report)
  - Error handling strategies documented

**FR1.5**: Create RFC 0005 - Windows Compatibility (CRITICAL)
- **Description**: Document Windows-specific compatibility strategies
- **Acceptance Criteria**:
  - Symlink privilege requirements documented
  - Cross-volume hardlink limitations addressed
  - Junction point fallback strategy specified
  - Reference-only mode for unprivileged environments
  - Link strategy decision tree included
  - Privilege detection approach documented
  - User setup instructions provided (Developer Mode)

**FR1.6**: Create RFC 0006 - Virtual FS
- **Description**: Document virtual filesystem for AI frontend integration
- **Acceptance Criteria**:
  - Virtual directory structure design documented
  - Hardlink/symlink directory layout specified
  - Frontend mount points defined (comfyui, forge, a1111)
  - Link strategy priority explained (hardlink > symlink > junction)
  - Category detection algorithm (checkpoint, lora, vae) included
  - Link refresh mechanism documented

**FR1.7**: Create RFC 0007 - HF Interception
- **Description**: Document HuggingFace ecosystem interception approach
- **Acceptance Criteria**:
  - monkeypatch vs HF_HOME comparison documented
  - Fake HF cache layout design specified
  - SHA256 ↔ BLAKE3 hash mapping strategy
  - Download deduplication flow documented
  - Python hook implementation approach
  - Compatibility testing matrix defined
  - Installation methods (3 options) documented

### FR2: Database Schema Design

**FR2.1**: Define SQLite Schema v1
- **Description**: Complete database schema for metadata storage
- **Acceptance Criteria**:
  - `models` table with all required fields
  - `aliases` table for path → hash mapping
  - `refs` table for workflow references
  - `downloads` table for HF download tracking
  - `wal_transactions` table for crash recovery
  - All indexes defined for performance
  - Foreign key relationships established
  - Check constraints for data validation
  - WAL mode configuration specified

**FR2.2**: Document Schema Rationale
- **Description**: Explain design decisions for database schema
- **Acceptance Criteria**:
  - Why SQLite (vs other databases)
  - Index selection rationale
  - Foreign key strategy
  - WAL mode benefits explained
  - Timestamp usage for audit trails

### FR3: Windows Compatibility Research

**FR3.1**: Document Windows Filesystem Limitations
- **Description**: Comprehensive analysis of Windows-specific constraints
- **Acceptance Criteria**:
  - Symlink privilege requirements documented
  - Developer Mode vs Admin privileges compared
  - Cross-volume hardlink prohibition explained
  - Junction point limitations (directory-only) noted
  - NTFS vs ReFS vs exFAT differences
  - Impact analysis (High/Medium/Low) for each limitation

**FR3.2**: Define Multi-Tier Link Strategy
- **Description**: Fallback strategies for different Windows scenarios
- **Acceptance Criteria**:
  - Same-volume strategy (hardlink)
  - Cross-volume with privileges (symlink)
  - Cross-volume without privileges (reference-only)
  - Privilege detection algorithm
  - Decision tree diagram
  - Testing matrix for all scenarios

**FR3.3**: User Communication Strategy
- **Description**: How to communicate limitations to users
- **Acceptance Criteria**:
  - Clear warning messages defined
  - Setup instructions for Developer Mode
  - Comparison table (Full vs Limited mode)
  - Troubleshooting section
  - Screenshots/diagrams for setup process

### FR4: Architecture Documentation

**FR4.1**: System Architecture Diagram
- **Description**: High-level system component diagram
- **Acceptance Criteria**:
  - All 6 core components shown
  - Component interactions illustrated
  - Data flow between layers
  - AI frontend integration points
  - CAS storage relationship

**FR4.2**: CAS Storage Layout Specification
- **Description**: Detailed CAS directory structure
- **Acceptance Criteria**:
  - Complete directory tree example
  - File naming conventions
  - Prefix sharding explanation
  - Immutability enforcement approach
  - Quarantine directory purpose

**FR4.3**: Data Flow Diagrams
- **Description**: Key operation flows documented
- **Acceptance Criteria**:
  - Initial scan & dedup flow
  - Deduplication execution flow
  - HF download interception flow
  - Each flow shows all major steps
  - Error handling paths included

### FR5: Algorithm Specifications

**FR5.1**: BLAKE3 Hash Computation Algorithm
- **Description**: Pseudocode for hash computation
- **Acceptance Criteria**:
  - Small file strategy (<10MB)
  - Large file strategy (mmap + parallel chunks)
  - Cache lookup logic
  - Performance optimization techniques
  - Memory usage considerations

**FR5.2**: Transactional Move Protocol
- **Description**: Two-phase commit protocol specification
- **Acceptance Criteria**:
  - Phase A (Prepare) steps detailed
  - Phase B (Commit) steps detailed
  - Crash recovery procedure
  - WAL record format
  - Atomicity guarantees explained
  - Rollback scenarios covered

**FR5.3**: Reference Counting Algorithm
- **Description**: How ref counts are computed and managed
- **Acceptance Criteria**:
  - Explicit reference counting
  - Implicit reference (aliases) counting
  - GC protection level determination
  - Quarantine trigger logic
  - Safe GC algorithm steps

**FR5.4**: Canonical Path Selection
- **Description**: Algorithm for selecting primary file in duplicate groups
- **Acceptance Criteria**:
  - Priority order defined (CAS > oldest > shortest > alphabetical)
  - Pseudocode provided
  - Tiebreaker logic
  - Deterministic behavior guaranteed

### FR6: Technical Stack Documentation

**FR6.1**: Rust Crates Specification
- **Description**: All required Rust dependencies listed
- **Acceptance Criteria**:
  - Core dependencies with versions
  - Feature flags specified
  - Platform-specific dependencies
  - Rationale for each major crate
  - No missing dependencies for Phase 0 design

**FR6.2**: Python Package Specification
- **Description**: modeld-hook package requirements
- **Acceptance Criteria**:
  - Project metadata (pyproject.toml)
  - Runtime dependencies
  - Dev dependencies
  - Version constraints
  - Installation methods

**FR6.3**: Repository Structure
- **Description**: Complete repository layout
- **Acceptance Criteria**:
  - All crate directories
  - Python package structure
  - Documentation directories
  - Test directories
  - CI/CD configuration
  - RFC document locations

## Non-Functional Requirements

### NFR1: Documentation Quality

**NFR1.1**: Completeness
- All design decisions must be documented with rationale
- No "TODO" or "TBD" in final Phase 0 documents
- All alternatives considered should be mentioned

**NFR1.2**: Clarity
- Anyone with systems programming background can understand the system
- Diagrams supplement text explanations
- Technical terms defined on first use

**NFR1.3**: Maintainability
- Documents use markdown format
- Easy to update as decisions evolve
- Version controlled in git

### NFR2: Design Soundness

**NFR2.1**: Correctness
- No logical inconsistencies between RFCs
- Database schema passes normalization checks
- Algorithms are sound (no race conditions, deadlocks)

**NFR2.2**: Completeness
- All edge cases addressed
- Error handling strategies defined
- Crash recovery scenarios covered

**NFR2.3**: Feasibility
- Design can be implemented in Rust + Python
- Performance targets are achievable
- Windows limitations have practical workarounds

### NFR3: Platform Coverage

**NFR3.1**: Windows Support
- All Windows-specific issues documented
- Fallback strategies defined for each limitation
- Developer Mode setup instructions provided
- Testing matrix covers all Windows scenarios

**NFR3.2**: Linux Support
- Standard Unix filesystem operations
- No distribution-specific dependencies
- Both ext4 and btrfs considered

**NFR3.3**: macOS Support
- APFS compatibility considered
- Any macOS-specific issues noted
- Unix-like operations should work

### NFR4: Performance Targets

**NFR4.1**: Hash Performance
- BLAKE3 hashing: ≥2GB/s on NVMe SSD
- Parallel chunk processing utilized
- Memory efficient for large files

**NFR4.2**: Database Performance
- WAL mode for concurrent access
- Indexes for O(log n) lookups
- Foreign key checks don't slow down inserts significantly

**NFR4.3**: Scalability
- CAS layout scales to millions of models
- Database can handle 10M+ records
- No O(n²) algorithms in critical paths

### NFR5: Safety & Reliability

**NFR5.1**: Data Safety
- Two-phase commit prevents data loss
- WAL ensures crash recovery
- Quarantine mechanism prevents accidental deletion
- Hash verification at every stage

**NFR5.2**: Atomicity
- File operations are atomic where possible
- Transaction boundaries clearly defined
- No partial states visible to users

**NFR5.3**: Consistency
- Database constraints enforce data integrity
- Foreign keys prevent orphaned records
- No race conditions in concurrent operations

## Constraints

### C1: No Implementation Code
- Phase 0 is design-only
- No Rust or Python code written (except pseudocode examples)
- Focus on specification and documentation

### C2: Time Constraints
- Phase 0 target: 2-3 weeks
- Must not delay Phase 1 unnecessarily
- Balance thoroughness with pragmatism

### C3: Technology Constraints
- Must use Rust for core (performance critical)
- Must use Python for HF hook (ecosystem compatibility)
- Must use SQLite (no server dependencies)
- Must use BLAKE3 (performance requirement)

### C4: Compatibility Constraints
- Must work on Windows 10/11
- Must work on major Linux distros
- Must work on macOS (nice to have)
- Must not require root/admin for basic operations

## Success Criteria

### Phase 0 Completion Checklist

- [ ] All 7 RFCs documented and reviewed
- [ ] Windows compatibility strategy clear and comprehensive
- [ ] SQLite schema v1 finalized
- [ ] CAS layout spec documented with rationale
- [ ] Transactional move protocol specified
- [ ] Hash strategy with performance targets
- [ ] Reference counting model detailed
- [ ] Virtual FS design complete
- [ ] HF interception approach decided
- [ ] Repository structure defined
- [ ] Technical stack specified

### Review Questions

Can someone read the design doc and:

1. **Understand the entire system?**
   - All components and their interactions
   - Data flows for major operations
   - Database schema and relationships

2. **Implement Phase 1 without ambiguity?**
   - File scanning algorithm clear
   - Hash computation strategy specified
   - Database operations defined

3. **Handle Windows edge cases?**
   - Privilege detection approach
   - Fallback strategies for each scenario
   - User communication plan

4. **Recover from crashes?**
   - WAL recovery procedure clear
   - All failure modes considered
   - Rollback strategies defined

5. **Make informed decisions?**
   - Rationale for major choices provided
   - Alternatives considered and documented
   - Tradeoffs explicitly stated

### Acceptance Criteria

**Design Document Quality**:
- Comprehensive: All 7 RFCs covered in detail
- Clear: Technical reviewer can understand without asking questions
- Complete: No major "TBD" or unresolved issues
- Consistent: No contradictions between sections
- Implementable: Sufficient detail for Phase 1 implementation

**Technical Soundness**:
- Database schema is normalized and efficient
- Algorithms are correct (no race conditions)
- Performance targets are realistic
- Windows workarounds are practical
- Error handling is comprehensive

**Deliverables**:
- design.md (this document) - complete and reviewed
- requirements.md (this document) - complete
- All 7 RFCs embedded in design.md
- Database schema SQL with indexes
- Key algorithm pseudocode
- Architecture diagrams
- Data flow diagrams

## Out of Scope (Phase 0)

The following are explicitly NOT required for Phase 0:

- [ ] No actual Rust code implementation
- [ ] No Python package implementation
- [ ] No CI/CD pipeline setup
- [ ] No performance benchmarks (just targets)
- [ ] No user-facing documentation (just internal design)
- [ ] No test code (design only)
- [ ] No build system configuration
- [ ] No Docker containers
- [ ] No deployment scripts

These will be addressed in subsequent phases.

## Dependencies

### Internal Dependencies
- None (Phase 0 is the foundation)

### External Dependencies
- Access to Windows machine for compatibility research
- Understanding of Rust ecosystem
- Understanding of Python ecosystem
- Familiarity with SQLite
- Knowledge of filesystem operations (hardlinks, symlinks)

## Risks & Mitigations

### Risk 1: Windows Compatibility Complexity
- **Risk**: Windows limitations may make design infeasible
- **Mitigation**: Define multiple fallback strategies
- **Status**: Addressed in RFC 0005

### Risk 2: Performance Targets Unrealistic
- **Risk**: 2GB/s hashing may not be achievable
- **Mitigation**: Based on BLAKE3 benchmarks, should be feasible on modern hardware
- **Status**: Needs validation in Phase 1 prototyping

### Risk 3: HF Interception Fragility
- **Risk**: Monkey-patching may break with HF updates
- **Mitigation**: Primary strategy is HF_HOME (stable), monkey-patch is fallback
- **Status**: Addressed in RFC 0007

### Risk 4: Database Schema Changes
- **Risk**: Schema may need changes after Phase 0
- **Mitigation**: Design with extensibility in mind (JSON metadata fields)
- **Status**: Accepted risk, v1 schema is best effort

### Risk 5: Phase 0 Taking Too Long
- **Risk**: Over-engineering the design phase
- **Mitigation**: 2-3 week timebox, focus on critical decisions
- **Status**: Actively managed

## Open Questions

Questions to be resolved during or after Phase 0:

1. **Chunk size optimization**: Is 64MB optimal across all hardware?
   - Answer: Needs benchmarking in Phase 1

2. **Quarantine TTL**: Is 30 days the right default?
   - Answer: Start with 30 days, make configurable

3. **Virtual FS performance**: Does symlink overhead affect model loading?
   - Answer: Measure in Phase 2

4. **HF SHA256 availability**: Do all HF downloads provide SHA256?
   - Answer: Research during Phase 3 implementation

5. **macOS APFS issues**: Are there APFS-specific problems?
   - Answer: Test on macOS in Phase 1

## Assumptions

1. **User Environment**:
   - Users have at least 10GB free space for CAS
   - Users can install Rust binaries
   - Users can install Python packages (pip)

2. **Hardware**:
   - Modern multi-core CPU (4+ cores)
   - SSD storage (HDD will be slower but functional)
   - At least 4GB RAM

3. **Software**:
   - Windows 10+ / Linux with kernel 4.0+ / macOS 10.15+
   - Python 3.8+
   - Rust 1.70+ for building from source

4. **AI Frontends**:
   - ComfyUI, Forge, A1111 use standard model paths
   - Model files are not encrypted or compressed
   - Standard file formats (safetensors, gguf, ckpt)

## Glossary

- **CAS**: Content-Addressable Storage - storage indexed by content hash
- **Hardlink**: Multiple directory entries pointing to same inode/file data
- **Symlink**: Symbolic link, pointer to another file path
- **Junction**: Windows directory symlink (doesn't require privileges)
- **BLAKE3**: Fast cryptographic hash function
- **WAL**: Write-Ahead Logging, crash recovery mechanism
- **HF**: HuggingFace, the AI model hosting platform
- **Dedup**: Deduplication, eliminating duplicate copies
- **GC**: Garbage Collection, removing unused objects
- **Quarantine**: Soft-delete state before permanent removal
- **RFC**: Request for Comments, design specification document

---

*Requirements Document Version: 1.0*
*Phase: Phase 0 - Architecture Design & RFC*
*Status: Complete*
*Date: 2024*
