---
name: create-skill
description: Authoring guide for FinallyAValueBot skills (YAML catalog + full body after activate_skill).
when_to_use: |
  Use when the user asks to create a new skill, define skill frontmatter for a new capability, or integrate a new SKILL.md via build_skill. For updating an existing skill, activate `modify-skill` instead. Also use when you need the canonical checklist for rate limits, credentials layout, and discovery rules.
license: MIT
platforms:
  - linux
  - darwin
  - windows
deps: []
---

# Create skill

Use this skill when the user asks you to **create** a new skill, add a capability, or when you need to author a skill from scratch. For **updates** to an existing skill, activate **`modify-skill`** first. Follow this guide so skills integrate with discovery, the **frontmatter-only** system prompt catalog, and `activate_skill`.

## Skill structure

Every skill lives in its own directory under the skills folder:

```
skills/
  my-skill/
    SKILL.md          # Required — frontmatter + instructions
    .env              # Optional — credentials or config
    helper_script.py  # Optional — any supporting files
```

The only required file is `SKILL.md`. The skill manager discovers skills by scanning for `SKILL.md` (or `skill.md`) in subdirectories under `WORKSPACE_DIR/skills/` (not under `shared/workspace/skills/`).

## Path discipline (strict)

When creating or editing skills:

- Prefer **`build_skill`** — it writes to the absolute skills directory on disk.
- If using `write_file`, path must be **`skills/<name>/SKILL.md`** (tool-relative from `shared/` cwd), never `workspace/skills/...`.
- In SKILL.md body examples, invoke scripts as `python3 skills/<name>/script.py`, not `workspace/skills/...`.
- Never use bash `mkdir workspace/skills/...` from tool cwd; that creates an undiscoverable shadow tree.

## SKILL.md format

The file must start with YAML frontmatter between `---` fences, followed by a markdown body. The **system prompt lists only frontmatter** (plus optional compact meta). The **full body** loads when the agent calls `activate_skill` for that skill name.

```markdown
---
name: my-skill
description: One-line summary of what the skill does (shown in the catalog).
when_to_use: |
  Bullet or short paragraph: user intents, tools involved, and when not to use this skill.
license: MIT
compatibility:
  os:
    - darwin
    - linux
  deps:
    - curl
    - python3
---

# My Skill

Instructions for the agent on how to use this skill (procedures, commands, examples).
```

### Required frontmatter fields

| Field | Type | Description |
|---|---|---|
| `name` | string | Skill name (should match the directory name). Lowercase, hyphens for spaces. |
| `description` | string | Tight one-line summary for the `<available_skills>` catalog. |
| `when_to_use` | string | Routing hints: when to call `activate_skill` for this skill, typical user asks, platform/deps assumptions, and explicit non-goals. May be a YAML block (`|`) for multiple lines. |

### Optional frontmatter fields

| Field | Type | Description |
|---|---|---|
| `license` | string | License identifier (e.g. `MIT`, `Apache-2.0`). |
| `compatibility.os` | list | Target platforms: `darwin`, `linux`, `windows`. Omit to mean all. |
| `compatibility.deps` | list | Required external tools (e.g. `curl`, `python3`, `ffmpeg`). |
| `source` | string | URL for the upstream source, if adapted from elsewhere. |
| `version` / `updated_at` | string | Optional semver / ISO-style stamp for operator visibility in catalog meta. |

### Body content

The markdown body after the frontmatter is what the agent reads **after** `activate_skill`. Write it as instructions **for the agent**, not for a human. Include:

1. **How to use** — Step-by-step instructions with concrete commands, code snippets, or tool invocations.
2. **Environment / prerequisites** — What needs to be installed or configured.
3. **Examples** — Realistic usage examples.
4. **Troubleshooting** — Common failure modes and fixes.

Put **when-to-activate** guidance in frontmatter (`when_to_use`), not only here, so the catalog stays accurate without pasting the body into the system prompt.

## Best practices

- **Write for the agent.** The SKILL.md body is loaded for the model after `activate_skill`. Be direct: "Run this command", "Use this tool", "Return this format".
- **Be specific.** Include exact commands, file paths, API patterns. Avoid vague instructions like "configure as needed".
- **Split routing vs procedures.** Keep matchable triggers in `when_to_use` and `description`; keep long procedures in the body.
- **Bundle scripts.** If the skill needs helper scripts (Python, bash, etc.), put them in the skill directory. In the SKILL.md body, document execution with **`run_skill_script`** (e.g. `run_skill_script(skill_name="my-skill", script="helper.py", args=[...])`). Use `skills/my-skill/helper.py` only for **file tool** reads, not bash from persona cwd.
- **Credentials in .env.** If the skill needs API keys or secrets, instruct the user to create `skills/my-skill/.env` with the required variables. Never hardcode secrets.
- **API Caution.** For any skill that makes API calls, you MUST be cautious about rate limits and usage. Optimize the call efficiency (e.g. batching, caching, or avoiding redundant calls) and include error handling for rate limit hits (e.g. 429 status codes).
- **Keep it focused.** One skill = one capability. Don't create a mega-skill that does everything.
- **Test the commands.** Before finalizing, verify that all commands and code snippets in the skill actually work.

## How to create a skill

Use the `build_skill` tool, which handles creating the directory and writing `SKILL.md`:

```
build_skill(name="my-skill", description="...", instructions="...")
```

Or create the files directly with `write_file` (tool-relative path only):

```
write_file(path="skills/my-skill/SKILL.md", content="---\nname: my-skill\n...")
```

Do **not** use `workspace/skills/my-skill/SKILL.md` — that path lands under `shared/workspace/` and will not appear in `/skills` or `activate_skill`.

After creating the skill at the canonical `skills/<name>/` location, it becomes available immediately for activation via `activate_skill`.

## Example: minimal skill

```markdown
---
name: translate
description: Translate text between languages using the trans CLI.
when_to_use: |
  Use when the user asks to translate words, sentences, or files between natural languages and trans is an acceptable backend.
license: MIT
compatibility:
  deps:
    - trans
---

# Translate

Use this skill when the user asks to translate text between languages.

## Usage

```bash
trans -b "Hello world" :es
```

Flags:
- `-b` — brief mode (translation only)
- `:es` — target language code (ISO 639-1)
- `-s en` — explicitly set source language

## Common language codes

| Language | Code |
|---|---|
| Spanish | es |
| French | fr |
| German | de |
| Japanese | ja |
| Chinese | zh |
```

## Example: skill with helper script

```markdown
---
name: image-resize
description: Resize and optimize images via a bundled Python script.
when_to_use: |
  Use when the user wants to resize, compress, or convert raster images and a local python3 script is appropriate.
compatibility:
  deps:
    - python3
---

# Image Resize

Run the bundled Python script:

```bash
python3 skills/image-resize/resize.py input.png --width 800 --output resized.png
```

Supported formats: PNG, JPEG, WebP, GIF.
```

Then also create `skills/image-resize/resize.py` with the actual implementation.

Put credentials in `skills/<skill-name>/.env` when needed (never commit secrets).
