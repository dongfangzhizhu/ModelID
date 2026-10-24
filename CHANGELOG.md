# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Security

- **The proxy is secure by default** — it now binds `127.0.0.1` and requires a
  bearer token; `require_token` defaults to `true` and `modeld proxy start`
  generates, persists and prints a token on first run. Requests without a valid
  token get `401` instead of being served. Binding to a non-loopback address
  now requires `--allow-lan` and is refused otherwise; `--allow-anonymous` is
  rejected unless the bind is loopback. (ROADMAP A3)
- **Token comparison is constant-time** — the previous `iter().any(|t| t == given)`
  leaked the position of the first differing byte and stopped early.
  (ROADMAP A3)
- **WebUI CORS is an origin allowlist** — `CorsLayer::very_permissive()` allowed
  every website to read API responses from a local server. Only the loopback
  aliases and the configured host are allowed now. (ROADMAP A3)
- **CSRF guard for state-changing requests** — a browser-originated
  `POST/PUT/PATCH/DELETE` must come from an allowed `Origin` *and* carry
  `X-Modeld-CSRF: 1`; otherwise `403`. Non-browser clients (no `Origin`) are
  unaffected. (ROADMAP A3)
- **The standalone WebUI can be protected** — `--token` / `MODELD_AUTH_TOKEN`
  are wired through, and a non-loopback bind without a token exits with code 1
  instead of starting an open server. (ROADMAP A3)

### Changed

- **`modeld tx` moved to its own module** — `commands/tx.rs` holds `TxAction`,
  `tx_command` and `parse_duration_str`. (ROADMAP B2)
- **`modeld db` moved to its own module** — `commands/db.rs` holds `DbAction` and
  `db_command` (status, backup, restore, vacuum, migrate). (ROADMAP B2)
- **`modeld store` moved to its own module** — `crates/modeld-cli/src/commands/store.rs`
  holds `StoreAction`, `store_command`, `store_migrate_command` and `copy_dir_all`;
  `main.rs` keeps the clap definition and dispatch. First step of the B2 split.
  (ROADMAP B2)
- **Directory fsync after every rename** — `platform::fsync_dir` (a real `fsync(2)` on
  Unix, a documented no-op on Windows) is called after the atomic replace in
  `links.rs`, the rename in `unlink.rs` and both CAS publish paths, so a crash right
  after the rename cannot lose the directory entry. (ROADMAP B1)

- **One config schema** — `[proxy]` is now part of `ModeldConfig`
  (`modeld-core::config::ProxyConfig`), so `modeld.toml` is described by a single
  struct instead of two competing ones. `modeld-proxy::config` re-exports the
  types under their historical names. (ROADMAP A6)
- **`modeld scan` no longer copies models into the store by default** — the new
  `IngestMode` defaults to `Index`, which only writes the database. Copying is
  opt-in via `--copy`, and `--adopt` hard-links the files instead (zero extra
  disk space, same volume). **This is a breaking change**: a scanned-but-not-adopted
  model has no CAS object, so `modeld proxy` cannot serve it until it is adopted,
  copied or downloaded. See `docs/scan-modes.md`. (ROADMAP A5)
- **Model extensions are configurable and broader** — `modeld.toml [scan] extensions`
  overrides the list, and the defaults now also cover `onnx`, `tflite`, `h5`, `pb`,
  `engine` and `npz` alongside the weight formats. Both the CLI and the WebUI
  scanner read the same config. (ROADMAP A5)
- **`modeld serve` requires `--allow-lan` for non-loopback hosts** — matching
  `modeld-webui` and `modeld proxy start`. A bearer token is always enforced.
  (ROADMAP A4)
- **Proxy streams instead of buffering** — `/v1/blobs/{hash}` and the HF-proxy
  routes hand a `File` handle to tiny_http instead of reading the whole object
  into a `Vec`, so memory stays flat regardless of model size (measured: 0 MB
  RSS growth for a 512 MiB download). (ROADMAP A2)
