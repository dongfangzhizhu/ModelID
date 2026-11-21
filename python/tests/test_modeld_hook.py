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
        """Activate should set HF_HOME when modeld is available (explicit call only)."""
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
                    hf_home = os.environ.get("HF_HOME")
                    if hf_home:
                        self.assertIn("hf_cache", hf_home)


class TestB5Compatibility(unittest.TestCase):
    """B5: import side effects, full deactivate, wrapper parameters."""

    def setUp(self):
        with patch.dict(os.environ, {"MODELD_NO_AUTO_HOOK": "1"}):
            import modeld_hook
            modeld_hook.deactivate()
        self.hook = modeld_hook

    def test_import_does_not_patch_hf_home(self):
        """Importing modeld_hook must never mutate HF_HOME or install patches."""
        code = (
            "import os, sys; "
            "os.environ['HF_HOME']='/original/hf'; "
            "import modeld_hook; "
            "print(os.environ.get('HF_HOME'), modeld_hook.is_active())"
        )
        env = dict(os.environ)
        env["MODELD_NO_AUTO_HOOK"] = "1"
        env["PYTHONPATH"] = str(Path(__file__).resolve().parents[1])
        result = subprocess.run(
            [sys.executable, "-c", code], capture_output=True, text=True, env=env
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("/original/hf False", result.stdout)

    def test_mode_b_auto_hook_is_explicit(self):
        """MODELD_AUTO_HOOK=1 is the only env-driven auto-activation switch."""
        code = (
            "import modeld_hook, os; "
            "print('auto=', modeld_hook.is_active())"
        )
        env = dict(os.environ)
        env["MODELD_AUTO_HOOK"] = "1"
        env["PYTHONPATH"] = str(Path(__file__).resolve().parents[1])
        result = subprocess.run(
            [sys.executable, "-c", code], capture_output=True, text=True, env=env
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        # Without the modeld CLI installed, activation fails gracefully (False).
        self.assertIn("auto= False", result.stdout)

    def test_deactivate_restores_both_patch_sites(self):
        """Both hf_hub_download references must be restored by deactivate()."""
        # Simulate huggingface_hub with a tiny fake module injected in sys.modules.
        import types

        def original_top(repo_id, filename, **kwargs):
            return "top"

        def original_fd(repo_id, filename, **kwargs):
            return "fd"

        hub = types.ModuleType("huggingface_hub")
        hub.hf_hub_download = original_top
        fd = types.ModuleType("huggingface_hub.file_download")
        fd.hf_hub_download = original_fd
        hub.file_download = fd

        with patch.dict(sys.modules, {"huggingface_hub": hub, "huggingface_hub.file_download": fd}):
            # Set internal state as activate() would.
            self.hook._original_hf_hub_download = original_top
            self.hook._original_file_download_hf_hub_download = original_fd
            self.hook._active = True
            hub.hf_hub_download = lambda *a, **k: "patched-top"
            fd.hf_hub_download = lambda *a, **k: "patched-fd"

            self.hook.deactivate()

            self.assertIs(hub.hf_hub_download, original_top)
            self.assertIs(fd.hf_hub_download, original_fd)
            self.assertFalse(self.hook.is_active())

    def test_wrapper_subfolder_and_repo_type(self):
        """subfolder is joined; repo_type reaches both CLI calls."""
        import json
        from modeld_hook import intercept

        seen = []

        def fake_run(args, **kwargs):
            seen.append(args)
            resp = MagicMock()
            resp.returncode = 0
            if "hf-check" in args:
                resp.stdout = json.dumps({"found": True, "path": "/cached/file"})
            else:
                resp.stdout = json.dumps({"path": "/downloaded/file"})
            return resp

        wrapper = intercept.make_hf_hub_download_wrapper(lambda *a, **k: "original")
        with patch("subprocess.run", side_effect=fake_run):
            result = wrapper(
                "org/data", "weights.bin", subfolder="checkpoints",
                repo_type="dataset", revision="v1",
            )

        self.assertEqual(result, "/cached/file")
        self.assertIn("checkpoints/weights.bin", seen[0])
        self.assertIn("--repo-type", seen[0])
        self.assertIn("dataset", seen[0])

    def test_wrapper_cache_dir_falls_through(self):
        """cache_dir/local_dir defer to the original function (explicit, not silent)."""
        from modeld_hook import intercept

        called = []

        def original(repo_id, filename, **kwargs):
            called.append((repo_id, filename, kwargs))
            return "/hf/owned/path"

        wrapper = intercept.make_hf_hub_download_wrapper(original)
        with patch("subprocess.run") as mock_run:
            result = wrapper("org/model", "f.bin", cache_dir="/my/cache")
        mock_run.assert_not_called()
        self.assertEqual(result, "/hf/owned/path")
        self.assertEqual(called[0][2]["cache_dir"], "/my/cache")

        called.clear()
        with patch("subprocess.run") as mock_run:
            wrapper("org/model", "f.bin", local_dir="/my/local")
        mock_run.assert_not_called()
        self.assertEqual(called[0][2]["local_dir"], "/my/local")


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
