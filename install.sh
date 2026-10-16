#!/usr/bin/env bash
set -euo pipefail

# Bash reads this file by byte offset. Editing it during a long install shifts
# those offsets and the run dies with a syntax error after real work has
# already succeeded. Re-exec a private copy so later edits cannot change the
# bytes this process is reading. A piped `curl | bash` has no script file.
_meido_src="${BASH_SOURCE[0]:-}"
if [ -z "${MEIDO_INSTALL_SNAPSHOT:-}" ] && [ -n "$_meido_src" ] && [ -f "$_meido_src" ]; then
  _meido_snap="$(mktemp "${TMPDIR:-/tmp}/meido-install.XXXXXX")"
  cp -- "$_meido_src" "$_meido_snap"
  export MEIDO_INSTALL_SNAPSHOT="$_meido_snap"
  exec bash "$_meido_snap" "$@"
fi
unset _meido_src _meido_snap
if [ -n "${MEIDO_INSTALL_SNAPSHOT:-}" ]; then
  trap 'rm -f "$MEIDO_INSTALL_SNAPSHOT"' EXIT
fi

REPO="${MEIDO_REPO:-${FINALLY_A_VALUE_BOT_REPO:-kenpyfin/meido}}"
BIN_NAME="meido"
API_URL="https://api.github.com/repos/${REPO}/releases/latest"

log_info() { printf "\x1b[34m[INFO]\x1b[0m %s\n" "$*"; }
log_success() { printf "\x1b[32m[SUCCESS]\x1b[0m %s\n" "$*"; }
log_warn() { printf "\x1b[33m[WARN]\x1b[0m %s\n" "$*"; }
log_error() { printf "\x1b[31m[ERROR]\x1b[0m %s\n" "$*" >&2; }
err() { log_error "$@"; }

need_cmd() {
  command -v "$1" >/dev/null 2>&1
}

check_deps() {
  local deps=("curl" "git" "tar")
  local missing=()
  for dep in "${deps[@]}"; do
    if ! need_cmd "$dep"; then
      missing+=("$dep")
    fi
  done

  if [ "${#missing[@]}" -ne 0 ]; then
    log_error "Missing required dependencies: ${missing[*]}"
    log_info "Please install them and try again."
    exit 1
  fi
}

# Soft checks for built-in optional features (web-first install still works without them).
report_optional_deps() {
  log_info "Optional runtime dependencies for built-in features:"
  if need_cmd python3; then
    log_info "  python3 — present (MemPalace recall venv + Cursor Python rollback)"
  else
    log_warn "  python3 — missing (MemPalace MCP auto-install and Cursor Python rollback need it)"
  fi
  if need_cmd node && need_cmd npm; then
    log_info "  node/npm — present (Cursor SDK engine sidecar)"
  else
    log_warn "  node/npm — missing (Cursor agent engine needs Node.js; Classic engine still works)"
  fi
  if need_cmd tmux; then
    log_info "  tmux — present (background shell jobs + cursor-agent detach)"
  else
    log_warn "  tmux — missing (spawn_background_command / cursor-agent tmux sessions unavailable)"
  fi
  if need_cmd docker; then
    log_info "  docker — present (Steel browser when BROWSER_MANAGED=true)"
  else
    log_warn "  docker — missing (Steel browser skill needs Docker when managed locally)"
  fi
}

build_web_assets_if_available() {
  # Build web/dist so embedded assets match web/src before Rust compile.
  if [ ! -d "web" ] || [ ! -f "web/package.json" ]; then
    return 0
  fi

  if ! need_cmd npm; then
    log_warn "npm not found; skipping web asset build. Embedded Web UI may be stale."
    return 0
  fi

  log_info "Building Web UI assets..."
  (
    cd web
    npm install
    npm run build
  )
  log_success "Web UI assets built."
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

detect_arch() {
  case "$(uname -m)" in
    x86_64|amd64) echo "x86_64" ;;
    arm64|aarch64) echo "aarch64" ;;
    *)
      err "Unsupported architecture: $(uname -m)"
      exit 1
      ;;
  esac
}

detect_install_dir() {
  if [ -n "${MEIDO_INSTALL_DIR:-}" ]; then
    echo "$MEIDO_INSTALL_DIR"
    return
  fi
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

download_release_json() {
  if need_cmd curl; then
    curl -fsSL "$API_URL"
  elif need_cmd wget; then
    wget -qO- "$API_URL"
  else
    err "Neither curl nor wget is available"
    exit 1
  fi
}

extract_asset_url() {
  # Match assets like:
  #   meido-0.0.5-aarch64-apple-darwin.tar.gz
  #   meido-v0.0.5-aarch64-unknown-linux-gnu.tar.gz
  # and keep fallback matching looser suffixes.
  local release_json="$1"
  local os="$2"
  local arch="$3"
  local os_regex arch_regex

  case "$os" in
    darwin) os_regex="apple-darwin|darwin" ;;
    linux) os_regex="unknown-linux-gnu|unknown-linux-musl|linux" ;;
    *)
      err "Unsupported OS for release matching: $os"
      return 1
      ;;
  esac

  case "$arch" in
    x86_64) arch_regex="x86_64|amd64" ;;
    aarch64) arch_regex="aarch64|arm64" ;;
    *)
      err "Unsupported architecture for release matching: $arch"
      return 1
      ;;
  esac

  printf '%s\n' "$release_json" \
    | grep -Eo 'https://[^"]+' \
    | grep '/releases/download/' \
    | grep -E "/${BIN_NAME}-v?[0-9]+\.[0-9]+\.[0-9]+-.*(apple-darwin|(unknown-)?linux-gnu|(unknown-)?linux-musl|(pc-)?windows-msvc)\.(tar\.gz|zip)$" \
    | grep -Ei "(${arch_regex}).*(${os_regex})|(${os_regex}).*(${arch_regex})" \
    | head -n1
}

