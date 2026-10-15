---
name: create-hook
description: Author and register lifecycle hooks (block/add_context/command/prompt) in hook_definitions.
when_to_use: |
  Use when the user wants deterministic automation around agent lifecycle events
  (BeforeTurn, PreToolUse, PostToolUse, PostToolBatch, PreStop, PreDelivery, PostDelivery),
  including command-hook scripts and prompt-hook policy checks.
license: MIT
compatibility:
  os:
    - darwin
    - linux
    - windows
---

# Create Hook

Use this skill to create or update bot-native hooks stored in SQLite `hook_definitions`.

## Rules

1. Prefer narrow matchers to avoid global side effects.
2. For `command` hooks, `action_payload_json.command` must be a relative path under:
   - `WORKSPACE_DIR/hooks/`
   - repository `builtin_hooks/` (or `FINALLY_A_VALUE_BOT_BUILTIN_HOOKS`)
3. Keep command hooks deterministic and return JSON only.
4. Use `fail_closed` only for policy-critical hooks.
5. Persona-specific behavior (for example PZ cleanup) should be enabled per persona via hook allowlists, not globally.

## Action payload templates

### block

```json
{"reason":"Blocked by hook policy."}
```

### add_context

```json
{"additional_context":"Reminder injected by hook."}
```

### command

```json
{
  "command": "my-hook.py",
  "timeout_secs": 10,
  "fail_closed": false,
  "cwd": "shared"
}
```

### prompt

```json
{
  "prompt": "Review this event and return JSON only. Input: $ARGUMENTS",
  "timeout_secs": 15,
  "fail_closed": false
}
```

## Command hook I/O

- stdin: event JSON (`event`, chat/persona/channel, tool info, stop reason, assistant text)
- stdout: JSON with optional keys:
  - `permission`: `allow` | `deny` | `ask`
  - `reason`, `user_message`, `agent_message`, `additional_context`
  - `updated_tool_input`
  - `effects.memory_tier3_prune.terminal_pz_post_ids`

Non-zero exit behavior:

- exit `2` => deny
- other non-zero => fail-open unless `fail_closed: true`

## Registration flow

1. Build payload JSON for selected action type.
2. Call the `register_hook` tool with:
   - `name`, `event_name`, `matcher`, `action_type`, `action_payload_json`, `enabled`
   - default scope is the current persona only
   - set `global: true` only when the user explicitly asks for cross-persona/global behavior
3. Confirm with `GET /api/hooks`.
4. Optionally tighten persona allowlists via `PATCH /api/personas/:id/policy` for extra defense.
