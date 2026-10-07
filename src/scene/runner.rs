//! Runs one scene: its target and Pins become a render `Request`, the hermetic environment
//! settles the frame with the UI trace on, and the frame becomes a `SceneDump`. No device is
//! created: a dump needs the layout, the text engine and the Image Store, not the adapter.

use std::path::PathBuf;

use serde_json::Value as Json;

use super::dump::{self, Kind, SceneDump};
use super::file::{Expect, Scene, Size};
use crate::ambient::Pins;
use crate::gfx::{AdapterReport, Gpu, Power};
use crate::render::{self, Failure, Request, env};

/// What the command line changes in every selected scene (`--env`, `--size`, `--param`...).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overrides {
    /// Pins after the scene's own, `key = value`.
    pub pins: Vec<(String, Json)>,
    pub size: Option<Size>,
    pub params: Vec<(String, Json)>,
    pub state: Vec<(String, Json)>,
    pub hide: Vec<String>,
    pub hover: Option<String>,
    pub time: Option<(u32, u32)>,
    pub plugin: Option<PathBuf>,
    pub wait: Option<f32>,
}

/// The software device scenes are drawn on, made when the first one is drawn. Only the
/// software adapter (WARP) is ever asked for, and a device on any other is refused
/// (`render::software_gpu`). One device serves every scene of a run: 25 golden scenes came out
/// byte-identical whether each had a device of its own or they shared one (measured on this
/// machine), and sharing is 2.6 times faster.
pub struct Device {
    made: Option<(Gpu, AdapterReport)>,
}

impl Device {
    pub fn new() -> Device {
        Device { made: None }
    }

    /// Runs `f` on the device (made if need be): the contact sheets and comparison pictures are
    /// drawn on the same software adapter as the scenes.
    pub fn with_gpu<T>(&mut self, f: impl FnOnce(&mut Gpu) -> Result<T, String>) -> Result<T, String> {
        if self.made.is_none() {
            self.made = Some(render::software_gpu()?);
        }
        f(&mut self.made.as_mut().expect("made above").0)
    }

    /// Draws a settled frame; the adapter it was drawn on comes with the image.
    fn draw(&mut self, settled: &mut render::Settled) -> Result<Drawn, String> {
        if self.made.is_none() {
            self.made = Some(render::software_gpu()?);
        }
        let (gpu, adapter) = self.made.as_mut().expect("made above");
        let image = render::draw(gpu, settled)?;
        Ok(Drawn { image, adapter: adapter.clone() })
    }
}

/// A scene drawn: the window's pixels over the backdrop, and the adapter that drew them.
pub struct Drawn {
    pub image: image::RgbaImage,
    pub adapter: AdapterReport,
}

/// Whether and how a run drew its frame.
pub enum Pixels {
    /// The run needed no pixels (a dump).
    Off,
    Drawn(Drawn),
    /// There was no software adapter, or it failed: infrastructure, not a finding.
    Failed(String),
}

/// What running a scene produced.
pub struct Outcome {
    pub dump: SceneDump,
    /// The record of the environment, as the sidecar `<id>.env.json`.
    pub sidecar: Json,
    /// What the scene's `[expect]` says is wrong, one line each.
    pub unmet: Vec<String>,
    /// The widget showed its error card: the message.
    pub error: Option<String>,
    /// Notices and warnings from the run.
    pub notes: Vec<String>,
    /// What the widget warned of while it was built (a binding that did not resolve...).
    pub warnings: Vec<String>,
    pub pixels: Pixels,
}

