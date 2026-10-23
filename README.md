<p align="center">
  <img src="web/public/meido-crest.svg" alt="Meido crest" width="180">
</p>

<h1 align="center">Meido</h1>

<p align="center"><strong>A personal agent operating system built beyond single-thread chatbots.</strong></p>

<p align="center">
  <a href="README.md">English</a> ·
  <a href="README.zh-CN.md">简体中文</a> ·
  <a href="README.ja.md">日本語</a>
</p>

Meido gives personal agents continuity: what matters now, what happened before, what your
knowledge base knows, and which operating procedures must govern the next action. Instead of
isolated prompt-and-response sessions, it combines multi-dimensional context management,
durable execution, and one operator-controlled runtime.

One Rust runtime orchestrates **40+ modular skills**, **35+ MCP tools**, scheduled jobs, and
multiple agent engines across the web and chat. Every channel enters the same agent path, so
memory, permissions, safety policy, and traceability stay consistent.

## Why Meido

Most assistants reset at the edge of a chat window. Meido maintains a working context system:

- **Three-tier session memory** — durable identity and principles, active projects, and recent
  focus are stored separately so short-term noise does not overwrite long-term context.
- **MemPalace recall** — semantic retrieval brings back relevant knowledge without stuffing the
  entire archive into every prompt.
- **Bulletin focus** — each persona carries a concise, visible statement of what matters now.
- **Obsidian knowledge vault** — Markdown remains human-readable, portable, and directly
  editable; Meido can search, index, and work with it.
- **Traceable agent history** — append-only memory events and run artifacts make agent behavior
  inspectable instead of opaque.

## One runtime, many surfaces

- **Shared agent path** — web, Telegram, Discord, WhatsApp, WeCom, and scheduled jobs resolve to
  the same orchestration entry point.
- **Multiple agent engines** — run Classic, Cursor, or Gemini ADK per persona while preserving
  the same surrounding context and controls.
- **Modular capability layer** — activate skills only when needed and connect MCP servers without
  building a separate bot for every workflow.
- **Scheduled and background work** — recurring jobs and long-running tasks use the same personas,
  memory, tools, and delivery paths as live conversations.
- **Web operator cockpit** — manage personas, models, channels, hooks, skills, schedules, memory,
  and system status from one interface.

## Safety is part of the operating system

Meido does not treat safety as a prompt suffix. Vault SOPs are injected into agent context,
tool access is authorized server-side, secrets stay masked and redacted, paths are scoped per
persona, and consequential work remains reviewable through agent history and artifacts.

## Core capabilities

- Local workspace access for files, shell commands, browser work, and repository-aware editing
- Obsidian vault search, semantic recall, self-indexing, and Markdown-native knowledge
- Persona-specific memory, models, hooks, skills, permissions, and external paths
- Multi-persona collaboration, guest turns, a shared board, and background handoffs
- Provider support for Anthropic, OpenAI, Gemini, local models, and other configured connectors
- Cross-channel delivery through Web, Telegram, Discord, WhatsApp, and WeCom
- Cron-style scheduling, background jobs, status tracking, and result delivery
- Extensible skills and MCP tooling with SOP-enforced operational guardrails

## Quick Start

The command is `meido`.

### 1. Install the binary

```bash
curl -fsSL https://raw.githubusercontent.com/kenpyfin/meido/main/install.sh | bash
```

That installs the binary. From a checkout, `bash install.sh` builds it. **Windows** — [`install.ps1`](install.ps1) installs the binary the same way. Neither script asks setup questions.

A new workspace gets [`defaults/AGENTS.md`](defaults/AGENTS.md) as `AGENTS.md` at the workspace root (or at `VAULT_PRINCIPLES_PATH` when that is set). The first `meido start` writes it when the file is missing, and saving vault paths from the web UI does the same. An existing principles file is left as-is.

Build from source: `cargo build --release` and `cd web && npm ci && npm run build`.

Meido reads configuration from a **`.env` file in the current working directory** (or from the path in `MEIDO_CONFIG`). Use a dedicated project directory and keep your `.env` there.

### 2. Bootstrap `.env`

Copy the example file and set only bootstrap variables:

