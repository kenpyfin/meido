#!/usr/bin/env bash
set -euo pipefail

BIN_NAME="finally-a-value-bot"

log_info() { printf "\x1b[34m[INFO]\x1b[0m %s\n" "$*"; }
log_success() { printf "\x1b[32m[SUCCESS]\x1b[0m %s\n" "$*"; }
log_warn() { printf "\x1b[33m[WARN]\x1b[0m %s\n" "$*"; }
log_error() { printf "\x1b[31m[ERROR]\x1b[0m %s\n" "$*" >&2; }

need_cmd() {
  command -v "$1" >/dev/null 2>&1
}

detect_os() {
  case "$(uname -s)" in
    Darwin) echo "darwin" ;;
    Linux) echo "linux" ;;
    *)
      err "Unsupported OS: $(uname -s)"
      exit 1
      ;;
  esac
}

resolve_targets() {
  local out=()

  if [ -n "${FINALLY_A_VALUE_BOT_INSTALL_DIR:-}" ]; then
    out+=("${FINALLY_A_VALUE_BOT_INSTALL_DIR%/}/$BIN_NAME")
  fi

  if need_cmd "$BIN_NAME"; then
    out+=("$(command -v "$BIN_NAME")")
  fi

  out+=("/usr/local/bin/$BIN_NAME")
  out+=("$HOME/.local/bin/$BIN_NAME")

  # De-duplicate while preserving order.
  local seen="|"
  local path
  for path in "${out[@]}"; do
    if [ -n "$path" ] && [[ "$seen" != *"|$path|"* ]]; then
      printf '%s\n' "$path"
      seen+="$path|"
    fi
  done
}

remove_file() {
  local target="$1"
  if [ ! -e "$target" ]; then
    return 1
  fi

  if [ -w "$target" ] || [ -w "$(dirname "$target")" ]; then
    rm -f "$target"
  else
    if need_cmd sudo; then
      sudo rm -f "$target"
    else
      err "No permission to remove $target and sudo is unavailable"
      return 2
    fi
  fi

  return 0
}

remove_gateway_service() {
  local candidates=(
    "${FINALLY_A_VALUE_BOT_INSTALL_DIR:-}/$BIN_NAME"
    "$(command -v "$BIN_NAME" 2>/dev/null || true)"
    "/usr/local/bin/$BIN_NAME"
    "$HOME/.local/bin/$BIN_NAME"
  )
  local cmd=""
  local c
  for c in "${candidates[@]}"; do
    if [ -n "$c" ] && [ -x "$c" ]; then
      cmd="$c"
      break
    fi
  done

  if [ -z "$cmd" ]; then
    return 0
  fi

  log_info "Uninstalling gateway service via CLI..."
  if "$cmd" gateway uninstall >/dev/null 2>&1; then
    log_success "Gateway service uninstalled."
  else
    log_warn "Gateway uninstall skipped or failed (service may not be installed)."
  fi
}

main() {
  local os removed=0 failed=0 target

  os="$(detect_os)"
  log_info "Uninstalling $BIN_NAME on $os..."

  while IFS= read -r target; do
    if [ -z "$target" ]; then
      continue
    fi
    if remove_file "$target"; then
      log_info "Removed: $target"
      removed=$((removed + 1))
    else
      rc=$?
      if [ "$rc" -eq 2 ]; then
        failed=1
      fi
    fi
  done < <(resolve_targets)

  remove_gateway_service

  if [ "$failed" -ne 0 ]; then
    exit 1
  fi

  if [ "$removed" -eq 0 ]; then
    log_info "$BIN_NAME binary not found. Nothing to uninstall."
    exit 0
  fi

  printf "\n"
  log_success "$BIN_NAME has been removed."
  log_info "Optional cleanup (not removed automatically):"
  log_info "  rm -rf ./finally-a-value-bot.data/runtime"
  log_info "  rm -f ./finally-a-value-bot.config.yaml ./finally-a-value-bot.config.yml"
  log_info "  rm -rf ./workspace"
}

main "$@"
