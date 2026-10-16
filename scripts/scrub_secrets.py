#!/usr/bin/env python3
"""Back up and scrub known secrets from agent_history and chat exports.

Prints counts only. Does not print secret values. Does not modify the SQLite DB.
"""

from __future__ import annotations

import argparse
import os
import re
import tarfile
import time
from pathlib import Path

REDACTED = "[REDACTED_SECRET]"
ENV_KEY_HINT = re.compile(
    r"(SECRET|PASSWORD|PASSWD|TOKEN|API_KEY|_KEY|PRIVATE_KEY|CREDENTIAL|USERHASH|_DSN|DATABASE_URL)",
    re.IGNORECASE,
)
KNOWN_KEY = re.compile(
    r"(?<![A-Za-z0-9_-])(?:"
    r"sk-ant-[A-Za-z0-9_-]{20,}|"
    r"sk-[A-Za-z0-9]{20,}|"
    r"AIza[0-9A-Za-z_-]{30,}|"
    r"ghp_[A-Za-z0-9]{20,}|"
    r"github_pat_[A-Za-z0-9_]{20,}|"
    r"xox[baprs]-[A-Za-z0-9-]{10,}|"
    r"crsr_[A-Za-z0-9]{16,}|"
    r"[0-9]{8,10}:[A-Za-z0-9_-]{30,}"
    r")"
)


def parse_env(path: Path) -> list[tuple[str, str]]:
    pairs: list[tuple[str, str]] = []
    if not path.is_file():
        return pairs
    for raw in path.read_text(errors="replace").splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("export "):
            line = line[len("export ") :].strip()
        if "=" not in line:
            continue
        key, value = line.split("=", 1)
        key = key.strip()
        value = value.strip().strip("'").strip('"')
        if key and value and ENV_KEY_HINT.search(key) and len(value) >= 8:
            pairs.append((key, value))
    return pairs


def collect_needles(root: Path) -> list[str]:
    needles: set[str] = set()
    for path in root.rglob("*.env"):
        if "node_modules" in path.parts or ".git" in path.parts:
            continue
        for _key, value in parse_env(path):
            needles.add(value)
    for path in root.rglob(".env"):
        if "node_modules" in path.parts or ".git" in path.parts:
            continue
        for _key, value in parse_env(path):
            needles.add(value)
    return sorted(needles, key=len, reverse=True)


def scrub_text(text: str, needles: list[str]) -> tuple[str, int]:
    count = 0
    out = text
    for needle in needles:
        hits = out.count(needle)
        if hits:
            out = out.replace(needle, REDACTED)
            count += hits
    out, n = KNOWN_KEY.subn(REDACTED, out)
    return out, count + n


def target_files(repo: Path) -> list[Path]:
    found: list[Path] = []
    runtime = repo / "workspace" / "runtime"
    history = runtime / "groups"
    if history.is_dir():
        found.extend(history.glob("*/*/agent_history/*.md"))
    exports = runtime / "exports"
    if exports.is_dir():
        found.extend(exports.glob("*.md"))
    data_exports = repo / "data" / "exports"
    if data_exports.is_dir():
        found.extend(p for p in data_exports.rglob("*") if p.is_file())
    recent = repo / "recent_chat.md"
    if recent.is_file():
        found.append(recent)
    return found


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", default=".", type=Path)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    repo = args.repo.resolve()
    needles = collect_needles(repo)
    files = target_files(repo)
    print(f"needles={len(needles)} files={len(files)} dry_run={args.dry_run}")

    if not args.dry_run and files:
        runtime = repo / "workspace" / "runtime"
        runtime.mkdir(parents=True, exist_ok=True)
        backup = runtime / f"scrub-backup-{time.strftime('%Y%m%d-%H%M%S')}.tar.gz"
        with tarfile.open(backup, "w:gz") as tar:
            for path in files:
                tar.add(path, arcname=str(path.relative_to(repo)))
        os.chmod(backup, 0o600)
        print(f"backup={backup.name}")

    changed = 0
    replacements = 0
    for path in files:
        original = path.read_text(errors="replace")
        updated, count = scrub_text(original, needles)
        if count and updated != original:
            changed += 1
            replacements += count
            if args.dry_run:
                sample = next((n for n in needles if n in original), "")
                masked = (sample[:4] + f"… len={len(sample)}") if sample else "pattern"
                print(f"would_scrub file={path.name} replacements={count} sample={masked}")
            else:
                path.write_text(updated)
                print(f"scrubbed file={path.name} replacements={count}")
    print(f"changed_files={changed} replacements={replacements}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
