//! Everything the engine reads from the machine it runs on, behind traits, so a render can be
//! repeated: the Windows Ambient reads the real machine, the Fixed one returns the same
//! values every time (research/12). Calendar, system probe, media session, audio capture,
//! icon source, fonts and the network.

mod capture;
mod fixed;
mod icon;
mod media;
mod pins;
mod sys;
mod win;
mod win_audio;
mod win_icon;
mod win_media;
mod win_sys;

use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use capture::{Capture, to_mono};
pub(crate) use win::localized_date;
pub use fixed::{CannedFetch, FixedCalendar, FixedMedia, OfflineFetch, PREROLL, ScriptedProbe, Silence, TileIcons, Tone};
pub use icon::IconSource;
pub use media::{Control, MediaBackend, Notify, Track};
pub use pins::{AudioPins, Backdrop, Canned, FetchMode, FontMode, IconMode, KEYS, MediaPins, Pins, Seam, SysPins};
pub use sys::{Battery, GpuLoad, Memory, Reading, SysProbe};
pub use win::WinCalendar;
pub use win_audio::Wasapi;
pub use win_icon::ShellIcons;
pub use win_media::WinMedia;
pub use win_sys::WinProbe;

use crate::net::Fetch;
use crate::text::FontSet;

/// A calendar time: local wall time, or UTC where a method says so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tm {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    /// 0 = Sunday.
    pub dow: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub ms: u32,
}

impl Tm {
    pub fn new(year: i32, month: u32, day: u32, dow: u32, hour: u32, minute: u32, second: u32, ms: u32) -> Self {
        Self { year, month, day, dow, hour, minute, second, ms }
    }

    /// Minutes since 1970-01-01 00:00, reading `self` as if it were UTC.
    pub(crate) fn epoch_minutes(&self) -> i64 {
        days_from_civil(self.year, self.month, self.day) * 1440 + self.hour as i64 * 60 + self.minute as i64
    }

    /// Milliseconds since the Unix epoch, reading `self` as UTC.
    pub fn unix_ms(&self) -> i64 {
        self.epoch_minutes() * 60_000 + self.second as i64 * 1000 + self.ms as i64
    }

    /// The UTC time `ms` milliseconds after the Unix epoch.
    pub fn from_unix_ms(ms: i64) -> Tm {
        let (secs, ms) = (ms.div_euclid(1000), ms.rem_euclid(1000));
        let (days, sod) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
        let (year, month, day) = civil_from_days(days);
        Tm { year, month, day, dow: (days + 4).rem_euclid(7) as u32, hour: (sod / 3600) as u32, minute: (sod % 3600 / 60) as u32, second: (sod % 60) as u32, ms: ms as u32 }
    }
}

/// Days since 1970-01-01 (Howard Hinnant's `days_from_civil`).
pub(crate) fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

/// The inverse of `days_from_civil`: (year, month, day) of a day count.
pub(crate) fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((yoe + era * 400 + i64::from(m <= 2)) as i32, m, d)
}

/// The date texts a clock shows, named by what they say, not by a locale pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateStyle {
    /// "Monday"
    Weekday,
    /// "Mon"
    WeekdayShort,
    /// "September"
    Month,
    /// "Monday, 21 September"
    Date,
    /// "21 Sep"
    DateShort,
}

/// How the user's weeks are written: the first day and the shortest day names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Week {
    /// Monday is 0, Sunday 6, as Windows numbers `LOCALE_IFIRSTDAYOFWEEK`.
    pub first: u32,
    /// The shortest day names, Monday first.
    pub names: [String; 7],
}

/// The engine's view of time: the wall clock, the moment it shows in UTC, a zone's time and
/// localized day and month names. The clock Data Source reads only this.
pub trait Calendar: Send + Sync {
    /// Local wall time.
    fn now(&self) -> Tm;
    /// Milliseconds since the Unix epoch (`wf.now_ms` for Code Sources).
    fn unix_ms(&self) -> i64;
    /// `tm`'s date as text, in the user's language.
    fn date_text(&self, tm: &Tm, style: DateStyle) -> String;
    /// The first day of the week and the shortest day names (the calendar widget's columns).
    fn week(&self) -> Week;
    /// The UTC moment the local time `local` shows, `None` if the zone cannot say.
    fn local_to_utc(&self, local: &Tm) -> Option<Tm>;
    /// Every time zone key this calendar knows.
    fn zone_keys(&self) -> Vec<String>;
    /// The local time in the zone `key` (one of `zone_keys`) at the UTC moment `utc`.
    fn to_zone(&self, key: &str, utc: &Tm) -> Option<Tm>;
}

