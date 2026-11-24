# modeld-hook

Transparent HuggingFace download interception for [modeld](https://github.com/your-org/modeld) CAS.

`modeld-hook` redirects HuggingFace Hub downloads into modeld's Content-Addressable
Store, so identical models are downloaded once and deduplicated across every HF-based
framework (diffusers, transformers, ComfyUI, Forge, A1111).

## How it works

Two complementary layers:

| Layer | Mechanism | Coverage |
|-------|-----------|----------|
| Layer 1 | Sets `HF_HOME` to modeld's fake HF cache | ~95% — uses the official HF API |
| Layer 2 | Monkeypatches `huggingface_hub.hf_hub_download` | ~5% — fallback for edge cases |

## Requirements

- Python ≥ 3.9
- The `modeld` Rust CLI installed and on `PATH` (it provides `hf-setup`, `hf-check`,
  `hf-download`). Without it the hook stays inactive and downloads behave normally.

## Installation

```bash
pip install modeld-hook
```

## Usage

### Method 1 — Environment variable (recommended, zero code)

```bash
export HF_HOME="$(modeld hf-setup --print-path)"
```

### Method 2 — Explicit activation

```python
import modeld_hook
modeld_hook.activate()      # import itself has no side effect
from diffusers import DiffusionPipeline
# Downloads are now routed through modeld CAS
```

### Method 3 — Manual control

```python
import modeld_hook

if modeld_hook.activate(verbose=True):
    print("modeld interception active")
# ...
modeld_hook.deactivate()
```

Importing `modeld_hook` never activates it implicitly. To opt in from the
environment, set `MODELD_AUTO_HOOK=1` (there is no `MODELD_NO_AUTO_HOOK`
switch).

## API

| Function | Description |
|----------|-------------|
| `activate(verbose=False)` | Activate interception; returns `True` on success |
| `deactivate()` | Restore original `huggingface_hub` functions |
| `is_active()` | Whether the hook is currently active |
| `get_hf_home()` | The HF cache path modeld manages, or `None` |

## Limitations

- The `modeld` binary must be available on `PATH`; otherwise the hook falls
  back to the original `huggingface_hub` functions.
- Calls that pass `cache_dir`, `local_dir`, or `force_download` are delegated
  directly to the original library.
- `snapshot_download` is not intercepted; only `hf_hub_download` is patched.
- On Windows, creating the fake cache symlinks needs Developer Mode or admin
  privileges; without it the CAS download still works but HF cache hits do not.

## License

MIT
