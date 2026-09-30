//! Everything the engine reads from the machine it runs on, behind traits, so a render can be
//! repeated: the Windows Ambient reads the real machine, the Fixed one returns the same
//! values every time (research/12). Calendar, system probe, media session, audio capture,
//! icon source, fonts and the network.

mod capture;
mod fixed;
mod icon;
mod media;
mod sys;
mod win;
mod win_audio;
mod win_icon;
mod win_media;
mod win_sys;

use std::path::Path;
use std::sync::Arc;

pub use capture::{Capture, to_mono};
pub use fixed::{FixedCalendar, FixedMedia, ScriptedProbe, Silence, TileIcons, Tone};
pub use icon::IconSource;
pub use media::{Control, MediaBackend, Notify, Track};
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

/// The engine's view of time: the wall clock, the moment it shows in UTC, a zone's time and
/// localized day and month names. The clock Data Source reads only this.
pub trait Calendar: Send + Sync {
    /// Local wall time.
    fn now(&self) -> Tm;
    /// Milliseconds since the Unix epoch (`wf.now_ms` for Code Sources).
    fn unix_ms(&self) -> i64;
    /// `tm`'s date as text, in the user's language.
    fn date_text(&self, tm: &Tm, style: DateStyle) -> String;
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
        }
    }

    /// A fixed instant and English names, the demo desktop's readings, a paused track, a
    /// steady tone and a tile per app icon: no machine reads, no files, no threads. The fonts
    /// are still the machine's (`fonts` is the system set until a bundled one exists) and
    /// there is no network.
    pub fn fixed() -> Ambient {
        Ambient {
            calendar: Arc::new(FixedCalendar::default()),
            sys: Arc::new(ScriptedProbe::demo()),
            media: Arc::new(FixedMedia::default()),
            capture: Arc::new(Tone::default()),
            icons: Arc::new(TileIcons),
            fonts: FontSet::system,
            fetch: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
