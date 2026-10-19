#!/usr/bin/env python3
"""Real QML power toggle with an isolated profile and no cloud accounts."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[1]


@unittest.skipUnless(shutil.which("qs") and shutil.which("rclone"), "needs Quickshell and rclone")
class PowerWidget(unittest.TestCase):
    def test_disable_enable_disable_with_no_automatic_reconnection(self):
        with tempfile.TemporaryDirectory(prefix="ck-widget-") as directory:
            root = Path(directory)
            runtime = root / "run"
            runtime.mkdir(mode=0o700)
            shell = root / "shell"
            shell.mkdir()
            (shell / "gui").mkdir()
            shutil.copy(REPO / "ui/Service.qml", shell / "gui/Service.qml")
            shutil.copy(REPO / "tests/power_widget.qml", shell / "shell.qml")
            stubs = root / "stubs"
            stubs.mkdir()
            systemctl = stubs / "systemctl"
            systemctl.write_text("#!/bin/sh\nexit 1\n")
            systemctl.chmod(0o755)
            env = dict(os.environ, HOME=str(root / "home"),
                       XDG_RUNTIME_DIR=str(runtime), XDG_CONFIG_HOME=str(root / "config"),
                       XDG_STATE_HOME=str(root / "state"), XDG_CACHE_HOME=str(root / "cache"),
                       RCLONE_CONFIG=str(root / "rclone.conf"),
                       QT_QPA_PLATFORM="offscreen", QT_QPA_PLATFORMTHEME="generic",
                       CLOUDKIT_TEST_PLUGIN=str(REPO), PATH=f"{stubs}:{os.environ['PATH']}")
            display = os.environ.get("WAYLAND_DISPLAY", "wayland-1")
            if not os.path.isabs(display):
                display = f"/run/user/{os.getuid()}/{display}"
            env["WAYLAND_DISPLAY"] = display
            (root / "rclone.conf").write_text("")
            cli = REPO / "bin/omarchy-cloudkit"
            def power(action):
                result = subprocess.run([str(cli), "power", action], env=env,
                                        capture_output=True, text=True, timeout=85)
                self.assertEqual(result.returncode, 0, result.stderr)
                return json.loads(result.stdout)
            try:
                self.assertFalse(power("off")["enabled"])
                result = subprocess.run(["qs", "-p", str(shell / "shell.qml")], env=env,
                                        capture_output=True, text=True, timeout=40)
                output = result.stdout + result.stderr
                self.assertEqual(result.returncode, 0, output)
                self.assertIn("POWER_WIDGET_PASS", output)
                self.assertNotIn("POWER_WIDGET_FAIL", output)
                self.assertFalse(power("status")["enabled"])
                self.assertFalse((runtime / "omarchy-cloudkit/daemon.sock").exists())
                self.assertFalse((runtime / "omarchy-cloudkit/rclone.sock").exists())
            finally:
                power("off")


if __name__ == "__main__":
    unittest.main(verbosity=2)
