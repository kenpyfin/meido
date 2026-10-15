#!/usr/bin/env python3
"""Steel browser skill CLI — session management and Playwright CDP automation."""

from __future__ import annotations

import argparse
import base64
import json
import os
import sys
import traceback
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

from dotenv import load_dotenv

SKILL_DIR = Path(__file__).resolve().parent
load_dotenv(SKILL_DIR / ".env")


def _env_bool(name: str, default: bool = False) -> bool:
    raw = os.getenv(name)
    if raw is None:
        return default
    return raw.strip().lower() in {"1", "true", "yes", "on"}


def _env_int(name: str, default: int) -> int:
    raw = os.getenv(name)
    if raw is None or not raw.strip():
        return default
    return int(raw.strip())


def make_client():
    from steel import Steel

    api_url = os.getenv("STEEL_API_URL", "http://127.0.0.1:13920").rstrip("/")
    api_key = os.getenv("STEEL_API_KEY", "").strip() or None
    return Steel(steel_api_key=api_key, base_url=api_url)


def cdp_url(session) -> str:
    url = session.websocket_url
    api_key = os.getenv("STEEL_API_KEY", "").strip()
    if api_key and "apiKey=" not in url:
        sep = "&" if "?" in url else "?"
        url = f"{url}{sep}apiKey={api_key}"
    return url


def session_to_dict(session) -> dict[str, Any]:
    profile_id = getattr(session, "profile_id", None)
    if profile_id is None and isinstance(session, dict):
        profile_id = session.get("profileId") or session.get("profile_id")
    return {
        "session_id": getattr(session, "id", None) or session.get("id"),
        "session_viewer_url": getattr(session, "session_viewer_url", None)
        or session.get("sessionViewerUrl"),
        "websocket_url": getattr(session, "websocket_url", None)
        or session.get("websocketUrl"),
        "status": getattr(session, "status", None) or session.get("status"),
        "profile_id": profile_id,
    }


def _profiles_api_available(api_url: str) -> bool:
    url = f"{api_url.rstrip('/')}/v1/profiles"
    request = urllib.request.Request(url, method="GET")
    try:
        with urllib.request.urlopen(request, timeout=5) as response:
            return response.status < 400
    except urllib.error.HTTPError as exc:
        return exc.code not in {404, 405}
    except Exception:
        return False


def _create_session_via_api(
    api_url: str,
    *,
    persist: bool = False,
    user_data_dir: str | None = None,
    use_proxy: bool | None = None,
    solve_captcha: bool | None = None,
    profile_id: str | None = None,
    persist_profile: bool = False,
    api_timeout_ms: int = 3_600_000,
) -> dict[str, Any]:
    payload: dict[str, Any] = {"apiTimeout": api_timeout_ms}
    if use_proxy is not None:
        payload["useProxy"] = use_proxy
    if solve_captcha is not None:
        payload["solveCaptcha"] = solve_captcha
    if profile_id:
        payload["profileId"] = profile_id
    if persist_profile:
        payload["persistProfile"] = True
    if persist:
        payload["persist"] = True
    if user_data_dir:
        payload["userDataDir"] = user_data_dir

    body = json.dumps(payload).encode("utf-8")
    request = urllib.request.Request(
        f"{api_url.rstrip('/')}/v1/sessions",
        data=body,
        method="POST",
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=60) as response:
        return json.loads(response.read().decode("utf-8"))


def emit(result: dict[str, Any], output: str | None = None) -> None:
    text = json.dumps(result, indent=2, ensure_ascii=False)
    if output:
        Path(output).write_text(text, encoding="utf-8")
    print(text)


def error_result(message: str, exc: Exception | None = None) -> dict[str, Any]:
    payload: dict[str, Any] = {"error": message}
    if exc is not None:
        payload["traceback"] = traceback.format_exc()
    return payload


