# Implementation Plan: modeld Phase 0 - Architecture Design & RFC

## Overview

This implementation plan covers Phase 0 of the modeld project - the foundational architecture design phase. The goal is to create comprehensive RFC documentation and system design before any core implementation begins.

**Phase 0 Principle**: "Don't write core business code, only do system design."

## Tasks

- [x] 1. Create RFC 0001 - Storage Layout: Create comprehensive documentation for the CAS storage layout design including prefix sharding strategy, directory structure, design decisions justification, OCI compatibility planning, chunk boundary reservation, scalability analysis, and immutability enforcement approach. Document in `docs/rfcs/0001-storage-layout.md`. **Dependencies**: None. **Estimated Effort**: 4 hours.

- [x] 2. Create RFC 0002 - Hash Strategy: Document the BLAKE3 hashing strategy with performance optimization including hash function selection rationale, 64MB chunk size justification, 10MB small file threshold strategy, incremental hashing with mtime/size caching, pseudocode for hash computation, performance targets (≥2GB/s on NVMe), and memory usage considerations. Document in `docs/rfcs/0002-hash-strategy.md`. **Dependencies**: None. **Estimated Effort**: 4 hours.

- [x] 3. Create RFC 0003 - Reference Model: Design the reference tracking system to prevent accidental deletion including explicit reference types (workflow refs, user tags), implicit reference types (aliases, hardlinks), reference counting algorithm, GC protection levels (PROTECTED, QUARANTINE, DELETED), quarantine mechanism (30-day TTL), safe GC algorithm, and recovery procedures. Document in `docs/rfcs/0003-ref-model.md`. **Dependencies**: Task 8. **Estimated Effort**: 5 hours.

- [x] 4. Create RFC 0004 - Deduplication Strategy: Document the file deduplication approach with transactional safety including canonical path selection algorithm, two-phase commit protocol, WAL record format, crash recovery procedure, dedup modes (interactive, dry-run, auto, report), progress reporting format, and error handling strategies. Document in `docs/rfcs/0004-dedup-strategy.md`. **Dependencies**: Task 8. **Estimated Effort**: 6 hours.

- [x] 5. Create RFC 0005 - Windows Compatibility (CRITICAL): Comprehensive Windows compatibility research and strategy design including all Windows filesystem limitations, symlink privilege requirements research, cross-volume hardlink prohibition documentation, link strategy decision tree, privilege detection approach (pseudocode), fallback strategies for each scenario, user communication strategy (warnings, setup instructions), Developer Mode setup process, and testing matrix for Windows scenarios. Document in `docs/rfcs/0005-windows-compat.md`. **Dependencies**: None. **Estimated Effort**: 8 hours.

- [x] 6. Create RFC 0006 - Virtual FS: Design the virtual filesystem layer for AI frontend integration including virtual directory structure, hardlink/symlink directory layout, frontend mount points (comfyui, forge, a1111), link strategy priority rules, category detection algorithm (checkpoint, lora, vae), link refresh mechanism, and frontend template format. Document in `docs/rfcs/0006-virtual-fs.md`. **Dependencies**: Task 5. **Estimated Effort**: 5 hours.

- [x] 7. Create RFC 0007 - HF Interception: Document HuggingFace ecosystem interception approach including monkeypatch vs HF_HOME comparison, fake HF cache layout design, SHA256 ↔ BLAKE3 mapping strategy, download deduplication flow, Python hook implementation approach, compatibility testing matrix, and installation methods (3 options). Document in `docs/rfcs/0007-hf-interception.md`. **Dependencies**: Tasks 2, 8. **Estimated Effort**: 6 hours.

- [x] 8. Define SQLite Schema v1: Create complete database schema with all tables and indexes including `models`, `aliases`, `refs`, `downloads`, `wal_transactions` tables, all indexes for performance, foreign key relationships, check constraints, WAL mode settings, and schema rationale documentation. Document in design document. **Dependencies**: Tasks 3, 4, 7. **Estimated Effort**: 4 hours.

- [x] 9. Document System Architecture: Create high-level system architecture documentation including system architecture diagram (all 6 components), component interactions, data flow between layers, AI frontend integration points, CAS storage relationship, CAS storage layout specification, and immutability enforcement. Document in design document. **Dependencies**: Tasks 1, 2, 3, 4, 5, 6, 7. **Estimated Effort**: 5 hours.

- [x] 10. Create Data Flow Diagrams: Document key operation flows through the system including "Initial Scan & Dedup" flow diagram, "Deduplication Execution" flow diagram, "HF Download Interception" flow diagram, error handling paths, decision points and branches, and text documentation for each step. Document in design document. **Dependencies**: Tasks 2, 4, 7. **Estimated Effort**: 4 hours.

- [x] 11. Document Algorithm Specifications: Provide pseudocode for all critical algorithms including BLAKE3 hash computation algorithm (small + large files), hash caching algorithm, transactional move protocol (Phase A & B), crash recovery algorithm, reference counting algorithm, canonical path selection algorithm, and link creation algorithm (Windows). Document in design document. **Dependencies**: Tasks 2, 3, 4, 5. **Estimated Effort**: 6 hours.

