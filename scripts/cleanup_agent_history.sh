#!/usr/bin/env bash
set -euo pipefail

log_info() { printf "\x1b[34m[INFO]\x1b[0m %s\n" "$*"; }
log_success() { printf "\x1b[32m[SUCCESS]\x1b[0m %s\n" "$*"; }
log_warn() { printf "\x1b[33m[WARN]\x1b[0m %s\n" "$*"; }
log_error() { printf "\x1b[31m[ERROR]\x1b[0m %s\n" "$*" >&2; }

usage() {
  cat <<'EOF'
Usage:
  ./scripts/cleanup_agent_history.sh [options]

Options:
  --runtime-dir <dir>   Runtime directory that contains groups/ (default: workspace/runtime)
  --keep <n>            Keep newest n .md history files per persona (default: 0)
  --purge               Remove each persona's entire agent_history directory
  --dry-run             Show what would be deleted without deleting
  -h, --help            Show this help

Examples:
  ./scripts/cleanup_agent_history.sh
  ./scripts/cleanup_agent_history.sh --keep 10
  ./scripts/cleanup_agent_history.sh --purge --dry-run
EOF
}

runtime_dir="workspace/runtime"
keep_count=0
purge_mode=0
dry_run=0

while [ "$#" -gt 0 ]; do
  case "$1" in
    --runtime-dir)
      runtime_dir="${2:-}"
      shift 2
      ;;
    --keep)
      keep_count="${2:-}"
      shift 2
      ;;
    --purge)
      purge_mode=1
      shift
      ;;
    --dry-run)
      dry_run=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      log_error "Unknown argument: $1"
      usage
      exit 1
      ;;
  esac
done

if ! [[ "$keep_count" =~ ^[0-9]+$ ]]; then
  log_error "--keep must be a non-negative integer"
  exit 1
fi

groups_dir="${runtime_dir%/}/groups"
if [ ! -d "$groups_dir" ]; then
  log_error "Groups directory not found: $groups_dir"
  exit 1
fi

log_info "Scanning persona agent_history directories in: $groups_dir"
[ "$dry_run" -eq 1 ] && log_warn "Dry run enabled: no files will be deleted."

shopt -s nullglob

persona_dirs_scanned=0
history_dirs_found=0
files_deleted=0
dirs_deleted=0

for chat_dir in "$groups_dir"/*; do
  [ -d "$chat_dir" ] || continue

  for persona_dir in "$chat_dir"/*; do
    [ -d "$persona_dir" ] || continue
    persona_dirs_scanned=$((persona_dirs_scanned + 1))

    history_dir="$persona_dir/agent_history"
    [ -d "$history_dir" ] || continue
    history_dirs_found=$((history_dirs_found + 1))

    if [ "$purge_mode" -eq 1 ]; then
      if [ "$dry_run" -eq 1 ]; then
        log_info "[dry-run] purge directory: $history_dir"
      else
        rm -rf "$history_dir"
        log_info "Purged: $history_dir"
      fi
      dirs_deleted=$((dirs_deleted + 1))
      continue
    fi

    files=("$history_dir"/*.md)
    file_count="${#files[@]}"
    if [ "$file_count" -eq 0 ]; then
      continue
    fi

    delete_count=0
    if [ "$keep_count" -lt "$file_count" ]; then
      delete_count=$((file_count - keep_count))
    fi
    [ "$delete_count" -gt 0 ] || continue

    mapfile -t sorted_files < <(printf '%s\n' "${files[@]}" | sort)
    for ((i = 0; i < delete_count; i++)); do
      target="${sorted_files[$i]}"
      if [ "$dry_run" -eq 1 ]; then
        log_info "[dry-run] delete file: $target"
      else
        rm -f "$target"
      fi
      files_deleted=$((files_deleted + 1))
    done
  done
done

shopt -u nullglob

log_success "Cleanup complete."
log_info "Persona directories scanned: $persona_dirs_scanned"
log_info "agent_history directories found: $history_dirs_found"

if [ "$purge_mode" -eq 1 ]; then
  log_info "agent_history directories removed: $dirs_deleted"
else
  log_info "History files removed: $files_deleted"
  log_info "Keep newest per persona: $keep_count"
fi
