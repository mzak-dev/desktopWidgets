//! The system media session (SMTC): what is playing in any app that reports it, with
//! play/pause, next and previous. Built in because WebAssembly cannot reach WinRT (ADR-0009).
//!
//! A thread starts at the first read and then waits on SMTC's events, never polling: a new
//! track, play or pause, a seek or another app taking over wakes it, and only a change worth
//! showing redraws the widgets. While playing, `position`, `progress` and `clock` tick once
//! a second; paused, nothing does.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::{Cadence, DataSource, Notifier, SourceCx};
use crate::value::Value;

/// Album art files kept in the cache folder; older ones go.
const ART_KEPT: usize = 32;
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
        m.insert("art".into(), Value::Str(self.art.clone()));
        m.insert("position".into(), Value::Num(position.floor()));
        m.insert("duration".into(), Value::Num(self.duration.round()));
        m.insert("progress".into(), Value::Num(progress));
        m.insert("clock".into(), Value::Str(clock(position)));
        m.insert("length".into(), Value::Str(if self.duration > 0.0 { clock(self.duration) } else { String::new() }));
        Value::Obj(m)
    }

    /// Whether the widgets must redraw for `new`: another track, play or pause, new art or a
    /// seek. The position running on as expected is not a change.
    pub fn differs(&self, new: &Track, now: Instant) -> bool {
        let same = (&self.active, &self.title, &self.artist, &self.album, &self.source, self.playing, &self.art) == (&new.active, &new.title, &new.artist, &new.album, &new.source, new.playing, &new.art);
        !same || (self.duration - new.duration).abs() > 0.5 || (self.position_at(now) - new.position_at(now)).abs() > SEEK_SECS
    }
}

/// `3:07`, or `1:02:03` past an hour.
pub fn clock(secs: f64) -> String {
    let s = secs.max(0.0).floor() as u64;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// A readable app name from its AppUserModelID: `Spotify.exe` is Spotify,
/// `Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic` is ZuneMusic.
pub fn app_name(aumid: &str) -> String {
    let id = aumid.split('!').next().unwrap_or(aumid);
    let id = id.split('_').next().unwrap_or(id);
    let id = id.strip_suffix(".exe").or_else(|| id.strip_suffix(".EXE")).unwrap_or(id);
    let name = id.rsplit(['.', '\\', '/']).next().unwrap_or(id);
    let mut c = name.chars();
    c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
}

/// The art file for a track: one name per track, so a new track is a new image id.
fn art_path(dir: &Path, t: &Track) -> PathBuf {
    let key = format!("{}\u{1}{}\u{1}{}\u{1}{}", t.source, t.title, t.artist, t.album);
    let h = key.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
    dir.join(format!("art-{h:016x}.png"))
}

/// Keeps the `keep` newest art files.
fn prune(dir: &Path, keep: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = rd.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("art-")).filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path()))).collect();
    files.sort();
    let extra = files.len().saturating_sub(keep);
    for (_, p) in files.into_iter().take(extra) {
        let _ = std::fs::remove_file(p);
    }
}

enum Msg {
    /// Something about the session changed.
    Refresh,
    /// Another app's session became current: listen to it instead.
    Session,
    Act(String),
}

pub struct Media {
    /// Album art goes here, a dot-folder so writing it never reloads content.
    cache: PathBuf,
    track: Arc<Mutex<Track>>,
    worker: Mutex<Option<Sender<Msg>>>,
    notify: Arc<Mutex<Option<Notifier>>>,
}

impl Media {
    pub fn new(cache: PathBuf) -> Media {
        Media { cache, track: Arc::default(), worker: Mutex::new(None), notify: Arc::default() }
    }

    /// The thread starts at the first read or action.
    fn send(&self, msg: Msg) {
        let mut w = self.worker.lock().unwrap();
        if w.is_none() {
            let (tx, rx) = mpsc::channel();
            let (track, notify, cache, events) = (self.track.clone(), self.notify.clone(), self.cache.clone(), tx.clone());
            let spawned = std::thread::Builder::new().name("media".into()).spawn(move || {
                if let Err(e) = watch(rx, events, &track, &notify, &cache) {
                    if let Some(n) = notify.lock().unwrap().as_ref() {
                        n.log(format!("media session unavailable: {e}"));
                    }
                }
            });
            if spawned.is_err() {
                return;
            }
            *w = Some(tx);
        }
        if let Some(tx) = w.as_ref() {
            let _ = tx.send(msg);
        }
    }
}

