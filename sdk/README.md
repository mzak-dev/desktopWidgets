# Writing plugin code for Wayfinder

A Wayfinder plugin can carry **Code Sources** (one `[code]`, or several `[[code]]` sharing its saved data): a Data Source written in Rust, compiled to WebAssembly and run sandboxed inside Wayfinder ([ADR-008](../docs/adr/0008-plugin-code.md)). The plugin's widgets stay TOML and bind to it like to `clock` or `sys`:

```toml
text = "{weather.temp}°"            # a value your code returned
on_click = "weather.refresh"         # an action your code handles
```

`sdk/examples/weather` is a complete plugin (Open-Meteo, no API key): Rust code, `plugin.toml` and a widget.

## 1. The crate

```toml
# Cargo.toml
[lib]
crate-type = ["cdylib"]

[dependencies]
wayfinder-plugin = { git = "https://github.com/mzak-dev/desktopWidgets" }

[profile.release]
opt-level = "s"
lto = true
strip = true
```

```rust
use wayfinder_plugin::*;

#[derive(Default)]
struct Weather;

impl Source for Weather {
    /// Called for each widget showing this source, when it first appears and again
    /// after `every(...)`. `cx.params` are that widget's settings.
    fn sample(&mut self, cx: &Cx) -> Sample {
        let url = format!("https://api.open-meteo.com/v1/forecast?latitude={}&longitude={}&current=temperature_2m", cx.number("latitude"), cx.number("longitude"));
        match http::get(&url).and_then(|r| r.json()) {
            Ok(v) => Sample::new(json!({ "temp": v["current"]["temperature_2m"] })).every(minutes(15)),
            Err(e) => Sample::error(e).every(minutes(2)),
        }
    }

    /// `on_click = "weather.refresh"` arrives as act("refresh", "", cx).
    fn act(&mut self, verb: &str, _arg: &str, _cx: &Cx) -> Act {
        if verb == "refresh" { Act::Resample } else { Act::Nothing }
    }
}

export_source!(Weather);
```

