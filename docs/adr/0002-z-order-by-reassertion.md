# Z-order: Rainmeter's sentinel mechanism, not reparenting and not a heuristic

Widgets are ordinary top-level windows kept on the desktop layer by z-order, the way Rainmeter does it (read from `Library/System.cpp` and `Library/Skin.cpp` on `master`). Nothing is reparented into the shell.

## What Rainmeter actually does, and what we copy

- **A hidden sentinel window pinned at `HWND_BOTTOM`** ("System"). In the normal state it sits *above* the desktop-icon host, because the shell never lets a window go below its own. Show Desktop works by Explorer raising the host above everything, so **"the sentinel is now below the host" is exactly Show Desktop**. Rainmeter asks this with `FindWindowEx(nullptr, host, "RainmeterSystem", "System")`, which searches only windows below the host. We ask the same question with a z-order comparison (`is_desktop_shown`, unit-tested on synthetic orders). An earlier version here guessed from "are any application windows open"; that was wrong (an empty desktop is not Show Desktop) and is gone.
- **Finding the host.** From `GetShellWindow()`, which must be `Progman`. On Windows 11 24H2+ the host is Progman itself (`SHELLDLL_DefView` is its child); before that it is a visible `WorkerW` of the shell's process that contains `SHELLDLL_DefView`. Rainmeter chooses by probing for `GetCurrentMonitorTopologyId`; we look at the structure directly, which needs no version test.
- **Two z-modes with different Show Desktop behaviour.** *Desktop* widgets stay visible while the desktop is shown: they go to the bottom of the topmost band, directly under the backmost foreign topmost window above the raised host (so the taskbar still draws over them), trying successive candidates because a candidate can refuse. *Bottom* widgets stay under the desktop and are hidden by it, by definition. Both drop back to `HWND_BOTTOM` afterwards. Normal and Topmost are not ours to manage.
- **A veto guard.** For Desktop and Bottom widgets a subclass adds `SWP_NOZORDER` to `WM_WINDOWPOSCHANGING`, so nothing else can re-order them. Our own calls pass `SWP_NOSENDCHANGING` (Rainmeter's `ZPOS_FLAGS`) and so never meet the veto.
- **Triggers.** Rainmeter hooks `EVENT_SYSTEM_FOREGROUND` and also polls every 250 ms (100 ms while shown). We hook foreground and minimise events and run a short retry ladder (4, 20, 60, 140, 300, 700 ms) after each, because Explorer reorders a few ms *after* the event, plus a 250 ms check only while the desktop is actually shown. An idle desktop costs nothing.

## Differences from what we assumed before reading the source

- The `WM_WINDOWPOSCHANGING` rewrite `hwndInsertAfter = GetBackmostTopWindow()` applies only to *Normal-stays-desktop* skins. For On Desktop and Bottom skins the handler is the `SWP_NOZORDER` veto above.
- Rainmeter's "PositioningHelper" window is a second hidden window, created DPI-unaware "to reproduce the legacy DPI-unaware SetWindowPos coordinate mapping". It exists to place skins relative to it in a group and keep their order; we position each widget directly, so we do not need it and do not inherit the DPI quirk.
- Plain `HWND_BOTTOM` puts a window directly above Progman, never below it: the shell keeps its own window at the bottom. That is why the sentinel test works at all.

## Verified, and how

Detection and response are tested without asking Explorer to hide anything: a fake host window is raised and lowered around the real sentinel, and the Show Desktop response is forced. On the development machine that confirmed the detection flips `false, true, false`, Desktop widgets become topmost and Bottom widgets do not, the guard vetoes a foreign raise while our own calls pass, and floating widgets sit directly under the backmost foreign topmost window and above the raised host.

Two facts learned from those runs: hidden windows are not held to the topmost-band ordering, so a fake host must be visible to be a fair stand-in; and Windows demotes the taskbar below normal windows while a fullscreen app is in front, so the taskbar's z-index is not a valid yardstick.

Not verified: a real Win+D against Explorer, fullscreen games, multiple monitors, Explorer restarting.