/// The render request a scene stands for.
pub fn request(s: &Scene, o: &Overrides) -> Result<Request, String> {
    let t = &s.target;
    let mut r = Request::new(t.widget.clone());
    let mut pins = Pins::default();
    for (k, v) in s.pins.iter() {
        pins.set(k, v).map_err(|e| format!("{} ({}): [env]/[look] {e}", s.file.display(), s.id))?;
    }
    for (k, v) in &o.pins {
        pins.set(k, v).map_err(|e| format!("--env {e}"))?;
    }
    pins.check().map_err(|e| format!("{} ({}): {e}", s.file.display(), s.id))?;
    r.pins = pins;
    match o.size.as_ref().unwrap_or(&t.size) {
        Size::Default => {}
        Size::Card(w, h) => r.size = Some((*w, *h)),
        Size::Tier(n) => r.tier = Some(n.clone()),
    }
    let merged = |base: &[(String, Json)], extra: &[(String, Json)]| {
        let mut out = base.to_vec();
        for (k, v) in extra {
            match out.iter_mut().find(|(n, _)| n == k) {
                Some(e) => e.1 = v.clone(),
                None => out.push((k.clone(), v.clone())),
            }
        }
        out
    };
    r.params = merged(&t.params, &o.params);
    r.state = merged(&t.state, &o.state);
    r.hide = if o.hide.is_empty() { t.hide.clone() } else { o.hide.clone() };
    r.items = t.items.clone();
    r.hover = o.hover.clone().or_else(|| t.hover.clone());
    r.hover_at = if o.hover.is_some() { None } else { t.hover_at };
    r.edit = t.edit;
    r.time = o.time;
    r.plugin = o.plugin.clone().or_else(|| t.plugin.clone());
    if let Some(w) = o.wait {
        r.wait = w;
    }
    Ok(r)
}

/// What an `[expect]` says is unmet by `dump`.
pub fn unmet(e: &Expect, d: &SceneDump) -> Vec<String> {
    let runs = d.texts();
    let mut out = Vec::new();
    for want in &e.text {
        if !runs.iter().any(|(_, t)| t.contains(want.as_str())) {
            out.push(format!("expect text: no text run holds `{want}`"));
        }
    }
    for bad in &e.no_text {
        if let Some((key, t)) = runs.iter().find(|(_, t)| t.contains(bad.as_str())) {
            out.push(format!("expect no_text: `{key}` holds `{bad}` (\"{t}\")"));
        }
    }
    out
}

/// Runs `s`. A request that cannot be made or a widget that cannot be found is `Failure::Bad`;
/// a code source that never answers or pictures that never finish is `Failure::Run`.
pub fn run(s: &Scene, o: &Overrides) -> Result<Outcome, Failure> {
    run_with(s, o, None)
}

/// Runs `s` and, with a `device`, draws it too (the software adapter, see `Device`).
pub fn run_with(s: &Scene, o: &Overrides, device: Option<&mut Device>) -> Result<Outcome, Failure> {
    let r = request(s, o).map_err(Failure::Bad)?;
    let mut settled = render::settle(&r, Power::Software, true)?;
    let dump = dump::build(&s.id, &settled);
    let pixels = match device {
        None => Pixels::Off,
        Some(d) => d.draw(&mut settled).map_or_else(Pixels::Failed, Pixels::Drawn),
    };
    let faces = settled.text.font_faces();
    let px = |v: f32| (v * settled.pins.scale).round() as u32;
    let adapter = if let Pixels::Drawn(d) = &pixels { Some(&d.adapter) } else { None };
    let facts = env::Facts { widget: &settled.content.id, size: (px(settled.window.0), px(settled.window.1)), pins: &settled.pins, real: &settled.real, installed: settled.content.installed, roots: &settled.content.roots, faces: &faces, adapter, deps: &settled.prepared.deps, paths: &settled.paths, code_sources: &settled.code_sources, rounds: settled.rounds };
    let sidecar = env::sidecar(&facts);
    let unmet = unmet(&s.expect, &dump);
    let error = settled.prepared.error.clone();
    Ok(Outcome { dump, sidecar, unmet, error, notes: settled.notes.clone(), warnings: settled.prepared.warnings.clone(), pixels })
}

