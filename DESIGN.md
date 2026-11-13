---
name: AIMyChats
description: A native Mac viewer for your own WeChat history — one accent that only marks what you chose.
colors:
  accent: "#0D7A45"
  accent-deep: "#085F37"
  accent-tint: "#EAF7F0"
  accent-tint-2: "#D8EFE3"
  select-paper: "#E6F5EC"
  select-ink: "#06693C"
  select-line: "#CCE7D8"
  paper: "#FFFFFF"
  paper-2: "#FAFAFB"
  paper-3: "#F2F2F4"
  chrome: "#ECECEF"
  chrome-line: "#DBDBDE"
  window: "#F0F0F2"
  label: "#1C1C1E"
  label-2: "#5C5C61"
  label-3: "#8A8A8F"
  hairline: "#E7E7EA"
  hairline-strong: "#D5D5D9"
  ok: "#2F6B4A"
  warn: "#855511"
  danger: "#A32B2B"
  info: "#2C5578"
typography:
  page-title:
    fontFamily: "-apple-system, BlinkMacSystemFont, SF Pro Text, PingFang SC, sans-serif"
    fontSize: "20px"
    fontWeight: 600
    lineHeight: 1.3
    letterSpacing: "-0.022em"
  section-title:
    fontFamily: "-apple-system, BlinkMacSystemFont, SF Pro Text, PingFang SC, sans-serif"
    fontSize: "14px"
    fontWeight: 600
    lineHeight: 1.4
    letterSpacing: "-0.008em"
  body:
    fontFamily: "-apple-system, BlinkMacSystemFont, SF Pro Text, PingFang SC, sans-serif"
    fontSize: "14px"
    fontWeight: 400
    lineHeight: 1.45
    letterSpacing: "-0.003em"
  reading:
    fontFamily: "-apple-system, BlinkMacSystemFont, SF Pro Text, PingFang SC, sans-serif"
    fontSize: "15px"
    fontWeight: 400
    lineHeight: 1.7
    letterSpacing: "-0.003em"
  list-primary:
    fontFamily: "-apple-system, BlinkMacSystemFont, SF Pro Text, PingFang SC, sans-serif"
    fontSize: "13.5px"
    fontWeight: 500
    lineHeight: 1.4
  secondary:
    fontFamily: "-apple-system, BlinkMacSystemFont, SF Pro Text, PingFang SC, sans-serif"
    fontSize: "13.5px"
    fontWeight: 400
    lineHeight: 1.65
  label:
    fontFamily: "-apple-system, BlinkMacSystemFont, SF Pro Text, PingFang SC, sans-serif"
    fontSize: "11px"
    fontWeight: 600
    lineHeight: 1.4
    letterSpacing: "0.04em"
  mono:
    fontFamily: "ui-monospace, SF Mono, SFMono-Regular, Menlo, monospace"
    fontSize: "12px"
    fontWeight: 400
    lineHeight: 1.55
    letterSpacing: "-0.01em"
    fontFeature: "tnum"
rounded:
  ctl: "6px"
  inner: "8px"
  bubble: "9px"
  group: "11px"
  pill: "12px"
spacing:
  xs: "4px"
  sm: "6px"
  md: "8px"
  lg: "12px"
  xl: "14px"
  xxl: "18px"
  page-inset-x: "40px"
  page-inset-y: "34px"
components:
  button-primary:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.paper}"
    typography: "{typography.list-primary}"
    rounded: "{rounded.ctl}"
    padding: "0 13px"
    height: "28px"
  button-secondary:
    backgroundColor: "{colors.paper}"
    textColor: "{colors.label}"
    typography: "{typography.list-primary}"
    rounded: "{rounded.ctl}"
    padding: "0 13px"
    height: "28px"
  panel:
    backgroundColor: "{colors.paper}"
    rounded: "{rounded.group}"
    padding: "18px 20px 20px"
  source-row:
    backgroundColor: "{colors.chrome}"
    textColor: "{colors.label}"
    typography: "{typography.body}"
    rounded: "{rounded.ctl}"
    padding: "0 8px"
    height: "28px"
  source-row-selected:
    backgroundColor: "{colors.select-paper}"
    textColor: "{colors.select-ink}"
    rounded: "{rounded.ctl}"
  input:
    backgroundColor: "{colors.paper}"
    textColor: "{colors.label}"
    typography: "{typography.list-primary}"
    rounded: "{rounded.ctl}"
    padding: "0 9px"
    height: "28px"
  segmented-item:
    backgroundColor: "{colors.paper}"
    textColor: "{colors.label}"
    typography: "{typography.secondary}"
    rounded: "{rounded.ctl}"
    padding: "0 13px"
    height: "22px"
  chip:
    backgroundColor: "{colors.paper}"
    textColor: "{colors.label-2}"
    typography: "{typography.secondary}"
    rounded: "{rounded.pill}"
    padding: "0 10px"
    height: "24px"
