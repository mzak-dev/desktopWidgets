# Theme cascade: global style, per-Instance overrides

Sub-project 1 of 3 of the widget rework (1. theme cascade, 2. motion, 3. widget redesign). Motion and the widget redesign get their own specs; this one only lays the ground they build on.

## Problem

Style is split three ways. Global: palette, font set, glyph set, icon pack, two token overrides (`accent`, `radius-lg`) and two switches (`blur`, `outlines`). Per Widget, disguised as params: `accent` on both clocks, and `transparent`, `opacity`, `blur`, `tint` on the Drawer only. So blur exists twice, transparency exists once, and a widget cannot use a different palette from its neighbours.

## Goal

Every style setting is global, any Instance can override any of them, and Widgets keep their own params for what only they have (seconds, gauges, icon size...).

## Decisions

- **Overrides live per Instance**, next to its params. No per-Widget-kind layer.
- **An Instance may pick its own palette, font set, glyph set and icon pack**, as well as override single tokens.
- **Style settings are Theme tokens** (approach A). The engine applies the card-level ones to every Widget in `Card`, so TOML and Rust Widgets obey them with no wiring. A Widget may still read them as `$tokens`.
- `header_drag` is behaviour, not style: it stays a single global switch.

## Style schema

`assets/style.toml`, in the same `[params.<name>]` syntax Widgets use and parsed by the same parser, declares the style settings (type, label, help, range). It drives the Settings forms at both scopes. The token name is the param name.

| Token | Type | Default | Notes |
|---|---|---|---|
| `accent` | color | palette | |
| `radius-lg` | number 0–40 | 22 | label "Corner roundness" |
| `outlines` | bool | true | |
| `blur` | bool | false | |
| `transparent` | bool | false | |
| `bg-opacity` | number 0–100 | 60 | used while `transparent` |
| `tint` | bool | true | off = neutral glass `#141414` |
| `shadow` | number 0–100 | 100 | percent of the palette's shadow; 0 = none |
| `text-scale` | number 80–140, step 5 | 100 | percent |
| `anim-speed` | enum off, fast, normal, relaxed | normal | duration factor 0, 0.6, 1, 1.6 |

Defaults not taken from a palette join `radius-lg` and friends in `theme.rs`'s built-in base tokens. Because palettes, font sets and glyph sets sit above the base, **a palette may set style defaults** (a glass palette can ship `transparent = true`).

## Storage

`Workspace` (`workspace.json`, version 2):

- `style: BTreeMap<String, serde_json::Value>` replaces `overrides`, `blur` and `outlines`.
- `theme: Selection` and `header_drag` are unchanged. `Flag` keeps only `HeaderDrag`.

`InstanceCfg`:

- `theme: ThemePick`, with `palette`, `fonts`, `glyphs`, `icon_pack`, each `Option<String>`; `None` = use the global one. Omitted from JSON when all are `None`.
- `style: BTreeMap<String, serde_json::Value>`: this Instance's overrides. Omitted when empty.

## Cascade

For one Instance, later wins:

1. base tokens (sizes and the style defaults above)
2. the first built-in of each axis (as today)
3. glyph set, font set, palette: the Instance's pick, else the global one
4. global `style`
5. the Instance's `style`

`Theme::compose` takes the picked `Selection` and the two style maps. An undefined token is still magenta.

## Migration (on load, when `version < 2`)

- `overrides` → `style` (values kept as strings; `Theme::num` already parses them).
- `blur: true` → `style.blur = true`; `outlines: false` → `style.outlines = false`.
- Drawer Instances: params `transparent`, `blur`, `tint` → the Instance's `style`; `opacity` → `style.bg-opacity`.
- `clock` and `digital_clock` Instances: param `accent` → the Instance's `style.accent`.
- Only built-in Widget ids are migrated: a user Widget's own `accent` param is left alone.
- The migrated file is written back as version 2 on the next save. Legacy fields are read, never written.

## The engine applies the card-level tokens

`Card::new(&theme)` replaces `Card::new(theme, blur, outlines)` and `Card::of(ws, cfg, theme)`. It reads `blur`, `outlines`, `transparent`, `bg-opacity`, `tint`, `shadow` and `text-scale` from the Instance's Theme. `window_node` then, on the root card:

| Token | Effect |
|---|---|
| `blur` | unchanged: no gutter, Windows 11 radius, DWM blur. Fill alpha is capped at 0.6 unless `transparent` is on |
| `outlines` off | unchanged: strips every border |
| `transparent` | root fill alpha (and gradient bottom) = `bg-opacity` / 100 |
| `tint` off | root fill colour = `#141414`, alpha kept |
| `shadow` | root shadow alpha × `shadow` / 100; 0 removes it |
| `text-scale` | every text node's size × `text-scale` / 100 (a tree walk, like the border strip) |

`anim-speed` travels in `ui::Env` as a duration factor. `emit()` multiplies transition durations, enter durations and enter delays by it; factor 0 means the value jumps to its target and nothing animates. The Settings window passes 1. Motion (sub-project 2) reads the same token for window tweens.

## App

- Each `InstWin` caches its composed `Theme`. `rebuild_theme()` rebuilds the global one and all Instances'; a change to one Instance's style or pick rebuilds only that one.
- `View.theme` and `View.icon_pack` come from the Instance's Theme and pick.
- Settings and the Edit Mode overlay keep the global Theme: they are chrome.
- Blur changing for an Instance (either scope) triggers the existing `regutter`.

## Settings