- **Proxy serves requests concurrently** — the accept loop now feeds a fixed-size
  worker pool (one thread per core, max 16) instead of handling one request at a
  time; a stalled download no longer blocks `/health` or `/v1/models`.
  (ROADMAP A2)
- **Range responses are chunked** — a single `206` is capped at 64 MiB
  (configurable via `[proxy] range_chunk_bytes`); clients that ask for
  `bytes=0-` on a huge object get the first window and re-request the rest.
  (ROADMAP A2)
- **HF-proxy downloads use their own SQLite connection** — a cache miss no longer
  holds the shared DB mutex for the whole upstream transfer. (ROADMAP A2)

- **`rust-version = "1.88"`** — the declared MSRV was unverified (the Dockerfile
  pinned 1.80, the roadmap assumed 1.82). Building with 1.82 fails on
  `include-flate-codegen` (edition 2024) and 1.86 fails on `libflate 2.3` (let-chains);
  1.88 builds and passes the whole suite. The Dockerfile and the CI MSRV job use the
  same number. (ROADMAP A8)
- **The crates are `publish = false`** — this repository ships a binary, not crates; the
  flag also lets `cargo-deny` accept the workspace-internal path dependencies.

### Added

- **Documented exit codes and an output-format type** — `output::ExitCode` (0 ok,
  1 failure, 2 usage, 3 partial) is printed by `modeld --help`, and
  `OutputFormat { Table, Json, Csv }` gives the upcoming command modules one place
  to decide what stdout looks like. (ROADMAP B2)
- **One crash-recovery implementation** — `recover::recover_store` is now the only
  place that replays the write-ahead log; `DedupEngine` and `modeld tx recover` both
  call it, and it reports what happened to *every* path it touched
  (`rolled-forward` / `rolled-back` / `skipped`) instead of a bare count.
  (ROADMAP B1)
- **Downloads are recorded in the write-ahead log too** — `download_hf_file` writes
  a `pending` `download` transaction (snapshot path, BLAKE3, repo/file) before the
  CAS write and commits it after the model, mapping, download record and HfCache
  alias exist. Recovery finishes the import with the HfCache/Symlink alias when
  the object is on disk. (ROADMAP B1)
- **Ingest is recorded in the write-ahead log** — `ingest_scan_results` writes a
  `pending` transaction (source path, hash, ingest mode) before the CAS write and
  commits it after the DB rows; recovery finishes an interrupted ingest by
  restoring the model and alias rows when the CAS object made it to disk. Before
  this the ingest path wrote no WAL at all, so a crash left a CAS object that
  nothing referenced. (ROADMAP B1)
- **`Database::in_transaction`** — `TransactionManager::begin`, `commit` and `fail` each write two
  rows (status + plan / end time); they now share one SQLite transaction
  (`BEGIN IMMEDIATE`), so a crash cannot leave a WAL row whose
  `affected_paths`/`rollback_plan` never landed — recovery reads exactly those
  fields to decide what to do. (ROADMAP B1)
- **A cross-process store lock** — `dedup` and `gc` (CLI and WebUI) take
  `<store>/.modeld.lock` before touching the layout, wait up to 30 s for a running
  process, and otherwise fail with the holder's pid, host and operation. The lock
  file is created with `O_EXCL` and a lock untouched for 24 h is treated as abandoned.
  (ROADMAP B1)

- **`MODELD_AUTH_TOKEN` is honoured by `modeld serve`** — the token is resolved as
  `[auth] token` → `MODELD_AUTH_TOKEN` → generated, and the environment variable is
  never written to `modeld.toml`, so a container can inject the secret without it
  landing in the store. Verified: no token → `401`, `Bearer env-secret-token` → `200`,
  config file byte-identical afterwards. (ROADMAP A8)
