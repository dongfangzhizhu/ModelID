# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **As-built documentation set** — `docs/architecture.md` was rewritten from
  the implemented code: every module table row points to a real source file,
  and the Phase 0 planning draft is archived at
  `docs/history/ARCHITECTURE_PHASE0.md`. Five ADRs now record storage layout,
  transaction/recovery, authentication defaults, statistics semantics and HF
  compatibility; `SECURITY.md` and `CONTRIBUTING.md` define the support and
  contribution contract. Seventeen root planning drafts moved to
  `docs/history/`, leaving four root Markdown files. (ROADMAP B9)
- **HTTP-level e2e tests for every API route** — `tests/api_e2e.rs` drives the
  real router (with auth/CSRF/CORS/middleware in the path) for all 18 route
  declarations, covering a 4xx from each handler that validates input and a
  401 sweep over every route when a token is configured. The route table is
  read from `src/api/mod.rs` at compile time, so adding a route without a test
  breaks the suite. (ROADMAP B8)
- **A five-point interruption matrix** — `interruption_matrix.rs` crashes a
  dedup at each write point (`begin`, staging copy, CAS publish, partial link,
  before commit) and checks the same invariants everywhere: `verify` clean, no
  transaction in flight, both library paths readable with original content.
  (ROADMAP B8)
- **Environment-class tests** — `tests/environment.rs` covers cross-volume
  adoption, a junction loop in the scanner, an unwritable store layout, and
  lock contention from another process; unavailable classes skip with a
  printed reason instead of pretending to pass. (ROADMAP B8)
- **Scan/dedup benchmarks with a CI regression gate** — `cargo bench -p
  modeld-core` writes `bench/latest.json`; `scripts/bench_gate.py` fails the
  build on a >20% slowdown, refuses to compare across platforms, and has a new
  CI job. (ROADMAP B8)
- **A modeld-core coverage report** — `scripts/coverage.py` uses the LLVM
  tools from `llvm-tools-preview` (no crates.io dependency), commits the
  summary under `coverage/`, and enforces a 75% line-coverage gate in CI.
  (ROADMAP B8)
- **Cross-process store lock** — every layout-mutating command (`scan`, `gc`,
  `dedup`, `unlink`, `quarantine cleanup/restore`) now takes an exclusive OS
  lock on `<store>/.modeld.lock`, so two processes can no longer interleave
  renames in the same store. The WebUI's scan and quarantine-restore endpoints
  take the same lock. (ROADMAP B7)
- **`--wait <seconds>` on layout-mutating commands** (default 30) — a second
  process queues instead of failing, and says who it is queuing behind:
  `The store at … is busy (pid 4242 on host running 'dedup' since …); waiting up
  to 30s…`. With `--wait 0` it exits 1 with the same holder description.
  (ROADMAP B7)
- **`modeld stats --json`** — the figures are machine-readable, so
  `physical_bytes` can be diffed against `du -sb <store>/cas` by a script.
  (ROADMAP B6)
- **Storage figures separate logical from physical size** — `modeld stats`
  (text and `--json`) and `GET /api/v1/stats` report `logical_bytes` (every
  alias path counted as a full file), `physical_bytes` (one copy per unique
  content, what the store actually owns), bytes already saved by links, and
  bytes still wasted by independent copies. `duplicate_groups` keeps its old
  meaning (2+ paths) so existing dashboards do not shift; the new
  `multi_path_groups` counts groups reachable through 2+ alias paths (what
  `dupes` lists); `duplicate_groups` counts groups with 2+ independent `original`
  copies (what a dedup can still reclaim). There is no `waste_groups` field.
  Wording corrected in B9 to match `StorageStats`. (ROADMAP B6)

### Changed

- **The store lock is an OS lock, not a marker file** — the previous `O_EXCL`
  marker relied on a 24 h mtime timeout to decide whether a holder was still
  alive, which both blocked on a clean `Ctrl-C` and let a stale file masquerade
  as a live lock. It is now `flock(LOCK_EX)` / `LockFileEx`, released by the
  kernel when the holder dies. The lock file is deliberately never deleted
  (unlink would let a waiter lock the old inode while a newcomer locks a fresh
  path). `current_holder()` probes the lock before reading the recorded holder,
  so a leftover record is not reported as contention. (ROADMAP B7)
- **`scan_command` takes one `ScanOptions`** instead of `incremental`, `full`,
  `exclude_globs` and `follow_symlinks` — those are exactly that struct's
  fields, and passing them one by one had grown past clippy's argument limit.
  (ROADMAP B7)