---

# Design System: AIMyChats

## Overview

**Creative North Star: "The Reading Desk"**

AIMyChats is a Mac document tool that happens to read chat history. The window
is a unified title bar over a full-height source list, the reading plane is
paper white, and the only colour in the product is a green mark on the thing the
operator has decided to keep. Everything else — chrome, rules, counts, badges —
is graphite on neutral grey. The interface is deliberately unremarkable in the
way that Things 3 is unremarkable: the conventions are the design.

Density is high but ordered. A 14px system face carries 10k-message lists; page
titles are 20px on the page itself, never inside the first group; each settings
area is a stack of hairline panels with the commit action closing the form. The
material is flat CSS on a real window: no floating cards, no bevels, no
gradient chrome. Depth exists only where something is transient — a drawer, a
dialog, a toast.

This is not a chat client, and it does not imitate WeChat. Bubbles have their
native silhouette so the transcript stays legible, but the surface exists for
review, selection, and production, and the selection mark is the loudest thing
on screen by design.

**Key Characteristics:**
- Unified title bar, 224px full-height source list, cool-neutral chrome (`#ECECEF`) against paper white (`#FFFFFF`).
- One green accent (`#0D7A45`) meaning exactly one thing: this was chosen, or this commits.
- Hairline separators and 11px grouped panels instead of cards and shadows.
- SF Pro / PingFang SC at 14px base; 20px page titles; 11–13.5px secondary tiers; tabular numerals for every count and id.
- Authored 16px SVG icons at 1.4 stroke — never glyphs, never emoji.
- Native control proportions: 28px buttons and fields, 22px segmented items, AppKit-style checkboxes.

## Colors

A green that appears rarely, on a neutral Mac ground — the accent is a verdict,
not a theme.

### Primary

- **Verdict Green** (`#0D7A45`): the single accent. Filled primary buttons, checked checkboxes, the selected conversation row's icon, the chosen-message check badge, focus rings. Deepened from WeChat's own green so white button text holds 4.8:1.
- **Verdict Deep** (`#085F37`): primary button border and hover/pressed fill; accent-coloured text on tint.
- **Chosen Tint** (`#EAF7F0`) and **Chosen Tint 2** (`#D8EFE3`): selected row background, hover on a selected row, the chosen message's wash, search-hit highlight.
- **Selection Ink** (`#06693C`) and **Selection Line** (`#CCE7D8`): text inside a selected row/group and the 0.5px hairline that closes its edge.

### Neutral

- **Paper** (`#FFFFFF`): reading plane, panels, field backgrounds. The transcript lives here, and it is genuinely white, not off-white.
- **Paper 2** (`#FAFAFB`): conversation rail, system notices, inset stat strips.
- **Paper 3** (`#F2F2F4`): pressed secondary buttons, the other party's bubble fill, disabled fields.
- **Chrome** (`#ECECEF`) over **Window** (`#F0F0F2`): source list and window ground. Cool, never beige, so paper reads as paper.
- **Chrome Line** (`#DBDBDE`): the one structural divider between source list and content.
- **Graphite** (`#1C1C1E`), **Graphite 2** (`#5C5C61`), **Graphite 3** (`#8A8A8F`): primary text, secondary text and help copy, tertiary metadata, timestamps, counts, group labels.
- **Hairline** (`#E7E7EA`) and **Hairline Strong** (`#D5D5D9`): panel borders, row separators, input borders, bubble outlines.
- **Semantic** (`#2F6B4A` ok, `#855511` warn, `#A32B2B` danger, `#2C5578` info) ship with their own tints and appear only when they carry literal meaning: a proven key, a WAL caveat, a failed capture, a local endpoint.

