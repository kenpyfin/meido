<p align="center">
  <img src="web/public/meido-crest.svg" alt="Meido crest" width="180">
</p>

<h1 align="center">Meido</h1>

<p align="center"><strong>The chat never starts over.</strong></p>

<p align="center">Official site: <a href="https://meido.app">https://meido.app</a></p>

<p align="center">
  <a href="README.md">English</a> ·
  <a href="README.zh-CN.md">简体中文</a> ·
  <a href="README.ja.md">日本語</a>
</p>

One conversation, kept alive by multi-dimensional context: three-tier memory, MemPalace recall,
a service briefing, and an Obsidian vault. One Rust runtime carries that conversation across the
web and chat, through scheduled jobs, with SOP guardrails and a history you can trace.

[Keep the chat going →](#quick-start)

<p align="center">
  <img src="assets/meido-desk.jpg" alt="The Meido desk: one transcript, topics beside it, and the service briefing above the thread" width="920">
</p>

<p align="center"><em>One desk, one transcript. Topics title stretches of that log. The service briefing sits above the thread. Focus and Transcript are two views of the same chat.</em></p>

## Stop opening a new chat

A cloud chatbot is a stack of finished sessions. The window fills, the thread dies, and the
next message starts from zero. You lose the work that was in motion, then spend the new chat
reconstructing it.

Meido has no session boundary. Context is managed on every turn, so the same conversation
continues. You do not open a fresh thread to keep going.

## How the conversation stays alive

**One thread.** Web, Telegram, Discord, WhatsApp, WeCom, and scheduled jobs enter the same
conversation. There is no new session to resume, and no second brain per channel.

**Context in layers.** Durable facts, active work, and recent focus stay separate. A long chat
does not collapse into the last few messages, and a burst of recent detail does not overwrite
what the work still depends on.

**Recall on demand.** MemPalace and the Obsidian vault bring back what this turn needs. The
archive is not pasted into the prompt, and it is not abandoned inside a dead session.

**Focus that moves.** The service briefing and the topic list say what the conversation is doing
now. Older turns remain part of the history.

## New chat vs. the same chat

**Where a turn begins**

- A typical cloud chatbot opens a session. When you come back, you start another one.
- Meido continues the conversation you already have.

**When the window fills**

- A typical cloud chatbot ends the thread. The next prompt has no past.
- Meido trims history to a budget and keeps the rest of the context: tiers, recall, the service briefing, and the vault.

**Where context lives**

- A typical cloud chatbot keeps context inside that session, on someone else's service.
- Meido keeps it on your machine: persona memory, MemPalace, and a Markdown vault you can read.

**Where you can speak**

- A typical cloud chatbot is one window.
- Meido is the same chat on the web, Telegram, Discord, WhatsApp, and WeCom.

**Work that outlasts the window**

- A typical cloud chatbot waits until you open it again.
- Scheduled jobs and background work re-enter the same conversation, with the same persona and
  tools.

**What you can check later**

- A typical cloud chatbot leaves you the transcript of a closed session.
- Meido leaves agent history, append-only memory events, and artifacts tied to the run.
  Tool access is authorized on the server, secrets stay masked, and vault SOPs stay in context.

## Keep the chat going

Install the binary, start Meido, and finish setup in the web UI. [Quick Start](#quick-start).

## How Meido thinks

A turn is assembled, not dumped. Identity, principles, layered memory, the skills you enabled,
vault operating procedures, and the current task are composed into context. Older history is
trimmed to a budget. Images are capped. The transcript stays one conversation.

Every surface uses that same path. The web UI, Telegram, Discord, WhatsApp, WeCom, and the
scheduler each resolve a chat and hand it to one runtime. A channel does not grow its own
agent.

The engine is a choice. The context is not. Each persona can run the Classic tool loop, Cursor,
or Gemini ADK. Memory, permissions, hooks, and delivery stay wrapped around that choice.

The chat model is not the background model. A system LLM does quiet work such as topic titles
and the service briefing. If that pair is missing, the job is skipped. It does not silently borrow the
persona's chat model.

Work has a procedure. Vault SOPs are part of context. Tools are authorized on the server.
Secrets stay masked. Paths are scoped per persona. A finished turn leaves history and artifacts
you can inspect.

## What you work with

### Operator desk

The web cockpit is the place you run Meido. A setup wizard opens until the workspace, vault,
a model, the system LLM, MemPalace, and the managed browser are in place. Settings and the
Persona page edit the rest. `.env` in the working directory is the bootstrap file. Workspace,
bind address, and vault path changes need a restart. API keys and runtime toggles apply when
you save them. On macOS and Linux, an optional login service starts Meido at login.

### Personas

A persona has a name, an engine, a model, its own memory, a service briefing, hooks, skills, and
external paths. Several personas can share a chat. Each one answers with its own context.

### The ongoing chat

The desk is the conversation. Personas, groups, and the board sit in the sidebar. The open
persona keeps one transcript. A topic list beside it titles stretches of that same log, and
filtering titles does not open a new session. Above the thread, the service briefing states
what this chat is doing now. Focus and Transcript are two views of it. Hide scheduled keeps
scheduled runs out of the way without removing them. Side chats stay attached to this thread.
Bookmarks jump back to a message in it.

### Context that keeps it going

Three tiers hold the conversation's context: durable facts, active work, and recent focus.
MemPalace recalls what is relevant. The service briefing states the current focus above the
thread. The
Obsidian vault is Markdown you can read, search, and index. Field-by-field layout of persona
memory is in [Memory File Spec](#memory-file-spec).

### Collaboration

Type `@Name` to invite another persona. Guests answer as themselves, in the order you tagged
them, on the host transcript. A group is a separate conversation, with its own transcript.
The board is four stages: TODO, PLAN, BUILD, and VERIFY. Deploy and Complete sit on every
card. Today, above the columns, is the live overview of cards, runs, and upcoming work.

### Work that continues

Cron schedules and background jobs use the same personas and tools as a live turn. A long
shell command can run in the background. A finished run can be attached back into the chat.
Files published from a run become artifacts tied to that run, not loose paths dropped into a
reply.

### Engines and models

Classic is the built-in tool loop. Cursor and Gemini ADK are the other engines, chosen per
persona. Connect Anthropic, OpenAI, Gemini, or another configured provider, or point a persona
at a local server (`ollama`, `llama`, or `llamacpp`). A persona can keep a local route for
work that does not need the cloud model.

### What you add

Skills and MCP servers are yours to install on this machine. Meido activates the skills a
persona is allowed to use, and connects the MCP servers you configured. There is no fixed
catalog you have to accept.

### Hooks

Checks run around a turn: before it starts, around tool use, before the reply is delivered,
and after delivery. The built-in set syncs focus, applies scheduler policy, gates skills,
stops a stuck tool loop, handles a reply that is too dense to drop into chat, and scrubs
secrets. You can add your own.

### The machine it can touch

Meido works in your workspace: files, a shell, and repository-aware edits. A managed browser
(Steel) is installed from the setup wizard when you want pages, not just APIs. A new
workspace receives operating principles from [`defaults/AGENTS.md`](defaults/AGENTS.md). An
existing principles file is left as it is.

### Channels

The web UI is enough. Telegram, Discord, WhatsApp, and WeCom are optional ways to reach the
same chat.

### Safety and trace

Tool use is authorized on the server. Each persona has its own path scope. Secrets stay
masked, and output is redacted. Vault SOPs stay in context. Memory changes are appended to
an event log. Agent history records how a turn actually ran.

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
