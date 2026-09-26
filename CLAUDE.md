# Wayfinder

Desktop widget engine for Windows (Rust, wgpu). See README.md for usage and CONTEXT.md for the domain language.

## Commit messages

Conventional Commits, Angular style: `type(scope): short summary`, imperative, lower case, no trailing period, under ~50 characters. Types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`. Keep the body to a line or two, only when the why isn't obvious.

Example: `feat(drawer): add blur and tint options`

## Refactor safety net (temporary)

Local layout-dump record/compare, until the scene baselines replace it. Baselines live in `$CARGO_TARGET_DIR/safety-net/` (untracked, per machine and per font set); use the same target dir to record and compare. No GPU.

- Record, on an untouched commit: `WF_NET=record cargo test --lib safety_net -- --ignored`
- Compare, after each refactor step: `WF_NET=compare cargo test --lib safety_net -- --ignored` (a failure names the case and node key)
