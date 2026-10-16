---
name: apple-reminders
description: Manage Apple Reminders on macOS using remindctl (lists, tasks, completion, deletion).
when_to_use: |
  macOS only, with remindctl installed. Use when the user asks to add, list, complete, edit, or remove items in Apple Reminders (Reminders.app) or manage reminder lists.
  Not for Apple Notes, Calendar-only workflows, or generic todo apps unless the user means Apple Reminders.
license: Proprietary. LICENSE.txt has complete terms
compatibility:
  os:
    - darwin
  deps:
    - remindctl
---

# Apple Reminders (remindctl)

Use this skill when users ask to manage Apple Reminders.

## Prerequisites

- macOS
- `remindctl` installed:

```bash
brew install steipete/tap/remindctl
```

- Grant Reminders access if prompted.

## Authorization checks

```bash
remindctl status
remindctl authorize
```

## Core commands

Today / tomorrow / week:

```bash
remindctl today
remindctl tomorrow
remindctl week
```

Add reminder:

```bash
remindctl add "Buy milk"
remindctl add --title "Pay rent" --list Personal --due tomorrow
```

Edit reminder:

```bash
remindctl edit 1 --title "Pay rent (landlord)" --due 2026-02-10
```

Complete reminder:

```bash
remindctl complete 1
```

Delete reminder:

```bash
remindctl delete 1 --force
```

List operations:

```bash
remindctl list
remindctl list Work
remindctl list Projects --create
```

JSON output for scripting:

```bash
remindctl today --json
```

## Usage guidance

- Prefer read/list before mutate/delete.
- Confirm IDs and list names before edit/complete/delete.
- If permission is denied, enable Terminal in:
  System Settings > Privacy & Security > Reminders.
