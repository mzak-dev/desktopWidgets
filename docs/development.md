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

- 329 unit tests (`cargo test --lib`), pure logic, no GPU
- Every built-in widget, at each size it allows, checked to build and fit without text cut off
- Every widget and the settings window rendered offscreen
- A 35-check scripted run of the live app on the **software** renderer: drag, live resize, undo, saving, folder expand, hot reload with error cards, the settings commands, and Show Desktop against a stand-in host window

</td>
<td width="50%" valign="top">

**⚠️ Not verified yet**

- The windowed app on a real GPU after the swapchain fix ([ADR-006](adr/0006-swapchain-resize-and-gpu-loss.md))
- Show Desktop against the real Explorer (`Win` + `D`)
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
cargo test --lib                     # 329 unit tests, pure logic, no GPU
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
| a data source | `src/data/<name>.rs` implementing `DataSource`, with the cadence of each field | `DataSources::from` |
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
wayfinder --render-widget clock --png out.png --size 300x200 --now 2026-03-08T15:42 --env sys.cpu=90   # also a path to a .toml
wayfinder plugin pack plugins\sunset            # sunset.wfplugin next to the folder, checked
wayfinder plugin check sunset.wfplugin          # 0 when it installs and loads cleanly, 1 with problems
```

`--render-widget` renders through the real pipeline on the software adapter, so CI can check widgets without a desktop. `wayfinder::plugins::{describe, check, pack}` do the same from Rust; `describe` is a stable API.

A render is **hermetic by default**: a fixed date and time (Thursday 2026-01-15 10:10:30 UTC), example system readings, an example paused track, a steady tone, tile icons instead of the shell's, no network, an empty temporary data folder, only the built-in widgets (plus the folder of a `.toml` you name), the theme's `anim-speed`, and the software adapter (`--gpu high|low` uses a hardware one and marks the run not hermetic). It waits for plugin code and pictures to settle (`--wait` seconds per code source; a timeout is an error, never a half-loaded PNG), and the frame is drawn 2 s after the first, so an animation has finished. Change the environment with `--now <ISO>`, `--time HH:MM` (the time of day on the pinned date) and `--env key=value` (repeatable, for example `sys.cpu=90`, `media.playing=true`, `zone="Tokyo Standard Time"`, `anim=off`, `settle=500ms`, `fetch.responses=[{...}]`; an unknown key suggests the nearest). `--real clock,sys,media,audio,icons,fetch` takes a seam from your machine instead. Every render writes `out.png.env.json` beside the PNG: the pins, what was read from the machine, the content and font set, the adapter and whether the run was `hermetic`. Two hermetic renders on one machine are byte-identical; text is measured with the machine's own fonts, so a render is exact on the same machine and font set, not across machines. The pins, the record and these flags are tooling, not a frozen interface.

> [!NOTE]
> This changes what `--render-widget` did before. It no longer reads your installed plugins, your own widgets or the real `--data` folder: add `--installed` (with `--data <dir>` for a folder other than the default) to look a widget up by id among them, as before. It no longer uses today's date and your machine's readings, media, sound and icons: `--time` now sets the time of day on the pinned date, and `--real` brings the machine back. Plugin code gets no network unless you ask (`--env fetch=real`, or canned answers with `--env fetch.responses=[...]`). `--palette`, `--scale` and `--transparent` are the pins of the same names, and the theme's `anim-speed` now applies to a render as it does on the desktop. A render that reads the machine or installed content says so on its last line and in the `.env.json`.

`--content-root <plugin folder>` loads a plugin folder where it stands, its widgets and its code, without installing it; the folder is hashed into the record and the run stays hermetic. Pass the widget's id (`wayfinder --render-widget sunset --content-root D:\dev\sunset --png out.png`).

**Scenes** name a widget and the world it is shown in, and `wayfinder scene dump` describes the settled frame as text, with no GPU: one line per element with its rectangle, colours, text, the font the shaper used, hit actions and clips, then the hit regions, the draw counts and the layout flags. A scene is a `*.scene.toml` file under a `scenes/` folder (`format = 1`, a `[widget]` table, optional `[look]`, `[env]` with the same keys as `--env`, `[expect]` and `[sweep]`: `sizes = "tiers"` makes one scene per Tier, `"grid:4x4"`, `"max"` or a list of sizes the other fits):

```powershell
wayfinder scene list [set|glob] [--sets]                     # ids, from ./scenes or --root
wayfinder scene dump fixtures/clock --view full               # outline (default, 200 lines) | full | texts | hits | flags
wayfinder scene dump fixtures/monitor-tiers* --find CPU --format json
wayfinder scene dump --widget clock --under clock-1/0 --depth 2   # one widget, one subtree
wayfinder scene check fits                                    # layout flags and [expect] for a set: exit 1 on findings
wayfinder scene diff engine --no-pixels                       # each scene against its baseline: what moved, by node key
wayfinder scene bless engine --reason "wider card padding"    # replace baselines on purpose
```

`scene check` runs the scenes and reads only the **layout flags**: `TRUNCATED` (one-line text wider than its box), `CLIPPED`, `OUTSIDE` (past the card), `SQUASHED`, `OVERFLOW-X`/`-Y`, `ZERO`, `EMPTY-TEXT-BOX`, `NO-IMAGE` and `TOFU` (a glyph no installed font has), computed outside scroll containers and printed at the end of a node's line and in the `--- flags` trailer of every dump. A scene's `[expect] flags` is `error` (a finding, exit 1), `warn` (the default: shown, exit 0) or `ignore`, and `allow = ["OVERFLOW-Y"]` names flags the scene is known to have. The built-in widgets are checked at their default size and a 4x4 grid from their smallest to their largest, with and without every switch on, by the `fits` set (`scenes/fits/`), which `cargo test` runs too.

Each run writes `<scenes folder>\.look\summary.txt`, `last.json` and, per scene, `<id>.dump.txt` and `<id>.env.json`, so the result survives a lost console. Exit codes: 0 clean, 1 findings (an error card, a failed `[expect]`, an error-level flag, a code source that never answered), 2 bad arguments or scene file (an unknown key suggests the nearest), 3 the output could not be written. The scene file, the dump and these commands are tooling (`format = 1`), not a frozen interface. Text is measured with the machine's fonts, so a dump is exact on the same machine and font set; its header carries a hash of the font set to say which.

**Baselines** keep the blessed dump of each scene. `scene diff <set>` runs the scenes again and compares each dump with its baseline, naming every change by node key and class: `geometry` (rect, clip, scroll), `text` (string, size, weight, face), `colour`, `hit` (what a click does), `flag` (a layout flag that came or went), `structure` (a node added or removed) and `env` (the pins in the header). When a sibling inserted early renumbers a subtree, it is shown as one line diff instead of a wall of keys. `N scenes changed only in colour` stands for the scenes a palette tweak repaints. Exit 0 means every scene matches; a changed scene, a scene with no baseline and a flagged one exit 1. `--no-pixels` is what `diff` does today (the pixel tier is a later addition).

`scene bless <set> --reason "..."` re-runs the scenes and writes their baselines. The reason is required and goes to `bless.log` beside them. It refuses, and writes nothing, when any selected run is not hermetic (it read the machine), shows an error card, fails an `[expect]` or has an error-level flag; there is no environment variable that updates baselines and no way to override a scene's environment while blessing. `--new-only` writes only the scenes that have no baseline.

There are no bundled fonts, so a dump's text metrics are exact only on the machine and font set that made it. Baselines are therefore per machine: `scenes/baselines/<fonts>/...`, named for the font hash in the dump header, and ignored by git. `scene diff` looks only in the folder of its own font set; baselines made with other fonts are not compared (exit 3 and a message), because they would report text metrics as geometry changes. `scene bless` on a new machine records baselines for its fonts beside the others. A team whose machines share a font set can commit the folder of that hash: remove `scenes/baselines/` from `.gitignore` and commit `scenes/baselines/system-<hash>/`; a machine with other fonts then finds nothing under its own hash and says so. The set `scenes/engine/` holds the built-in widgets (86 scenes, about 0.5 MB of baselines): each widget at its smallest, default and largest size with defaults and with every switch on, `items = "demo"` where it lists shortcuts, the System Monitor at each Tier and with each Module put away. The widgets that read a file or a folder (`gif_player`, `photo_frame`, `photo_gallery`) are scenes of their empty state: a path is the machine's, so a run with one is not hermetic and cannot be blessed.

## Commit messages

Conventional Commits, Angular style: `type(scope): short summary`, imperative, lower case, no trailing period, under ~50 characters. Types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`. Keep the body to a line or two, only when the why isn't obvious. Example: `feat(drawer): add blur and tint options`.

> [!WARNING]
> `examples/phase0_spike.rs` is kept only as the record of the early measurements. **Do not run it**: it stress-tests multi-window swapchains and, together with the bug fixed in ADR-006, crashed an AMD driver during development.

<!-- pager:start -->
<br>

---

<table width="100%"><tr><td><a href="architecture.md">← ⚙️ How it works</a></td><td align="right"><a href="../README.md">Back to Wayfinder →</a></td></tr></table>
<!-- pager:end -->
