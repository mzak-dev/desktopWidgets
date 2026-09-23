# Making Wayfinder themes

A theme has three parts, and you pick each one on its own in **Settings → Appearance** (left-click the tray icon):

| Part | Folder | What it sets |
|---|---|---|
| Palette | `palettes/*.toml` | colours |
| Font set | `fonts/*.toml` | typefaces |
| Glyph set | `glyphs/*.toml` | icons on buttons and chrome |
| Icon pack | `iconpacks/<name>/` | app icons (PNG files, no TOML) |

Wayfinder watches this folder. When you save a `.toml` file, it reloads a moment later. If a file fails to load, the reason is written to `wayfinder.log`.

**With Claude Code:** open a terminal in this folder, run `claude`, and ask for what you want ("a warm sunset palette", "a light version of Aurora"). The `wayfinder-theme` skill in `.claude/skills/` tells it how.

## Every file

```toml
name = "Sunset"        # shown in Settings; must be unique
[tokens]
accent = "#ff8a3dff"
```

- The file name does not matter, but it must end in `.toml`.
- A file with the `name` of a built-in replaces that built-in.
- Built-ins: palettes **Midnight**, **Daylight**, **Aurora**, **Graphite**. Font sets **System**, **Editorial**, **Technical**. Glyph sets **Fluent**, **MDL2**, **Symbols**.
- Put top-level keys (`name`, `files`) **above** `[tokens]`. Anything below that line belongs to the tokens table.

## Palette

A complete palette (this is Midnight). Copy it, rename it and change the values:

```toml
name = "My Palette"
[tokens]
surface = "#161c2ce6"
surface-2 = "#0e1320e6"
surface-hover = "#ffffff1a"
face = "#ffffff12"
border = "#ffffff2e"
text = "#f2f5ffff"
text-dim = "#a9b4d0ff"
accent = "#6ea8ffff"
accent-text = "#0b1020ff"
danger = "#ff6b6bff"
shadow = "#00000080"
scrim = "#05070dcc"
track = "#ffffff1f"
hand = "#f2f5ffff"
tick = "#f2f5ff99"
```

| Token | Used for |
|---|---|
| `surface`, `surface-2` | card background, a top-to-bottom gradient; Settings window |
| `surface-hover` | hover highlight on rows and icons |
| `face` | clock face, folder tile |
| `border` | outlines and dividers |
| `text` | main text |
| `text-dim` | labels, secondary text |
| `accent` | selection, primary buttons, toggles, progress arcs |
| `accent-text` | text and icons drawn on `accent` |
| `danger` | delete buttons, errors |
| `shadow` | drop shadow under each card |
| `scrim` | dimmed backdrop (not drawn yet; define it anyway) |
| `track` | the empty part of sliders, toggles and arcs |
| `hand` | clock hands |
| `tick` | clock tick marks |

**Colours** are `#rgb`, `#rrggbb` or `#rrggbbaa` (alpha last, `ff` = opaque), or `transparent`, `white`, `black`. Anything else is drawn in magenta, which means "fix me".

Tips:

- Widgets are truly transparent windows. Surfaces at alpha `e6` to `f0` let a little of the wallpaper through; `ff` is fully solid.
- Keep `text` on `surface` at a contrast ratio of at least 4.5:1, and `text-dim` at 3:1 or more. `accent-text` must be readable on `accent`.
- Light palettes need a lighter shadow: Daylight uses `#2a3a5a4d`.
- A token you leave out falls back to Midnight's value, not magenta. Define all of them anyway, so the palette looks the same whatever changes in Midnight.

## Sizes

Every theme inherits these numbers. Any palette, font set or glyph set can change them in its `[tokens]`, for example `radius-lg = 12` for squarer cards. They are numbers, not strings.

| Token | Default |
|---|---|
| `radius-sm`, `radius-md`, `radius-lg` | 8, 14, 22 |
| `space-1` … `space-4` | 4, 8, 12, 16 |
| `font-size-sm`, `-md`, `-lg`, `-xl`, `-2xl` | 12, 14, 18, 28, 48 |
| `stroke` | 1 |
| `gutter` | 20 (room for the shadow around each card; best left alone) |

## Font set

```toml
name = "Rounded"
files = ["Nunito-Regular.ttf", "Nunito-Bold.ttf"]   # optional, paths relative to fonts/
[tokens]
font-body = "Nunito"
font-display = "Nunito"
font-mono = "Cascadia Mono"
```

- Values are **font family names**, as Windows lists them in Settings → Fonts, not file names.
- Any font installed in Windows works.
- For your own font, copy the `.ttf` / `.otf` into `fonts/` and list it in `files`. Listed files load as soon as you select the set; other font files in `fonts/` load when Wayfinder starts.

## Glyph set

`font-glyph` names the icon font. Each `glyph-*` token is one character, usually written as a `\uXXXX` escape. Define all 16:

```toml
name = "My Glyphs"
[tokens]
font-glyph = "Segoe Fluent Icons"
glyph-gear = ""
glyph-close = ""
glyph-minimize = ""
glyph-chevron-down = ""
glyph-chevron-right = ""
glyph-check = ""
glyph-add = ""
glyph-delete = ""
glyph-folder = ""
glyph-edit = ""
glyph-refresh = ""
glyph-move = ""
glyph-palette = ""
glyph-widgets = ""
glyph-info = ""
glyph-launch = ""
```

## Icon pack

Make a folder `iconpacks/<Pack Name>/` and put PNG files in it, named after the app's file, in lower case: `chrome.png` covers `Chrome.lnk` and `chrome.exe`. Use square images of 64 to 256 px with transparency. Apps without a PNG keep their own icon. The folder name appears in the Icon pack list.

## Your own tokens

Widget files (`widgets/*.toml`) refer to any token with `$name`, for example `color = "$accent"`. You can add tokens of your own, such as `glow = "#ffb86bff"` in a palette, and use `$glow` in your widgets. A palette that doesn't define it shows magenta there and a warning in the log.

## When it doesn't look right

- **The palette isn't listed:** the file failed to load. Read `wayfinder.log`.
- **The accent or roundness ignores your palette:** Settings → Appearance → *Accent colour* and *Card roundness* are overrides, and they win over every palette. Click **Clear all overrides**.
- **Something is magenta:** a colour value is invalid, or a widget uses a token the palette doesn't define.
- **Switching themes:** use Settings. `workspace.json` belongs to the running app, and it overwrites edits made while it runs.