impl DataSource for Media {
    fn name(&self) -> &str {
        "media"
    }

    fn value(&self, _cx: &SourceCx) -> Value {
        if self.worker.lock().unwrap().is_none() {
            self.send(Msg::Session);
        }
        self.track.lock().unwrap().value(Instant::now())
    }

    fn cadence(&self, field: &str, _cx: &SourceCx) -> Option<Cadence> {
        let ticking = matches!(field, "" | "position" | "progress" | "clock");
        (ticking && self.track.lock().unwrap().playing).then_some(Cadence::Second)
    }

    fn act(&self, verb: &str, _arg: &str, _cx: &SourceCx) -> bool {
        if !matches!(verb, "play_pause" | "next" | "prev") {
            return false;
        }
        self.send(Msg::Act(verb.into()));
        true
    }

    fn attach(&self, notify: Notifier) {
        *self.notify.lock().unwrap() = Some(notify);
    }
}

use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager, GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status};
use windows::Storage::Streams::DataReader;

/// Reads the current session whenever SMTC says something changed.
fn watch(rx: Receiver<Msg>, events: Sender<Msg>, track: &Mutex<Track>, notify: &Mutex<Option<Notifier>>, cache: &Path) -> windows::core::Result<()> {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
    }
    let manager = Manager::RequestAsync()?.join()?;
    let tx = events.clone();
    manager.CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
        let _ = tx.send(Msg::Session);
        Ok(())
    }))?;
    let mut listening: Option<(Session, [i64; 3])> = None;
    while let Ok(msg) = rx.recv() {
        let session = manager.GetCurrentSession().ok();
        match msg {
            Msg::Session => {
                if let Some((s, [a, b, c])) = listening.take() {
                    let _ = (s.RemoveMediaPropertiesChanged(a), s.RemovePlaybackInfoChanged(b), s.RemoveTimelinePropertiesChanged(c));
                }
                if let Some(s) = &session {
                    let tokens = [s.MediaPropertiesChanged(&refresh_on(&events))?, s.PlaybackInfoChanged(&refresh_on(&events))?, s.TimelinePropertiesChanged(&refresh_on(&events))?];
                    listening = Some((s.clone(), tokens));
                }
            }
            Msg::Act(verb) => {
                if let Some(s) = &session {
                    let op = match verb.as_str() {
                        "play_pause" => s.TryTogglePlayPauseAsync(),
                        "next" => s.TrySkipNextAsync(),
                        _ => s.TrySkipPreviousAsync(),
                    };
                    let _ = op.and_then(|o| o.join());
                }
            }
            Msg::Refresh => {}
        }
        let now = Instant::now();
        let old = track.lock().unwrap().clone();
        let new = match &session {
            Some(s) => read(s, &old, cache).unwrap_or_default(),
            None => Track::default(),
        };
        let changed = old.differs(&new, now);
        *track.lock().unwrap() = new;
        if changed {
            if let Some(n) = notify.lock().unwrap().as_ref() {
                n.changed();
            }
        }
    }
    Ok(())
}

/// A session event handler that asks the thread to read the session again.
fn refresh_on<A: windows::core::RuntimeType + 'static>(events: &Sender<Msg>) -> TypedEventHandler<Session, A> {
    let tx = events.clone();
    TypedEventHandler::new(move |_, _| {
        let _ = tx.send(Msg::Refresh);
        Ok(())
    })
}

/// Seconds since 1601, as WinRT's `DateTime` counts.
fn winrt_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64()) + 11_644_473_600.0
}

