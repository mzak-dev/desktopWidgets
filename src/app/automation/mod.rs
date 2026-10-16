//! The Automation Bridge (ADR-0013): with `--automation`, Wayfinder answers Raywright about
//! what its windows drew, draws pictures on request, runs a few commands and can stop its
//! animation clock. All of it runs on the event loop's thread: the bridge's own threads only
//! hand each request over as `UserEvent::Automation`.

mod commands;
mod describe;

use std::collections::BTreeMap;
use std::sync::Arc;

use raywright_bridge::protocol::{CaptureParams, CaptureResult, ClockMode, ClockParams, ClockStepParams, ClockStepResult, ElementsParams, RpcError, WindowDesc, methods};
use raywright_bridge::{Bridge, CommandRegistry, Notifier, Request, Responder, capability};
use serde_json::{Value, json};

use super::*;

/// What a command asks of the event loop once it has run: things that need the
/// `ActiveEventLoop`, which a command does not get.
pub(super) enum Then {
    Menu(String),
    Settings(Option<String>),
}

pub(super) struct Automation {
    notifier: Notifier,
    commands: Arc<CommandRegistry<App>>,
    then: Vec<Then>,
}

/// One window an outside program can see.
struct Surface {
    desc: WindowDesc,
    window: Arc<Window>,
    /// The Instance, for its params and for pictures.
    instance: Option<usize>,
}

/// Starts the bridge, listening where `RAYWRIGHT_BRIDGE` says (or on a default address).
pub(super) fn start(proxy: EventLoopProxy<UserEvent>) -> Result<Automation, String> {
    let commands = Arc::new(commands::registry());
    let proxy = Mutex::new(proxy);
    let handle = Bridge::builder("wayfinder", env!("CARGO_PKG_VERSION"))
        .capabilities([capability::ELEMENTS, capability::CAPTURE, capability::FRAME_EVENTS, capability::CALL, capability::MANUAL_CLOCK])
        .commands(commands.names())
        .start(move |req: Request, reply: Responder| {
            // a closed loop drops the event and with it the responder, which then answers with an error
            let _ = proxy.lock().unwrap_or_else(|e| e.into_inner()).send_event(UserEvent::Automation(req, reply));
        })
        .map_err(|e| e.to_string())?;
    Ok(Automation { notifier: handle.notifier(), commands, then: Vec::new() })
}

fn hwnd_id(window: &Window) -> Option<u64> {
    win32::hwnd_of(window).map(|h| h.0 as usize as u64)
}

fn px(r: u32) -> f64 {
    r as f64
}

impl App {
    pub(super) fn automation_event(&mut self, el: &ActiveEventLoop, req: Request, reply: Responder) {
        match req.method.as_str() {
            methods::WINDOWS => reply.ok(self.surfaces().into_iter().map(|s| s.desc).collect::<Vec<_>>()),
            methods::ELEMENTS => match req.params::<ElementsParams>() {
                Ok(p) => reply.reply(self.automation_elements(p.window)),
                Err(e) => reply.err(e),
            },
            methods::CAPTURE => match req.params::<CaptureParams>() {
                Ok(p) => reply.reply(self.automation_capture(p.window)),
                Err(e) => reply.err(e),
            },
            methods::CALL => reply.reply(self.automation_call(el, &req.params)),
            methods::CLOCK => match req.params::<ClockParams>() {
                Ok(p) => {
                    anim::clock::set_manual(p.mode == ClockMode::Manual);
                    self.redraw_all();
                    reply.ok(Value::Null);
                }
                Err(e) => reply.err(e),
            },
            methods::CLOCK_STEP => match req.params::<ClockStepParams>() {
                Ok(p) => reply.reply(self.automation_step(&p)),
                Err(e) => reply.err(e),
            },
            other => reply.not_found(other),
        }
    }

    /// Every window of the app, as Raywright's `windows` answer.
    fn surfaces(&self) -> Vec<Surface> {
        let desc = |window: &Window, title: String, kind: &str, name: Option<String>, attrs: BTreeMap<String, String>| {
            let id = hwnd_id(window)?;
            let pos = window.inner_position().ok()?;
            let size = window.inner_size();
            let mut d = WindowDesc::new(id, title);
            d.kind = Some(kind.into());
            d.name = name;
            d.scale = window.scale_factor();
            d.client = raywright_bridge::protocol::Rect::new(pos.x as f64, pos.y as f64, px(size.width), px(size.height));
            d.attrs = attrs;
            Some(d)
        };
        let mut out = Vec::new();
        for (i, iw) in self.wins.iter().enumerate() {
            let Some(window) = iw.window.clone() else { continue };
            let cfg = &self.ws.instances[i];
            let attrs = BTreeMap::from([("widget".to_string(), cfg.widget.clone())]);
            if let Some(d) = desc(&window, format!("Wayfinder {}", cfg.id), "instance", Some(cfg.id.clone()), attrs) {
                out.push(Surface { desc: d, window, instance: Some(i) });
            }
        }
        if let Some(s) = &self.settings {
            let attrs = BTreeMap::from([("page".to_string(), s.page().to_string())]);
            if let Some(d) = desc(&s.window, "Wayfinder Settings".into(), "settings", None, attrs) {
                out.push(Surface { desc: d, window: s.window.clone(), instance: None });
            }
        }
        if let Some(m) = &self.menu {
            if let Some(d) = desc(&m.window, "Wayfinder menu".into(), "menu", None, BTreeMap::new()) {
                out.push(Surface { desc: d, window: m.window.clone(), instance: None });
            }
        }
        out
    }