download_file() {
  local url="$1"
  local output="$2"
  if need_cmd curl; then
    curl -fL "$url" -o "$output"
  else
    wget -O "$output" "$url"
  fi
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

install_from_archive() {
  local archive="$1"
  local install_dir="$2"
  local tmpdir="$3"
  local extracted=0

  case "$archive" in
    *.tar.gz|*.tgz)
      tar -xzf "$archive" -C "$tmpdir"
      extracted=1
      ;;
    *.zip)
      if ! need_cmd unzip; then
        err "unzip is required to extract zip archives"
        return 1
      fi
      unzip -q "$archive" -d "$tmpdir"
      extracted=1
      ;;
  esac

  if [ "$extracted" -eq 0 ]; then
    # Fallback: detect by content if extension is missing/changed.
    if tar -tzf "$archive" >/dev/null 2>&1; then
      tar -xzf "$archive" -C "$tmpdir"
      extracted=1
    elif need_cmd unzip && unzip -tq "$archive" >/dev/null 2>&1; then
      unzip -q "$archive" -d "$tmpdir"
      extracted=1
    fi
  fi

  if [ "$extracted" -eq 0 ]; then
    err "Unknown archive format: $archive"
    return 1
  fi

  local bin_path
  bin_path="$(find "$tmpdir" -type f -name "$BIN_NAME" | head -n1)"
  if [ -z "$bin_path" ]; then
    err "Could not find '$BIN_NAME' in archive"
    return 1
  fi

  chmod +x "$bin_path"
  install_binary_atomic "$bin_path" "$install_dir"
}


seed_env_file() {
  if [ -f .env ]; then
    return 0
  fi
  if [ -f .env.example ]; then
    cp .env.example .env
    chmod 600 .env || true
    log_success "Copied .env.example to .env."
    return 0
  fi
  cat > .env <<'EOF'
WORKSPACE_DIR=./workspace
TIMEZONE=UTC
WEB_ENABLED=true
WEB_HOST=127.0.0.1
WEB_PORT=10961
EOF
  chmod 600 .env || true
  log_success "Created .env with local web UI defaults."
}

main() {
  local os arch install_dir release_json asset_url tmpdir archive asset_filename

  check_deps
  os="$(detect_os)"
  arch="$(detect_arch)"
  install_dir="$(detect_install_dir)"

  if [ -f "Cargo.toml" ]; then
    build_web_assets_if_available
    log_info "Building ${BIN_NAME} from source (Cargo.toml found)..."
    if ! need_cmd cargo; then
      log_warn "cargo is required to build from source but was not found."
      printf "Would you like to install Rust (via rustup)? [Y/n] "
      read -r install_rust
      install_rust="$(echo "${install_rust:-y}" | tr '[:upper:]' '[:lower:]')"
      if [ "$install_rust" != "n" ] && [ "$install_rust" != "no" ]; then
        log_info "Installing Rust..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        if [ -f "$HOME/.cargo/env" ]; then
          source "$HOME/.cargo/env"
        fi
      else
        log_error "Cannot build without cargo. Exiting."
        exit 1
      fi
    fi
    cargo build --release
    local bin_path="target/release/${BIN_NAME}"
    if [ ! -x "$bin_path" ]; then
      log_error "Failed to build ${BIN_NAME} executable."
      exit 1
    fi
    install_binary_atomic "$bin_path" "$install_dir"
  else
    log_info "Installing ${BIN_NAME} for ${os}/${arch} from GitHub releases..."
    release_json="$(download_release_json)"
    asset_url="$(extract_asset_url "$release_json" "$os" "$arch" || true)"
    if [ -z "$asset_url" ]; then
      log_error "No prebuilt binary found for ${os}/${arch} in the latest GitHub release."
      log_info "Use a separate install method instead:"
      log_info "  Build from source: https://github.com/${REPO}"
      exit 1
    fi

    tmpdir="$(mktemp -d)"
    trap 'if [ -n "${tmpdir:-}" ]; then rm -rf "$tmpdir"; fi; if [ -n "${MEIDO_INSTALL_SNAPSHOT:-}" ]; then rm -f "$MEIDO_INSTALL_SNAPSHOT"; fi' EXIT
    asset_filename="${asset_url##*/}"
    asset_filename="${asset_filename%%\?*}"
    if [ -z "$asset_filename" ] || [ "$asset_filename" = "$asset_url" ]; then
      asset_filename="${BIN_NAME}.archive"
    fi
    archive="$tmpdir/$asset_filename"
    log_info "Downloading: $asset_url"
    download_file "$asset_url" "$archive"
    install_from_archive "$archive" "$install_dir" "$tmpdir"
  fi

  log_success "Installed Meido (${BIN_NAME})."
  if [ "$install_dir" = "$HOME/.local/bin" ]; then
    log_info "Make sure '$HOME/.local/bin' is in PATH."
    log_info "Example: export PATH=\"\$HOME/.local/bin:\$PATH\""
  fi

  report_optional_deps
  seed_env_file

  printf "\n"
  log_info "Start Meido, then finish setup in the web UI."
  log_info "  ${BIN_NAME} start"
  log_info "  open http://127.0.0.1:10961"
  log_info "The setup wizard opens when workspace, vault, or the system LLM is not set in .env."
  log_info "Edit .env directly any time. Settings shows the same values and writes them back."
  log_info "Commands: ${BIN_NAME} start · ${BIN_NAME} stop · ${BIN_NAME} help · ${BIN_NAME} gateway install (optional login service)"
}

main "$@"