### Named Rules

**The One Meaning Rule.** Green means "chosen" or "this commits". It never means unread, new, group, info, or decoration — unread counters, group badges and nav group labels are graphite on a neutral fill. If a new element wants green, ask what the operator decided.

**The No Warm Drift Rule.** Chrome, rules and tints stay on the cool neutral axis. Paper is `#FFFFFF`; a warm grey (`#F7F5F1` and relatives) is the failure mode this palette exists to prevent.

**The Semantic Is Not Decorative Rule.** Status colour is earned by state data (`ok` / `warn` / `danger` / `info`), never used to make a panel look alive. A page with no status has no status colour.

## Typography

**Display Font:** SF Pro Text / system UI (`-apple-system`, `BlinkMacSystemFont`, `"PingFang SC"`)
**Body Font:** the same stack — one face for the whole app
**Label/Mono Font:** SF Mono / `ui-monospace` for ids, hashes, byte sizes, timestamps

**Character:** System-native and content-first. The face disappears so the Chinese copy and the Latin identifiers can share a line without a style seam. There is no brand typeface; the brand is the restraint of the ramp.

### Hierarchy

- **Page title** (600, 20px, 1.3, −0.022em): one per area, on the page margin above the first panel — 聊天记录, 聊天白名单, 关于.
- **Panel title** (600, 14px, 1.4, −0.008em): the name of a group inside a panel.
- **Body / reading** (400, 14px base, 15px inside the transcript, 1.7): help copy at 13.5px/1.65; message bodies capped at 68ch so a long evening post never becomes a line the eye loses.
- **List primary** (500, 13.5px, 1.4): conversation names, history titles, buttons, field values.
- **Secondary** (400, 12–12.5px): previews, metadata lines, stat labels; tertiary metadata drops to 11.5px `--label-3`.
- **Label** (600, 11px, +0.04em): the `dt` terms in 关于 meta grids and the one uppercase-ish tier; Chinese copy stays unrelettered, the tracking exists for Latin keys only.
- **Mono** (400, 12px, tabular): account ids, database counts, byte sizes, model names, the live selected-message count.

### Named Rules

**The Tabular Truth Rule.** Every number that the operator might compare — `20 个库`, `16,159`, `242.9 MB`, `已选 1 个`, a timestamp column — uses tabular numerals. Proportional digits in a column are a bug, not a style.

**The One Face Rule.** No second webfont, no display face, no hosted font. The CSP forbids remote assets and the app ships offline; the system stack is the design commitment, not a fallback.

**The Title On The Page Rule.** A page title is part of the page frame (20px, outside any panel). It is never a panel heading and never an eyebrow above one.

## Layout

The window is `1360 × 900` by default with a `940 × 640` floor, and it is one
full-height grid: **224px source list** plus the content plane. The source list
starts under transparent traffic lights (44px drag strip, lights at 18/18), then
a 30px brand row, grouped nav rows at 28px, and a foot line carrying the read-only state
(「只读 · 本地副本」) and the promise that data stays on this machine.

The chat surface inside that frame is its own grid: **288px conversation rail**
(`#FAFAFB`) beside the paper transcript, growing to `288 / 1fr / 404` when the
assemble-and-produce drawer is open. The transcript is a two-column row grid
(26px avatar, `1fr` body capped at 68ch) with a day-divider rhythm.

Settings areas share one frame: a scrollable page with `max-width: 1100px`,
`34px 40px 56px` padding, `.page-head` holding the 20px title and its lead
sentence, then panels separated by 14px, each `18px 20px 20px` with an
11px radius and a hairline border. The action that commits the page sits
right-aligned under the last panel (`.page-actions`), never floating in a
toolbar.

Spacing runs on a 2/4/6/8/12/14/18/20 rhythm. Density tiers: 28px controls and
nav rows, 26px chat-surface buttons, 24px filter chips, 22px segmented items,
8–12px row padding in tables.