fn read(s: &Session, old: &Track, cache: &Path) -> windows::core::Result<Track> {
    let props = s.TryGetMediaPropertiesAsync()?.join()?;
    let playing = s.GetPlaybackInfo()?.PlaybackStatus()? == Status::Playing;
    let tl = s.GetTimelineProperties()?;
    let secs = |ticks: i64| ticks as f64 / 1e7;
    let duration = (secs(tl.EndTime()?.Duration) - secs(tl.StartTime()?.Duration)).max(0.0);
    let mut position = secs(tl.Position()?.Duration);
    // apps report the position now and then; it has run on since, while playing
    if playing {
        let updated = secs(tl.LastUpdatedTime()?.UniversalTime);
        if updated > 0.0 {
            position += (winrt_now() - updated).max(0.0);
        }
    }
    let mut t = Track {
        active: true,
        title: props.Title()?.to_string(),
        artist: props.Artist()?.to_string(),
        album: props.AlbumTitle()?.to_string(),
        source: app_name(&s.SourceAppUserModelId()?.to_string()),
        playing,
        art: String::new(),
        position,
        duration,
        at: Some(Instant::now()),
    };
    let same_track = (&old.source, &old.title, &old.artist, &old.album) == (&t.source, &t.title, &t.artist, &t.album);
    t.art = if same_track && !old.art.is_empty() { old.art.clone() } else { art(&props, &t, cache).unwrap_or_default() };
    Ok(t)
}

/// Saves the track's thumbnail as a PNG in the cache and returns its image id.
fn art(props: &windows::Media::Control::GlobalSystemMediaTransportControlsSessionMediaProperties, t: &Track, cache: &Path) -> Option<String> {
    let path = art_path(cache, t);
    if !path.is_file() {
        let stream = props.Thumbnail().ok()?.OpenReadAsync().ok()?.join().ok()?;
        let size = stream.Size().ok()?.min(16 << 20) as u32;
        let reader = DataReader::CreateDataReader(&stream).ok()?;
        let n = reader.LoadAsync(size).ok()?.join().ok()?;
        let mut bytes = vec![0u8; n as usize];
        reader.ReadBytes(&mut bytes).ok()?;
        let img = image::load_from_memory(&bytes).ok()?;
        std::fs::create_dir_all(cache).ok()?;
        let part = path.with_extension("part");
        img.save_with_format(&part, image::ImageFormat::Png).ok()?;
        std::fs::rename(&part, &path).ok()?;
        prune(cache, ART_KEPT);
    }
    Some(format!("file:{}", path.display()))
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
    fn times_and_app_names_read_well() {
        assert_eq!((clock(0.0), clock(7.9), clock(187.0), clock(3723.0)), ("0:00".into(), "0:07".into(), "3:07".into(), "1:02:03".into()));
        assert_eq!(app_name("Spotify.exe"), "Spotify");
        assert_eq!(app_name("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"), "ZuneMusic");
        assert_eq!(app_name("chrome"), "Chrome");
        assert_eq!(app_name("C:\\Program Files\\foobar2000\\foobar2000.exe"), "Foobar2000");
    }

    #[test]
    fn art_has_one_file_per_track_and_old_ones_go() {
        let dir = std::env::temp_dir().join(format!("wf-media-art-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let t = playing(0.0, Instant::now());
        assert_eq!(art_path(&dir, &t), art_path(&dir, &t.clone()));
        assert_ne!(art_path(&dir, &t), art_path(&dir, &Track { title: "Other".into(), ..t.clone() }));
        for i in 0..5 {
            std::fs::write(dir.join(format!("art-{i}.png")), "x").unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        prune(&dir, 2);
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["art-3.png", "art-4.png"], "the newest stay");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn it_ticks_only_while_playing() {
        let m = Media::new(std::env::temp_dir());
        let (cfg, params) = (crate::workspace::InstanceCfg::default(), BTreeMap::new());
        let cx = SourceCx { cfg: &cfg, params: &params, tm: super::super::now_local(), icon_pack: "Default" };
        assert_eq!(m.cadence("position", &cx), None, "paused or nothing playing");
        *m.track.lock().unwrap() = playing(0.0, Instant::now());
        assert_eq!((m.cadence("progress", &cx), m.cadence("title", &cx)), (Some(Cadence::Second), None));
        assert!(!m.act("eject", "", &cx));
    }
}
