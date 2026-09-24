#![windows_subsystem = "windows"]
//! Wayfinder: desktop widgets for Windows. See CONTEXT.md and docs/adr/.
//!
//!   wayfinder                     run (tray icon, widgets on the desktop)
//!   wayfinder --data <dir>        use a different data directory
//!   wayfinder --exit-after <sec>  quit by itself (automated runs)
//!   wayfinder --gpu <mode>        high | low | software (this run only)
//!   wayfinder --edit              start in Edit Mode
//!   wayfinder --selftest          drive the interactive paths with synthetic input and report
//!   wayfinder --install <file>    install a .wfplugin (what double-clicking one runs)
//!
//! Commands for authors, which print and exit (see `cli.rs`):
//!
//!   wayfinder --render-widget <id | file.toml> --png <out.png> [--size WxH] [--param k=v]...
//!   wayfinder plugin pack <folder> [out.wfplugin]
//!   wayfinder plugin check <file.wfplugin | folder>
//!
//! An app built on Wayfinder does the same with its own Data Sources:
//! `let mut o = Options::from_args(); o.extra_sources.push(Box::new(Mine)); wayfinder::run(o)`.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(cmd) = wayfinder::cli::command(&args) {
        std::process::exit(wayfinder::cli::run(cmd));
    }
    wayfinder::run(wayfinder::Options::from_args());
}
