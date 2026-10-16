# Shipped hook catalog

This directory is the **source of truth** for hooks that ship with the bot. On every database init/migrate, the binary reads all `*.hook.json` files here and upserts rows into SQLite `hook_definitions`.

## Fresh install

A new clone already contains these manifests (no manual setup):

| Manifest | Event | Runtime handler |
|----------|-------|-----------------|
| `postdelivery-persona-focus-sync.hook.json` | PostDelivery | `builtin_persona_focus_sync` |
| `beforeturn-scheduler-policy-context.hook.json` | BeforeTurn | `builtin_scheduler_policy_context` |
| `pretool-turn-skill-gate.hook.json` | PreToolUse | `builtin_turn_skill_gate` |
| `prestop-deferred-commitment-guard.hook.json` | PreStop | `builtin_deferred_commitment_guard` |
| `postbatch-loop-guard.hook.json` | PostToolBatch | `builtin_loop_guard` |
| `predelivery-dense-delivery-guard.hook.json` | PreDelivery | `builtin_dense_delivery_guard` |
| `postdelivery-secret-vault-scrub.hook.json` | PostDelivery | `builtin_secret_vault_scrub` |

Execution is implemented in Rust (`src/hook_runtime.rs`). Manifests declare **which** hooks exist and their lifecycle binding; they are not subprocess scripts.

## Not shipped here

- **PZ terminal cleanup** and other persona-specific command hooks — add under `{WORKSPACE_DIR}/hooks/` and register via the web API or `POST /api/hooks`.
- **Template / example hooks** — removed from the catalog; use `builtin_skills/create-hook` for custom hooks.

## Manifest format

```json
{
  "name": "unique-hook-name",
  "event_name": "PostDelivery",
  "matcher": null,
  "action_type": "builtin_persona_focus_sync",
  "action_payload": {},
  "enabled": true
}
```

Shipped manifests must use `action_type` values starting with `builtin_`.

## Resolution

The bot locates this directory via `FINALLY_A_VALUE_BOT_BUILTIN_HOOKS`, sibling of `WORKSPACE_DIR`, cwd, executable parent, or `CARGO_MANIFEST_DIR/builtin_hooks` (see `src/builtin_hooks.rs`).
