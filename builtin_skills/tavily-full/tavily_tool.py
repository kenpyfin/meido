#!/usr/bin/env python3
"""
Call Tavily REST APIs: search, extract, crawl, map, research (+ poll), usage.

Auth: reads TAVILY_API_KEY from the environment, or from ./.env next to this script
(via python-dotenv if available). POST bodies include "api_key"; GET uses
Authorization: Bearer <key> per Tavily docs.

Docs: https://docs.tavily.com/
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from typing import Any, Dict, Optional

BASE = "https://api.tavily.com"

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))


def _load_dotenv() -> None:
    env_path = os.path.join(SCRIPT_DIR, ".env")
    if not os.path.isfile(env_path):
        return
    try:
        from dotenv import load_dotenv  # type: ignore

        load_dotenv(env_path)
    except ImportError:
        with open(env_path, "r", encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line or line.startswith("#") or "=" not in line:
                    continue
                k, _, v = line.partition("=")
                k, v = k.strip(), v.strip().strip('"').strip("'")
                if k and k not in os.environ:
                    os.environ[k] = v


def api_key() -> str:
    _load_dotenv()
    k = os.getenv("TAVILY_API_KEY", "").strip()
    if not k:
        print(
            json.dumps(
                {
                    "error": "TAVILY_API_KEY missing. Set it in the bot .env or in this skill's .env",
                },
                indent=2,
            ),
            file=sys.stderr,
        )
        sys.exit(2)
    return k


def _headers_json_post(key: str) -> Dict[str, str]:
    return {
        "Content-Type": "application/json",
        "Authorization": f"Bearer {key}",
    }


def _headers_bearer(key: str) -> Dict[str, str]:
    return {"Authorization": f"Bearer {key}"}


def _request_json(
    method: str,
    path: str,
    body: Optional[Dict[str, Any]] = None,
    timeout: float = 120.0,
) -> Dict[str, Any]:
    key = api_key()
    url = f"{BASE}{path}"
    data_bytes: Optional[bytes] = None
    headers: Dict[str, str]
    if method.upper() == "GET":
        headers = _headers_bearer(key)
    else:
        headers = _headers_json_post(key)
        payload = dict(body or {})
        payload.setdefault("api_key", key)
        data_bytes = json.dumps(payload).encode("utf-8")

    req = urllib.request.Request(url, data=data_bytes, headers=headers, method=method.upper())
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read().decode("utf-8", errors="replace")
            return json.loads(raw) if raw.strip() else {}
    except urllib.error.HTTPError as e:
        try:
            err_body = e.read().decode("utf-8", errors="replace")
        except Exception:
            err_body = ""
        try:
            parsed = json.loads(err_body) if err_body.strip() else {}
        except json.JSONDecodeError:
            parsed = {"raw": err_body}
        return {"http_error": e.code, "detail": parsed, "path": path}


def cmd_usage(_: argparse.Namespace) -> int:
    key = api_key()
    req = urllib.request.Request(
        f"{BASE}/usage",
        headers=_headers_bearer(key),
        method="GET",
    )
    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            print(resp.read().decode("utf-8"))
    except urllib.error.HTTPError as e:
        err = e.read().decode("utf-8", errors="replace")
        print(json.dumps({"http_error": e.code, "body": err}, indent=2))
        return 1
    return 0


def cmd_search(args: argparse.Namespace) -> int:
    body: Dict[str, Any] = {"query": args.query}
    if args.max_results is not None:
        body["max_results"] = args.max_results
    if args.search_depth:
        body["search_depth"] = args.search_depth
    if args.topic:
        body["topic"] = args.topic
    if args.time_range:
        body["time_range"] = args.time_range
    if args.include_answer:
        body["include_answer"] = True
    out = _request_json("POST", "/search", body, timeout=60.0)
    print(json.dumps(out, indent=2))
    return 0 if "http_error" not in out else 1


def cmd_extract(args: argparse.Namespace) -> int:
    urls = [u.strip() for u in args.urls.split(",") if u.strip()]
    body: Dict[str, Any] = {"urls": urls if len(urls) > 1 else urls[0]}
    if args.query:
        body["query"] = args.query
    if args.format:
        body["format"] = args.format
    if args.extract_depth:
        body["extract_depth"] = args.extract_depth
    if args.chunks_per_source is not None:
        body["chunks_per_source"] = args.chunks_per_source
    if args.include_images:
        body["include_images"] = True
    out = _request_json("POST", "/extract", body, timeout=120.0)
    print(json.dumps(out, indent=2))
    return 0 if "http_error" not in out else 1


def _normalize_url(url: str) -> str:
    u = url.strip()
    if not u.startswith(("http://", "https://")):
        return "https://" + u
    return u


def cmd_crawl(args: argparse.Namespace) -> int:
    body: Dict[str, Any] = {"url": _normalize_url(args.url)}
    if args.instructions:
        body["instructions"] = args.instructions
    if args.limit is not None:
        body["limit"] = args.limit
    if args.max_depth is not None:
        body["max_depth"] = args.max_depth
    if args.max_breadth is not None:
        body["max_breadth"] = args.max_breadth
    if args.format:
        body["format"] = args.format
    if args.extract_depth:
        body["extract_depth"] = args.extract_depth
    out = _request_json("POST", "/crawl", body, timeout=float(args.timeout or 150))
    print(json.dumps(out, indent=2))
    return 0 if "http_error" not in out else 1


def cmd_map(args: argparse.Namespace) -> int:
    body: Dict[str, Any] = {"url": _normalize_url(args.url)}
    if args.instructions:
        body["instructions"] = args.instructions
    if args.limit is not None:
        body["limit"] = args.limit
    if args.max_depth is not None:
        body["max_depth"] = args.max_depth
    if args.max_breadth is not None:
        body["max_breadth"] = args.max_breadth
    out = _request_json("POST", "/map", body, timeout=float(args.timeout or 150))
    print(json.dumps(out, indent=2))
    return 0 if "http_error" not in out else 1


def cmd_research(args: argparse.Namespace) -> int:
    body: Dict[str, Any] = {"input": args.input, "model": args.model or "auto"}
    if args.stream:
        body["stream"] = True
    out = _request_json("POST", "/research", body, timeout=60.0)
    print(json.dumps(out, indent=2))
    return 0 if "http_error" not in out else 1


def cmd_research_wait(args: argparse.Namespace) -> int:
    key = api_key()
    rid = args.request_id
    deadline = time.time() + float(args.max_wait)
    interval = float(args.interval)
    safe_rid = urllib.parse.quote(rid, safe="")
    path = f"/research/{safe_rid}"
    while time.time() < deadline:
        req = urllib.request.Request(
            f"{BASE}{path}",
            headers=_headers_bearer(key),
            method="GET",
        )
        try:
            with urllib.request.urlopen(req, timeout=60) as resp:
                code = resp.getcode()
                raw = resp.read().decode("utf-8", errors="replace")
                data = json.loads(raw) if raw.strip() else {}
        except urllib.error.HTTPError as e:
            try:
                raw = e.read().decode("utf-8", errors="replace")
                data = json.loads(raw) if raw.strip() else {"raw": raw}
            except Exception:
                data = {"error": str(e)}
            print(json.dumps({"http_status": e.code, "body": data}, indent=2))
            return 1

        if code == 202:
            time.sleep(interval)
            continue
        if code == 200:
            print(json.dumps(data, indent=2))
            st = data.get("status")
            return 0 if st in ("completed", "failed") else 1
        print(json.dumps({"http_status": code, "body": data}, indent=2))
        return 1
    print(json.dumps({"error": "timeout waiting for research", "request_id": rid}, indent=2))
    return 1


def main() -> int:
    p = argparse.ArgumentParser(description="Tavily API CLI")
    sub = p.add_subparsers(dest="cmd", required=True)

    u = sub.add_parser("usage", help="GET /usage — credits and plan")
    u.set_defaults(func=cmd_usage)

    s = sub.add_parser("search", help="POST /search — same family as built-in web_search")
    s.add_argument("--query", required=True)
    s.add_argument("--max-results", type=int, dest="max_results")
    s.add_argument("--search-depth", dest="search_depth")
    s.add_argument("--topic")
    s.add_argument("--time-range", dest="time_range")
    s.add_argument("--include-answer", action="store_true", dest="include_answer")
    s.set_defaults(func=cmd_search)

    ex = sub.add_parser("extract", help="POST /extract — clean content from URLs")
    ex.add_argument(
        "--urls",
        required=True,
        help="Comma-separated URLs (max 20 per Tavily limits)",
    )
    ex.add_argument("--query", help="Rerank chunks to this intent")
    ex.add_argument("--format", choices=("markdown", "text"))
    ex.add_argument("--extract-depth", dest="extract_depth", choices=("basic", "advanced"))
    ex.add_argument("--chunks-per-source", type=int, dest="chunks_per_source")
    ex.add_argument("--include-images", action="store_true", dest="include_images")
    ex.set_defaults(func=cmd_extract)

    cr = sub.add_parser("crawl", help="POST /crawl — graph crawl from base URL")
    cr.add_argument("--url", required=True)
    cr.add_argument("--instructions")
    cr.add_argument("--limit", type=int)
    cr.add_argument("--max-depth", type=int, dest="max_depth")
    cr.add_argument("--max-breadth", type=int, dest="max_breadth")
    cr.add_argument("--format", choices=("markdown", "text"))
    cr.add_argument("--extract-depth", dest="extract_depth", choices=("basic", "advanced"))
    cr.add_argument("--timeout", type=float, default=150.0)
    cr.set_defaults(func=cmd_crawl)

    m = sub.add_parser("map", help="POST /map — site URL discovery")
    m.add_argument("--url", required=True)
    m.add_argument("--instructions")
    m.add_argument("--limit", type=int)
    m.add_argument("--max-depth", type=int, dest="max_depth")
    m.add_argument("--max-breadth", type=int, dest="max_breadth")
    m.add_argument("--timeout", type=float, default=150.0)
    m.set_defaults(func=cmd_map)

    r = sub.add_parser("research", help="POST /research — start async research task")
    r.add_argument("--input", required=True)
    r.add_argument("--model", choices=("auto", "mini", "pro"), default="auto")
    r.add_argument("--stream", action="store_true", help="Request SSE stream (advanced)")
    r.set_defaults(func=cmd_research)

    rw = sub.add_parser(
        "research-wait",
        help="GET /research/{id} until completed/failed or timeout",
    )
    rw.add_argument("--request-id", required=True, dest="request_id")
    rw.add_argument("--interval", type=float, default=5.0)
    rw.add_argument("--max-wait", type=float, default=600.0, dest="max_wait")
    rw.set_defaults(func=cmd_research_wait)

    args = p.parse_args()
    return int(args.func(args))


if __name__ == "__main__":
    raise SystemExit(main())
