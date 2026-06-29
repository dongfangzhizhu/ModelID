# modeld

Content-Addressable Storage (CAS) infrastructure for AI models.

**Status**: All phases complete (Phases 1–5 + Web UI) · v1.0 · [MIT License](LICENSE)

## Overview

modeld is the "containerd + git-lfs + nix store" for AI models. It provides:

- **Deduplication**: Automatic space savings via BLAKE3-based CAS — one copy of each unique model, regardless of how many frontends reference it
- **Content Addressing**: BLAKE3-addressed immutable storage with crash-safe two-phase commit
- **Transparent Integration**: Works with ComfyUI, Forge, A1111, HuggingFace ecosystem (`diffusers`, `transformers`) — no config changes required
- **Resumable Downloads**: HTTP Range-based download resumption with WAL-backed crash recovery
- **Workflow Awareness**: Parse ComfyUI workflow JSON to build a model reference graph and enable safe GC
- **Local Proxy**: LAN-wide HuggingFace mirror — download once, serve at full LAN bandwidth
- **Cross-Platform**: Windows, Linux, macOS support (with Windows hardlink/junction fallback strategy)

## Quick Start

```bash
# Initialize store (default: ~/.local/share/modeld)
modeld init

# Scan models directory — hash, index, detect duplicates
modeld scan ~/models/

# Check store status and quarantine summary
modeld status

# Detailed stats: unique models, aliases, duplicate groups, potential savings
modeld stats

# Inspect indexed content
modeld list
modeld list --limit 20 --json
modeld info <blake3-hash>
modeld info <blake3-hash> --json

# Hash a file (without storing)
modeld hash model.safetensors
```

## Deduplication

```bash
# Preview what would be deduplicated (no changes made)
modeld dedup --dry-run

# Report only — analyze and print, no modifications
modeld dedup --report

# Execute automatically (no confirmation prompts)
modeld dedup --auto
```

Dedup uses a crash-safe two-phase commit (WAL-backed) and chooses a canonical
path per content group (CAS > oldest mtime > shortest path). Replaced files go
to a 30-day quarantine that can be inspected and restored.

### Duplicate report

```bash
modeld dupes                      # list all duplicate groups
modeld dupes --min-size 500MB     # filter small files
modeld dupes --json               # machine-readable JSON output
```

### Quarantine management

```bash
modeld quarantine list     # show quarantined files and expiry countdown
modeld quarantine cleanup  # permanently delete expired entries (>30 days)
```

## HuggingFace integration

```bash
# Check whether a HF file is already in the modeld cache
modeld hf-check stabilityai/stable-diffusion-xl-base-1.0 sd_xl_base_1.0.safetensors
modeld hf-check <repo_id> <filename> --revision v1.0 --json

# Download via modeld CAS (deduplicates against existing content, resumable)
modeld hf-download stabilityai/stable-diffusion-xl-base-1.0 sd_xl_base_1.0.safetensors
modeld hf-download <repo_id> <filename> --token <HF_TOKEN> --json

# Configure shell to point HF_HOME at the modeld fake cache
modeld hf-setup               # prints activation instructions
modeld hf-setup --print-path  # machine-readable path only (used by Python hook)

# Show HF cache stats: repos cached, blobs, download history
modeld hf-status
```

For automatic interception from `diffusers` / `transformers` / ComfyUI Manager,
install the Python hook (`pip install modeld-hook`) and `import modeld_hook`.
The hook auto-activates on import and silently falls back to the original HF
download if `modeld` CLI is unavailable.

See [python/README.md](python/README.md) for details.

## Workflow reference graph & GC

```bash
# Index ComfyUI workflow JSON files and resolve model references
modeld workflow-scan ~/comfyui/user/workflows/

# Show all model dependencies of a single workflow (checkpoint, lora, vae, …)
modeld workflow-deps workflow.json

# List models with no workflow references (orphans)
modeld refs-orphans
modeld refs-orphans --json

# Safe GC: move zero-reference models to 30-day quarantine
modeld gc --preview   # dry run — show what would be quarantined
modeld gc             # execute
modeld gc --cleanup-quarantine   # also permanently delete expired entries
```

GC protection layers:
1. **Hard protected** — any workflow reference → never GC'd
2. **Soft protected** — alias exists (frontend virtual directory link) → warn only
3. **Quarantined** — ref count = 0, alias = 0 → 30-day grace period
4. **Deleted** — quarantine TTL expired

## Local registry & proxy

Share one CAS store across the LAN — download once, serve everywhere at full
LAN bandwidth, with transparent HuggingFace deduplication.

