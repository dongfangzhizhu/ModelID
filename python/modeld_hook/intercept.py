"""
modeld_hook.intercept: Monkeypatch wrappers for huggingface_hub functions.

These wrappers check the modeld CAS cache before allowing HuggingFace
to perform a download, avoiding redundant network requests.
"""

import functools
import json
import logging
import os
import subprocess
import sys
from pathlib import Path
from typing import Callable, Optional

logger = logging.getLogger(__name__)


def _run_modeld(args: list[str], timeout: int = 30) -> Optional[dict]:
    """
    Run a modeld CLI command and return its JSON output, or None on failure.
    """
    try:
        result = subprocess.run(
            ["modeld"] + args + ["--json"],
            capture_output=True,
            text=True,
            timeout=timeout,
        )
        if result.returncode == 0 and result.stdout.strip():
            return json.loads(result.stdout.strip())
    except (FileNotFoundError, subprocess.TimeoutExpired, json.JSONDecodeError) as e:
        logger.debug(f"modeld CLI call failed: {e}")
    return None


def _check_modeld_cache(
    repo_id: str,
    filename: str,
    revision: str = "main",
) -> Optional[str]:
    """
    Check if modeld already has the file in its cache.
    Returns the local file path if found, None otherwise.
    """
    result = _run_modeld([
        "hf-check",
        repo_id,
        filename,
        "--revision", revision,
    ])
    if result and result.get("found"):
        return result.get("path")
    return None


def _download_via_modeld(
    repo_id: str,
    filename: str,
    revision: str = "main",
    token: Optional[str] = None,
) -> Optional[str]:
    """
    Trigger a download via modeld and return the local path when done.
    """
    cmd = ["hf-download", repo_id, filename, "--revision", revision]
    if token:
        cmd += ["--token", token]

    result = _run_modeld(cmd, timeout=3600)  # 1 hour for large models
    if result and result.get("path"):
        return result["path"]
    return None


def make_hf_hub_download_wrapper(original_func: Callable) -> Callable:
    """
    Create a wrapper around `huggingface_hub.hf_hub_download` that checks
    the modeld cache first, and falls back to the original function.
    """
    @functools.wraps(original_func)
    def wrapper(
        repo_id: str,
        filename: str,
        *,
        revision: Optional[str] = None,
        token: Optional[str] = None,
        **kwargs,
    ):
        rev = revision or "main"

        logger.debug(f"modeld intercept: hf_hub_download({repo_id}, {filename}@{rev})")

        # Step 1: Check modeld cache
        cached_path = _check_modeld_cache(repo_id, filename, rev)
        if cached_path:
            logger.info(f"modeld cache hit: {repo_id}/{filename} → {cached_path}")
            return cached_path

        # Step 2: Try to download via modeld (gets it into CAS + creates HF cache links)
        try:
            modeld_path = _download_via_modeld(repo_id, filename, rev, token)
            if modeld_path:
                logger.info(f"modeld download complete: {repo_id}/{filename} → {modeld_path}")
                return modeld_path
        except Exception as e:
            logger.warning(f"modeld download failed, falling back to HF: {e}")

        # Step 3: Fall back to original HuggingFace download
        logger.debug(f"Falling back to original hf_hub_download for {repo_id}/{filename}")
        return original_func(repo_id, filename, revision=revision, token=token, **kwargs)

    return wrapper


def make_snapshot_download_wrapper(original_func: Callable) -> Callable:
    """
    Create a wrapper around `huggingface_hub.snapshot_download`.
    For now, falls through to original; future versions will cache full repos.
    """
    @functools.wraps(original_func)
    def wrapper(repo_id: str, *, revision: Optional[str] = None, **kwargs):
        logger.debug(f"modeld intercept: snapshot_download({repo_id}@{revision or 'main'})")
        # TODO: Implement per-file caching for snapshot downloads
        return original_func(repo_id, revision=revision, **kwargs)

    return wrapper
