<div align="center">

# 🧭 Wayfinder

### Desktop widgets for Windows, rebuilt for the GPU era

Transparent, GPU-composited widgets that live on your desktop.
Move and resize them in real time, restyle everything, and pay nothing while they sit idle.

<br>

[![Rust](https://img.shields.io/badge/Rust-2024_edition-DEA584?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org)
[![Windows](https://img.shields.io/badge/Windows-10_%2F_11-0078D4?style=for-the-badge&logo=windows11&logoColor=white)](#-quick-start)
[![wgpu](https://img.shields.io/badge/wgpu-30-6A4FFF?style=for-the-badge&logo=webgpu&logoColor=white)](https://wgpu.rs)
[![DirectX 12](https://img.shields.io/badge/DirectX_12-DirectComposition-107C10?style=for-the-badge&logo=xbox&logoColor=white)](docs/adr/0001-dx12-dcomp-presentation.md)

![Status](https://img.shields.io/badge/status-alpha-F5A623?style=flat-square)
![Tests](https://img.shields.io/badge/unit_tests-331_passing-2EA44F?style=flat-square)
![Idle](https://img.shields.io/badge/idle_CPU-0%25-2EA44F?style=flat-square)
![Widgets](https://img.shields.io/badge/built--in_widgets-13-8A63D2?style=flat-square)
![Palettes](https://img.shields.io/badge/palettes-7-3B82F6?style=flat-square)
[![License](https://img.shields.io/badge/license-MIT-2EA44F?style=flat-square)](LICENSE)

<br>

<img src="docs/img/hero.png" alt="A calendar, a media controller, a 3D audio visualizer, a system monitor, Claude Code sessions, an analog clock and a photo frame on a desktop" width="900">

<sub>Seven of the thirteen built-in widgets, drawn by the engine itself with sample data.</sub>

<br>

[Quick start](#-quick-start) ·
[Guides](#-guides) ·
[Widgets](docs/widgets.md) ·
[Visualizer](docs/visualizer.md) ·
[Make it yours](docs/customizing.md) ·
[Workspaces](docs/workspaces.md) ·
[Write a widget](docs/writing-widgets.md)

</div>

<br>

## ✨ Why Wayfinder

Rainmeter and the Windows Vista/7 sidebar showed how good widgets on a desktop can be. Wayfinder keeps that idea and moves it onto a modern GPU stack.

|  |  |
|---|---|
| 🪟 **Real transparency** | One borderless window per widget with true per-pixel alpha (DirectComposition), soft shadows and rounded corners. |
| ✋ **Edit in place** | Drag to move, drag an edge to resize; text and icons **reflow live**, with snapping to the grid, monitor edges and other widgets. [More](docs/using.md) |
| 📐 **Size-aware** | Widgets change *what* they show with their size: one line when low, the essentials when small, everything when large. [More](docs/widgets.md#one-widget-four-sizes) |
| 😴 **Free when idle** | A widget redraws only when something it shows *can* have changed. A desktop of clocks sits at 0% CPU. [How](docs/architecture.md) |
| 🎨 **Restyle everything** | Palettes, font sets, glyph sets and app icons, for every widget or just one. [More](docs/customizing.md) |
| 🗂️ **Workspaces** | Sets of widgets that come up by themselves on a virtual desktop or when you dock. [More](docs/workspaces.md) |
| 📄 **Widgets are text files** | A widget is TOML with bindings like `{clock.hour}`; save it and it reloads. Its options become a settings form. [More](docs/writing-widgets.md) |
| 🧩 **Plugins** | Share widgets and themes as one `.wfplugin` file, with sandboxed WebAssembly for live data. [More](docs/plugins.md) |
| 🛡️ **Hard to break** | Bindings cannot loop or hang, a broken widget file never crashes the desktop, and a GPU reset rebuilds the renderer. |

<br>

## 🚀 Quick start

You need Windows 10 or 11 and a [Rust toolchain](https://rustup.rs) (edition 2024).

```powershell
git clone https://github.com/mzak-dev/desktopWidgets.git
cd desktopWidgets
cargo build --release
.\target\release\wayfinder.exe
```

The first run puts four widgets on the right-hand side of your main monitor, clear of your desktop icons, and adds a **tray icon**. Left-click the tray icon for Settings; press **`Ctrl` + `Shift` + `E`** to move and resize widgets. [Using Wayfinder](docs/using.md) has the rest.

<details>
<summary><b>Try it without touching your GPU driver</b></summary>

<br>

Wayfinder can render on the CPU (the "Microsoft Basic Render Driver"). It is slower, but it never touches the vendor GPU driver, which makes it a good first run and the mode the automated tests use:

```powershell
.\target\release\wayfinder.exe --gpu software --data $env:TEMP\wayfinder-try
```

`--data` points it at a throwaway data folder so your real settings stay untouched.

</details>

<details>
<summary><b>Prefer an installer?</b></summary>

<br>

Each [release](https://github.com/mzak-dev/desktopWidgets/releases) also has a `Setup.exe`, built with [Velopack](https://velopack.io) ([ADR-012](docs/adr/0012-velopack-installer-and-autoupdate.md)). It installs to `%LocalAppData%\Wayfinder` with a Start Menu shortcut and an uninstaller, and the app checks for updates in the background and applies them silently the next time it starts — no prompts, nothing to run by hand. `%APPDATA%\Wayfinder` (your widgets, themes, settings) is untouched by installs or updates.

</details>

<br>

## 📚 Guides

<table>
<tr>
<td width="50%" valign="top">
<a href="docs/widgets.md"><img src="docs/img/sizes.png" alt="The calendar and media controller at four sizes" width="100%"></a>
<br><b><a href="docs/widgets.md">The widgets</a></b><br>
<sub>All thirteen, what each shows and what you can change, and how they adapt to four sizes.</sub>
</td>
<td width="50%" valign="top">
<a href="docs/visualizer.md"><img src="docs/img/visualizer.png" alt="The audio visualizer in eight styles" width="100%"></a>
<br><b><a href="docs/visualizer.md">The audio visualizer</a></b><br>
<sub>Eight styles, from bars to a 3D waterfall and an oscilloscope, and every option.</sub>
</td>
</tr>
<tr>
<td width="50%" valign="top">
<a href="docs/customizing.md"><img src="docs/img/palettes.png" alt="Widgets in seven palettes" width="100%"></a>
<br><b><a href="docs/customizing.md">Make it yours</a></b><br>
<sub>Palettes, fonts, icons and style: what you can change, where, and which setting wins.</sub>
</td>
<td width="50%" valign="top">
<a href="docs/workspaces.md"><img src="docs/img/settings_workspaces.png" alt="The Workspaces page in Settings" width="100%"></a>
<br><b><a href="docs/workspaces.md">Workspaces</a></b><br>
<sub>Sets of widgets tied to virtual desktops and monitor setups.</sub>
</td>
</tr>
</table>

| Guide | What's in it |
|---|---|
| 🖱️ [Using Wayfinder](docs/using.md) | Edit Mode and its keys, the Settings window, monitors coming and going, which GPU to use |
| 🧩 [The widgets](docs/widgets.md) | The thirteen built-in widgets and their options; one widget at four sizes |
| 🎚️ [The audio visualizer](docs/visualizer.md) | Its eight styles, how sound becomes a picture, every option |
| 🎨 [Make it yours](docs/customizing.md) | Palettes, font and glyph sets, style, which value wins, the data folder |
| 🗂️ [Workspaces](docs/workspaces.md) | Several sets of widgets, tied to virtual desktops and monitors |
| 🧩 [Plugins](docs/plugins.md) | Sharing widgets and themes as `.wfplugin` files, and plugin code for live data |
| ✍️ [Write a widget](docs/writing-widgets.md) | A widget file step by step, and the full reference: elements, attributes, data, expressions, options |
| ⚙️ [How it works](docs/architecture.md) | From TOML to pixels, why idle is free, where the code lives, the decisions behind it |
| 🛠️ [Development](docs/development.md) | Status, tests, offscreen renders, extending the engine, commit messages |

The words Wayfinder uses (Widget, Instance, Data Source, Workspace...) are defined in [CONTEXT.md](CONTEXT.md), and the decisions with their measurements are in [docs/adr](docs/adr).

<br>

## 📊 Status

**Alpha.** It works and is tested (331 unit tests, every widget rendered offscreen and checked to fit at each size), but it is young: some parts have not yet run on a real Windows desktop. See [what is and isn't verified](docs/development.md#status).

<br>

## 📜 License

Wayfinder is released under the [MIT License](LICENSE). You may use, copy, change and share it, in your own projects and commercially, as long as you **credit it**: keep the copyright notice and the license text with every copy or substantial part of it.

## 🙏 Credits

Inspired by [Rainmeter](https://www.rainmeter.net) and the Windows Vista and 7 sidebar gadgets. Built on
[wgpu](https://wgpu.rs), [winit](https://github.com/rust-windowing/winit), [taffy](https://github.com/DioxusLabs/taffy),
[glyphon](https://github.com/grovesNL/glyphon), [`windows`](https://github.com/microsoft/windows-rs),
[tray-icon](https://github.com/tauri-apps/tray-icon), [notify](https://github.com/notify-rs/notify),
[global-hotkey](https://github.com/tauri-apps/global-hotkey), [rustfft](https://github.com/ejmahler/RustFFT) and
[wasmi](https://github.com/wasmi-labs/wasmi).

<div align="center">
<br>
<sub>Copyright © 2026 Mateusz Żak</sub>
</div>
