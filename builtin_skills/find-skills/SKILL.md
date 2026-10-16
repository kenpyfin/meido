---
name: find-skills
description: Find reusable skills in the vercel-labs/skills registry and suggest how to install or adapt them for FinallyAValueBot.
when_to_use: |
  Use when the user asks whether a skill already exists for a task, wants to discover upstream skills by keyword, or wants to reuse instead of authoring from scratch. Requires curl for GitHub/raw lookups.
license: Proprietary. LICENSE.txt has complete terms
compatibility:
  os:
    - darwin
    - linux
    - windows
  deps:
    - curl
---

# Find Skills (vercel-labs/skills)

Use this skill when users ask:
- "Do we already have a skill for X?"
- "Find skills for <task>"
- "What existing skill can I reuse instead of writing from scratch?"

Primary source:
- https://github.com/vercel-labs/skills

## Discovery workflow

1. Clarify the target task in one sentence.
2. Search the registry by keywords (task, toolchain, framework, platform).
3. For each candidate skill, extract:
   - Skill name/path
   - What problem it solves
   - Required tools/dependencies
   - Any platform assumptions
4. Recommend one best-fit skill and one fallback.
5. If none fit exactly, propose adaptation steps for FinallyAValueBot.

## Useful commands

Search repo metadata quickly:

```bash
curl -s "https://api.github.com/repos/vercel-labs/skills/contents" 
```

Search issues/paths by keyword (example):

```bash
curl -s "https://api.github.com/search/code?q=repo:vercel-labs/skills+keyword"
```

Fetch raw README/skill docs when needed:

```bash
curl -sL "https://raw.githubusercontent.com/vercel-labs/skills/main/README.md"
```

## Output format

When returning results, use this structure:

1. Best match
2. Why it fits
3. Requirements
4. Install/adapt steps for FinallyAValueBot
5. Alternative options

## FinallyAValueBot adaptation hints

- Convert upstream skill metadata to local `SKILL.md` frontmatter (`name`, `description`, optional `platforms`/`deps`).
- Keep instructions actionable with `bash`, file tools, and existing MCP tools.
- If upstream skill assumes another runtime, add a short "FinallyAValueBot notes" section describing equivalent commands.
