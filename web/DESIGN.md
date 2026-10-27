# Lit Parlor — parlor and service ledger

Operator cockpit for Meido. Adapted from the Linear-derived canvas (deep surface ladder, hairline borders, one scarce accent) into a Japanese maid parlor: wool lamp light, porcelain lace, a claret ribbon, and an antique-brass brooch. This is an application UI, not a marketing page.

---
version: alpha
name: Lit Parlor
description: "A wool parlor canvas (#0c0a10) with a warm plum-charcoal surface ladder, porcelain-ivory ink (#fbf7f2), and a single claret ribbon accent (#e25b78). Antique brass (#e0b56a) is reserved for active-work highlights. Display type is Cormorant Garamond; body stays Inter. Depth comes from lamp glow, warm shadows, and a katyusha lace rule."

colors:
  primary: "#e25b78"
  on-primary: "#1c1619"
  primary-hover: "#e86886"
  primary-focus: "#c23d5e"
  brass: "#e0b56a"
  ink: "#fbf7f2"
  ink-muted: "#b7aea8"
  ink-subtle: "#8f867d"
  ink-tertiary: "#8a817c"
  canvas: "#0c0a10"
  surface-1: "#16131c"
  surface-2: "#1e1926"
  surface-3: "#1c1824"
  surface-4: "#241e2e"
  sidebar: "#100e15"
  hairline: "#3a3144"
  hairline-strong: "#4a4054"
  lace: "#5a3d4e"
  inverse-canvas: "#fdfbf7"
  inverse-surface-1: "#f5f0e8"
  inverse-surface-2: "#faf6f0"
  inverse-ink: "#1c1619"
  inverse-hairline: "#e5ddd0"
  semantic-success: "#4ade80"
  semantic-warning: "#d4a359"
  semantic-danger: "#ef4444"

typography:
  display-md:
    fontFamily: Cormorant Garamond
    fontSize: 32px
    fontWeight: 600
    lineHeight: 1.15
    letterSpacing: -0.4px
  headline:
    fontFamily: Cormorant Garamond
    fontSize: 24px
    fontWeight: 600
    lineHeight: 1.2
    letterSpacing: -0.2px
  card-title:
    fontFamily: Cormorant Garamond
    fontSize: 20px
    fontWeight: 600
    lineHeight: 1.25
    letterSpacing: 0
  body:
    fontFamily: Inter
    fontSize: 16px
    fontWeight: 400
    lineHeight: 1.5
    letterSpacing: 0
  body-sm:
    fontFamily: Inter
    fontSize: 14px
    fontWeight: 400
    lineHeight: 1.45
    letterSpacing: 0
  caption:
    fontFamily: Inter
    fontSize: 12px
    fontWeight: 500
    lineHeight: 1.35
    letterSpacing: 0.04em
  button:
    fontFamily: Inter
    fontSize: 14px
    fontWeight: 500
    lineHeight: 1.2
    letterSpacing: 0
  mono:
    fontFamily: JetBrains Mono
    fontSize: 13px
    fontWeight: 400
    lineHeight: 1.5
    letterSpacing: 0

rounded:
  xs: 4px
  sm: 6px
  md: 8px
  lg: 12px
  xl: 18px
  pill: 9999px

spacing:
  xxs: 4px
  xs: 8px
  sm: 12px
  md: 16px
  lg: 24px
  xl: 32px

components:
  katyusha-header-bar:
    backgroundColor: "{colors.surface-1}"
    textColor: "{colors.ink}"
    typography: "{typography.body-sm}"
    height: 56px
  apron-message-card:
    backgroundColor: "{colors.surface-1}"
    textColor: "{colors.ink}"
    typography: "{typography.body}"
    rounded: "{rounded.xl}"
    padding: 12px 14px
  ribbon-cta-button:
    backgroundColor: "{colors.primary}"
    textColor: "{colors.on-primary}"
    typography: "{typography.button}"
    rounded: "{rounded.md}"
    padding: 8px 14px
  brooch-status-badge:
    backgroundColor: "{colors.brass}"
    textColor: "{colors.on-primary}"
    typography: "{typography.caption}"
    rounded: "{rounded.pill}"
    padding: 2px 8px
  service-ledger-strip:
    backgroundColor: "{colors.surface-1}"
    textColor: "{colors.ink}"
    typography: "{typography.caption}"
  parlor-empty-state:
    backgroundColor: "{colors.canvas}"
    textColor: "{colors.ink-muted}"
    typography: "{typography.headline}"
---

## Overview

The cockpit stays dense. The maid world shows up in materials, not in decoration that gets in the way of a run queue.

