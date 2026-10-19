#!/usr/bin/env python3
"""Exercise settings controls with a mock backend; no real account is changed."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[1]
SHELL = Path('/usr/share/omarchy/shell')

@unittest.skipUnless(shutil.which('qs') and SHELL.is_dir(), 'needs Quickshell and Omarchy')
class SettingsWidget(unittest.TestCase):
    def test_settings_controls_and_live_theme(self):
        with tempfile.TemporaryDirectory(prefix='ck-settings-') as directory:
            root = Path(directory)
            runtime = root / 'run'
            runtime.mkdir(mode=0o700)
            shell = root / 'shell'
            shell.mkdir()
            for name in ('Commons', 'Ui', 'services', 'plugins'):
                (shell / name).symlink_to(SHELL / name)
            shutil.copytree(REPO / 'ui', shell / 'gui')
            shutil.copy(REPO / 'tests/settings_widget.qml', shell / 'shell.qml')
            env = dict(os.environ, HOME=str(root / 'home'), XDG_RUNTIME_DIR=str(runtime),
                       QT_QPA_PLATFORM='offscreen', QT_QPA_PLATFORMTHEME='generic',
                       QT_QUICK_BACKEND='software')
            display = os.environ.get('WAYLAND_DISPLAY', 'wayland-1')
            if not os.path.isabs(display):
                display = f'/run/user/{os.getuid()}/{display}'
            env['WAYLAND_DISPLAY'] = display
            result = subprocess.run(['qs','-p',str(shell / 'shell.qml')],env=env,
                                    capture_output=True,text=True,timeout=20)
            output = result.stdout + result.stderr
            self.assertEqual(result.returncode,0,output)
            self.assertIn('SETTINGS_WIDGET_PASS',output)
            self.assertNotIn('SETTINGS_WIDGET_FAIL',output)
            self.assertNotIn('ReferenceError',output)
            self.assertNotIn('TypeError',output)
            self.assertNotIn('Unable to assign [undefined] to QColor',output)

if __name__ == '__main__':
    unittest.main(verbosity=2)
