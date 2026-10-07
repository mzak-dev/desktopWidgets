# Wayfinder

Desktop widget engine for Windows (Rust, wgpu). See README.md for usage and CONTEXT.md for the domain language.

## Commit messages

Conventional Commits, Angular style: `type(scope): short summary`, imperative, lower case, no trailing period, under ~50 characters. Types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`. Keep the body to a line or two, only when the why isn't obvious.

Example: `feat(drawer): add blur and tint options`

## Seeing what you built (the visual loop)

Do not hand-write render examples, run the app, or ask the user for screenshots: use the `wayfinder scene` commands (`wayfinder` below is `cargo run --release --`, or the built exe). A scene is a widget in a fixed world (clock, system readings, no network); the commands describe the settled frame as text and draw it on the **software adapter (WARP) only**, and write their results under `scenes/.look/` (gitignored). `wayfinder scene guide` prints the whole guide, `assets/guides/LOOK.md`, which is also shipped to plugin authors.

1. Text first, no GPU: read the dump before any image. `wayfinder scene dump engine/clock@220x220#defaults --view flags` and `wayfinder scene dump engine/digital_clock@300x132#defaults --find Thursday` give every node's rect, clip, text, colours and hit actions, then the layout flags (`TRUNCATED`, `CLIPPED`, `OUTSIDE`, `ZERO`...). Start with the outline and the flags; `--under KEY --depth N` takes a subtree.
2. After any change to a Widget, Module, Tier, theme, layout code or Settings page: `wayfinder scene check fits` (layout flags over every built-in widget at a grid of sizes, exit 1 on findings) and `wayfinder scene diff engine --no-pixels`. Read the semantic diff; it names the node that moved, was reworded or changed colour.
3. Look once, not many times: `wayfinder scene sheet golden --diff` writes one image (baseline | new | difference); read that, not one PNG per scene. `wayfinder scene render golden --dump` writes a PNG and its dump per scene.
4. Only when the change is intended: `wayfinder scene bless engine --reason "<what and why>"`. Never bless to make a diff go away, and never bless a scene you did not read.
5. A few stray pixels in scenes you did not touch: `wayfinder scene selfcheck golden --perturb` (render is deterministic here, or it is not) before blaming the code.
6. A new Widget, Tier, Module or Settings page gets a scene in `scenes/` in the same commit, then `wayfinder scene bless <ids> --new-only --reason "<new scene>"`.

Rules. Pipe output (`| Out-Host`) or read `scenes/.look/summary.txt`. Exit 0 clean, 1 findings, 2 bad arguments or scene, 3 infrastructure. Scene verbs take no `--gpu` and never use a hardware adapter; where a command does take it (`--render-widget`), type `--gpu software` literally, never a variable and never another value (an unknown value falls back to the hardware GPU, which has crashed this machine's driver). Never run the app windows, `--selftest` or `examples/phase0_spike`. There are no bundled fonts: text is measured with this machine's fonts, so dumps and baselines are per machine (`scenes/baselines/<fonts hash>/`, untracked): on a machine with none, `wayfinder scene bless engine --reason "first baselines here"` records them. Scene files, dumps and verbs are tooling (`format = 1`), not a frozen interface.

## Refactor safety net (temporary)

Local layout-dump record/compare, until the scene baselines replace it. Baselines live in `$CARGO_TARGET_DIR/safety-net/` (untracked, per machine and per font set); use the same target dir to record and compare. No GPU.

- Record, on an untouched commit: `WF_NET=record cargo test --lib safety_net -- --ignored`
- Compare, after each refactor step: `WF_NET=compare cargo test --lib safety_net -- --ignored` (a failure names the case and node key)
