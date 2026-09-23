# Theme Cascade, Size Limits, Edit-Mode Remove and Fixes: Implementation Plan

> Work happens on `feat/theme-cascade`, one commit per task, with a checkpoint report after Tasks 2, 5, 7 and 10.

**Goal:** Make every style setting global and overridable per Instance, which the engine applies to every Widget. Give Widgets a tested min/max card size with a per-Instance switch, and a remove button in Edit Mode. Fix three reported bugs: the outline after blur off/on/off, the card roundness slider doing nothing, and the Settings Remove button being pushed out of the window.

**Architecture:** Style settings are Theme tokens declared in `assets/style.toml`. The Theme for an Instance composes: base → axes (the Instance's pick, else global) → global `style` → the Instance's `style`. `Card` reads the card-level tokens and applies them in `window_node`; `Anim` scales durations by `anim-speed`. Settings generates the style rows from the schema at both scopes.

**Tech stack:** Rust 2024, taffy, glyphon (CPU text layout), serde/serde_json, winit, windows-rs.

**Spec:** `docs/superpowers/specs/2026-09-23-theme-cascade-design.md` (branch `feat/theme-cascade`). Items added after the spec, from the user's review, are marked **(review)**.

## Context

The user asked to rework theming. Today style is split between global settings, a few global overrides, and style options living as params on some Widgets (Drawer transparency and blur, the clocks' accent). The user wants everything global, overridable per Widget, with Widgets keeping their own small tweaks. The same round brought size limits (a max next to the min, a switch to lift the max, tests at both sizes) and a reported blur-off outline bug. The review then added:

- Card roundness slider does nothing. Root cause: the user's `workspace.json` has `"blur": true`, and `Card::window_node` forces every card's radius to `WIN11_BLUR_RADIUS` (8) under blur, because DWM's rounded blur region only comes in fixed sizes. Separately, `clock.toml` computes its own radius and never reads `$radius-lg`.
- The Remove button is pushed out of the Settings window. Root cause: `w/right` (`settings.rs` `page_widgets`) has no `min_w(0)`. Its flex minimum is therefore its content's min-content width, and the text measurer (`ui.rs` measure closure) treats min-content as unbounded, so a long description counts at its full single-line width. A global min-content fix is unsafe: text wraps `WordOrGlyph` and would collapse to one glyph. So the fix is local.
- A remove button in Edit Mode, at a card corner.
- Lucide icons: **deferred** (see the end).

Sub-projects 2 (motion) and 3 (widget redesign) follow with their own specs.

## Global constraints

- Commits: Conventional Commits, `type(scope): summary`, imperative, lower case, no trailing period, under ~50 chars. No AI attribution anywhere (no Co-Authored-By trailers, no "generated with" lines).
- **Never run wgpu/DX12 on the user's GPU without asking.** Renders use the software adapter (the examples do so by default); the live app only with `--gpu software`.
- Tests: `cargo test --lib` (no GPU). Lint: `cargo clippy --all-targets`.
- An undefined token stays magenta (decision 14). Legacy `workspace.json` fields are read, never written.
- Match the code's style: short doc comments, `ponytail:` comments on deliberate shortcuts.

## Deviation from the spec (deliberate)

The spec says each Instance caches its Theme. Instead `App::theme_of(i)` composes on demand (a few `BTreeMap` merges, only on render, drag and commands, never while idle). That means no cache invalidation at add, remove, reload or override. Mark it: `// ponytail: composed per call; cache on Instance if drags ever profile slow`.

---

### Task 1: Blur off restores the default corners (fix)

**Files:** Modify `src/platform/win32.rs:155-183`.

**Interfaces:** Produces `pub fn blur_window_attributes(on: bool) -> (i32 /*corner pref*/, u32 /*accent state*/)` (Task 5 adds a corners argument).

- [ ] Failing test in `win32.rs` (add a `#[cfg(test)] mod tests` if none exists):
```rust
#[test]
fn blur_off_returns_to_the_never_blurred_corners() {
    assert_eq!(blur_window_attributes(true), (2, 3)); // DWMWCP_ROUND, ACCENT_ENABLE_BLURBEHIND
    assert_eq!(blur_window_attributes(false), (0, 0)); // DWMWCP_DEFAULT, ACCENT_DISABLED
}
```
- [ ] `cargo test --lib blur_off` → fails (no such fn).
- [ ] Implement it and use it in `set_blur`:
```rust
/// Rounding is only for the blur; left on, DWM keeps drawing its border at the window
/// edge, which sits outside the card once the shadow gutter comes back.
pub fn blur_window_attributes(on: bool) -> (i32, u32) {
    if on { (2, 3) } else { (0, 0) }
}
```
In `set_blur`: `let (corners, accent) = blur_window_attributes(on);`, pass `&corners` to `DwmSetWindowAttribute(…, DWMWINDOWATTRIBUTE(33), …)` and `state: accent`.
- [ ] Green. Commit `fix(platform): restore default corners when blur goes off`. Also copy this plan to `docs/superpowers/plans/2026-09-23-theme-cascade.md` and commit it as `docs(theme): add theme cascade plan`.

### Task 2: The Settings Remove button stays in the window (review, fix)

**Files:** Modify `src/settings.rs` (`page_widgets`, the `right` node; tests).

- [ ] Failing test in the `settings.rs` tests (imports: `crate::anim::Anim`, `crate::text::TextEngine`, `std::time::Instant`):
```rust
#[test]
fn a_long_description_wraps_and_keeps_remove_in_the_window() {
    let mut w = world();
    w.ws.instances.push(InstanceCfg { id: "system_monitor-1".into(), widget: "system_monitor".into(), ..Default::default() });
    let c = ctx(&w);
    let mut ui = UiState::default();
    ui.selected = Some("system_monitor-1".into());
    let (root, _) = ui.build(&c, WIN);
    let (mut text, mut anim) = (TextEngine::new(), Anim::default());
    let mut env = Env { text: &mut text, anim: &mut anim, hover: None, now: Instant::now(), scale: 1.0 };
    let f = ui::layout(&root, WIN, &mut env);
    let [x, _, bw, _] = f.rect_of("ip/system_monitor-1/del").unwrap();
    assert!(x + bw <= WIN.0, "Remove ends at {} in a {} px window", x + bw, WIN.0);
    let [_, _, _, dh] = f.rect_of("ip/system_monitor-1/hs").unwrap();
    assert!(dh > 20.0, "the description wraps onto more lines ({dh} px tall)");
}
```
- [ ] Run → fails (Remove past the edge, description one line).
- [ ] Fix: `Node::new("w/right").col().grow(1.0).min_w(0.0)` (both the `None` and `Some` arms). Add a comment on why: `// min_w(0): text reports its unwrapped width as min-content, so without this a long description pushes Remove out`.
- [ ] Green, including `every_page_builds_with_unique_keys`. Commit `fix(settings): wrap long descriptions beside remove`.

### Task 3: Style tokens in the Theme

**Files:** Create `assets/style.toml`. Modify `src/format.rs` (extract the param parser), `src/theme.rs`, `src/anim.rs`. Mechanical `compose` call-site updates in `src/app/mod.rs`, `src/card.rs`, `src/settings.rs`, `src/widgets/mod.rs`, `src/widgets/registry.rs`, `src/format.rs` tests, `examples/*.rs`.

**Interfaces (produced):**
- `format::parse_params(pt: &toml::Table) -> Result<Vec<ParamDef>, String>` (the loop body now inside `WidgetDef::parse`, moved as is; `WidgetDef::parse` calls it)
- `theme::style_schema() -> &'static [ParamDef]` (a `OnceLock`; parses `include_str!("../assets/style.toml")`'s `[params]`; `expect("built-in style schema is valid")`)
- `Theme::compose(lib: &Library, sel: &Selection, layers: &[&BTreeMap<String, Value>]) -> Theme`: later layers win
- `Theme::flag(&self, name: &str) -> bool`: `Bool(b)` → b, `Num(n)` → n != 0, `Str` → `== "true"`, else false (a string `"false"` must be false)
- `theme::ThemePick { palette, fonts, glyphs, icon_pack: Option<String> }` (serde, Default, PartialEq), with `fn resolve(&self, global: &Selection) -> Selection` and `fn is_empty(&self) -> bool`
- `anim::duration_factor(speed: &str) -> f32`: `"off"` 0.0, `"fast"` 0.6, `"relaxed"` 1.6, anything else 1.0

`assets/style.toml`: one `[params.<token>]` each, in Settings order:
- `accent`: color, **no default**, so it comes from the palette
- `radius-lg`: number 0–40, step 1, default 22, label "Corner roundness", help "With blur on, Windows only draws square, small or standard corners, so this snaps to the nearest."
- `outlines`: bool, true
- `blur`: bool, false, help = the current `Flag::Blur` help
- `transparent`: bool, false
- `bg-opacity`: number 0–100, step 5, 60, help "Used while the background is transparent."
- `tint`: bool, true, help "Off gives a neutral dark glass."
- `shadow`: number 0–100, step 10, 100, help "0 removes it"
- `text-scale`: number 80–140, step 5, 100
- `anim-speed`: enum, choices `["off","fast","normal","relaxed"]`, default `"normal"`

`base_sizes()` → `base_tokens()`: the size map, then every schema param's default except an empty string (so `accent` stays palette-owned).

- [ ] Failing tests in `theme.rs`:
```rust
#[test]
fn style_layers_cascade_over_palette_and_base() {
    let lib = Library::load(Path::new("nope"));
    let v = |s: &str| Value::Str(s.into());
    assert!(Theme::compose(&lib, &Selection::default(), &[]).flag("outlines"), "schema default");
    assert_eq!(Theme::compose(&lib, &Selection::default(), &[]).num("bg-opacity"), 60.0);
    let global = BTreeMap::from([("accent".to_string(), v("#111111")), ("blur".to_string(), Value::Bool(true))]);
    let mine = BTreeMap::from([("accent".to_string(), v("#222222"))]);
    let t = Theme::compose(&lib, &Selection::default(), &[&global, &mine]);
    assert_eq!(t.color("accent").to_hex(), "#222222", "instance beats global");
    assert!(t.flag("blur"), "global beats base");
    assert!(!Theme::compose(&lib, &Selection::default(), &[&BTreeMap::from([("blur".to_string(), v("false"))])]).flag("blur"));
}

#[test]
fn a_palette_can_set_style_defaults_and_a_pick_overrides_the_global_axes() {
    let mut lib = Library::load(Path::new("nope"));
    lib.palettes.push(Axis::parse("name = 'Glass'\n[tokens]\ntransparent = true\nbg-opacity = 30", None).unwrap());
    let pick = ThemePick { palette: Some("Glass".into()), ..Default::default() };
    let t = Theme::compose(&lib, &pick.resolve(&Selection::default()), &[]);
    assert!(t.flag("transparent"));
    assert_eq!(t.num("bg-opacity"), 30.0);
    assert!(ThemePick::default().is_empty() && !pick.is_empty());
    assert_eq!(ThemePick::default().resolve(&Selection::default()), Selection::default());
}
```
In `anim.rs` tests: `assert_eq!((duration_factor("off"), duration_factor("relaxed"), duration_factor("?")), (0.0, 1.6, 1.0));`
- [ ] Run → fail. Implement. Update every `Theme::compose(.., &x)` call: `&BTreeMap::new()` / `&Default::default()` becomes `&[]`; in `app/mod.rs` (lines ~119, ~201) it becomes `&[&overrides]`. Existing theme tests keep passing with `&[&ov]`.
- [ ] `cargo test --lib` green, `cargo clippy --all-targets` clean. Commit `feat(theme): declare style tokens and layer them`.

### Task 4: The engine applies the style tokens

**Files:** Modify `src/card.rs`, `src/widgets/mod.rs:130-170`, `src/widgets/meta.rs` (delete `has_own_blur`), `src/anim.rs`, `src/app/mod.rs` (`card`, `new_card`, new `global_style`, `theme_of`), `src/app/render.rs`, `examples/render_widgets.rs`, `assets/widgets/drawer.toml`, `assets/widgets/clock.toml`, `assets/widgets/digital_clock.toml`.

**Interfaces:**
- Consumes: `Theme::flag`, `Theme::num`, `anim::duration_factor`.
- Produces:
  - `Card::new(theme: &Theme) -> Card`, with fields `gutter, blur, outlines, radius: f32` (`radius-lg`), `bg_alpha: Option<f32>` (Some when `transparent`), `tint: bool, shadow: f32` (0–1), `text_scale: f32` (1.0 = 100 %)
  - `Card::window_node(&self, key: &str, window: (f32, f32), card: Node) -> Node` (no `widget_styles_blur` argument). `Card::of` and `Card::blurs` are deleted.
  - `Anim { duration_factor: f32, .. }` with a manual `Default` where it is 1.0. `Anim::value` multiplies `ms` and `delay_ms` by it before the `ms == 0` check.
  - `App::global_style(&self) -> BTreeMap<String, Value>`: **a shim until Task 6.** It returns `ws.overrides` (as `Value::Str`) plus `blur`/`outlines` from the legacy `ws.blur`/`ws.outlines` bools.
  - `App::theme_of(&self, i: usize) -> Theme` = `Theme::compose(&self.lib, &self.ws.theme, &[&self.global_style()])` for now.

`window_node` applies, in order:
1. tint off → fill (and gradient bottom) colour `Color::parse("#141414")`, keeping each alpha;
2. `bg_alpha` → fill and gradient-bottom alpha = it, **else** if `blur`, cap them at `MAX_FILL_ALPHA_OVER_BLUR`;
3. `blur` → radius `WIN11_BLUR_RADIUS` (Task 5 replaces this);
4. shadow → `None` when `shadow == 0`, else colour `mul_alpha(shadow)`;
5. one tree walk that strips borders when `!outlines` and multiplies `Kind::Text` `size` by `text_scale`.

App wiring:
- `card(i)` = `Card::new(&self.theme_of(i))`; `new_card()` = `Card::new(&self.theme)`.
- `render.rs`: compose `let theme = self.theme_of(i);` before the `App { .. }` destructure, pass `&theme` to `View`, and set `iw.anim.duration_factor = anim::duration_factor(&theme.str("anim-speed"))` before `prepare`.
- `widgets::prepare`: delete the `params.insert("blur", …)` and `own_blur`, and call `window_node(&v.cfg.id, v.window_size, b.root)`.

Widget files:
- `drawer.toml` drops params `transparent`, `opacity`, `blur`, `tint`. Its root becomes `fill = ["$surface", "$surface-2"]`, `radius = "$radius-lg"`, and loses `fill_alpha`.
- `clock.toml` and `digital_clock.toml` drop `[params.accent]`; `{param.accent}` becomes `$accent`.
- `clock.toml` root `radius` becomes `"$radius-lg"` **(review: the roundness slider must reach the analog clock too)**.

- [ ] Rewrite the `card.rs` tests to the new API (keep the round-trip and expand tests), with `fn theme_with(pairs: &[(&str, Value)]) -> Theme` composing one layer. Add:
```rust
#[test]
fn style_tokens_reach_the_card_and_its_text() {
    let card = || Node::new("c").fill(Color([0.2, 0.3, 0.4, 0.9])).shadow(14.0, 5.0, Color([0.0, 0.0, 0.0, 0.5]))
        .child(Node::text("c/t", "hi", 10.0, Color([1.0; 4])));
    let root = |pairs: &[(&str, Value)]| Card::new(&theme_with(pairs)).window_node("k", (100.0, 100.0), card()).children.remove(0);
    let see_through = root(&[("transparent", Value::Bool(true)), ("bg-opacity", Value::Num(40.0))]);
    assert!((see_through.look.fill.0[3] - 0.4).abs() < 1e-6);
    let glass = root(&[("tint", Value::Bool(false))]);
    assert_eq!((glass.look.fill.with_alpha(1.0).to_hex(), glass.look.fill.0[3]), ("#141414".to_string(), 0.9));
    assert!(root(&[("shadow", Value::Num(0.0))]).look.shadow.is_none());
    assert!((root(&[("shadow", Value::Num(50.0))]).look.shadow.unwrap().color.0[3] - 0.25).abs() < 1e-6);
    let big = root(&[("text-scale", Value::Num(120.0))]);
    assert!(matches!(&big.children[0].kind, Kind::Text(t) if (t.size - 12.0).abs() < 1e-4));
    let blurred = root(&[("blur", Value::Bool(true))]);
    assert_eq!(blurred.look.fill.0[3], MAX_FILL_ALPHA_OVER_BLUR);
    let blurred_see_through = root(&[("blur", Value::Bool(true)), ("transparent", Value::Bool(true)), ("bg-opacity", Value::Num(20.0))]);
    assert!((blurred_see_through.look.fill.0[3] - 0.2).abs() < 1e-6, "transparent wins over the blur cap");
}
```
In `anim.rs`:
```rust
#[test]
fn duration_factor_stretches_or_skips_animation() {
    let t0 = Instant::now();
    let mut a = Anim { duration_factor: 1.6, ..Default::default() };
    a.begin_frame();
    a.value("k", "x", [1.0; 4], 100, Ease::Linear, 0, Some([0.0; 4]), t0);
    let v = a.value("k", "x", [1.0; 4], 100, Ease::Linear, 0, None, t0 + Duration::from_millis(80));
    assert!((v[0] - 0.5).abs() < 1e-3, "{v:?}"); // 80 of 160 ms
    let mut off = Anim { duration_factor: 0.0, ..Default::default() };
    off.begin_frame();
    assert_eq!(off.value("k", "x", [1.0; 4], 100, Ease::Linear, 0, Some([0.0; 4]), t0), [1.0; 4]);
    assert!(!off.animating(t0));
}
```
Update the `widgets/mod.rs` Badge test: `Card::new(&theme_with_outlines_off)` instead of `Card::new(&t, false, false)`; drop `has_own_blur`.
- [ ] Fail → implement → `cargo test --lib` green (including `every_file_in_assets_widgets_is_built_in_and_parses`). Update `examples/render_widgets.rs:59` to `Card::new(&theme)`, then `cargo build --examples`.
- [ ] Commit `feat(card): apply style tokens to every widget`.

### Task 5: Corner roundness works under blur (review, fix)

**Files:** Modify `src/card.rs`, `src/platform/win32.rs`, `src/app/render.rs`, `src/app/instance.rs:41,84`.

**Interfaces (produced):**
- `card::blur_corners(radius: f32) -> (i32 /*DWM corner pref*/, f32 /*card radius*/)`: `radius < 2.0` → `(1, 0.0)` DONOTROUND; `< 6.0` → `(3, 4.0)` ROUNDSMALL; else `(2, 8.0)` ROUND. These are the only radii DWM can clip a blur to; they scale with DPI the same way logical px do.
- `Card::blur_corner_pref(&self) -> i32` = `blur_corners(self.radius).0`
- `win32::blur_window_attributes(on: bool, corners: i32) -> (i32, u32)`: on → `(corners, 3)`, off → `(0, 0)`. Also `win32::set_blur(hwnd, on: bool, corners: i32)`.
- `Instance.blur_applied: Option<i32>`: the corner pref currently applied, `None` = blur off.

`window_node` under blur sets `card.look.radius = blur_corners(self.radius).1`; delete `WIN11_BLUR_RADIUS`. In `render.rs`: `let want = card.blur.then(|| card.blur_corner_pref()); if want != iw.blur_applied { win32::set_blur(h, want.is_some(), want.unwrap_or(0)); iw.blur_applied = want; }`. That way a roundness change under blur re-applies.

- [ ] Failing tests: in `card.rs`:
```rust
#[test]
fn roundness_under_blur_snaps_to_what_windows_can_draw() {
    assert_eq!(blur_corners(0.0), (1, 0.0));
    assert_eq!(blur_corners(4.0), (3, 4.0));
    assert_eq!(blur_corners(22.0), (2, 8.0));
    let c = Card::new(&theme_with(&[("blur", Value::Bool(true)), ("radius-lg", Value::Num(3.0))]));
    let n = c.window_node("k", (100.0, 100.0), Node::new("c").radius(22.0)).children.remove(0);
    assert_eq!((n.look.radius, c.blur_corner_pref()), (4.0, 3));
}
```
Update the Task 1 test to `blur_window_attributes(true, 3) == (3, 3)` and `blur_window_attributes(false, 3) == (0, 0)`.
- [ ] Fail → implement → green. Commit `fix(card): let roundness reach blurred widgets`.

### Task 6: Style storage, migration and the global Settings page

**Files:** Modify `src/workspace.rs`, `src/app/mod.rs`, `src/app/commands.rs`, `src/app/render.rs`, `src/settings.rs`, `src/app/selftest.rs` (only if it references removed items).

**Interfaces (produced):**
- `Workspace.style: BTreeMap<String, serde_json::Value>`. Legacy fields, read but never written: `#[serde(default, skip_serializing)] overrides: BTreeMap<String, String>`, `blur: Option<bool>`, `outlines: Option<bool>`. `Workspace::default().version == 2`.
- `InstanceCfg.theme: ThemePick` with `#[serde(default, skip_serializing_if = "ThemePick::is_empty")]`, and `InstanceCfg.style: BTreeMap<String, serde_json::Value>` with `#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]`
- `Workspace::migrate(&mut self)`, called by `load` on a parsed file when `version < 2`; it sets `version = 2`
- `Workspace::style_map() -> BTreeMap<String, Value>`, `InstanceCfg::style_map() -> BTreeMap<String, Value>`
- `Workspace::theme_for(&self, lib: &Library, cfg: &InstanceCfg) -> Theme` = `compose(lib, &cfg.theme.resolve(&self.theme), &[&self.style_map(), &cfg.style_map()])`; `Workspace::global_theme(&self, lib) -> Theme`
- `Flag` keeps only `HeaderDrag` (`ALL: [Flag; 1]`)
- `settings::Scope { Global, Instance(String) }` with `fn parse(s: &str) -> Scope` (`"*"` = Global) and `fn key(&self) -> &str`
- `Cmd::Style(Scope, String, Option<Value>)` and `Cmd::ThemePick(String, String, Option<String>)` (id, axis `palette|fonts|glyphs|pack`, `None` = global). `Cmd::Override` is removed.
- Settings keys: style controls are `sy:<scope>:<token>` (the `st:` prefix is taken by shortcut targets).
  - toggle action `sy:<scope>|<token>`;
  - slider `sl:sy:<scope>:<token>`;
  - colour `cp:sy:<scope>:<token>` / `hx:sy:<scope>:<token>`;
  - enum dropdown `sy:<scope>:<token>`;
  - reset `syreset:<scope>|<token>` (`|*` = all at that scope).
- `UiState::style_row(&self, k: &Kit, ctx: &Ctx, scope: &Scope, pd: &ParamDef) -> Node`: shared by both pages. `fn scope_theme(ctx: &Ctx, scope: &Scope) -> Theme`: `ctx.ws.global_theme(ctx.lib)` for Global (not `ctx.theme`, which a test can build without `ws.style`), `ctx.ws.theme_for(ctx.lib, cfg)` for an Instance.

Migration rules (spec):
- `overrides` → `style` (string values kept);
- `blur == Some(true)` → `style.blur = true`; `outlines == Some(false)` → `style.outlines = false`;
- Drawer Instances: params `transparent`, `blur`, `tint` → `style`, and `opacity` → `style["bg-opacity"]`;
- `clock`/`digital_clock` Instances: param `accent` → `style.accent`;
- other widget ids are untouched.

App:
- Delete the `global_style` shim. `theme_of(i)` = `self.ws.theme_for(&self.lib, &self.ws.instances[i])`.
- `rebuild_theme` uses `self.ws.global_theme(&self.lib)` and loads the font files of the global **and every Instance's** picked font set.
- `render.rs`: `icon_pack` = `cfg.theme.resolve(&ws.theme).icon_pack`.
- `Cmd::Style`: snapshot `was: Vec<Card>`, write or remove in the scope's map, `refit_window_around_card` where `blur` changed (the same loop as `Cmd::Flag` today), `rebuild_theme()`, `mark_save()`.
- `Cmd::ThemePick`: set the field on the Instance's `ThemePick`, then as above.
- `Cmd::Flag` stays for header drag.

Settings (global page):
- `page_appearance` keeps the palette cards and the fonts/glyphs/pack rows. Then comes `k.section("ap/s3", "Style")`, then `style_schema().iter().map(|pd| self.style_row(k, ctx, &Scope::Global, pd))`, then "Reset all style" (`syreset:*|*`). The hand-written blur, outlines, accent and radius rows go.
- `style_row` builds its control by `pd.ty` exactly as `param_row` does for widget params (toggle, slider + value, swatch + hex input, dropdown). Its value comes from `scope_theme`, and it adds a `Reset` button when the scope's map has the token.
- Extend `slider_spec`/`slider_cmd`, `color_of_target`/`color_cmd`, `dropdown_items`/`dropdown_current`/`apply_pick` and `act` (`"sy"`, `"syreset"`) for the `sy:` keys. `slider_spec` takes min, max and step from the schema `ParamDef`.

- [ ] Failing tests. In `workspace.rs`:
```rust
#[test]
fn a_version_1_file_migrates_style_and_never_writes_legacy_fields() {
    let old = r##"{"version":1,"overrides":{"accent":"#ff8800","radius-lg":"30"},"blur":true,"outlines":false,
      "instances":[{"id":"drawer-1","widget":"drawer","params":{"transparent":true,"opacity":40,"blur":true,"tint":false,"title":"Apps"}},
                   {"id":"clock-1","widget":"clock","params":{"accent":"#00ff00","ticks":false}},
                   {"id":"mine-1","widget":"mine","params":{"accent":"#123456"}}]}"##;
    let dir = std::env::temp_dir().join(format!("wf-migrate-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(Workspace::path(&dir), old).unwrap();
    let (w, err) = Workspace::load(&dir);
    assert!(err.is_none());
    assert_eq!(w.version, 2);
    assert_eq!(w.style.get("accent"), Some(&serde_json::json!("#ff8800")));
    assert_eq!((w.style.get("blur"), w.style.get("outlines")), (Some(&serde_json::json!(true)), Some(&serde_json::json!(false))));
    let d = &w.instances[0];
    assert_eq!(d.style.get("bg-opacity"), Some(&serde_json::json!(40)));
    assert!(!d.params.contains_key("opacity") && d.params.contains_key("title"));
    assert_eq!(w.instances[1].style.get("accent"), Some(&serde_json::json!("#00ff00")));
    assert!(w.instances[2].style.is_empty() && w.instances[2].params.contains_key("accent"), "user widgets keep their own accent");
    w.save(&dir).unwrap();
    let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(Workspace::path(&dir)).unwrap()).unwrap();
    assert!(json.get("overrides").is_none() && json.get("blur").is_none() && json.get("outlines").is_none());
    assert!(json["instances"][2].get("style").is_none() && json["instances"][2].get("theme").is_none(), "empty maps stay out of the file");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn theme_for_layers_instance_over_global() {
    let lib = crate::theme::Library::load(Path::new("nope"));
    let mut w = Workspace::default();
    w.style.insert("accent".into(), serde_json::json!("#111111"));
    let mut c = InstanceCfg::default();
    assert_eq!(w.theme_for(&lib, &c).color("accent").to_hex(), "#111111");
    c.style.insert("accent".into(), serde_json::json!("#222222"));
    c.theme.palette = Some("Daylight".into());
    let t = w.theme_for(&lib, &c);
    assert_eq!((t.color("accent").to_hex(), t.color("text").to_hex()), ("#222222".to_string(), "#141a2a".to_string()));
}
```
Update `flags_read_and_write_the_saved_fields` for `HeaderDrag` only. In `settings.rs`, replace the `Cmd::Override` asserts:
- `hex_input_only_commits_valid_colours` focuses `hx:sy:*:accent` and expects `Cmd::Style(Scope::Global, "accent".into(), Some(Value::Str("#ff8800".into())))`;
- `slider_target_specs…` checks `slider_spec(&c, "sy:*:radius-lg")` → `(0.0, 40.0, 1.0, 22.0)` and `slider_cmd("sy:*:radius-lg", 30.0)` → `Cmd::Style(Scope::Global, "radius-lg".into(), Some(Value::Num(30.0)))`;
- `colour_picker_emits…` uses `cp:sy:clock-1:accent` → `Cmd::Style(Scope::Instance("clock-1".into()), "accent".into(), Some(..))`.

New test:
```rust
#[test]
fn style_toggles_and_resets_at_both_scopes() {
    let mut w = world();
    w.ws.style.insert("blur".into(), serde_json::json!(true));
    let c = ctx(&w);
    let mut ui = UiState::default();
    assert_eq!(ui.act("sy:*|outlines", &c, None), vec![Cmd::Style(Scope::Global, "outlines".into(), Some(Value::Bool(false)))]);
    assert_eq!(ui.act("sy:clock-1|blur", &c, None), vec![Cmd::Style(Scope::Instance("clock-1".into()), "blur".into(), Some(Value::Bool(false)))], "the instance starts from the inherited global value");
    assert_eq!(ui.act("syreset:*|blur", &c, None), vec![Cmd::Style(Scope::Global, "blur".into(), None)]);
    assert_eq!(ui.act("pick:sy:*:anim-speed|off", &c, None), vec![Cmd::Style(Scope::Global, "anim-speed".into(), Some(Value::Str("off".into())))]);
}
```
- [ ] Fail → implement → `cargo test --lib` green (`every_page_builds_with_unique_keys` covers the new rows). Commit `feat(theme): store style globally and migrate old files`.

### Task 7: Per-widget Style section

**Files:** Modify `src/settings.rs` (`instance_panel`, dropdown handling, `act`).

**Interfaces:**
- Consumes: `style_row`, `scope_theme`, `Scope`, `Cmd::ThemePick`, `Workspace::theme_for`.
- Produces: theme-pick dropdown keys `tp:<id>:<axis>` (axis `palette|fonts|glyphs|pack`). Items: `("", format!("Global ({})", global_name))` first, then the library's names. `apply_pick` maps `""` → `Cmd::ThemePick(id, axis, None)`.

`instance_panel` order: Placement, Options, then the Style section:
- `k.section("ip/{id}/s3", "Style")`;
- a "Theme" row holding the four `tp:` dropdowns in a wrapping row (`CONTROL_W / 2.0` wide each);
- every `style_row(.., &Scope::Instance(id), pd)`;
- a "Reset style" button (`syreset:{id}|*`), only when `!cfg.style.is_empty() || !cfg.theme.is_empty()`. `syreset:<id>|*` also emits `Cmd::ThemePick(id, axis, None)` for each set axis.

In `style_row` at Instance scope, a token the Instance does not override shows a dim `"global"` label after the control; an overridden one shows an 8 px accent dot and `Reset`.

- [ ] Failing test:
```rust
#[test]
fn a_widget_can_pick_its_own_palette_or_go_back_to_global() {
    let w = world();
    let c = ctx(&w);
    let mut ui = UiState::default();
    let items = ui.dropdown_items(&c, "tp:clock-1:palette");
    assert_eq!(items[0], (String::new(), "Global (Midnight)".to_string()));
    assert_eq!(ui.act("pick:tp:clock-1:palette|Daylight", &c, None), vec![Cmd::ThemePick("clock-1".into(), "palette".into(), Some("Daylight".into()))]);
    assert_eq!(ui.act("pick:tp:clock-1:palette|", &c, None), vec![Cmd::ThemePick("clock-1".into(), "palette".into(), None)]);
}
```
- [ ] Fail → implement → green. Extend Task 2's layout test with a second assertion loop over every style row at Instance scope: the `ip/<id>` rows' right edges are ≤ `WIN.0`. Render the settings on software: `cargo run --release --example render_settings -- <scratch dir>`, then look at the Widgets page PNG. Commit `feat(settings): per-widget style overrides`.

### Task 8: Size limits

**Files:** Modify `src/format.rs` (`TOP`, parse), `src/widgets/meta.rs`, `src/widgets/mod.rs` (Badge test literal), `src/edit.rs` (`dragged_rect` and its tests), `src/card.rs` (`min_window_px` → `window_px`), `src/workspace.rs`, `src/app/edit_mode.rs`, `src/app/commands.rs`, `src/settings.rs`.

**Interfaces (produced):**
- TOML `max_size = [w, h]` → `WidgetMeta.max_card_size: Option<(f32, f32)>`. Parse error `"max_size is smaller than min_size"` when either side is below the min.
- `edit::dragged_rect(handle, start, dx, dy, min: (i32, i32), max: Option<(i32, i32)>, snap: &Snap) -> Rect`. For each moving edge, after snapping and the min clamp, clamp to the max: `l = l.max(rr - m.0)`, `rr = rr.min(l + m.0)`, `t = t.max(b - m.1)`, `b = b.min(t + m.1)`. `Move` ignores it.
- `Card::window_px(&self, card: (f32, f32), scale: f64) -> (i32, i32)` (the old `min_window_px`, renamed; the one call site is updated)
- `InstanceCfg.size_limit: bool`, default `true` (through `InstanceCfg::default`)
- `App::max_size_phys(&self, i: usize) -> Option<(i32, i32)>`: `None` when `!cfg.size_limit` or the meta has no max
- `Cmd::SizeLimit(String, bool)`; settings action `lim:<id>`

Behaviour:
- `drag_update` passes `max.map(|m| (m.0 - 2 * g, m.1 - 2 * g))`.
- `Cmd::SizeLimit(id, on)`: set the field. If `on` and there is a max: `c = card.card_of_window(outer_rect, s)`, `c.w = c.w.min(max.0)`, `c.h = c.h.min(max.1)` in card px, `set_window_rect(window_of_card(c))`, `save_rect_to_workspace`, `mark_save`.
- Placement gets a row, shown only when the meta has a max: "Size limit", help "Keep it within the size it was designed for. Off lets it grow larger; the minimum always applies.", with `k.toggle("lim:{id}", cfg.size_limit, "lim:{id}")`.

Start max values (Task 9 may adjust): clock `[520, 520]`, digital_clock `[640, 260]`, system_monitor `[720, 440]`, icon_list `[480, 900]`, icon_folder `[200, 240]`, drawer `[900, 720]`.

- [ ] Failing tests in `edit.rs`:
```rust
#[test]
fn resize_stops_at_the_max_only_while_a_max_is_given() {
    let start = Rect::new(400, 300, 200, 100);
    let free = Snap { threshold: 0, ..Default::default() };
    let r = dragged_rect(Handle::SE, start, 500, 500, (60, 40), Some((300, 150)), &free);
    assert_eq!((r.x, r.y, r.w, r.h), (400, 300, 300, 150));
    let r = dragged_rect(Handle::NW, start, -500, -500, (60, 40), Some((300, 150)), &free);
    assert_eq!((r.right(), r.bottom(), r.w, r.h), (600, 400, 300, 150), "anchored on the far edge");
    let r = dragged_rect(Handle::E, start, 500, 0, (60, 40), None, &free);
    assert_eq!(r.w, 700, "no limit");
    let r = dragged_rect(Handle::W, start, 500, 0, (60, 40), Some((300, 150)), &free);
    assert_eq!(r.w, 60, "the min holds either way");
}
```
In `format.rs` tests: parsing `max_size = [300, 200]` gives `Some((300.0, 200.0))`; `min_size = [100, 100]` with `max_size = [80, 200]` is an error containing `"max_size"`. In `settings.rs`: `ui.act("lim:clock-1", &c, None) == vec![Cmd::SizeLimit("clock-1".into(), false)]`. Existing `dragged_rect` calls in tests gain `None`.
- [ ] Fail → implement → green. Commit `feat(edit): add per-widget max size with an off switch`.

### Task 9: Min/max layout harness

**Files:** Create `src/widgets/fits.rs` (test-only); add `#[cfg(test)] mod fits;` in `src/widgets/mod.rs`. Tune `min_size`/`max_size` in `assets/widgets/*.toml`. Add max-size cases to `examples/render_widgets.rs`.

**Interfaces:** Consumes `Registry`, `WidgetMeta::{min_card_size, default_card_size, max_card_size, seed_params}`, `DataSources`, `ui::layout`, `TextEngine::measure(key, spec, None) -> (f32, f32)`.

```rust
//! Every built-in Widget lays out cleanly at its min, default and max card size.
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use super::{Inputs, ParamType, Registry};
use crate::anim::Anim;
use crate::data::{DataSources, SourceCx, Tm};
use crate::text::{TextEngine, TextSpec};
use crate::theme::{Library, Selection, Theme};
use crate::ui::{self, Env, Kind, Node};
use crate::value::Value;
use crate::workspace::InstanceCfg;

/// Text a reader must see whole: anything outside a scroll container.
fn fixed_texts(n: &Node, out: &mut Vec<(String, TextSpec)>) {
    if n.scroll_offset.is_some() {
        return;
    }
    if let Kind::Text(t) = &n.kind {
        if !t.text.trim().is_empty() {
            out.push((n.key.clone(), t.clone()));
        }
    }
    n.children.iter().for_each(|c| fixed_texts(c, out));
}

#[test]
fn every_builtin_fits_its_min_default_and_max_size() {
    let reg = Registry::load(Path::new("no-such-dir"));
    let theme = Theme::compose(&Library::load(Path::new("nope")), &Selection::default(), &[]);
    let sources = DataSources::builtin();
    // widest realistic clock text
    let tm = Tm { year: 2026, month: 9, day: 23, dow: 3, hour: 23, minute: 58, second: 58, ms: 0 };
    let mut text = TextEngine::new();
    let mut failures = Vec::new();
    for id in reg.ids() {
        let Some(Ok(w)) = reg.get(&id) else { continue };
        let meta = w.meta();
        let mut sizes = vec![("min", meta.min_card_size), ("default", meta.default_card_size)];
        sizes.extend(meta.max_card_size.map(|m| ("max", m)));
        let all_on: BTreeMap<String, Value> = meta.params.iter().filter(|p| p.ty == ParamType::Bool).map(|p| (p.name.clone(), Value::Bool(true))).collect();
        for (variant, extra) in [("defaults", BTreeMap::new()), ("every switch on", all_on)] {
            let mut cfg = InstanceCfg { id: format!("{id}-1"), widget: id.clone(), ..Default::default() };
            meta.seed_params(&mut cfg);
            extra.iter().for_each(|(k, v)| cfg.set_param(k, v));
            let params = cfg.params_map();
            for (size_name, size) in &sizes {
                let case = format!("{id} at {size_name} {size:?}, {variant}");
                let cx = SourceCx { cfg: &cfg, tm, icon_pack: "Default" };
                let read = |n: &str| sources.value(n, &cx);
                let state = BTreeMap::new();
                let inp = Inputs { params: &params, state: &state, card_size: *size, key_prefix: &cfg.id, read_source: &read };
                let b = match w.build(&inp, &theme, &|_| None) {
                    Ok(b) => b,
                    Err(e) => {
                        failures.push(format!("{case}: {e}"));
                        continue;
                    }
                };
                if !b.warnings.is_empty() {
                    failures.push(format!("{case}: warnings {:?}", b.warnings));
                }
                let frame = {
                    let mut anim = Anim::default();
                    let mut env = Env { text: &mut text, anim: &mut anim, hover: None, now: Instant::now(), scale: 1.0 };
                    ui::layout(&b.root, *size, &mut env)
                };
                let mut texts = Vec::new();
                fixed_texts(&b.root, &mut texts);
                for (key, spec) in texts {
                    let Some([x, y, tw, th]) = frame.rect_of(&key) else { continue };
                    if tw < 1.0 || th < 1.0 {
                        failures.push(format!("{case}: `{}` squashed to {tw}x{th}", spec.text));
                    } else if x < -0.5 || y < -0.5 || x + tw > size.0 + 0.5 || y + th > size.1 + 0.5 {
                        failures.push(format!("{case}: `{}` at {:?} sticks out of the card", spec.text, [x, y, tw, th]));
                    } else if !spec.wrap && text.measure(&key, &spec, None).0 > tw + 1.0 {
                        failures.push(format!("{case}: `{}` is cut off ({tw} px wide)", spec.text));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
```
- [ ] Run `cargo test --lib every_builtin_fits`. For each failure, **raise that widget's `min_size`** to the smallest size that passes (or lower its `max_size` if the max breaks). Do not loosen the test. Record each changed value and its reason in the commit body.
- [ ] Add a `Case` at max size for each widget to `examples/render_widgets.rs`. Run `cargo run --release --example render_widgets -- <scratch>\widgets.png` (software adapter) and check the max-size tiles look intended; adjust `max_size` if not.
- [ ] Green. Commit `test(widgets): lay out every widget at its size limits`.

### Task 10: Remove button in Edit Mode (review)

**Files:** Modify `src/edit.rs`, `src/app/render.rs` (overlay call), `src/app/input.rs` (`on_mouse`, `on_cursor`), `src/app/commands.rs` (extract removal), `src/app/mod.rs` (field), `src/app/edit_mode.rs` (`set_edit` disarms), `src/settings.rs` (`SettingsWin::invalidate`).

Placement: the **bottom-right** corner, inset inside the card. At the top right it would collide with the position/size pill on narrow widgets (the icon folder is 92 px wide).

**Interfaces (produced):**
- `edit::REMOVE_BUTTON: f32 = 24.0`, `edit::remove_button_center(card: [f32; 4]) -> (f32, f32)` = `(x + w - 22.0, y + h - 22.0)`, `edit::over_remove_button(px, py, card) -> bool` (within 14 px of the centre)
- `edit::overlay(id, window, gutter, label, theme, active, remove_armed: bool) -> Node` gains a child keyed `{id}!ov/x`:
  - unarmed: a 24 px circle (fill `Color([0.04, 0.05, 0.09, 0.86])`, border 2 `danger`, glyph `theme.str("glyph-close")` in font `theme.str("font-glyph")`, 11 px, white) with `.enter(180, 6.0, 0)`, so it pops up when Edit Mode opens;
  - armed: a pill with the text child `{id}!ov/xt` "Remove?" (danger fill, white, 12 px bold), right-aligned to the same anchor.
- `App.remove_armed: Option<String>` (Instance id)
- `App::remove_instance(&mut self, id: &str)` = the body of `Cmd::Remove`, plus `remove_armed = None` and `settings.invalidate()`; `Cmd::Remove` calls it
- `SettingsWin::invalidate(&mut self)` sets its private `redraw = true`

Behaviour (a two-click confirm, the same pattern as the Settings "Really remove?"):
- In Edit Mode, a press over the button removes the Instance if it's already armed, otherwise arms it and redraws; either way no drag starts.
- Any other press disarms (and redraws the previously armed window). `set_edit(false)` disarms.
- In `on_cursor` (Edit Mode), hovering the button sets `CursorIcon::Pointer`, checked before the resize-grip cursor.

- [ ] Failing tests in `edit.rs` (a `theme()` helper composes the default Theme):
```rust
#[test]
fn the_remove_button_sits_inside_the_card_and_never_steals_a_resize_grip() {
    for card in [[20.0, 20.0, 220.0, 220.0], [0.0, 0.0, 72.0, 44.0], [20.0, 20.0, 48.0, 48.0]] {
        let (cx, cy) = remove_button_center(card);
        assert!(over_remove_button(cx, cy, card));
        assert_eq!(hit_handle(cx, cy, card, 16.0), Handle::Move, "{card:?}: its centre is not a resize grip");
        let [x, y, w, h] = card;
        let r = REMOVE_BUTTON / 2.0;
        assert!(cx - r >= x && cy - r >= y && cx + r <= x + w && cy + r <= y + h, "{card:?}: fully inside the card");
    }
    assert!(!over_remove_button(30.0, 30.0, [20.0, 20.0, 220.0, 220.0]));
}

#[test]
fn overlay_shows_a_remove_button_that_turns_into_a_confirm_pill() {
    fn find<'a>(n: &'a Node, key: &str) -> Option<&'a Node> {
        if n.key == key { Some(n) } else { n.children.iter().find_map(|c| find(c, key)) }
    }
    let plain = overlay("w", (140.0, 120.0), 20.0, "0, 0", &theme(), None, false);
    assert!(find(&plain, "w!ov/x").is_some() && find(&plain, "w!ov/xt").is_none());
    let armed = overlay("w", (140.0, 120.0), 20.0, "0, 0", &theme(), None, true);
    assert!(matches!(&find(&armed, "w!ov/xt").unwrap().kind, Kind::Text(s) if s.text == "Remove?"));
}
```
- [ ] Fail → implement → green. Self-test on software (`--selftest --gpu software`) still passes its Edit Mode drag checks. Commit `feat(edit): remove a widget from edit mode`.

### Task 11: Docs

**Files:** `assets/guides/THEMES.md`, `assets/guides/wayfinder-theme/SKILL.md`, `CONTEXT.md`, `README.md` (the widget-authoring reference gains `max_size`; "Themeable end to end" mentions per-widget style; the Edit layout table gains "the × in the corner: remove (click twice)").

- `THEMES.md`: a new "## Style" section after "## Sizes":
  - the `style.toml` token table (token, type, default, meaning);
  - a palette may set any of them (e.g. `transparent = true` for a glass palette);
  - each widget can override them or pick its own palette, fonts, glyphs and icon pack in its Settings panel;
  - with blur on, roundness snaps to square, small or standard.
  
  Troubleshooting: "Clear all overrides" becomes **Reset all style** (global) and **Reset style** (on a widget).
- `SKILL.md` step 6: the same rename.
- `CONTEXT.md`:
  - add **Style**: "The Theme tokens `style.toml` declares (accent, roundness, outlines, blur, transparency, tint, shadow, text scale, animation speed). Set globally, overridable per Instance, applied by the engine to every Card.";
  - change **Theme**'s _Avoid_ to `skin`;
  - add **Size Limit**: "A Widget's maximum card size; an Instance may switch it off, never the minimum."
- [ ] Commit `docs(theme): document style tokens and size limits`.

---

## Deferred: Lucide icons (review)

Scope as chosen by the user: a built-in **Lucide glyph set** (selectable in Appearance, mapping the 16 chrome glyph tokens), **plus** any Lucide icon usable by name in widget files (e.g. `type = "icon"`, `name = "cpu"`). The user said "drop it for now", because it needs `lucide.ttf` and its name→codepoint map from the `lucide-static` package (ISC). Pick it up together with sub-project 3, whose redesigned widgets would use it, once the font file question is settled.

## Verification

1. `cargo test --lib` passes: all new tests, plus the existing theme, card, settings, registry, workspace and edit tests.
2. `cargo clippy --all-targets` has no new warnings, and `cargo build --release --examples` succeeds.
3. Offscreen renders on the software adapter: `render_widgets` (including max sizes, transparent and tint-off variants) and `render_settings` (the Appearance Style section, and a widget's Style section with the Remove button inside the window).
4. Self-test on software: `cargo run --release -- --selftest --gpu software --data $env:TEMP\wf-test`. This briefly opens widget windows on the desktop.
5. **Manual, by the user, on their normal GPU:**
   - blur off → on → off leaves no outline;
   - the roundness slider changes blurred widgets in three steps and unblurred ones smoothly;
   - a per-widget palette and transparency apply to that widget only;
   - the size limit stops a resize, and switching it off lets the widget grow;
   - the Edit Mode × removes a widget on a second click;
   - loading their real `workspace.json` (global blur on, the Drawer transparent with `opacity: 0` and tint off) keeps the Drawer looking the same.

## Self-review against the spec and the review

- Schema, defaults and palette-set defaults: Task 3. Storage, cascade and migration: Tasks 3 and 6. Engine application and `anim-speed`: Task 4. Settings at both scopes and the commands: Tasks 6 and 7. Removed items: Task 4. Docs: Task 11. Blur fix: Task 1. Size limits and the harness: Tasks 8 and 9.
- Review items: roundness under blur, Task 5 (+ the clock radius in Task 4); Remove button in Settings, Task 2 (+ the Task 7 check); Edit Mode remove, Task 10; Lucide, deferred.
- Names are consistent across tasks: `Card::new(&Theme)`, `blur_corners`, `blur_window_attributes(on, corners)`, `Theme::compose(.., &[layers])`, `Scope`, `scope_theme`, `Cmd::Style`/`ThemePick`/`SizeLimit`, `sy:`/`tp:`/`lim:` keys, `max_card_size`, `window_px`, `duration_factor`, `remove_button_center`, `over_remove_button`, `remove_instance`.
- Known gaps, accepted: guides are only written when missing, so existing data folders keep the old guide. An Instance already larger than a lowered `max_size` is only shrunk when the switch is toggled. Removing in Edit Mode is confirmed, not undoable.