Breakpoints are window-driven: **1180px** narrows the rail to 264px and the
drawer to 360px; **980px** turns the source list into a horizontal strip under
the title bar (78px left inset for the traffic lights, each nav group keeping
its own width); **900px** collapses chat to a single pane with the drawer
overlaying at `min(420px, 100%)`; **760px / 720px** tighten page padding and
stack the date grid. No phone-sized layout ships — 940px is the floor.

### Named Rules

**The Fixed Frame Rule.** Only the content plane scrolls. The source list, the
page header and the commit action never move; a scrollbar inside a rail is the
only scrollbar you should see in a side region.

**The 40px Inset Rule.** Every settings page uses the same `40px` side inset and
1100px measure; a page that ends early still aligns its panels to the same
edges as one that fills the window.

## Elevation & Depth

The system is flat at rest. There are no panel shadows, no card lift, no hover
elevation: separation is carried by hairlines (`#E7E7EA`), tonal steps
(`#FFFFFF` → `#FAFAFB` → `#F2F2F4`) and the chrome/paper contrast between
regions.

Depth appears only for things that are transient or floating, where a Mac user
expects it: the produce/assemble drawer and dialog, the toast, the popover, and
the 0.5px contact edge that makes a button read as pressable.

### Shadow Vocabulary

- **Control edge** (`0 0.5px 1px rgba(24, 22, 20, 0.14)`): the default primary/secondary button, and the segmented control's selected thumb (`0 0.5px 1.5px … , 0 0 0 0.5px …`) — a contact line, not a lift.
- **Popover** (`0 12px 34px rgba(24, 22, 20, 0.18), 0 1px 2px rgba(24, 22, 20, 0.10)`): window-level floating surfaces.
- **Drawer / dialog** (`0 22px 60px rgba(24, 22, 20, 0.28), 0 2px 6px rgba(24, 22, 20, 0.12)`): the portalled produce panel.
- **Toast** (`0 8px 24px rgba(24, 22, 20, 0.16)`): the one notification surface, with `backdrop-filter: saturate(180%) blur(14px)` over the transcript.

### Named Rules

**The Flat-By-Default Rule.** If an element is part of the page, it gets a hairline, not a shadow. Shadows are reserved for surfaces that sit above the page and can be dismissed.

**The One Edge Rule.** Pressable controls carry a single 0.5px contact line. Two shadows on a control is a card trying to be a button.

## Shapes

Geometry is AppKit-standard: 6px control radius, 7px avatar plate (6px at the
small size), 8px inner block and unread badge, 9px message bubble, 11px grouped
panel, 12px chip. Every corner is a rounded rectangle; the only circle in the
system is the 12px chosen-message check disc. Nothing is 16px.

Separators are always 1px hairlines (0.5px inset box-shadows where a border would
add layout width). Panels are outlined, never filled grey. The sidebar has one
structural border on its right; every other division is a row separator.

The chosen-message mark is the signature silhouette: a tinted row, a green
hairline around the bubble, and a 12px green circle with a white check at the
row's trailing edge — deliberately *not* a decorative side rule.

## Components

### Buttons

- **Shape:** 6px radius (`--r-ctl`), 28px height, 13px side padding, 13.5px/500 label, inline-flex with a 6px icon gap.
- **Primary:** Verdict Green fill, `#085F37` border and hover, white text — used only for the action that commits the page (保存…, 开始总结, 授权).
- **Secondary:** white fill, 1px `rgba(27,27,25,0.16)` border, `--paper-3` hover, inset pressed edge.
- **Danger:** white fill with a `rgba(163,43,43,0.28)` border and `#A32B2B` text; tint fill only on hover (删除本地副本).
- **Focus:** `2px` accent outline at `2px` offset on every button variant. Disabled drops to 0.42–0.45 opacity and loses the contact edge.
- **In-surface variant:** the chat plane uses a 26px/13px button so controls stay subordinate to the transcript.

### Chips

- **Style:** white pill (12px radius), 24px tall, 12.5px `--label-2`, 1px `rgba(27,27,25,0.14)` border.
- **State:** hover goes `--paper-3` with graphite text; chosen fills Verdict Green with a `#085F37` border and white text — the same one meaning as everywhere else.

### Panels / Containers

