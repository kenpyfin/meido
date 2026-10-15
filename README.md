# FinallyAValueBot

FinallyAValueBot is a powerful agentic assistant that lives in your chat (Telegram, Discord, WhatsApp) and has direct access to your local workspace, tools, and Obsidian vault.

## Key Features

- **Agentic Loop**: Autonomous reasoning using LLMs (Claude, GPT, etc.).
- **Local Workspace**: Access to your files, bash, and browser.
- **Obsidian Integration**: Uses your vault as a knowledge base with semantic search.
- **Skill System**: Extensible with custom instructions and tools.
- **Multi-Channel**: Syncs conversations across Telegram, Discord, WhatsApp, and Web.
- **Self-Indexing**: Automatically schedules vault indexing and updates.

## Quick Start

### 1. Install the binary

```bash
curl -fsSL https://raw.githubusercontent.com/finally-a-value-bot/finally-a-value-bot/main/install.sh | bash
```

Alternatives: **Homebrew** (macOS) — `brew tap everettjf/tap && brew install finally-a-value-bot`; **Windows** — see [`install.ps1`](install.ps1) in this repo. Building from source is described in [DEVELOP.md](DEVELOP.md).

The bot reads configuration from a **`.env` file in the current working directory** (or from the path in `FINALLY_A_VALUE_BOT_CONFIG`). Use a dedicated project directory and keep your `.env` there.

### 2. Bootstrap `.env`

Copy the example file and set only bootstrap variables:

```bash
cp .env.example .env
# Set workspace + web bootstrap values in .env
```

Bootstrap variables stay in `.env` (repo root): workspace path, config path override, and web host/port/auth values needed before the web server starts.

### 3. Start and finish onboarding in Web UI

```bash
finally-a-value-bot start
```

Then open **http://127.0.0.1:10961**. Configure **LLM and bot tokens in repo-root `.env`** (see `.env.example`). Use **Settings** for bot integrations (extra instances), persona scope per channel, and optional **restart** when `FINALLY_A_VALUE_BOT_RESTART_COMMAND` is set. Changes to `.env` require a process restart.

**Useful checks:**

```bash
finally-a-value-bot doctor      # preflight diagnostics
finally-a-value-bot test-llm    # test LLM connectivity
finally-a-value-bot help        # all commands
```

**Minimum configuration**

- **Channel:** at least one of **Telegram** (`TELEGRAM_BOT_TOKEN` + `BOT_USERNAME`) or **Discord** (`DISCORD_BOT_TOKEN`).
- **LLM:** at least one provider API key in `.env` (e.g. `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`); choose provider and model in **Web UI → Settings → LLM**. Local providers (`ollama`, `llama`, `llamacpp`) may omit API keys.

See [.env.example](.env.example) for the full list (web UI, scheduler, vault, safety, etc.).

### 4. Web UI

When web is enabled (default), the UI is served at **http://127.0.0.1:10961** by default (`WEB_PORT`). If you bind to a non-local address, set `WEB_AUTH_TOKEN`.

### 5. Background service (optional)

```bash
finally-a-value-bot gateway install   # install and start service
finally-a-value-bot gateway status
finally-a-value-bot gateway logs
```

When channel + LLM configuration is complete, FinallyAValueBot seeds an onboarding task so you can get started from chat.

### Vault search (optional)

Built-in vault skills need an embedding endpoint. Configure `VAULT_*` in `.env` when you use the vault integration, and set `VAULT_EMBEDDING_SERVER_URL` in each skill’s local `.env` or in the process environment — see comments in [.env.example](.env.example).

## Documentation

- [ARCHITECTURE.md](ARCHITECTURE.md): Agentic loop and project structure.
- [DEVELOP.md](DEVELOP.md): Contributing and building from source.
- [TEST.md](TEST.md): Testing guide.
- [docs/memory-framework.md](docs/memory-framework.md): Canonical memory model and safety notes.
- Docker deployment is not recommended and is no longer supported in this repo. Prefer the native binary install flow and `finally-a-value-bot gateway install`.
- [DOCKER.md.bak](DOCKER.md.bak): Archived legacy container notes (reference only).

## Memory File Spec

Canonical persona memory is stored per chat/persona under:

- `groups/{chat_id}/{persona_id}/memory_state.json` (current state)
- `groups/{chat_id}/{persona_id}/memory_events.jsonl` (append-only event log)

Legacy `MEMORY.md` may still exist for migration compatibility, but `memory_state.json` is the operational source of truth.

### `memory_state.json`

Top-level object:

- `meta`: state metadata
- `identity`: persona identity and communication constraints
- `tier1`: long-term durable memory
- `tier2`: active project memory
- `tier3`: short-term recent focus
- `workflow_memory`: intent-pattern retention and outcomes
- `links`: references to external memory/index artifacts

#### `meta`

