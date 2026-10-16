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

# Allowlisted .env keys the guide may write. Values are never logged.
ENV_KEY_ALLOWLIST='^(ANTHROPIC_API_KEY|OPENAI_API_KEY|GEMINI_API_KEY|XAI_API_KEY|OPENROUTER_API_KEY|DEEPSEEK_API_KEY|CURSOR_API_KEY|TAVILY_API_KEY|BROWSER_MANAGED|STEEL_API_PORT|STEEL_CDP_PORT|STEEL_API_URL|STEEL_DOCKER_IMAGE|STEEL_DOCKER_CONTAINER_NAME|WORKSPACE_DIR|TIMEZONE|VAULT_ORIGIN_VAULT_PATH|VAULT_VECTOR_DB_PATH|VAULT_VECTOR_DB_COLLECTION|VAULT_EMBEDDING_SERVER_URL|VAULT_ORIGIN_VAULT_REPO|VAULT_PRINCIPLES_PATH|LLM_PROVIDER|LLM_MODEL|SYSTEM_LLM_PROVIDER|SYSTEM_LLM_MODEL|TOPICS_MODEL_SOURCE|TOPICS_MODEL|WEB_HOST|WEB_PORT|WEB_AUTH_TOKEN)$'

confirm_yes() {
  local prompt="$1"
  local default_yes="${2:-0}"
  local ans hint
  if [ ! -t 0 ]; then
    return 1
  fi
  if [ "$default_yes" = "1" ]; then
    hint="[Y/n]"
  else
    hint="[y/N]"
  fi
  printf "%s %s " "$prompt" "$hint"
  read -r ans || ans=""
  ans="$(printf '%s' "$ans" | tr '[:upper:]' '[:lower:]')"
  if [ -z "$ans" ]; then
    if [ "$default_yes" = "1" ]; then
      return 0
    fi
    return 1
  fi
  [ "$ans" = "y" ] || [ "$ans" = "yes" ]
}

prompt_secret() {
  local prompt="$1"
  local value=""
  if [ ! -t 0 ]; then
    return 1
  fi
  printf "%s" "$prompt" >&2
  read -rs value || value=""
  printf "\n" >&2
  printf '%s' "$value" | tr -d '\r\n'
}

env_key_set() {
  local key="$1"
  local file="${2:-.env}"
  [ -f "$file" ] || return 1
  grep -Eq "^${key}=[^[:space:]#].*" "$file"
}

env_current_value() {
  local key="$1"
  local line raw
  [ -f .env ] || return 0
  line="$(grep -E "^${key}=" .env | tail -n1 || true)"
  [ -n "$line" ] || return 0
  raw="${line#*=}"
  raw="${raw#\"}"
  raw="${raw%\"}"
  raw="${raw#\'}"
  raw="${raw%\'}"
  printf '%s' "$raw"
}

# Enter keeps the bracketed value (existing .env value, otherwise the default).
prompt_prefilled() {
  local key="$1"
  local fallback="$2"
  local current shown value
  current="$(env_current_value "$key")"
  if [ -n "$current" ]; then
    shown="$current"
  else
    shown="$fallback"
  fi
  if [ -n "$shown" ]; then
    printf "%s [%s]: " "$key" "$shown" >&2
  else
    printf "%s [empty]: " "$key" >&2
  fi
  if [ ! -t 0 ]; then
    printf '%s' "$shown"
    return 0
  fi
  read -r value || value=""
  value="$(printf '%s' "$value" | tr -d '\r')"
  if [ -z "$value" ]; then
    printf '%s' "$shown"
  else
    printf '%s' "$value"
  fi
}

save_prefilled() {
  local key="$1"
  local fallback="$2"
  local description="${3:-}"
  local value
  if [ -n "$description" ]; then
    printf "  %s\n" "$description" >&2
  fi
  value="$(prompt_prefilled "$key" "$fallback")"
  case "$value" in
    *$'\n'*|*$'\r'*|*'"'*)
      log_error "Skipped ${key}: value must be a single line without quotes."
      return 0
      ;;
  esac
  if [ -z "$value" ]; then
    log_info "Left ${key} unset."
    return 0
  fi
  upsert_env_key "$key" "$value" || return 1
  log_success "Saved ${key}=${value}"
}

