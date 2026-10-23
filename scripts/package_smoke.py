#!/usr/bin/env python3
"""Install/extract each native artifact and launch it with isolated synthetic state."""
import argparse
import os
from pathlib import Path
import platform
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]


def exercise(binary_dir):
    subprocess.run([sys.executable, str(ROOT / 'scripts/smoke.py'), '--bin-dir', str(binary_dir)], check=True, timeout=120)
    suffix = '.exe' if os.name == 'nt' else ''
    with tempfile.TemporaryDirectory(prefix='hr-package-demo-') as data:
        env = {**os.environ, 'HUMBLE_REDEEMER_DATA_DIR': data}
        try:
            command = [str(binary_dir / ('humble-redeemer' + suffix)), '--smoke-test']
            if platform.system() == 'Linux': command = ['xvfb-run', '-a', *command]
            subprocess.run(command, env=env, check=True, timeout=60)
            subprocess.run([*command, '--login-demo'], env=env, check=True, timeout=60)
        finally:
            subprocess.run([str(binary_dir / ('hrctl' + suffix)), '--demo', 'quit', '--pause'], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30)
            time.sleep(1)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--dist', type=Path, default=ROOT / 'dist')
    args = parser.parse_args()
    folder = args.dist.resolve()
    with tempfile.TemporaryDirectory(prefix='hr-install-check-') as temp:
        temporary = Path(temp)
        system = platform.system()
        if system == 'Darwin':
            image = next(folder.glob('*.dmg'))
            mount = temporary / 'volume'
            mount.mkdir()
            subprocess.run(['hdiutil', 'attach', '-readonly', '-nobrowse', '-mountpoint', str(mount), str(image)], check=True)
            try:
                exercise(mount / 'Humble Redeemer.app/Contents/MacOS')
            finally:
                for attempt in range(5):
                    detached=subprocess.run(['hdiutil', 'detach', str(mount)], capture_output=True)
                    if detached.returncode==0: break
                    time.sleep(1)
                else:
                    # The test has explicitly quit its worker. WebKit auxiliary
                    # processes can briefly retain the disposable mounted bundle.
                    subprocess.run(['hdiutil', 'detach', '-force', str(mount)], check=True)
        elif system == 'Windows':
            installer = next(folder.glob('Humble-Redeemer-*-Windows-*.exe'))
            installed = temporary / 'installed'
            subprocess.run([str(installer), '/S', '/D=' + str(installed)], check=True, timeout=60)
            try:
                exercise(installed)
            finally:
                # NSIS deletes itself asynchronously; CI's user profile is disposable.
                subprocess.run([str(installed / 'Uninstall.exe'), '/S'], check=True, timeout=60)
        elif system == 'Linux':
            image = next(folder.glob('*.AppImage'))
            subprocess.run([str(image), '--appimage-extract'], cwd=temporary, check=True, stdout=subprocess.DEVNULL)
            exercise(temporary / 'squashfs-root/usr/bin')
            deb = next(folder.glob('*.deb'))
            subprocess.run(['dpkg-deb', '--extract', str(deb), str(temporary / 'deb')], check=True)
            exercise(temporary / 'deb/opt/humble-redeemer')
        else:
            raise SystemExit('Unsupported platform')
    print('PASS: installed/extracted artifact GUI, worker, CLI and MCP interoperability')


if __name__ == '__main__':
    main()
