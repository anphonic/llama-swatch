# Skins and Mini Layout — Design

Date: 2026-10-15

## Purpose

Let people change how Llama Swatch looks without making the app heavier for anyone who doesn't.
A skin picks a colour palette and a layout. One layout, **Mini**, shrinks the window to a small
frameless strip that shows only which model is loaded and whether it is working.

### Success criteria

- A fresh install, or an existing user who never opens the skin picker, sees no change: the default
  skin **Standard** is today's look and follows the system light/dark setting.
- Built-in skins can be picked in Settings and preview live; Cancel reverts.
- Skin files dropped into the skins folder appear in the picker; broken files are listed as skipped
  with a reason and never break the app.
- Choosing a Mini skin turns the main window into a strip; switching back restores the previous
  size and position.
- With several models loaded, Mini shows the busiest one plus a count, or cycles through them when
  that setting is on.

### Out of scope

- Fonts, sizes, spacing or density options (the existing compact-rows toggle is unchanged).
- Raw CSS, scripts, images or any other skin content beyond colour values and a layout name.
- A separate widget window. Mini is the main window in a different layout.
- Downloading or sharing skins from inside the app.

## Concepts

- **Palette**: values for the 12 colour tokens in `src/styles.css` (`bg`, `panel`, `text`, `muted`,
  `border`, `track`, `grey`, `green`, `amber`, `amberText`, `blue`, `red`), with a light and a dark
  variant.
