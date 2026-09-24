//! One-shot commands for widget and plugin authors. Each prints to the console it was run
//! from and exits; none starts the desktop app or needs one running.
//!
//!   wayfinder --render-widget <id | file.toml> --png <out.png> [--size WxH] [--param k=v]...
//!             [--state k=v]... [--palette name] [--scale 1.25] [--time HH:MM] [--transparent]
//!             [--wait secs] [--data dir] [--gpu software|high|low]
//!   wayfinder plugin pack <folder> [out.wfplugin]
//!   wayfinder plugin check <file.wfplugin | folder>
//!
//! Exit codes: 0 fine, 1 the widget or plugin has problems, 2 bad arguments.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use crate::anim::Anim;
use crate::card::Card;
use crate::code::runtime::Limits;
use crate::code::{Deps, WasmSource};
use crate::content::{Catalog, Root};
use crate::data::{DataSources, Tm};
use crate::gfx::{Gpu, Power};
use crate::icons::IconService;
use crate::plugins::{self, PluginStore};
use crate::text::TextEngine;
use crate::theme::{Selection, Theme};
use crate::value::Value;
use crate::widgets::{Services, View, prepare};
use crate::workspace::{self, InstanceCfg};

pub const USAGE: &str = "usage:
  wayfinder --render-widget <id | file.toml> --png <out.png> [--size WxH] [--param k=v]... [--state k=v]...
            [--palette name] [--scale 1.25] [--time HH:MM] [--transparent] [--wait secs] [--data dir] [--gpu mode]
  wayfinder plugin pack <folder> [out.wfplugin]
  wayfinder plugin check <file.wfplugin | folder>";

#[derive(Debug, PartialEq)]
pub enum Command {
    Render(Render),
    Pack { dir: PathBuf, out: Option<PathBuf> },
    Check(PathBuf),
    Usage(String),
}

#[derive(Debug, PartialEq)]
pub struct Render {
    /// A widget id (built-in, a plugin's or the user's) or a widget file.
    pub widget: String,
    pub png: PathBuf,
    /// The card's size in logical px; the widget's default size without it.
    pub size: Option<(f32, f32)>,
    pub params: Vec<(String, serde_json::Value)>,
    pub state: Vec<(String, serde_json::Value)>,
    pub palette: Option<String>,
    pub scale: f32,
    pub time: Option<(u32, u32)>,
    pub transparent: bool,
    /// How long to wait for plugin code to answer.
    pub wait: f32,
    pub data: PathBuf,
    pub gpu: String,
}

/// `v` as JSON when it is (numbers, booleans, lists), else as text.
fn loose(v: &str) -> serde_json::Value {
    serde_json::from_str(v).unwrap_or_else(|_| serde_json::Value::String(v.to_string()))
}

fn pair(s: &str, what: &str) -> Result<(String, serde_json::Value), String> {
    let (k, v) = s.split_once('=').ok_or_else(|| format!("{what} `{s}`: write it as name=value"))?;
    Ok((k.trim().to_string(), loose(v)))
}

