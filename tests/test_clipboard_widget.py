"""Exercise real QML clipboard pipes without touching the desktop clipboard."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[1]


@unittest.skipUnless(shutil.which("qs"), "needs Quickshell")
class ClipboardWidget(unittest.TestCase):
    def test_private_text_uses_stdin_and_preserves_unicode_and_newlines(self):
        with tempfile.TemporaryDirectory(prefix="ck-clipboard-") as directory:
            root = Path(directory)
            runtime = root / "run"
            runtime.mkdir(mode=0o700)
            shell = root / "shell"
            shell.mkdir()
            (shell / "gui").mkdir()
            shutil.copy(REPO / "ui/Service.qml", shell / "gui/Service.qml")
            stubs = root / "stubs"
            stubs.mkdir()
            copier = stubs / "wl-copy"
            copier.write_text(
                "#!/usr/bin/env python3\n"
                "import json, os, pathlib, sys\n"
                "text = sys.stdin.read()\n"
                "raw = pathlib.Path('/proc/self/cmdline').read_bytes()\n"
                "pathlib.Path(os.environ['COPY_RESULT']).write_text(\n"
                "    json.dumps({'text': text, 'argv': sys.argv[1:], 'cmdline': raw.decode()}))\n"
            )
            copier.chmod(0o755)
            payload = 'private/Faculdade/ação.txt\n"$(touch /tmp/never-run)"\n' + "á秘密" * 30000
            (root / "payload").write_text(payload)
            (shell / "shell.qml").write_text('''
import QtQuick
import Quickshell
import Quickshell.Io
import "gui" as G
ShellRoot {
  G.Service { id: service; powered: false; powerReady: true }
  FileView { id: payload; path: Quickshell.env("COPY_PAYLOAD"); blockLoading: true }
  Timer {
    interval: 100; running: true
    onTriggered: service.copyText(payload.text())
  }
  Timer { interval: 1800; running: true; onTriggered: Qt.quit() }
}
''')
            result_path = root / "result.json"
            env = dict(os.environ, HOME=str(root / "home"), XDG_RUNTIME_DIR=str(runtime),
                       QT_QPA_PLATFORM="offscreen", QT_QPA_PLATFORMTHEME="generic",
                       QT_QUICK_BACKEND="software", PATH=f"{stubs}:{os.environ['PATH']}",
                       COPY_RESULT=str(result_path), COPY_PAYLOAD=str(root / "payload"))
            result = subprocess.run(["qs", "-p", str(shell / "shell.qml")], env=env,
                                    capture_output=True, text=True, timeout=15)
            output = result.stdout + result.stderr
            self.assertEqual(result.returncode, 0, output)
            self.assertTrue(result_path.exists(), output)
            data = json.loads(result_path.read_text())
            self.assertEqual(data["text"], payload)
            self.assertEqual(data["argv"], [])
            self.assertNotIn("private/Faculdade", data["cmdline"])
            self.assertNotIn("ReferenceError", output)
            self.assertNotIn("TypeError", output)


if __name__ == "__main__":
    unittest.main(verbosity=2)
