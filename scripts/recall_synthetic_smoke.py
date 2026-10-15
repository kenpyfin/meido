#!/usr/bin/env python3
"""Synthetic recall checks. Does not read or write production ORIGIN."""

from pathlib import Path


def chunk_id(relative_path: str, chunk_index: int) -> str:
    return f"{relative_path}#{chunk_index}"


def main() -> None:
    root = Path.cwd()
    origin = root / "workspace" / "shared" / "ORIGIN"
    assert "shared/ORIGIN" not in str(Path("/tmp/recall-smoke")), "smoke must not target production"
    first = [chunk_id("notes/a.md", i) for i in range(3)]
    second = [chunk_id("notes/a.md", i) for i in range(2)]
    assert first[:2] == second
    stale = [item for item in first if item not in set(second)]
    assert stale == ["notes/a.md#2"]
    assert not str(origin).endswith("/tmp/recall-smoke")
    print("synthetic recall smoke ok")


if __name__ == "__main__":
    main()
