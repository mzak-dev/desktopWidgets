# Tray menu drawn by Wayfinder

## Problem

Right-clicking the tray icon shows muda's Win32 popup (`TrackPopupMenu`): grey, classic, ignores the Theme, and its only animation is the system fade. Everything else Wayfinder shows is drawn by its own engine and moves with its own clock.

## Goal

The tray menu is drawn by Wayfinder (same element tree, layout, renderer and `Anim` as Settings), follows the active Workspace's Theme, and moves: it builds out of the tray, a highlight glides between rows, the Workspace list expands in place.

## Decisions

- **One reusable popup window** (`MenuWin`), not muda. Borderless, transparent, topmost, no taskbar button. Created on the first right-click, hidden on close, reused, so only the first open builds a swapchain.
- **The window never resizes.** It is sized for the tallest menu (Workspaces expanded); the card inside grows and shrinks. Resizing per frame would make the window edge trail the card (ADR-0006).
- **No acrylic blur.** DWM blurs the whole window rectangle, which is larger than the card. The card is solid (98% alpha) with a shadow, like the Settings dropdowns.
- **Workspaces expand inline**, not as a cascading submenu.
- **Same action ids as today** (`edit`, `settings`, `ws:{i}`, `wsmanage`, `reload`, `folder`, `quit`), fed to the existing `UserEvent::Menu` handler. No command logic changes.
- **No screen-reader support.** muda's menu was readable by Narrator; this one is not, like the Settings window. Full keyboard navigation stays.

## Structure

- `src/menu.rs` (new): `MenuWin` (window, target, anim, hover, frame, `MenuState`), shaped like `SettingsWin`; a pure `build(&MenuState, &Ctx) -> Node`; `event(&WindowEvent) -> Vec<MenuOut>` where `MenuOut` is a picked id or close.
- `src/settings.rs`: `Kit` and the helpers the menu uses become `pub(crate)`.
- `src/app/tray.rs`: `tray_menu()` and every `Menu`/`Submenu`/`CheckMenuItem` go. `TrayIconBuilder` drops `with_menu`. A right-click `Click { button: Right, button_state: Up, position, .. }` sends `UserEvent::TrayMenu(position)`; left-click still opens Settings. `refresh_tray` only invalidates an open menu: it is rebuilt from state each frame, so the "put the tick back" workaround goes.
- `src/app/mod.rs`: routes the menu window's events and redraws, like Settings; `TrayMenu` opens the menu, or closes it if open.

## Menu content

Edit layout (`edit` glyph, `Ctrl+Shift+E` hint right) · Settings… (`gear`) · Workspace: *active* (`grid`, chevron) → when expanded: one row per Workspace, the active one with an accent `check`, then Manage workspaces… · separator · Reload widgets and themes (`refresh`) · Open widgets folder (`folder`) · separator · Quit Wayfinder (`power`).

## Look

From the active Theme through `Kit`: card fill `bg` lerped 5% to `text` at 98% alpha, 1 px `line` border, shadow 18/6, corner radius from the Style's roundness. About 260 logical px wide, 32 px rows, glyph 14 px in `text-dim`, label 13 px, hint 11 px `text-dim`. Separators are 1 px `line` with 4 px vertical margin.

## Placement and dismissal

- Opens at the click point on that monitor, at its DPI, clamped to its work area. The card is anchored to the edge nearest the taskbar (bottom taskbar: card bottom-aligned at the cursor, grows upward).
- `SetForegroundWindow` on open so a click elsewhere takes focus.
- Closes on focus loss, Esc, a second right-click on the tray, or after a pick.

## Animation

All declarative through `Anim`, scaled by `anim-speed` (`off` = instant); a still menu draws no frames.

| Moment | Motion |
|---|---|
| Open | Card fades in and slides 8 px from the taskbar side, 180 ms ease-out. Rows `enter` 14 ms apart (fade + 4 px slide). |
| Hover | One shared pill (accent at low alpha) behind the rows; its `offset` eases 140 ms to the hovered row, so it glides. Fades out when nothing is hovered. Keyboard moves the same pill. |
| Expand | Chevron right ↔ down crossfades (the glyph's key changes, `enter` replays). Card height and the rows below glide via the reflow `follow` (the card sits at depth ≥ 2). Workspace rows stagger in. Collapse: they go, the card glides shorter. |
| Pick | Pill flashes to the accent; the action runs at once; the card fades and drifts out 120 ms ease-in; the window hides when nothing is animating. |

## Keyboard

Up / Down move (wrapping, skipping separators), Enter / Space pick, Right expands and Left collapses Workspaces, Esc closes. Hover and keyboard share one cursor.

## Testing

- Unit tests in `menu.rs`, no GPU: collapsed has no Workspace rows; expanded has one per Workspace with the active one ticked; Down + Enter returns the expected id; Enter on the Workspace header expands without closing; Esc closes.
- `examples/render_settings.rs` renders the menu collapsed and expanded to PNG on the software adapter.
- Live check on the real GPU is run by the user, not by Claude (the AMD driver crashed under GPU tests before).

## Risks

- **Foreground on open.** If Windows refuses `SetForegroundWindow` from the tray callback, focus loss never fires and the menu stays up. Verified in the live check; the fallback is calling it from inside the tray event handler, which runs while Explorer's grant is fresh.