impl Outcome {
    /// The copy on screen: every text run, in order.
    pub fn texts(&self) -> Vec<String> {
        self.dump.nodes.iter().filter_map(|n| if let Kind::Text(t) = &n.kind { Some(t.text.clone()) } else { None }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::file::{self, Target};

    fn scene(widget: &str) -> Scene {
        Scene { id: "t/x".into(), name: "x".into(), tags: vec![], file: PathBuf::from("x.scene.toml"), root: PathBuf::from("."), target: Target { widget: widget.into(), plugin: None, size: Size::Default, params: vec![], state: vec![], hide: vec![], items: None, hover: None, hover_at: None, edit: None }, pins: vec![], expect: Expect::default(), source_hash: String::new() }
    }

    #[test]
    fn a_scenes_pins_then_the_command_lines_become_the_requests_environment() {
        let mut s = scene("clock");
        s.pins = vec![("sys.cpu".into(), Json::from(42)), ("scale".into(), Json::from(1.25))];
        let o = Overrides { pins: vec![("sys.cpu".into(), Json::from(90))], size: Some(Size::Card(300.0, 200.0)), params: vec![("ticks".into(), Json::Bool(false))], ..Default::default() };
        let r = request(&s, &o).unwrap();
        assert_eq!((r.pins.sys.cpu, r.pins.scale, r.size, r.params.len()), (90, 1.25, Some((300.0, 200.0)), 1));
        assert_eq!((r.gpu.as_str(), r.installed), ("software", false), "a scene request is the hermetic default");
    }

    #[test]
    fn a_bad_pin_names_the_scene_and_the_nearest_key_and_a_conflict_is_refused() {
        let mut s = scene("clock");
        s.pins = vec![("sys.cpo".into(), Json::from(1))];
        let e = request(&s, &Overrides::default()).unwrap_err();
        assert!(e.contains("x.scene.toml") && e.contains("t/x") && e.contains("did you mean `sys.cpu`"), "{e}");
        let mut s = scene("clock");
        s.pins = vec![("real".into(), Json::from("sys")), ("sys.cpu".into(), Json::from(1))];
        assert!(request(&s, &Overrides::default()).unwrap_err().contains("real"));
    }

    #[test]
    fn a_tier_size_and_the_widgets_hidden_modules_reach_the_request() {
        let mut s = scene("system_monitor");
        s.target.size = Size::Tier("compact".into());
        s.target.hide = vec!["gauge:cpu".into()];
        let r = request(&s, &Overrides::default()).unwrap();
        assert_eq!((r.tier.as_deref(), r.hide.clone()), (Some("compact"), vec!["gauge:cpu".to_string()]));
        let r = request(&s, &Overrides { hide: vec!["footer".into()], ..Default::default() }).unwrap();
        assert_eq!(r.hide, ["footer"], "the command line's list replaces the scene's");
    }

    #[test]
    fn a_widget_of_a_plugin_folder_runs_hermetically_without_installing_it() {
        let dir = std::env::temp_dir().join(format!("wf-scene-plugin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("widgets")).unwrap();
        std::fs::write(dir.join("plugin.toml"), "id = 'hello'\nname = 'Hello'\nversion = '1.0'").unwrap();
        std::fs::write(dir.join("widgets/hello.toml"), "name = 'Hello'\nsize = [120, 60]\n[root]\npadding = 8\n  [[root.children]]\n  type = 'text'\n  text = 'Hello from {pad(clock.minute, 2)}'\n  size = 14\n").unwrap();
        let mut s = scene("hello");
        s.target.plugin = Some(dir.clone());
        let out = run(&s, &Overrides::default());
        let _ = std::fs::remove_dir_all(&dir);
        let out = out.unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(out.texts(), ["Hello from 10"], "the pinned clock, not the machine's");
        assert!(out.dump.header.hermetic, "{:?}", out.dump.header.not_hermetic_because);
        let kinds: Vec<&str> = out.sidecar["content"]["roots"].as_array().unwrap().iter().filter_map(|r| r["kind"].as_str()).collect();
        assert_eq!(kinds, ["plugin-folder"], "the folder is hashed into the record");
        assert!(out.sidecar["adapter"].is_null() && out.error.is_none());
        let missing = scene("nope");
        assert!(matches!(run(&missing, &Overrides::default()), Err(Failure::Bad(e)) if e.contains("no widget `nope`")));
    }

    #[test]
    fn unmet_expectations_name_the_text_that_is_missing_or_present() {
        let d = crate::scene::dump::fixtures::tiny_dump(&["Thu 15 Jan", "10:10"]);
        let e = Expect { text: vec!["Thu".into(), "Friday".into()], no_text: vec!["NaN".into(), "10:10".into()], ..Default::default() };
        let got = unmet(&e, &d);
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(got[0].contains("no text run holds `Friday`") && got[1].contains("holds `10:10`"), "{got:?}");
        assert!(unmet(&Expect::default(), &d).is_empty());
        let _ = file::FORMAT;
    }
}