/// The bundle of machine facts the engine reads. `windows` reads the real machine, `fixed`
/// answers the same every time.
#[derive(Clone)]
pub struct Ambient {
    pub calendar: Arc<dyn Calendar>,
    pub sys: Arc<dyn SysProbe>,
    pub media: Arc<dyn MediaBackend>,
    pub capture: Arc<dyn Capture>,
    pub icons: Arc<dyn IconSource>,
    /// Builds the fonts a `TextEngine` shapes with: the system's for now, a fixed bundled
    /// set once there is one. Called where an engine is made, so a `fixed` Ambient reads
    /// no fonts until then.
    pub fonts: fn() -> FontSet,
    /// The transport for plugin code's network, `None` until something needs it: the app
    /// opens it when a Plugin lists hosts.
    pub fetch: Option<Arc<dyn Fetch>>,
    /// Where the coding agents' session files are read (the `agents` source): `None` is the
    /// user's profile. A hermetic render points it at its own empty data folder, so no
    /// session of the machine shows.
    pub home: Option<PathBuf>,
}

impl Ambient {
    /// The real machine; album art is cached under `<data>/.cache/media` (a dot-folder never
    /// reloads content). Nothing is read and no thread starts until a source is asked.
    pub fn windows(data: &Path) -> Ambient {
        Ambient {
            calendar: Arc::new(WinCalendar),
            sys: Arc::new(WinProbe::default()),
            media: Arc::new(WinMedia::new(data.join(".cache").join("media"))),
            capture: Arc::new(Wasapi::default()),
            icons: Arc::new(ShellIcons),
            fonts: FontSet::system,
            fetch: None,
            home: None,
        }
    }

    /// `fixed_with` the default Pins: the always-on environment of a hermetic render, and
    /// what tests use for a machine that never changes. No machine reads, no files, no threads.
    pub fn fixed() -> Ambient {
        Self::fixed_with(&Pins::default())
    }

    /// The machine-facing Pins as an Ambient: the pinned instant in the pinned zone with
    /// English names, the pinned desktop (rolled through 60 samples so its graphs are full),
    /// the pinned track, a tone or silence, a tile per app icon, the machine's fonts and a
    /// network that is offline or answers the canned URLs. The rest of the Pins (`settle`,
    /// `scale`, `palette`, `anim`, `transparent`, `backdrop`, `real`, `icons = system`,
    /// `fetch = real`) steer the render path, which supplies the real seams itself.
    ///
    /// Panics on a zone or locale `Pins::set` would have refused.
    pub fn fixed_with(pins: &Pins) -> Ambient {
        Self::fixed_at(pins, std::time::Instant::now())
    }

