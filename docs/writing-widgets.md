<!-- nav:start -->
<sub>[🧭 Wayfinder](../README.md) · [Using it](using.md) · [Widgets](widgets.md) · [Visualizer](visualizer.md) · [Make it yours](customizing.md) · [Workspaces](workspaces.md) · [Plugins](plugins.md) · **Write a widget** · [How it works](architecture.md) · [Development](development.md)</sub>
<!-- nav:end -->

# ✍️ Write a widget

A widget is one TOML file. Put it in `widgets/` in the data folder (`%APPDATA%\Wayfinder`), save it, and it appears in **Settings → Widgets → Add a widget**. Save again and it reloads at once; a mistake shows a red error card in place of the widget instead of breaking anything.

The built-in widgets in [`assets/widgets/`](../assets/widgets) are complete examples: copy one into your `widgets/` folder under the same name and your copy replaces it.

- [Your first widget](#your-first-widget)
- [Elements](#elements)
- [Attributes](#attributes)
- [Data you can bind to](#data-you-can-bind-to)
- [Sizes and modules](#sizes-and-modules)
- [Expressions](#expressions)
- [Options (params)](#options-params)

## Your first widget

```toml
name = "My Clock"
category = "Time"               # groups it in Settings > Add a widget
icon = "clock"                  # a glyph set's glyph-* name, shown beside it
size = [240, 120]
min_size = [160, 80]            # Edit Mode resizes within these;
max_size = [480, 240]           # a widget's "Size limit" switch lifts the max

[params.accent]                 # becomes a colour picker in Settings
type = "color"
default = "$accent"
label = "Accent"

[root]
fill = ["$surface", "$surface-2"]    # $tokens come from the active theme
radius = "$radius-lg"
align = "center"
justify = "center"

  [[root.children]]
  type = "text"
  text = "{pad(clock.hour, 2)}:{pad(clock.minute, 2)}"     # live binding
  size = "{min(self.h * 0.5, self.w * 0.2)}"               # scales with the window
  color = "{param.accent}"

  [[root.children]]
  when = "{self.h >= 200}"                                 # only when there is room
  type = "text"
  text = "{clock.date}"
  color = "$text-dim"
```

Three things make it work:

- **`{...}` bindings** are [expressions](#expressions) over live [data](#data-you-can-bind-to), your widget's [options](#options-params) (`param.*`), its own size (`self.w`, `self.h`) and its state (`state.*`). Wayfinder redraws only when what they read can change.
- **`$tokens`** come from the theme: `$accent`, `$surface`, `$text-dim`, `$radius-lg`, `$font-display`... Users restyle every widget at once by changing them; see [Make it yours](customizing.md#which-value-wins).
- **`[root]`** is a box laid out with flexbox, and its `children` nest as deep as you need.

## Elements

`type` picks what an element is; a box is the default. Each kind has the [shared attributes](#attributes) and its own:

| Element | Draws | Its own attributes |
|---|---|---|
| `box` | a container, laid out with flexbox | (only the shared ones) |
| `text` | text, shaped and wrapped | `text` `size` `color` `font` `weight` `text_align` `text_wrap` `line_height` |
| `image` | a picture or an animation | `src` `tint` `fit` `feather` `max` `anim` `frame` `fade` |
| `hand` | a clock hand from the centre | `angle` `length` `tail` `stroke` `color` |
| `ticks` | tick marks around a face | `count` `major_every` `tick_length` `major_length` `tick_width` `major_width` `color` `major_color` `tick_inset` |
| `arc` | a ring gauge | `value` `start` `sweep` `stroke` `color` `track` |
| `graph` | a line through `values`, `span` slots wide, from `min` to `max`, with an `area` under it as see-through as `area_opacity` | `values` `min` `max` `span` `stroke` `color` `area` `area_opacity` |
| `block` | a 3D block, `depth` px deep, fading from `color` to `color_bottom` | `depth` `color` `color_bottom` |
| `repeat` | one child per item of a list | `for` `as` `index` |
| `slot` | the box that arranged [modules](#modules) fill | `slot` `max` |

**Images.** `src` takes PNG, JPEG, WebP, GIF or BMP: `./logo.png` is next to the widget file, or a full path like `{item.target}`. `fit` is `contain` or `cover`. `feather` fades the edge over that many px, inside `radius`; a large `radius` on a square image is a circle. `max = 256` uses a small copy, made off the UI thread and cached in `.cache/thumbs`; use it for photo grids. `anim = false` stops a GIF, WebP or APNG, `frame` shows one frame, and `fade = 400` crossfades a new `src` in over the old one for that many ms.

## Attributes

| Group | Attributes |
|---|---|
| Layout (flexbox) | `direction` `wrap` `align` `justify` `gap` `padding` `margin` `width` `height` `min_*` `max_*` `grow` `shrink` `basis` `aspect` `position` `inset` |
| Look | `fill` (or `[top, bottom]` gradient) `border` `border_color` `radius` `shadow` `opacity` `clip` |
| Behaviour | `on_click` `on_drop` `on_slide` `hover` `transition` `enter` `scroll` `scroll_x` `when` |

**Clicks and drops.**

- `on_click` runs `launch <path>`, `toggle <state>`, `set <state> <value>` (numbers and `true`/`false` keep their type, `'quotes'` keep text) or `param <name> <value>`, which saves a setting. A data source can take actions too: `on_click = "media.play_pause"`.
- `on_drop` runs its action with a dropped file's path after it; `on_drop = "param folder"` saves it as the widget's `folder` setting.
- `on_slide` makes a bar to drag across. While dragging, `state.slide` is 0-1 and `state.sliding` is true; letting go runs the action with the fraction after it, `media.seek 0.42`. Empty turns it off: `on_slide = "{media.can_seek ? 'media.seek' : ''}"`.
- `scroll_x` scrolls sideways; the wheel over it writes `state.scroll_x`, and Shift+wheel does too.

`when = "{...}"` keeps an element only while it holds. An optional colour that evaluates to `nil` is left out: `area = "{on ? '$accent' : nil}"`. Unknown attributes are rejected with a suggestion, for example ``unknown attribute `colour` on `text` (did you mean `color`?)``.

## Data you can bind to

| Source | Fields |
|---|---|
| `clock.*` | hour, minute, second, `date`, angles for hands, and `clock.zones` for a `cities` param |
| `calendar.*` | the month around today: `weeks` (each `week`, the ISO number, and `days`: `day` `this_month` `today` `weekend`), `day_names` `day_letters` `month_name` `year` `day` `weekday` `week` |
| `sys.*` | `gauges`, `gauges_all`, `graphs`, `gpus` (one per adapter: `label` `value` `history`), `gpu_count`, `cpu_history`, `ram_history`, `net_history`, `net_down`, `net_up`, uptime |
| `shortcuts.items` | the widget's app shortcuts |
| `media.*` | what any app plays through the system media controls: `title` `artist` `album` `source` `playing` `can_seek` `art` `position` `duration` `progress` `clock` `length` `active`. Actions: `media.play_pause`, `media.next`, `media.prev`, and `media.seek <0-1>` from an `on_slide` bar |
| `audio.*` | what the speakers play, for [visualizers](visualizer.md): `bands` `peaks` `level` `bass`, the waveform `wave` (-1 to 1, starting where it rises through zero, scaled to fit), `history` (the bands of the last 1.4 s, a row every 60 ms, oldest first) and `active`. Shaped by the widget's `bands` `fmin` `fmax` `gain` `attack` `release` `peak_fall` `timebase` params |
| `gallery.*` | the pictures in the widget's `folder`: `items` (`name` `path` `ext` `size` `modified_ms` `date`), `count`, `folder_name`, `error`, `truncated`, and the slide a frame shows, `index` and `current`, which move on every `interval` seconds and with `gallery.next` / `gallery.prev`. `on_drop = "gallery.drop"` makes a dropped folder, or a dropped photo's folder, the widget's |
| `agents.*` | Claude Code, Copilot CLI and Antigravity CLI sessions for the widget's `provider`: `items` (`name` `cwd` `tool` `label` `working` `done` `age`), `count`, `working` |
| `param.*` | the widget's [options](#options-params) |
| `state.*` | the widget's own state, set by `toggle`, `set` and `on_slide` |
| `self.w` `self.h` | the card's size, for [sizes](#sizes-and-modules) |

Plugins add their own sources (`{weather.temp}`); see [Plugins](plugins.md#plugin-code).

## Sizes and modules

**Size tiers** are plain `when` conditions on `self.w` and `self.h`: show more when there is room, and the engine animates the change. The built-in widgets share four [size families](widgets.md#one-widget-four-sizes).

### Modules

Modules let the user arrange a widget in Settings: the widget on top, a tab per size, and the parts dragged between slots or into a Hidden tray. A file opts in by declaring what can move:

```toml
[tiers.compact]                       # first tier whose `when` holds; one without `when` is the fallback
size = [240, 130]                     # what Settings previews the tab at
when = "{self.w < 260}"
layout = { bars = ["gauge:cpu", "gauge:gpu*"] }   # the default: slot -> modules (`*` matches a prefix)
[tiers.normal]
layout = { gauges = ["gauge:cpu", "gauge:gpu*"] }
[slots.bars]
[slots.gauges]
[modules.gauge]                       # a Module is a box, or any element; `for` makes one per item
for = "{sys.gauges_all}"
as = "g"
key = "{g.key}"                       # ids are `gauge:<key>`
slots = ["bars", "gauges"]            # where it may be dropped
when = "{sys.has_battery}"            # optional: whether it exists at all
label = "{g.label}"
direction = "{slot == 'bars' ? 'row' : 'column'}"   # `tier` and `slot` are available inside
[root]
[[root.children]]
type = "slot"                         # the box the arranged modules fill
slot = "gauges"
when = "{tier != 'compact'}"
max = "{floor((self.w - 28) / 50)}"   # optional: how many fit; the rest are left out, not overflowed
```

A `[params.x]` with `module = "gauge:cpu,graph:cpu"` shows in Settings only while one of those modules is selected; without it, it is an option of the whole widget. `legacy = { "gauge:cpu" = "show_cpu" }` on a module turns an old saved `show_cpu = false` into a layout without it. Files without `[modules]` work as before. [`assets/widgets/system_monitor.toml`](../assets/widgets/system_monitor.toml) is the full example.

## Expressions

Expressions are total: arithmetic, comparison, `&&` `||` `!`, `? :`, strings and the functions `min` `max` `abs` `round` `floor` `ceil` `clamp` `len` `upper` `lower` `pad` `at` `clock`. There are no loops and no side effects, so evaluating one can never hang a redraw.

| | |
|---|---|
| `{expr}` | interpolate a value into text |
| `{expr\|02}` · `{expr\|.1}` | format: two digits, one decimal |
| `clock(secs)` | seconds as a player shows them, `3:07` or `1:02:03`: `{clock(state.slide * media.duration)}` under a seek bar |
| `at(list, i)` | an item by a computed position (negative counts from the end, past the end is nothing) or key, with a field after it: `at(gallery.items, state.selected).url` |
| `nil` | nothing: an optional colour set to it is left out |

## Options (params)

Every `[params.x]` becomes a control in **Settings → Widgets**, and its value is `param.x` in bindings:

| Key | Meaning |
|---|---|
| `type` | `color` · `font` · `number` · `enum` · `bool` · `string` · `folder` (a folder picker; also `path`) · `file` (a file picker) · `duration` · `shortcuts` |
| `default` | the starting value; a `$token` follows the theme |
| `label`, `help` | the control's name and the line under it |
| `min`, `max`, `step` | a number's range: a slider |
| `choices` | an `enum`'s values, plain strings or `{ value = "fast", label = "Fast (30 fps)" }` |
| `group` | params with the same `group = "Motion"` get their own heading |
| `module` | show it only while one of these [modules](#modules) is selected |
| `seed` | `seed = "starter-apps"` on a `shortcuts` param gives each new widget a few apps to start from; unlike `default`, it is written once and then edited like any other value |

**Needs.** `needs = ["weather"]` at the top of a widget names the data sources it cannot work without. When one is missing, the widget and Settings say which, instead of showing a blank card.

<!-- pager:start -->
<br>

---

<table width="100%"><tr><td><a href="plugins.md">← 🧩 Plugins</a></td><td align="right"><a href="architecture.md">⚙️ How it works →</a></td></tr></table>
<!-- pager:end -->