def cmd_session_create(args: argparse.Namespace) -> dict[str, Any]:
    api_url = os.getenv("STEEL_API_URL", "http://127.0.0.1:13920").rstrip("/")
    timeout_ms = _env_int("STEEL_SESSION_TIMEOUT_MS", 3_600_000)
    use_proxy = args.use_proxy if args.use_proxy is not None else _env_bool("STEEL_USE_PROXY")
    solve_captcha = (
        args.solve_captcha
        if args.solve_captcha is not None
        else _env_bool("STEEL_SOLVE_CAPTCHA")
    )
    persist = (
        args.persist
        if args.persist is not None
        else _env_bool("STEEL_PERSIST", default=False)
    )
    persist_profile = (
        args.persist_profile
        if args.persist_profile is not None
        else _env_bool("STEEL_PERSIST_PROFILE", default=False)
    )
    profile_id = (
        args.profile_id
        or os.getenv("STEEL_PROFILE_ID", "").strip()
        or None
    )
    user_data_dir = (
        args.user_data_dir
        or os.getenv("STEEL_USER_DATA_DIR", "").strip()
        or None
    )

    if persist or user_data_dir or (persist_profile and not _profiles_api_available(api_url)):
        session_data = _create_session_via_api(
            api_url,
            persist=persist or bool(user_data_dir) or persist_profile,
            user_data_dir=user_data_dir,
            use_proxy=use_proxy,
            solve_captcha=solve_captcha,
            profile_id=profile_id if _profiles_api_available(api_url) else None,
            persist_profile=persist_profile and _profiles_api_available(api_url),
            api_timeout_ms=timeout_ms,
        )
        result = session_to_dict(session_data)
        result["persist"] = persist or bool(user_data_dir) or persist_profile
        result["user_data_dir"] = user_data_dir
    else:
        client = make_client()
        create_kwargs: dict[str, Any] = {
            "api_timeout": timeout_ms,
            "use_proxy": use_proxy,
            "solve_captcha": solve_captcha,
        }
        if persist_profile:
            create_kwargs["persist_profile"] = True
        if profile_id:
            create_kwargs["profile_id"] = profile_id
        session = client.sessions.create(**create_kwargs)
        result = session_to_dict(session)
        if persist_profile:
            result["persist_profile"] = True

    if result.get("profile_id"):
        result["message"] = (
            "Loaded persisted profile. Share session_viewer_url for manual login if needed, "
            "then run browse commands. Release the session to save profile updates."
        )
    elif result.get("persist") or persist_profile:
        result["message"] = (
            "Persistent session created. Log in via session_viewer_url, then release the session "
            "to save browser state for future Docker restarts."
        )
    else:
        result["message"] = (
            "Share session_viewer_url with the user for manual login before browse commands."
        )
    return result


def cmd_session_status(args: argparse.Namespace) -> dict[str, Any]:
    client = make_client()
    session = client.sessions.retrieve(args.session_id)
    return session_to_dict(session)


def cmd_session_release(args: argparse.Namespace) -> dict[str, Any]:
    client = make_client()
    response = client.sessions.release(args.session_id)
    return {
        "session_id": args.session_id,
        "released": True,
        "response": getattr(response, "model_dump", lambda: str(response))()
        if hasattr(response, "model_dump")
        else str(response),
    }


from contextlib import contextmanager


@contextmanager
def page_session(session_id: str):
    from playwright.sync_api import sync_playwright

    client = make_client()
    session = client.sessions.retrieve(session_id)
    pw = sync_playwright().start()
    browser = pw.chromium.connect_over_cdp(cdp_url(session))
    try:
        context = browser.contexts[0] if browser.contexts else browser.new_context()
        page = context.pages[0] if context.pages else context.new_page()
        yield page
    finally:
        try:
            browser.close()
        except Exception:
            pass
        pw.stop()


def _interactive_elements(page, limit: int = 40) -> list[dict[str, str]]:
    elements: list[dict[str, str]] = []
    for selector, role in (
        ("a[href]", "link"),
        ("button", "button"),
        ("input", "input"),
        ("textarea", "textarea"),
        ("select", "select"),
        ("[role='button']", "button"),
    ):
        for el in page.locator(selector).all()[:limit]:
            try:
                tag = el.evaluate("el => el.tagName.toLowerCase()")
                text = (el.inner_text(timeout=500) or "").strip()[:120]
                attrs = el.evaluate(
                    """el => ({
                        id: el.id || '',
                        name: el.getAttribute('name') || '',
                        type: el.getAttribute('type') || '',
                        href: el.getAttribute('href') || '',
                        placeholder: el.getAttribute('placeholder') || '',
                        ariaLabel: el.getAttribute('aria-label') || '',
                    })"""
                )
                elements.append(
                    {
                        "role": role,
                        "tag": tag,
                        "text": text,
                        **{k: str(v) for k, v in attrs.items() if v},
                    }
                )
            except Exception:
                continue
        if len(elements) >= limit:
            break
    return elements[:limit]


def cmd_browse_goto(args: argparse.Namespace) -> dict[str, Any]:
    with page_session(args.session_id) as page:
        page.goto(args.url, wait_until="domcontentloaded", timeout=60_000)
        return {
            "session_id": args.session_id,
            "url": page.url,
            "title": page.title(),
        }


def cmd_browse_snapshot(args: argparse.Namespace) -> dict[str, Any]:
    with page_session(args.session_id) as page:
        body_text = page.locator("body").inner_text(timeout=10_000)
        excerpt = body_text[:4000] + ("…" if len(body_text) > 4000 else "")
        return {
            "session_id": args.session_id,
            "url": page.url,
            "title": page.title(),
            "text_excerpt": excerpt,
            "interactive_elements": _interactive_elements(page),
        }


def cmd_browse_screenshot(args: argparse.Namespace) -> dict[str, Any]:
    with page_session(args.session_id) as page:
        png = page.screenshot(full_page=bool(args.full_page))
        if args.output:
            out = Path(args.output)
            out.parent.mkdir(parents=True, exist_ok=True)
            out.write_bytes(png)
            return {
                "session_id": args.session_id,
                "url": page.url,
                "title": page.title(),
                "screenshot_path": str(out.resolve()),
            }
        return {
            "session_id": args.session_id,
            "url": page.url,
            "title": page.title(),
            "screenshot_base64": base64.b64encode(png).decode("ascii"),
        }