    fn automation_elements(&self, id: u64) -> Result<Value, RpcError> {
        let s = self.surfaces().into_iter().find(|s| s.desc.id == id).ok_or_else(|| RpcError::app(format!("no window {id}")))?;
        let frame = match s.instance {
            Some(i) => self.wins[i].frame.as_ref(),
            None if self.settings.as_ref().is_some_and(|w| w.window.id() == s.window.id()) => self.settings.as_ref().and_then(|w| w.frame()),
            None => self.menu.as_ref().and_then(|m| m.frame()),
        };
        let mut attrs = s.desc.attrs.clone();
        if let Some(name) = &s.desc.name {
            attrs.insert("instance".into(), name.clone());
        }
        let els = frame.map(|f| describe::elements(f, s.window.scale_factor() as f32, &|k| self.text.text_of(k), &attrs)).unwrap_or_default();
        serde_json::to_value(els).map_err(|e| RpcError::app(e.to_string()))
    }

    /// The window's last frame, drawn again off screen: exactly what Wayfinder drew, with
    /// nothing in front of it and no desktop behind.
    fn automation_capture(&mut self, id: u64) -> Result<Value, RpcError> {
        let i = self.surfaces().into_iter().find(|s| s.desc.id == id).and_then(|s| s.instance).ok_or_else(|| RpcError::app(format!("no widget window {id}")))?;
        let App { gpu, text, wins, .. } = self;
        let (Some(gpu), Some(iw)) = (gpu.as_mut(), wins.get(i)) else { return Err(RpcError::app("not drawn yet")) };
        let (Some(window), Some(frame)) = (&iw.window, &iw.frame) else { return Err(RpcError::app("not drawn yet")) };
        let size = window.inner_size();
        let mut rgba = gpu.render_offscreen(size.width.max(1), size.height.max(1), &frame.list, text).map_err(RpcError::app)?;
        // the GPU hands back premultiplied alpha; a PNG holds straight alpha
        for c in rgba.chunks_exact_mut(4) {
            let a = c[3] as u32;
            if a > 0 && a < 255 {
                for v in &mut c[..3] {
                    *v = ((*v as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        let img = image::RgbaImage::from_raw(size.width.max(1), size.height.max(1), rgba).ok_or_else(|| RpcError::app("picture size mismatch"))?;
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| RpcError::app(e.to_string()))?;
        serde_json::to_value(CaptureResult::from_png(&png, img.width(), img.height(), iw.frame_seq)).map_err(|e| RpcError::app(e.to_string()))
    }

    fn automation_call(&mut self, el: &ActiveEventLoop, params: &Value) -> Result<Value, RpcError> {
        let Some(commands) = self.automation.as_ref().map(|a| a.commands.clone()) else { return Err(RpcError::app("automation is off")) };
        let result = commands.dispatch_call(self, params);
        let thens = self.automation.as_mut().map(|a| std::mem::take(&mut a.then)).unwrap_or_default();
        for t in thens {
            match t {
                Then::Menu(id) => self.user_event(el, UserEvent::Menu(id)),
                Then::Settings(page) => {
                    self.open_settings(el);
                    if let (Some(s), Some(p)) = (&mut self.settings, page) {
                        s.show_page(&p);
                    }
                }
            }
        }
        result
    }

    /// Moves the stopped animation clock on and draws every widget at the new time before
    /// answering, so the next picture shows it.
    fn automation_step(&mut self, p: &ClockStepParams) -> Result<Value, RpcError> {
        if !(p.ms.is_finite() && p.ms >= 0.0) {
            return Err(RpcError::invalid_params("ms must be 0 or more"));
        }
        let total = anim::clock::step(Duration::from_secs_f64(p.ms / 1000.0)).ok_or_else(|| RpcError::app("the clock is running: set it to manual first"))?;
        let mut seq = 0;
        for i in 0..self.wins.len() {
            if self.wins[i].window.is_some() && self.wins[i].target.is_some() {
                self.render(i);
                seq = seq.max(self.wins[i].frame_seq);
            }
        }
        serde_json::to_value(ClockStepResult::new(seq, total.as_secs_f64() * 1000.0)).map_err(|e| RpcError::app(e.to_string()))
    }

    /// Tells connected clients that window `i` presented a new frame.
    pub(super) fn automation_frame(&self, i: usize) {
        let Some(a) = &self.automation else { return };
        if let Some(id) = self.wins[i].window.as_deref().and_then(hwnd_id) {
            a.notifier.frame(id, self.wins[i].frame_seq);
        }
    }

    /// What the `state` command answers.
    pub(super) fn automation_state(&self) -> Value {
        let instances: Vec<Value> = self.ws.instances.iter().zip(&self.wins).map(|(c, w)| json!({ "id": c.id, "widget": c.widget, "x": c.x, "y": c.y, "w": c.w, "h": c.h, "window": w.window.is_some() })).collect();
        json!({
            "edit": self.edit,
            "workspace": self.ws.active,
            "instances": instances,
            "settings": self.settings.as_ref().map(|s| s.page()),
            "menu": self.menu.is_some(),
            "clock": { "manual": anim::clock::is_manual() },
        })
    }
}