    /// `fixed_with`, with a playing track started at `origin` on the caller's (virtual)
    /// monotonic clock instead of at this call, so a render that reads sources at
    /// `origin + 2 s` finds it 2 s further on, however long the machine took to get there.
    pub fn fixed_at(pins: &Pins, origin: std::time::Instant) -> Ambient {
        let m = &pins.media;
        let art = if m.art.is_empty() || m.art.starts_with("file:") { m.art.clone() } else { format!("file:{}", m.art) };
        let track = Track { active: m.active, title: m.title.clone(), artist: m.artist.clone(), album: m.album.clone(), source: m.source.clone(), playing: m.playing, can_seek: m.can_seek, art, position: m.position, duration: m.duration, at: m.playing.then_some(origin) };
        let capture: Arc<dyn Capture> = if pins.audio.silence { Arc::new(Silence) } else { Arc::new(Tone::new(pins.audio.tones.iter().copied(), pins.audio.amp)) };
        let fetch: Option<Arc<dyn Fetch>> = match &pins.fetch {
            FetchMode::Offline => Some(Arc::new(OfflineFetch)),
            FetchMode::Canned(answers) => Some(Arc::new(CannedFetch(answers.clone()))),
            FetchMode::Real => None,
        };
        assert!(pins::LOCALES.contains(&pins.locale.as_str()), "the fixed calendar has no names for `{}`", pins.locale);
        Ambient {
            calendar: Arc::new(FixedCalendar::in_zone(pins.now, &pins.zone)),
            sys: Arc::new(ScriptedProbe::rolled(&pins.sys)),
            media: Arc::new(FixedMedia::new(track)),
            capture,
            icons: Arc::new(TileIcons),
            fonts: match pins.fonts {
                FontMode::System => FontSet::system_shared,
            },
            fetch,
            home: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{Request, Target};

    #[test]
    fn civil_days_round_trip_and_unix_ms_read_utc() {
        for (y, m, d) in [(1970, 1, 1), (2000, 2, 29), (2026, 3, 8), (2026, 12, 31), (1969, 12, 31), (2100, 3, 1)] {
            assert_eq!(civil_from_days(days_from_civil(y, m, d)), (y, m, d));
        }
        let t = Tm::from_unix_ms(1_774_224_000_123); // 2026-03-23 00:00:00.123 UTC, a Monday
        assert_eq!(t, Tm::new(2026, 3, 23, 1, 0, 0, 0, 123));
        assert_eq!(t.unix_ms(), 1_774_224_000_123);
        assert_eq!(Tm::from_unix_ms(-1), Tm::new(1969, 12, 31, 3, 23, 59, 59, 999));
    }
    fn pinned(f: impl FnOnce(&mut Pins)) -> Ambient {
        let mut p = Pins::default();
        f(&mut p);
        Ambient::fixed_with(&p)
    }

    #[test]
    fn fixed_is_the_default_pins() {
        let (a, b) = (Ambient::fixed(), Ambient::fixed_with(&Pins::default()));
        assert_eq!((a.calendar.now(), a.calendar.unix_ms()), (b.calendar.now(), 1_768_471_830_000));
        assert_eq!(a.calendar.now(), Tm::new(2026, 1, 15, 4, 10, 10, 30, 0));
        assert_eq!(a.media.track().title, "Example Song");
        assert_eq!(a.sys.read(), b.sys.read());
        assert!(a.fetch.is_some(), "offline, not absent");
    }

    #[test]
    fn the_calendar_follows_now_and_zone() {
        let a = pinned(|p| {
            p.set("now", &serde_json::json!("2026-03-08T15:42:10.5")).unwrap();
            p.set("zone", &serde_json::json!("Eastern Standard Time")).unwrap();
        });
        assert_eq!(a.calendar.now(), Tm::new(2026, 3, 8, 0, 15, 42, 10, 500));
        assert_eq!(a.calendar.unix_ms(), Tm::new(2026, 3, 8, 0, 19, 42, 10, 500).unix_ms(), "15:42 EDT is 19:42 UTC");
        assert_eq!(a.calendar.date_text(&a.calendar.now(), DateStyle::Date), "Sunday, 8 March");
    }

    #[test]
    fn the_desktop_ends_on_the_pinned_readings_after_sixty_samples() {
        let a = pinned(|p| {
            p.set("sys.cpu", &serde_json::json!(42)).unwrap();
            p.set("sys.ram", &serde_json::json!(71)).unwrap();
            p.set("sys.battery", &serde_json::json!(64)).unwrap();
            p.set("sys.charging", &serde_json::json!(true)).unwrap();
            p.set("sys.gpus", &serde_json::json!([30, 60])).unwrap();
        });
        assert_eq!(a.sys.steady(), Some(PREROLL));
        let last = (0..PREROLL).map(|_| a.sys.read()).last().unwrap();
        assert_eq!((last.mem.load_pct, last.battery, last.gpus.iter().map(|g| (g.key.as_str(), g.load)).collect::<Vec<_>>()), (71, Some(Battery { percent: 64, charging: true }), vec![("gpu", 30.0), ("gpu2", 60.0)]));
        let none = pinned(|p| p.set("sys.gpus", &serde_json::json!([])).unwrap());
        assert!(none.sys.read().gpus.is_empty() && none.sys.read().battery.is_none());
    }

    #[test]
    fn the_track_carries_the_media_pins() {
        let a = pinned(|p| {
            for (k, v) in [("media.title", "T"), ("media.artist", "A"), ("media.album", "L"), ("media.source", "S")] {
                p.set(k, &serde_json::json!(v)).unwrap();
            }
            p.set("media.playing", &serde_json::json!(true)).unwrap();
            p.set("media.can_seek", &serde_json::json!(false)).unwrap();
            p.set("media.duration", &serde_json::json!(300)).unwrap();
            p.set("media.art", &serde_json::json!("cover.png")).unwrap();
        });
        let t = a.media.track();
        assert_eq!((t.title.as_str(), t.artist.as_str(), t.album.as_str(), t.source.as_str(), t.playing, t.can_seek, t.duration, t.art.as_str()), ("T", "A", "L", "S", true, false, 300.0, "file:cover.png"));
        assert!(t.at.is_some(), "a playing track runs on");
        assert!(Ambient::fixed().media.track().at.is_none() && !pinned(|p| p.set("media.active", &serde_json::json!(false)).unwrap()).media.track().active);
    }

    #[test]
    fn a_playing_track_runs_from_the_origin_it_is_given() {
        let origin = std::time::Instant::now() + std::time::Duration::from_secs(1000);
        let mut p = Pins::default();
        p.set("media.playing", &serde_json::json!(true)).unwrap();
        let track = Ambient::fixed_at(&p, origin).media.track();
        assert_eq!((track.position_at(origin), track.position_at(origin + std::time::Duration::from_secs(2))), (83.0, 85.0));
        p.set("media.playing", &serde_json::json!(false)).unwrap();
        assert_eq!(Ambient::fixed_at(&p, origin).media.track().position_at(origin + std::time::Duration::from_secs(2)), 83.0, "paused stays put");
    }

    #[test]
    fn the_capture_is_a_tone_or_silence() {
        let hear = |a: &Ambient| {
            let mut out = vec![];
            a.capture.read(&mut out).unwrap();
            out
        };
        let tone = hear(&Ambient::fixed());
        assert!(tone.iter().any(|s| s.abs() > 0.1));
        let one = hear(&pinned(|p| {
            p.set("audio.tones", &serde_json::json!(480)).unwrap();
            p.set("audio.amp", &serde_json::json!(0.5)).unwrap();
        }));
        assert!(one.iter().all(|s| s.abs() <= 0.5) && one.iter().any(|s| s.abs() > 0.49));
        assert!(hear(&pinned(|p| p.set("audio", &serde_json::json!("silence")).unwrap())).iter().all(|s| *s == 0.0));
    }

    #[test]
    fn icons_are_tiles_and_fonts_are_the_systems() {
        let a = pinned(|p| p.set("fonts", &serde_json::json!("system")).unwrap());
        assert!(a.icons.shell_icon(std::path::Path::new("x.exe")).is_some());
        assert_eq!(a.fonts as *const () as usize, FontSet::system_shared as *const () as usize);
    }

    #[test]
    fn the_network_is_offline_canned_or_left_to_the_render_path() {
        let ask = |a: &Ambient, url: &str| a.fetch.as_ref().expect("a transport").fetch(&Target { host: "api.example".into(), path: "/".into() }, &Request { method: "GET".into(), url: url.into(), ..Default::default() });
        assert_eq!(ask(&Ambient::fixed(), "https://api.example/").unwrap_err(), "offline (render environment)");
        let canned = pinned(|p| p.set("fetch.responses", &serde_json::json!([{ "url": "https://api.example/", "body": { "temp": -3 } }])).unwrap());
        let r = ask(&canned, "https://api.example/").unwrap();
        assert_eq!((r.status, r.content_type.as_str(), r.body.as_str()), (200, "application/json", r#"{"temp":-3}"#));
        assert!(ask(&canned, "https://other.example/").unwrap_err().contains("no canned answer"));
        assert!(pinned(|p| p.set("fetch", &serde_json::json!("real")).unwrap()).fetch.is_none());
    }
}
