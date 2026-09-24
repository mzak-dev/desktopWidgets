---
name: wayfinder-plugin
description: Create, change or package a Wayfinder plugin (a shareable bundle of widgets, palettes, font sets, glyph sets and icon packs) in the Wayfinder data folder. Use when the user asks for a plugin, a theme pack to share, a .wfplugin file, or to bundle widgets and themes together.
---

# Wayfinder plugins

The working directory is Wayfinder's data folder (`%APPDATA%\Wayfinder`). Wayfinder reloads plugins in `plugins/` as soon as a file is saved.

1. Read `PLUGINS.md` in this folder for the layout, the manifest and the rules. For palettes, font sets, glyph sets and icon packs, read `THEMES.md` as well.
2. List `plugins/` to see what exists. To change a plugin, edit its folder there.
3. Create `plugins/<id>/plugin.toml` with `id`, `name`, `version`, `author` and `description`. The `id` must match the folder name and use only lower-case letters, digits, `-` and `_`.
4. Add the content in the plugin's own `widgets/`, `palettes/`, `fonts/`, `glyphs/` and `iconpacks/` folders. Widgets use the same TOML format as files in this folder's `widgets/`, if the user has any. Give new widgets distinctive file names; a file named like a built-in (`clock.toml`) replaces it.
5. Only use `toml png jpg jpeg gif webp bmp ttf otf ttc otc md txt wasm` files. Anything else stops the plugin from installing elsewhere.
6. For live data, the plugin can carry code: a Rust crate using `wayfinder-plugin`, built with `cargo build --release --target wasm32-unknown-unknown` (never a WASI target), its `.wasm` copied into the plugin and named in a `[code]` table. Read the `Code` section of `PLUGINS.md` and the SDK README it links. List every host the code calls in `net`.
7. Tell the user to open Settings → Plugins (left-click the tray icon) to see it, switch it on and read any errors. `wayfinder.log` has the details.
8. To share it: zip the plugin folder and rename the `.zip` to `<id>.wfplugin`. On Windows, PowerShell does it in one line: `Compress-Archive plugins\<id>\* <id>.zip; Rename-Item <id>.zip <id>.wfplugin`.

Do not edit `workspace.json`. The running app owns it and overwrites edits.
