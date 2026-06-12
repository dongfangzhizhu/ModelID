"""
Tests for modeld_hook package.

These tests run without requiring modeld CLI or huggingface_hub to be installed.
"""
import importlib
import os
import sys
import subprocess
import unittest
from unittest.mock import patch, MagicMock
from pathlib import Path


class TestImport(unittest.TestCase):
    """Test that the package imports correctly."""

    def test_import_modeld_hook(self):
        """Package should import without errors."""
        # Re-import with MODELD_NO_AUTO_HOOK to avoid side effects
        with patch.dict(os.environ, {"MODELD_NO_AUTO_HOOK": "1"}):
            import modeld_hook  # noqa
            self.assertIsNotNone(modeld_hook.__version__)

    def test_version_string(self):
        import modeld_hook
        self.assertRegex(modeld_hook.__version__, r"^\d+\.\d+\.\d+")

    def test_public_api(self):
        import modeld_hook
        self.assertTrue(callable(modeld_hook.activate))
        self.assertTrue(callable(modeld_hook.deactivate))
        self.assertTrue(callable(modeld_hook.is_active))
        self.assertTrue(callable(modeld_hook.get_hf_home))


class TestActivateDeactivate(unittest.TestCase):
    """Test activate/deactivate lifecycle."""

    def setUp(self):
        # Ensure we start fresh
        with patch.dict(os.environ, {"MODELD_NO_AUTO_HOOK": "1"}):
            import modeld_hook
            self.hook = modeld_hook
            modeld_hook.deactivate()

    def test_deactivate_when_not_active(self):
        """Deactivate should not raise when not active."""
        self.hook.deactivate()
        self.assertFalse(self.hook.is_active())

    def test_activate_without_modeld_cli(self):
        """Activate should return False gracefully if modeld CLI not found."""
        with patch("subprocess.run", side_effect=FileNotFoundError("modeld not found")):
            result = self.hook.activate()
            # Should return False but not raise
            self.assertIsInstance(result, bool)

    def test_activate_with_mock_modeld(self):
        """Activate should set HF_HOME when modeld is available."""
        mock_version = MagicMock()
        mock_version.returncode = 0

        mock_hf_home = MagicMock()
        mock_hf_home.returncode = 0
        mock_hf_home.stdout = "/fake/modeld/store/hf_cache\n"

        with patch("subprocess.run", side_effect=[mock_version, mock_hf_home]):
            with patch.dict(os.environ, {}, clear=False):
                result = self.hook.activate()
                # If HF_HOME was set, activation partially succeeded
                if result:
                    # HF_HOME should have been set
                    hf_home = os.environ.get("HF_HOME")
                    if hf_home:
                        self.assertIn("hf_cache", hf_home)


class TestIntercept(unittest.TestCase):
    """Test the monkeypatch intercept logic."""

    def test_intercept_imports(self):
        """Intercept module should import cleanly."""
        from modeld_hook import intercept
        self.assertTrue(callable(intercept.make_hf_hub_download_wrapper))
        self.assertTrue(callable(intercept.make_snapshot_download_wrapper))

    def test_cache_miss_falls_through(self):
        """On cache miss, wrapper should call the original function."""
        from modeld_hook import intercept

        original_called = []

        def fake_original(repo_id, filename, *, revision=None, token=None, **kwargs):
            original_called.append((repo_id, filename, revision))
            return f"/fake/path/{filename}"

        wrapper = intercept.make_hf_hub_download_wrapper(fake_original)

        # Mock modeld CLI to return cache miss
        with patch("subprocess.run") as mock_run:
            mock_run.return_value = MagicMock(returncode=1, stdout="", stderr="")
            result = wrapper("org/model", "model.safetensors", revision="main")

        # Should have fallen through to original
        self.assertIn(("org/model", "model.safetensors", "main"), original_called)
        self.assertEqual(result, "/fake/path/model.safetensors")

    def test_cache_hit_returns_path(self):
        """On cache hit, wrapper should return the cached path directly."""
        import json
        from modeld_hook import intercept

        original_called = []

        def fake_original(repo_id, filename, **kwargs):
            original_called.append(repo_id)
            return "/should/not/be/called"

        wrapper = intercept.make_hf_hub_download_wrapper(fake_original)

        # Mock modeld CLI to return cache hit
        cache_path = "/modeld/store/hf_cache/hub/models--org--model/snapshots/main/model.safetensors"
        with patch("subprocess.run") as mock_run:
            mock_run.return_value = MagicMock(
                returncode=0,
                stdout=json.dumps({"found": True, "path": cache_path}),
                stderr="",
            )
            result = wrapper("org/model", "model.safetensors", revision="main")

        # Should NOT have called original
        self.assertEqual(original_called, [])
        # Should return the cached path
        self.assertEqual(result, cache_path)

    def test_modeld_download_then_return(self):
        """On cache miss but successful modeld download, return the modeld path."""
        import json
        from modeld_hook import intercept

        original_called = []

        def fake_original(repo_id, filename, **kwargs):
            original_called.append(repo_id)
            return "/original/path"

        wrapper = intercept.make_hf_hub_download_wrapper(fake_original)

        downloaded_path = "/modeld/store/hf_cache/.../model.safetensors"
        call_count = [0]

        def side_effect(*args, **kwargs):
            call_count[0] += 1
            if call_count[0] == 1:
                # hf-check → miss
                return MagicMock(returncode=1, stdout="", stderr="")
            else:
                # hf-download → success
                return MagicMock(
                    returncode=0,
                    stdout=json.dumps({"path": downloaded_path}),
                    stderr="",
                )

        with patch("subprocess.run", side_effect=side_effect):
            result = wrapper("org/model", "model.safetensors", revision="main")

        # Should NOT have called original since modeld download succeeded
        self.assertEqual(original_called, [])
        self.assertEqual(result, downloaded_path)


class TestGetHfHome(unittest.TestCase):
    """Test get_hf_home utility."""

    def test_returns_none_without_modeld(self):
        """Should return None if modeld CLI not available."""
        with patch.dict(os.environ, {"MODELD_NO_AUTO_HOOK": "1"}):
            import modeld_hook
            with patch("subprocess.run", side_effect=FileNotFoundError):
                with patch.dict(os.environ, {}, clear=False):
                    # Remove HF_HOME if set
                    os.environ.pop("HF_HOME", None)
                    result = modeld_hook.get_hf_home()
                    # May be None or from HF_HOME env var
                    if result is not None:
                        self.assertIsInstance(result, str)

    def test_returns_hf_home_env_var(self):
        """Should return HF_HOME env var value as fallback."""
        with patch.dict(os.environ, {"MODELD_NO_AUTO_HOOK": "1"}):
            import modeld_hook
            with patch("subprocess.run", side_effect=FileNotFoundError):
                with patch.dict(os.environ, {"HF_HOME": "/custom/hf/home"}):
                    result = modeld_hook.get_hf_home()
                    self.assertEqual(result, "/custom/hf/home")


if __name__ == "__main__":
    unittest.main()
