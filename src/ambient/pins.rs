//! The values a headless render fixes so the same scene gives the same image: the date and
//! time, zone, locale, system readings, the playing track, sound, network answers, the
//! settle time and the look. `Ambient::fixed_with` turns the machine-facing ones into an
//! Ambient; the render path reads the rest. A Pin is set by a dotted key (`sys.cpu`) and a
//! JSON-or-text value, the spelling of `--env sys.cpu=42` and of a scene's `[env]` table.
//!
//! Fonts are the machine's (there is no bundled set), so `fonts` has one mode and a render
//! is exact on the machine and font set it ran on, not across machines.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::Value;

use super::fixed::ZONE_TABLE;
use super::{Tm, civil_from_days, days_from_civil};
use crate::suggest::suggest;

/// Every key `Pins::set` knows, in the order `--help` style listings show them.
pub const KEYS: &[&str] = &[
    "now", "zone", "locale",
    "sys.cpu", "sys.ram", "sys.disk", "sys.net_down", "sys.net_up", "sys.processes", "sys.uptime", "sys.battery", "sys.charging", "sys.gpus",
    "media.active", "media.playing", "media.title", "media.artist", "media.album", "media.source", "media.position", "media.duration", "media.can_seek", "media.art",
    "audio", "audio.tones", "audio.amp",
    "icons", "fonts", "fetch", "fetch.responses",
    "settle", "scale", "palette", "anim", "transparent", "backdrop", "real",
];

/// The locales `FixedCalendar` has name tables for.
pub(super) const LOCALES: &[&str] = &["en-US"];
/// Animation speeds, the values of the `anim-speed` style token.
const ANIM: &[&str] = &["off", "fast", "normal", "relaxed"];

/// A seam a render may take from the real machine instead of the fixed one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Seam {
    Clock,
    Sys,
    Media,
    Audio,
    Icons,
    Fonts,
    Fetch,
    Gpu,
}

impl Seam {
    pub const ALL: [Seam; 8] = [Seam::Clock, Seam::Sys, Seam::Media, Seam::Audio, Seam::Icons, Seam::Fonts, Seam::Fetch, Seam::Gpu];

    pub fn name(self) -> &'static str {
        match self {
            Seam::Clock => "clock",
            Seam::Sys => "sys",
            Seam::Media => "media",
            Seam::Audio => "audio",
            Seam::Icons => "icons",
            Seam::Fonts => "fonts",
            Seam::Fetch => "fetch",
            Seam::Gpu => "gpu",
        }
    }
}

/// What the system monitor reads. Percentages are whole numbers.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub struct SysPins {
    pub cpu: u8,
    pub ram: u8,
    /// Percent of the system drive in use.
    pub disk: u8,
    /// MB/s (1 MB = 1 048 576 bytes).
    pub net_down: f64,
    pub net_up: f64,
    pub processes: u32,
    /// Seconds since boot.
    pub uptime: u64,
    /// Battery percent, `None` on a desktop.
    pub battery: Option<u8>,
    pub charging: bool,
    /// The load of each graphics adapter in percent; empty = none.
    pub gpus: Vec<u8>,
}

impl Default for SysPins {
    /// The demo desktop: 16 GB of RAM, C: and D:, three days and four hours up, one GPU.
    fn default() -> Self {
        Self { cpu: 37, ram: 58, disk: 44, net_down: 2.4, net_up: 0.176, processes: 212, uptime: 3 * 86_400 + 4 * 3600, battery: None, charging: false, gpus: vec![21] }
    }
}

#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub struct MediaPins {
    pub active: bool,
    pub playing: bool,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub source: String,
    /// Seconds into the track.
    pub position: f64,
    pub duration: f64,
    pub can_seek: bool,
    /// An image file for the cover, "" for none. The scene runner makes it absolute.
    pub art: String,
}

impl Default for MediaPins {
    fn default() -> Self {
        Self { active: true, playing: false, title: "Example Song".into(), artist: "Example Artist".into(), album: "Example Album".into(), source: "Player".into(), position: 83.0, duration: 215.0, can_seek: true, art: String::new() }
    }
}

/// What the capture hears: a sum of steady tones, or nothing.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub struct AudioPins {
    pub silence: bool,
    pub tones: Vec<f32>,
    pub amp: f32,
}

