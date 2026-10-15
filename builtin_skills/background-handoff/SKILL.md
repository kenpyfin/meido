---
name: background-handoff
description: Hand off long-running user asks or subtasks to background execution with a clear status contract.
when_to_use: |
  Use when work may exceed normal interactive latency, waits on queues/GPU/long network, or the user asks to run in the background or be notified asynchronously. Follow the handoff and memory rules in the body after activation.
platforms: [linux, darwin, windows]
source: built-in
version: 1.1.0
updated_at: 2026-05-15
---

Use this skill when a request is likely to exceed normal foreground latency, or when the user explicitly asks to run in the background.

## Shell vs agent background

- **Shell/code** (scripts, builds, GPU CLI, long `bash` work): use the core tool `spawn_background_command`. It runs in tmux and sends a separate completion message when done. On success, the server may auto-enqueue an agent run to summarize outputs for the user; on failure, it may auto-enqueue an agent run with the log output to fix and retry.
- **Full agent re-run** (after web timeout or PTE handoff): return the `##BACKGROUND_JOB_HANDOFF##` sentinel so the server enqueues an agent background job (tokio worker, not tmux).

## Decision rule

- Choose background handoff when any of these are true:
  - expected runtime exceeds normal interactive response window
  - operation waits on external queues/GPU/long network processing
  - user asks to "run in background", "keep working and notify me", or similar

## Handoff contract

1. Acknowledge handoff immediately with:
   - what is being run
   - that work continues asynchronously
2. Return or reference a stable run/job id.
3. Provide progress updates:
   - on significant state changes
   - otherwise every ~30 seconds while active
4. Send final completion/failure summary with key outputs.

## Memory rules

- Store one concise active status in Tier 2.
- Do not keep duplicate "monitoring" lines.
- If stalled, ask user to choose:
  - retry fresh run
  - keep waiting and check later