- [x] 12. Specify Technical Stack: Document all technology choices and dependencies including all Rust crates with versions, feature flags for each crate, platform-specific dependencies, Python package requirements, pyproject.toml specification, major technology choices justification, and version constraints. Document in design document. **Dependencies**: None. **Estimated Effort**: 3 hours.

- [x] 13. Define Repository Structure: Document the complete repository layout including all crate directories, Python package structure, documentation directories, test directories, CI/CD configuration locations, RFC document locations, and directory tree diagram. Document in design document. **Dependencies**: Task 12. **Estimated Effort**: 2 hours.

- [x] 14. Review and Finalize Design Document: Comprehensive review of all design documentation including reviewing all 7 RFCs for consistency, checking database schema completeness, verifying algorithm correctness, ensuring Windows compatibility is comprehensive, reviewing data flow diagrams, checking for contradictions between sections, verifying all success criteria met, proofreading for clarity, and adding glossary terms. **Dependencies**: Tasks 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13. **Estimated Effort**: 4 hours.

- [x] 15. Create Summary Documentation: Create supplementary documentation for Phase 0 completion including architecture.md summary, success criteria checklist, open questions for future phases, assumptions documentation, glossary creation, next steps for Phase 1 identification, and risks and mitigations documentation. Create `docs/architecture.md`. **Dependencies**: Task 14. **Estimated Effort**: 3 hours.

## Notes

**Total Tasks**: 15
**Estimated Total Effort**: 69 hours (~2-3 weeks for one person)

**Critical Path**:
1. Tasks 1, 2, 5, 12 can be done in parallel (independent)
2. Tasks 6, 7 depend on Task 5 (Windows strategy)
3. Task 8 depends on Tasks 3, 4, 7 (understanding use cases)
4. Task 9 depends on all RFCs (Tasks 1-7)
5. Tasks 10, 11 depend on specific RFCs
6. Task 13 depends on Task 12 (technical stack)
7. Task 14 depends on all previous tasks (Tasks 1-13)
8. Task 15 depends on Task 14 (final review)

**Parallelization Opportunities**:
- Wave 1: Tasks 1, 2, 5, 12 (independent RFCs and tech stack)
- Wave 2: Tasks 3, 4, 6, 7 (depend on Task 5 and/or Task 8)
- Wave 3: Task 8 (depends on Tasks 3, 4, 7)
- Wave 4: Tasks 9, 10, 11, 13 (documentation and algorithms)
- Wave 5: Task 14 (comprehensive review)
- Wave 6: Task 15 (summary documentation)

**Priority Order** (if time-constrained):
1. **Critical**: Task 5 (Windows Compatibility) - highest risk
2. **High**: Tasks 1, 2, 4 (Storage, Hash, Dedup) - core functionality
3. **High**: Task 8 (Database Schema) - foundation for all features
4. **Medium**: Tasks 3, 6, 7 (Ref Model, Virtual FS, HF) - important but Phase 2+
5. **Medium**: Tasks 9, 10, 11 (Documentation) - clarifies design
6. **Low**: Tasks 12, 13 (Tech Stack, Repo) - can be adjusted later
7. **Essential**: Tasks 14, 15 (Review, Summary) - must be done

## Task Dependency Graph

```
Task 1 (RFC Storage) ────────────┐
Task 2 (RFC Hash) ───────────────┼──────┐
Task 5 (RFC Windows) ────┬───────┼──────┤
                         │       │      │
Task 6 (RFC Virtual FS) ─┤       │      │
                         │       │      │
Task 7 (RFC HF) ─────────┼───┐   │      │
                         │   │   │      │
Task 3 (RFC Ref) ────────┼───┤   │      │
Task 4 (RFC Dedup) ──────┼───┤   │      │
                         │   │   │      │
                         ▼   ▼   │      │
Task 8 (DB Schema) ──────────────┤      │
                                 │      │
Task 12 (Tech Stack) ────┐       │      │
                         │       │      │
                         ▼       ▼      ▼
Task 13 (Repo Structure) ┤       │      │
                         │       │      │
                         ▼       ▼      ▼
Task 9 (Architecture) ───────────┤      │
Task 10 (Data Flows) ────────────┤      │
Task 11 (Algorithms) ────────────┤      │
                                 │      │
                                 ▼      ▼
Task 14 (Review & Finalize) ────────────┤
                                        │
                                        ▼
Task 15 (Summary Documentation) ────────┘
```

**Dependency Summary**:
- No dependencies: Tasks 1, 2, 5, 12
- Depends on Task 5: Task 6
- Depends on Task 8: Tasks 3, 4
- Depends on Tasks 2, 8: Task 7
- Depends on Tasks 3, 4, 7: Task 8
- Depends on Tasks 1-7: Task 9
- Depends on Tasks 2, 4, 7: Task 10
- Depends on Tasks 2, 3, 4, 5: Task 11
- Depends on Task 12: Task 13
- Depends on Tasks 1-13: Task 14
- Depends on Task 14: Task 15
