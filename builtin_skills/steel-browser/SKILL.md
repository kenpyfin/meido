---
name: steel-browser
description: Human-in-the-loop browser automation via Steel (remote Chrome + session viewer). Login-gated social feeds, posting, and interactive flows.
when_to_use: |
  Use when the user needs logged-in browsing, human oversight (session viewer), or interactive automation that web_fetch/scrapling cannot handle.
  Typical: Instagram/X feeds, posting as the user, DMs, multi-step flows behind login walls.
  Workflow: `session create` → share `session_viewer_url` for manual login → `browse *` commands on the same `session_id` → `session release`.
  Prefer web_search/web_fetch for public pages; prefer scrapling-skill for anti-bot public scraping without login.
license: MIT
compatibility:
  os:
    - darwin
    - linux
  deps:
    - python3
    - docker
---

# Steel Browser

Steel provides isolated cloud (or self-hosted) Chrome sessions with a **session viewer** for human-in-the-loop login and oversight. The agent connects over CDP via Playwright.

## Prerequisites

1. **Run Steel** (self-hosted recommended to start):

```bash
docker run -d \
  --name finally-a-value-bot-steel \
  -p 13920:3000 -p 13923:9223 \
  -v finally-a-value-bot-steel-cache:/app/.cache \
  -v finally-a-value-bot-steel-profile:/app/api/user-data-dir \
  -e DOMAIN=127.0.0.1:13920 \
  -e CDP_DOMAIN=127.0.0.1:13923 \
  ghcr.io/steel-dev/steel-browser:latest
```

- API: `http://127.0.0.1:13920`
- Session viewer UI: `http://127.0.0.1:13920/ui`

Or set `BROWSER_MANAGED=true` in repo-root `.env` and the bot will start this container on boot (native installs with Docker available).

2. **Python env** (once per workspace):

```bash
bash builtin_skills/steel-browser/setup_steel_env.sh
```

3. **Configure** — copy `.env.example` to `.env` in this skill directory:

| Variable | Description | Default |
| --- | --- | --- |
| `STEEL_API_URL` | Steel API base URL | `http://127.0.0.1:13920` |
| `STEEL_API_KEY` | API key (Steel Cloud; optional for local) | empty |
| `STEEL_SESSION_TIMEOUT_MS` | Session timeout passed to `api_timeout` | `3600000` |
| `STEEL_USE_PROXY` | `true` / `false` | `false` |
| `STEEL_SOLVE_CAPTCHA` | `true` / `false` | `false` |
| `STEEL_PERSIST` | Self-hosted: reuse Chrome profile across sessions (needs profile volume) | `false` |
| `STEEL_PROFILE_ID` | Steel Cloud profile id to resume | empty |
| `STEEL_PERSIST_PROFILE` | Steel Cloud: snapshot profile on release | `false` |

## Scripts

Run via **`run_skill_script`** after `activate_skill`:

```bash
shared/.venv-steel/bin/python builtin_skills/steel-browser/steel_tool.py <command> [args]
```

Or system Python when `steel-sdk` and `playwright` are installed globally.

## Commands

### Session lifecycle

| Command | Purpose |
| --- | --- |
| `session create` | Create session → JSON with `session_id`, `session_viewer_url`, `websocket_url` |
| `session create --persist` | Self-hosted: load saved Chrome profile (Instagram login survives restarts) |
| `session create --persist-profile` | Steel Cloud: create/update named profile |
| `session status <id>` | Check if session is alive |
| `session release <id>` | Tear down session (persists profile when `STEEL_PERSIST=true`) |

### Browse (requires existing `session_id`)

| Command | Purpose |
| --- | --- |
| `browse goto <id> <url>` | Navigate |
| `browse snapshot <id>` | Page title, URL, text excerpt, interactive elements |
| `browse screenshot <id> [--output PATH]` | PNG screenshot (path or base64 in JSON) |
| `browse click <id> --selector SELECTOR` | Click element |
| `browse fill <id> --selector SELECTOR --text TEXT` | Fill input |
| `browse text <id> [--selector SELECTOR]` | Extract text from selector (default `body`) |

## Human-in-the-loop flow

1. `run_skill_script` → `steel_tool.py session create` (add `--persist` for durable login on self-hosted Docker)
2. Give the user **`session_viewer_url`** from the JSON output
3. Wait for the user to log in manually in the viewer
4. Run `browse goto`, `browse snapshot`, etc. with the same `session_id`
5. `session release <id>` when done (writes profile to the Docker volume when `STEEL_PERSIST=true`)

### Persistent login (self-hosted Docker)

Steel Cloud uses **Profiles** (`--persist-profile`, `STEEL_PROFILE_ID`). Self-hosted Steel uses a mounted Chrome user-data directory instead:

1. Mount `finally-a-value-bot-steel-profile:/app/api/user-data-dir` on the container (included when `BROWSER_MANAGED=true`).
2. Set `STEEL_PERSIST=true` in this skill's `.env`.
3. `session create --persist` → log in once via session viewer → `session release`.
4. Future `session create --persist` calls reopen the same Instagram login, even after `docker restart`.

## Integration with social workflows

When `social-media-scout` hits a login wall or the user asks for "my feed" / "post as me":

1. `web_search` (public peek)
2. `social-media-reader` / RapidAPI (structured)
3. `scrapling-skill` (anti-bot public pages)
4. **`steel-browser`** (login required, human oversight)

## Troubleshooting

- **Connection refused on API port:** Start Steel Docker container (see above) or set `BROWSER_MANAGED=true` for bot auto-start.
- **405 / Method Not Allowed:** Another service may be bound to the Steel API port; change `STEEL_API_PORT` / `STEEL_API_URL` or stop the conflicting process.
- **Missing packages:** Re-run `setup_steel_env.sh`.
- **Empty authenticated page:** User may not have finished login in the session viewer — ask them to complete login, then retry `browse snapshot`.
