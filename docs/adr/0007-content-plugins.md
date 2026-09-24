# Content plugins, unpacked into the data folder

A Plugin is a folder with the data folder's layout (`widgets/ palettes/ fonts/ glyphs/ iconpacks/`) and a `plugin.toml`. It is shared as a `.wfplugin` file, which is a plain zip of that folder, and installing it unpacks it into `plugins/<id>/`. Loading straight from the zip would need a virtual file system under every loader (fonts, images, Icon Pack lookups) and would lose hot reload. Unpacking keeps one loader for built-ins, Plugins and the user's own files: each is a content root, read in the order built-ins < Plugins (by id) < the data folder, and a later root wins.

Widget ids stay flat. A Plugin may therefore restyle a built-in Widget, and a user's copy in `widgets/` still beats the Plugin's. Two Plugins with the same id collide; the later one wins and the Plugins page says so. Namespacing (`sunset:clock`) would rule out restyling built-ins and would change the file a user copies to override one.

An archive may hold `toml`, fonts (`ttf otf ttc otc`), images (`png jpg jpeg gif webp bmp`), `md txt`, a `.wasm` module for its code (ADR-0008) and licence or readme files, nothing else, because the `launch` verb opens files with the shell, and `launch` also refuses any target inside `plugins/`. Native code (DLLs) is ruled out: it would run with the user's rights inside the desktop process, where one crash takes every widget down, and Rust has no stable ABI to load it against.

Code runs as WebAssembly behind the Data Source seam: a module produces values and handles action verbs, while its Widgets stay TOML, the way the Drawer wraps its own definition. ADR-0008 records how.

Installing unpacks next to the data folder (`<data>.install-…`) and renames the result in, so the folder watcher never sees half a Plugin and an upgrade either replaces the old version whole or leaves it untouched. Fonts are read into memory rather than memory-mapped, because Windows refuses to delete a mapped file, and that would make Remove fail while a Plugin's font is loaded.