# Keep a path that lives under WORKSPACE_DIR as workspace-relative.
# An absolute path outside the workspace is returned unchanged.
to_workspace_relative() {
  local ws="$1"
  local value="$2"
  local ws_abs value_abs
  ws_abs="$(normalize_abs_path "$ws")"
  value_abs="$(normalize_abs_path "$value")"
  case "$value_abs" in
    "$ws_abs"/*)
      printf '%s' "${value_abs#"$ws_abs"/}"
      ;;
    /*)
      printf '%s' "$value"
      ;;
    *)
      printf '%s' "${value#./}"
      ;;
  esac
}

# Bracket and description follow the workspace path just entered.
# Enter stores the workspace-relative path.
save_inside_workspace() {
  local key="$1"
  local rel_fallback="$2"
  local description="${3:-}"
  local ws shown value stored
  ws="$(env_current_value WORKSPACE_DIR)"
  [ -n "$ws" ] || ws="./workspace"
  shown="${ws%/}/${rel_fallback#./}"
  if [ -n "$description" ]; then
    printf "  %s Default follows the workspace you entered (%s): %s.\n" \
      "$description" "$ws" "$shown" >&2
  fi
  printf "%s [%s]: " "$key" "$shown" >&2
  if [ ! -t 0 ]; then
    value="$shown"
  else
    read -r value || value=""
    value="$(printf '%s' "$value" | tr -d '\r')"
    if [ -z "$value" ]; then
      value="$shown"
    fi
  fi
  case "$value" in
    *$'\n'*|*$'\r'*|*'"'*)
      log_error "Skipped ${key}: value must be a single line without quotes."
      return 0
      ;;
  esac
  stored="$(to_workspace_relative "$ws" "$value")"
  if [ -z "$stored" ]; then
    log_info "Left ${key} unset."
    return 0
  fi
  upsert_env_key "$key" "$stored" || return 1
  case "$stored" in
    /*) log_success "Saved ${key}=${stored}" ;;
    *) log_success "Saved ${key}=${stored} (inside ${ws})" ;;
  esac
}

is_local_web_host() {
  case "$1" in
    127.0.0.1|localhost|::1|'[::1]') return 0 ;;
    *) return 1 ;;
  esac
}

# Split a host, IP, or http URL into host and optional port. Prints "host port".
split_web_address() {
  local raw="$1" host port rest
  raw="${raw#http://}"
  raw="${raw#https://}"
  raw="${raw%%/*}"
  raw="${raw%%\?*}"
  raw="${raw%%#*}"
  if [ "${raw#\[}" != "$raw" ]; then
    host="${raw%%]*}"
    host="${host#\[}"
    rest="${raw#*]}"
    port="${rest#:}"
  elif [ "${raw#*:}" != "$raw" ] && [ "${raw#*:*:}" = "$raw" ]; then
    host="${raw%%:*}"
    port="${raw##*:}"
  else
    host="$raw"
    port=""
  fi
  printf '%s %s' "$host" "$port"
}

web_name_ok() {
  local host="$1"
  printf '%s' "$host" | grep -Eq \
    '^[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?(\.[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?)*$'
}

dns_prompt_default() {
  case "$1" in
    ''|127.0.0.1|localhost|::1|0.0.0.0|[0-9]*|*:*) printf '%s' 'pp.local' ;;
    *) printf '%s' "$1" ;;
  esac
}

lan_ipv4() {
  ip -4 route get 1.1.1.1 2>/dev/null | awk '{
    for (i = 1; i <= NF; i++) if ($i == "src") { print $(i + 1); exit }
  }'
}

print_dns_setup() {
  local name="$1"
  local ip
  ip="$(lan_ipv4 || true)"
  [ -n "$ip" ] || ip="<this-machine-LAN-IP>"
  log_info "How to make ${name} resolve to this machine"
  log_info "Meido does not register DNS. Point ${name} at ${ip}, then open http://${name}:10961 (or your WEB_PORT)."
  log_info "  mDNS, when the name ends in .local and matches this computer's hostname:"
  log_info "    Linux: sudo apt install avahi-daemon && sudo systemctl enable --now avahi-daemon"
  log_info "           hostname    # the advertised name is <that>.local"
  log_info "           Extra name: avahi-publish -a -R ${name} ${ip}"
  log_info "    macOS: sudo scutil --set LocalHostName <label>    # answers as <label>.local"
  log_info "    Windows: rename the PC to the label before .local (Settings → System → About)"
  log_info "  Any other device, or a name that is not this PC's hostname — one hosts-file line:"
  log_info "    Linux/macOS /etc/hosts"
  log_info "    Windows C:\\Windows\\System32\\drivers\\etc\\hosts"
  log_info "    ${ip}  ${name}"
  log_info "  Router or Pi-hole: an A record from the name to ${ip}. Prefer a suffix such as .lan; many routers will not answer names that end in .local."
}

print_web_ui_url() {
  local host port open
  host="$(env_current_value WEB_HOST)"
  port="$(env_current_value WEB_PORT)"
  [ -n "$host" ] || host="127.0.0.1"
  [ -n "$port" ] || port="10961"
  case "$host" in
    0.0.0.0|::|"[::]") open="127.0.0.1" ;;
    *) open="$host" ;;
  esac
  log_info "Start Meido before opening the web UI: ${BIN_NAME} start"
  if [ "$port" = "10961" ]; then
    log_info "After it starts, open http://${open}:${port}. Default port is 10961. If 10961 is taken, Meido uses the next free port, up to 50 ports higher."
  else
    log_info "After it starts, open http://${open}:${port} (WEB_PORT)."
  fi
  case "$host" in
    0.0.0.0|::|"[::]")
      log_info "It will listen on ${host}:${port}. Other devices use this machine's IP and the same port."
      ;;
  esac
}

step_web_access() {
  local current choice host parsed_host parsed_port default_choice dns_default
  printf "\n"
  log_info "Step — Web UI address"
  log_info "Choose who can open the web UI."
  log_info "  1) This machine only — http://127.0.0.1:10961"
  log_info "  2) Every interface — 0.0.0.0, so any IP on this machine can reach it. Requires an access token."
  log_info "  3) A DNS name or URL (example: pp.local or http://pp.local:10961). Listens on every interface under that name. Requires an access token."
  if [ ! -t 0 ]; then
    return 0
  fi
  current="$(env_current_value WEB_HOST)"
  [ -n "$current" ] || current="127.0.0.1"
  case "$current" in
    0.0.0.0) choice="2" ;;
    127.0.0.1|localhost|::1) choice="1" ;;
    *) choice="3" ;;
  esac
  printf "Choose [%s]: " "$choice"
  default_choice="$choice"
  read -r choice || choice=""
  [ -n "$choice" ] || choice="$default_choice"
  case "$choice" in
    2)
      host="0.0.0.0"
      ;;
    3)
      dns_default="$(dns_prompt_default "$current")"
      printf "DNS name or URL [%s]: " "$dns_default" >&2
      read -r host || host=""
      [ -n "$host" ] || host="$dns_default"
      set -- $(split_web_address "$host")
      parsed_host="$1"
      parsed_port="${2:-}"
      host="$parsed_host"
      if ! web_name_ok "$host"; then
        log_error "Enter a DNS name such as pp.local. Kept 127.0.0.1."
        host="127.0.0.1"
        parsed_port=""
      fi
      if [ -n "$parsed_port" ]; then
        case "$parsed_port" in
          ''|*[!0-9]*) log_warn "Ignored port ${parsed_port}." ;;
          *)
            if [ "$parsed_port" -ge 1 ] && [ "$parsed_port" -le 65535 ]; then
              save_prefilled_value WEB_PORT "$parsed_port"
            else
              log_warn "Ignored port ${parsed_port}."
            fi
            ;;
        esac
      fi
      ;;
    *)
      host="127.0.0.1"
      ;;
  esac
  save_prefilled_value WEB_HOST "$host" || return 1
  if is_local_web_host "$host"; then
    print_web_ui_url
    return 0
  fi
  case "$host" in
    ''|*[!0-9.]*) print_dns_setup "$host" ;;
  esac
  log_info "This name is reachable beyond this machine, so WEB_AUTH_TOKEN is required. The value is hidden and is not printed."
  write_secret_if_chosen "WEB_AUTH_TOKEN" "Paste WEB_AUTH_TOKEN (input hidden)"
  if ! env_key_set "WEB_AUTH_TOKEN"; then
    log_warn "No access token, so the web UI stays on 127.0.0.1."
    save_prefilled_value WEB_HOST "127.0.0.1" || return 1
    host="127.0.0.1"
  fi
  print_web_ui_url
}

save_prefilled_value() {
  local key="$1"
  local value="$2"
  case "$value" in
    *$'\n'*|*$'\r'*|*'"'*)
      log_error "Skipped ${key}: value must be a single line without quotes."
      return 1
      ;;
  esac
  upsert_env_key "$key" "$value" || return 1
  log_success "Saved ${key}=${value}"
}

step_workspace() {
  printf "\n"
  log_info "Step — workspace"
  log_info "Press Enter to keep the value in brackets."
  save_prefilled "WORKSPACE_DIR" "./workspace" \
    "Root folder for chats, skills, runtime data, and tools (workspace/runtime, workspace/skills, workspace/shared)."
  save_prefilled "TIMEZONE" "UTC" \
    "IANA timezone for the clock the agent sees and for scheduled jobs (example: America/Los_Angeles)."
}

step_vault() {
  local ws
  ws="$(env_current_value WORKSPACE_DIR)"
  [ -n "$ws" ] || ws="./workspace"
  printf "\n"
  log_info "Step — ORIGIN vault"
  log_info "Defaults follow the workspace you entered (${ws}). Press Enter to keep the bracketed path."
  log_info "The embedding server URL has no default; Enter leaves it unset."
  save_inside_workspace "VAULT_ORIGIN_VAULT_PATH" "shared/ORIGIN" \
    "Obsidian vault of markdown notes."
  save_inside_workspace "VAULT_VECTOR_DB_PATH" "shared/vault_db" \
    "Local Chroma directory for the vault embeddings."
  save_prefilled "VAULT_VECTOR_DB_COLLECTION" "vault" \
    "Chroma collection name inside that database."
  save_prefilled "VAULT_PRINCIPLES_PATH" "AGENTS.md" \
    "Principles file the agent reads. Default is AGENTS.md at the workspace root."
  save_prefilled "VAULT_EMBEDDING_SERVER_URL" "" \
    "HTTP URL of the embedding server (for example llama.cpp). Leave empty until you have one."
  save_prefilled "VAULT_ORIGIN_VAULT_REPO" "" \
    "Git URL used to clone or pull the vault. Leave empty to use only the local folder."
  ignore_workspace_and_vault
  seed_default_agents_md
}

# Copy defaults/AGENTS.md into the workspace when that principles file is missing.
# A checkout uses the local template. A release install downloads the same file.
# An existing file is left unchanged.
seed_default_agents_md() {
  local ws rel dest src
  ws="$(workspace_dir_from_env)"
  [ -n "$ws" ] || ws="./workspace"
  rel="$(env_current_value VAULT_PRINCIPLES_PATH)"
  [ -n "$rel" ] || rel="AGENTS.md"
  case "$rel" in
    /*|*..*|*:*)
      log_warn "Not writing default principles for ${rel}."
      return 0
      ;;
  esac
  dest="${ws%/}/${rel}"
  if [ -e "$dest" ]; then
    log_info "Principles file already present: ${dest}"
    return 0
  fi
  mkdir -p -- "$(dirname "$dest")" || return 0
  if [ -f defaults/AGENTS.md ]; then
    cp -- defaults/AGENTS.md "$dest"
    log_success "Wrote default principles to ${dest}"
    return 0
  fi
  src="$(mktemp)"
  if curl -fsSL "https://raw.githubusercontent.com/${REPO}/main/defaults/AGENTS.md" -o "$src" \
    && [ -s "$src" ]; then
    cp -- "$src" "$dest"
    rm -f "$src"
    log_success "Wrote default principles to ${dest}"
    return 0
  fi
  rm -f "$src"
  log_info "Default principles will be written on first start if this Meido build includes them."
}

# Collapse "." and ".." without touching the filesystem. Prints an absolute path.
normalize_abs_path() {
  local input="$1"
  local abs part out=""
  local -a parts=()
  case "$input" in
    /*) abs="${input#/}" ;;
    *) abs="$(pwd)/${input}"; abs="${abs#/}" ;;
  esac
  local IFS=/
  for part in $abs; do
    case "$part" in
      ''|.) ;;
      ..)
        if [ "${#parts[@]}" -gt 0 ]; then
          unset 'parts[${#parts[@]}-1]'
        fi
        ;;
      *) parts+=("$part") ;;
    esac
  done
  if [ "${#parts[@]}" -eq 0 ]; then
    printf '/'
    return 0
  fi
  for part in "${parts[@]}"; do
    out="${out}/${part}"
  done
  printf '%s' "$out"
}

gitignore_line_present() {
  local file="$1"
  local pattern="$2"
  local bare="${pattern%/}"
  [ -f "$file" ] || return 1
  grep -Fxq -e "$pattern" -e "$bare" -e "/${bare}" -e "/${pattern}" -- "$file"
}

append_gitignore_pattern() {
  local pattern="$1"
  local file=".gitignore"
  local marker="# Meido workspace and vault (installer)"
  local last=""
  if gitignore_line_present "$file" "$pattern"; then
    return 0
  fi
  if [ ! -f "$file" ]; then
    printf '%s\n%s\n' "$marker" "$pattern" > "$file"
    log_success "Added ${pattern} to .gitignore"
    return 0
  fi
  if [ -s "$file" ]; then
    last="$(tail -c 1 "$file" || true)"
    if [ -n "$last" ]; then
      printf '\n' >> "$file"
    fi
  fi
  if ! grep -Fxq "$marker" "$file"; then
    printf '%s\n' "$marker" >> "$file"
  fi
  printf '%s\n' "$pattern" >> "$file"
  log_success "Added ${pattern} to .gitignore"
}

# Git only ignores paths inside the work tree. Reject glob and comment characters.
add_gitignore_dir() {
  local raw="$1"
  local root target rel pattern
  case "$raw" in
    ''|*[![:print:]]*|*'#'*|*'!'*|*'\\'*|'*'*|'?'*|*'['*)
      log_warn "Skipped .gitignore for an unsafe path."
      return 0
      ;;
  esac
  root="$(normalize_abs_path "$(pwd)")"
  target="$(normalize_abs_path "$raw")"
  case "$target" in
    "$root"/*) ;;
    *)
      log_warn "Not adding ${raw} to .gitignore (outside this repo)."
      return 0
      ;;
  esac
  rel="${target#"$root"/}"
  case "$rel" in
    ''|'.'|'.git'|'.git'/*)
      log_warn "Refusing to gitignore ${raw}."
      return 0
      ;;
  esac
  pattern="${rel}/"
  append_gitignore_pattern "$pattern"
}

ignore_workspace_and_vault() {
  local ws origin db
  if [ ! -e .git ] && [ ! -f .gitignore ]; then
    return 0
  fi
  ws="$(env_current_value WORKSPACE_DIR)"
  [ -n "$ws" ] || ws="./workspace"
  origin="$(env_current_value VAULT_ORIGIN_VAULT_PATH)"
  [ -n "$origin" ] || origin="shared/ORIGIN"
  db="$(env_current_value VAULT_VECTOR_DB_PATH)"
  [ -n "$db" ] || db="shared/vault_db"
  add_gitignore_dir "$ws"
  case "$origin" in
    /*) add_gitignore_dir "$origin" ;;
    *) add_gitignore_dir "${ws%/}/${origin#./}" ;;
  esac
  case "$db" in
    /*) add_gitignore_dir "$db" ;;
    *) add_gitignore_dir "${ws%/}/${db#./}" ;;
  esac
}

workspace_dir_from_env() {
  local raw="./workspace" line
  if [ -f .env ]; then
    line="$(grep -E '^WORKSPACE_DIR=' .env | tail -n1 || true)"
    if [ -n "$line" ]; then
      raw="${line#WORKSPACE_DIR=}"
      raw="${raw#\"}"
      raw="${raw%\"}"
      raw="${raw#\'}"
      raw="${raw%\'}"
    fi
  fi
  if [ -z "$raw" ]; then
    raw="./workspace"
  fi
  printf '%s' "$raw"
}

upsert_env_key() {
  local key="$1"
  local value="$2"
  local file="${3:-.env}"
  local tmp
  if ! printf '%s' "$key" | grep -Eq "$ENV_KEY_ALLOWLIST"; then
    log_error "Refusing to write unexpected env key."
    return 1
  fi
  case "$value" in
    *$'\n'*|*$'\r'*)
      log_error "Refusing to write a multi-line value for ${key}."
      return 1
      ;;
  esac
  if [ -z "$value" ]; then
    log_warn "Skipped ${key} (empty)."
    return 0
  fi
  tmp="$(mktemp)"
  if [ -f "$file" ]; then
    awk -v k="$key" -v v="$value" '
      BEGIN { found = 0 }
      $0 ~ ("^" k "=") { print k "=" v; found = 1; next }
      { print }
      END { if (!found) print k "=" v }
    ' "$file" >"$tmp"
  else
    printf '%s=%s\n' "$key" "$value" >"$tmp"
  fi
  mv -f "$tmp" "$file"
  chmod 600 "$file" || true
}

upsert_env_key_if_empty() {
  local key="$1"
  local value="$2"
  if env_key_set "$key"; then
    return 0
  fi
  upsert_env_key "$key" "$value"
}

write_secret_if_chosen() {
  local key="$1"
  local label="$2"
  local value
  if env_key_set "$key"; then
    if ! confirm_yes "${key} is already set. Replace it?" 0; then
      log_info "Kept existing ${key}."
      return 0
    fi
  fi
  value="$(prompt_secret "${label}: ")"
  if [ -z "$value" ]; then
    log_warn "Skipped ${key}."
    return 0
  fi
  upsert_env_key "$key" "$value" || return 1
  log_success "Saved ${key} in .env (value not shown)."
}

catalog_default_model() {
  case "$1" in
    anthropic) printf '%s' 'claude-opus-4-7' ;;
    openai) printf '%s' 'gpt-5.4' ;;
    google) printf '%s' 'gemini-3.1-pro-preview' ;;
    xai) printf '%s' 'grok-4.3' ;;
    openrouter) printf '%s' 'openrouter/auto' ;;
    deepseek) printf '%s' 'deepseek-chat' ;;
    *) printf '%s' '' ;;
  esac
}

save_llm_setting() {
  local key="$1"
  local value="$2"
  local ws db now
  upsert_env_key "$key" "$value" || return 1
  log_success "Saved ${key}=${value}"
  ws="$(workspace_dir_from_env)"
  db="${ws}/runtime/meido.db"
  if [ ! -f "$db" ] && [ -f "${ws}/runtime/finally_a_value_bot.db" ]; then
    # Pre-rename install; the binary renames this file on its next start.
    db="${ws}/runtime/finally_a_value_bot.db"
  fi
  if [ ! -f "$db" ]; then
    return 0
  fi
  if ! need_cmd python3; then
    log_warn "python3 is missing, so ${key} is in .env only until the next start creates settings."
    return 0
  fi
  now="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  python3 - "$db" "$key" "$value" "$now" <<'PY'
import sqlite3, sys
db, key, value, now = sys.argv[1:]
con = sqlite3.connect(db)
con.execute(
    "INSERT INTO app_settings (key, value, updated_at) VALUES (?, ?, ?) "
    "ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
    (key, value, now),
)
con.commit()
PY
}

fetch_provider_models() {
  local provider="$1"
  local key="$2"
  local url tmp
  case "$provider" in
    anthropic) url="https://api.anthropic.com/v1/models?limit=100" ;;
    google) url="https://generativelanguage.googleapis.com/v1beta/openai/models" ;;
    openai) url="https://api.openai.com/v1/models" ;;
    xai) url="https://api.x.ai/v1/models" ;;
    openrouter) url="https://openrouter.ai/api/v1/models" ;;
    deepseek) url="https://api.deepseek.com/v1/models" ;;
    *) return 1 ;;
  esac
  if ! need_cmd curl || ! need_cmd python3; then
    return 1
  fi
  tmp="$(mktemp)"
  if [ "$provider" = "anthropic" ]; then
    curl -fsS -H "x-api-key: ${key}" -H "anthropic-version: 2023-06-01" "$url" -o "$tmp" || {
      rm -f "$tmp"
      return 1
    }
  else
    curl -fsS -H "Authorization: Bearer ${key}" "$url" -o "$tmp" || {
      rm -f "$tmp"
      return 1
    }
  fi
  python3 - "$tmp" <<'PY'
import json, sys
raw = json.load(open(sys.argv[1], encoding="utf-8"))
rows = raw.get("data") or raw.get("models") or []
seen = set()
for item in rows:
    if not isinstance(item, dict):
        continue
    model_id = str(item.get("id") or item.get("name") or "")
    if model_id.startswith("models/"):
        model_id = model_id[len("models/"):]
    if model_id and model_id not in seen:
        seen.add(model_id)
        print(model_id)
PY
  local status=$?
  rm -f "$tmp"
  return "$status"
}

prompt_model_choice() {
  local purpose="$1"
  local fallback="$2"
  local listfile="$3"
  local -a models=()
  local model count=0 shown=0 choice picked i
  if [ -f "$listfile" ]; then
    while IFS= read -r model; do
      [ -n "$model" ] || continue
      models+=("$model")
      count=$((count + 1))
    done <"$listfile"
  fi
  if [ "$count" -eq 0 ]; then
    printf "  %s\n" "$purpose" >&2
    printf "Model [%s]: " "$fallback" >&2
    read -r choice || choice=""
    if [ -z "$choice" ]; then
      printf '%s' "$fallback"
    else
      printf '%s' "$choice"
    fi
    return 0
  fi
  for i in "${!models[@]}"; do
    if [ "$shown" -ge 40 ]; then
      break
    fi
    printf "  %d) %s\n" "$((i + 1))" "${models[$i]}" >&2
    shown=$((shown + 1))
  done
  if [ "$count" -gt 40 ]; then
    printf "  Showing 40 of %s. Type a model id if yours is not listed.\n" "$count" >&2
  fi
  printf "  %s\n" "$purpose" >&2
  printf "Model [%s]: " "$fallback" >&2
  read -r choice || choice=""
  if [ -z "$choice" ]; then
    printf '%s' "$fallback"
    return 0
  fi
  if printf '%s' "$choice" | grep -Eq '^[0-9]+$'; then
    if [ "$choice" -ge 1 ] && [ "$choice" -le "$shown" ]; then
      printf '%s' "${models[$((choice - 1))]}"
      return 0
    fi
  fi
  printf '%s' "$choice"
}

step_llm_api_key() {
  local choice key provider secret listfile system_llm fallback
  printf "\n"
  log_info "Step — System LLM"
  log_info "This model splits chats into topics, updates the bulletin, and runs other learning."
  log_info "It is saved as SYSTEM_LLM_PROVIDER and SYSTEM_LLM_MODEL. The channel chat model is chosen later in Settings."
  log_info "  1) Anthropic   ANTHROPIC_API_KEY"
  log_info "  2) OpenAI      OPENAI_API_KEY"
  log_info "  3) Gemini      GEMINI_API_KEY"
  log_info "  4) xAI         XAI_API_KEY"
  log_info "  5) OpenRouter  OPENROUTER_API_KEY"
  log_info "  6) DeepSeek    DEEPSEEK_API_KEY"
  log_info "  7) Skip"
  if [ ! -t 0 ]; then
    return 0
  fi
  printf "Choose a provider [1-7]: "
  read -r choice || choice="7"
  case "$choice" in
    1) key="ANTHROPIC_API_KEY"; provider="anthropic" ;;
    2) key="OPENAI_API_KEY"; provider="openai" ;;
    3) key="GEMINI_API_KEY"; provider="google" ;;
    4) key="XAI_API_KEY"; provider="xai" ;;
    5) key="OPENROUTER_API_KEY"; provider="openrouter" ;;
    6) key="DEEPSEEK_API_KEY"; provider="deepseek" ;;
    *) log_info "Skipped system LLM."; return 0 ;;
  esac
  write_secret_if_chosen "$key" "Paste ${key} (input hidden)"
  if ! env_key_set "$key"; then
    log_warn "System LLM was not saved because ${key} is unset."
    return 0
  fi
  secret="$(env_current_value "$key")"
  fallback="$(env_current_value SYSTEM_LLM_MODEL)"
  [ -n "$fallback" ] || fallback="$(env_current_value TOPICS_MODEL)"
  [ -n "$fallback" ] || fallback="$(catalog_default_model "$provider")"
  listfile="$(mktemp)"
  if fetch_provider_models "$provider" "$secret" >"$listfile"; then
    log_info "Loaded models from the ${provider} API."
  else
    log_warn "Could not list models from the ${provider} API. Type a model id, or press Enter for ${fallback}."
    : >"$listfile"
  fi
  system_llm="$(prompt_model_choice "System LLM. Topic split, bulletin, and learning use this model." "$fallback" "$listfile")"
  rm -f "$listfile"
  if [ -z "$system_llm" ]; then
    log_warn "No system LLM saved."
    return 0
  fi
  save_llm_setting "SYSTEM_LLM_PROVIDER" "$provider" || return 1
  save_llm_setting "SYSTEM_LLM_MODEL" "$system_llm" || return 1
}

step_cursor_api_key() {
  printf "\n"
  log_info "Step — Cursor agent engine API key"
  log_info "Cursor Dashboard → Integrations. The sidecar also needs Node.js and npm."
  if ! need_cmd node || ! need_cmd npm; then
    log_warn "node/npm not found. Install Node.js before using Agent engine = Cursor."
  fi
  if ! confirm_yes "Save CURSOR_API_KEY into .env now?" 1; then
    log_info "Skipped CURSOR_API_KEY. Set it before switching the agent engine to Cursor."
    return 0
  fi
  write_secret_if_chosen "CURSOR_API_KEY" "Paste CURSOR_API_KEY (input hidden)"
}

step_mempalace() {
  local ws py venv pip
  printf "\n"
  log_info "Step — MemPalace recall"
  log_info "Installs the mempalace package into WORKSPACE_DIR/runtime/mempalace-venv."
  log_info "Startup does this too when MEMPALACE_LIGHT_MCP_COMMAND is empty."
  if ! need_cmd python3; then
    log_warn "python3 is missing. Install it, then rerun this installer or start the bot."
    return 0
  fi
  if ! confirm_yes "Install MemPalace into the workspace runtime now?" 1; then
    log_info "Skipped MemPalace install."
    return 0
  fi
  ws="$(workspace_dir_from_env)"
  mkdir -p "${ws}/runtime/mempalace"
  venv="${ws}/runtime/mempalace-venv"
  py="${venv}/bin/python"
  if [ ! -x "$py" ]; then
    log_info "Creating venv at ${venv}"
    if ! python3 -m venv "$venv"; then
      log_warn "Could not create the MemPalace venv. The bot will retry on startup."
      return 0
    fi
  fi
  pip="${venv}/bin/pip"
  log_info "Installing mempalace (this can take a few minutes)..."
  if "$pip" install mempalace; then
    log_success "MemPalace is installed in ${venv}."
  else
    log_warn "MemPalace pip install failed. The bot will retry on startup."
  fi
}

step_steel() {
  local ws image script
  printf "\n"
  log_info "Step — Steel browser"
  log_info "Pulls ghcr.io/steel-dev/steel-browser and sets BROWSER_MANAGED=true so the bot starts it."
  if ! need_cmd docker; then
    log_warn "docker is missing. Install Docker, then rerun this installer to pull Steel."
    return 0
  fi
  if ! confirm_yes "Install Steel (docker pull + local skill venv) and enable BROWSER_MANAGED?" 1; then
    log_info "Skipped Steel."
    return 0
  fi
  image="ghcr.io/steel-dev/steel-browser:latest"
  log_info "Pulling ${image}..."
  if ! docker pull "$image"; then
    log_warn "docker pull failed. Check that the Docker daemon is running."
  fi
  upsert_env_key "BROWSER_MANAGED" "true"
  upsert_env_key_if_empty "STEEL_API_PORT" "13920"
  upsert_env_key_if_empty "STEEL_CDP_PORT" "13923"
  upsert_env_key_if_empty "STEEL_API_URL" "http://127.0.0.1:13920"
  upsert_env_key_if_empty "STEEL_DOCKER_IMAGE" "$image"
  upsert_env_key_if_empty "STEEL_DOCKER_CONTAINER_NAME" "meido-steel"
  log_success "Steel bootstrap written to .env. The bot starts the container on boot."
  script="builtin_skills/steel-browser/setup_steel_env.sh"
  if [ -f "$script" ] && need_cmd python3; then
    ws="$(workspace_dir_from_env)"
    log_info "Creating the Steel Python venv (steel-sdk + playwright client)..."
    if WORKSPACE_DIR="$ws" bash "$script"; then
      log_success "Steel skill venv ready."
    else
      log_warn "Steel skill venv setup failed. Rerun: WORKSPACE_DIR=${ws} bash ${script}"
    fi
  else
    log_info "Steel skill venv: from a Meido checkout, run bash builtin_skills/steel-browser/setup_steel_env.sh"
  fi
}

step_more_builtins() {
  printf "\n"
  log_info "Step — other built-ins"
  if need_cmd tmux; then
    log_info "tmux is present (background shell and cursor-agent detach)."
  else
    log_warn "tmux is missing. Install it for spawn_background_command and cursor-agent tmux sessions."
  fi
  if confirm_yes "Save TAVILY_API_KEY for the tavily-full web search skill?" 0; then
    write_secret_if_chosen "TAVILY_API_KEY" "Paste TAVILY_API_KEY (input hidden)"
  else
    log_info "Web search still works via SearXNG (SEARXNG_URL) or the DuckDuckGo fallback."
  fi
  log_info "Messaging channels (Telegram, Discord, WhatsApp, WeCom) are configured in Web UI → Settings → Integrations."
}

print_builtin_guide() {
  local ws
  ws="$(env_current_value WORKSPACE_DIR)"
  [ -n "$ws" ] || ws="./workspace"
  printf "\n"
  log_info "Built-in setup guide (non-interactive). From a checkout, rerun: bash install.sh"
  log_info "  1) Workspace"
  log_info "     WORKSPACE_DIR=${ws} — chats, skills, runtime data, and tools"
  log_info "     TIMEZONE=UTC — IANA zone for the agent clock and scheduled jobs"
  log_info "     Web UI — 1 this machine (127.0.0.1), 2 every interface (0.0.0.0, needs WEB_AUTH_TOKEN), 3 a DNS name such as pp.local"
  log_info "     DNS — Meido does not register the name. Use Avahi/mDNS for <hostname>.local, a hosts-file line, or a router A record to this machine's LAN IP."
  log_info "  2) ORIGIN vault (default folders are inside WORKSPACE_DIR)"
  log_info "     VAULT_ORIGIN_VAULT_PATH=shared/ORIGIN — ${ws%/}/shared/ORIGIN"
  log_info "     VAULT_VECTOR_DB_PATH=shared/vault_db — ${ws%/}/shared/vault_db"
  log_info "     VAULT_VECTOR_DB_COLLECTION=vault — Chroma collection name"
  log_info "     VAULT_PRINCIPLES_PATH=AGENTS.md — principles file at the workspace root"
  log_info "     A missing principles file is filled from defaults/AGENTS.md. An existing file is left as-is."
  log_info "     VAULT_EMBEDDING_SERVER_URL — embedding server URL (no default)"
  log_info "     VAULT_ORIGIN_VAULT_REPO — git URL to clone or pull the vault (no default)"
  log_info "  3) System LLM — pick one API key, then choose the model topic split, bulletin, and learning use. The channel chat model is set later in Settings"
  log_info "  4) Cursor engine — install Node.js, set CURSOR_API_KEY (Cursor Dashboard → Integrations), then pick Cursor in Web UI → Settings → Agent engine"
  log_info "  5) MemPalace — python3 -m venv workspace/runtime/mempalace-venv && workspace/runtime/mempalace-venv/bin/pip install mempalace"
  log_info "     Or start the bot; it installs MemPalace when MEMPALACE_LIGHT_MCP_COMMAND is empty."
  log_info "  6) Steel — install Docker, set BROWSER_MANAGED=true, then: docker pull ghcr.io/steel-dev/steel-browser:latest"
  log_info "     From a checkout: WORKSPACE_DIR=./workspace bash builtin_skills/steel-browser/setup_steel_env.sh"
  log_info "  7) Optional — TAVILY_API_KEY, tmux"
  log_info "  8) ${BIN_NAME} start serves the web UI and returns. Default port is 10961 (http://127.0.0.1:10961). If 10961 is taken, Meido uses the next free port, up to 50 ports higher."
  log_info "     Optional login service (macOS and Linux): ${BIN_NAME} gateway install. It starts Meido at login and restarts it if the process exits. Not required for the web UI."
  ignore_workspace_and_vault
}

guide_builtins() {
  if [ ! -f .env ] && [ -f .env.example ]; then
    cp .env.example .env
    chmod 600 .env || true
    log_success "Copied .env.example to .env."
  elif [ ! -f .env ]; then
    log_warn "No .env.example in this directory. Create .env before configuring built-ins."
    print_builtin_guide
    return 0
  fi

  if [ ! -t 0 ]; then
    print_builtin_guide
    seed_default_agents_md
    return 0
  fi

  printf "\n"
  log_info "Guided setup for built-in features. Secrets are written to .env and are not printed."
  step_workspace
  step_web_access
  step_vault
  step_llm_api_key
  step_cursor_api_key
  step_mempalace
  step_steel
  step_more_builtins
  printf "\n"
  log_success "Built-in setup finished."
  log_info "Start (returns immediately): ${BIN_NAME} start"
  log_info "Stop that process: ${BIN_NAME} stop"
  print_web_ui_url
}

setup_gateway_service() {
  local install_dir="$1"
  local bot_cmd="${install_dir}/${BIN_NAME}"

  if [ ! -x "$bot_cmd" ]; then
    return 0
  fi

  printf "\n"
  log_info "Optional login service (macOS and Linux)."
  log_info "${BIN_NAME} start serves the web UI. It returns immediately and stays up until ${BIN_NAME} stop or a reboot."
  log_info "The gateway is a user service that starts Meido at login and restarts it if the process exits. Skip this if you only want ${BIN_NAME} start."
  printf "Install the login service now? [y/N] "
  read -r install_svc
  install_svc="$(echo "${install_svc:-n}" | tr '[:upper:]' '[:lower:]')"

  if [ "$install_svc" = "y" ] || [ "$install_svc" = "yes" ]; then
    if "$bot_cmd" gateway install; then
      log_success "Login service installed. It starts Meido at login and restarts it if the process exits."
    else
      log_error "Login service install failed."
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
  guide_builtins

  if [ -t 0 ]; then
    setup_gateway_service "$install_dir"
  fi

  printf "\n"
  log_info "Commands: ${BIN_NAME} start (web UI) · ${BIN_NAME} stop · ${BIN_NAME} help · ${BIN_NAME} gateway install (optional login service)"
}

main "$@"
