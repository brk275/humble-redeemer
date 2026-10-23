#!/usr/bin/env python3
"""Native packaging only; Python is not an application runtime dependency."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import shutil
import subprocess
import time
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def copy_common(destination, binary_dir, extension="", support=None):
    destination.mkdir(parents=True, exist_ok=True)
    for name in ("humble-redeemer", "hrctl"):
        shutil.copy2(binary_dir / (name + extension), destination / (name + extension))
    support = support or destination
    support.mkdir(parents=True, exist_ok=True)
    shutil.copy2(ROOT / "LICENSE", support)
    for document in ("README.md", "SECURITY.md"):
        shutil.copy2(ROOT / document, support)
    shutil.copytree(ROOT / "docs", support / "docs", dirs_exist_ok=True)
    if (ROOT / "target/notices").exists():
        shutil.copytree(ROOT / "target/notices", support / "third-party-licenses", dirs_exist_ok=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--profile", default="release", choices=("release", "debug"))
    parser.add_argument("--skip-installers", action="store_true")
    parser.add_argument("--demo-app", action="store_true", help="Separate local native-UI verification bundle")
    args = parser.parse_args()
    binary_dir = ROOT / "target" / args.profile
    output = ROOT / "dist"
    output.mkdir(exist_ok=True)
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    arch = "arm64" if platform.machine().lower() in ("arm64", "aarch64") else "x64"
    system = platform.system()
    if system == "Darwin":
        app = output / ("Humble Redeemer Demo.app" if args.demo_app else "Humble Redeemer.app")
        if app.exists(): shutil.rmtree(app)
        resources = app / "Contents/Resources"
        copy_common(app / "Contents/MacOS", binary_dir, support=resources)
        shutil.copy2(ROOT / "assets/icon.svg", resources)
        with (app / "Contents/Info.plist").open("wb") as info:
            plistlib.dump({"CFBundleName": "Humble Redeemer Demo" if args.demo_app else "Humble Redeemer", "CFBundleDisplayName": "Humble Redeemer Demo" if args.demo_app else "Humble Redeemer", "CFBundleExecutable": "humble-redeemer", "CFBundleIdentifier": "io.github.brk275.humble-redeemer.demo" if args.demo_app else "io.github.brk275.humble-redeemer", "CFBundleVersion": version, "CFBundleShortVersionString": version, "CFBundlePackageType": "APPL", "NSHighResolutionCapable": True, "LSMinimumSystemVersion": "12.0", "NSHumanReadableCopyright": "Copyright 2026 brk275. Unofficial utility."}, info)
        subprocess.run(["codesign", "--force", "--sign", "-", str(app / "Contents/MacOS/hrctl")], check=True)
        subprocess.run(["codesign", "--force", "--sign", "-", str(app)], check=True)
        subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)
        if not args.skip_installers:
            dmg = output / f"Humble-Redeemer-{version}-macOS-{arch}-unsigned.dmg"
            staging = output / "dmg-staging"
            staging.mkdir(exist_ok=True)
            shutil.copytree(app, staging / app.name, dirs_exist_ok=True)
            applications = staging / "Applications"
            if not applications.exists():
                applications.symlink_to("/Applications")
            command = ["hdiutil", "create", "-volname", "Humble Redeemer", "-srcfolder", str(staging), "-ov", "-format", "UDZO", str(dmg)]
            for attempt in range(5):
                result = subprocess.run(command, capture_output=True, text=True)
                if result.returncode == 0:
                    print(result.stdout, end="", flush=True)
                    break
                # macOS image services can briefly retain a staging volume.
                # Retry that transient condition only; never hide other errors.
                if "Resource busy" not in result.stderr or attempt == 4:
                    print(result.stderr, end="", flush=True)
                    result.check_returncode()
                time.sleep(2)
            shutil.rmtree(staging)
    elif system == "Windows":
        staging = output / "windows-stage"
        copy_common(staging, binary_dir, ".exe")
        shutil.copy2(ROOT / "assets/icon.svg", staging)
        installer = output / f"Humble-Redeemer-{version}-Windows-{arch}-unsigned.exe"
        nsi = output / "installer.nsi"
        lines = [
            'Unicode True', 'Name "Humble Redeemer"', f'OutFile "{installer}"',
            'InstallDir "$LOCALAPPDATA\\Programs\\HumbleRedeemer"', 'RequestExecutionLevel user',
            'Page directory', 'Page instfiles', 'UninstPage uninstConfirm', 'UninstPage instfiles',
            'Section "Install"', 'SetOutPath "$INSTDIR"', f'File /r "{staging}\\*"',
            'CreateDirectory "$SMPROGRAMS\\Humble Redeemer"',
            'CreateShortcut "$SMPROGRAMS\\Humble Redeemer\\Humble Redeemer.lnk" "$INSTDIR\\humble-redeemer.exe"',
            'CreateShortcut "$DESKTOP\\Humble Redeemer.lnk" "$INSTDIR\\humble-redeemer.exe"',
            'WriteUninstaller "$INSTDIR\\Uninstall.exe"', 'SectionEnd', 'Section "Uninstall"',
            'ExecWait \'"$INSTDIR\\hrctl.exe" quit --pause\'',
            'Delete "$DESKTOP\\Humble Redeemer.lnk"', 'RMDir /r "$SMPROGRAMS\\Humble Redeemer"',
            'RMDir /r "$INSTDIR"', 'SectionEnd',
        ]
        nsi.write_text("\n".join(lines), encoding="utf-8")
        if not args.skip_installers:
            makensis = shutil.which("makensis") or str(Path(os.environ.get("ProgramFiles(x86)", "C:/Program Files (x86)")) / "NSIS/makensis.exe")
            subprocess.run([makensis, str(nsi)], check=True)
    elif system == "Linux":
        appdir = output / "HumbleRedeemer.AppDir"
        copy_common(appdir / "usr/bin", binary_dir)
        launcher = appdir / "AppRun"
        launcher.write_text('#!/bin/sh\nHERE="$(dirname "$(readlink -f "$0")")"\nexec "$HERE/usr/bin/humble-redeemer" "$@"\n')
        launcher.chmod(0o755)
        desktop = '[Desktop Entry]\nType=Application\nName=Humble Redeemer\nExec=humble-redeemer\nIcon=humble-redeemer\nCategories=Utility;Game;\nTerminal=false\n'
        (appdir / "humble-redeemer.desktop").write_text(desktop)
        shutil.copy2(ROOT / "assets/icon.svg", appdir / "humble-redeemer.svg")
        if not args.skip_installers:
            tool = Path(os.environ.get("APPIMAGETOOL", "appimagetool"))
            image = output / f"Humble-Redeemer-{version}-Linux-{arch}.AppImage"
            subprocess.run([str(tool), "--no-appstream", str(appdir), str(image)], env={**os.environ, "ARCH": "x86_64", "APPIMAGE_EXTRACT_AND_RUN": "1"}, check=True)
            deb = output / "deb-stage"
            copy_common(deb / "opt/humble-redeemer", binary_dir)
            (deb / "usr/bin").mkdir(parents=True, exist_ok=True)
            for name in ("humble-redeemer", "hrctl"):
                (deb / "usr/bin" / name).symlink_to("/opt/humble-redeemer/" + name)
            (deb / "usr/share/applications").mkdir(parents=True, exist_ok=True)
            (deb / "usr/share/applications/humble-redeemer.desktop").write_text(desktop)
            icon_dir = deb / "usr/share/icons/hicolor/scalable/apps"
            icon_dir.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / "assets/icon.svg", icon_dir / "humble-redeemer.svg")
            (deb / "DEBIAN").mkdir(exist_ok=True)
            (deb / "DEBIAN/control").write_text(f"Package: humble-redeemer\nVersion: {version}\nArchitecture: amd64\nMaintainer: brk275 <88346297+brk275@users.noreply.github.com>\nDepends: libwebkit2gtk-4.1-0, libgtk-3-0, libdbus-1-3, libx11-6, libxkbcommon0, libxkbcommon-x11-0, libgl1, libfontconfig1\nDescription: Private native Rust Humble-to-Steam key manager\n")
            subprocess.run(["dpkg-deb", "--root-owner-group", "--build", str(deb), str(output / f"humble-redeemer_{version}_amd64.deb")], check=True)
            shutil.rmtree(deb)
    else:
        raise SystemExit("Unsupported packaging platform")
    checksums = []
    for artifact in sorted(output.iterdir()):
        if artifact.is_file() and artifact.suffix in (".dmg", ".exe", ".deb", ".AppImage", ".zip"):
            checksums.append(hashlib.sha256(artifact.read_bytes()).hexdigest() + "  " + artifact.name)
    (output / f"SHA256SUMS-{system}-{arch}.txt").write_text("\n".join(checksums) + "\n")
    print(json.dumps({"platform": system, "architecture": arch, "packaged": True, "signed_by_publisher": False}))


if __name__ == "__main__":
    main()
