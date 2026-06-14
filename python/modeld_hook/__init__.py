"""
modeld-hook: HuggingFace download interception for modeld CAS

Automatically deduplicates HuggingFace model downloads by redirecting them
to the modeld Content-Addressable Store.

## Usage

### Method 1: Environment variable (recommended, zero-code)
    export HF_HOME="$(modeld hf-setup --print-path)"

### Method 2: Explicit import
    import modeld_hook
    from diffusers import DiffusionPipeline
    # Downloads are now intercepted

### Method 3: sitecustomize.py (system-wide, automatic)
    modeld hook install
"""

__version__ = "0.1.0"
__all__ = ["activate", "deactivate", "is_active", "get_hf_home"]

import logging
import os
import subprocess
import sys
from pathlib import Path
from typing import Optional

logger = logging.getLogger(__name__)

_original_hf_hub_download = None
_original_snapshot_download = None
_active = False


def _get_modeld_store() -> Optional[Path]:
    """Find the modeld store path by querying the CLI."""
    try:
        result = subprocess.run(
            ["modeld", "hf-setup", "--print-path"],
            capture_output=True,
            text=True,
            timeout=5,
        )
        if result.returncode == 0:
            path = result.stdout.strip()
            if path:
                return Path(path)
    except (FileNotFoundError, subprocess.TimeoutExpired):
        pass
    # Fallback: check HF_HOME env var (kept as raw string, see get_hf_home)
    return None


def _get_modeld_store_raw() -> Optional[str]:
    """Return the modeld HF cache path as a raw string.

    For the env-var fallback we preserve the original value verbatim instead of
    round-tripping it through `Path`, which on Windows would rewrite POSIX
    separators (e.g. ``/custom/hf/home`` → ``\\custom\\hf\\home``) and surprise
    callers. The CLI-provided path is already platform-native and is returned
    via `str(Path(...))`.
    """
    try:
        result = subprocess.run(
            ["modeld", "hf-setup", "--print-path"],
            capture_output=True,
            text=True,
            timeout=5,
        )
        if result.returncode == 0:
            path = result.stdout.strip()
            if path:
                return path
    except (FileNotFoundError, subprocess.TimeoutExpired):
        pass
    return os.environ.get("HF_HOME")


def get_hf_home() -> Optional[str]:
    """Return the current HF_HOME path managed by modeld, or None.

    Environment-variable values are returned verbatim (no separator rewriting);
    CLI-provided paths are returned in the platform-native form.
    """
    return _get_modeld_store_raw()


def is_active() -> bool:
    """Return True if modeld hook is currently active."""
    return _active


def activate(verbose: bool = False) -> bool:
    """
    Activate the modeld HuggingFace download hook.

    Returns True if activation succeeded, False if huggingface_hub is not
    installed or modeld CLI is not available.
    """
    global _active, _original_hf_hub_download, _original_snapshot_download

    if _active:
        logger.debug("modeld-hook already active")
        return True

    # Check if modeld CLI is available
    try:
        subprocess.run(
            ["modeld", "--version"],
            capture_output=True,
            check=True,
            timeout=5,
        )
    except (FileNotFoundError, subprocess.CalledProcessError, subprocess.TimeoutExpired):
        if verbose:
            logger.warning(
                "modeld CLI not found. Install it from https://github.com/modeld-ai/modeld\n"
                "Running without deduplication."
            )
        return False

    # Set HF_HOME to modeld-managed path (Layer 1 — primary strategy)
    hf_home = get_hf_home()
    if hf_home:
        os.environ["HF_HOME"] = hf_home
        if verbose:
            logger.info(f"modeld-hook: HF_HOME set to {hf_home}")
    else:
        if verbose:
            logger.warning("modeld-hook: Could not determine modeld HF cache path")

    # Monkeypatch huggingface_hub (Layer 2 — fallback for edge cases)
    try:
        import huggingface_hub
        from . import intercept

        _original_hf_hub_download = huggingface_hub.hf_hub_download
        huggingface_hub.hf_hub_download = intercept.make_hf_hub_download_wrapper(
            _original_hf_hub_download
        )

        # Also patch the module-level reference used by diffusers/transformers
        try:
            import huggingface_hub.file_download as _fd
            _fd.hf_hub_download = huggingface_hub.hf_hub_download
        except ImportError:
            pass

        _active = True
        if verbose:
            logger.info("modeld-hook: HuggingFace download interception active")
        return True

    except ImportError:
        # huggingface_hub not installed — HF_HOME strategy is still in effect
        _active = bool(hf_home)
        if verbose and not hf_home:
            logger.debug("huggingface_hub not installed; HF_HOME strategy inactive too")
        return _active

    except Exception as e:
        logger.error(f"modeld-hook: Failed to activate monkeypatch: {e}")
        # HF_HOME strategy is still active even if monkeypatch fails
        _active = bool(hf_home)
        return _active


def deactivate():
    """Restore original huggingface_hub functions."""
    global _active, _original_hf_hub_download, _original_snapshot_download

    if not _active:
        return

    try:
        import huggingface_hub
        if _original_hf_hub_download is not None:
            huggingface_hub.hf_hub_download = _original_hf_hub_download
            _original_hf_hub_download = None

        try:
            import huggingface_hub.file_download as _fd
            if _original_hf_hub_download is not None:
                _fd.hf_hub_download = _original_hf_hub_download
        except ImportError:
            pass

    except ImportError:
        pass

    _active = False
    logger.debug("modeld-hook: Deactivated")


# Auto-activate on import (users can opt out with MODELD_NO_AUTO_HOOK=1)
if not os.environ.get("MODELD_NO_AUTO_HOOK"):
    activate()
