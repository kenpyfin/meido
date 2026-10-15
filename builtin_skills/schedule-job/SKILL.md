---
name: schedule-job
description: Preflight and validation for cron and one-time scheduled tasks before schedule_task or update_scheduled_task.
when_to_use: |
  Always activate before creating or changing timing on scheduled_tasks rows (schedule_task / update_scheduled_task). Use when the user asks about cron expressions, one-time runs, or timezone handling for the bot scheduler.
  Not for vault SOPs — when the user means a workflow/pipeline procedure, follow the ORIGIN vault SOP (search_vault), not cron alone.
license: MIT
compatibility:
  os:
    - darwin
    - linux
    - windows
  deps:
    - python3
---

# Schedule Job

Use this skill when a user asks to **create**, **change**, or **reason about** a **cron / one-time scheduled task** (rows in `scheduled_tasks`, tools `schedule_task` / `update_scheduled_task` / pause / resume / cancel).

**Not vault SOPs:** Standard Operating Procedures live in the ORIGIN vault (markdown). Cron only sets *when* a job runs; the SOP defines *what* to do when it fires. Do not confuse scheduling with executing a pipeline — load the vault SOP (`search_vault`) when running scheduled generation jobs.

## Non-negotiable policy

1. Always run this skill preflight before calling `schedule_task` or before changing timing with `update_scheduled_task`.
2. Cron is normalized to **6 fields**: `sec min hour dom month dow`.
3. If timezone is unknown, assume **UTC** and say that explicitly to the user.
4. If the user provides a timezone, include it in the tool’s `timezone` field.
5. For one-time jobs, use an ISO 8601 timestamp. Prefer explicit offset (for example `+07:00`).

## Scheduling preflight checklist

Before calling `schedule_task` (new row) or `update_scheduled_task` (existing `task_id`), collect and confirm:

- the task prompt (`prompt`) when creating or when changing prompt
- schedule type (`cron` or `once`)
- schedule expression or timestamp (`schedule_value`)
- timezone context (`timezone`, or default to UTC with explicit notice)

Then:

- normalize cron from 5 fields to 6 fields by prepending `0`
- validate cron field count is exactly 6
- validate one-time timestamp is valid ISO 8601

## Helper script

From the **repository root**:

```bash
python3 builtin_skills/schedule-job/schedule_helper.py cron "*/15 * * * *" --timezone "Asia/Bangkok"
python3 builtin_skills/schedule-job/schedule_helper.py once "2026-03-14T09:30:00" --timezone "Asia/Bangkok"
```

If no timezone is known:

```bash
python3 builtin_skills/schedule-job/schedule_helper.py cron "0 9 * * *"
```

The script prints JSON with normalized output and explicit timezone assumptions.

## Mapping to tools

### New task — `schedule_task`

- `schedule_type="cron"`:
  - `schedule_value`: normalized 6-field cron
  - `timezone`: user timezone if known, otherwise `"UTC"`
- `schedule_type="once"`:
  - `schedule_value`: ISO 8601 timestamp (include offset when available)
  - `timezone`: optional; include `"UTC"` when timezone was unknown and defaulted

### Change existing task — `update_scheduled_task`

- Pass `task_id` (from `list_scheduled_tasks`).
- Optional fields: `status`, `persona_id`, `prompt`, and/or **`schedule_type` + `schedule_value`** together (with optional `timezone` for preflight).
- Do not confuse with authored YAML workflows; this only updates the scheduled row.

## Examples

- User: "Remind me every weekday at 9am"
  - If timezone unknown: ask to confirm default UTC phrasing in response and schedule with UTC.
  - Cron example: `0 0 9 * * 1-5`

- User: "Run this once on 2026-04-01 14:00 in Jakarta"
  - Convert/format as ISO 8601 with offset: `2026-04-01T14:00:00+07:00`
  - Use `schedule_type="once"` and `timezone="Asia/Jakarta"`.
