# Making Wayfinder plugins

A plugin packs widgets, palettes, font sets, glyph sets and icon packs into one thing people can install, switch off and remove in **Settings → Plugins**. It holds content only: it never runs programs.

## Layout

A plugin is a folder laid out like this data folder, plus a `plugin.toml`:

```text
sunset/
├─ plugin.toml
├─ widgets/        *.toml   widgets (the file name is the widget's id)
├─ palettes/       *.toml   colour sets        ─┐
├─ fonts/          *.toml   font sets, and      │ see THEMES.md for
│                  .ttf / .otf files            │ every token
├─ glyphs/         *.toml   glyph sets          │
├─ iconpacks/<name>/ *.png  app icons          ─┘
└─ images/         *.png    anything your widgets show
```

Every folder is optional. A plugin with only `palettes/` is a theme pack.

## plugin.toml

```toml
id = "sunset"                  # lower-case letters, digits, - and _; also the installed folder's name
name = "Sunset"                # shown in Settings
version = "1.2.0"
author = "Your Name"
description = "Warm evening colours and a weather card."
homepage = "https://example.com"   # optional
wayfinder = "0.1"                  # optional: the oldest Wayfinder it works with
```

Unknown keys are refused, so a typo shows up at once.

## Rules worth knowing

- **Ids are shared.** A widget file named `clock.toml` replaces the built-in Analog Clock, which is how a plugin restyles one. Pick distinctive names (`sunset_weather.toml`) for new widgets.
- **Order:** built-ins, then plugins by id, then the user's own files. The user's `widgets/clock.toml` beats yours.
- **Images next to a widget:** `src = "./logo.png"` is a file in the widget's own folder, and `src = "./../images/logo.png"` reaches the plugin's `images/`. A path may not leave the plugin.
- **Fonts:** put `.ttf` / `.otf` files in `fonts/` and name the family (as it is inside the font) in a font set.
- **Allowed files:** `toml png ttf otf ttc otc md txt`, plus `LICENSE`, `README` and `NOTICE`. Anything else stops the install.
- A widget's `launch` can never open a file inside `plugins/`.

## Try it, then share it

1. Put the folder in `plugins/` here. Wayfinder loads it as you save, like any other file, and **Settings → Plugins** shows it with any errors.
2. To share it, zip the folder (right-click it → **Send to → Compressed (zipped) folder**) and rename the `.zip` to `.wfplugin`.
3. Double-clicking a `.wfplugin`, dropping it on the Settings window, or **Install from file...** installs it. Installing one with the same `id` replaces the old version and keeps it switched on or off.

**With Claude Code:** run `claude` in this folder and ask for a plugin ("a plugin with a sunset palette and a matching clock"). The `wayfinder-plugin` skill in `.claude/skills/` tells it how.
