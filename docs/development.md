<!-- nav:start -->
<sub>[🧭 Wayfinder](../README.md) · [Using it](using.md) · [Widgets](widgets.md) · [Visualizer](visualizer.md) · [Make it yours](customizing.md) · [Workspaces](workspaces.md) · [Plugins](plugins.md) · [Write a widget](writing-widgets.md) · [How it works](architecture.md) · **Development**</sub>
<!-- nav:end -->

# 🛠️ Development

Building, testing and extending Wayfinder, and what has and hasn't been verified yet.

## Status

**Alpha.** It works and is tested, but it is young.

<table>
<tr>
<td width="50%" valign="top">

**✅ Verified**

- 331 unit tests (`cargo test --lib`), pure logic, no GPU
- Every built-in widget, at each size it allows, checked to build and fit without text cut off
- Every widget and the settings window rendered offscreen
- A 42-check scripted run of the live app on the **software** renderer: drag, live resize, undo, saving, folder expand, hot reload with error cards, the settings commands, Show Desktop against a stand-in host window, and moving a widget behind the desktop icons and back

</td>
<td width="50%" valign="top">

**⚠️ Not verified yet**

- The windowed app on a real GPU after the swapchain fix ([ADR-006](adr/0006-swapchain-resize-and-gpu-loss.md))
- Show Desktop against the real Explorer (`Win` + `D`)
- Widgets behind the icons drawing on a real desktop, on 24H2 and before, and through an Explorer restart ([ADR-0013](adr/0013-behind-icons-by-reparenting.md))
- Workspaces on a real desktop: reading virtual desktops, the tray submenu, docking
- The audio visualizer with real music: capture, the oscilloscope's trigger and scale
- Fullscreen games, mixed DPI, monitor hot-unplug
- Vulkan and OpenGL (`WAYFINDER_BACKEND`) are for experiments only

</td>
</tr>
</table>

**Ideas, not promises:** a plugin catalogue to browse and install from, shader widgets.

## Build and test

```powershell
cargo test --lib                     # 331 unit tests, pure logic, no GPU
cargo run --release -- --selftest --gpu software --data $env:TEMP\wf-test
```

Offscreen renders, on the software adapter by default:

```powershell
cargo run --release --example render_widgets  -- docs\img\widgets.png
cargo run --release --example render_settings -- docs\img
```

## Extend it

Each kind of extension is one file plus one line of registration:

| To add | Write | Register |
|---|---|---|
| a built-in TOML widget | `assets/widgets/<id>.toml` | nothing: `build.rs` finds it |
| a Rust widget | `src/widgets/<id>.rs` implementing `Widget` (the Drawer is the example) | `registry.rs` |
| a data source | `src/data/<name>.rs` implementing `DataSource`, with the cadence of each field | `DataSources::builtin` |
| a data source in your own build | a `DataSource` in a crate that depends on `wayfinder` | `Options.extra_sources` before `wayfinder::run` |
| an element kind | `src/elements/<name>.rs` with a `KIND` and, if it draws, a `Shape` | `elements::KINDS` |
| a Workspace-wide style switch | a `Flag` variant and its `Workspace` field | a `flag_row` in Settings |
| widgets and themes to share | a folder with `plugin.toml` in `<data>/plugins/` (see [`assets/guides/PLUGINS.md`](../assets/guides/PLUGINS.md)) | nothing: it loads as you save |
| live data from a plugin | a Rust crate on [`wayfinder-plugin`](../sdk/README.md), built for `wasm32-unknown-unknown` | a `[code]` table in its `plugin.toml` |

**Your own build.** The engine is a library: an app can add native data sources and ship its own exe. A source says it changed through the `Notifier` it is given in `attach`, handles `on_click = "media.play_pause"` in `act`, saves a widget's setting with `Notifier::set_param`, frees per-widget state in `retain`, and gives a `cadence(field, cx)` that may follow its state (`Second` while playing, `None` while paused):

```rust
fn main() {
    let mut o = wayfinder::Options::from_args();
    o.extra_sources.push(Box::new(media::Media::default()));
    wayfinder::run(o);
}
```

**Command line for authors.** These print to the console and exit. Wayfinder is a Windows app, so pipe its output (`| Out-Host`) for the shell to wait for it and set `$LASTEXITCODE`:

```powershell
wayfinder --render-widget sunset_weather --png out.png --size 300x200 --param city=Oslo --time 15:42   # also a path to a .toml
wayfinder plugin pack plugins\sunset            # sunset.wfplugin next to the folder, checked
wayfinder plugin check sunset.wfplugin          # 0 when it installs and loads cleanly, 1 with problems
```

`--render-widget` renders through the real pipeline on the software adapter, with plugin code running (`--wait` seconds for its first answer), so CI can check widgets without a desktop. `wayfinder::plugins::{describe, check, pack}` do the same from Rust; `describe` is a stable API.

## Commit messages

Conventional Commits, Angular style: `type(scope): short summary`, imperative, lower case, no trailing period, under ~50 characters. Types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`. Keep the body to a line or two, only when the why isn't obvious. Example: `feat(drawer): add blur and tint options`.

> [!WARNING]
> `examples/phase0_spike.rs` is kept only as the record of the early measurements. **Do not run it**: it stress-tests multi-window swapchains and, together with the bug fixed in ADR-006, crashed an AMD driver during development.

<!-- pager:start -->
<br>

---

<table width="100%"><tr><td><a href="architecture.md">← ⚙️ How it works</a></td><td align="right"><a href="../README.md">Back to Wayfinder →</a></td></tr></table>
<!-- pager:end -->
