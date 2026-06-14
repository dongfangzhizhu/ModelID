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

### Method 2 — Explicit import

```python
import modeld_hook          # auto-activates on import
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

To opt out of auto-activation on import, set `MODELD_NO_AUTO_HOOK=1`.

## API

| Function | Description |
|----------|-------------|
| `activate(verbose=False)` | Activate interception; returns `True` on success |
| `deactivate()` | Restore original `huggingface_hub` functions |
| `is_active()` | Whether the hook is currently active |
| `get_hf_home()` | The HF cache path modeld manages, or `None` |

## License

MIT