impl Default for AudioPins {
    fn default() -> Self {
        Self { silence: false, tones: vec![110.0, 440.0, 1760.0], amp: 0.3 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IconMode {
    /// A hash-coloured tile per file stem.
    #[default]
    Tiles,
    /// The shell's icons; the render path supplies them, as they are the machine's.
    System,
}

/// Where text metrics come from. The machine's fonts are the only set there is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FontMode {
    #[default]
    System,
}

/// One canned answer to a request for exactly `url`.
#[derive(Clone, Debug, PartialEq)]
pub struct Canned {
    pub url: String,
    pub status: u16,
    pub content_type: String,
    pub body: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum FetchMode {
    /// Every request fails `offline (render environment)`.
    #[default]
    Offline,
    /// These answers; any other request fails as offline does.
    Canned(Vec<Canned>),
    /// The machine's network; the render path supplies it.
    Real,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Backdrop {
    /// The built-in gradient.
    #[default]
    Gradient,
    Solid([u8; 3]),
}

/// The environment of one render. `Pins::default()` is the always-on set.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub struct Pins {
    /// Local wall time, frozen for the whole render.
    pub now: Tm,
    /// A Windows zone key of the embedded table.
    pub zone: String,
    pub locale: String,
    pub sys: SysPins,
    pub media: MediaPins,
    pub audio: AudioPins,
    pub icons: IconMode,
    pub fonts: FontMode,
    pub fetch: FetchMode,
    /// How long the animation clock runs before the capture.
    pub settle: Duration,
    pub scale: f32,
    pub palette: String,
    /// An `anim-speed` value that overrides the theme's; `None` keeps the theme's.
    pub anim: Option<String>,
    pub transparent: bool,
    pub backdrop: Backdrop,
    /// Seams the render takes from the real machine, which makes it not hermetic.
    pub real: BTreeSet<Seam>,
}

impl Default for Pins {
    /// Thursday 2026-01-15 10:10:30 UTC, en-US, the demo desktop, a paused example track, a
    /// steady tone, tile icons, no network, a 2 s settle at scale 1.25 in `Midnight`.
    fn default() -> Self {
        Self {
            now: Tm::new(2026, 1, 15, 4, 10, 10, 30, 0),
            zone: "UTC".into(),
            locale: "en-US".into(),
            sys: SysPins::default(),
            media: MediaPins::default(),
            audio: AudioPins::default(),
            icons: IconMode::default(),
            fonts: FontMode::default(),
            fetch: FetchMode::default(),
            settle: Duration::from_secs(2),
            scale: 1.25,
            palette: "Midnight".into(),
            anim: None,
            transparent: false,
            backdrop: Backdrop::default(),
            real: BTreeSet::new(),
        }
    }
}

fn show(v: &Value) -> String {
    v.to_string()
}

fn text(key: &str, v: &Value) -> Result<String, String> {
    match v {
        Value::String(s) => Ok(s.clone()),
        Value::Number(_) | Value::Bool(_) => Ok(v.to_string()),
        _ => Err(format!("{key} is text, not {}", show(v))),
    }
}

fn num(key: &str, v: &Value) -> Result<f64, String> {
    v.as_f64().filter(|n| n.is_finite()).ok_or_else(|| format!("{key} is a number, not {}", show(v)))
}

fn flag(key: &str, v: &Value) -> Result<bool, String> {
    v.as_bool().ok_or_else(|| format!("{key} is true or false, not {}", show(v)))
}

/// A whole number of percent, 0 to 100.
fn percent(key: &str, v: &Value) -> Result<u8, String> {
    let n = num(key, v)?;
    if (0.0..=100.0).contains(&n) { Ok(n.round() as u8) } else { Err(format!("{key} is a percentage from 0 to 100, not {}", show(v))) }
}

/// A non-negative number up to `max`.
fn ranged(key: &str, v: &Value, max: f64) -> Result<f64, String> {
    let n = num(key, v)?;
    if (0.0..=max).contains(&n) { Ok(n) } else { Err(format!("{key} is a number from 0 to {max}, not {}", show(v))) }
}

/// One of `names`, written in lower case.
fn choice(key: &str, v: &Value, names: &[&'static str]) -> Result<&'static str, String> {
    let s = text(key, v)?;
    names.iter().copied().find(|n| n.eq_ignore_ascii_case(&s)).ok_or_else(|| format!("{key}: `{s}` is not one of {}{}", names.join(", "), suggest(&s.to_lowercase(), &[names])))
}

fn list<'a>(key: &str, v: &'a Value) -> Result<&'a [Value], String> {
    match v {
        Value::Array(a) => Ok(a),
        Value::Null => Ok(&[]),
        _ => Err(format!("{key} is a list, not {}", show(v))),
    }
}

/// Seconds, `2`, `2s` or `500ms`.
fn seconds(key: &str, v: &Value) -> Result<Duration, String> {
    let secs = match v {
        Value::String(s) => {
            let s = s.trim();
            let (n, scale) = s.strip_suffix("ms").map(|n| (n, 0.001)).or_else(|| s.strip_suffix('s').map(|n| (n, 1.0))).unwrap_or((s, 1.0));
            n.trim().parse::<f64>().ok().filter(|n| n.is_finite()).map(|n| n * scale).ok_or_else(|| format!("{key} is a time like 2s or 500ms, not `{s}`"))?
        }
        _ => num(key, v)?,
    };
    if (0.0..=60.0).contains(&secs) { Ok(Duration::from_secs_f64(secs)) } else { Err(format!("{key} is from 0 to 60 seconds, not {}", show(v))) }
}

/// `2026-03-08T15:42`, with optional seconds and milliseconds (`T` or a space between).
fn parse_now(s: &str) -> Result<Tm, String> {
    let bad = || format!("now is a local time like 2026-01-15T10:10:30, not `{s}`");
    let (date, time) = s.trim().split_once(['T', ' ']).ok_or_else(bad)?;
    let mut d = date.split('-').map(|p| p.parse::<u32>().map_err(|_| bad()));
    let (Some(y), Some(mo), Some(dd), None) = (d.next(), d.next(), d.next(), d.next()) else { return Err(bad()) };
    let (year, month, day) = (y? as i32, mo?, dd?);
    let (hms, frac) = time.split_once('.').unwrap_or((time, "0"));
    let mut t = hms.split(':').map(|p| p.parse::<u32>().map_err(|_| bad()));
    let (Some(h), Some(mi)) = (t.next(), t.next()) else { return Err(bad()) };
    let second = t.next().unwrap_or(Ok(0))?;
    let (hour, minute) = (h?, mi?);
    if t.next().is_some() || frac.is_empty() || frac.len() > 3 || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad());
    }
    let ms = format!("{frac:0<3}").parse::<u32>().map_err(|_| bad())?;
    let days = days_from_civil(year, month.clamp(1, 12), 1);
    let real = (1..=12).contains(&month) && day >= 1 && civil_from_days(days + i64::from(day) - 1) == (year, month, day);
    if !real || hour > 23 || minute > 59 || second > 59 {
        return Err(format!("now: `{s}` is not a real date and time"));
    }
    if !(2008..=2100).contains(&year) {
        return Err(format!("now: the zone table's daylight rules hold for 2008 to 2100, not {year}"));
    }
    let dow = (days_from_civil(year, month, day) + 4).rem_euclid(7) as u32;
    Ok(Tm::new(year, month, day, dow, hour, minute, second, ms))
}

fn canned(key: &str, v: &Value) -> Result<Vec<Canned>, String> {
    list(key, v)?
        .iter()
        .map(|item| {
            let Value::Object(o) = item else { return Err(format!("{key} holds {{url, status, body}} objects, not {}", show(item))) };
            if let Some(k) = o.keys().find(|k| !["url", "status", "body", "content_type"].contains(&k.as_str())) {
                return Err(format!("{key}: unknown field `{k}`{}", suggest(k, &[&["url", "status", "body", "content_type"]])));
            }
            let url = o.get("url").and_then(Value::as_str).ok_or_else(|| format!("{key}: every answer needs a `url`"))?.to_string();
            let status = match o.get("status") {
                None => 200,
                Some(s) => s.as_u64().filter(|s| (100..600).contains(s)).ok_or_else(|| format!("{key}: status is an HTTP code, not {}", show(s)))? as u16,
            };
            let body = match o.get("body") {
                None => String::new(),
                Some(Value::String(s)) => s.clone(),
                Some(other) => other.to_string(),
            };
            let content_type = match o.get("content_type") {
                Some(c) => text("content_type", c)?,
                None if body.trim_start().starts_with(['{', '[']) => "application/json".into(),
                None => "text/plain".into(),
            };
            Ok(Canned { url, status, content_type, body })
        })
        .collect()
}

fn seams(key: &str, v: &Value) -> Result<BTreeSet<Seam>, String> {
    let names: Vec<String> = match v {
        Value::String(s) => s.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect(),
        other => list(key, other)?.iter().map(|i| text(key, i)).collect::<Result<_, _>>()?,
    };
    let all: Vec<&str> = Seam::ALL.iter().map(|s| s.name()).collect();
    names.iter().map(|n| Seam::ALL.iter().copied().find(|s| s.name() == n.to_lowercase()).ok_or_else(|| format!("{key}: `{n}` is not a seam ({}){}", all.join(", "), suggest(&n.to_lowercase(), &[&all])))).collect()
}

impl Pins {
    /// Sets the Pin `key` to `value` (`loose` JSON or text). An unknown key or a value out
    /// of range is an error that says what was expected and, for a near miss, what was
    /// meant; nothing is changed then. Conflicts between Pins (a `sys` Pin with `sys` real)
    /// are found by `check`, since they depend on the order the keys come in.
    pub fn set(&mut self, key: &str, v: &Value) -> Result<(), String> {
        let mut next = self.clone();
        next.apply(key, v)?;
        *self = next;
        Ok(())
    }

