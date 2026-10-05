# Behind the desktop icons: reparented into Explorer's icon layer

Users asked for widgets under the desktop icons, like a live wallpaper. ADR-0002 keeps every widget a top-level window ordered by z-order, and z-order cannot do this: the shell keeps its own windows at the bottom, so `HWND_BOTTOM` lands *above* the icon host, never below it. The only way under the icons is to become a child of Explorer's own desktop windows, which is what Lively and Wallpaper Engine do. So this one setting reparents into the shell, and ADR-0002 still holds for everything else.

## How

- **The setting is a Style token, `behind-icons`.** That makes it global in Appearance → Surface and overridable per Instance in its Advanced → Style, with Reset, for free. It applies to the Desktop and Bottom Z-modes only: a widget the user put on Normal or Always on top stays there.
- **Finding the layer** (`win32::icon_layer`). Send Progman the undocumented `0x052C` (`wParam 0xD`, `lParam 1`), which gives the wallpaper a WorkerW of its own and is a no-op once it has. Then look at the structure, like `desktop_icon_host`, with no version test: on Windows 11 24H2+ `SHELLDLL_DefView` is Progman's child, and widgets become Progman's children directly under it, with Progman's wallpaper WorkerW moved under them so it cannot paint over them. Before 24H2 the icons live in a top-level WorkerW and the wallpaper in another directly behind it, and widgets become children of that one.
- **Position** stays in screen pixels everywhere. `set_rect` maps to the parent's client area when the window has a parent, and moving in or out keeps the window where it was on screen.
- **Presentation.** After a move the window's swapchain is rebuilt the same way as after a GPU loss (ADR-0006), in case its DirectComposition target is bound to where the window was. If that fails, the widget goes back in front of the icons, and the failure is logged.

## What it costs, accepted

- **Look only.** The icon window covers the whole desktop and takes the mouse, so clicks on a widget behind it go to the desktop (selecting icons, the desktop menu). The window is made click-through to say so, which also gives it `WS_EX_LAYERED`. Passing clicks on would need a low-level mouse hook running on every mouse move system-wide, which breaks "free when idle", so it is not done.
- **Edit Mode lifts widgets out** to top-level windows, so they can be dragged, and puts them back when it ends.
- **No blur.** DWM's accent blur works on top-level windows only, so `Card` treats Blur as off while `behind-icons` is on, and the shadow gutter stays.
- **No DPI messages.** A child window never gets `WM_DPICHANGED`, so a widget behind the icons draws at its monitor's scale, not at winit's cached one.
- **Show Desktop** leaves these widgets alone: they are part of the desktop that Win+D shows.
- **Explorer restarting** takes the icon layer down, and these windows with it. A destroyed widget window is opened again; `TaskbarCreated` (caught on the sentinel, which stays top-level, together with the display broadcasts that used to be caught on the first widget window) moves any survivors into the new layer, with retries at 1, 3 and 8 s because Explorer builds the desktop after the taskbar.

## Verified, and how

Compiled for `x86_64-pc-windows-gnu`, and the unit tests run under Wine: the ones Wine can run give the same results as before the change (a few fail or crash under Wine either way), and the new ones pass (the order check behind `is_behind_icons`, and no blur behind the icons). `--selftest` gains seven checks: in and out of the layer through the Style command, the same place on screen, click-through, a render target after the move, and Edit Mode lifting the widget out and putting it back.

Not verified: any of it against a real Explorer. In particular whether DirectComposition content shows in a child of Explorer's windows on 24H2, which changed this window tree and which wallpaper apps had to rework for; whether the 24H2 wallpaper WorkerW stays under the widgets when the wallpaper changes; Explorer restarting; mixed DPI.
