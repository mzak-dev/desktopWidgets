//! The system media session as the engine sees it: a `Track` to read, `Control`s to send and
//! a line back to say something changed. The Windows adapter is a thread on SMTC's events;
//! the fixed one is a track that stays put and remembers what it was told.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use crate::value::{Value, clock_text};

/// A position this far from where it should be is a seek, worth a redraw while paused.
const SEEK_SECS: f64 = 2.0;

/// What is playing, as last read. The position runs on from `at` while playing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub active: bool,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// The app playing it, by name ("Spotify").
    pub source: String,
    pub playing: bool,
    /// The app lets `media.seek` move the position.
    pub can_seek: bool,
    /// `file:` path of the album art, or "".
    pub art: String,
    pub position: f64,
    pub duration: f64,
    pub at: Option<Instant>,
}

impl Track {
    /// Seconds into the track at `now`.
    pub fn position_at(&self, now: Instant) -> f64 {
        let ran = match (self.playing, self.at) {
            (true, Some(at)) => now.saturating_duration_since(at).as_secs_f64(),
            _ => 0.0,
        };
        let p = self.position + ran;
        if self.duration > 0.0 { p.clamp(0.0, self.duration) } else { p.max(0.0) }
    }

    pub fn value(&self, now: Instant) -> Value {
        let position = self.position_at(now);
        let progress = if self.duration > 0.0 { position / self.duration } else { 0.0 };
        let mut m = BTreeMap::new();
        m.insert("active".into(), Value::Bool(self.active));
        m.insert("title".into(), Value::Str(self.title.clone()));
        m.insert("artist".into(), Value::Str(self.artist.clone()));
        m.insert("album".into(), Value::Str(self.album.clone()));
        m.insert("source".into(), Value::Str(self.source.clone()));
        m.insert("playing".into(), Value::Bool(self.playing));
        m.insert("can_seek".into(), Value::Bool(self.can_seek));
        m.insert("art".into(), Value::Str(self.art.clone()));
        m.insert("position".into(), Value::Num(position.floor()));
        m.insert("duration".into(), Value::Num(self.duration.round()));
        m.insert("progress".into(), Value::Num(progress));
        m.insert("clock".into(), Value::Str(clock_text(position)));
        m.insert("length".into(), Value::Str(if self.duration > 0.0 { clock_text(self.duration) } else { String::new() }));
        Value::Obj(m)
    }

    /// Whether the widgets must redraw for `new`: another track, play or pause, new art or a
    /// seek. The position running on as expected is not a change.
    pub fn differs(&self, new: &Track, now: Instant) -> bool {
        let same = (&self.active, self.playing, self.can_seek, &self.art) == (&new.active, new.playing, new.can_seek, &new.art) && same_track(self, new);
        !same || (self.duration - new.duration).abs() > 0.5 || (self.position_at(now) - new.position_at(now)).abs() > SEEK_SECS
    }
}

/// The same song from the same app, whatever its position or cover.
pub(crate) fn same_track(a: &Track, b: &Track) -> bool {
    (&a.source, &a.title, &a.artist, &a.album) == (&b.source, &b.title, &b.artist, &b.album)
}

/// What the media source asks of the session.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Control {
    /// Start listening to the current session. The source sends it before the first read; a
    /// real backend starts nothing until it is asked.
    Session,
    PlayPause,
    Next,
    Prev,
    /// Move to this fraction of the track, 0-1.
    Seek(f64),
}

/// A source's line back to the app, from any thread.
pub trait Notify: Send + Sync {
    /// The track changed in a way worth a redraw.
    fn changed(&self);
    /// A line for Wayfinder's log.
    fn log(&self, line: String);
}

/// The system media session. `track` says what is known now and never blocks; the backend
/// keeps it current on its own and says so through the `Notify` it is given in `attach`.
pub trait MediaBackend: Send + Sync {
    fn track(&self) -> Track;
    fn control(&self, control: Control);
    /// Called once, when the source is registered.
    fn attach(&self, notify: Arc<dyn Notify>);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn playing(position: f64, at: Instant) -> Track {
        Track { active: true, title: "Song".into(), artist: "Band".into(), source: "Spotify".into(), playing: true, position, duration: 200.0, at: Some(at), ..Default::default() }
    }

    #[test]
    fn the_position_runs_on_while_playing_and_stops_when_paused() {
        let t0 = Instant::now();
        let t = playing(60.0, t0);
        let v = t.value(t0 + Duration::from_secs(5));
        assert_eq!((v.get("position"), v.get("clock"), v.get("length")), (Some(&Value::Num(65.0)), Some(&Value::Str("1:05".into())), Some(&Value::Str("3:20".into()))));
        assert_eq!(v.get("progress"), Some(&Value::Num(65.0 / 200.0)));
        assert_eq!(t.position_at(t0 + Duration::from_secs(500)), 200.0, "never past the end");
        let paused = Track { playing: false, ..t };
        assert_eq!(paused.position_at(t0 + Duration::from_secs(5)), 60.0);
        let none = Track::default().value(t0);
        assert_eq!((none.get("active"), none.get("length"), none.get("progress")), (Some(&Value::Bool(false)), Some(&Value::Str(String::new())), Some(&Value::Num(0.0))));
    }

    #[test]
    fn only_a_change_worth_showing_redraws() {
        let t0 = Instant::now();
        let a = playing(60.0, t0);
        let later = t0 + Duration::from_secs(10);
        assert!(!a.differs(&playing(70.0, later), later), "the position ran on as expected");
        assert!(a.differs(&Track { playing: false, ..a.clone() }, later), "paused");
        assert!(a.differs(&Track { title: "Next".into(), ..a.clone() }, later), "a new track");
        assert!(a.differs(&playing(150.0, later), later), "a seek");
    }

    #[test]
    fn a_seekable_player_says_so() {
        let t0 = Instant::now();
        let a = playing(60.0, t0);
        let b = Track { can_seek: true, ..a.clone() };
        assert!(a.differs(&b, t0), "the bar appears as soon as the app allows seeking");
        assert_eq!(b.value(t0).get("can_seek"), Some(&Value::Bool(true)));
    }

    #[test]
    fn times_read_well() {
        assert_eq!((clock_text(0.0), clock_text(7.9), clock_text(187.0), clock_text(3723.0)), ("0:00".into(), "0:07".into(), "3:07".into(), "1:02:03".into()));
    }
}
