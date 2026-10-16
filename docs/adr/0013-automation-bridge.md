# Automation Bridge for end-to-end tests and agents

Wayfinder draws every pixel of its widgets itself, so Windows' UI Automation sees one empty window per widget. Tests and agents need to find "the play button", not a coordinate, and to see an animation frame by frame.

Considered image matching (brittle against themes, fonts and scale), UI Automation providers per widget (a large surface to build and keep in step with a layout engine that already knows every node), and the selftest (it clicks by the same data, but only inside the process and with synthetic input).

So: the layout already produces what a tool needs, `ui::Frame` (hit regions with their actions, rects by key) and the draw list's texts. With `--automation`, in a build with the `automation` feature (off by default, so release builds and the installer do not change), `raywright-bridge` answers [Raywright](https://github.com/mzak-dev/Raywright) over a named pipe: windows, elements derived from those frames (`app/automation/describe.rs`, pure and unit-tested), a widget's own picture (the last draw list rendered off screen), a few commands (`app/automation/commands.rs`, one line each), and a manual animation clock. Requests are handed to the event loop as `UserEvent::Automation`, so nothing is read from another thread.

The animation clock (`anim::clock`) is the one change to normal code: widget animation time, window glides and animated pictures take `clock::now()` instead of `Instant::now()`. It returns the wall clock unless a bridge client stops it, and then moves only on `clock_step`, which draws every widget before it answers. Debounces, reload timers and Settings keep the wall clock. A recording of an animation is then the same frames on any machine, so its keyframes can be compared with stored pictures.

The bridge adds no user-facing behavior and no network surface: it listens on a pipe named for the process, only for the user who started it.

Raywright is a git dependency (`raywright-bridge`, optional) pinned to a commit; the tests are a separate package (`e2e/`) so Raywright and tokio never join Wayfinder's graph.