**Appearance page (global).** Palette cards and the font, glyph and icon pack pickers are unchanged. The hand-written blur, outlines, accent and roundness rows are replaced by a **Style** section generated from `style.toml` with the same controls Widget params use (toggle, slider, colour swatch plus hex, dropdown). A row that differs from its default shows **Reset**. "Clear all overrides" becomes **Reset all style**.

**Widget panel.** Order: Placement, Options (the Widget's own params), **Style**:

- a **Theme** row: palette, fonts, glyphs, icon pack dropdowns, first choice "Global (<name>)";
- the style rows showing the Instance's effective value. An inherited value is marked with a dim "global" hint; editing creates an Instance override; an overridden row shows an accent dot and **Reset** (back to global);
- **Reset style**, only when the Instance has overrides.

**Commands.** `Cmd::Override` and `Cmd::Flag(Blur | Outlines)` become:

- `Cmd::Style { scope, token, value: Option<Value> }`: `None` resets
- `Cmd::ThemePick { scope, axis, name: Option<String> }`: at Instance scope, `None` = global

`scope` is `Global` or `Instance(id)`. The global `Cmd::Theme(Selection)` stays for the palette cards.

## Removed

- `WidgetMeta::has_own_blur()` and the `widget_styles_blur` argument of `Card::window_node`.
- `prepare()` writing `params["blur"]`.
- `drawer.toml`: params `transparent`, `opacity`, `blur`, `tint`, and the `fill`, `fill_alpha` and `radius` expressions that used them (it becomes a plain `$surface` card like the rest).
- `clock.toml`, `digital_clock.toml`: param `accent`; their `{param.accent}` becomes `$accent`.
- The `fill_alpha` attribute stays: user Widgets may use it.

## Docs

- `assets/guides/THEMES.md`: a "Style" section listing the style tokens; a palette may set them; per-Widget themes live in the Widget's Settings panel; troubleshooting points at **Reset all style** and a Widget's **Reset style**.
- `assets/guides/wayfinder-theme/SKILL.md` step 6: same rename.
- `CONTEXT.md`: add **Style** (the token subset `style.toml` declares, overridable per Instance) and adjust **Theme**'s entry, since "Avoid: style" no longer holds.
- Guides are only written when missing, so existing data folders keep the old guide. Accepted.

## Also in this plan: blur-off outline fix

Reported: turning blur off, on, then off again leaves an outline a few pixels outside the widget's border.

Root cause: `win32::set_blur` sets `DWMWA_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND` on every call, including `on = false`, and nothing ever resets it. Popup windows are not rounded by default, so an Instance that never had blur gets no DWM corners and no DWM border. After the first blur-on, DWM keeps rounding the window and drawing its system border along the window's edge. With blur off again the window grows back by the shadow gutter, so that border now sits outside the card. That explains why it needs off, on, off: the first "off" never called `set_blur`.

Fix: `DWMWCP_ROUND` while blur is on, `DWMWCP_DEFAULT` when it goes off, which is the never-blurred state. The choice of attributes moves into a pure `blur_window_attributes(on)` with a unit test; the live check is manual (off, on, off: no outline). This is independent of the cascade and lands first, as its own commit.

## Also in this plan: size limits

Every Widget gets a maximum card size next to its minimum, and both are tested so a Widget never breaks within them.

- **Declared:** `max_size = [w, h]` in a TOML Widget, next to `min_size`; `WidgetMeta.max_card_size`. A Rust Widget sets it in its meta. The built-in values are chosen per Widget from renders at that size.
- **Enforced:** Edit Mode resize clamps the card between min and max (`edit::dragged_rect` takes the max as well). Moving is unaffected. An `[expand]` state is not clamped by the max; it is the Widget's own open size and is already clamped to the work area.
- **Switch:** a per-Instance **Size limit** toggle in the Placement section, on by default. Off lifts the maximum only; the minimum always holds, because below it the layout really does break. Turning it back on shrinks an oversized Instance to its max, keeping its top-left.
- **Tested** (`cargo test --lib`: layout needs only `TextEngine`, which is CPU-side). For every built-in Widget, at its min, default and max size, with default params and with every bool param on:
  - it builds with no warnings or error card;
  - every text node's rect lies inside the card, except inside a scroll container, which scrolls by design;
  - no text node with non-empty text is laid out at zero width or height.
  
  A current Widget that fails gets its `min_size` raised. The harness stays in place and guards the redesign in sub-project 3.

## Testing

All in `cargo test --lib`, no GPU.

- `theme`: cascade order (Instance style > global style > palette > base); an Instance palette pick; a palette setting a style default; unknown token still magenta.
- `workspace`: a version-1 file with `overrides`, `blur`, `outlines`, a Drawer with style params, a clock with `accent` and a user Widget with `accent` migrates as specified and round-trips; legacy fields are not written.
- `card`: `transparent` + `bg-opacity`; `tint` off; `shadow` 0 and 50; `text-scale` 120 on nested text; all of it on a Rust Widget too; blur cap vs `transparent`.
- `ui` / `anim`: factor 0 never reports animating; factor 1.6 stretches a 100 ms transition to 160 ms.
- `settings`: set and reset a style token at both scopes; a Theme row dropdown lists "Global (...)" first and picking it sends `name: None`.
- `platform`: `blur_window_attributes(false)` gives `DWMWCP_DEFAULT`, `(true)` gives `DWMWCP_ROUND`.
- `edit`: a resize stops at the max with the limit on and passes it with the limit off; the min holds either way.
- `widgets`: the min/max layout harness above.
- Visual: `examples/render_widgets` and `examples/render_settings`, on the software adapter, including each Widget at its max size.

## Out of scope

Window motion and layout transitions (sub-project 2). New widget layouts, size tiers and new data (sub-project 3). A per-Widget-kind style layer.