- **A multi-stage Docker image** — `rust:1.88-slim` builder, `debian:bookworm-slim` runtime
  with only the stripped binary (10.4 MB), a non-root user, `/data` as a volume and
  `--locked` in the build. `.dockerignore` keeps the build context small. (ROADMAP A8)
- **`deny.toml` and four new CI jobs** — `cargo deny` (licences, advisories, bans,
  sources), an MSRV job that reads `rust-version` from `Cargo.toml`, the frontend test
  suite (`npm ci` + `npm test`), and a Docker job that builds the image, asserts it is
  under 200 MB and checks `401`/`200` with and without the token. Every cargo command in
  CI is now `--locked`. (ROADMAP A8)

- **`/metrics` counts requests and errors** — a middleware installed as the outermost
  layer increments `modeld_requests_total` and `modeld_errors_total{status}` for every
  request, including the ones the CSRF/CORS guards reject. Before this the
  counters existed but nothing ever incremented them. (ROADMAP A7)
- **`audit.log` is written again** — `audit::record` appends an NDJSON line for dedup
  (per committed group), GC (per quarantined object), `unlink`, `config set` (token
  values redacted) and `PUT /settings`. Auditing never fails the operation it
  records; a write error goes to stderr. (ROADMAP A7)
- **`[scan] incremental` and `[serve] open_browser`** — two settings the Web UI
  exposed but nothing read are now real: the scan endpoint re-hashes only
  changed files unless the setting (or `force_rehash`) says otherwise, and
  `modeld serve` opens the browser when the config asks for it. Both are editable
  through `modeld config set`. (ROADMAP A7)
- **`modeld config list` and a wider `config get/set`** — 20 keys are covered
  (store, serve, dedup, gc, auth, scan, proxy), an unknown key now gets a
  "did you mean" suggestion, and `auth.token` is masked when read.
  (ROADMAP A6)
- **`modeld scan --adopt`** — hard-links a model into the CAS and atomically
  re-points the original path at the object (`hard_link` to a temporary name +
  `rename`, the same protocol as A1). Adopting a 128 MiB model changed the volume's
  free space by 0.3 MB and produced an object with the *same* file id. Adopting
  across volumes fails with an explicit `cross-volume … use --copy instead` error
  rather than silently falling back to a copy. (ROADMAP A5)
- **`--index-only` / `--copy` scan flags and a WebUI `mode` field** — the three
  modes are selectable from the CLI and from `POST /api/v1/scan`. (ROADMAP A5)

### Fixed

- **`modeld tx recover` disagreed with the dedup engine about the same crash** —
  a `copied` transaction (CAS object written, alias links partially re-pointed)
  was rolled *forward* by `DedupEngine` but marked *failed* by
  `TransactionManager::recover`. Recovery is now shared, so the outcome no longer
  depends on which command runs first. (ROADMAP B1)

- **`.gitignore` was double-encoded and re-included `node_modules`** — the file contained
  mojibake instead of em dashes, and two `!/crates/modeld-webui/ui/node_modules/`
  rules re-included the installed tree, which is how
  `ui/node_modules/.package-lock.json` ended up committed. The file is rewritten, that
  file is untracked, and `ui/package.json` / `ui/package-lock.json` stay tracked so
  `npm ci` in CI is reproducible. (ROADMAP A8)

- **`PUT /api/v1/settings` was a no-op** — it returned `204` and wrote nothing, while
  `GET` returned hard-coded defaults (`port: 8234`, `quarantine_days: 30`,
  `incremental_scan: true`). Settings are now read from and written to
  `modeld.toml`, validated (`port` ≠ 0, `quarantine_days` 1–3650, non-empty
  host) and survive a restart. (ROADMAP A7)
- **The settings page showed toggles that did nothing** — "Auto-scan on Start",
  "File Watch (inotify)" and "Confirm before GC" had no implementation behind
  them; they were removed from the API and the UI rather than persisted as
  decoration. The store root is now marked read-only, because it mirrors the
  `--store` the server was started with. (ROADMAP A7)
