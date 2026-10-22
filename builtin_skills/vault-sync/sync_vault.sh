#!/bin/bash
# Sync script for ORIGIN vault with union merge strategy to keep both edits on conflict.

# Vault location, first match wins:
#   1. ORIGIN_VAULT_PATH
#   2. <WORKSPACE_DIR>/shared/ORIGIN
#   3. relative to this script: <workspace>/skills/vault-sync -> <workspace>/shared/ORIGIN,
#      or <repo>/builtin_skills/vault-sync -> <repo>/workspace/shared/ORIGIN
resolve_vault_dir() {
    if [ -n "${ORIGIN_VAULT_PATH:-}" ]; then
        echo "$ORIGIN_VAULT_PATH"
        return
    fi
    local ws="${WORKSPACE_DIR:-}"
    if [ -n "$ws" ]; then
        echo "${ws%/}/shared/ORIGIN"
        return
    fi
    local here
    here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    local candidate
    for candidate in "$here/../../shared/ORIGIN" "$here/../../workspace/shared/ORIGIN"; do
        if [ -d "$candidate" ]; then
            (cd "$candidate" && pwd)
            return
        fi
    done
    echo "$here/../../shared/ORIGIN"
}

VAULT_DIR="$(resolve_vault_dir)"

if [ ! -d "$VAULT_DIR" ]; then
    echo "Error: Could not access vault directory $VAULT_DIR (set ORIGIN_VAULT_PATH or WORKSPACE_DIR)"
    exit 1
fi

cd "$VAULT_DIR" || exit 1

# Ensure .gitattributes exists for union merge
if [ ! -f .gitattributes ] || ! grep -q "merge=union" .gitattributes; then
    echo "* merge=union" > .gitattributes
    git add .gitattributes
    git commit -m "auto: add/update .gitattributes for union merge strategy" || true
fi

# 1. Commit any local changes first
git add -A
if git commit -m "auto: vault sync $(date '+%Y-%m-%d %H:%M:%S')"; then
    echo "Committed local changes."
else
    echo "No local changes to commit."
fi

# 2. Pull from remote (this will use union merge if there are conflicts)
echo "Pulling from origin..."
# We assume 'main' branch and 'origin' remote. 
# You can customize these if necessary.
if git pull --no-rebase --no-edit origin main; then
    echo "Successfully pulled and merged."
else
    echo "Merge failed or conflict occurred. Since merge=union is set, Git should have handled simple conflicts. Checking status..."
    git status
fi

# 3. Push back to remote
echo "Pushing to origin..."
if git push origin main; then
    echo "Successfully pushed to origin."
else
    echo "Error: Failed to push to origin."
    exit 1
fi
