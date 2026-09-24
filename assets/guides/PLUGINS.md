# Making Wayfinder plugins

A plugin packs widgets, palettes, font sets, glyph sets and icon packs into one thing people can install, switch off and remove in **Settings → Plugins**. It can also carry sandboxed code for live data (see **Code** below), but it never starts programs.

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
                   (also .jpg .gif .webp .bmp; GIFs play)
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
- **Allowed files:** `toml png jpg jpeg gif webp bmp ttf otf ttc otc md txt wasm`, plus `LICENSE`, `README` and `NOTICE`. Anything else stops the install.
- A widget's `launch` can never open a file inside `plugins/`.
- **Needs:** a widget that reads a data source another plugin or a Wayfinder build provides can say so with `needs = ["media"]` at its top. When it is missing, the widget, Settings and the Plugins page name it.

## Code

For live data (weather, feeds, prices, a to-do list), a plugin can carry a WebAssembly module written in Rust with the `wayfinder-plugin` crate. It serves a data source that the plugin's widgets bind to like any other. For several sources, write `[[code]]` once per module instead of `[code]`; they share the plugin's saved data.

```toml
[code]
module = "code/weather.wasm"          # inside the plugin
source = "weather"                    # widgets read {weather.temp}; on_click = "weather.refresh"
net = ["api.open-meteo.com"]          # the only hosts it may reach, over HTTPS

[code.initial]                        # shown until the first answer, with {weather.loading}
temp = 0
```

To read files, list the folders under your home folder it may read, and any params that hold a folder the user picks:

```toml
fs_read = ["~/.claude"]               # read-only, shown when installing
fs_read_params = ["folder"]           # the folder the user chose in this param, in Settings
                                      # or by dropping it on an on_drop = "param folder" element
```

A widget showing its values opens only `https://` links, unless the plugin lists more:

```toml
launch = ["vscode", "~/.claude"]      # vscode:// links; folders and documents in ~/.claude
```

The module runs sandboxed: it cannot write files or start programs, reads only the folders listed, reaches only the hosts listed, keeps up to 1 MB of saved data, and has a time limit on every call. `{weather.error}` holds its last error. The install prompt and the Plugins page show what it can reach. How to write and build one: the SDK's README, https://github.com/mzak-dev/desktopWidgets/tree/main/sdk.

## Try it, then share it

1. Put the folder in `plugins/` here. Wayfinder loads it as you save, like any other file, and **Settings → Plugins** shows it with any errors.
2. To share it, run `wayfinder plugin pack plugins\sunset | Out-Host` in PowerShell: it writes `sunset.wfplugin` next to the folder and checks it. Or zip the folder (right-click it → **Send to → Compressed (zipped) folder**) and rename the `.zip` to `.wfplugin`; `wayfinder plugin check sunset.wfplugin | Out-Host` then says whether it installs and loads cleanly.
3. Double-clicking a `.wfplugin`, dropping it on the Settings window, or **Install from file...** installs it. Installing one with the same `id` replaces the old version and keeps it switched on or off.

**With Claude Code:** run `claude` in this folder and ask for a plugin ("a plugin with a sunset palette and a matching clock"). The `wayfinder-plugin` skill in `.claude/skills/` tells it how.
