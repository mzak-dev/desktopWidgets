//! The real media backend: the system media session (SMTC), what is playing in any app that
//! reports it. Built in because WebAssembly cannot reach WinRT (ADR-0009).
//!
//! A thread starts at the first ask and then waits on SMTC's events, never polling: a new
//! track, play or pause, a seek or another app taking over wakes it, and only a change worth
//! showing redraws the widgets.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::media::{Control, MediaBackend, Notify, Track, same_track};

/// Album art files kept in the cache folder; older ones go.
const ART_KEPT: usize = 32;
/// When to read the cover again after the track changes: Spotify sends the new title before
/// its new thumbnail, and not always another event once the thumbnail catches up.
const RECHECK_MS: [u64; 3] = [500, 1500, 3000];

/// Where a seek `f` of the way through lands, in the timeline's 100 ns ticks.
fn seek_ticks(start: i64, end: i64, f: f64) -> i64 {
    start + ((end - start).max(0) as f64 * f.clamp(0.0, 1.0)).round() as i64
}

/// The cover rechecks after a track change at `at`.
fn recheck_plan(at: Instant) -> Vec<Instant> {
    RECHECK_MS.iter().map(|ms| at + Duration::from_millis(*ms)).collect()
}

/// How long to wait for an event before the next recheck is due; `None` waits for events
/// alone, so an idle player costs nothing. Rechecks that fell due together count once.
fn next_wait(plan: &mut Vec<Instant>, now: Instant) -> Option<Duration> {
    while plan.len() > 1 && plan[1] <= now {
        plan.remove(0);
    }
    plan.first().map(|d| d.saturating_duration_since(now))
}

/// A readable app name from its AppUserModelID: `Spotify.exe` is Spotify,
/// `Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic` is ZuneMusic.
fn app_name(aumid: &str) -> String {
    let id = aumid.split('!').next().unwrap_or(aumid);
    let id = id.split('_').next().unwrap_or(id);
    let id = id.strip_suffix(".exe").or_else(|| id.strip_suffix(".EXE")).unwrap_or(id);
    let name = id.rsplit(['.', '\\', '/']).next().unwrap_or(id);
    let mut c = name.chars();
    c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
}

/// The art file for a thumbnail, named by its bytes: a new picture is a new image id, and
/// a cover that arrives after its track's title can never be filed under the wrong song.
fn art_path(dir: &Path, bytes: &[u8]) -> PathBuf {
    let h = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100_0000_01b3));
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
    /// The track's title, artist, album or cover changed.
    Props,
    /// Play, pause or the position changed: the cover stays.
    Refresh,
    /// Another app's session became current: listen to it instead.
    Session,
    Act(String),
    /// Move to this fraction of the track.
    Seek(f64),
    /// A cover recheck fell due (the thread's own).
    Recheck,
}

pub struct WinMedia {
    /// Album art goes here, a dot-folder so writing it never reloads content.
    cache: PathBuf,
    track: Arc<Mutex<Track>>,
    worker: Mutex<Option<Sender<Msg>>>,
    notify: Arc<Mutex<Option<Arc<dyn Notify>>>>,
}

impl WinMedia {
    pub fn new(cache: PathBuf) -> WinMedia {
        WinMedia { cache, track: Arc::default(), worker: Mutex::new(None), notify: Arc::default() }
    }

    /// The thread starts at the first ask.
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

impl MediaBackend for WinMedia {
    fn track(&self) -> Track {
        self.track.lock().unwrap().clone()
    }

    fn control(&self, control: Control) {
        self.send(match control {
            Control::Session => Msg::Session,
            Control::PlayPause => Msg::Act("play_pause".into()),
            Control::Next => Msg::Act("next".into()),
            Control::Prev => Msg::Act("prev".into()),
            Control::Seek(f) => Msg::Seek(f),
        });
    }

    fn attach(&self, notify: Arc<dyn Notify>) {
        *self.notify.lock().unwrap() = Some(notify);
    }
}

use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager, GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status};
use windows::Storage::Streams::DataReader;

