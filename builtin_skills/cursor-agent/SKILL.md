---
name: cursor-agent
description: Guide for using the Cursor Agent CLI for code development only—multi-file implementation, refactors, and codebase-wide code changes.
when_to_use: |
  Use only for software development in the repo: features, refactors, bug fixes, and coordinated code edits that are too large or cross-cutting for single-file edits.
  Do not delegate non-code work (copywriting, research-only, ops without code changes, etc.) to cursor-agent.
  Prefer the built-in file tools for tiny one-off edits unless the user explicitly wants cursor-agent.
license: MIT
compatibility:
  os:
    - darwin
    - linux
  deps:
    - cursor-agent
---

# Cursor Agent

Use this skill only for **code development**: complex, multi-file programming work that exceeds comfortable single-file edits. This skill guides how to use the `cursor-agent` CLI for that scope—not for general-purpose delegation.

## Capabilities (code only)

- **Codebase-wide refactoring**: coordinated changes across many source files.
- **Feature implementation**: scaffold and implement modules, APIs, and behavior in code.
- **Bug discovery and fixing**: search for patterns and fix defects in the codebase.

Doc or comment tweaks are in scope only when they are part of a code change (e.g. updating a docstring or README section tied to the implementation).

## When to use

- When a **development** task is too large for the main agent's context.
- When you need deep repo exploration plus coordinated **code** edits.
- When the user explicitly asks to use cursor-agent for a **coding** task.

## Out of scope

Do not use cursor-agent for work whose primary output is not code: standalone writing, research summaries, data analysis, infrastructure runbooks with no repo edits, or other non-development tasks—even if they are large or cross-cutting.

## How to use

Invoke the `cursor_agent` tool or call the CLI directly via `bash`.

### Basic invocation (via bash)

```bash
cursor-agent "Implement a new authentication layer in src/auth"
```

Prompts should describe **what to build or change in code** (paths, behavior, constraints)—not open-ended non-development tasks.

### Detached mode (for long-running tasks)

If tmux is available and supported, use `detach: true` in the `cursor_agent` tool to run it in the background.

```bash
# Example logic for the agent
# 1. Decide on a clear prompt.
# 2. Call cursor_agent tool with prompt and detach: true.
# 3. Monitor the run via list_cursor_agent_runs.
```

## Tips for Better Results

1. **Clear Prompts**: Provide a detailed description of what needs to be changed.
2. **Context**: Mention specific files or directories that are relevant.
3. **Verification**: Always verify the changes made by the agent once it's finished.
4. **Iterate**: If the first run doesn't get it quite right, provide a follow-up prompt to the agent.

## Limitations

- For **code development** only; see Out of scope above.
- The agent is an external process and does not share your immediate conversational memory unless explicitly provided in the prompt.
- Do not use for tasks requiring interactive user input.
