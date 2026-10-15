#!/usr/bin/env bash
# Quick reload: build from source, copy binary, restart gateway.
# Run from project root (where Cargo.toml is). Skips full install flow.
set -euo pipefail

REPO_ROOT="${FINALLY_A_VALUE_BOT_REPO_ROOT:-.}"
BIN_NAME="finally-a-value-bot"

log_info() { printf "\x1b[34m[INFO]\x1b[0m %s\n" "$*"; }
log_success() { printf "\x1b[32m[SUCCESS]\x1b[0m %s\n" "$*"; }
log_warn() { printf "\x1b[33m[WARN]\x1b[0m %s\n" "$*"; }
log_error() { printf "\x1b[31m[ERROR]\x1b[0m %s\n" "$*" >&2; }

need_cmd() {
  command -v "$1" >/dev/null 2>&1
}

detect_install_dir() {
  if [ -n "${FINALLY_A_VALUE_BOT_INSTALL_DIR:-}" ]; then
    echo "$FINALLY_A_VALUE_BOT_INSTALL_DIR"
    return
  fi
  if [ -w "/usr/local/bin" ]; then
    echo "/usr/local/bin"
    return
  fi
  if [ -d "$HOME/.local/bin" ] || mkdir -p "$HOME/.local/bin" 2>/dev/null; then
    echo "$HOME/.local/bin"
    return
  fi
  echo "/usr/local/bin"
}

build_web_assets_if_available() {
  if [ ! -d "$REPO_ROOT/web" ] || [ ! -f "$REPO_ROOT/web/package.json" ]; then
    return 0
  fi
  if ! need_cmd npm; then
    log_warn "npm not found; skipping web asset build."
    return 0
  fi
  log_info "Building Web UI assets..."
  (cd "$REPO_ROOT/web" && npm install && npm run build)
  log_success "Web UI assets built."
}

install_binary_atomic() {
  local source_bin="$1"
  local install_dir="$2"
  local target_path="${install_dir}/${BIN_NAME}"
  local tmp_target="${install_dir}/.${BIN_NAME}.new.$$"

  if [ -w "$install_dir" ]; then
    cp "$source_bin" "$tmp_target"
    chmod +x "$tmp_target"
    mv -f "$tmp_target" "$target_path"
  else
    if need_cmd sudo; then
      sudo cp "$source_bin" "$tmp_target"
      sudo chmod +x "$tmp_target"
      sudo mv -f "$tmp_target" "$target_path"
    else
      log_error "No write permission for $install_dir and sudo not available"
      return 1
    fi
  fi
}

main() {
  cd "$REPO_ROOT" || { log_error "Cannot cd to $REPO_ROOT"; exit 1; }

  if [ ! -f "Cargo.toml" ]; then
    log_error "Cargo.toml not found. Run from project root."
    exit 1
  fi

  if ! need_cmd cargo; then
    log_error "cargo not found. Install Rust first."
    exit 1
  fi

  build_web_assets_if_available
  log_info "Building from source..."
  cargo build --release

  local bin_path="target/release/${BIN_NAME}"
  if [ ! -x "$bin_path" ]; then
    log_error "Build failed: $bin_path not found or not executable"
    exit 1
  fi

  local install_dir
  install_dir="$(detect_install_dir)"
  install_binary_atomic "$bin_path" "$install_dir"
  log_success "Binary installed to ${install_dir}/${BIN_NAME}"

  # Soft-drain then force-recycle sidecar so deploys never keep a stale runner.
  if [ -f "$REPO_ROOT/scripts/recycle-cursor-sidecar.sh" ]; then
    # shellcheck disable=SC1091
    . "$REPO_ROOT/scripts/recycle-cursor-sidecar.sh"
    recycle_cursor_sidecar "$REPO_ROOT"
  else
    log_warn "scripts/recycle-cursor-sidecar.sh missing; skipping sidecar recycle"
  fi

  local bot_cmd="${install_dir}/${BIN_NAME}"
  if "$bot_cmd" gateway status >/dev/null 2>&1; then
    log_info "Restarting gateway service..."
    "$bot_cmd" gateway stop 2>/dev/null || true
    "$bot_cmd" gateway start
    log_success "Gateway restarted with new binary."
  else
    log_info "Gateway not running. If you run the bot manually, restart with: $BIN_NAME start"
  fi
}

main "$@"