- `version` (`number`): Schema version. Normalized to current version (`1`) on write.
- `revision` (`number`): Monotonic state revision; incremented on each successful write.
- `updated_at` (`string`, RFC3339 timestamp): Last state write time.

#### `identity`

- `display_name` (`string`): Persona name used for identity grounding.
- `self_model` (`string`): Short self-description/model framing.
- `voice_style` (`string`): Tone/style preferences for responses.
- `non_negotiables` (`string[]`): Hard constraints/rules the persona should not violate.

Normalization notes: list entries are trimmed, empty values removed, and duplicates deduped case-insensitively.

#### `tier1`

- `stable_facts` (`string[]`): Long-lived facts or evergreen context for this persona.
- `workflow_principles` (`string[]`): High-confidence recurring workflow rules learned from repeated successful execution.

Normalization notes: entries are trimmed, empty values dropped, deduped case-insensitively.

#### `tier2`

- `active_projects` (`object[]`): In-flight projects/tasks.
  - `id` (`string`): Stable project key. Auto-derived from summary if missing.
  - `status` (`string`): Project status (defaults to `"active"` if omitted).
  - `summary` (`string`): Human-readable project description (required; empty summary entries are dropped).
  - `updated_at` (`string`, RFC3339 timestamp): Last update timestamp for this project (auto-filled if missing).

Normalization notes: project IDs are unique; duplicate IDs are collapsed (first/highest-priority retained).

#### `tier3`

- `recent_focus` (`string[]`): Short-term current focus items.

Constraint: capped to 15 entries (extras are truncated during normalization).

#### `workflow_memory`

- `intents` (`object[]`): Intent-pattern memory entries.
  - `intent_signature` (`string`): Canonicalized intent key (trimmed + lowercased). Must be non-empty.
  - `approach_summary` (`string`): Brief description of the approach taken.
  - `step_trace` (`string[]`): Ordered high-level steps/tool flow.
  - `outcome` (`string`): Outcome label (for example `success`, `failure`, `unknown`); defaults to `"unknown"` if empty.
  - `failure_reason` (`string | null`): Optional explanation when outcome is not successful.
  - `confidence` (`number`): Confidence score in range `[0.0, 1.0]` (clamped during normalization).
  - `support_count` (`number`): Number of supporting observations/successes for this pattern.
  - `last_seen_at` (`string`, RFC3339 timestamp): Most recent observation time.
  - `evidence_refs` (`string[]`): References to supporting artifacts/logs/messages.

Normalization notes:

- duplicate `intent_signature` entries are merged by keeping the strongest support (`support_count`)
- `intents` are sorted descending by `support_count`
- arrays are trimmed/deduped case-insensitively

#### `links`

- `mem_palace_refs` (`string[]`): References to mem-palace notes/snapshots used for retrieval alignment.

### `memory_events.jsonl`

Append-only newline-delimited JSON log (one JSON object per line), used for auditability and recovery diagnostics.

Each event contains:

- `ts` (`string`, RFC3339 timestamp): Event creation time.
- `event_type` (`string`): Event name (examples: `memory_state_initialized`, `memory_migrated_from_markdown`, `memory_parse_error`, `memory_parse_error_recovered`, `manual_edit`).
- `actor` (`string`): Who initiated the event (for example `system`, `migration`, `user`).
- `chat_id` (`number`): Chat/group ID associated with the event.
- `persona_id` (`number`): Persona ID associated with the event.
- `payload` (`object`): Event-specific metadata (paths, errors, changed fields, etc.).

Common `payload` meanings by `event_type`:

- `memory_state_initialized`: usually contains `schema_version`.
- `memory_migrated_from_markdown`: usually contains `legacy_path` and `schema_version`.
- `memory_parse_error`: usually contains `path`, `backup_path`, and `error`.
- `memory_parse_error_recovered`: usually contains `path`, `backup_path`, and `error` from the failed primary parse.
- `manual_edit`: usually contains changed field metadata (for example `field`).

### Write/validation behavior

- State is normalized before validation/write.
- Validation enforces core invariants (`meta.version >= 1`, `tier3` size cap, non-empty workflow intent signatures, workflow confidence range).
- Writes are atomic (`.tmp` + rename) with backup (`.json.bak`) when previous state exists.
- On parse errors, backup recovery is attempted and corresponding diagnostic events are logged.

For a full nested field-by-field reference (including example JSON objects), see [`docs/memory-framework.md`](docs/memory-framework.md).
Runtime prompts also inject a compact `<memory_field_legend>` plus both `<memory_this_persona>` and `<memory_state_json>` blocks so field meanings and live state are available to the model.

## License

Mixed license. See [LICENSE](LICENSE).

- **kenpyfin modifications and new files (Copyright 2026):** proprietary, source-available — not MIT. Personal, non-commercial use of this tree is allowed; copying, redistributing, or relicensing those changes is not.
- **Original work by everettjf (Copyright 2025):** MIT, retained as required.
