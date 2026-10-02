# ket components

The spec every piece of ket's UI is held to. Its picture is
[`style-guide.html`](style-guide.html) — open it in a browser — and its
working copy is the Components page of the design canvas the Desk redesign
came from. When the two disagree with the code, this file is what the code
is moving towards; the [gaps](#where-the-code-is-not-there-yet) are listed at
the end.

Build from `crates/ket-ui/src/ui/`, never a one-off `div` that looks like a
control (see `AGENTS.md`). This file says what those recipes should look
like; it does not license new ones.

## Foundations

### Grounds

The window is cards on a desk. Each step is visible from the next, and a
well must never equal the card it sits in: it is a fill, not a border.

| Name     | Token              | Desk      | Use                                             |
| -------- | ------------------ | --------- | ----------------------------------------------- |
| Desk     | `backdrop`         | `#050607` | Window ground, the gutters between cards        |
| Card     | `surface`, `panel` | `#0d0f12` | Header, sidebar, panes, right panel             |
| Well     | `sunken`           | `#14171b` | Fields, segmented tracks, tab track             |
| Float    | `elevated`         | `#181b20` | Menus, popovers, dialogs, toasts, tooltips      |
| Hover    | `hover`            | `#171a1f` | Pointer over a row or control                   |
| Selected | `selection`        | `#22262c` | Chosen segment, active tab, pressed icon button |
| Border   | `border`           | `#262a31` | Control edges, float edges                      |
| Rule     | `rule`             | `#1a1d22` | Card edges, hairlines inside a card             |

### Ink and signals

| Name        | Token              | Desk      | Use                                           |
| ----------- | ------------------ | --------- | --------------------------------------------- |
| Ink         | `text.primary`     | `#e7e9ec` | Everything read first                         |
| Dim ink     | `text.dim`         | `#8b929b` | Labels, detail lines, captions                |
| Placeholder | —                  | `#6b727c` | Empty fields only                             |
| Accent      | `accent`           | `#eceef1` | Primary button fill, focus ring, switches on  |
| Marker      | `marker` (new)     | `#f59e0b` | The selected row's rail. Nothing else         |
| Brand       | —                  | `#7ee08a` | The ket mark                                  |
| Running     | `status.running`   | `#4cd08a` | An agent at work                              |
| Waiting     | `status.attention` | `#7aa7ff` | Blocked on a person                           |
| Failed      | `status.failed`    | `#ef6a74` | Ended badly; danger actions                   |
| Merging     | `status.merging`   | `#b8a7e0` | A merge or rebase in flight                   |
| Override    | `terminal.ansi.cyan` | `#3fd2e0` | A project launching an agent with its own command (`agent_override`) |
| Added       | `diff.added`       | `#4cd08a` | `+` counts, `A` / `U`                         |
| Removed     | `diff.removed`     | `#ef6a74` | `−` counts, `D`                               |
| Modified    | `diff.modified`    | `#f5c451` | `M` / `R`, the dot on a changed folder        |

Colour is spent on state, never decoration. Colours that must be told apart
also differ in lightness. Waiting is blue so it never reads as the marker.

### Type

Geist for words, Geist Mono for anything you would quote — branches, paths,
figures, keys. Sentence case everywhere; no tracked uppercase labels.

| Role       | Face       | Size / weight | Example                         |
| ---------- | ---------- | ------------- | ------------------------------- |
| Value      | Geist Mono | 15 / 500      | `41%`, `+128 −14`, a branch     |
| Title      | Geist      | 15 / 600      | Dialog and popover titles       |
| Control    | Geist      | 13 / 500      | Button and segment labels       |
| Body       | Geist      | 13 / 400      | Dialog copy, field text         |
| Identifier | Geist Mono | 13.5 / 400    | Worktree branch in a row        |
| Label      | Geist      | 12.5 / 500    | Field labels, figure labels     |
| Secondary  | Geist      | 12 / 400      | A row's detail line, help text  |
| Caption    | Geist Mono | 11 / 400      | Key hints, resets, counts       |
| Code       | Monaspace Neon | 13 / 400  | Terminal and editor only        |

`fonts.rs` owns the families; reach prose through `.prose()`.

### Shape and size

| Radius | px | Constant      | Use                               |
| ------ | -- | ------------- | --------------------------------- |
| Chip   | 5  | `RADIUS_SM`   | Segments, tabs, badges, kbd       |
| Control| 7  | `RADIUS_MD`   | Buttons, fields, tracks           |
| Row    | 8  | —             | List rows                         |
| Float  | 10 | `RADIUS_LG`   | Menus, popovers, dialogs          |
| Card   | 12 | `CARD_RADIUS` | Cards                             |

| Height | px | Constant     |
| ------ | -- | ------------ |
| Control (button, icon well) | 32 | `CONTROL_H`, `WELL` |
| Field                       | 36 | `FIELD_H`           |
| Menu row / tall row         | 28 / 38 | `menu.rs`      |
| Tab strip, panel strip      | 46 | `TOP_STRIP`         |
| Worktree row                | 52 | —                   |
| Header                      | 56 | `header::HEADER_H`  |

Gutter between cards: 10 (`header::GUTTER`). Inside a card: 8 to 12.

### Icons

16px, 1.5 stroke, round caps and joins, drawn in the ink of the text beside
them. Never emoji, never filled glyphs for chrome.

## Buttons — `ui::button`

- **Primary** — accent fill, `on_accent` ink, semibold. One per surface.
- **Secondary** — transparent, `border` edge. Everything else.
- **Ghost** — transparent, dim ink, no edge. Toolbars and footers.
- **Danger** — failed ink and a 45% failed edge. Never the default focus.
- States: hover is `hover` fill (`accent_hover` for primary); disabled keeps
  its shape at 55% ink. A key hint trails the label in Geist Mono 11.
- `.detail()` names what the action acts on — the branch a Start makes — in
  Geist Mono 11 after the label, cut short past 180.
- Sizes: default 32 / 13px; small 26 / 12px; `.w_full()` for a block button.

`icon_button` for a glyph alone: a 32 well in toolbars, `.bare()` inside rows
and headers, `.small()` 28, `.dense()` 24. Pressed is the `selection` fill.
`.indicator()` lights a 3px LED — running when on, border when off. Every
icon-only button has a tooltip.

`ui::cluster::icon_cluster` puts related glyph buttons in one frame with
touching cells and hairlines between them. `.small()` is the compact
header's size: 24px dense cells, 28 wide.

## Inputs — `ui::field`

- A `sunken` well, 36 tall, 7 corner, 10 inset, 13px Geist.
- No edge at rest. Hover: dim ink at 28%. Focus: `focus_ring` plus a soft
  3px halo. Invalid: failed edge and an error line below.
- Label above in dim 12.5/500; help and errors below in 12px.
- A leading icon is dim; a trailing key hint is a `kbd` chip.
- **Textarea** (`ui::textarea`) — the same well and focus, 1.55 line
  height, a footer row (`.footer()`) under a hairline holding the count, the
  send chord and the action.
- **Switch** — on is an accent track; off is a well with a border.
- **Checkbox** — 16px, chip corner; on is accent with a dark check.

## Dropdowns and menus — `ui::menu`

One menu for everything: a float at 10 corner, 5 padding, two-layer shadow.

- Filter row at the top once a menu passes eight items.
- 20px icon column, so titles align with or without icons.
- Rows 28 (tall 38 with a subtitle), 5 corner, 12.5px Geist.
- Chord right-aligned in Geist Mono 10.5, dim.
- Hairline separators between sections; a dim heading when a section needs
  a name (`MenuEntry::Heading`).
- `.wells()` sets each icon in a 20px well — sunken behind a glyph, a
  provider mark's colour at 14% behind a mark. For short launcher menus
  (the `+` menu); context menus keep bare glyphs.
- Every row of a launcher menu carries a chord the shell answers with the
  menu shut.
- Hover and keyboard selection share the `selection` fill. Checked items carry
  a check at the far end. Danger is red ink, never a red fill. Disabled items
  stay in place at half ink.

A **select** (`ui::select`) is a field-shaped trigger (well, value, chevron)
over the same menu, hung 6px below it; the chevron flips while open. In a
sentence — "Send to [ket ▾] in [main ▾]" — it is `.inline()`: 26 tall, a
border edge, 5 corner, as wide as its value. `.trailing(..)` puts a note between
the value and the chevron — the command a project's own override will run.

## Button groups and tabs — `ui::group`

- The track is a `sunken` well, 3px inset, 7 corner, no edge.
- The chosen segment is raised with the `selection` fill; others hover to
  `hover`.
- A setting's group hugs its answers. A view switch or filter uses `.fill()`
  and shares the width.
- A count rides in the label in Geist Mono 11, a step under dim, and
  brightens with its segment.
- Popup headers use the 18px size.
- A group sharing its row with other controls is `.dense()`. A segment can
  be a `.glyph()` alone or a status `.dot()` and its count, with no word;
  such a segment names itself in a tooltip hung on it with `.wrap()`.

The **tab strip** is the same track: tabs are pills, the active one raised.
The agent's state lives in the tab's mark, not in a line. The close mark and
the unsaved dot share one 16px cell and swap on hover so labels never shift.

## Containers

Four surfaces, chosen by what a thing is, not how loud it should be.

- **Desk** — `backdrop`, 10px gutters. The footer sits on it.
- **Card** — `panel`, `rule` edge, 12 corner (`header::card`). Every region
  of the window.
- **Well** — `sunken`, 7 corner, set into a card.
- **Float** — `elevated`, `border` edge, 10 corner, two-layer shadow.
  - *Popover* (`ui::popup`) — anchored to its trigger.
  - *Dialog* (`ui::dialog`) — centred over a scrim; the only float with one,
    and only one at a time. Title 15/600, body 13 dim, actions right-aligned.
  - *Toast* (`ui::toast`) — top right, stacked; icon in a tinted well.
  - *Tooltip* (`ui::tooltip`) — 7 corner, 11.5px, on every glyph-only button.
- **Banner** (`ui::banner`) — its signal's tint at 8%, edge at 25%, an icon,
  one line and one action.

## Rows and read-outs

- **Worktree row** — 52 tall, 8 corner, no hairline. Agent mark, branch in
  Geist Mono 13.5, detail in Geist 12 dim, state right-aligned in mono 11.5
  with its dot (`running`, `waiting`, `merging`, `failed`, or dim `idle`).
  Selected: `hover` fill plus a 3px `marker` rail inset 10 top and bottom.
- **Project heading** — 20px badge (initial in Geist Mono semibold), name in
  Geist 13.5, count in mono, chevron at the end.
- **File tree row** — 28 tall, Geist Mono 12.5. The git letter sits at the far
  end in its signal colour (`A U` added, `M R` modified, `D !` removed); a
  folder with changes under it gets a dot.
- **Figure** — label over value: dim Geist 12.5 over Geist Mono 15. A quota bar
  is 56 × 4 with round ends and takes the ramp colour with the figure.
- **Progress** (`ui::progress`) — every bar that reports a share: a `border`
  track, an `accent` fill (or the quota ramp), round ends at any height, a
  fill never narrower than it is tall. `.width()` inside a figure, `.grow()`
  in a row's free column, full width otherwise. Not for sliders.
- **Chips** — `kbd` (mono 11, border, 5 corner), `badge`, `tag` (well fill),
  `tinted_tag` (signal at 10%). `key_hint` is a `kbd` and its word — `↵ save`
  — for the strip of keys under a surface.
- **Read-out** (`chip::readout`) — a figure in a small recessed well, 22
  tall, 5 corner, Geist Mono 12, usually led by a glyph. A line of them is the
  compact header: provider mark + figure + 32 × 4 bar + reset; branch glyph +
  branch; `+128 −14`; status dot + "1 running".

## Where the code is not there yet

As of 2026-09-26: nothing outstanding here.
