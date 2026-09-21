# Z-order by continuous re-assertion, not reparenting

Rainmeter does not reparent to WorkerW; it repeatedly re-applies `SetWindowPos(HWND_BOTTOM)` and reacts to shell events. Phase 0 measured on Windows 11 build 26200:

- The desktop-icon host is `Progman` (`SHELLDLL_DefView`'s parent). Any code hardcoding `WorkerW` is wrong on 24H2+, so resolve it by looking for `SHELLDLL_DefView`.
- Plain `HWND_BOTTOM` lands a window directly *above* `Progman` and below every application window. Explicitly reordering the shell-owned host window gave an identical result, so we do not touch it.
- Under Show Desktop (Win+D) a `HWND_BOTTOM` window disappeared while the icons stayed visible. Desktop-mode Instances therefore need Show-Desktop handling (raise while the desktop is shown), driven by shell events rather than a timer, so an idle desktop costs nothing.

Not measured on this machine: fullscreen games, multi-monitor, monitor hot-unplug.
