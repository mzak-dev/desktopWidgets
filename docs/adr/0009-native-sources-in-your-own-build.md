# Native data sources in your own build, not in DLLs

Some widgets need what a sandbox cannot give: the system media session (SMTC), audio loopback, thumbnails decoded on a thread. ADR-0007 rules out native plugins loaded as DLLs, because a crash would take every widget down and Rust has no stable ABI to load against. Instead the engine is a library: `wayfinder::run(Options)` is the whole app, and an exe built on it passes its own Data Sources in `Options.extra_sources`. They sit next to the built-ins, and TOML widgets bind to them the same way. Such a build compiles against one engine version, so there is no ABI to keep stable, and the person installing it chooses to run a different program, not a plugin.

The `DataSource` trait gives a native source what Code Sources already had:

- **`act(verb, arg, cx)`**: `on_click = "media.play_pause"` reaches it.
- **`attach(Notifier)`**: a thread says "I changed", for every Instance or one, and only the Instances that read the source redraw. There is no polling, and an idle desktop still costs nothing (ADR-0004). `set_param` saves an Instance's setting as if the user had set it, so a folder dropped on a gallery survives a restart. Code Sources get no Notifier: plugin code never changes params.
- **`cadence(field, cx)`**: asked after every redraw, per Instance, so it follows the source's state: a progress bar ticks each second while playing and sleeps while paused.
- **`retain(live)`**: per-Instance state goes when an Instance is removed.
- **`watched_paths(cx)` and `path_changed`**: a source watches any path param. A change to one redraws; it does not reload content.

Two builds may be installed side by side, so `.wfplugin` files go to the first one that starts. Neither takes them from the other while its exe still exists; one whose exe is gone holds nothing. Settings → General switches them on request. Otherwise each start would rewrite the association.

A widget that depends on such a source declares it (`needs = ["media"]`), so a stock Wayfinder shows "needs the `media` data source" instead of a blank card. Sources that belong to everyone move into the engine as built-ins after a code review; `media` (SMTC) and `audio` (WASAPI loopback) have. Neither costs anything unread: the media thread starts at the first read and then only wakes on SMTC events, and audio capture runs only while a widget reads it or waits through silence for sound.
