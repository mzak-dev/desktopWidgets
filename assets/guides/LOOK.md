# Looking at what you made

You changed a widget, a Module, a Tier or a theme. Do not open the desktop and do not ask for a screenshot: render it from the command line. The `wayfinder scene` commands draw a widget in a fixed world (a fixed date and time, example system readings, no network) and describe what they drew as text, so you can check the layout by reading.

`wayfinder` below is the exe (in a checkout of the engine: `cargo run --release --`). It is a Windows app, so pipe its output (`| Out-Host`) for the shell to wait for it, or read `.look\summary.txt` (below). Every command exits `0` when clean, `1` with findings, `2` for bad arguments or a bad scene file, `3` when it could not run or write.

**Safety.** Scene commands use the software adapter (WARP) only: they take no `--gpu` and never touch a hardware GPU. `scene dump`, `scene check` and `scene diff --no-pixels` create no graphics device at all. Never run the app, `--selftest` or `examples/phase0_spike` to look at something, and never pass anything but `--gpu software` to a command that takes `--gpu`.

**What is and is not promised.** Scene files, dumps, baselines and these commands are tooling: they carry `format = 1`, may change in a minor release and are not a frozen interface. Text is measured with the fonts installed on this machine, so a dump and a baseline are exact on the same machine and font set, not across machines. Baselines are per machine: they are kept under `scenes\baselines\<fonts hash>\` and a machine with other fonts does not compare against them.

## The loop

1. **Read, do not look.** The dump lists every element with its rectangle, clip, text, colours and hit actions, then the layout flags. Take a part of it: `--find` (the lines that contain a text), `--under` (one subtree), `--view flags` (only the problems). Start with the outline; it is capped at 200 lines.

   ```powershell
   wayfinder scene dump --widget digital_clock --find Thursday | Out-Host
   wayfinder scene dump card@1,1 --root plugins\sunset\scenes --under sunset-1/1 --depth 2 | Out-Host
   wayfinder scene dump card@0,0 --root plugins\sunset\scenes --view flags | Out-Host
   ```

2. **Check the layout.** `scene check` reads the layout flags: `TRUNCATED` (a line of text wider than its box), `CLIPPED`, `OUTSIDE` (past the card), `SQUASHED`, `OVERFLOW-X` and `-Y`, `ZERO`, `EMPTY-TEXT-BOX`, `NO-IMAGE`, `TOFU` (a glyph no installed font has). Exit `1` means one is an error in a scene's `[expect]`.

   ```powershell
   wayfinder scene check plugins\sunset\scenes | Out-Host
   ```

3. **See what moved.** After a change, `scene diff` runs the scenes again and names, per node, what moved, was reworded or changed colour. A scene without a baseline is a finding (`NEW`) so a forgotten `bless` cannot pass silently.

   ```powershell
   wayfinder scene diff plugins\sunset\scenes --no-pixels | Out-Host
   ```

4. **Look once, not many times.** Only when text is not enough (colours, images, how it feels), draw it: one contact sheet of several scenes, or the baseline, the new image and the difference side by side. Read the sheet; do not open one PNG per scene. These draw on the software adapter.

   ```powershell
   wayfinder scene sheet plugins\sunset\scenes --out sunset-sheet.png | Out-Host
   wayfinder scene sheet plugins\sunset\scenes --diff | Out-Host
   wayfinder scene render plugins\sunset\scenes --dump --out sunset-look | Out-Host
   ```

5. **Accept the change, only if you meant it.** `scene bless` writes the new baselines and records why. It refuses a run that is not hermetic, shows an error card, fails an `[expect]` or has an error-level flag. Never bless to make a diff go away, and never bless a scene you did not look at.

   ```powershell
   wayfinder scene bless plugins\sunset\scenes --reason "wider padding on the card" | Out-Host
   ```

6. **New widget, Tier or Module: add a scene in the same change**, then record it with `wayfinder scene bless <ids> --new-only --reason "<what it is>"`.

7. **A few stray pixels in a scene you did not touch?** Ask whether it is you or the environment before blaming the code: `scene selfcheck` renders the scenes again in fresh processes (a gap apart, on one CPU, at low priority) and says whether anything differed. Exit `0` means the render is deterministic here.

   ```powershell
   wayfinder scene selfcheck plugins\sunset\scenes --perturb | Out-Host
   ```

## Where output goes

Each run writes to `<scenes folder>\.look\` (`.look\` in the current folder for `--widget` without a scene file; ignored by git, never packed): `summary.txt` (one line per scene and the totals: what the command prints), `last.json`, and per scene `<id>.dump.txt` and `<id>.env.json` (what the run pinned and read from the machine). If the console shows nothing, read `summary.txt`. `--out DIR` puts them elsewhere.

## A scene

A scene is a `*.scene.toml` file under a `scenes\` folder: a widget and the world it is shown in. In a plugin, put them in `plugins\<id>\scenes\` (the plugin is implied; `plugin pack` leaves `scenes\` and `.look\` out of the `.wfplugin`).

```toml
format = 1
name = "Sunset card"

[widget]
id = "sunset"               # or file = "widgets/sunset.toml"
params = { show_seconds = true }

[env]
now = "2026-03-08T15:42:10"  # the same keys as --env on the command line
sys.cpu = 42

[sweep]
sizes = "grid:3x3"           # one scene per size, smallest to largest ("tiers", "max" or a list also work)

[expect]
flags = "error"              # error: a layout flag is a finding. warn (the default) shows it, ignore skips it
text = ["Sunset"]            # must appear in some text
no_text = ["undefined", "NaN"]
```

To see one widget or one change without a scene file, name the widget and set the world on the command line (`--size`, `--tier`, `--param`, `--state`, `--hide`, `--env`, `--now`, `--time`, `--palette`, `--scale`). A plugin's widget is found with `--content-root`, which loads the plugin folder where it stands without installing it:

```powershell
wayfinder scene dump --widget sunset --content-root plugins\sunset --size 300x200 --env sys.cpu=90 --find CPU | Out-Host
wayfinder scene render --widget sunset --content-root plugins\sunset --size 300x200 --dump --out sunset-look | Out-Host
```

A scene never reads your installed plugins, your real data folder or today's date unless it says so (`--real clock,sys` takes a value from this machine and makes the run not hermetic, so it cannot be blessed).

`wayfinder scene --help` prints the loop in a few lines, `wayfinder scene guide` prints this file, and `wayfinder scene list --sets` lists what there is to run.
