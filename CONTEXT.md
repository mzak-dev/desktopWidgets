# Wayfinder

A desktop widget engine for Windows: transparent, GPU-composited widgets that live on the desktop.

## Language

**Widget**:
A *kind* of thing that can be placed — Clock, DigitalClock, IconFolder, IconList. Authored as a definition file and shareable.
_Avoid_: gadget, skin, plugin

**Instance**:
One configured, positioned, sized copy of a Widget on a desktop. What the user drags and resizes. Owns one OS window.
_Avoid_: widget (when you mean the placed copy)

**Workspace**:
The saved arrangement of Instances.
_Avoid_: layout (taffy owns that word), profile

**Theme**:
A named set of design tokens made of three independently swappable axes: a palette, a font set and a glyph set.
_Avoid_: skin, style

**Glyph Set**:
Icons for the engine's own chrome — gear, chevron, close.

**Icon Pack**:
Icons for launched applications, falling back to the executable's own embedded icon.
_Avoid_: theme, glyph set

**Edit Mode**:
The global state in which Instances show handles and can be dragged and resized.

**Data Source**:
A named producer of values that widget definitions bind to (`clock`, `shortcuts`).
_Avoid_: measure, plugin

**Z-mode**:
Where an Instance sits relative to other windows: Desktop, Bottom, Normal or Topmost.

**Park**:
To hide an Instance whose monitor is absent while remembering it, instead of relocating or deleting it.

## Relationships

- A **Widget** is instantiated as zero or more **Instances**; a **Workspace** is the set of all Instances.
- A **Widget** binds to one or more **Data Sources**; its appearance resolves through the active **Theme**.
- An **Instance** has exactly one **Z-mode** and is anchored to one monitor; if that monitor is absent it is **Parked**.
