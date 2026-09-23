---
name: wayfinder-theme
description: Create or change Wayfinder desktop widget themes (palettes, font sets, glyph sets, icon packs) in the Wayfinder data folder. Use when the user asks for a new theme, colour scheme, palette, light or dark variant, fonts or icons for their widgets, or wants to tweak an existing one.
---

# Wayfinder themes

The working directory is Wayfinder's data folder (`%APPDATA%\Wayfinder`). Wayfinder reloads themes by itself as soon as a file is saved.

1. Read `THEMES.md` in this folder. It holds the file format, every token and a complete example of each part.
2. List `palettes/`, `fonts/` and `glyphs/` to see what the user already has. Most requests only need a palette.
3. Write one file per part, e.g. `palettes/sunset.toml`, starting from the complete example in `THEMES.md`. Give it a `name` no other file uses. Reusing a built-in's name replaces that built-in, so only do it when asked.
4. Define every token of that part. Colours are `#rrggbbaa`. Keep surfaces slightly translucent, `text` on `surface` at a contrast ratio of 4.5:1 or more, and `accent-text` readable on `accent`.
5. For a custom font, copy the `.ttf` / `.otf` into `fonts/` and list it in `files` above `[tokens]`. Use the family name inside the font, not the file name.
6. Tell the user to pick it in Settings → Appearance (left-click the tray icon). If it isn't listed, read `wayfinder.log`. If the accent looks wrong, a Style setting is winning: **Reset all style** in Settings → Appearance, or **Reset style** on that widget.

Do not edit `workspace.json`. The running app owns it and overwrites edits.