    fn apply(&mut self, key: &str, v: &Value) -> Result<(), String> {
        match key {
            "now" => self.now = parse_now(&text(key, v)?)?,
            "zone" => {
                let z = text(key, v)?;
                let keys: Vec<&str> = ZONE_TABLE.iter().map(|(k, ..)| *k).collect();
                self.zone = keys.iter().find(|k| **k == z).ok_or_else(|| format!("zone: `{z}` is not in the zone table{}", suggest(&z, &[&keys])))?.to_string();
            }
            "locale" => self.locale = choice(key, v, LOCALES).map_err(|e| format!("{e} (only locales with name tables)"))?.into(),
            "sys.cpu" => self.sys.cpu = percent(key, v)?,
            "sys.ram" => self.sys.ram = percent(key, v)?,
            "sys.disk" => self.sys.disk = percent(key, v)?,
            "sys.net_down" => self.sys.net_down = ranged(key, v, 1e6)?,
            "sys.net_up" => self.sys.net_up = ranged(key, v, 1e6)?,
            "sys.processes" => self.sys.processes = ranged(key, v, f64::from(u32::MAX))?.round() as u32,
            "sys.uptime" => self.sys.uptime = ranged(key, v, 1e9)?.round() as u64,
            "sys.battery" => self.sys.battery = if v.is_null() { None } else { Some(percent(key, v)?) },
            "sys.charging" => self.sys.charging = flag(key, v)?,
            "sys.gpus" => {
                let items = if v.is_number() { std::slice::from_ref(v) } else { list(key, v)? };
                if items.len() > 4 {
                    return Err("sys.gpus lists at most 4 adapters".into());
                }
                self.sys.gpus = items.iter().map(|i| percent(key, i)).collect::<Result<_, _>>()?;
            }
            "media.active" => self.media.active = flag(key, v)?,
            "media.playing" => self.media.playing = flag(key, v)?,
            "media.title" => self.media.title = text(key, v)?,
            "media.artist" => self.media.artist = text(key, v)?,
            "media.album" => self.media.album = text(key, v)?,
            "media.source" => self.media.source = text(key, v)?,
            "media.position" => self.media.position = ranged(key, v, 1e6)?,
            "media.duration" => self.media.duration = ranged(key, v, 1e6)?,
            "media.can_seek" => self.media.can_seek = flag(key, v)?,
            "media.art" => self.media.art = text(key, v)?,
            "audio" => self.audio.silence = choice(key, v, &["tone", "silence"])? == "silence",
            "audio.tones" => {
                let items = if v.is_number() { std::slice::from_ref(v) } else { list(key, v)? };
                let tones = items.iter().map(|i| num(key, i).and_then(|hz| if hz > 0.0 && hz < 24_000.0 { Ok(hz as f32) } else { Err(format!("{key} are frequencies from 0 to 24000 Hz, not {hz}")) }));
                self.audio.tones = tones.collect::<Result<_, _>>()?;
            }
            "audio.amp" => self.audio.amp = ranged(key, v, 1.0)? as f32,
            "icons" => self.icons = if choice(key, v, &["tiles", "system"])? == "system" { IconMode::System } else { IconMode::Tiles },
            "fonts" => {
                choice(key, v, &["system"]).map_err(|e| format!("{e}; there are no bundled fonts, text is measured with the machine's"))?;
                self.fonts = FontMode::System;
            }
            "fetch" => {
                self.fetch = match choice(key, v, &["offline", "real"])? {
                    "real" => FetchMode::Real,
                    _ => FetchMode::Offline,
                }
            }
            "fetch.responses" => self.fetch = FetchMode::Canned(canned(key, v)?),
            "settle" => self.settle = seconds(key, v)?,
            "scale" => self.scale = num(key, v).ok().filter(|s| *s > 0.0 && *s <= 4.0).ok_or_else(|| format!("scale is a number from 0 to 4, not {}", show(v)))? as f32,
            "palette" => self.palette = text(key, v)?,
            "anim" => self.anim = if v.is_null() { None } else { Some(choice(key, v, ANIM)?.into()) },
            "transparent" => self.transparent = flag(key, v)?,
            "backdrop" => {
                let s = text(key, v)?;
                self.backdrop = match s.strip_prefix('#').filter(|h| h.len() == 6 && h.bytes().all(|b| b.is_ascii_hexdigit())) {
                    Some(h) => Backdrop::Solid([0, 2, 4].map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0))),
                    None if s == "gradient" => Backdrop::Gradient,
                    None => return Err(format!("backdrop is a colour like #1e2a3c, or gradient, not `{s}`")),
                };
            }
            "real" => self.real = seams(key, v)?,
            _ => return Err(format!("unknown pin `{key}`{} (the pins are {})", suggest(key, &[KEYS]), KEYS.join(", "))),
        }
        Ok(())
    }

    /// Every Pin as the `(key, value)` that `set` would take to make it, in `KEYS` order, for
    /// the render's record of the environment. Setting these on a default `Pins` gives an equal
    /// one. A canned network is one `fetch.responses` entry instead of `fetch`.
    pub fn entries(&self) -> Vec<(&'static str, Value)> {
        use serde_json::json;
        let f32v = |n: f32| json!((f64::from(n) * 1e6).round() / 1e6);
        let t = &self.now;
        let now = if t.ms == 0 { format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute, t.second) } else { format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}", t.year, t.month, t.day, t.hour, t.minute, t.second, t.ms) };
        let (fetch, responses) = match &self.fetch {
            FetchMode::Offline => (Some(json!("offline")), None),
            FetchMode::Real => (Some(json!("real")), None),
            FetchMode::Canned(c) => (None, Some(Value::Array(c.iter().map(|a| json!({ "url": a.url, "status": a.status, "content_type": a.content_type, "body": a.body })).collect()))),
        };
        let mut out: Vec<(&'static str, Value)> = vec![
            ("now", json!(now)),
            ("zone", json!(self.zone)),
            ("locale", json!(self.locale)),
            ("sys.cpu", json!(self.sys.cpu)),
            ("sys.ram", json!(self.sys.ram)),
            ("sys.disk", json!(self.sys.disk)),
            ("sys.net_down", json!(self.sys.net_down)),
            ("sys.net_up", json!(self.sys.net_up)),
            ("sys.processes", json!(self.sys.processes)),
            ("sys.uptime", json!(self.sys.uptime)),
            ("sys.battery", json!(self.sys.battery)),
            ("sys.charging", json!(self.sys.charging)),
            ("sys.gpus", json!(self.sys.gpus)),
            ("media.active", json!(self.media.active)),
            ("media.playing", json!(self.media.playing)),
            ("media.title", json!(self.media.title)),
            ("media.artist", json!(self.media.artist)),
            ("media.album", json!(self.media.album)),
            ("media.source", json!(self.media.source)),
            ("media.position", json!(self.media.position)),
            ("media.duration", json!(self.media.duration)),
            ("media.can_seek", json!(self.media.can_seek)),
            ("media.art", json!(self.media.art)),
            ("audio", json!(if self.audio.silence { "silence" } else { "tone" })),
            ("audio.tones", Value::Array(self.audio.tones.iter().map(|hz| f32v(*hz)).collect())),
            ("audio.amp", f32v(self.audio.amp)),
            ("icons", json!(match self.icons { IconMode::Tiles => "tiles", IconMode::System => "system" })),
            ("fonts", json!("system")),
        ];
        out.extend(fetch.map(|f| ("fetch", f)));
        out.extend(responses.map(|r| ("fetch.responses", r)));
        out.extend([
            ("settle", json!(self.settle.as_secs_f64())),
            ("scale", f32v(self.scale)),
            ("palette", json!(self.palette)),
            ("anim", json!(self.anim)),
            ("transparent", json!(self.transparent)),
            ("backdrop", json!(match self.backdrop { Backdrop::Gradient => "gradient".to_string(), Backdrop::Solid([r, g, b]) => format!("#{r:02x}{g:02x}{b:02x}") })),
            ("real", json!(self.real.iter().map(|s| s.name()).collect::<Vec<_>>())),
        ]);
        out
    }

    /// Finds Pins that cannot both hold: a value pinned for a seam that is taken from the
    /// machine, or a charging state with no battery.
    pub fn check(&self) -> Result<(), String> {
        let d = Pins::default();
        let pinned = [
            (Seam::Clock, self.now != d.now || self.zone != d.zone || self.locale != d.locale, "now, zone or locale"),
            (Seam::Sys, self.sys != d.sys, "a sys.* pin"),
            (Seam::Media, self.media != d.media, "a media.* pin"),
            (Seam::Audio, self.audio != d.audio, "an audio pin"),
            (Seam::Fetch, self.fetch != d.fetch, "a fetch pin"),
        ];
        if let Some((seam, _, what)) = pinned.iter().find(|(s, set, _)| *set && self.real.contains(s)) {
            return Err(format!("{what} is set but `{}` is real; a real seam cannot be pinned", seam.name()));
        }
        if self.sys.charging && self.sys.battery.is_none() {
            return Err("sys.charging needs sys.battery".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The default Pins with `key` set to `value`, which must be accepted.
    fn with(key: &str, value: Value) -> Pins {
        let mut p = Pins::default();
        p.set(key, &value).unwrap_or_else(|e| panic!("{key}={value}: {e}"));
        p
    }

    fn err(key: &str, value: Value) -> String {
        let mut p = Pins::default();
        let e = p.set(key, &value).expect_err(key);
        assert_eq!(p, Pins::default(), "a refused value changes nothing");
        e
    }

    #[test]
    fn the_defaults_are_the_documented_environment() {
        let p = Pins::default();
        // 2026-01-15 is a Thursday, and the stored weekday agrees with the date
        assert_eq!(p.now, Tm::from_unix_ms(p.now.unix_ms()));
        assert_eq!((p.now.unix_ms(), p.now.dow), (1_768_471_830_000, 4));
        assert_eq!((p.zone.as_str(), p.locale.as_str(), p.settle, p.scale, p.palette.as_str()), ("UTC", "en-US", Duration::from_secs(2), 1.25, "Midnight"));
        assert_eq!((p.sys.cpu, p.media.title.as_str(), p.audio.tones.len(), p.icons, p.fetch.clone(), p.anim.clone(), p.transparent), (37, "Example Song", 3, IconMode::Tiles, FetchMode::Offline, None, false));
        assert!(p.real.is_empty() && p.check().is_ok());
    }

    #[test]
    fn every_listed_key_is_handled() {
        // a handled key fails on a null value with its own message, never as unknown
        for k in KEYS {
            let e = Pins::default().set(k, &Value::Null).err().unwrap_or_default();
            assert!(!e.starts_with("unknown pin"), "{k} is listed but not handled");
        }
        assert_eq!(KEYS.len(), KEYS.iter().collect::<BTreeSet<_>>().len(), "no key twice");
    }

    #[test]
    fn now_takes_an_iso_local_time() {
        let p = with("now", json!("2026-03-08T02:30:00"));
        assert_eq!(p.now, Tm::new(2026, 3, 8, 0, 2, 30, 0, 0), "a Sunday");
        assert_eq!(with("now", json!("2026-12-31 23:59:58.25")).now, Tm::new(2026, 12, 31, 4, 23, 59, 58, 250));
        assert_eq!(with("now", json!("2026-03-08T15:42")).now.second, 0);
        for bad in ["2026-02-30T10:00:00", "2026-13-01T10:00:00", "2026-03-08T24:00:00", "2026-03-08", "soon", "1999-01-01T00:00:00", "2026-03-08T10:00:00.1234"] {
            assert!(Pins::default().set("now", &json!(bad)).is_err(), "{bad}");
        }
        assert!(err("now", json!(5)).starts_with("now is a local time"), "a bare number is not a time");
    }

    #[test]
    fn zone_and_locale_name_what_the_fixed_calendar_has() {
        assert_eq!(with("zone", json!("Tokyo Standard Time")).zone, "Tokyo Standard Time");
        assert!(err("zone", json!("Tokyo Standart Time")).contains("did you mean `Tokyo Standard Time`"));
        assert!(err("zone", json!("Egypt Standard Time")).contains("not in the zone table"));
        assert_eq!(with("locale", json!("en-us")).locale, "en-US");
        assert!(err("locale", json!("pl-PL")).contains("not one of en-US"));
    }

    #[test]
    fn sys_pins_take_percentages_rates_and_counts() {
        let p = with("sys.cpu", json!(42));
        assert_eq!(p.sys, SysPins { cpu: 42, ..SysPins::default() });
        assert_eq!(with("sys.ram", json!(71.4)).sys.ram, 71);
        assert_eq!(with("sys.disk", json!(90)).sys.disk, 90);
        assert_eq!(with("sys.net_down", json!(12.5)).sys.net_down, 12.5);
        assert_eq!(with("sys.net_up", json!(0)).sys.net_up, 0.0);
        assert_eq!(with("sys.processes", json!(300)).sys.processes, 300);
        assert_eq!(with("sys.uptime", json!(60)).sys.uptime, 60);
        assert_eq!((with("sys.battery", json!(80)).sys.battery, with("sys.battery", json!(null)).sys.battery), (Some(80), None));
        assert!(with("sys.charging", json!(true)).sys.charging);
        assert_eq!(with("sys.gpus", json!([10, 90])).sys.gpus, [10, 90]);
        assert_eq!((with("sys.gpus", json!(33)).sys.gpus, with("sys.gpus", json!([])).sys.gpus), (vec![33], vec![]));
        for (k, v) in [("sys.cpu", json!(101)), ("sys.cpu", json!("high")), ("sys.ram", json!(-1)), ("sys.net_down", json!(-2)), ("sys.charging", json!("yes")), ("sys.gpus", json!([1, 2, 3, 4, 5])), ("sys.battery", json!(120))] {
            err(k, v);
        }
    }

    #[test]
    fn media_pins_describe_the_track() {
        let p = with("media.title", json!("Blue in Green"));
        assert_eq!(p.media, MediaPins { title: "Blue in Green".into(), ..MediaPins::default() });
        assert_eq!(with("media.title", json!(1984)).media.title, "1984", "a number reads as text");
        assert_eq!(with("media.artist", json!("Miles")).media.artist, "Miles");
        assert_eq!(with("media.album", json!("Kind of Blue")).media.album, "Kind of Blue");
        assert_eq!(with("media.source", json!("Spotify")).media.source, "Spotify");
        assert!(!with("media.active", json!(false)).media.active);
        assert!(with("media.playing", json!(true)).media.playing);
        assert_eq!(with("media.position", json!(12.5)).media.position, 12.5);
        assert_eq!(with("media.duration", json!(300)).media.duration, 300.0);
        assert!(!with("media.can_seek", json!(false)).media.can_seek);
        assert_eq!(with("media.art", json!("cover.png")).media.art, "cover.png");
        err("media.playing", json!("maybe"));
        err("media.position", json!(-1));
    }

    #[test]
    fn audio_is_a_tone_or_silence_with_its_own_tones_and_level() {
        assert!(with("audio", json!("silence")).audio.silence);
        assert!(!with("audio", json!("tone")).audio.silence);
        assert_eq!(with("audio.tones", json!([220, 880.5])).audio.tones, [220.0, 880.5]);
        assert_eq!(with("audio.tones", json!(440)).audio.tones, [440.0]);
        assert_eq!(with("audio.amp", json!(0.5)).audio.amp, 0.5);
        assert!(err("audio", json!("loud")).contains("tone, silence"));
        err("audio.tones", json!([0]));
        err("audio.tones", json!([30000]));
        err("audio.amp", json!(2));
    }

    #[test]
    fn icons_fonts_and_fetch_choose_a_mode() {
        assert_eq!(with("icons", json!("system")).icons, IconMode::System);
        assert_eq!(with("icons", json!("tiles")).icons, IconMode::Tiles);
        assert_eq!(with("fonts", json!("system")).fonts, FontMode::System);
        let e = err("fonts", json!("bundled"));
        assert!(e.contains("no bundled fonts"), "{e}");
        assert_eq!(with("fetch", json!("real")).fetch, FetchMode::Real);
        assert_eq!(with("fetch", json!("offline")).fetch, FetchMode::Offline);
        let p = with("fetch.responses", json!([{ "url": "https://api.example/weather?city=Oslo", "body": { "temp": -3 } }, { "url": "https://x.example/", "status": 404 }]));
        assert_eq!(
            p.fetch,
            FetchMode::Canned(vec![
                Canned { url: "https://api.example/weather?city=Oslo".into(), status: 200, content_type: "application/json".into(), body: r#"{"temp":-3}"#.into() },
                Canned { url: "https://x.example/".into(), status: 404, content_type: "text/plain".into(), body: String::new() },
            ])
        );
        assert!(err("fetch.responses", json!([{ "ulr": "x" }])).contains("did you mean `url`"));
        err("fetch.responses", json!([{ "body": "no url" }]));
        assert!(err("icons", json!("sytem")).contains("did you mean `system`"));
    }

    #[test]
    fn the_look_pins_are_settle_scale_palette_anim_and_backdrop() {
        assert_eq!(with("settle", json!(3)).settle, Duration::from_secs(3));
        assert_eq!(with("settle", json!("500ms")).settle, Duration::from_millis(500));
        assert_eq!(with("settle", json!("2s")).settle, Duration::from_secs(2));
        err("settle", json!("soon"));
        err("settle", json!(120));
        assert_eq!(with("scale", json!(2)).scale, 2.0);
        err("scale", json!(0));
        err("scale", json!(5));
        assert_eq!(with("palette", json!("Dawn")).palette, "Dawn");
        assert_eq!(with("anim", json!("off")).anim.as_deref(), Some("off"));
        assert_eq!(with("anim", json!(null)).anim, None);
        assert!(err("anim", json!("slow")).contains("off, fast, normal, relaxed"));
        assert!(with("transparent", json!(true)).transparent);
        assert_eq!(with("backdrop", json!("#1e2a3c")).backdrop, Backdrop::Solid([0x1e, 0x2a, 0x3c]));
        assert_eq!(with("backdrop", json!("gradient")).backdrop, Backdrop::Gradient);
        err("backdrop", json!("blue"));
    }

    #[test]
    fn real_seams_come_as_a_list_or_a_comma_string() {
        assert_eq!(with("real", json!("clock, sys")).real, BTreeSet::from([Seam::Clock, Seam::Sys]));
        assert_eq!(with("real", json!(["gpu"])).real, BTreeSet::from([Seam::Gpu]));
        assert!(err("real", json!("clok")).contains("did you mean `clock`"));
    }

    #[test]
    fn entries_set_on_a_default_give_the_same_pins_back() {
        let mut p = Pins::default();
        for (k, v) in [("now", json!("2026-03-08T15:42:10.25")), ("zone", json!("Tokyo Standard Time")), ("sys.cpu", json!(9)), ("sys.battery", json!(40)), ("sys.gpus", json!([1, 2])), ("media.playing", json!(true)), ("media.art", json!("c.png")), ("audio.tones", json!([220.5])), ("audio.amp", json!(0.3)), ("icons", json!("system")), ("settle", json!("500ms")), ("scale", json!(1.5)), ("anim", json!("off")), ("backdrop", json!("#1e2a3c")), ("real", json!("gpu"))] {
            p.set(k, &v).unwrap();
        }
        let back = |p: &Pins| {
            let mut q = Pins::default();
            for (k, v) in p.entries() {
                q.set(k, &v).unwrap_or_else(|e| panic!("{k}={v}: {e}"));
            }
            q
        };
        assert_eq!(back(&p), p);
        assert_eq!(back(&Pins::default()), Pins::default());
        p.set("fetch.responses", &json!([{ "url": "https://a.example/", "body": "x" }])).unwrap();
        assert_eq!(back(&p), p, "a canned network comes back as fetch.responses");
        let keys: Vec<&str> = p.entries().iter().map(|(k, _)| *k).collect();
        assert!(keys.iter().all(|k| KEYS.contains(k)) && keys.len() == KEYS.len() - 1, "every key but the other spelling of fetch");
        assert_eq!(Pins::default().entries()[0].1, json!("2026-01-15T10:10:30"));
    }

    #[test]
    fn an_unknown_key_suggests_the_nearest_and_changes_nothing() {
        let e = err("sys.cpo", json!(1));
        assert!(e.starts_with("unknown pin `sys.cpo` (did you mean `sys.cpu`?)"), "{e}");
        assert!(err("transparant", json!(true)).contains("did you mean `transparent`"));
        assert!(err("nothing.like.it", json!(1)).starts_with("unknown pin `nothing.like.it` (the pins are"), "no guess when none is near");
    }

    #[test]
    fn a_pin_for_a_real_seam_or_a_charge_without_a_battery_is_a_conflict() {
        let mut p = Pins::default();
        p.set("real", &json!("sys")).unwrap();
        assert!(p.check().is_ok());
        p.set("sys.cpu", &json!(42)).unwrap();
        assert!(p.check().unwrap_err().contains("`sys` is real"));
        let mut c = Pins::default();
        c.set("sys.charging", &json!(true)).unwrap();
        assert_eq!(c.check(), Err("sys.charging needs sys.battery".into()));
        c.set("sys.battery", &json!(50)).unwrap();
        assert!(c.check().is_ok());
    }
}