/// The command in `args` (without the program name), or None to run the app.
pub fn command(args: &[String]) -> Option<Command> {
    let usage = |e: String| Command::Usage(e);
    if args.first().map(String::as_str) == Some("plugin") {
        return Some(match (args.get(1).map(String::as_str), args.get(2)) {
            (Some("pack"), Some(dir)) => Command::Pack { dir: dir.into(), out: args.get(3).map(PathBuf::from) },
            (Some("check"), Some(p)) => Command::Check(p.into()),
            _ => usage("wayfinder plugin: pack <folder> or check <file>".into()),
        });
    }
    let i = args.iter().position(|a| a == "--render-widget")?;
    let parse = || -> Result<Render, String> {
        let widget = args.get(i + 1).filter(|w| !w.starts_with("--")).ok_or("--render-widget needs a widget id or file")?.clone();
        let mut r = Render { widget, png: PathBuf::new(), size: None, params: vec![], state: vec![], palette: None, scale: 1.25, time: None, transparent: false, wait: 5.0, data: workspace::data_dir(), gpu: "software".into() };
        let mut it = args.iter().enumerate().filter(|(j, _)| *j != i && *j != i + 1).map(|(_, a)| a);
        while let Some(a) = it.next() {
            let mut val = || it.next().cloned().ok_or_else(|| format!("{a} needs a value"));
            match a.as_str() {
                "--png" => r.png = val()?.into(),
                "--size" => {
                    let v = val()?;
                    let (w, h) = v.split_once(['x', 'X']).ok_or("--size is WxH, like 300x200")?;
                    r.size = Some((w.trim().parse().map_err(|_| "--size is WxH")?, h.trim().parse().map_err(|_| "--size is WxH")?));
                }
                "--param" => r.params.push(pair(&val()?, "--param")?),
                "--state" => r.state.push(pair(&val()?, "--state")?),
                "--palette" => r.palette = Some(val()?),
                "--scale" => r.scale = val()?.parse().ok().filter(|s: &f32| *s > 0.0 && *s <= 4.0).ok_or("--scale is a number from 0 to 4")?,
                "--time" => {
                    let v = val()?;
                    let (h, m) = v.split_once(':').ok_or("--time is HH:MM")?;
                    r.time = Some((h.parse().map_err(|_| "--time is HH:MM")?, m.parse().map_err(|_| "--time is HH:MM")?));
                }
                "--transparent" => r.transparent = true,
                "--wait" => r.wait = val()?.parse().map_err(|_| "--wait is seconds")?,
                "--data" => r.data = val()?.into(),
                "--gpu" => r.gpu = val()?,
                other => return Err(format!("unknown option `{other}`")),
            }
        }
        if r.png.as_os_str().is_empty() {
            return Err("--render-widget needs --png <out.png>".into());
        }
        Ok(r)
    };
    Some(parse().map_or_else(usage, Command::Render))
}

/// Runs a command and returns the process exit code.
pub fn run(cmd: Command) -> i32 {
    attach_console();
    match cmd {
        Command::Usage(e) => {
            eprintln!("wayfinder: {e}\n{USAGE}");
            2
        }
        Command::Pack { dir, out } => {
            let out = out.unwrap_or_else(|| {
                let id = plugins::describe(&dir).map(|(m, _)| m.id).unwrap_or_else(|_| "plugin".into());
                dir.parent().unwrap_or(Path::new(".")).join(format!("{id}.wfplugin"))
            });
            match plugins::pack(&dir, &out) {
                Ok(m) => {
                    println!("packed {} {} into {}", m.name, m.version, out.display());
                    let r = plugins::check(&out);
                    print_report(&r);
                    i32::from(!r.problems.is_empty())
                }
                Err(e) => {
                    eprintln!("wayfinder: could not pack {}: {e}", dir.display());
                    1
                }
            }
        }
        Command::Check(p) => {
            let r = plugins::check(&p);
            if let Some(m) = &r.manifest {
                println!("{} {} ({}){}", m.name, m.version, m.id, if m.author.is_empty() { String::new() } else { format!(" by {}", m.author) });
            }
            print_report(&r);
            if r.problems.is_empty() {
                println!("ok");
            }
            i32::from(!r.problems.is_empty())
        }
        Command::Render(r) => match render(&r) {
            Ok(errors) => i32::from(errors),
            Err(e) => {
                eprintln!("wayfinder: {e}");
                1
            }
        },
    }
}

fn print_report(r: &plugins::Report) {
    if !r.contents.is_empty() {
        println!("  holds {}", r.contents.summary());
    }
    for n in &r.notes {
        println!("  {n}");
    }
    for w in &r.warnings {
        println!("  warning: {w}");
    }
    for p in &r.problems {
        println!("  problem: {p}");
    }
}

