# Wayfinder

A desktop widget engine for Windows: transparent, GPU-composited widgets that live on the desktop.

## Language

**Widget**:
A *kind* of thing that can be placed — Clock, DigitalClock, IconFolder, IconList, Drawer. Either a TOML Widget or a Rust Widget.
_Avoid_: gadget, skin, plugin

**TOML Widget**:
A Widget authored as a definition file: shareable, hot-reloaded, for simple Widgets whose behaviour the format can declare.
_Avoid_: skin, template

**Rust Widget**:
A Widget implemented in code, for behaviour a definition file cannot declare. It may still draw its tree from a definition file of the same id (the Drawer does), so restyling it stays a file edit.
_Avoid_: native widget, plugin

**Element**:
A building block of a definition file's tree: box, text, image, hand, ticks, arc, and the structural repeat.
_Avoid_: node (the engine's laid-out tree), component

**Seed**:
A param value written once when an Instance is added (starter apps for a shortcut list). Unlike a default, it is saved and shows in Settings.
_Avoid_: default, preset

**Instance**:
One configured, positioned, sized copy of a Widget on a desktop. What the user drags and resizes. Owns one OS window.
_Avoid_: widget (when you mean the placed copy)

**Card**:
The visible body of an Instance inside its window. The window adds a transparent shadow gutter around the card, except while the desktop behind it is blurred.
_Avoid_: frame, body, panel

**Workspace**:
The saved arrangement of Instances.
_Avoid_: layout (taffy owns that word), profile

**Theme**:
A named set of design tokens made of three independently swappable axes: a palette, a font set and a glyph set.
_Avoid_: skin

**Style**:
The Theme tokens `assets/style.toml` declares: accent, roundness, outlines, blur, transparency, tint, shadow, text scale, animation speed. Set globally, overridable per Instance, applied by the engine to every Card.
_Avoid_: tweaks, overrides (for the whole set)

**Size Tier**:
A range of card sizes over which a Widget shows the same content; crossing into the next one adds or drops detail (city clocks, graphs, labels), not just scale. Declared with `when` on `self.w` / `self.h`.
_Avoid_: breakpoint, mode

**Size Limit**:
A Widget's maximum card size. An Instance may switch it off to grow larger; the minimum always applies.

**Glyph Set**:
Icons for the engine's own chrome — gear, chevron, close.

**Icon Pack**:
Icons for launched applications, falling back to the executable's own embedded icon.
_Avoid_: theme, glyph set

**Edit Mode**:
The global state in which Instances show handles and can be dragged and resized.

**Data Source**:
A named producer of values that widget definitions bind to (`clock`, `sys`, `shortcuts`). It declares how often each of its fields can change, or says through its Notifier when it changed, which is what lets an idle desktop cost nothing. It may also handle action verbs (`media.play_pause`). Built-ins, Code Sources and native sources an app built on Wayfinder registers (ADR-0009) are all Data Sources.
_Avoid_: measure, plugin

**Z-mode**:
Where an Instance sits relative to other windows: Desktop, Bottom, Normal or Topmost. Desktop and Bottom both sit under application windows; Desktop stays visible while the desktop is shown (Win+D), Bottom is hidden by it.

**Park**:
To hide an Instance whose monitor is absent while remembering it, instead of relocating or deleting it.

**Plugin**:
A package of content (Widgets, palettes, font sets, glyph sets, Icon Packs), and optionally one Code Source, that is installed, switched off and on, and removed as one unit. Its folder under `plugins/` has the data folder's layout plus a `plugin.toml` manifest; it is shared as a `.wfplugin` file, a zip of that folder.
_Avoid_: add-on, extension, skin, package (for the installed folder)

**Code Source**:
A Data Source whose values come from a Plugin's WebAssembly module (written in Rust with the `wayfinder-plugin` crate), run sandboxed on its own thread. The Plugin's Widgets stay TOML Widgets that bind to it (`{weather.temp}`) and send it actions (`weather.refresh`).
_Avoid_: Rust Widget (that is engine code, like the Drawer), script, native plugin

**File access**:
What a Code Source may read: folders under the home folder its `plugin.toml` declares (`fs_read`), and the folder the user picked in a param it names (`fs_read_params`). Read-only, shown when installing.

**Plugin data**:
What a Plugin's code saves between runs, up to 1 MB, in `plugin-data/<id>.json`. It survives upgrades and goes when the Plugin is removed.

**Content root**:
A folder laid out like the data folder that content is read from: each enabled Plugin, then the data folder itself, after the built-ins. A later root wins, so a Plugin may restyle a built-in Widget and the user's own file still beats the Plugin's.

**Catalog**:
Everything the engine can show once every content root is read, and which root each piece came from.

## Relationships

- A **Widget** is instantiated as zero or more **Instances**; a **Workspace** is the set of all Instances.
- A **TOML Widget** is a tree of **Elements**; a **Rust Widget** builds its tree in code or wraps a TOML Widget's.
- An **Instance** shows one **Card**; its params may be **Seeded** when it is added.
- A **Widget** binds to one or more **Data Sources**; its appearance resolves through the active **Theme**.
- An **Instance** takes the global **Style** and axes unless it overrides them; its own values win.
- An **Instance** has exactly one **Z-mode** and is anchored to one monitor; if that monitor is absent it is **Parked**.
- A **Plugin** is a **Content root** while it is on, and runs its **Code Source** while it is on. An **Instance** whose Widget only a switched-off Plugin provides is hidden like a Parked one, and shows again when the Plugin is back on.

## Commit messages

Conventional Commits, Angular style: `type(scope): short summary`, imperative, lower case, no trailing period, under ~50 characters. Types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`. Keep the body to a line or two, only when the why isn't obvious.

Example: `feat(drawer): add blur and tint options`
