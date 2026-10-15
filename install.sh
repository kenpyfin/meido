#!/usr/bin/env bash
set -euo pipefail

REPO="${FINALLY_A_VALUE_BOT_REPO:-finally-a-value-bot/finally-a-value-bot}"
BIN_NAME="finally-a-value-bot"
API_URL="https://api.github.com/repos/${REPO}/releases/latest"

log_info() { printf "\x1b[34m[INFO]\x1b[0m %s\n" "$*"; }
log_success() { printf "\x1b[32m[SUCCESS]\x1b[0m %s\n" "$*"; }
log_warn() { printf "\x1b[33m[WARN]\x1b[0m %s\n" "$*"; }
log_error() { printf "\x1b[31m[ERROR]\x1b[0m %s\n" "$*" >&2; }

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
  #   finally-a-value-bot-0.0.5-aarch64-apple-darwin.tar.gz
  #   finally-a-value-bot-v0.0.5-aarch64-unknown-linux-gnu.tar.gz
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

setup_gateway_service() {
  local install_dir="$1"
  local bot_cmd="${install_dir}/${BIN_NAME}"

  if [ ! -x "$bot_cmd" ]; then
    return 0
  fi

  printf "\n"
  log_info "Optionally, install background service via gateway (cross-platform)."
  printf "Install gateway service now? [y/N] "
  read -r install_svc
  install_svc="$(echo "${install_svc:-n}" | tr '[:upper:]' '[:lower:]')"

  if [ "$install_svc" = "y" ] || [ "$install_svc" = "yes" ]; then
    if "$bot_cmd" gateway install; then
      log_success "Gateway service installed."
    else
      log_error "Gateway install failed."
    fi
  fi
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
      log_info "  Homebrew (macOS): brew tap everettjf/tap && brew install finally-a-value-bot"
      log_info "  Build from source: https://github.com/${REPO}"
      exit 1
    fi

    tmpdir="$(mktemp -d)"
    trap 'if [ -n "${tmpdir:-}" ]; then rm -rf "$tmpdir"; fi' EXIT
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

  log_success "Installed ${BIN_NAME}."
  if [ "$install_dir" = "$HOME/.local/bin" ]; then
    log_info "Make sure '$HOME/.local/bin' is in PATH."
    log_info "Example: export PATH=\"\$HOME/.local/bin:\$PATH\""
  fi

  if [ ! -f .env ] && [ -f .env.example ]; then
    cp .env.example .env
    log_success "Copied .env.example to .env"
  fi

  if [ -t 0 ]; then
    setup_gateway_service "$install_dir"
  fi

  printf "\n"
  log_info "Next steps:"
  log_info "  1) cd to a project directory (copy .env.example to .env)"
  log_info "  2) Set bootstrap vars in .env (workspace + web host/port/auth when needed)"
  log_info "  3) Run: ${BIN_NAME} start"
  log_info "  4) Open Web UI: http://127.0.0.1:10961 and finish channel/LLM/persona settings"
  printf "\n"
  log_info "Run: ${BIN_NAME} help for more options (${BIN_NAME} doctor for preflight checks)."
}

main "$@"