- **Layout**: `full` (today's dashboard) or `mini` (the strip). Only the app defines layouts.
- **Skin** = name + palette + layout.
- **Theme** (existing System/Light/Dark setting) chooses which half of the palette is used. "System"
  stays the default, so every skin follows the OS unless Light or Dark is forced.

## Skins

### Built-ins (compiled into the app)

| id | Name | Layout | Palette |
|---|---|---|---|
| `builtin:standard` | Standard (default) | full | today's colours |
| `builtin:high-contrast` | High contrast | full | stronger text/border contrast |
| `builtin:muted` | Muted | full | low-saturation, Nord-style |
| `builtin:minimal` | Minimal | mini | Standard |

### Skin files

Location: `<app config dir>/skins/*.json`. Example:

```json
{
  "name": "Night Owl Mini",
  "layout": "mini",
  "light": { "bg": "#f6f7f9", "panel": "#ffffff", "text": "#1b1f24" },
  "dark":  { "bg": "#011627", "panel": "#0b2942", "text": "#d6deeb" }
}
```

Rules, enforced in Rust before anything reaches the webview:

- Files over 16 KB are skipped.
- `name` is required, trimmed, at most 40 characters.
- `layout` is `full` or `mini`; absent means `full`; any other value skips the file.
- Each colour must match `#rgb` or `#rrggbb`. Anything else (including `url(...)`, named colours,
  `var(...)`) skips the file with the offending key in the reason.
- Missing colours fall back to Standard's value for that variant.
- A missing `dark` block reuses the skin's `light` colours.
- Unknown keys are ignored so later versions can add fields.
- id is `file:<filename>`; ids are stable across rescans.

### Loading

- The backend scans the folder at startup and when Settings asks (open or **Rescan**).
- The result is a list of valid skins plus a list of `{file, reason}` skips. A missing folder is an
  empty list, not an error; an unreadable folder yields built-ins only and a folder error message.
- If the saved skin id is not found, the app uses Standard and reports
  "Skin `<file>` not found, using Standard".

### Applying

- `theme.ts` sets the 12 CSS variables on `:root` from the resolved palette variant and sets
  `data-layout="mini"` when the skin's layout is `mini`.
- The `@media (prefers-color-scheme)` rules in `styles.css` stay as the Standard fallback, so the
  window is styled before the first skin is applied.

## Mini layout

### Contents

One line, about 300×36 px:

```
[icon]  qwen3-30b  ● streaming  +1        [–] [×]
```

- **Model**: the busiest loaded model. Ranking: stalled > busy (streaming or waiting) > loading >
  idle/loaded; ties go to the most recent request. Computed in Rust as a `mini` field on the
  snapshot so it is unit tested.
- **Status**: the dot colour and word the dashboard cards use.
- **+N**: other loaded models; the tooltip lists them.
- **Cycle** (setting `miniCycle`, off by default): rotate through loaded models every 4 s, in the
  same ranked order. While any model is stalled, rotation stops on it.
- Nothing loaded: "No model loaded". Connection problems: the dashboard's banner wording, in its
  colour.

### Window handling

- Entering Mini: save the window's size and position as `fullBounds`, `setDecorations(false)`,
  resize to the strip keeping the top-left corner fixed.
- Leaving Mini: `setDecorations(true)`, restore `fullBounds`.
- On restart the app opens in the layout of the saved skin; `fullBounds` persists.
- If `fullBounds` is off every monitor, restore the default centred size instead.
- Drag anywhere on the strip except the buttons (`data-tauri-drag-region`).
- **–** minimizes; **×** closes the app, the same as the OS buttons.
- Double-click or right-click opens a small menu: **Full dashboard** (switches to the last
  full-layout skin used, Standard if none) and **Settings…**.
- Always-on-top stays its own setting and is not forced.

### Permissions

Add to `src-tauri/capabilities/default.json`: `core:window:allow-set-decorations`,
`allow-set-size`, `allow-set-position`, `allow-start-dragging`, `allow-minimize`, `allow-close`.
No `opener` permission and no new network access.

## Settings

New section **Skin**, under Theme:

- Dropdown: built-ins, a separator, then file skins by name; Mini skins are tagged "(mini)".
- Picking a skin previews it live; Save keeps it, Cancel reverts (same draft pattern as the
  update-check answer).
- Skins folder: read-only path with **Copy**, and **Rescan**.
- Skipped files listed beneath, e.g. "`broken.json`: colour `bg` isn't a hex value".
- Checkbox "Cycle through loaded models in Mini".

New settings fields, all defaulted so older settings files load unchanged:

| field | type | default |
|---|---|---|
| `skin` | string id | `builtin:standard` |
| `lastFullSkin` | string id | `builtin:standard` |
| `miniCycle` | bool | `false` |
| `fullBounds` | `{x, y, width, height}` or null | null |

## Error handling

| Condition | Behaviour |
|---|---|
| Skins folder missing | Built-ins only, no message |
| Skins folder unreadable | Built-ins only, "Can't read the skins folder" in Settings |
| Bad skin file | Skipped, reason listed in Settings |
| Saved skin gone | Standard, note in Settings |
| `fullBounds` off-screen | Default centred size |

## Testing

Rust unit tests:

- skin parsing: valid file; partial colours fill from Standard; missing `dark` copies `light`;
  bad hex rejected; `url(...)` rejected; oversize skipped; unknown `layout` rejected; unknown keys
  ignored; name trimmed/limited; id round-trip.
- folder scan: missing folder is empty; mix of good and bad files.
- settings: old JSON without the new fields loads with defaults.
- Mini selection: busiest ranking, tie on most recent request, stalled always wins, `+N` count,
  nothing loaded.
- snapshot JSON shape test gains the `mini` key.

`npm run build` type-checks the TS side.

Manual checks, listed in each PR: Full → Mini → Full restores bounds; drag; minimize and close;
double-click and right-click menu; OS light/dark flips the palette; a broken skin file shows as
skipped; Cancel in Settings reverts a previewed skin.

## Delivery

Two PRs, each documented and reviewable on its own:

1. **Skins**: palettes, built-ins, skin files, scan and validation, Settings picker, live preview.
2. **Mini layout**: the `mini` snapshot field, the strip view, window handling, permissions, cycle
   setting. Builds on PR 1.
