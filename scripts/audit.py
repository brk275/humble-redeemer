#!/usr/bin/env python3
"""Publication gate. Reports filenames/counts, never matching private values."""
import argparse
import csv
import os
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]
FAKE_KEYS = {b"AAAAA-BBBBB-CCCCC", b"DDDDD-EEEEE-FFFFF"}
KEY = re.compile(rb"\b[A-Z0-9]{5}(?:-[A-Z0-9]{5}){2,4}\b")
STEAM_ID = re.compile(rb"\b7656119[0-9]{10}\b")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--private-history", type=Path)
    parser.add_argument("--artifacts", type=Path)
    args = parser.parse_args()
    forbidden = {str(Path.home()).encode()}
    if args.private_history:
        for name in ("redeemed.csv", "already_owned.csv", "errored.csv"):
            source = args.private_history / name
            if source.exists():
                with source.open(encoding="utf-8-sig", newline="") as handle:
                    for row in csv.reader(handle):
                        if len(row) >= 3:
                            for value in (row[0], row[2]):
                                if len(value) >= 10 and value not in ("None", "False"):
                                    forbidden.add(value.encode())
    tracked = subprocess.check_output(["git", "ls-files", "-z"], cwd=ROOT).split(b"\0")
    files = [ROOT / os.fsdecode(name) for name in tracked if name]
    if not files:
        raise SystemExit("Stage the intended public source before running the publication audit")
    if args.artifacts:
        files += [p for p in args.artifacts.rglob("*") if p.is_file() and p.suffix not in (".dmg", ".AppImage", ".zip", ".deb")]
    violations = []
    def inspect(label, data):
        if any(secret and secret in data for secret in forbidden):
            violations.append((label, "known private value or home directory"))
        if any(match.group() not in FAKE_KEYS for match in KEY.finditer(data)):
            violations.append((label, "unexpected product-key-shaped value"))
        if STEAM_ID.search(data):
            violations.append((label, "account identifier"))
        if re.search(rb"(?:gh[pousr]_[A-Za-z0-9]{20,}|-----BEGIN (?:RSA |OPENSSH |EC )?PRIVATE KEY-----)", data):
            violations.append((label, "credential pattern"))
    for path in files:
        if not path.is_file():
            continue
        label = str(path.relative_to(ROOT)) if path.is_relative_to(ROOT) else path.name
        if path.suffix in (".csv", ".sqlite", ".db", ".log", ".p12", ".pem") and path.is_relative_to(ROOT) and not args.artifacts:
            violations.append((label, "runtime/private file staged"))
        inspect(label, path.read_bytes())
    commits = subprocess.run(["git", "rev-list", "--all"], cwd=ROOT, capture_output=True, text=True).stdout.splitlines()
    blobs = set()
    if commits:
        for entry in subprocess.check_output(["git", "rev-list", "--objects", "--all"], cwd=ROOT, text=True).splitlines():
            object_id = entry.split()[0]
            if subprocess.check_output(["git", "cat-file", "-t", object_id], cwd=ROOT).strip() == b"blob":
                blobs.add(object_id)
        for object_id in blobs:
            inspect("historical blob " + object_id[:8], subprocess.check_output(["git", "cat-file", "blob", object_id], cwd=ROOT))
        emails = subprocess.check_output(["git", "log", "--all", "--format=%ae%n%ce"], cwd=ROOT, text=True).splitlines()
        if any(not email.endswith("@users.noreply.github.com") for email in emails if email):
            violations.append(("commit metadata", "non-noreply email address"))
    if violations:
        for label, category in sorted(set(violations)):
            print(f"BLOCKED: {label}: {category}")
        raise SystemExit(1)
    print(f"PASS: {len(files)} source/artifact files and {len(blobs)} historical blobs; no matching private values")


if __name__ == "__main__":
    main()