/// Reads the current session whenever SMTC says something changed.
fn watch(rx: Receiver<Msg>, events: Sender<Msg>, track: &Mutex<Track>, notify: &Mutex<Option<Arc<dyn Notify>>>, cache: &Path) -> windows::core::Result<()> {
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
    let mut rechecks: Vec<Instant> = Vec::new();
    loop {
        let msg = match next_wait(&mut rechecks, Instant::now()) {
            None => match rx.recv() {
                Ok(m) => m,
                Err(_) => break,
            },
            Some(wait) => match rx.recv_timeout(wait) {
                Ok(m) => m,
                Err(RecvTimeoutError::Timeout) => {
                    rechecks.remove(0);
                    Msg::Recheck
                }
                Err(RecvTimeoutError::Disconnected) => break,
            },
        };
        let session = manager.GetCurrentSession().ok();
        // the cover is read again unless only play, pause or the position changed
        let mut fresh_art = true;
        match msg {
            Msg::Session => {
                if let Some((s, [a, b, c])) = listening.take() {
                    let _ = (s.RemoveMediaPropertiesChanged(a), s.RemovePlaybackInfoChanged(b), s.RemoveTimelinePropertiesChanged(c));
                }
                if let Some(s) = &session {
                    let tokens = [s.MediaPropertiesChanged(&send_on(&events, || Msg::Props))?, s.PlaybackInfoChanged(&send_on(&events, || Msg::Refresh))?, s.TimelinePropertiesChanged(&send_on(&events, || Msg::Refresh))?];
                    listening = Some((s.clone(), tokens));
                }
            }
            Msg::Act(verb) => {
                fresh_art = false;
                if let Some(s) = &session {
                    let op = match verb.as_str() {
                        "play_pause" => s.TryTogglePlayPauseAsync(),
                        "next" => s.TrySkipNextAsync(),
                        _ => s.TrySkipPreviousAsync(),
                    };
                    let _ = op.and_then(|o| o.join());
                }
            }
            Msg::Seek(f) => {
                let ticks = session.as_ref().and_then(|s| s.GetTimelineProperties().ok()).and_then(|tl| Some(seek_ticks(tl.StartTime().ok()?.Duration, tl.EndTime().ok()?.Duration, f)));
                let moved = session.as_ref().zip(ticks).is_some_and(|(s, t)| s.TryChangePlaybackPositionAsync(t).and_then(|o| o.join()).unwrap_or(false));
                // Spotify reports the new position late: show it now, or the bar jumps back after the drag
                if let (true, Some(t)) = (moved, ticks) {
                    let mut tr = track.lock().unwrap();
                    tr.position = t as f64 / 1e7;
                    tr.at = Some(Instant::now());
                    drop(tr);
                    if let Some(n) = notify.lock().unwrap().as_ref() {
                        n.changed();
                    }
                }
                continue;
            }
            Msg::Refresh => fresh_art = false,
            Msg::Props | Msg::Recheck => {}
        }
        let now = Instant::now();
        let old = track.lock().unwrap().clone();
        let new = match &session {
            Some(s) => read(s, &old, cache, fresh_art).unwrap_or_default(),
            None => Track::default(),
        };
        if new.active && !same_track(&old, &new) {
            rechecks = recheck_plan(now);
        }
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

/// A session event handler that sends the thread `msg`.
fn send_on<A: windows::core::RuntimeType + 'static>(events: &Sender<Msg>, msg: fn() -> Msg) -> TypedEventHandler<Session, A> {
    let tx = events.clone();
    TypedEventHandler::new(move |_, _| {
        let _ = tx.send(msg());
        Ok(())
    })
}

/// Seconds since 1601, as WinRT's `DateTime` counts.
fn winrt_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64()) + 11_644_473_600.0
}