- **`modeld stats` no longer touches the filesystem** — it used to build a
  `DedupEngine` and stat every alias path. It is now one aggregate query, so
  the summary stays cheap on a large library. `dupes` remains the command that
  inspects files. (ROADMAP B6)
- **The dashboard is two queries instead of N+1** — `get_stats` previously
  looped over every model and queried its aliases; it now uses `storage_stats()`
  and `frontend_stats()`. A failed aggregate returns 500 instead of rendering
  "0 bytes". (ROADMAP B6)

### Fixed

- **Windows HF cache symlinks were dangling** — the fake HF cache wrote
  relative symlink targets with `/` separators. Windows creates those links
  successfully but cannot resolve them, so `snapshots/<sha>/<file>` and
  `blobs/<sha>` were dead links and `huggingface_hub` could not hit the cache.
  Targets now use the host separator (`\` on Windows, `/` on Unix), with a
  Windows `canonicalize` assertion in `test_create_cache_entry`. Found by the
  full regression run during B9. (ROADMAP B9)
- **A `toString` phase could break the dashboard** — `resolvePhaseKey` looked up
  phases in a plain object with `PHASE_KEYS[phase] || fallback`, so the
  inherited `Object.prototype.toString` (and `constructor`) came back as a
  function and was passed to `t()` as an i18n key. It now only accepts keys the
  map actually owns. Found by the property test with `"toString"` as a
  counterexample. (ROADMAP B8)
- **`scan` could report success after failing every file** — a cross-volume
  `--adopt` printed `adopt failed: cross-volume …` for each file, then exited 0,
  so an automated ingest said "success" with an empty store. `scan_command`
  now exits non-zero when any file failed to ingest, while still printing the
  per-path reason. (ROADMAP B8)
- **Every stored object leaked an empty staging directory** —
  `CasStore::store_crash_safe` created `tmp/cas_staging/{tx_id}/`, renamed the
  `.part` file into the CAS, and never removed the directory, so each
  `scan --copy` / copy-to-cas dedup left one more empty UUID directory behind.
  `verify` never saw them (it only scans for `.part` files) and only `repair`
  would have collected them. `remove_dir` — not `remove_dir_all`, because
  `scan` passes the same `tx_id` for every file and deleting a `.part` in
  flight would reintroduce exactly the corruption the protocol prevents.
  (ROADMAP B7)
- **`dedup` silently did nothing when the store path was relative** — `links.rs`
  had its own `is_same_volume` that read the `Prefix` path component, which a
  relative path does not have, so every duplicate was declared cross-volume and
  recorded as `reference_only` while the file stayed on disk at full size.
  The duplicate implementation is gone and the check delegates to
  `platform::is_same_volume`, which also strips the `\\?\` verbatim prefix that
  `canonicalize` adds on Windows. (ROADMAP B6)
- **`dedup --auto` still prompted** — the flag is documented as "execute
  without confirmation prompts" but only `--yes` skipped the prompt, so a
  scripted run aborted on non-interactive stdin. (ROADMAP B6)
- **Copy-to-cas dedup now says it reclaimed nothing** — the fallback records a
  `reference_only` alias and leaves the file in place; it logged nothing, so a
  run that freed no space looked identical to one that did. (ROADMAP B6)
- **Downloader no longer trusts stale partial files** — a `.part` file is only
  resumed when its `<part>.meta` side-car still matches the URL, ETag and
  expected size. A changed object or a missing side-car discards the partial.
  `If-Range` is sent on resume; a server ignoring Range restarts from byte 0.
  (ROADMAP B4)
- **Downloads are verified while streaming** — BLAKE3 and SHA256 are computed
  in a fixed 64 KB buffer as bytes arrive, instead of re-reading the finished
  file twice. A known expected size must match exactly; when remote metadata
  is unavailable the result is explicitly marked `unverified` (both in the
  API result and `downloads.error_message`) instead of silently claiming
  success. (ROADMAP B4)
- **Download DB writes are atomic** — the WAL `pending` row, model / SHA256
  mapping / download record / HF alias and the `committed` transition now run
  in one `Database::in_transaction`, so `recover_store` can finish or roll
  back a crash consistently. (ROADMAP B4)

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

- **HF cache layout matches `huggingface_hub`** — `refs/{branch}` now stores a
  resolved 40-hex commit SHA (never a branch name) extracted from the download
  redirect; `blobs/*` and `snapshots/*/*` are relative symlinks. Symlink failure
  is reported explicitly instead of silently copying. `RepoType` selects the
  `models--` / `datasets--` / `spaces--` prefix, exposed as `--repo-type` on
  `hf-check` / `hf-download`. (ROADMAP B5)
- **The Python hook no longer changes your environment on import** — activation
  is explicit (`modeld_hook.activate()`) or opt-in via `MODELD_AUTO_HOOK=1`.
  `deactivate()` now restores both `hf_hub_download` patch sites and the
  original `HF_HOME`. The wrapper honours `repo_type`, `subfolder`, `cache_dir`
  and `local_dir`, deferring to upstream when caller-owned paths are requested.
  (ROADMAP B5)
- **The library logs through `tracing`, not stderr** — all 31 `println!`/`eprintln!`
  calls in `modeld-core` and `modeld-proxy` are now `tracing::warn!`/`info!`/`error!`,
  so a caller decides where they go. The CLI installs a stderr subscriber
  controlled by `MODELD_LOG` (default `warn`), which keeps `--json` stdout clean
  no matter how chatty a run is. (ROADMAP B3)
- **One place writes JSON** — `output::write_json` replaces the seven
  `println!("{}", serde_json::to_string_pretty(…))` call sites, so "`--json` means
  stdout is JSON" is a single function to audit rather than a convention. A new
  test parses the output of every `--json` command. (ROADMAP B2)
- **`main.rs` is down to the dispatch** — 4153 → 162 lines. The clap tree moved to
  `cli.rs`, the shared helpers to `util.rs`, and each command family already had
  its module under `commands/`. (ROADMAP B2)
- **`--store` is a global flag** — it was declared on 37 separate subcommand
  variants (and again on the nested action enums). One `global = true` argument
  on `Cli` replaces all of them, so `modeld --store X status` and
  `modeld status --store X` both work and no command has to repeat it.
  (ROADMAP B2)
- **`dedup` and `serve` moved to their own modules** — `commands/dedup.rs` and
  `commands/serve.rs`; `main.rs` is down to the dispatch and the shared helpers.
  (ROADMAP B2)
- **`scan` and `gc` moved to their own modules** — `commands/scan.rs` and
  `commands/gc.rs`; the GC keeps its store lock. (ROADMAP B2)
- **`quarantine`/`verify`/`unlink` moved to `commands/repair.rs`** — the commands
  that change or audit store state now live together. (ROADMAP B2)
- **`status`/`stats`/`dupes`/`list`/`info` moved to `commands/query.rs`** — the
  read-only queries now live together. (ROADMAP B2)
- **`hf-check`/`hf-setup`/`hf-status`/`workflow-*`/`refs-orphans` moved to
  `commands/hfstore.rs`** — they only read the local store, so they do not belong
  with the Hub-facing `hf` module. (ROADMAP B2)
- **`modeld proxy` moved to its own module** — `commands/proxy.rs` holds
  `ProxyAction`, `ProxyStartOptions` and the start/discover/status/token-rotate
  handlers. (ROADMAP B2)
- **`modeld tag|note|pin|favorite|refs` moved to `commands/meta.rs`** — the five
  families share one shape (hash, optional store, optional `--json`), so they live
  in one module rather than five files of boilerplate. (ROADMAP B2)
- **`modeld hf` moved to its own module** — `commands/hf.rs` holds the three
  HF action enums, `hf_command`, `hf_subcommand_*`, `hf_cache_command`,
  `hf_token_command` and `read_secret_from_stdin`. (ROADMAP B2)
- **`modeld config` moved to its own module** — `commands/config.rs` holds
  `ConfigAction`, `config_command`, `CONFIG_KEYS` and the get/set/validation helpers.
  (ROADMAP B2)
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

- **`--log-format json`** — the global flag switches the stderr subscriber to one
  JSON object per line (`timestamp`, `level`, `message`, span fields); the default
  stays human-readable. `scan`, `dedup` and `gc` are instrumented, and span close
  events carry `time.busy`/`time.idle`, so a slow run says where the time went.
  (ROADMAP B3)
- **CLI snapshot tests** — `crates/modeld-cli/tests/cli_snapshot.rs` pins what a user
  or a script can depend on: the command list in `--help`, the four documented
  exit codes, `--version`, `--store` in both positions, exit 2 for an unknown
  subcommand, pure-JSON `tx list --json`, the `config list` keys, a clean
  `verify`, and a `status` that fails (without creating a database) on a store
  that does not exist. Ten tests, no new dependencies. (ROADMAP B2)
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
