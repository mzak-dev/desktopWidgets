//! The system media session (SMTC) as a Data Source: what is playing in any app that reports
//! it, with play/pause, next and previous. Built in because WebAssembly cannot reach WinRT
//! (ADR-0009). The session itself is a `MediaBackend` (a thread waiting on SMTC's events,
//! never polling); this is the rule for when the widgets redraw and how a verb is read.
//!
//! A new track, play or pause, a seek or another app taking over redraws the widgets. While
//! playing, `position`, `progress` and `clock` tick once a second; paused, nothing does.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::{Cadence, DataSource, Notifier, SourceCx};
use crate::ambient::{Control, MediaBackend, Notify};
use crate::value::Value;

impl Notify for Notifier {
    fn changed(&self) {
        Notifier::changed(self);
    }

    fn log(&self, line: String) {
        Notifier::log(self, line);
    }
}

/// `media.seek 0.4213`: how far into the track, 0-1; anything but a number is refused.
fn seek_arg(arg: &str) -> Option<f64> {
    let f: f64 = arg.trim().parse().ok()?;
    f.is_finite().then(|| f.clamp(0.0, 1.0))
}

pub struct Media {
    backend: Arc<dyn MediaBackend>,
    /// The backend has been asked something. It starts watching the session at the first
    /// read, unless an action came first.
    asked: AtomicBool,
}

impl Media {
    pub fn new(backend: Arc<dyn MediaBackend>) -> Media {
        Media { backend, asked: AtomicBool::new(false) }
    }
}

impl DataSource for Media {
    fn name(&self) -> &str {
        "media"
    }

    fn value(&self, cx: &SourceCx) -> Value {
        if !self.asked.swap(true, Ordering::Relaxed) {
            self.backend.control(Control::Session);
        }
        self.backend.track().value(cx.now())
    }

    fn cadence(&self, field: &str, _cx: &SourceCx) -> Option<Cadence> {
        let ticking = matches!(field, "" | "position" | "progress" | "clock");
        (ticking && self.backend.track().playing).then_some(Cadence::Second)
    }

    fn act(&self, verb: &str, arg: &str, _cx: &SourceCx) -> bool {
        let control = match verb {
            "play_pause" => Control::PlayPause,
            "next" => Control::Next,
            "prev" => Control::Prev,
            "seek" => match seek_arg(arg) {
                Some(f) => Control::Seek(f),
                None => return false,
            },
            _ => return false,
        };
        self.asked.store(true, Ordering::Relaxed);
        self.backend.control(control);
        true
    }

    fn attach(&self, notify: Notifier) {
        self.backend.attach(Arc::new(notify));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ambient::{FixedMedia, Track};
    use std::collections::BTreeMap;
    use std::time::{Duration, Instant};

    fn cx<'a>(cfg: &'a BTreeMap<String, serde_json::Value>, params: &'a BTreeMap<String, Value>) -> SourceCx<'a> {
        SourceCx::new(super::super::InstanceRef::new("", cfg), params, crate::data::Tm::new(2026, 9, 21, 1, 12, 0, 0, 0), "Default")
    }

    fn playing(position: f64, at: Instant) -> Track {
        Track { active: true, title: "Song".into(), artist: "Band".into(), source: "Spotify".into(), playing: true, position, duration: 200.0, at: Some(at), ..Default::default() }
    }

    #[test]
    fn seek_takes_a_fraction_and_refuses_anything_else() {
        let fake = Arc::new(FixedMedia::default());
        let m = Media::new(fake.clone());
        let (cfg, params) = (BTreeMap::new(), BTreeMap::new());
        let cx = cx(&cfg, &params);
        assert!(m.act("seek", "0.5", &cx));
        assert_eq!(fake.controls(), [Control::Seek(0.5)]);
        assert!(m.act("seek", "7", &cx));
        assert_eq!(fake.controls().last(), Some(&Control::Seek(1.0)), "clamped to the end");
        assert!(m.act("seek", "-3", &cx));
        assert_eq!(fake.controls().last(), Some(&Control::Seek(0.0)), "and to the start");
        assert!(!m.act("seek", "x", &cx) && !m.act("seek", "NaN", &cx) && !m.act("seek", "inf", &cx) && !m.act("seek", "", &cx));
        assert_eq!(fake.controls().len(), 3, "nothing sent for a bad one");
    }

    #[test]
    fn the_transport_verbs_reach_the_session_and_others_are_refused() {
        let fake = Arc::new(FixedMedia::default());
        let m = Media::new(fake.clone());
        let (cfg, params) = (BTreeMap::new(), BTreeMap::new());
        let cx = cx(&cfg, &params);
        assert!(m.act("play_pause", "", &cx) && m.act("next", "", &cx) && m.act("prev", "", &cx));
        assert_eq!(fake.controls(), [Control::PlayPause, Control::Next, Control::Prev]);
        assert!(!m.act("eject", "", &cx));
        assert_eq!(fake.controls().len(), 3);
    }

    #[test]
    fn the_session_is_started_by_the_first_read_once_unless_an_action_came_first() {
        let (cfg, params) = (BTreeMap::new(), BTreeMap::new());
        let cx = cx(&cfg, &params);
        let fake = Arc::new(FixedMedia::default());
        let m = Media::new(fake.clone());
        assert!(fake.controls().is_empty(), "nothing starts before it is asked");
        m.cadence("position", &cx);
        assert!(fake.controls().is_empty(), "a cadence question is not a read");
        m.value(&cx);
        m.value(&cx);
        assert_eq!(fake.controls(), [Control::Session]);
        let fake = Arc::new(FixedMedia::default());
        let m = Media::new(fake.clone());
        m.act("next", "", &cx);
        m.value(&cx);
        assert_eq!(fake.controls(), [Control::Next], "the thread is already running");
    }

    #[test]
    fn a_source_reads_the_now_it_is_given() {
        let t0 = Instant::now();
        let m = Media::new(Arc::new(FixedMedia::new(playing(60.0, t0))));
        let (saved, params) = (BTreeMap::new(), BTreeMap::new());
        let at = |now| m.value(&cx(&saved, &params).with_now(now)).get("position").cloned();
        assert_eq!(at(t0), Some(Value::Num(60.0)));
        assert_eq!(at(t0 + Duration::from_secs(2)), Some(Value::Num(62.0)), "the injected now, not the wall clock");
    }

    #[test]
    fn the_fixed_session_reads_as_a_paused_seekable_track() {
        let m = Media::new(Arc::new(FixedMedia::default()));
        let (cfg, params) = (BTreeMap::new(), BTreeMap::new());
        let v = m.value(&cx(&cfg, &params));
        assert_eq!((v.get("active"), v.get("playing"), v.get("can_seek"), v.get("title")), (Some(&Value::Bool(true)), Some(&Value::Bool(false)), Some(&Value::Bool(true)), Some(&Value::Str("Song".into()))));
        assert_eq!((v.get("clock"), v.get("length"), v.get("progress")), (Some(&Value::Str("1:00".into())), Some(&Value::Str("3:20".into())), Some(&Value::Num(0.3))));
    }

    #[test]
    fn it_ticks_only_while_playing() {
        let (cfg, params) = (BTreeMap::new(), BTreeMap::new());
        let cx = cx(&cfg, &params);
        let paused = Media::new(Arc::new(FixedMedia::default()));
        assert_eq!(paused.cadence("position", &cx), None, "paused or nothing playing");
        let m = Media::new(Arc::new(FixedMedia::new(playing(0.0, Instant::now()))));
        assert_eq!((m.cadence("progress", &cx), m.cadence("title", &cx)), (Some(Cadence::Second), None));
    }
}