- **`sample`** returns an object: its fields are what `{weather.field}` reads. Wayfinder adds `loading` (true until the first answer) and `error` (your `Sample::error`, or why the code stopped).
- **`every(d)`** asks to be sampled again after `d`: at least 1 s, or 1 minute when the sample used the network, at most a day. Without it, a widget is sampled once, then again only after an action or a change to its settings.
- **`act`** handles `on_click = "<source>.<verb> <arg>"` and says what to sample again.
- **State**: your `Source` value lives while the module runs, but starts fresh (`Default`) after a crash. Keep per-widget state keyed by `cx.instance`, and anything that must last in `store`.
- **`http::get` / `http::request`**: HTTPS only, to hosts in `[code] net`. Redirects are handed back (`location`), never followed. No cookies, no Windows credentials. A burst of 20 requests, then one every 6 s, and 2 MB per answer.
- **`fs::{list, stat, read_text, read_range}`**: read-only, under the folders in `[code] fs_read` (`~/.claude`) and, per widget, the folder the user picked in a param named in `fs_read_params`. Paths are `~/…` or full paths. A read returns at most 1 MB of text (`read_range(p, -4096, 4096)` is a file's last 4 KB), a call can read 16 MB in all, and a folder lists at most 2000 entries. Links cannot lead out of a folder, and Wayfinder's own data folder is never readable.
- **`launch`**: a widget showing your values may `launch` only `https://` links, unless `[code] launch` lists URL schemes (`vscode`; never `file`, `shell`, `search-ms` or `ms-…`) or folders under home, inside which folders and documents open but programs and scripts never do.
- **`store::{get, set, remove}`**: up to 1 MB of text per plugin in `plugin-data/<id>.json`. It survives restarts and upgrades, and goes when the plugin is removed.
- **`log!`**: a line in `wayfinder.log` and Settings > Log (20 lines a minute).
- **Limits**: each call has a fuel budget (a few hundred million instructions) and 32 MB of memory. A module that crashes or runs out is restarted after a pause that doubles up to 30 minutes, and the Plugins page shows the error.

## 2. Build

```sh
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
```

Use `wasm32-unknown-unknown`, not a WASI target: Wayfinder provides no WASI, and refuses a module that asks for it.

## 3. Package

Copy `target/wasm32-unknown-unknown/release/<name>.wasm` into the plugin folder and name it in `plugin.toml`:

```toml
id = "weather"
name = "Weather"
version = "1.0.0"
author = "You"

[code]
module = "code/weather.wasm"
source = "weather"                 # lower-case letters, digits, _; not clock, sys, shortcuts, param, state, self, item or index
net = ["api.open-meteo.com"]       # exact hosts, or *.example.com for its subdomains
# fs_read = ["~/.claude"]          # folders under home it may read
# fs_read_params = ["folder"]      # params holding a folder the user picks
# launch = ["vscode", "~/.claude"] # what its widgets may open besides https:// links

[code.initial]                     # shown until the first sample
temp = 0
```

Put the folder in `%APPDATA%\Wayfinder\plugins\` to try it (rebuilding the `.wasm` restarts it), then zip the folder and rename it `weather.wfplugin` to share it. Installing asks the user first and names the hosts your code can reach.

## 4. Test

On your own machine the crate talks to a stand-in host, so `cargo test` works with no Wayfinder. `testing::files_at(dir)` serves `fs` from a folder your test made, with `~/` meaning `dir`:

```rust
#[test]
fn it_reads_the_temperature() {
    testing::answer_http(|_req| http::Response { status: 200, body: r#"{"current":{"temperature_2m":12.6}}"#.into(), ..Default::default() });
    let s = Weather.sample(&testing::cx("weather-1", json!({ "latitude": 59.9, "longitude": 10.7 })));
    assert_eq!(s.value().unwrap()["temp"], 12.6);
}
```

## The ABI, for other languages

Any language that compiles to core WebAssembly can do the same. Everything is JSON in linear memory; pointers are i32, and an i64 result is `ptr << 32 | len`.

- **Exports:**
  - `memory`
  - `wf_abi() -> i32`, which must return 1
  - `wf_alloc(len) -> ptr`, where the host writes each input
  - `wf_sample(ptr, len) -> i64`
  - optionally `wf_act(ptr, len) -> i64`
- **Imports (module `wf`):**
  - `log(ptr, len)`
  - `now_ms() -> i64`
  - `http(ptr, len) -> i32`
  - `fs(ptr, len) -> i32`
  - `store_get(ptr, len) -> i32` (-1 if missing)
  - `store_set(kptr, klen, vptr, vlen) -> i32` (0 ok, -1 over 1 MB; `vlen` 0 deletes)
  - `take(ptr)`: http and store_get park their result and return its length; `take` copies it to `ptr`.
- **Sample in:** `{"abi":1, "instance":"weather-1", "params":{…}, "now_ms":…, "local":{"year","month","day","hour","minute","second"}}`.
- **Sample out:** `{"value":{…}, "refresh_s":900}` or `{"error":"…"}`.
- **Act in:** the same, plus `verb` and `arg`.
- **Act out:** `{"resample":"this" | "all" | "none"}`.
- **File request:** `{"op":"list" | "stat" | "read", "path":"~/…", "offset":0, "max":1048576}`; the answer is `{"entries":[{"name","dir","size","modified_ms"}], "truncated"}`, `{"dir","size","modified_ms"}`, `{"text","offset","size","truncated"}` or `{"error":"…"}`.
- **HTTP request:** `{"method":"GET", "url":"https://…", "headers":{…}, "body":"…"}`.
- **HTTP response:** `{"status":200, "content_type":"…", "location":"…", "body":"…"}` or `{"error":"…"}`.
