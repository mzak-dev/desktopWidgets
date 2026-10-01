<!-- nav:start -->
<sub>[🧭 Wayfinder](../README.md) · **Using it** · [Widgets](widgets.md) · [Visualizer](visualizer.md) · [Make it yours](customizing.md) · [Workspaces](workspaces.md) · [Plugins](plugins.md) · [Write a widget](writing-widgets.md) · [How it works](architecture.md) · [Development](development.md)</sub>
<!-- nav:end -->

# 🖱️ Using Wayfinder

Moving widgets around, the Settings window, and what happens when monitors come and go. New here? Start with the [Quick start](../README.md#-quick-start).

<table>
<tr>
<td width="44%" valign="top">

## Edit layout

Press **`Ctrl` + `Shift` + `E`** (or use the tray menu) to enter Edit Mode. Every widget gets an outline and eight handles.

| | |
|---|---|
| Drag the body | move |
| Drag an edge / corner | resize, with live reflow |
| The × in the corner | remove (click twice) |
| `Shift` while dragging | ignore snapping |
| Arrow keys | nudge 1 px (`Shift`: 10 px) |
| `Ctrl` + `Z` | undo |
| `Esc` | finish |

Your arrangement is saved to `workspace.json`. Unplug a monitor and its widgets are **parked**: hidden but remembered, and back where they were when it returns.

</td>
<td width="56%" valign="top">

## Settings

<img src="img/settings_widgets_gallery.png" alt="Settings, adding a widget from the gallery" width="100%">

**Widgets** adds, removes and edits widgets: each one sits at the top with a tab per size, and its parts are dragged into place or hidden. **Workspaces**, **Appearance**, **Plugins**, **General** and a **Log** have their own pages.

</td>
</tr>
</table>

## Which GPU?

> [!IMPORTANT]
> **Which GPU?** Wayfinder defaults to the **integrated** GPU (`"gpu": "low"`). On the AMD machine it was developed on, selecting the dedicated GPU pinned one CPU core at 99% while idle with two or more widgets on screen, in a driver thread outside Wayfinder, whereas the integrated GPU idled at 0.00%. Widgets are tiny, so the integrated GPU is plenty. Change it in **Settings → General**, or set `"gpu": "high"` in `workspace.json`. `"software"` renders on the CPU. Details in [ADR-005](adr/0005-adapter-and-present-mode.md).

<!-- pager:start -->
<br>

---

<table width="100%"><tr><td><a href="../README.md">← Wayfinder</a></td><td align="right"><a href="widgets.md">🧩 The widgets →</a></td></tr></table>
<!-- pager:end -->