```bash
cp .env.example .env
# Set workspace + web bootstrap values in .env
```

Bootstrap variables stay in `.env` (repo root): workspace path, config path override, and web host/port/auth values needed before the web server starts.

### 3. Start and finish onboarding in Web UI

```bash
meido start
```

`meido start` returns immediately and leaves Meido running. Stop that process with `meido stop`. Add `--foreground` to keep it attached to the terminal. Then open **http://127.0.0.1:10961**. The setup wizard opens when workspace, vault, a provider key, the system LLM, MemPalace, or Steel Browser is missing. **Settings → Environment** shows the same `.env` values and writes them back. Secrets stay masked. Editing `.env` by hand shows up the next time Settings loads. Workspace, web bind, and vault path changes need **Restart Meido**. API keys and runtime toggles apply without a restart. MemPalace and Steel Browser install from one button, and that button skips anything already installed. The login service stays optional.

To open the UI on a DNS name such as `pp.local`, set `WEB_HOST=pp.local` and `WEB_AUTH_TOKEN` in `.env`. The process listens on every interface. Meido does not register the name; point it at this machine's LAN IP:

- **mDNS** (names ending in `.local` that match this computer's hostname): on Linux, `sudo apt install avahi-daemon` and `sudo systemctl enable --now avahi-daemon` (`hostname` shows the label, advertised as `<hostname>.local`). Publish a different name with `avahi-publish -a -R pp.local <LAN-IP>`. On macOS, `sudo scutil --set LocalHostName pp`. On Windows, rename the PC to that label.
- **Hosts file**, on each device that should open the UI: `<LAN-IP>  pp.local` in `/etc/hosts` (Linux/macOS) or `C:\Windows\System32\drivers\etc\hosts` (Windows).
- **Router or Pi-hole:** an A record to that LAN IP. Prefer a suffix such as `.lan`. Many routers will not answer names that end in `.local`.

**Useful checks:**

```bash
meido help        # all commands
```

**Minimum configuration**

- **Web UI:** sufficient on its own. Messaging channels (Telegram, Discord, WhatsApp, WeCom) are optional.
- **LLM:** at least one provider API key under **Web UI → Settings → Connections** (Anthropic, OpenAI, Gemini, and the other listed connectors). Choose provider and model on the Persona page. Local providers (`ollama`, `llama`, `llamacpp`) may omit API keys.

See [.env.example](.env.example) for the full list (web UI, scheduler, vault, safety, etc.).

### 4. Web UI

When web is enabled (default), the UI is served at **http://127.0.0.1:10961** by default (`WEB_PORT`). If you bind to a non-local address, set `WEB_AUTH_TOKEN`.

### 5. Login service (optional, macOS and Linux)

`meido start` serves the web UI and returns. It stays up until `meido stop` or a reboot. You do not need the gateway to open the web UI.

`meido gateway install` is a user service that runs that same start at login and restarts Meido if the process exits. Windows has no gateway.

```bash
meido gateway install   # optional: install and start the login service
meido gateway status
meido gateway logs
```

When channel + LLM configuration is complete, Meido seeds an onboarding task so you can get started from chat.

### Vault search (optional)

Built-in vault skills need an embedding endpoint. Configure `VAULT_*` in `.env` when you use the vault integration, and set `VAULT_EMBEDDING_SERVER_URL` in each skill’s local `.env` or in the process environment — see comments in [.env.example](.env.example).

## Documentation

- Docker deployment is not recommended and is no longer supported in this repo. Prefer the native binary install flow and `meido start`. `meido gateway install` is only the optional login service.
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

For a full nested field-by-field reference (including example JSON objects), see docs/memory-framework.md.
Runtime prompts also inject a compact `<memory_field_legend>` plus both `<memory_this_persona>` and `<memory_state_json>` blocks so field meanings and live state are available to the model.

## License

Mixed license. See [LICENSE](LICENSE).

- **kenpyfin modifications and new files (Copyright 2026):** proprietary, source-available — not MIT. Personal, non-commercial use of this tree is allowed; copying, redistributing, or relicensing those changes is not.
- **Upstream-derived portions:** MIT, attribution retained in [LICENSE](LICENSE) as required.
