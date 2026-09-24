# Modules and slots: arranging a widget in Settings

A Widget may declare **Tiers**, **Slots** and **Modules**, and the user arranges the Modules per Tier on a live preview at the top of the Instance's page in Settings. The Arrangement is saved with the Instance as `layout` (tier -> slot -> Module ids).

**Ordered slots, not free placement.** The user reorders Modules inside Slots the author declared, moves them between Slots that accept them, or hides them in a tray. Taffy still lays everything out, so size tiers, wrapping and the engine's animation keep working, and a Plugin author gets a small vocabulary (`[tiers]`, `[slots]`, `[modules]`, a `slot` element) instead of a canvas. Free x,y or a grid would break flex sizing and could not be made safe for other people's Widgets.

**Drag and drop lives in Settings, not on the desktop.** Instance windows are transparent, click-through in places, and their Card is a move and resize surface in Edit Mode; a second mode inside the Card would fight it. The Settings preview is the same `build` with Modules as the only hit targets, so it works for any TOML Widget, Plugin ones included, without a Rust Widget.

**Tiers are named by the Widget.** A size tier used to be an anonymous `when`. Naming them (with a preview size) is what lets Settings offer a tab per tier and store an Arrangement under a stable key; the engine picks the current tier from the card size, and the first whose `when` holds wins.

**Opt-in, and old data survives.** A file without `[modules]` builds and edits as before. A Module may name the old `show_*` param it replaced (`legacy`), and a saved `false` becomes an Arrangement without it. New Modules from a Widget update reach an untouched Tier through its default, and land in the tray of a customised one, so an update never rearranges what the user did.

**Consequence.** Sizing that a Widget used to compute from how many gauges it had (long pasted expressions and an index cut-off) is now the slot's job: `max` says how many fit, the rest are left out and named in Settings, and each Module sizes itself with flex.
