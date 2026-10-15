#!/usr/bin/env python3
"""Normalize scheduling inputs for schedule_task with explicit UTC behavior."""

from __future__ import annotations

import argparse
import json
import re
import sys
from datetime import datetime
from zoneinfo import ZoneInfo

CRON_TOKEN_RE = re.compile(r"^[\w*/,\-?#LW]+$")


def normalize_cron(expr: str) -> list[str]:
    parts = expr.strip().split()
    if len(parts) == 5:
        parts = ["0"] + parts
    if len(parts) != 6:
        raise ValueError("cron must have 5 or 6 fields")
    for token in parts:
        if not CRON_TOKEN_RE.match(token):
            raise ValueError(f"invalid cron token: {token}")
    return parts


def parse_iso(ts: str) -> datetime:
    normalized = ts.strip()
    if normalized.endswith("Z"):
        normalized = normalized[:-1] + "+00:00"
    return datetime.fromisoformat(normalized)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=["cron", "once"])
    parser.add_argument("value")
    parser.add_argument("--timezone", dest="timezone")
    args = parser.parse_args()

    timezone = args.timezone or "UTC"
    assumption = (
        "Provided timezone is used." if args.timezone else "Timezone not provided. Defaulting to UTC."
    )

    if args.mode == "cron":
        fields = normalize_cron(args.value)
        out = {
            "ok": True,
            "mode": "cron",
            "normalized_cron": " ".join(fields),
            "timezone_used": timezone,
            "assumption": assumption,
        }
        print(json.dumps(out, indent=2))
        return 0

    # mode == once
    dt = parse_iso(args.value)
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=ZoneInfo(timezone))
    utc_dt = dt.astimezone(ZoneInfo("UTC"))
    out = {
        "ok": True,
        "mode": "once",
        "input_timestamp": args.value,
        "normalized_timestamp": dt.isoformat(),
        "utc_timestamp": utc_dt.isoformat(),
        "timezone_used": timezone,
        "assumption": assumption,
    }
    print(json.dumps(out, indent=2))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:  # pragma: no cover
        print(json.dumps({"ok": False, "error": str(exc)}), file=sys.stderr)
        raise SystemExit(1)
