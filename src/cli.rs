//! One-shot commands for widget and plugin authors. Each prints to the console it was run
//! from and exits; none starts the desktop app or needs one running.
//!
//!   wayfinder --render-widget <id | file.toml> --png <out.png> [--size WxH] [--param k=v]...
//!             [--state k=v]... [--palette name] [--scale 1.25] [--time HH:MM] [--now ISO]
//!             [--env key=value]... [--real seams] [--installed] [--transparent] [--wait secs]
//!             [--data dir] [--gpu software|high|low]
//!   wayfinder plugin pack <folder> [out.wfplugin]
//!   wayfinder plugin check <file.wfplugin | folder>
//!
//! Exit codes: 0 fine, 1 the widget or plugin has problems, 2 bad arguments.
//!
//! A render is hermetic unless told otherwise (see `render`): a fixed date, system readings,
//! track and sound, no network, no installed content, the software adapter. `--env`, `--now`,
//! `--real` and `--installed` change that, and `<png>.env.json` records what was used.

use std::path::{Path, PathBuf};

use crate::plugins;
pub use crate::render::Request as Render;
#[cfg(test)]
use crate::ambient::Pins;

pub const USAGE: &str = "usage:
  wayfinder --render-widget <id | file.toml> --png <out.png> [--size WxH] [--param k=v]... [--state k=v]...
            [--palette name] [--scale 1.25] [--time HH:MM] [--now ISO] [--env key=value]... [--real seams]
            [--installed] [--transparent] [--wait secs] [--data dir] [--gpu mode]
  wayfinder plugin pack <folder> [out.wfplugin]
  wayfinder plugin check <file.wfplugin | folder>";

#[derive(Debug, PartialEq)]
pub enum Command {
    Render(Render),
    Pack { dir: PathBuf, out: Option<PathBuf> },
    Check(PathBuf),
    Usage(String),
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
        let mut r = Render::new(widget);
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
                "--palette" => r.pins.palette = val()?,
                "--scale" => r.pins.scale = val()?.parse().ok().filter(|s: &f32| *s > 0.0 && *s <= 4.0).ok_or("--scale is a number from 0 to 4")?,
                "--time" => {
                    let v = val()?;
                    let (h, m) = v.split_once(':').ok_or("--time is HH:MM")?;
                    r.time = Some((h.parse().map_err(|_| "--time is HH:MM")?, m.parse().map_err(|_| "--time is HH:MM")?));
                }
                "--now" => r.pins.set("now", &serde_json::Value::String(val()?)).map_err(|e| format!("--now: {e}"))?,
                "--real" => r.pins.set("real", &serde_json::Value::String(val()?)).map_err(|e| format!("--real: {e}"))?,
                "--installed" => r.installed = true,
                "--transparent" => r.pins.transparent = true,
                "--wait" => r.wait = val()?.parse().map_err(|_| "--wait is seconds")?,
                "--data" => r.data = Some(val()?.into()),
                "--gpu" => r.gpu = val()?,
                "--env" => {
                    let (k, v) = pair(&val()?, "--env")?;
                    r.pins.set(&k, &v).map_err(|e| format!("--env {e}"))?;
                }
                other => return Err(format!("unknown option `{other}`")),
            }
        }
        if r.png.as_os_str().is_empty() {
            return Err("--render-widget needs --png <out.png>".into());
        }
        r.pins.check().map_err(|e| format!("--env {e}"))?;
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
        Command::Render(r) => match crate::render::render(&r) {
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
    fn env_pins_parse_with_loose_values_and_bad_ones_are_usage_errors() {
        let render = |extra: &[&str]| command(&args(&[&["--render-widget", "clock", "--png", "o.png"][..], extra].concat()));
        let Some(Command::Render(r)) = render(&["--env", "sys.cpu=42", "--env", "media.playing=true", "--env", "now=2026-03-08T15:42:00", "--env", "media.title=Blue in Green", "--env", "sys.gpus=[5,6]"]) else { panic!() };
        assert_eq!((r.pins.sys.cpu, r.pins.media.playing, r.pins.media.title.as_str(), r.pins.sys.gpus.clone()), (42, true, "Blue in Green", vec![5, 6]));
        assert_eq!((r.pins.now.month, r.pins.now.day, r.pins.now.hour, r.pins.now.minute), (3, 8, 15, 42));
        // an unknown key names the nearest, a bad value says what was expected; both come back as Usage, which exits 2
        let Some(Command::Usage(e)) = render(&["--env", "sys.cpo=42"]) else { panic!() };
        assert!(e.contains("unknown pin `sys.cpo` (did you mean `sys.cpu`?)"), "{e}");
        for bad in [&["--env", "sys.cpu=lots"][..], &["--env", "sys.cpu"], &["--env"], &["--env", "real=sys", "--env", "sys.cpu=1"], &["--env", "sys.charging=true"]] {
            assert!(matches!(render(bad), Some(Command::Usage(_))), "{bad:?}");
        }
    }

    #[test]
    fn the_look_flags_set_pins_and_the_environment_flags_are_sugar_over_them() {
        let render = |extra: &[&str]| command(&args(&[&["--render-widget", "clock", "--png", "o.png"][..], extra].concat()));
        let Some(Command::Render(r)) = render(&["--palette", "Dawn", "--scale", "2", "--now", "2026-03-08T02:30:00", "--time", "15:42", "--real", "sys,gpu", "--installed", "--data", "D:/wf", "--gpu", "low", "--wait", "9"]) else { panic!() };
        assert_eq!((r.pins.palette.as_str(), r.pins.scale, r.installed, r.data, r.gpu.as_str(), r.wait), ("Dawn", 2.0, true, Some(PathBuf::from("D:/wf")), "low", 9.0));
        assert_eq!((r.pins.now.month, r.pins.now.day, r.pins.now.hour, r.time), (3, 8, 2, Some((15, 42))), "--time is applied after the pins, to their date");
        assert_eq!(r.pins.real.len(), 2);
        for bad in [&["--now", "soon"][..], &["--now", "2026-02-30T10:00"], &["--real", "clok"], &["--real", "clock", "--now", "2026-03-08T02:30:00"], &["--scale", "9"], &["--now"]] {
            assert!(matches!(render(bad), Some(Command::Usage(_))), "{bad:?}");
        }
    }

    #[test]
    fn render_options_parse() {
        let Some(Command::Render(r)) = command(&args(&["--render-widget", "clock", "--png", "o.png", "--size", "300x200", "--param", "title=Quick launch", "--param", "smooth=true", "--state", "selected=2", "--time", "15:42", "--transparent"])) else { panic!() };
        assert_eq!((r.widget.as_str(), r.png, r.size, r.time, r.pins.transparent), ("clock", PathBuf::from("o.png"), Some((300.0, 200.0)), Some((15, 42)), true));
        assert_eq!((r.installed, r.data, r.gpu.as_str(), r.wait), (false, None, "software", 5.0));
        assert_eq!(r.params, [("title".to_string(), serde_json::json!("Quick launch")), ("smooth".to_string(), serde_json::json!(true))]);
        assert_eq!(r.state, [("selected".to_string(), serde_json::json!(2))]);
        assert_eq!(r.pins, Pins { transparent: true, ..Pins::default() }, "without --env the environment is the default");
        for bad in [&["--render-widget", "clock"][..], &["--render-widget", "clock", "--png", "o.png", "--size", "big"], &["--render-widget", "clock", "--png", "o.png", "--nope"], &["--render-widget", "--png", "o.png"]] {
            assert!(matches!(command(&args(bad)), Some(Command::Usage(_))), "{bad:?}");
        }
    }
}
