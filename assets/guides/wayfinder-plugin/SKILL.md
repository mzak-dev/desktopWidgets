---
name: wayfinder-plugin
description: Create, change or package a Wayfinder plugin (a shareable bundle of widgets, palettes, font sets, glyph sets and icon packs) in the Wayfinder data folder. Use when the user asks for a plugin, a theme pack to share, a .wfplugin file, or to bundle widgets and themes together.
---

# Wayfinder plugins

The working directory is Wayfinder's data folder (`%APPDATA%\Wayfinder`). Wayfinder reloads plugins in `plugins/` as soon as a file is saved.

1. Read `PLUGINS.md` in this folder for the layout, the manifest and the rules. For palettes, font sets, glyph sets and icon packs, read `THEMES.md` as well.
2. List `plugins/` to see what exists. To change a plugin, edit its folder there.
3. Create `plugins/<id>/plugin.toml` with `id`, `name`, `version`, `author` and `description`. The `id` must match the folder name and use only lower-case letters, digits, `-` and `_`.
4. Add the content in the plugin's own `widgets/`, `palettes/`, `fonts/`, `glyphs/` and `iconpacks/` folders. Widgets use the same TOML format as files in this folder's `widgets/`, if the user has any. Give new widgets distinctive file names; a file named like a built-in (`clock.toml`) replaces it. If the user should be able to arrange a widget's parts in Settings, declare tiers, slots and modules (see the Modules section of `PLUGINS.md`).
5. Only use `toml png jpg jpeg gif webp bmp ttf otf ttc otc md txt wasm` files. Anything else stops the plugin from installing elsewhere.
6. For live data, the plugin can carry code: a Rust crate using `wayfinder-plugin`, built with `cargo build --release --target wasm32-unknown-unknown` (never a WASI target), its `.wasm` copied into the plugin and named in a `[code]` table. Read the `Code` section of `PLUGINS.md` and the SDK README it links. List every host the code calls in `net`, and every folder it reads in `fs_read` (`~/…`).
7. Tell the user to open Settings → Plugins (left-click the tray icon) to see it, switch it on and read any errors. `wayfinder.log` has the details.
8. Check your work without the desktop, and read `LOOK.md` in this folder first (`wayfinder scene guide` prints it): it is the loop. For one widget: `wayfinder scene dump --widget <widget> --content-root plugins\<id> --size 300x200 | Out-Host` prints every element's position, text and flags such as TRUNCATED; read that before you draw it with `wayfinder scene render --widget <widget> --content-root plugins\<id> --size 300x200 --dump --out look | Out-Host` and look at the PNG in `look\`. For several sizes and both Tiers, add a scene under `plugins\<id>\scenes\` (`LOOK.md` shows the format) and run `wayfinder scene check plugins\<id>\scenes | Out-Host` (exit 0 means no layout problems). After you change the widget, `wayfinder scene diff plugins\<id>\scenes --no-pixels | Out-Host` says what moved; `wayfinder scene bless plugins\<id>\scenes --reason "<what and why>" | Out-Host` accepts it, only when you meant it. Scenes render on the software adapter only; never open the app or use `--selftest` to look at a widget.
9. To share it: `wayfinder plugin pack plugins\<id> | Out-Host` writes `<id>.wfplugin` and reports any problem (`scenes\` and `.look\` stay out of the bundle). `wayfinder plugin check <file> | Out-Host` checks one. To see a widget the way it is installed here: `wayfinder --render-widget <id> --installed --png out.png --gpu software | Out-Host` (`--installed` reads the widgets and plugins installed here; add `--env fetch=real` if its code needs the network), then look at the image.

Do not edit `workspace.json`. The running app owns it and overwrites edits.
