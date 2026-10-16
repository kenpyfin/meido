#!/usr/bin/env bash
set -euo pipefail

# Sync ORIGIN Obsidian vault from git.
# Set ORIGIN_VAULT_PATH (default: ./workspace/shared/ORIGIN) and optionally ORIGIN_VAULT_REPO.

ORIGIN_VAULT_PATH="${ORIGIN_VAULT_PATH:-./workspace/shared/ORIGIN}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

cd "$REPO_ROOT"

if [ ! -d "$ORIGIN_VAULT_PATH" ]; then
  if [ -z "${ORIGIN_VAULT_REPO:-}" ]; then
    echo "ORIGIN_VAULT_PATH=$ORIGIN_VAULT_PATH does not exist. Set ORIGIN_VAULT_REPO to clone it."
    exit 1
  fi
  mkdir -p "$(dirname "$ORIGIN_VAULT_PATH")"
  git clone "$ORIGIN_VAULT_REPO" "$ORIGIN_VAULT_PATH"
else
  (cd "$ORIGIN_VAULT_PATH" && git pull --rebase)
fi
