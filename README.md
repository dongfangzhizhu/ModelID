# modeld

Content-Addressable Storage (CAS) infrastructure for AI models.

**Status**: Phases 1–5 complete

## Overview

modeld is the "containerd + git-lfs + nix store" for AI models. It provides:

- **Deduplication**: Automatic space savings (30-60% typical)
- **Content Addressing**: BLAKE3-based immutable storage
- **Transparent Integration**: Works with ComfyUI, Forge, A1111, HuggingFace
- **Cross-Platform**: Windows, Linux, macOS support

## Phases

| Phase | Focus | Status |
|-------|-------|--------|
| 1 | Core CAS — BLAKE3, storage, SQLite, scanner, CLI | ✅ Complete |
| 2 | Dedup engine — two-phase commit, links, quarantine | ✅ Complete |
| 3 | HuggingFace interception — fake HF cache, downloader, Python hook | ✅ Complete |
| 4 | Workflow reference graph + safe GC | ✅ Complete |
| 5 | Local registry & proxy (LAN sharing, mDNS, Range requests) | ✅ Complete |

## Quick Start

```bash
# Initialize store
modeld init

# Scan models directory and ingest into CAS
modeld scan ~/models/

# Check status
modeld status

# Hash a file
modeld hash model.safetensors
```

## Deduplication

```bash
# Preview what would be deduplicated (no changes)
modeld dedup --dry-run

# Execute automatically
modeld dedup --auto
```

Dedup uses a crash-safe two-phase commit (WAL-backed) and chooses a canonical
path per content group (CAS > oldest mtime > shortest path). Replaced files go
to a 30-day quarantine that can be inspected and restored.

## HuggingFace integration

```bash
# Check whether a HF file is already in the modeld cache
modeld hf-check stabilityai/stable-diffusion-xl-base-1.0 sd_xl_base_1.0.safetensors

# Download via modeld CAS (dedups against existing content)
modeld hf-download stabilityai/stable-diffusion-xl-base-1.0 sd_xl_base_1.0.safetensors

# Point HF_HOME at the modeld fake cache
modeld hf-setup
```

For automatic interception from `diffusers`/`transformers`/ComfyUI, install the
Python hook (`pip install modeld-hook`) and `import modeld_hook`.

## Workflow reference graph & GC

```bash
# Index ComfyUI workflow JSON and resolve model references
modeld workflow-scan ~/comfyui/user/workflows/

# Show dependencies of a single workflow
modeld workflow-deps workflow.json

# List models with no workflow references
modeld refs-orphans

# Safe GC: quarantine unreferenced models (workflow-referenced models are protected)
modeld gc --preview
modeld gc
```

## Local registry & proxy

Share one CAS store across the LAN — download once, serve everywhere at full
LAN bandwidth, with transparent HuggingFace deduplication.

```bash
# Start the proxy (on the machine holding the store)
modeld proxy start --port 8234 --store .modeld

# Discover proxies on the LAN (mDNS)
modeld proxy discover

# Check a running server
modeld proxy status --url http://localhost:8234
```

Use it as a transparent HuggingFace mirror — point `HF_ENDPOINT` at the proxy
and every HF-based framework (diffusers/transformers/ComfyUI/Forge/A1111)
downloads through modeld with no code changes:

```bash
export HF_ENDPOINT="http://192.168.1.5:8234/v1/hf-proxy"
```

A Rust client SDK (`modeld-client`) provides health checks, model listing, and
resumable blob/HF downloads. See the [proxy setup guide](docs/proxy-setup.md)
and [HTTP API reference](docs/proxy-api.md).

## Documentation

- [Phase 0 Architecture](docs/architecture.md)
- [RFCs](docs/rfcs/)
- [Phase 1 Plan](PHASE1_PLAN.md) · [Phase 1 Summary](PHASE1_SUMMARY.md)
- [Phase 2 Plan](PHASE2_PLAN.md) · [Phase 3 Plan](PHASE3_PLAN.md)
- [Phase 4 Plan](PHASE4_PLAN.md) · [Phase 5 Plan](PHASE5_PLAN.md)
- [Proxy setup](docs/proxy-setup.md) · [Proxy HTTP API](docs/proxy-api.md)
- [Publish Guide](PUBLISH_GUIDE.md)

## Building

```bash
cargo build --release
```

## Testing

```bash
# Rust
cargo test --workspace

# Python hook
cd python && python -m pytest tests/
```

## License

MIT
