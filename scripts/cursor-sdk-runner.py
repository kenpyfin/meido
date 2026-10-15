#!/usr/bin/env python3
"""Cursor SDK sidecar (Python rollback) for FinallyAValueBot.

The default runner is scripts/cursor-sdk-runner.mjs (@cursor/sdk in-process).
Keep this file for CURSOR_SDK_RUNNER_SCRIPT rollback only.

Requires on the host:
  pip install cursor-sdk aiohttp

Environment:
  CURSOR_API_KEY   User API key from Cursor Dashboard → Integrations
  CURSOR_RUN_CONCURRENCY  Max concurrent /run turns (default 4)
  CURSOR_BRIDGE_*  Idle TTL, pool max, orphan grace, launch timeout
  CURSOR_SIDECAR_MAX_UPTIME_SECS  Soft self-recycle when idle (default 86400)
  CURSOR_RUN_WAIT_TIMEOUT_MS      Hang watchdog for run.wait() after stream (default 120000; not a turn budget)

API:
  GET  /health
  POST /admin/request_recycle  Idle-only drain/exit (does not cancel in-flight /run)
  POST /run
    Body: {"prompt": "...", "cwd": "...", "model": "composer-2.5", "agent_id": "..."}
    Response: NDJSON stream
      {"type":"text","text":"..."}
      {"type":"done","status":"finished","agent_id":"...","result":"..."}
      {"type":"error","message":"..."}
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import re
import signal
import subprocess
import sys
import time
import threading
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from typing import Any, AsyncIterator

try:
    from aiohttp import web
except ImportError as exc:  # pragma: no cover
    print(
        "cursor-sdk-runner requires aiohttp: pip install aiohttp cursor-sdk",
        file=sys.stderr,
    )
    raise SystemExit(1) from exc

DEFAULT_PORT = 3848
DEFAULT_MODEL = "composer-2.5"
BRIDGE_RETRY_MAX_ATTEMPTS = 3
BRIDGE_RETRY_BACKOFF_SECS = (0.5, 1.5, 3.0)
DEFAULT_BRIDGE_LAUNCH_TIMEOUT_SECS = 60
# Warm interactive bridges (main / focus sessions). Scheduled + background
# scopes are one-shot and must not stay pooled.
DEFAULT_BRIDGE_IDLE_TTL_SECS = 600
DEFAULT_BRIDGE_POOL_MAX = 16
DEFAULT_BRIDGE_REAPER_INTERVAL_SECS = 60
# Orphan OS processes not tracked in _POOL (e.g. after client disconnect).
DEFAULT_BRIDGE_ORPHAN_GRACE_SECS = 120
DEFAULT_RUN_CONCURRENCY = 4
DEFAULT_SIDECAR_MAX_UPTIME_SECS = 86400
DEFAULT_RUN_WAIT_TIMEOUT_MS = 120_000
_UUID_RE = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$",
    re.IGNORECASE,
)


def _bridge_launch_timeout_secs() -> int:
    raw = os.environ.get("CURSOR_BRIDGE_LAUNCH_TIMEOUT_SECS", "").strip()
    if not raw:
        return DEFAULT_BRIDGE_LAUNCH_TIMEOUT_SECS
    try:
        return max(30, int(raw))
    except ValueError:
        return DEFAULT_BRIDGE_LAUNCH_TIMEOUT_SECS


def _bridge_idle_ttl_secs() -> int:
    raw = os.environ.get("CURSOR_BRIDGE_IDLE_TTL_SECS", "").strip()
    if not raw:
        return DEFAULT_BRIDGE_IDLE_TTL_SECS
    try:
        return max(60, int(raw))
    except ValueError:
        return DEFAULT_BRIDGE_IDLE_TTL_SECS


def _bridge_pool_max() -> int:
    raw = os.environ.get("CURSOR_BRIDGE_POOL_MAX", "").strip()
    if not raw:
        return DEFAULT_BRIDGE_POOL_MAX
    try:
        return max(4, int(raw))
    except ValueError:
        return DEFAULT_BRIDGE_POOL_MAX


def _bridge_orphan_grace_secs() -> int:
    raw = os.environ.get("CURSOR_BRIDGE_ORPHAN_GRACE_SECS", "").strip()
    if not raw:
        return DEFAULT_BRIDGE_ORPHAN_GRACE_SECS
    try:
        return max(30, int(raw))
    except ValueError:
        return DEFAULT_BRIDGE_ORPHAN_GRACE_SECS


def _run_concurrency() -> int:
    raw = os.environ.get("CURSOR_RUN_CONCURRENCY", "").strip()
    if not raw:
        return DEFAULT_RUN_CONCURRENCY
    try:
        return max(1, int(raw))
    except ValueError:
        return DEFAULT_RUN_CONCURRENCY


def _sidecar_max_uptime_secs() -> int:
    raw = os.environ.get("CURSOR_SIDECAR_MAX_UPTIME_SECS", "").strip()
    if not raw:
        return DEFAULT_SIDECAR_MAX_UPTIME_SECS
    try:
        return max(300, int(raw))
    except ValueError:
        return DEFAULT_SIDECAR_MAX_UPTIME_SECS


def _run_wait_timeout_secs() -> float:
    raw = os.environ.get("CURSOR_RUN_WAIT_TIMEOUT_MS", "").strip()
    if not raw:
        n = DEFAULT_RUN_WAIT_TIMEOUT_MS
    else:
        try:
            n = int(raw)
        except ValueError:
            n = DEFAULT_RUN_WAIT_TIMEOUT_MS
    return max(3000, n) / 1000.0


def _is_ephemeral_session_scope(session_scope: str) -> bool:
    """One-shot scopes that must never keep a warm bridge after /run.

    Scheduled tasks use `scheduled:<task_id>:<iso_ts>` (unique each fire).
    Background jobs use a UUID run_key. Both leak FDs if pooled forever.
    """
    scope = session_scope.strip()
    if not scope:
        return False
    if scope.startswith("scheduled:"):
        return True
    if scope.startswith("background:") or scope.startswith("bg:"):
        return True
    return bool(_UUID_RE.match(scope))


@dataclass
class _PooledBridge:
    client: Any
    cwd: str
    session_scope: str
    pool_key: str
    state_root: str
    lock: asyncio.Lock = field(default_factory=asyncio.Lock)
    last_used_monotonic: float = field(default_factory=time.monotonic)


_POOL: dict[str, _PooledBridge] = {}
_POOL_GUARD = asyncio.Lock()
# Serialize cold `Client.launch_bridge` across all pools/personas/sessions.
_BRIDGE_LAUNCH_SEM = asyncio.Semaphore(1)
_REAPER_TASK: asyncio.Task[None] | None = None
# PIDs of bridges currently being launched (not yet in _POOL).
_LAUNCHING_PIDS: set[int] = set()
_ORPHANS_KILLED_TOTAL = 0
_RUNS_IN_FLIGHT = 0
_RUNS_IN_FLIGHT_LOCK = asyncio.Lock()
_STARTED_AT_MONOTONIC = time.monotonic()
_STARTED_AT_UNIX = time.time()
_RECYCLE_REQUESTED = False
_RECYCLE_LOCK = asyncio.Lock()
_SHUTDOWN_TRIGGERED = False


@dataclass
class _ActiveTurn:
    """Tracks the in-flight sync SDK run so disconnect can call run.cancel()."""

    cancel_requested: bool = False
    evict: bool = False
    run: Any | None = None
    lock: threading.Lock = field(default_factory=threading.Lock)


def _cancel_sdk_run(run: Any) -> None:
    if run is None:
        return
    try:
        supports = getattr(run, "supports", None)
        if callable(supports) and not supports("cancel"):
            return
        cancel = getattr(run, "cancel", None)
        if callable(cancel):
            cancel()
    except Exception:
        pass


def _request_turn_cancel(active: _ActiveTurn) -> None:
    with active.lock:
        active.cancel_requested = True
        active.evict = True
        run = active.run
    _cancel_sdk_run(run)


def _wait_sdk_run(run: Any, timeout_s: float) -> Any | None:
    """Hang watchdog for run.wait() after the stream has already ended.

    Tool rounds and long replies live in run.messages() and are not timed here.
    A hung wait() would otherwise pin _RUNS_IN_FLIGHT and block idle recycle.
    """
    wait = getattr(run, "wait", None)
    if not callable(wait):
        return None
    box: dict[str, Any] = {"done": False, "result": None, "err": None}

    def _call() -> None:
        try:
            box["result"] = wait()
        except Exception as err:  # pragma: no cover - SDK/runtime
            box["err"] = err
        finally:
            box["done"] = True

    thread = threading.Thread(target=_call, name="cursor-run-wait", daemon=True)
    thread.start()
    thread.join(timeout_s)
    if not box["done"]:
        print(
            f"[cursor-sdk-runner] run.wait timed out after {timeout_s:.0f}s; cancelling",
            file=sys.stderr,
        )
        _cancel_sdk_run(run)
        return None
    if box["err"] is not None:
        print(f"[cursor-sdk-runner] run.wait failed: {box['err']}", file=sys.stderr)
        return None
    return box["result"]


async def _try_begin_run() -> bool:
    global _RUNS_IN_FLIGHT
    limit = _run_concurrency()
    async with _RUNS_IN_FLIGHT_LOCK:
        if _RUNS_IN_FLIGHT >= limit:
            return False
        _RUNS_IN_FLIGHT += 1
        return True


async def _end_run() -> None:
    global _RUNS_IN_FLIGHT
    async with _RUNS_IN_FLIGHT_LOCK:
        _RUNS_IN_FLIGHT = max(0, _RUNS_IN_FLIGHT - 1)


def _uptime_secs() -> int:
    return max(0, int(time.monotonic() - _STARTED_AT_MONOTONIC))


async def _mark_recycle_requested(reason: str) -> bool:
    """Request idle exit. Returns True if accepted (idle). Never cancels runs."""
    global _RECYCLE_REQUESTED
    async with _RECYCLE_LOCK:
        async with _RUNS_IN_FLIGHT_LOCK:
            in_flight = _RUNS_IN_FLIGHT
        if in_flight > 0:
            return False
        if not _RECYCLE_REQUESTED:
            _RECYCLE_REQUESTED = True
            print(
                f"[cursor-sdk-runner] recycle requested ({reason}); "
                "will exit when idle",
                file=sys.stderr,
            )
        return True


async def _trigger_clean_shutdown(reason: str) -> None:
    """Close pools and stop the process. Safe only when idle."""
    global _SHUTDOWN_TRIGGERED
    async with _RECYCLE_LOCK:
        if _SHUTDOWN_TRIGGERED:
            return
        async with _RUNS_IN_FLIGHT_LOCK:
            if _RUNS_IN_FLIGHT > 0:
                return
        _SHUTDOWN_TRIGGERED = True
    print(
        f"[cursor-sdk-runner] shutting down for recycle ({reason})",
        file=sys.stderr,
    )
    try:
        await _close_all_pooled_bridges()
    except Exception as err:  # pragma: no cover
        print(f"[cursor-sdk-runner] pool close during recycle: {err}", file=sys.stderr)
    # SIGTERM lets web.run_app unwind; hard exit as a backstop.
    try:
        os.kill(os.getpid(), signal.SIGTERM)
    except Exception:
        pass
    asyncio.get_running_loop().call_later(3.0, os._exit, 0)


def _client_is_loopback(request: web.Request) -> bool:
    peer = request.remote or ""
    return peer in ("127.0.0.1", "::1", "localhost")


def _api_key() -> str:
    return os.environ.get("CURSOR_API_KEY", "").strip()


def _is_stale_agent_error(err: Exception) -> bool:
    msg = str(getattr(err, "message", err)).lower()
    return "not found" in msg and "agent" in msg


def _is_busy_agent_error(err: Exception) -> bool:
    name = type(err).__name__
    if "AgentBusy" in name:
        return True
    msg = str(getattr(err, "message", err)).lower()
    return (
        "already has active run" in msg
        or "agent is busy" in msg
        or "agent_busy" in msg
    )


def _is_retryable_bridge_error(err: Exception) -> bool:
    if getattr(err, "is_retryable", False):
        return True
    msg = str(getattr(err, "message", err)).lower()
    needles = (
        "bridge request failed",
        "bridge request timed out",
        "timed out waiting for bridge discovery",
        "bridge discovery",
        "connection refused",
        "errno 111",
        "broken pipe",
        "connection reset",
        "cannot write to closing transport",
        "incomplete chunked read",
        "peer closed connection",
    )
    return any(needle in msg for needle in needles)


def _bridge_pool_key(cwd: str, session_scope: str = "") -> str:
    persona = hashlib.sha256(os.path.realpath(cwd).encode("utf-8")).hexdigest()[:16]
    scope = session_scope.strip()
    if not scope:
        session = "main"
    else:
        session = hashlib.sha256(scope.encode("utf-8")).hexdigest()[:16]
    return f"{persona}:{session}"


def _bridge_state_root(cwd: str, session_scope: str = "") -> str:
    pool_key = _bridge_pool_key(cwd, session_scope).replace(":", "-")
    root = os.environ.get("CURSOR_SDK_STATE_ROOT", "").strip()
    if not root:
        runtime = os.environ.get("FINALLY_A_VALUE_BOT_RUNTIME_DATA", "").strip()
        if not runtime:
            runtime = os.path.join(
                os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                "workspace",
                "runtime",
            )
        root = os.path.join(runtime, "cursor-sdk-state")
    return os.path.join(root, pool_key)


def _close_agent_only(agent: Any | None) -> None:
    if agent is not None:
        try:
            agent.close()
        except Exception:
            pass


def _bridge_process(client: Any) -> Any | None:
    bridge = getattr(client, "_owned_bridge", None)
    if bridge is None:
        return None
    return getattr(bridge, "process", None)


def _force_kill_bridge_process(client: Any) -> None:
    """Last-resort terminate if Client.close() left the subprocess alive."""
    proc = _bridge_process(client)
    if proc is None or proc.poll() is not None:
        return
    try:
        proc.terminate()
        try:
            proc.wait(timeout=3)
        except Exception:
            proc.kill()
            try:
                proc.wait(timeout=3)
            except Exception:
                pass
    except Exception:
        pass


def _bridge_pid(client: Any) -> int | None:
    proc = _bridge_process(client)
    if proc is None:
        return None
    try:
        return int(proc.pid)
    except Exception:
        return None


def _list_sdk_bridge_pids() -> set[int]:
    """PIDs of live cursor-sdk-bridge.js processes (Linux/host)."""
    try:
        out = subprocess.check_output(
            ["pgrep", "-f", "cursor-sdk-bridge.js"],
            text=True,
            stderr=subprocess.DEVNULL,
        )
    except (subprocess.CalledProcessError, FileNotFoundError, OSError):
        return set()
    pids: set[int] = set()
    for line in out.splitlines():
        line = line.strip()
        if line.isdigit():
            pids.add(int(line))
    return pids


def _proc_elapsed_secs(pid: int) -> float | None:
    try:
        out = subprocess.check_output(
            ["ps", "-o", "etimes=", "-p", str(pid)],
            text=True,
            stderr=subprocess.DEVNULL,
        ).strip()
        if not out:
            return None
        return float(out.split()[0])
    except (subprocess.CalledProcessError, FileNotFoundError, OSError, ValueError):
        return None


def _kill_orphan_bridge_processes_sync() -> int:
    """Terminate bridge OS processes not owned by the in-memory pool."""
    global _ORPHANS_KILLED_TOTAL
    owned: set[int] = set(_LAUNCHING_PIDS)
    for entry in list(_POOL.values()):
        pid = _bridge_pid(entry.client)
        if pid is not None:
            owned.add(pid)
    live = _list_sdk_bridge_pids()
    grace = float(_bridge_orphan_grace_secs())
    killed = 0
    for pid in live - owned:
        age = _proc_elapsed_secs(pid)
        if age is not None and age < grace:
            continue
        try:
            os.kill(pid, signal.SIGTERM)
            killed += 1
            print(
                f"[cursor-sdk-runner] killed orphan bridge pid={pid} "
                f"age_secs={age if age is not None else '?'}",
                file=sys.stderr,
            )
        except ProcessLookupError:
            continue
        except OSError as err:
            print(
                f"[cursor-sdk-runner] failed to kill orphan pid={pid}: {err}",
                file=sys.stderr,
            )
    if killed:
        _ORPHANS_KILLED_TOTAL += killed
        # Give processes a moment, then SIGKILL stragglers.
        time.sleep(0.5)
        still = _list_sdk_bridge_pids() - owned
        for pid in still:
            try:
                os.kill(pid, signal.SIGKILL)
            except OSError:
                pass
    return killed


async def _kill_orphan_bridge_processes() -> int:
    return await asyncio.to_thread(_kill_orphan_bridge_processes_sync)


def _close_pooled_bridge(entry: _PooledBridge) -> None:
    client = entry.client
    try:
        client.close()
    except Exception:
        pass
    _force_kill_bridge_process(client)
    print(
        f"[cursor-sdk-runner] closed bridge pool_key={entry.pool_key}",
        file=sys.stderr,
    )


def _bridge_subprocess_exited(client: Any) -> bool:
    proc = _bridge_process(client)
    return proc is not None and proc.poll() is not None


def _bridge_ping_ok(client: Any) -> bool:
    try:
        client.ping()
        return True
    except Exception:
        return False


async def _evict_pooled_bridge(pool_key: str) -> None:
    async with _POOL_GUARD:
        entry = _POOL.pop(pool_key, None)
    if entry is not None:
        await asyncio.to_thread(_close_pooled_bridge, entry)


def _launch_bridge_client(cwd: str, state_root: str, timeout_secs: int) -> Any:
    from cursor_sdk import Client, LocalAgentOptions

    local = LocalAgentOptions(cwd=cwd)
    return Client.launch_bridge(
        workspace=cwd,
        state_root=state_root,
        local=local,
        timeout=timeout_secs,
    )


async def _evict_idle_and_over_cap_bridges() -> int:
    """Close idle warm bridges and enforce a hard pool cap (oldest first)."""
    now = time.monotonic()
    ttl = float(_bridge_idle_ttl_secs())
    cap = _bridge_pool_max()
    to_close: list[_PooledBridge] = []

    async with _POOL_GUARD:
        # Do not call client.ping() under the lock — it can block all /run traffic.
        idle_keys = [
            key
            for key, entry in _POOL.items()
            if (now - entry.last_used_monotonic) >= ttl
            or _bridge_subprocess_exited(entry.client)
        ]
        for key in idle_keys:
            entry = _POOL.pop(key, None)
            if entry is not None:
                to_close.append(entry)

        if len(_POOL) > cap:
            overflow = sorted(
                _POOL.values(),
                key=lambda e: e.last_used_monotonic,
            )[: len(_POOL) - cap]
            for entry in overflow:
                removed = _POOL.pop(entry.pool_key, None)
                if removed is not None:
                    to_close.append(removed)

    if to_close:
        await asyncio.to_thread(_close_entries_parallel, to_close)
    return len(to_close)


def _close_entries_parallel(entries: list[_PooledBridge]) -> None:
    if not entries:
        return
    workers = min(32, max(1, len(entries)))
    with ThreadPoolExecutor(max_workers=workers) as pool:
        list(pool.map(_close_pooled_bridge, entries))


async def _bridge_idle_reaper() -> None:
    while True:
        try:
            await asyncio.sleep(DEFAULT_BRIDGE_REAPER_INTERVAL_SECS)
            closed = await _evict_idle_and_over_cap_bridges()
            orphans = await _kill_orphan_bridge_processes()
            if closed or orphans:
                print(
                    f"[cursor-sdk-runner] reaper closed {closed} bridge(s); "
                    f"orphans_killed={orphans}; pool_size={len(_POOL)}",
                    file=sys.stderr,
                )

            # Soft self-recycle after max uptime when idle (no run cancel).
            if _uptime_secs() >= _sidecar_max_uptime_secs():
                await _mark_recycle_requested("max_uptime")

            if _RECYCLE_REQUESTED:
                async with _RUNS_IN_FLIGHT_LOCK:
                    in_flight = _RUNS_IN_FLIGHT
                if in_flight == 0:
                    await _trigger_clean_shutdown("idle_recycle")
                    return
        except asyncio.CancelledError:
            raise
        except Exception as err:  # pragma: no cover
            print(f"[cursor-sdk-runner] reaper error: {err}", file=sys.stderr)


async def _get_pooled_bridge(cwd: str, session_scope: str = "") -> _PooledBridge:
    pool_key = _bridge_pool_key(cwd, session_scope)
    state_root = _bridge_state_root(cwd, session_scope)
    os.makedirs(state_root, exist_ok=True)

    candidate: _PooledBridge | None = None
    stale_dead: _PooledBridge | None = None
    async with _POOL_GUARD:
        entry = _POOL.get(pool_key)
        if entry is not None and not _bridge_subprocess_exited(entry.client):
            candidate = entry
        elif entry is not None:
            _POOL.pop(pool_key, None)
            stale_dead = entry

    if stale_dead is not None:
        await asyncio.to_thread(_close_pooled_bridge, stale_dead)

    if candidate is not None:
        # Ping outside the global pool lock so a slow/dead bridge cannot stall
        # every other /run and the idle reaper.
        if await asyncio.to_thread(_bridge_ping_ok, candidate.client):
            candidate.last_used_monotonic = time.monotonic()
            return candidate
        await _evict_pooled_bridge(pool_key)

    async with _BRIDGE_LAUNCH_SEM:
        candidate = None
        stale: _PooledBridge | None = None
        async with _POOL_GUARD:
            entry = _POOL.get(pool_key)
            if entry is not None and not _bridge_subprocess_exited(entry.client):
                candidate = entry
            elif entry is not None:
                stale = _POOL.pop(pool_key, None)

        if candidate is not None:
            if await asyncio.to_thread(_bridge_ping_ok, candidate.client):
                candidate.last_used_monotonic = time.monotonic()
                return candidate
            await _evict_pooled_bridge(pool_key)

        if stale is not None:
            await asyncio.to_thread(_close_pooled_bridge, stale)

        # Make room before a cold launch so we never grow past cap.
        await _evict_idle_and_over_cap_bridges()
        if len(_POOL) >= _bridge_pool_max():
            await _evict_idle_and_over_cap_bridges()

        timeout_secs = _bridge_launch_timeout_secs()
        client = await asyncio.to_thread(
            _launch_bridge_client,
            cwd,
            state_root,
            timeout_secs,
        )
        launch_pid = _bridge_pid(client)
        if launch_pid is not None:
            _LAUNCHING_PIDS.add(launch_pid)
        try:
            entry = _PooledBridge(
                client=client,
                cwd=cwd,
                session_scope=session_scope,
                pool_key=pool_key,
                state_root=state_root,
            )
            async with _POOL_GUARD:
                _POOL[pool_key] = entry
        finally:
            if launch_pid is not None:
                _LAUNCHING_PIDS.discard(launch_pid)
        scope_label = session_scope.strip() or "main"
        print(
            "[cursor-sdk-runner] launched bridge for "
            f"persona={cwd} session={scope_label} "
            f"(pool_key={pool_key}, timeout={timeout_secs}s, "
            f"ephemeral={_is_ephemeral_session_scope(session_scope)})",
            file=sys.stderr,
        )
        return entry


async def _close_all_pooled_bridges() -> None:
    async with _POOL_GUARD:
        entries = list(_POOL.values())
        _POOL.clear()
    await asyncio.to_thread(_close_entries_parallel, entries)
    try:
        from cursor_sdk._client import close_default_client

        close_default_client()
    except Exception:
        pass


def _release_cursor_bridge(client: Any | None = None, agent: Any | None = None) -> None:
    """Legacy helper: close a one-off client/agent and any default singleton."""
    _close_agent_only(agent)
    if client is not None:
        try:
            client.close()
        except Exception:
            pass
    try:
        from cursor_sdk._client import close_default_client

        close_default_client()
    except Exception:
        pass


def _serialize_model_param_values(raw_params: Any) -> list[dict[str, str]]:
    out: list[dict[str, str]] = []
    for item in raw_params or []:
        if isinstance(item, dict):
            param_id = str(item.get("id") or "").strip()
            value = str(item.get("value") or "").strip()
        else:
            param_id = str(getattr(item, "id", "") or "").strip()
            value = str(getattr(item, "value", "") or "").strip()
        if param_id and value:
            out.append({"id": param_id, "value": value})
    return out


def _build_model_selection(model: str, model_params: list[dict[str, str]] | None):
    params = _serialize_model_param_values(model_params)
    if not params:
        return model
    try:
        from cursor_sdk import ModelParameterValue, ModelSelection
    except ImportError:
        return model
    return ModelSelection(
        id=model,
        params=[ModelParameterValue(id=p["id"], value=p["value"]) for p in params],
    )


def _serialize_model_entry(model: Any) -> dict[str, Any] | None:
    model_id = getattr(model, "id", None) or getattr(model, "model", None)
    if not model_id:
        return None
    entry: dict[str, Any] = {"id": str(model_id)}
    display_name = getattr(model, "display_name", None) or getattr(model, "displayName", None)
    if display_name:
        entry["display_name"] = str(display_name)

    parameters: list[dict[str, Any]] = []
    for param in getattr(model, "parameters", None) or []:
        param_id = getattr(param, "id", None)
        if not param_id:
            continue
        values: list[dict[str, str]] = []
        for value in getattr(param, "values", None) or []:
            raw_value = getattr(value, "value", None)
            if raw_value is None:
                continue
            value_entry: dict[str, str] = {"value": str(raw_value)}
            value_label = getattr(value, "display_name", None) or getattr(
                value, "displayName", None
            )
            if value_label:
                value_entry["display_name"] = str(value_label)
            values.append(value_entry)
        if not values:
            continue
        param_entry: dict[str, Any] = {"id": str(param_id), "values": values}
        param_label = getattr(param, "display_name", None) or getattr(param, "displayName", None)
        if param_label:
            param_entry["display_name"] = str(param_label)
        parameters.append(param_entry)
    if parameters:
        entry["parameters"] = parameters

    variants: list[dict[str, Any]] = []
    for variant in getattr(model, "variants", None) or []:
        variant_params = _serialize_model_param_values(getattr(variant, "params", None))
        variant_name = getattr(variant, "display_name", None) or getattr(
            variant, "displayName", None
        )
        if not variant_name:
            continue
        variant_entry: dict[str, Any] = {
            "params": variant_params,
            "display_name": str(variant_name),
            "is_default": bool(getattr(variant, "is_default", False) or getattr(variant, "isDefault", False)),
        }
        description = getattr(variant, "description", None)
        if description:
            variant_entry["description"] = str(description)
        variants.append(variant_entry)
    if variants:
        entry["variants"] = variants
    return entry


def _open_agent(
    client: Any,
    *,
    agent_id: str | None,
    model: str,
    model_params: list[dict[str, str]] | None,
    cwd: str,
    api_key: str,
    opts: Any,
    mcp_servers: dict[str, Any] | None,
):
    from cursor_sdk import AgentOptions, LocalAgentOptions

    model_selection = _build_model_selection(model, model_params)
    agent_opts = AgentOptions(
        api_key=api_key,
        model=model_selection,
        local=LocalAgentOptions(cwd=cwd),
        mcp_servers=mcp_servers or None,
    )
    if not agent_id:
        return client.create_agent(agent_opts)

    try:
        return client.resume_agent(agent_id, opts)
    except Exception as err:
        from cursor_sdk import CursorAgentError

        if isinstance(err, CursorAgentError) and _is_stale_agent_error(err):
            return client.create_agent(agent_opts)
        raise


async def _stream_run(
    body: dict[str, Any],
    active_pool_key: list[str | None] | None = None,
    active_turn_slot: list[_ActiveTurn | None] | None = None,
) -> AsyncIterator[str]:
    prompt = (body.get("prompt") or "").strip()
    cwd = (body.get("cwd") or ".").strip() or "."
    model = (body.get("model") or DEFAULT_MODEL).strip() or DEFAULT_MODEL
    model_params = body.get("model_params")
    agent_id = (body.get("agent_id") or "").strip() or None
    session_scope = (body.get("session_scope") or "").strip()
    mcp_servers = body.get("mcp_servers")
    if not isinstance(mcp_servers, dict):
        mcp_servers = None

    if not prompt:
        yield json.dumps({"type": "error", "message": "prompt required"}) + "\n"
        return

    api_key = _api_key()
    if not api_key:
        yield json.dumps(
            {"type": "error", "message": "CURSOR_API_KEY is not set on the sidecar host"}
        ) + "\n"
        return

    try:
        from cursor_sdk import AgentOptions, CursorAgentError
    except ImportError:
        yield json.dumps(
            {
                "type": "error",
                "message": "cursor-sdk is not installed (pip install cursor-sdk)",
            }
        ) + "\n"
        return

    opts = AgentOptions(api_key=api_key)
    resume_id = agent_id
    pool_key = _bridge_pool_key(cwd, session_scope)
    if active_pool_key is not None:
        active_pool_key[0] = pool_key

    for attempt in range(BRIDGE_RETRY_MAX_ATTEMPTS):
        agent = None
        pooled: _PooledBridge | None = None
        active_turn: _ActiveTurn | None = None
        try:
            pooled = await _get_pooled_bridge(cwd, session_scope)
            async with pooled.lock:
                pooled.last_used_monotonic = time.monotonic()
                agent = await asyncio.to_thread(
                    _open_agent,
                    pooled.client,
                    agent_id=resume_id,
                    model=model,
                    model_params=model_params if isinstance(model_params, list) else None,
                    cwd=cwd,
                    api_key=api_key,
                    opts=opts,
                    mcp_servers=mcp_servers,
                )
                active_turn = _ActiveTurn()
                if active_turn_slot is not None:
                    active_turn_slot[0] = active_turn
                async for line in _stream_agent_turn_async(
                    agent, prompt, resume_id, mcp_servers, active_turn
                ):
                    yield line
                return
        except CursorAgentError as err:
            if attempt == 0 and resume_id and _is_stale_agent_error(err):
                resume_id = None
                continue
            if (
                attempt + 1 < BRIDGE_RETRY_MAX_ATTEMPTS
                and _is_busy_agent_error(err)
            ):
                print(
                    "[cursor-sdk-runner] agent busy "
                    f"(attempt {attempt + 1}/{BRIDGE_RETRY_MAX_ATTEMPTS}): {err}",
                    file=sys.stderr,
                )
                await _evict_pooled_bridge(pool_key)
                if attempt >= 1:
                    resume_id = None
                continue
            if (
                attempt + 1 < BRIDGE_RETRY_MAX_ATTEMPTS
                and _is_retryable_bridge_error(err)
            ):
                print(
                    "[cursor-sdk-runner] retryable bridge error "
                    f"(attempt {attempt + 1}/{BRIDGE_RETRY_MAX_ATTEMPTS}): {err}",
                    file=sys.stderr,
                )
                await _evict_pooled_bridge(pool_key)
                await asyncio.sleep(BRIDGE_RETRY_BACKOFF_SECS[attempt])
                continue
            yield json.dumps(
                {
                    "type": "error",
                    "message": (
                        "Cursor SDK startup failed: "
                        f"{getattr(err, 'message', err)}"
                    ),
                }
            ) + "\n"
            return
        except Exception as err:  # pragma: no cover - surfaced to bot logs
            if (
                attempt + 1 < BRIDGE_RETRY_MAX_ATTEMPTS
                and _is_busy_agent_error(err)
            ):
                print(
                    "[cursor-sdk-runner] agent busy "
                    f"(attempt {attempt + 1}/{BRIDGE_RETRY_MAX_ATTEMPTS}): {err}",
                    file=sys.stderr,
                )
                await _evict_pooled_bridge(pool_key)
                if attempt >= 1:
                    resume_id = None
                continue
            if (
                attempt + 1 < BRIDGE_RETRY_MAX_ATTEMPTS
                and _is_retryable_bridge_error(err)
            ):
                print(
                    "[cursor-sdk-runner] retryable bridge error "
                    f"(attempt {attempt + 1}/{BRIDGE_RETRY_MAX_ATTEMPTS}): {err}",
                    file=sys.stderr,
                )
                await _evict_pooled_bridge(pool_key)
                await asyncio.sleep(BRIDGE_RETRY_BACKOFF_SECS[attempt])
                continue
            yield json.dumps({"type": "error", "message": str(err)}) + "\n"
            return
        finally:
            if (
                active_turn_slot is not None
                and active_turn is not None
                and active_turn_slot[0] is active_turn
            ):
                active_turn_slot[0] = None
            _close_agent_only(agent)
            # One-shot scheduled/background scopes must not keep bridges warm.
            # Cancelled / wait-timeout turns evict after pooled.lock is released.
            should_evict = False
            if active_turn is not None:
                with active_turn.lock:
                    should_evict = active_turn.cancel_requested or active_turn.evict
            if pooled is not None and (
                should_evict or _is_ephemeral_session_scope(session_scope)
            ):
                await _evict_pooled_bridge(pool_key)
            elif pooled is not None:
                pooled.last_used_monotonic = time.monotonic()


async def _stream_agent_turn_async(
    agent: Any,
    prompt: str,
    resume_id: str | None,
    mcp_servers: dict[str, Any] | None,
    active: _ActiveTurn,
) -> AsyncIterator[str]:
    """Run sync `_stream_agent_turn` in a worker thread; yield NDJSON on the loop."""
    queue: asyncio.Queue[tuple[str, Any]] = asyncio.Queue()
    loop = asyncio.get_running_loop()

    def producer() -> None:
        try:
            for line in _stream_agent_turn(
                agent, prompt, resume_id, mcp_servers, active
            ):
                asyncio.run_coroutine_threadsafe(queue.put(("line", line)), loop).result()
            asyncio.run_coroutine_threadsafe(queue.put(("end", None)), loop).result()
        except Exception as err:  # pragma: no cover
            asyncio.run_coroutine_threadsafe(queue.put(("error", err)), loop).result()

    producer_task = asyncio.create_task(asyncio.to_thread(producer))
    completed = False
    try:
        while True:
            kind, payload = await queue.get()
            if kind == "line":
                yield payload
            elif kind == "end":
                completed = True
                break
            elif kind == "error":
                raise payload
    finally:
        if not completed:
            await asyncio.to_thread(_request_turn_cancel, active)
        try:
            await producer_task
        except Exception:
            pass


def _stream_agent_turn(
    agent: Any,
    prompt: str,
    resume_id: str | None,
    mcp_servers: dict[str, Any] | None,
    active: _ActiveTurn | None = None,
):
    final_text_parts: list[str] = []
    status = "error"
    # Expire a leftover local run so the next user message is not blocked
    # with "Agent … already has active run".
    send_options: dict[str, Any] = {"local": {"force": True}}
    if mcp_servers:
        send_options["mcp_servers"] = mcp_servers

    returned_agent_id: str | None = getattr(agent, "agent_id", None) or resume_id
    run = agent.send(prompt, send_options)
    if active is not None:
        with active.lock:
            active.run = run
            cancel_now = active.cancel_requested
        if cancel_now:
            _cancel_sdk_run(run)
            with active.lock:
                active.evict = True
            yield json.dumps(
                {"type": "error", "message": "Run cancelled"}
            ) + "\n"
            return

    for message in run.messages():
        if active is not None:
            with active.lock:
                cancel_now = active.cancel_requested
                if cancel_now:
                    active.evict = True
            if cancel_now:
                _cancel_sdk_run(run)
                yield json.dumps(
                    {"type": "error", "message": "Run cancelled"}
                ) + "\n"
                return
        msg_type = getattr(message, "type", None)
        # Some SDK builds use "assistant"; also accept common aliases.
        if msg_type not in (None, "assistant", "assistant_message", "message"):
            # Still try if role looks assistant-like.
            role = getattr(message, "role", None)
            if role != "assistant":
                continue
        payload = getattr(message, "message", None)
        if payload is None:
            payload = message
        content = getattr(payload, "content", None)
        if isinstance(content, str) and content.strip():
            final_text_parts.append(content)
            yield json.dumps({"type": "text", "text": content}) + "\n"
            continue
        for block in content or []:
            if isinstance(block, str):
                if block.strip():
                    final_text_parts.append(block)
                    yield json.dumps({"type": "text", "text": block}) + "\n"
                continue
            block_type = getattr(block, "type", None)
            if block_type == "text":
                text = getattr(block, "text", "") or ""
                if not text:
                    continue
                final_text_parts.append(text)
                yield json.dumps({"type": "text", "text": text}) + "\n"
            elif block_type == "tool_use":
                name = getattr(block, "name", "") or ""
                tool_input = getattr(block, "input", None)
                if not isinstance(tool_input, dict):
                    tool_input = {}
                yield json.dumps(
                    {
                        "type": "tool_use",
                        "name": name,
                        "input": tool_input,
                    }
                ) + "\n"
            elif block_type == "tool_result":
                output = getattr(block, "content", "") or ""
                if not isinstance(output, str):
                    output = str(output)
                yield json.dumps(
                    {
                        "type": "tool_result",
                        "name": getattr(block, "name", "") or "",
                        "output": output,
                        "is_error": bool(getattr(block, "is_error", False)),
                    }
                ) + "\n"
            elif block_type == "thinking":
                thinking = getattr(block, "thinking", "") or getattr(block, "text", "") or ""
                if thinking:
                    yield json.dumps({"type": "thinking", "thinking": thinking}) + "\n"

    if active is not None:
        with active.lock:
            cancel_after_stream = active.cancel_requested
            if cancel_after_stream:
                active.evict = True
        if cancel_after_stream:
            _cancel_sdk_run(run)
            yield json.dumps(
                {"type": "error", "message": "Run cancelled"}
            ) + "\n"
            return

    result = _wait_sdk_run(run, _run_wait_timeout_secs())
    if active is not None:
        with active.lock:
            if active.cancel_requested:
                _cancel_sdk_run(run)
                active.evict = True
                cancelled = True
            else:
                cancelled = False
                if result is None:
                    active.evict = True
        if cancelled:
            yield json.dumps(
                {"type": "error", "message": "Run cancelled"}
            ) + "\n"
            return
    status = getattr(result, "status", None) or "finished"
    result_text = getattr(result, "result", None)
    if isinstance(result_text, str) and result_text.strip():
        # Prefer wait().result when stream collected no assistant text.
        if not any(p.strip() for p in final_text_parts):
            final_text_parts = [result_text]
    returned_agent_id = getattr(agent, "agent_id", None) or returned_agent_id

    joined = "\n\n".join(p.strip() for p in final_text_parts if p.strip())
    yield json.dumps(
        {
            "type": "done",
            "status": status,
            "agent_id": returned_agent_id,
            "result": joined,
        }
    ) + "\n"


async def handle_health(_request: web.Request) -> web.Response:
    cursor_sdk_installed = False
    try:
        import cursor_sdk  # noqa: F401

        cursor_sdk_installed = True
    except ImportError:
        pass
    return web.json_response(
        {
            "ok": True,
            "service": "cursor-sdk-runner",
            "api_key_configured": bool(_api_key()),
            "cursor_sdk_installed": cursor_sdk_installed,
            "mcp_supported": True,
            "persona_bridges_active": len(_POOL),
            "session_scoped_bridges": True,
            "bridge_idle_ttl_secs": _bridge_idle_ttl_secs(),
            "bridge_pool_max": _bridge_pool_max(),
            "bridge_orphan_grace_secs": _bridge_orphan_grace_secs(),
            "run_concurrency": _run_concurrency(),
            "runs_in_flight": _RUNS_IN_FLIGHT,
            "orphans_killed_total": _ORPHANS_KILLED_TOTAL,
            "os_bridge_pids": len(_list_sdk_bridge_pids()),
            "started_at_unix": int(_STARTED_AT_UNIX),
            "uptime_secs": _uptime_secs(),
            "max_uptime_secs": _sidecar_max_uptime_secs(),
            "recycle_requested": _RECYCLE_REQUESTED,
        }
    )


async def handle_request_recycle(request: web.Request) -> web.Response:
    if not _client_is_loopback(request):
        return web.json_response(
            {"accepted": False, "reason": "loopback_only"},
            status=403,
        )
    async with _RUNS_IN_FLIGHT_LOCK:
        in_flight = _RUNS_IN_FLIGHT
    if in_flight > 0:
        return web.json_response(
            {
                "accepted": False,
                "runs_in_flight": in_flight,
                "reason": "busy",
            },
            status=202,
        )
    accepted = await _mark_recycle_requested("admin")
    if accepted:
        # Kick exit promptly; reaper also polls.
        asyncio.create_task(_trigger_clean_shutdown("admin_recycle"))
    return web.json_response(
        {
            "accepted": accepted,
            "runs_in_flight": 0,
            "reason": "accepted" if accepted else "busy",
        }
    )


async def handle_run(request: web.Request) -> web.StreamResponse:
    try:
        body = await request.json()
    except json.JSONDecodeError as exc:
        return web.json_response(
            {"type": "error", "message": f"invalid JSON: {exc}"},
            status=400,
        )

    if not await _try_begin_run():
        return web.json_response(
            {
                "type": "error",
                "message": (
                    f"Cursor sidecar at capacity "
                    f"(CURSOR_RUN_CONCURRENCY={_run_concurrency()})"
                ),
            },
            status=503,
        )

    response = web.StreamResponse(
        status=200,
        reason="OK",
        headers={"Content-Type": "application/x-ndjson"},
    )
    await response.prepare(request)

    active_pool_key: list[str | None] = [None]
    active_turn_slot: list[_ActiveTurn | None] = [None]
    try:
        async for line in _stream_run(
            body if isinstance(body, dict) else {},
            active_pool_key=active_pool_key,
            active_turn_slot=active_turn_slot,
        ):
            try:
                await response.write(line.encode("utf-8"))
            except (ConnectionResetError, BrokenPipeError, OSError) as err:
                print(
                    "[cursor-sdk-runner] client disconnected during /run "
                    f"({err}); requesting turn cancel "
                    f"pool_key={active_pool_key[0]}",
                    file=sys.stderr,
                )
                # Never evict while the turn may still hold pooled.lock —
                # cancel the SDK run and let _stream_run finally clean up.
                if active_turn_slot[0] is not None:
                    await asyncio.to_thread(
                        _request_turn_cancel, active_turn_slot[0]
                    )
                return response
        await response.write_eof()
    except Exception as err:
        # Includes aiohttp ClientConnectionResetError on chunked write.
        print(
            f"[cursor-sdk-runner] /run stream failed ({err}); "
            f"requesting turn cancel pool_key={active_pool_key[0]}",
            file=sys.stderr,
        )
        if active_turn_slot[0] is not None:
            await asyncio.to_thread(_request_turn_cancel, active_turn_slot[0])
        raise
    finally:
        await _end_run()
    return response


async def handle_models(_request: web.Request) -> web.Response:
    api_key = _api_key()
    if not api_key:
        return web.json_response(
            {"ok": False, "error": "CURSOR_API_KEY is not set on the sidecar host"},
            status=503,
        )
    try:
        from cursor_sdk import Cursor
    except ImportError:
        return web.json_response(
            {"ok": False, "error": "cursor-sdk is not installed (pip install cursor-sdk)"},
            status=503,
        )
    try:
        models = Cursor.models.list(api_key=api_key)
        payload = []
        for m in models or []:
            entry = _serialize_model_entry(m)
            if entry:
                payload.append(entry)
        return web.json_response({"ok": True, "models": payload})
    except Exception as err:  # pragma: no cover
        return web.json_response({"ok": False, "error": str(err)}, status=502)


async def _on_startup(app: web.Application) -> None:
    global _REAPER_TASK
    _REAPER_TASK = asyncio.create_task(_bridge_idle_reaper())
    app["bridge_reaper"] = _REAPER_TASK


async def _on_cleanup(_app: web.Application) -> None:
    global _REAPER_TASK
    task = _REAPER_TASK
    _REAPER_TASK = None
    if task is not None:
        task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass
    await _close_all_pooled_bridges()


def _run_self_tests() -> None:
    assert _is_retryable_bridge_error(
        RuntimeError("Timed out waiting for bridge discovery")
    )
    assert _is_retryable_bridge_error(RuntimeError("bridge discovery failed"))
    assert not _is_retryable_bridge_error(RuntimeError("prompt required"))
    assert _is_busy_agent_error(
        RuntimeError(
            "Agent agent-1c9d98ad-57cf-46ae-87ac-f6c5910b081c already has active run"
        )
    )
    assert not _is_busy_agent_error(RuntimeError("prompt required"))
    assert _bridge_launch_timeout_secs() >= 30
    os.environ["CURSOR_BRIDGE_LAUNCH_TIMEOUT_SECS"] = "45"
    assert _bridge_launch_timeout_secs() == 45
    os.environ.pop("CURSOR_BRIDGE_LAUNCH_TIMEOUT_SECS", None)
    assert _is_ephemeral_session_scope(
        "scheduled:17:2026-08-11T07:00:49.089870296+00:00"
    )
    assert _is_ephemeral_session_scope("a1b2c3d4-e5f6-7890-abcd-ef1234567890")
    assert not _is_ephemeral_session_scope("")
    assert not _is_ephemeral_session_scope("focus:project-x")
    assert _bridge_idle_ttl_secs() >= 60
    assert _bridge_pool_max() >= 4
    assert _bridge_orphan_grace_secs() >= 30
    assert _sidecar_max_uptime_secs() >= 300
    assert _is_retryable_bridge_error(
        RuntimeError("Cannot write to closing transport")
    )
    assert _is_retryable_bridge_error(
        RuntimeError("peer closed connection without sending complete message body")
    )


def main() -> None:
    port = int(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_PORT
    app = web.Application()
    app.on_startup.append(_on_startup)
    app.on_cleanup.append(_on_cleanup)
    app.router.add_get("/health", handle_health)
    app.router.add_get("/models", handle_models)
    app.router.add_post("/run", handle_run)
    app.router.add_post("/admin/request_recycle", handle_request_recycle)
    print(f"Cursor SDK runner listening on 0.0.0.0:{port}", file=sys.stderr)
    web.run_app(app, host="0.0.0.0", port=port, print=None)


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--self-test":
        _run_self_tests()
        print("cursor-sdk-runner self-tests ok", file=sys.stderr)
    else:
        main()
