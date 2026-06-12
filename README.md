# modeld

Content-Addressable Storage (CAS) infrastructure for AI models.

**Status**: Phase 1 - Core CAS Implementation (In Progress)

## Overview

modeld is the "containerd + git-lfs + nix store" for AI models. It provides:

- **Deduplication**: Automatic space savings (30-60% typical)
- **Content Addressing**: BLAKE3-based immutable storage
- **Transparent Integration**: Works with ComfyUI, Forge, A1111, HuggingFace
- **Cross-Platform**: Windows, Linux, macOS support

## Phase 1 Status

Current implementation:
- [ ] BLAKE3 hashing (≥2GB/s target)
- [ ] CAS storage layer
- [ ] SQLite metadata index
- [ ] File scanner
- [ ] Basic CLI (`init`, `scan`, `status`, `hash`)

## Quick Start

```bash
# Initialize store
modeld init

# Scan models directory
modeld scan ~/models/

# Check status
modeld status

# Hash a file
modeld hash model.safetensors
```

## Documentation

- [Phase 0 Architecture](docs/architecture.md)
- [RFCs](docs/rfcs/)
- [Phase 1 Plan](PHASE1_PLAN.md)

## Building

```bash
cargo build --release
```

## Testing

```bash
cargo test --workspace
```

## License

MIT
