#!/usr/bin/env bash
# Soft-drain then force-recycle the Cursor SDK sidecar.
# Safe to source from reload.sh or run standalone from the repo root.
# During migration, leftover cursor-sdk-bridge.js PIDs from the Python runner are reaped.
set -euo pipefail

log_info() { printf "\x1b[34m[INFO]\x1b[0m %s\n" "$*"; }
log_warn() { printf "\x1b[33m[WARN]\x1b[0m %s\n" "$*"; }

recycle_cursor_sidecar() {
  local repo_root="${1:-.}"
  local port="${CURSOR_SDK_RUNNER_PORT:-3848}"
  local url="${CURSOR_SDK_RUNNER_URL:-http://127.0.0.1:${port}}"
  url="${url%/}"
  local runtime_dir="${FINALLY_A_VALUE_BOT_RUNTIME_DATA:-${repo_root}/workspace/runtime}"
  local pid_file="${runtime_dir}/cursor-sdk-sidecar.pid"
  local soft_wait_secs="${CURSOR_SIDECAR_RELOAD_SOFT_WAIT_SECS:-60}"
  local i=0

  log_info "Recycling Cursor SDK sidecar at ${url} ..."

  if command -v curl >/dev/null 2>&1; then
    local resp
    resp="$(curl -sS -m 3 -X POST "${url}/admin/request_recycle" 2>/dev/null || true)"
    if printf '%s' "$resp" | grep -q '"accepted"[[:space:]]*:[[:space:]]*true'; then
      log_info "Soft recycle accepted; waiting up to ${soft_wait_secs}s for exit"
    elif printf '%s' "$resp" | grep -q '"reason"[[:space:]]*:[[:space:]]*"busy"'; then
      log_warn "Sidecar busy (runs in flight); waiting up to ${soft_wait_secs}s then forcing"
    elif [ -n "$resp" ]; then
      log_warn "Soft recycle response: ${resp}"
    else
      log_warn "Soft recycle unreachable (wedged or down); will force-kill"
    fi

    i=0
    while [ "$i" -lt "$soft_wait_secs" ]; do
      if ! curl -sS -m 2 "${url}/health" >/dev/null 2>&1; then
        log_info "Sidecar no longer responding on ${url}"
        break
      fi
      # If idle after soft request, give it a moment then force.
      local health
      health="$(curl -sS -m 2 "${url}/health" 2>/dev/null || true)"
      if printf '%s' "$health" | grep -q '"runs_in_flight"[[:space:]]*:[[:space:]]*0'; then
        if [ "$i" -ge 5 ]; then
          break
        fi
      fi
      sleep 1
      i=$((i + 1))
    done
  else
    log_warn "curl not found; skipping soft recycle"
  fi

  if [ -f "$pid_file" ]; then
    local pid
    pid="$(tr -d '[:space:]' <"$pid_file" || true)"
    if [ -n "${pid:-}" ] && kill -0 "$pid" 2>/dev/null; then
      log_warn "Force-killing sidecar pid=${pid}"
      kill -TERM "$pid" 2>/dev/null || true
      sleep 0.4
      kill -KILL "$pid" 2>/dev/null || true
    fi
    rm -f "$pid_file"
  fi

  if command -v fuser >/dev/null 2>&1; then
    fuser -k "${port}/tcp" 2>/dev/null || true
  elif command -v lsof >/dev/null 2>&1; then
    lsof -ti:"${port}" 2>/dev/null | xargs -r kill -TERM 2>/dev/null || true
  fi
  sleep 0.3

  # Reap bridge PIDs tied to this install's runtime state.
  if command -v pgrep >/dev/null 2>&1; then
    local killed=0
    while read -r line; do
      case "$line" in
        *"${runtime_dir}"*)
          local bpid
          bpid="$(printf '%s' "$line" | awk '{print $1}')"
          if [ -n "$bpid" ]; then
            kill -TERM "$bpid" 2>/dev/null || true
            killed=$((killed + 1))
          fi
          ;;
      esac
    done < <(pgrep -af 'cursor-sdk-bridge.js' 2>/dev/null || true)
    if [ "$killed" -gt 0 ]; then
      log_info "Terminated ${killed} cursor-sdk-bridge.js process(es)"
    fi
  fi

  log_info "Cursor SDK sidecar recycle complete (gateway bootstrap will respawn if needed)."
}

# Allow `bash scripts/recycle-cursor-sidecar.sh` without sourcing.
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
  recycle_cursor_sidecar "$REPO_ROOT"
fi