**Canvas** `{colors.canvas}` (#060508) is pressed black wool with a warm undertone, not pure black and not the old blue-black Linear canvas. The surface ladder (`{colors.sidebar}`, `{colors.surface-1}` through `{colors.surface-4}`) lifts panels without shadows. Hairlines stay 1px (`{colors.hairline}`, `{colors.hairline-strong}`).

**Claret ribbon** `{colors.primary}` (#d94f70) is the only interactive accent: focus rings, primary actions, the active persona rail, links. **Antique brass** `{colors.brass}` (#d4a359) marks work in progress (running tools, pinned attention). It is not a second CTA color.

**Light mode** is sunlit parlor linen: `{colors.inverse-canvas}` / `{colors.inverse-surface-*}` with espresso ink `{colors.inverse-ink}`. Body text stays at or above 4.5:1. The ribbon accent does not change.

**Key characteristics:**

- Dark-first parlor. Light mode is linen, not a washed-out clone of the dark ladder.
- One chromatic accent (claret). Brass is semantic, for active work.
- Display serif for the wordmark, persona title, and empty-thread greeting. Everything operational stays sans.
- Signature structure is the katyusha lace rule (a repeating scallop under the header and bulletin) and the apron card (assistant messages with a pin-tuck edge and a double hairline).
- Motion stays 150–200ms. No decorative animation beyond the existing thinking pulse.

## Colors

### Brand and accent

- **Claret ribbon** (`{colors.primary}`): primary actions, focus, active selection, inbox emphasis.
- **Ribbon hover** (`{colors.primary-hover}`): hovered ribbon controls.
- **Ribbon press** (`{colors.primary-focus}`): pressed ribbon.
- **On ribbon** (`{colors.on-primary}`): espresso ink on the ribbon so labels stay readable.
- **Antique brass** (`{colors.brass}`): running tools, brooch counters when a task is live. Not for links or buttons.

### Surface (dark)

- **Canvas** (`{colors.canvas}`): page background.
- **Sidebar** (`{colors.sidebar}`): same family as the canvas, one step lighter. No heavy panel contrast.
- **Surface 1–4**: panels, composer tray, menus, nested lifts.
- **Hairline / hairline strong**: 1px structure.
- **Lace** (`{colors.lace}`): the scallop rule. Quiet; it should disappear if you are not looking for the hem.

### Surface (light)

- **Inverse canvas / surfaces**: warm linen and starched apron.
- **Inverse hairline**: lace-colored rules on linen.
- **Inverse ink**: espresso body text.

### Text

- **Ink**: headlines and body on the dark canvas.
- **Ink muted**: secondary labels. Not for long body copy in light mode.
- **Ink subtle / tertiary**: meta, disabled, footnotes.

### Semantic

- Success, warning (brass), and danger stay semantic. Do not recolor them with the ribbon.

## Typography

| Role | Family | Use |
| --- | --- | --- |
| Display | Cormorant Garamond (self-hosted), then Shippori Mincho, Hiragino Mincho ProN, Yu Mincho, Georgia | Wordmark, persona title, welcome greeting, ledger section titles |
| Sans | Inter Variable, Hiragino Sans, Noto Sans JP, system-ui | Controls, thread body, settings |
| Mono | JetBrains Mono | Tool traces, terminal, keyboard chips |

Display is scarce. A settings form does not become a menu card. Tracking on display is slight and negative; eyebrows and ledger labels use a little positive tracking and uppercase sans.

Do not load fonts from Google at runtime. The display face ships via `@fontsource/cormorant-garamond`.

## Layout

- Base unit 4px. Operator density stays `gap-2` / `p-2`–`p-3`.
- Sidebar shares the parlor background and a hairline, not a contrasting slab.
- Radius default is 8px (`--mc-radius`). Assistant apron cards use the larger 18px speech corner. Do not pill-round primary buttons.
- The thread is the main room. The header is the headdress hem. The bulletin is the service ledger under it. The composer is the silver tray.

## Elevation

| Level | Treatment | Use |
| --- | --- | --- |
| 0 | Canvas, no border | Thread field, empty parlor |
| 1 | Surface-1 + 1px hairline | Sidebar rows, ledger cards |
| 2 | Surface-2 + hairline-strong | Composer tray, hovered row |
| 3 | Apron card | Assistant message: gradient, inset hairline, pin-tuck on the leading edge |
| 4 | Focus | 2px ribbon ring at low opacity |

No dramatic drop shadows. A 1px top sheen on lifted panels is enough.

## Signature components

### `katyusha-header-bar`

Sticky header. Surface-1 at 95%, then a 14px scallop (`.mc-katyusha-lace`) along the bottom edge. The straight rule is gone; the canvas shows in the dips. The persona name is display type. On a desk the bar is one row: persona and session on the left, then three tool clusters separated by hairlines. Live attention is `Cockpit`, `Inbox` (Lucide inbox, same stroke as Copy), and `Schedules` when unread runs exist. Configuration is `Persona` and `Settings`. The `Records` menu holds `Schedules` when idle, `Artifacts`, `Terminal`, `Principles`, `Memory`, and `Last agent run`. On a phone the persona name keeps a reserved width so a long chat name ellipsizes beside it.

### `apron-message-card`

Assistant text. Warm parlor gradient mixed from the ribbon, 18px radius with a tighter lower-leading corner, inset double hairline, and a 3px pin-tuck in the ribbon color. User text stays flat and aligned to the right. Do not put the operator's words in an apron card.

### `ribbon-cta-button`

Primary actions. Ribbon fill, espresso label, 8px radius, 150ms hover to `{colors.primary-hover}`. The composer send control is that same 8px ribbon square, with a 14px espresso bow taken from the crest.

### `brooch-status-badge`

Pill for a live count. Brass or ribbon fill, espresso numerals. One badge per cluster.

### `ledger-dialog`

Settings and Persona. Canvas field, panel header, katyusha scallop under the header, uppercase eyebrow with a brass brooch (`Household ledger`, `Attendant dossier`), display title. On a desk the body is a 220px index rail and a page. On a phone the index is the list, and a page opens with Back. A brass word on an index row repeats an owed state. A section is a hairline panel with a top sheen. Its header carries a scope tag: Applies live in muted ink, or Requires restart in brass while that section’s draft differs. A brass pin-tuck on the leading edge means that section is owed (missing setup or restart). Long rules sit in a closed Service note. Persona → Engine is a 2×2 grid: name, one-line summary, and Ready or Needs setup, then the active engine’s settings card. Status rows say Ready, Missing, Restart needed, or No restart owed. Brass repeats an owed word. Muted repeats a settled word. A missing row is a jump (`Open Connections`, `Open persona engine`). The footer switches between the household ledger and the attendant dossier.

### `service-ledger-strip`

Bulletin under the header. Uppercase sans label, display not required. Empty copy is "No active briefing on the tray yet." A brass diamond sits before the label. The same scallop hem as the header runs along the bottom.

### `parlor-empty-state`

Empty thread. Centered crest, display greeting, one line of instruction, then the existing shortcut hints. No illustration scene and no extra accent colors. The crest file is the medallion.

## Theme presets

`data-ui-theme` on `<html>`. The palette menu lists only these six rooms:

| Key | Name | Accent | Dark canvas |
| --- | --- | --- | --- |
| `lit` | Lit Parlor | `#e25b78` | `#0c0a10` (default) |
| `lit-sakura` | Sakura Gaslight | `#f472b6` | `#120a0f` |
| `lit-matcha` | Matcha Tea Room | `#4ade80` | `#08100c` |
| `lit-salon` | Velvet Salon | `#7aa2f7` | `#080b12` |
| `lit-amber` | Amber Hearth | `#f59e42` | `#110c08` |
| `lit-wisteria` | Wisteria Room | `#b48ef7` | `#0d0914` |

All six share one dark parlor structure under `html.dark[data-ui-theme^="lit"]`. The crest sits on the sidebar with a transparent background. The composer field is a dark recessed well (`color-mix` of wool and panel) inside a ribbon-tinted tray. Porcelain (`--mc-porcelain`) is only the schedule run-sheet header. Each room sets its own wool, lamp glow, scallop, ribbon, brass, and warm shadow. The assistant apron is `22px 22px 22px 8px`. Warm shadows stay on the apron, the composer tray, and the desktop run sheet. Light mode keeps the shared linen surfaces and retints `--mc-accent` and `--mc-accent-brass`. Lit Parlor is the default; a saved key that is no longer listed falls back to `lit`.

## Service principles

Interaction is specified in `docs/design-system.md` (Interaction) and `.cursor/rules/meido-design-principles.mdc`. Those rules decide which control is the ribbon, which box is brass, and what a notice says. This file decides materials.

- One ribbon button per surface. That is the commit.
- Header tools stay secondary. They do not write.
- Brass means in progress or still owed (a running tool, a pending queue, Restart Meido). It is not a second commit color.
- Status is a word. The tint repeats the word.

## Do

- Keep the ribbon scarce: the one commit on a surface, focus, selection, links.
- Use brass for in-progress work, a pinned mark, or a consequence that is still owed (restart, missing key).
- Use display type for the wordmark, the open persona, welcome, and ledger titles.
- Keep light-mode body text on `--mc-text-primary`.
- Honor `prefers-reduced-motion`.

## Don't

- Don't add a second CTA color.
- Don't use emoji as icons.
- Don't load Google Fonts in the embedded UI.
- Don't put drop shadows on cards (`0 25px 50px` and similar), except the apron, composer tray, and desktop run sheet on the dark `lit` family.
- Don't restyle the agent persona or system prompt from this file. Voice is not a coat of paint.
- Don't pill-round primary buttons or fill large regions with the ribbon.

## Product tokens

See `docs/design-system.md`. CSS variables stay `--mc-*`. Shadcn tokens bridge from them.
