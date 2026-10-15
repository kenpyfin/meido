---
name: vault-sync
description: Sync the ORIGIN vault with remote Git repository using a union merge strategy.
when_to_use: |
  Use when the user wants to synchronize the Obsidian vault with its remote Git repository.
  Especially useful when edits happen from multiple locations and conflicts need to be resolved by keeping all changes (union merge).
license: MIT
compatibility:
  os:
    - linux
  deps:
    - git
    - bash
---

# Vault Sync

This skill synchronizes the ORIGIN vault with its remote repository. It uses a union merge strategy to ensure that if the same line is edited in two places, both edits are preserved.

## Usage

Run the sync script:

```bash
# In your turn:
run_skill_script(skill_name="vault-sync", script="sync_vault.sh")
```

The script will:
1. Ensure `.gitattributes` is set to `* merge=union`.
2. Commit any local changes in the vault with a timestamp.
3. Pull from the remote `main` branch using `--no-rebase --no-edit`.
4. Push the combined changes back to the remote.

## Configuration

The script targets the `ORIGIN` directory in the shared workspace.