```bash
# Start the proxy (on the machine holding the store)
modeld proxy start --port 8234 --store .modeld

# With authentication
modeld proxy start --port 8234 --token <secret> --no-allow-anonymous

# From a config file
modeld proxy start --config modeld.toml

# Discover proxies on the LAN (mDNS)
modeld proxy discover
modeld proxy discover --timeout 10

# Check a running server's health and model inventory
modeld proxy status
modeld proxy status --url http://192.168.1.5:8234
modeld proxy status --url http://192.168.1.5:8234 --token <secret>
```

Use as a transparent HuggingFace mirror — point `HF_ENDPOINT` at the proxy
and every HF-based framework (diffusers / transformers / ComfyUI / Forge / A1111)
downloads through modeld with no code changes:

```bash
export HF_ENDPOINT="http://192.168.1.5:8234/v1/hf-proxy"
```

A Rust client SDK (`modeld-client`) provides health checks, model listing, and
resumable blob/HF downloads. See the [proxy setup guide](docs/proxy-setup.md)
and [HTTP API reference](docs/proxy-api.md).

## Web UI (`modeld-webui`)

A standalone browser-based dashboard for managing your modeld store visually,
built with an embedded Axum HTTP server + vanilla JS frontend (no Node.js required).

```bash
# Start the Web UI (default: http://127.0.0.1:8234)
modeld-webui --store .modeld

# Bind to all interfaces for LAN access, auto-open browser
modeld-webui --store .modeld --host 0.0.0.0 --port 9000 --open
```

### Pages

| Page | Description |
|------|-------------|
| **Dashboard** | Live stats (unique models, duplicate waste, total size, last scan time); per-frontend breakdown table; one-click scan with real-time progress bar; quick GC trigger |
| **Duplicates** | Duplicate group list with wasted bytes; one-click deduplication with confirmation dialog |
| **Library** | Full model inventory — search, filter, copy hash, view aliases |
| **Downloads** | HuggingFace download history with status (pending / downloading / done / failed) |
| **Refs** | Workflow reference graph — orphaned models list, workflow-to-model dependency view |
| **Proxy** | Local proxy server status and configuration |
| **Settings** | Store path, GC quarantine TTL, auto-scan, incremental scan, UI host/port settings |

### Technical features

- **REST API** (`/api/v1/*`) — stats, models, dupes, dedup, downloads, scan, GC preview/run, settings GET/PUT
- **WebSocket** (`/ws`) — real-time push events: scan progress (phase, file count, path), dedup progress, daemon status; auto-reconnect on disconnect
- **Embedded static files** — UI assets compiled into the binary via `rust-embed`; zero external dependencies at runtime
- **i18n** — English / Chinese (Simplified) language toggle, persisted across sessions
- **Confirmation dialogs** — modal-based confirm before destructive GC operations
- **Toast notifications** — non-blocking success/error/info feedback

## Store layout


```
$MODELD_STORE/          (default: .modeld/)
├── cas/blake3/
│   └── ab/
│       └── abcdef…     (immutable, read-only CAS object)
├── hf_cache/hub/
│   └── models--org--repo/
│       ├── blobs/       (symlinks → CAS objects)
│       └── snapshots/   (HF-compatible layout)
├── tmp/
│   ├── downloads/       (in-progress .part files, resumable)
│   └── cas_staging/     (two-phase commit staging)
├── quarantine/          (deferred deletion, 30-day TTL)
└── modeld.db            (SQLite: models, aliases, refs, downloads)
```

## Phases

| Phase | Focus | Status |
|-------|-------|--------|
| 1 | Core CAS — BLAKE3 hashing, SQLite registry, file scanner, dedup report | ✅ Complete |
| 2 | Dedup engine — two-phase commit, hardlinks/symlinks, quarantine, GC | ✅ Complete |
| 3 | HuggingFace interception — fake HF cache, resumable downloader, Python hook | ✅ Complete |
| 4 | Workflow reference graph — ComfyUI parser, dependency graph, safe GC | ✅ Complete |
| 5 | Local registry & proxy — LAN HF mirror, mDNS discovery, Range requests, auth | ✅ Complete |
| — | Web UI (`modeld-webui`) — embedded dashboard with REST API + WebSocket | ✅ Complete |

> **v1.0**: All 7 implementation areas are complete and shipping.

## Building

```bash
cargo build --release
# Produces two binaries:
#   target/release/modeld         — CLI tool (all Phase 1–5 commands)
#   target/release/modeld-webui   — Web UI server (browser dashboard)
```


## Testing

```bash
# Rust unit + integration tests
cargo test --workspace

# Python hook
cd python && python -m pytest tests/ -v
```

## Documentation

- [Proxy setup guide](docs/proxy-setup.md)
- [Proxy HTTP API reference](docs/proxy-api.md)
- [Architecture overview](docs/architecture.md)
- [RFCs](docs/rfcs/) — storage layout, hash strategy, ref model, dedup, Windows compat, virtual FS, HF interception
- [Python hook](python/README.md)

## License

[MIT](LICENSE)