- **Corner:** 11px (`--r-group`); inner containers (stat strips, session lists) 8px.
- **Background:** paper white; nested blocks use `--paper-2` / `--paper-3` tonal steps, never a second border stack.
- **Border:** 1px `--line`; no shadow.
- **Padding:** 18px 20px 20px for a page panel; 10–12px inside list rows; 14px for empty states (dashed `--line-strong`, `--paper-2` fill).
- **Rhythm:** 14px between panels, `18px` between page header and first panel, `14px` between the last panel and the commit action.

### Inputs / Fields

- **Style:** 1px `rgba(27,27,25,0.18)`, 6px radius, 28px height (`9px` side padding), inset `0 1px 1.5px` contact shade, white fill.
- **Focus:** accent border plus a `3.5px` `rgba(13,122,69,0.22)` ring; no layout shift.
- **Select:** native control reshaped — appearance stripped, 26px right padding, 10×12 SVG chevron.
- **Checkbox:** drawn explicitly at 14px, 4px radius, `rgba(27,27,25,0.28)` border, accent fill and a white SVG check when chosen (WKWebView otherwise drops the box on checked rows). Focus ring uses `--select-line`.
- **Disabled:** `--paper-3` fill with `--label-3` text.

### Navigation

- **Source list:** `#ECECEF` plane, 28px rows, 16px authored icon + 14px label + optional mono count, 9px gaps. Hover is a 5% ink wash; the active row is `--select-paper` with `--select-ink` text, a 0.5px `--select-line` inset, and an accent icon.
- **Group labels:** 11.5px/600, `--label-3`, `0.03em` tracking, hidden on the narrow strip.
- **Segmented control:** `--fill-1` trough at 8px radius, 2px padding, 22px items; the chosen item is white with a 0.5px shadow edge — never green, because navigation is not a choice the operator made about content.

### Transcript and the chosen mark (signature)

- **Rows:** 5px 22px 5px 20px padding, 26px avatar column, 110ms background transition, focus outline inset by 2px.
- **Bubbles:** 15px/1.7, 9px radius, `--paper-3` for the other party and white with a `--line-strong` outline for self; max 68ch.
- **Chosen:** `--accent-tint` row wash, `rgba(13,122,69,0.38)` bubble outline, 12px accent check disc at the trailing edge. Selection is toggled per message and extended with Shift range across conversations.
- **Day dividers:** sticky at the top of the reading plane — 11px mono, `0.06em` tracking, `--label-3`, `12px 24px 10px` padding, on a paper-coloured fade, with a hairline that fills the rest of the row. The date stays with the messages it labels.

### Toast

A single dismissible notice in the bottom-right corner of the chat plane: 13px text, 8px radius, `--line-strong` border, 96%-opaque paper with a 14px blur, error variant in `#862222` on `--danger-tint`. One at a time; it never covers the composer or the commit action.

## Do's and Don'ts

### Do:

- **Do** keep exactly one accent and use it only for chosen state and the committing action.
- **Do** put the 20px page title on the page margin and start the first panel 18px below it.
- **Do** close a settings form with its commit button right-aligned under the last panel.
- **Do** use 1px hairlines and tonal steps to separate regions; reserve shadow for drawer, popover, toast.
- **Do** use 11px/12px mono with `font-variant-numeric: tabular-nums` for every count, id, hash and byte size.
- **Do** author icons as 16px SVG at 1.4 stroke, and draw checkboxes explicitly for WKWebView.
- **Do** state failure as a fact plus the next action (「未检测到授权」→「去系统设置」), never a spinner.
- **Do** respect `prefers-reduced-motion` — drop drawer/card animations and the message-row transition.

### Don't:

- **Don't** use green for unread counters, group badges, "new" markers, info states, or nav group labels.
- **Don't** put a page title inside the first panel, or float a save button in a toolbar detached from the form it commits.
- **Don't** add shadows, 16px radii, or floating white cards to page content.
- **Don't** introduce a second font, a hosted font, or a dark theme; the app ships offline under a strict CSP.
- **Don't** use glyphs, emoji or `<img>` sprites as icons.
- **Don't** centre the About lockup or give it a card; the mark and name sit left-aligned at the page margin.
- **Don't** style selection with a decorative side rule, a left border stripe, or a coloured row number.
- **Don't** ship a layout below the 940px window floor; there is no phone class for this product.