/// The exe has no console of its own; print to the one it was started from.
fn attach_console() {
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE};
    unsafe {
        let redirected = GetStdHandle(STD_OUTPUT_HANDLE).is_ok_and(|h| !h.is_invalid() && !h.0.is_null());
        if !redirected {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

/// Renders one widget as the desktop would and writes a PNG. Returns whether the widget
/// showed an error.
fn render(r: &Render) -> Result<bool, String> {
    let plugin_list = PluginStore::new(&r.data).list();
    let mut roots = plugins::roots(&plugin_list, &Default::default());
    roots.push(Root::user(&r.data));
    let mut cat = Catalog::load(&roots);
    let file = Path::new(&r.widget);
    let id = if r.widget.ends_with(".toml") {
        let dir = file.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        cat.registry.load_dir(dir);
        file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    } else {
        r.widget.clone()
    };
    let def = cat.registry.get(&id).ok_or_else(|| format!("no widget `{id}`"))?.clone();
    for e in cat.registry.errors().into_iter().filter(|e| e.contains(&id)) {
        println!("warning: {e}");
    }

    let mut gpu = Gpu::new_headless(Power::parse(&r.gpu))?;
    let mut text = TextEngine::new();
    for e in text.sync_fonts(&cat.font_files) {
        println!("warning: {e}");
    }
    let mut icons = IconService::new(cat.icon_packs.clone());
    let sel = Selection { palette: r.palette.clone().unwrap_or_default(), ..Default::default() };
    let theme = Theme::compose(&cat.library, &sel, &[]);
    let card = Card::new(&theme);

    // plugin code runs as in the app, without the user's saved data
    let mut sources = DataSources::builtin();
    let (news_tx, news) = mpsc::channel();
    let news_tx = Mutex::new(news_tx);
    let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        let _ = news_tx.lock().unwrap().send(());
    });
    sources.set_waker(notify.clone());
    let (specs, _) = plugins::code_specs(&plugin_list, &Default::default());
    let fetch: Option<Arc<dyn crate::net::Fetch>> = match specs.iter().any(|(_, s)| !s.hosts.is_empty()) {
        true => crate::platform::winhttp::WinHttp::new().ok().map(|w| Arc::new(w) as Arc<dyn crate::net::Fetch>),
        false => None,
    };
    let places = crate::code::fs::Places { home: std::env::var_os("USERPROFILE").map(PathBuf::from), private: vec![r.data.clone()] };
    sources.sync_code(specs, |spec| WasmSource::start(spec, Deps { fetch: fetch.clone(), store: None, notify: notify.clone(), limits: Limits::default(), places: places.clone() }));

    let meta = match &def {
        Ok(w) => Some(w.meta().clone()),
        Err(_) => None,
    };
    let card_size = r.size.or(meta.as_ref().map(|m| m.default_card_size)).unwrap_or((200.0, 120.0));
    let size = card.window_size(card_size);
    let mut cfg = InstanceCfg { id: format!("{id}-1"), widget: id.clone(), w: size.0, h: size.1, ..Default::default() };
    if let Some(m) = &meta {
        m.seed_params(&mut cfg);
    }
    for (k, v) in &r.params {
        cfg.params.insert(k.clone(), v.clone());
    }
    let mut state: BTreeMap<String, Value> = meta.as_ref().map(|m| m.initial_state.clone()).unwrap_or_default();
    state.extend(r.state.iter().map(|(k, v)| (k.clone(), Value::from(v))));
    let now_tm = crate::data::now_local();
    let tm = match r.time {
        Some((hour, minute)) => Tm { hour, minute, second: 0, ms: 0, ..now_tm },
        None => now_tm,
    };

    let mut anim = Anim::default();
    let base = Instant::now();
    let frame = |at: Instant, gpu: &mut Gpu, text: &mut TextEngine, icons: &mut IconService, anim: &mut Anim| {
        let v = View { cfg: &cfg, state: &state, window_size: size, theme: &theme, icon_pack: "Default", tm, hover: None, scale: r.scale, now: at, card };
        let mut sv = Services { gpu, icons, text, anim, sources: &sources };
        prepare(&def, &v, &mut sv)
    };
    // enter animations start transparent: render once to start them, then after they settle
    let first = frame(base, &mut gpu, &mut text, &mut icons, &mut anim);
    if !sources.launch_rules(&first.deps).is_empty() {
        let deadline = Instant::now() + Duration::from_secs_f32(r.wait.max(0.0));
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            let _ = news.recv_timeout(left);
            let got = sources.take_news().iter().any(|(_, n)| n.all || n.changed.contains(&cfg.id));
            if got {
                break;
            }
        }
    }
    let p = frame(base + Duration::from_secs(2), &mut gpu, &mut text, &mut icons, &mut anim);
    for w in &p.warnings {
        println!("warning: {w}");
    }
    if let Some(e) = &p.error {
        println!("error: {e}");
    }
    let (pw, ph) = ((size.0 * r.scale).round() as u32, (size.1 * r.scale).round() as u32);
    let mut px = gpu.render_offscreen(pw, ph, &p.frame.list, &mut text)?;
    // premultiplied: over a backdrop, or back to straight alpha for a transparent PNG
    for (i, c) in px.chunks_exact_mut(4).enumerate() {
        let a = c[3] as f32 / 255.0;
        if r.transparent {
            if a > 0.0 {
                for k in 0..3 {
                    c[k] = (c[k] as f32 / a).min(255.0) as u8;
                }
            }
        } else {
            let (x, y) = ((i as u32 % pw) as f32 / pw as f32, (i as u32 / pw) as f32 / ph as f32);
            let bg = [30.0 + 70.0 * x, 60.0 + 50.0 * (1.0 - y), 120.0 + 60.0 * y];
            for k in 0..3 {
                c[k] = (c[k] as f32 + bg[k] * (1.0 - a)).clamp(0.0, 255.0) as u8;
            }
            c[3] = 255;
        }
    }
    let img = image::RgbaImage::from_raw(pw, ph, px).ok_or("the renderer returned the wrong size")?;
    img.save(&r.png).map_err(|e| format!("{}: {e}", r.png.display()))?;
    println!("rendered {id} at {:.0}x{:.0} to {}", card_size.0, card_size.1, r.png.display());
    Ok(p.error.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn plain_runs_are_not_commands() {
        assert_eq!(command(&args(&[])), None);
        assert_eq!(command(&args(&["--data", "D:\\wf", "--edit"])), None);
    }

    #[test]
    fn plugin_commands_parse() {
        assert_eq!(command(&args(&["plugin", "pack", "sunset"])), Some(Command::Pack { dir: "sunset".into(), out: None }));
        assert_eq!(command(&args(&["plugin", "pack", "sunset", "out.wfplugin"])), Some(Command::Pack { dir: "sunset".into(), out: Some("out.wfplugin".into()) }));
        assert_eq!(command(&args(&["plugin", "check", "x.wfplugin"])), Some(Command::Check("x.wfplugin".into())));
        assert!(matches!(command(&args(&["plugin", "zip"])), Some(Command::Usage(_))));
    }

    #[test]
    fn render_options_parse() {
        let Some(Command::Render(r)) = command(&args(&["--render-widget", "clock", "--png", "o.png", "--size", "300x200", "--param", "title=Quick launch", "--param", "smooth=true", "--state", "selected=2", "--time", "15:42", "--transparent"])) else { panic!() };
        assert_eq!((r.widget.as_str(), r.png, r.size, r.time, r.transparent), ("clock", PathBuf::from("o.png"), Some((300.0, 200.0)), Some((15, 42)), true));
        assert_eq!(r.params, [("title".to_string(), serde_json::json!("Quick launch")), ("smooth".to_string(), serde_json::json!(true))]);
        assert_eq!(r.state, [("selected".to_string(), serde_json::json!(2))]);
        for bad in [&["--render-widget", "clock"][..], &["--render-widget", "clock", "--png", "o.png", "--size", "big"], &["--render-widget", "clock", "--png", "o.png", "--nope"], &["--render-widget", "--png", "o.png"]] {
            assert!(matches!(command(&args(bad)), Some(Command::Usage(_))), "{bad:?}");
        }
    }
}
