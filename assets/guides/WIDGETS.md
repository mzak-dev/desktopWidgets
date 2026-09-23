# Making Wayfinder widgets

A widget is one `.toml` file in the `widgets` folder. Wayfinder watches this folder like it watches `palettes`, `fonts`, `glyphs` and `iconpacks`: save a file and it reloads a moment later. If a file fails to parse, its card shows the error instead of silently falling back, and the reason is written to `wayfinder.log`.

A file named after a built-in widget's id (`clock`, `digital_clock`, `drawer`, `icon_folder`, `icon_list`, `system_monitor`) **replaces** that built-in — including replacing it with an error card if the file is broken. Anything else adds a new widget of its own.

**With Claude Code:** open a terminal in this folder, run `claude`, and describe the widget you want. Point it at this file and at the built-in `.toml` files next to it (`clock.toml` is the most elaborate; `digital_clock.toml` and `drawer.toml` are simpler starting points) as examples of the format.

## A minimal widget

```toml
name = "Hello"
description = "A card that says hello."
size = [200, 100]

[root]
direction = "column"
align = "center"
justify = "center"
fill = ["$surface", "$surface-2"]
radius = "$radius-lg"
border = 1

  [[root.children]]
  type = "text"
  text = "Hello, {param.who}!"
  size = "$font-size-lg"
  color = "$text"

[params.who]
type = "string"
default = "world"
label = "Name"
```

The file name (minus `.toml`) becomes the widget's id. `name` is what's shown when adding a widget; `size` is its default card size in logical pixels. `$tokens` pull from the active theme (see `THEMES.md`) — the same palette, font and glyph tokens widgets have always had.

## Top-level keys