- **"Dedup saved" reported twice the space actually freed** — every link alias was
  counted as saved bytes, so a content with two hard links claimed `2 × size` while
  only `1 × size` was freed (the first link points at an inode that already existed).
  The figure is now `(links + copies - max(copies, 1)) × size`; verified against NTFS
  file ids: measured 5 242 880 B, reported 5 242 880 B (0% difference). (ROADMAP A7)
- **"Wasted space" grew after a successful dedup** — it multiplied the model size
  by `alias_count - 1`, and dedup *adds* aliases. Only independent copies
  (`original`/`reference_only`) count now, so the figure falls to zero when the
  duplicates become links. (ROADMAP A7)
- **`last_scan_at` was always `null`** — it is now `MAX(path_index.last_seen)`.
  (ROADMAP A7)
- **`[gc] quarantine_ttl_days` was ignored by GC** — `GcEngine` always used a
  hard-coded 30-day TTL; it now builds its `QuarantineManager` from the config, so
  the value in the settings page is the one GC uses. (ROADMAP A7)
- **`config set` deleted every section it did not know about** — `save_config`
  serialized the whole struct and overwrote the file, so a hand-written `[proxy]`
  table, an experimental `[my.experiment]` block or a comment above a section was
  lost on the next write. It now parses the existing document with `toml_edit`,
  replaces only the tables it owns, and writes atomically. (ROADMAP A6)
- **`proxy token-rotate` wrote a token the proxy never read** — rotation
  updated `[auth] token` only, while the server enforces `[proxy.auth] tokens`,
  so the old token kept working after a restart. Both sections are written now,
  and `proxy start` falls back to `[auth] token` for older configs. (ROADMAP A6)
- **`modeld scan --adopt` refuses a file that changed after it was scanned** —
  the cached scan result skips the re-hash, so a size check now stands between a
  stale hash and a corrupt CAS object. (ROADMAP A5)
- **The Web UI opened the wrong database** — `modeld-webui` used
  `<store>/metadata.db`, so it showed an empty library next to a store that
  `modeld scan` had filled. It now uses `<store>/modeld.db` like every other
  command, and prints a hint when it creates a fresh one. (ROADMAP A4)
- **The WebSocket could not authenticate** — browsers cannot set headers on a
  handshake, so `/ws` was unreachable whenever a token was configured. It now
  accepts `?token=…` (that path only; API routes still reject query tokens).
  (ROADMAP A4)
- **The UI silently failed when a token was required** — `api.js` never sent
  `Authorization`. It now reads the token from `localStorage` and asks for it
  on the first `401` instead of rendering empty pages. (ROADMAP A4)
- **The WebSocket reconnected forever** — a rejected handshake retried every 5 s
  with no feedback. Reconnects now back off exponentially (1 s to 30 s) and the
  indicator reports "not authorised" after three failures. (ROADMAP A4)
- **Frontend tests no longer ship in the binary** — `ui/tests/*.js` matched the
  `*.js` embed glob and was served at `/tests/…`; `tests/` and `node_modules/` are now
  excluded from the embedded assets. (ROADMAP A4)
- **A panicking request handler no longer kills the proxy** — every job runs under
  `catch_unwind` and a poisoned DB mutex is recovered instead of aborting the
  request; the old `.expect("db lock poisoned")` paths are gone. (ROADMAP A2)
- **Scan progress counters could go backwards** — hashing runs on rayon threads, so
  two callbacks could deliver `files_scanned=2` before `files_scanned=1`. The
  counter increment and the event send are now serialized. (found by the WebUI
  property test `prop_scan_lifecycle_...`)

- **Atomic link replacement** — dedup no longer deletes a duplicate before
  linking it. The link is staged under a temporary name in the same directory
  and renamed over the destination, so a failure can never leave the user's
  path missing. `unlink` uses the same protocol and restores ordinary file
  permissions on the copy it writes back. (ROADMAP A1)

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