/// The session as it is now. The cover is read again when `fresh_art`, and otherwise kept
/// from `old` for the same track; a failed read of the same track's cover keeps it too.
fn read(s: &Session, old: &Track, cache: &Path, fresh_art: bool) -> windows::core::Result<Track> {
    let props = s.TryGetMediaPropertiesAsync()?.join()?;
    let info = s.GetPlaybackInfo()?;
    let playing = info.PlaybackStatus()? == Status::Playing;
    let can_seek = info.Controls().and_then(|c| c.IsPlaybackPositionEnabled()).unwrap_or(false);
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
        can_seek,
        art: String::new(),
        position,
        duration,
        at: Some(Instant::now()),
    };
    let same = same_track(old, &t);
    t.art = match (fresh_art, same && !old.art.is_empty()) {
        (false, true) => old.art.clone(),
        (_, keep) => art(&props, cache).unwrap_or_else(|| if keep { old.art.clone() } else { String::new() }),
    };
    Ok(t)
}

/// Saves the session's thumbnail as a PNG in the cache, once per picture, and returns its image id.
fn art(props: &windows::Media::Control::GlobalSystemMediaTransportControlsSessionMediaProperties, cache: &Path) -> Option<String> {
    let stream = props.Thumbnail().ok()?.OpenReadAsync().ok()?.join().ok()?;
    let size = stream.Size().ok()?.min(16 << 20) as u32;
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    let n = reader.LoadAsync(size).ok()?.join().ok()?;
    let mut bytes = vec![0u8; n as usize];
    reader.ReadBytes(&mut bytes).ok()?;
    let path = art_path(cache, &bytes);
    if !path.is_file() {
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

    #[test]
    fn app_names_read_well() {
        assert_eq!(app_name("Spotify.exe"), "Spotify");
        assert_eq!(app_name("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"), "ZuneMusic");
        assert_eq!(app_name("chrome"), "Chrome");
        assert_eq!(app_name("C:\\Program Files\\foobar2000\\foobar2000.exe"), "Foobar2000");
    }

    #[test]
    fn art_has_one_file_per_picture_and_old_ones_go() {
        let dir = std::env::temp_dir().join(format!("wf-media-art-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(art_path(&dir, b"cover one"), art_path(&dir, b"cover one"), "one file per picture");
        assert_ne!(art_path(&dir, b"cover one"), art_path(&dir, b"cover two"), "a new picture is a new image id, whatever the title says");
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
    fn a_new_track_rechecks_its_cover_a_few_times_then_waits_for_events() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut plan = recheck_plan(t0);
        assert_eq!(next_wait(&mut plan, t0), Some(ms(500)));
        assert_eq!(next_wait(&mut plan, t0 + ms(700)), Some(Duration::ZERO), "due");
        plan.remove(0); // the thread's recheck
        assert_eq!(next_wait(&mut plan, t0 + ms(700)), Some(ms(800)));
        let mut late = recheck_plan(t0);
        assert_eq!((next_wait(&mut late, t0 + ms(2000)), late.len()), (Some(Duration::ZERO), 2), "two that fell due while busy are one");
        assert_eq!(next_wait(&mut Vec::new(), t0), None, "idle: no polling");
    }

    #[test]
    fn a_seek_lands_a_fraction_of_the_way_through_the_timeline() {
        assert_eq!(seek_ticks(0, 2_000_000_000, 0.25), 500_000_000, "a quarter of 200 s is 50 s");
        assert_eq!((seek_ticks(10, 110, 0.5), seek_ticks(10, 110, 1.5)), (60, 110), "from the start, never past the end");
    }

    #[test]
    fn controls_reach_the_thread_as_the_messages_they_stand_for() {
        let m = WinMedia::new(std::env::temp_dir());
        // a channel of its own, so the test never touches what is really playing
        let (tx, rx) = mpsc::channel();
        *m.worker.lock().unwrap() = Some(tx);
        for c in [Control::Session, Control::PlayPause, Control::Next, Control::Prev, Control::Seek(0.5)] {
            m.control(c);
        }
        assert!(matches!(rx.try_recv(), Ok(Msg::Session)));
        assert!(matches!(rx.try_recv(), Ok(Msg::Act(v)) if v == "play_pause"));
        assert!(matches!(rx.try_recv(), Ok(Msg::Act(v)) if v == "next"));
        assert!(matches!(rx.try_recv(), Ok(Msg::Act(v)) if v == "prev"));
        assert!(matches!(rx.try_recv(), Ok(Msg::Seek(f)) if f == 0.5));
        assert!(rx.try_recv().is_err());
    }
}
