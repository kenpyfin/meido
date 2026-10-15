---
name: modify-skill
description: Safely update an existing FinallyAValueBot skill (SKILL.md, helpers, .env layout).
when_to_use: |
  Use when the user asks to update, modify, change, refine, fix, or extend an **existing** skill
  (including SKILL.md frontmatter, body procedures, helper scripts, or skill-local .env).
  Do not use for brand-new skills — activate `create-skill` instead.
license: MIT
compatibility:
  os:
    - darwin
    - linux
    - windows
---

# Modify skill

Use this skill before changing any **existing** workspace skill. The runtime requires `activate_skill` with `modify-skill` in the same turn before `build_skill` (when the skill folder already exists) or file edits under `skills/<name>/`.

## Preflight (required)

1. **Identify the skill** — Confirm the directory name under the workspace skills root (must match frontmatter `name`).
2. **Read current state** — `read_file` on `skills/<name>/SKILL.md` and any helpers you will touch. Note frontmatter fields (`description`, `when_to_use`, `compatibility`) and body procedures.
3. **Plan the delta** — List what changes (routing vs body vs scripts). Keep `when_to_use` in frontmatter accurate; do not bury routing-only edits in the body.
4. **Apply changes** — Prefer **`build_skill`** for substantive rewrites; use `apply_search_replace` / `edit_file` only for small, targeted edits.

## Path discipline (strict)

- Tool cwd is `shared/`. Skill paths are **`skills/<name>/...`** (resolves to the canonical skills data directory).
- Never use `workspace/skills/...` — that creates an undiscoverable shadow tree under `shared/workspace/`.
- Prefer **`build_skill(name=..., description=..., instructions=...)`** over manual writes when rewriting most of the skill.
- Put or update credentials only in `skills/<name>/.env`; never hardcode secrets in SKILL.md.

## Editing rules

| Change type | Approach |
|-------------|----------|
| Frontmatter (`description`, `when_to_use`, compatibility) | Update YAML between `---` fences; keep valid YAML |
| Body procedures / examples | `build_skill` or `apply_search_replace` on `skills/<name>/SKILL.md` |
| Helper scripts | `write_file` / `apply_search_replace` under `skills/<name>/` |
| New optional file (e.g. script) | `write_file` under `skills/<name>/`; reference with `skills/<name>/script.py` in the body |

## After editing

1. Verify the skill still has valid frontmatter (`name`, `description`, `when_to_use`).
2. Confirm paths in examples use `skills/<name>/...`, not `workspace/skills/...`.
3. Tell the user what changed and whether they need to update `skills/<name>/.env`.

## When not to use

- **New skill** — activate `create-skill` and use its checklist.
- **Built-in skills** under repository `builtin_skills/` — those ship with the checkout; changing them is a code change, not a workspace skill edit.
- **Sync from external catalog** — use `sync_skills` when the user asked to import from an external source.