def cmd_browse_click(args: argparse.Namespace) -> dict[str, Any]:
    with page_session(args.session_id) as page:
        page.locator(args.selector).first.click(timeout=15_000)
        page.wait_for_load_state("domcontentloaded", timeout=15_000)
        return {
            "session_id": args.session_id,
            "clicked": args.selector,
            "url": page.url,
            "title": page.title(),
        }


def cmd_browse_fill(args: argparse.Namespace) -> dict[str, Any]:
    with page_session(args.session_id) as page:
        page.locator(args.selector).first.fill(args.text, timeout=15_000)
        return {
            "session_id": args.session_id,
            "filled": args.selector,
            "url": page.url,
            "title": page.title(),
        }


def cmd_browse_text(args: argparse.Namespace) -> dict[str, Any]:
    with page_session(args.session_id) as page:
        text = page.locator(args.selector).first.inner_text(timeout=15_000)
        return {
            "session_id": args.session_id,
            "selector": args.selector,
            "url": page.url,
            "title": page.title(),
            "text": text,
        }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Steel browser automation CLI")
    parser.add_argument("--output", help="Write JSON result to file")
    sub = parser.add_subparsers(dest="command", required=True)

    create = sub.add_parser("session", help="Session lifecycle")
    create_sub = create.add_subparsers(dest="session_cmd", required=True)

    p_create = create_sub.add_parser("create", help="Create a Steel session")
    p_create.add_argument("--use-proxy", action="store_true", default=None)
    p_create.add_argument("--no-use-proxy", action="store_false", dest="use_proxy")
    p_create.add_argument("--solve-captcha", action="store_true", default=None)
    p_create.add_argument("--no-solve-captcha", action="store_false", dest="solve_captcha")
    p_create.add_argument(
        "--persist",
        action="store_true",
        default=None,
        help="Self-hosted: reuse Chrome user-data-dir across sessions (survives container restart when volume-mounted)",
    )
    p_create.add_argument(
        "--persist-profile",
        action="store_true",
        default=None,
        help="Steel Cloud: create or update a named profile",
    )
    p_create.add_argument(
        "--profile-id",
        help="Steel Cloud profile id to load (or set STEEL_PROFILE_ID)",
    )
    p_create.add_argument(
        "--user-data-dir",
        help="Self-hosted Chrome profile path inside the container (default: /app/api/user-data-dir when --persist)",
    )

    p_status = create_sub.add_parser("status", help="Session status")
    p_status.add_argument("session_id")

    p_release = create_sub.add_parser("release", help="Release session")
    p_release.add_argument("session_id")

    browse = sub.add_parser("browse", help="Browse commands (existing session)")
    browse_sub = browse.add_subparsers(dest="browse_cmd", required=True)

    p_goto = browse_sub.add_parser("goto", help="Navigate to URL")
    p_goto.add_argument("session_id")
    p_goto.add_argument("url")

    p_snap = browse_sub.add_parser("snapshot", help="Page snapshot")
    p_snap.add_argument("session_id")

    p_shot = browse_sub.add_parser("screenshot", help="Screenshot")
    p_shot.add_argument("session_id")
    p_shot.add_argument("--output", help="PNG output path")
    p_shot.add_argument("--full-page", action="store_true")

    p_click = browse_sub.add_parser("click", help="Click selector")
    p_click.add_argument("session_id")
    p_click.add_argument("--selector", required=True)

    p_fill = browse_sub.add_parser("fill", help="Fill input")
    p_fill.add_argument("session_id")
    p_fill.add_argument("--selector", required=True)
    p_fill.add_argument("--text", required=True)

    p_text = browse_sub.add_parser("text", help="Extract text")
    p_text.add_argument("session_id")
    p_text.add_argument("--selector", default="body")

    return parser


def dispatch(args: argparse.Namespace) -> dict[str, Any]:
    if args.command == "session":
        if args.session_cmd == "create":
            return cmd_session_create(args)
        if args.session_cmd == "status":
            return cmd_session_status(args)
        if args.session_cmd == "release":
            return cmd_session_release(args)
    if args.command == "browse":
        if args.browse_cmd == "goto":
            return cmd_browse_goto(args)
        if args.browse_cmd == "snapshot":
            return cmd_browse_snapshot(args)
        if args.browse_cmd == "screenshot":
            return cmd_browse_screenshot(args)
        if args.browse_cmd == "click":
            return cmd_browse_click(args)
        if args.browse_cmd == "fill":
            return cmd_browse_fill(args)
        if args.browse_cmd == "text":
            return cmd_browse_text(args)
    return error_result(f"Unknown command: {args.command}")


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    try:
        result = dispatch(args)
        emit(result, args.output)
        return 0 if "error" not in result else 1
    except Exception as exc:
        emit(error_result(str(exc), exc), getattr(args, "output", None))
        return 1


if __name__ == "__main__":
    sys.exit(main())
