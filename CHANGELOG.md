# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] — 2026-07-14

### First Public Release

#### Added
- **Core CAS**: BLAKE3 content-addressable storage with 256-prefix directory layout, immutable objects, crash-safe two-phase commit
- **CLI**: `init`, `scan`, `status`, `stats`, `list`, `info`, `hash`, `dedup`, `dupes`, `verify`, `unlink` commands
- **Quarantine**: 30-day TTL quarantine system with `quarantine list/cleanup/restore` commands
- **Garbage Collection**: Three-tier protection (hard/soft/orphan) with safe GC preview and execution
- **HuggingFace Integration**: Fake HF cache layout, resumable downloader with SHA256 verification, Python monkey-patch hook
- **Workflow Reference Graph**: ComfyUI workflow JSON parser (17 loader types), dependency resolution, orphan detection
- **Local Proxy Server**: LAN-wide HF mirror with Range request support, Bearer token auth, IP allow/deny lists, mDNS discovery
- **Web UI**: Embedded Axum dashboard with REST API, WebSocket real-time events, i18n (EN/ZH), 7 pages (Dashboard, Duplicates, Library, Downloads, Refs, Proxy, Settings)
- **Rust Client SDK**: `modeld-client` library for health checks, model listing, and resumable blob downloads
- **Cross-Platform**: Windows (hardlink/junction fallback), Linux, macOS support
- **Internationalization**: English and Simplified Chinese with sys-locale auto-detection

#### Security
- Proxy default bind address changed to `127.0.0.1` (was `0.0.0.0`)
- WebSocket endpoint secured with Bearer token middleware
- CORS tightened from permissive to specific methods/headers
- Proxy start command now correctly passes `--token` and `--config` flags (regression fix)

#### Changed
- `from_str` renamed to `parse_str` in `tx.rs` to avoid `FromStr` trait confusion
- Manual string slicing replaced with `strip_prefix` in `platform.rs`

#### Fixed
- Clippy warnings: `Vec::new()` + `push` → `vec![]`, `sort_by` → `sort_by_key` with `Reverse`
- Formatting: 237 files normalized to `cargo fmt` standards
- Windows read-only attribute clearing properly annotated with clippy allow directives
- Dedup command: removed unused `#[expect(dead_code)]` and `too_many_arguments` lint violations

#### Documentation
- Archived Phase 0 audit docs (`AUDIT.md`, `ARCHITECTURE_CURRENT.md`) to `docs/history/`
- Updated README: corrected version from v1.0 to v0.1.0, added install instructions, improved Quick Start flow
- Created CHANGELOG.md following Keep a Changelog format
