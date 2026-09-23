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
A named producer of values that widget definitions bind to (`clock`, `sys`, `shortcuts`). It declares how often each of its fields can change, which is what lets an idle desktop cost nothing.
_Avoid_: measure, plugin

**Z-mode**:
Where an Instance sits relative to other windows: Desktop, Bottom, Normal or Topmost. Desktop and Bottom both sit under application windows; Desktop stays visible while the desktop is shown (Win+D), Bottom is hidden by it.

**Park**:
To hide an Instance whose monitor is absent while remembering it, instead of relocating or deleting it.

## Relationships

- A **Widget** is instantiated as zero or more **Instances**; a **Workspace** is the set of all Instances.
- A **TOML Widget** is a tree of **Elements**; a **Rust Widget** builds its tree in code or wraps a TOML Widget's.
- An **Instance** shows one **Card**; its params may be **Seeded** when it is added.
- A **Widget** binds to one or more **Data Sources**; its appearance resolves through the active **Theme**.
- An **Instance** takes the global **Style** and axes unless it overrides them; its own values win.
- An **Instance** has exactly one **Z-mode** and is anchored to one monitor; if that monitor is absent it is **Parked**.

## Commit messages

Conventional Commits, Angular style: `type(scope): short summary`, imperative, lower case, no trailing period, under ~50 characters. Types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`. Keep the body to a line or two, only when the why isn't obvious.

Example: `feat(drawer): add blur and tint options`