| Key | What it does |
|---|---|
| `name`, `description` | Shown in the widget picker. |
| `size` | `[width, height]`, the default card size. |
| `min_size`, `max_size` | Optional `[width, height]` resize limits. `max_size` must not be smaller than `min_size`. |
| `params` | The widget's settings — see below. |
| `state` | Initial values for things the widget itself changes at runtime (scroll position, an expanded flag). Defaults to empty. |
| `expand` | Optional: lets the card grow past its normal bounds (used by the clock's world-clock row). Rarely needed for a first widget. |
| `root` | The layout tree — every widget has exactly one. |

## The layout tree (`root` and `children`)

Every node (`root`, and everything under `[[*.children]]`) is a box by default (`type` omitted or `"box"`), or one of `text`, `image`, `hand`, `ticks`, `arc`, `graph`, or the structural `repeat`. Nodes lay out with the same flexbox model as CSS: `direction` (`row`/`column`/`row-reverse`/`column-reverse`), `wrap`, `align`, `justify`, `gap`, `grow`, `shrink`, `basis`.

Attributes every node accepts:

| Group | Attributes |
|---|---|
| Size | `width`, `height`, `min_width`, `min_height`, `max_width`, `max_height`, `aspect` |
| Flex | `direction`, `wrap`, `align`, `justify`, `align_self`, `gap`, `grow`, `shrink`, `basis` |
| Box model | `padding`, `margin` — a number, `[v, h]`, or `[top, right, bottom, left]` |
| Position | `position = "absolute"`, `inset`, `left`, `top`, `right`, `bottom` |
| Look | `fill` (a colour, or `[top, bottom]` for a gradient), `fill_alpha`, `border`, `border_color`, `radius`, `opacity`, `shadow`, `clip` |
| Interaction | `on_click`, `hover`, `transition`, `enter`, `scroll`, `hit` |
| Other | `id` (only needed if you must reference a node elsewhere) |

`shadow` is either a number (blur) or a table: `shadow = { blur = 16, dy = 5, color = "$shadow" }`. `hover` is a table of properties to swap in on hover: `hover = { fill = "$surface-hover" }`. `enter` animates a node in when it first appears: `enter = { ms = 220, dy = 6, stagger = 30 }` (`stagger` multiplies by the node's `repeat` index, for staggered lists).

### Node types and their own attributes

| Type | Attributes | Notes |
|---|---|---|
| `box` (default) | — | A container; draws `fill`/`border`/`radius`/`shadow` like anything else. |
| `text` | `text`, `size`, `color`, `font`, `weight`, `text_align`, `text_wrap`, `line_height` | `font` names a font-set token, e.g. `"$font-body"`. |
| `image` | `src`, `tint` | `src` is an icon id — usually `{item.icon_id}` from a `shortcuts` repeat, or `"file:C:\path\to.png"`. |
| `hand` | `angle`, `length`, `tail`, `stroke`, `color` | A clock hand from the node's centre; `angle` is degrees clockwise from 12 o'clock, `length`/`tail` are fractions of the node's half-size. |
| `ticks` | `count`, `major_every`, `tick_length`, `major_length`, `tick_width`, `major_width`, `color`, `major_color`, `tick_inset` | Clock-face tick marks. |
| `arc` | `value`, `start`, `sweep`, `stroke`, `color`, `track` | A gauge ring — `value` 0–100, `start`/`sweep` in degrees. |
| `graph` | `values`, `max`, `span`, `stroke`, `color`, `area` | A sparkline from a list of numbers (e.g. `sys.cpu_history`). |

`hand`, `ticks` and `arc` fill their parent by default if you don't give them a `width`/`height` — that's what makes `clock.toml`'s face work: size the parent box, and the hands/ticks/arc inside it follow automatically.

### `repeat`

Repeats its children once per item in a list:

```toml
[[root.children]]
type = "repeat"
for = "{shortcuts.items}"
as = "item"
  [[root.children.children]]
  type = "text"
  text = "{item.name}"
```

`for` must evaluate to a list. `as` names the loop variable (default `item`); `index` names the 0-based position (default `index`). Use `index` in a `when` to cap how many items render (`when = "{index < 5}"`), and in `enter`'s `stagger` for a staggered appear animation.

### `when`

Any node (not `root`) can take `when = "{expression}"`: the node (and everything under it) is skipped entirely when it's false. This is how widgets adapt to size — `digital_clock.toml`'s city row only appears `when = "{self.h >= 190 && ...}"`.

## Params

```toml
[params.show_seconds]
type = "bool"
default = false
label = "Show seconds"
help = "Updates every second instead of every minute."
```

Types: `bool`, `string`, `number` (`min`/`max`/`step` optional), `color`, `enum` (with `choices = ["a", "b"]`), `path`, `duration`, `shortcuts` (a user-editable app list — see `icon_list.toml`; `seed = "starter-apps"` pre-fills a new one with a few common apps). Read a param in the layout tree with `param.<name>`.

## Expressions

Anything in `{ }` is an expression, evaluated fresh whenever something it reads changes. Inside a plain string it interpolates: `text = "Hi {param.who}"`. A whole attribute can also be one expression: `size = "{self.w / 4}"`.

Available names:
- `self.w`, `self.h` — the card's current size.
- `param.<name>`, `state.<name>` — this widget's params and state.
- `clock.*` — `hour`, `hour12`, `minute`, `second`, `ms`, `is_pm`, `ampm`, `day`, `month`, `year`, `weekday`, `weekday_short`, `month_name`, `date`, `date_short`, `hour_angle`, `minute_angle`, `second_angle`, `second_smooth`, `zones` (from a widget's `cities` param — see `clock.toml`).
- `sys.*` — `cpu`, `ram`, `commit`, `disk`, `disk_text`, `disk_name`, `net_down`, `net_up`, `cpu_history`, `ram_history`, `net_history`, `gauges`. See `system_monitor.toml` for how these are used.
- `shortcuts.*` — `items` (a list of `{name, icon_id, target}`, from a `shortcuts` param), `count`.

These three (`clock`, `sys`, `shortcuts`) are the only data sources; a `.toml` widget can't add new ones — that takes Rust code.

Operators: `+ - * /`, comparisons (`== != > < >= <=`), `&& ||`, ternary `cond ? a : b`. Functions: `min`, `max`, `abs`, `round`, `floor`, `ceil`, `clamp(n, lo, hi)`, `len`, `pad(n, width)` (zero-pads). Inside a `{ }` interpolation you can also append a format spec after `|`: `{clock.minute|02}` (zero-padded width 2), `{sys.cpu|.1}` (1 decimal place).

Only read what you need — Wayfinder wakes and redraws a widget only when a value it actually reads changes, so reading `clock.hour` instead of the whole `clock` object keeps an idle widget idle.

## Actions

`on_click = "verb argument"` on any node. Built-in verbs: `launch <path>` (open a file, folder or shortcut target), `toggle <state-key>` (flips a boolean in `state`, e.g. an expanded section), `set <state-key> <value>`, `settings` (opens Wayfinder's Settings). A `.toml` widget can only use these four; a widget with its own verbs needs Rust code (like the built-in `drawer` widget).

## When it doesn't look right

- **The widget isn't listed:** the file failed to parse. Read `wayfinder.log`; parse errors name the exact key and usually suggest the correct spelling.
- **A card shows "Widget error":** same thing, but for a file that replaced a built-in — Wayfinder never silently falls back to the built-in version.
- **Something is magenta:** an undefined `$token`, or a colour value that didn't parse.
- **A value never updates:** check the expression actually reads the field that changes (e.g. `clock.second`, not just `clock`) — reads are also how Wayfinder knows when to redraw.
- **Sizing looks wrong at small/large sizes:** widgets should scale using `self.w`/`self.h` in expressions rather than fixed pixel values; see `clock.toml` for a widget with several size tiers.
