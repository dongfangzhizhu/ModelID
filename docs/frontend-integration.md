# Frontend integration (C7)

modeld exposes a virtual, frontend-shaped view of every CAS-backed model under
`<store>/virtual/`. The frontends keep their normal directory layout; modeld
fills it with hardlinks (or symlinks across volumes), so the same bytes are
shared rather than copied.

## Built-in adapters

| Frontend | Virtual root | Checkpoints | LoRAs |
|---|---|---|---|
| ComfyUI | `<store>/virtual/comfyui` | `checkpoints/` | `loras/` |
| Forge | `<store>/virtual/forge` | `models/Stable-diffusion/` | `models/Lora/` |
| A1111 | `<store>/virtual/a1111` | `models/Stable-diffusion/` | `models/Lora/` |

Full category mapping (VAE, embeddings, ControlNet, upscalers, CLIP) is in
`crates/modeld-core/src/virtual_fs.rs`, the as-built implementation of RFC 0006.

## Commands

```bash
modeld frontend list
modeld frontend sync --frontend comfyui          # repeat --frontend for more
modeld frontend sync --dry-run                    # plan only, no writes
modeld frontend connect comfyui --target /path/to/ComfyUI/models
```

`sync` is idempotent: it leaves existing links untouched and only records a new
alias when it creates a new link. `connect` refuses to replace a non-empty
directory; it creates a symlink from the frontend models directory to the
virtual root. On Windows this requires Developer Mode or administrator rights;
the command fails with an explicit message rather than silently copying files.

One-shot wrappers: `scripts/connect-frontend.sh` and
`scripts/connect-frontend.ps1`.

## Boundaries

- Frontends must support pointing at a model directory or a `extra_model_paths`
  style configuration. modeld does not patch frontend source code.
- If a model has no CAS object (index-only ingest), it is counted as
  `missing_cas` and no virtual link is created.
- Cross-volume links fall back to symlinks only when the OS permits them;
  otherwise the sync reports a failure instead of writing a stub file.
- Actual image generation in the three frontends was not exercised on this
  machine (see `docs/ROADMAP.md` §12.3); the verified contract is the directory
  layout, link sharing and alias bookkeeping.
