//! A calendar that never reads the machine: a fixed instant, English names and an embedded
//! table of the zones the clock's city list names, with the US, EU, Australian and New
//! Zealand daylight rules as they stand since 2008. Not valid for earlier years. Beside it,
//! the system probe, media session, capture and icon source that answer the same every time.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::capture::Capture;
use super::icon::IconSource;
use super::media::{Control, MediaBackend, Notify, Track};
use super::pins::{Canned, SysPins};
use super::sys::{Battery, GpuLoad, Memory, Reading, SysProbe};
use super::{Calendar, DateStyle, Tm, Week, civil_from_days, days_from_civil};
use crate::images::Decoded;
use crate::net::{Fetch, Request, Response, Target};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Dst {
    None,
    /// Second Sunday of March to first Sunday of November, both at 02:00 local.
    Us,
    /// Last Sunday of March to last Sunday of October, both at 01:00 UTC.
    Eu,
    /// First Sunday of October (02:00 standard) to first Sunday of April (03:00 daylight).
    Au,
    /// Last Sunday of September (02:00 standard) to first Sunday of April (03:00 daylight).
    Nz,
}

/// Windows zone keys with their standard offset from UTC in minutes and their daylight rule.
/// Zones whose rules are none of these (Egypt, Israel) are left out and read as unknown.
pub(super) const ZONE_TABLE: &[(&str, i32, Dst)] = &[
    ("UTC", 0, Dst::None),
    ("Eastern Standard Time", -300, Dst::Us),
    ("Central Standard Time", -360, Dst::Us),
    ("Mountain Standard Time", -420, Dst::Us),
    ("US Mountain Standard Time", -420, Dst::None),
    ("Pacific Standard Time", -480, Dst::Us),
    ("Alaskan Standard Time", -540, Dst::Us),
    ("Hawaiian Standard Time", -600, Dst::None),
    ("Central Standard Time (Mexico)", -360, Dst::None),
    ("E. South America Standard Time", -180, Dst::None),
    ("Argentina Standard Time", -180, Dst::None),
    ("GMT Standard Time", 0, Dst::Eu),
    ("Greenwich Standard Time", 0, Dst::None),
    ("Romance Standard Time", 60, Dst::Eu),
    ("W. Europe Standard Time", 60, Dst::Eu),
    ("Central European Standard Time", 60, Dst::Eu),
    ("Central Europe Standard Time", 60, Dst::Eu),
    ("GTB Standard Time", 120, Dst::Eu),
    ("FLE Standard Time", 120, Dst::Eu),
    ("Turkey Standard Time", 180, Dst::None),
    ("Russian Standard Time", 180, Dst::None),
    ("South Africa Standard Time", 120, Dst::None),
    ("W. Central Africa Standard Time", 60, Dst::None),
    ("E. Africa Standard Time", 180, Dst::None),
    ("Arabian Standard Time", 240, Dst::None),
    ("Iran Standard Time", 210, Dst::None),
    ("Pakistan Standard Time", 300, Dst::None),
    ("India Standard Time", 330, Dst::None),
    ("Bangladesh Standard Time", 360, Dst::None),
    ("SE Asia Standard Time", 420, Dst::None),
    ("Singapore Standard Time", 480, Dst::None),
    ("China Standard Time", 480, Dst::None),
    ("Taipei Standard Time", 480, Dst::None),
    ("W. Australia Standard Time", 480, Dst::None),
    ("Korea Standard Time", 540, Dst::None),
    ("Tokyo Standard Time", 540, Dst::None),
    ("Cen. Australia Standard Time", 570, Dst::Au),
    ("AUS Eastern Standard Time", 600, Dst::Au),
    ("E. Australia Standard Time", 600, Dst::None),
    ("New Zealand Standard Time", 720, Dst::Nz),
];

fn zone(key: &str) -> Option<(i32, Dst)> {
    ZONE_TABLE.iter().find(|(k, ..)| *k == key).map(|&(_, std, dst)| (std, dst))
}

/// Day count of the `n`th Sunday of a month (`n` from 1), or of the last one (`n` = -1).
fn sunday(year: i32, month: u32, n: i32) -> i64 {
    let dow = |days: i64| (days + 4).rem_euclid(7); // 1970-01-01 was a Thursday
    if n > 0 {
        let first = days_from_civil(year, month, 1);
        first + (7 - dow(first)) % 7 + 7 * (n as i64 - 1)
    } else {
        let (y, m) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
        let last = days_from_civil(y, m, 1) - 1;
        last - dow(last)
    }
}

/// Whether daylight time is on at the UTC minute `utc` (minutes since the epoch) in a zone
/// whose standard offset is `std` minutes.
fn daylight(dst: Dst, std: i32, utc: i64) -> bool {
    let year = civil_from_days(utc.div_euclid(1440)).0;
    // the UTC minute at which a local clock reads `mins` minutes into `day`, at offset `off`
    let at = |day: i64, mins: i64, off: i32| day * 1440 + mins - off as i64;
    let south_end = at(sunday(year, 4, 1), 180, std + 60);
    match dst {
        Dst::None => false,
        Dst::Us => utc >= at(sunday(year, 3, 2), 120, std) && utc < at(sunday(year, 11, 1), 120, std + 60),
        Dst::Eu => utc >= sunday(year, 3, -1) * 1440 + 60 && utc < sunday(year, 10, -1) * 1440 + 60,
        Dst::Au => utc >= at(sunday(year, 10, 1), 120, std) || utc < south_end,
        Dst::Nz => utc >= at(sunday(year, 9, -1), 120, std) || utc < south_end,
    }
}

/// The time `epoch_minutes` after 1970-01-01 00:00, keeping `second` and `ms`.
fn from_minutes(epoch_minutes: i64, second: u32, ms: u32) -> Tm {
    let days = epoch_minutes.div_euclid(1440);
    let of_day = epoch_minutes.rem_euclid(1440);
    let (year, month, day) = civil_from_days(days);
    Tm::new(year, month, day, (days + 4).rem_euclid(7) as u32, (of_day / 60) as u32, (of_day % 60) as u32, second, ms)
}

pub struct FixedCalendar {
    now: Tm,
    /// The zone `now` is local to.
    local: &'static str,
}

impl FixedCalendar {
    /// `now` as UTC wall time.
    pub fn new(now: Tm) -> Self {
        Self { now, local: "UTC" }
    }

    /// `now` local to the zone `key` (a key of the embedded table), for tests of local time.
    pub fn in_zone(now: Tm, key: &str) -> Self {
        let local = ZONE_TABLE.iter().find(|(k, ..)| *k == key).unwrap_or_else(|| panic!("no zone `{key}` in the fixed table")).0;
        Self { now, local }
    }
}

impl Default for FixedCalendar {
    /// Monday 21 September 2026, 12:00 UTC.
    fn default() -> Self {
        Self::new(Tm::new(2026, 9, 21, 1, 12, 0, 0, 0))
    }
}

const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

impl Calendar for FixedCalendar {
    fn now(&self) -> Tm {
        self.now
    }

    fn unix_ms(&self) -> i64 {
        self.local_to_utc(&self.now).map_or(0, |t| t.unix_ms())
    }

    fn date_text(&self, tm: &Tm, style: DateStyle) -> String {
        let (day, month) = (DAYS[tm.dow as usize % 7], MONTHS[(tm.month as usize).clamp(1, 12) - 1]);
        match style {
            DateStyle::Weekday => day.into(),
            DateStyle::WeekdayShort => day[..3].into(),
            DateStyle::Month => month.into(),
            DateStyle::Date => format!("{day}, {} {month}", tm.day),
            DateStyle::DateShort => format!("{} {}", tm.day, &month[..3]),
        }
    }

    /// en-US: weeks start on Sunday.
    fn week(&self) -> Week {
        Week { first: 6, names: ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"].map(String::from) }
    }

    /// In the hour a fall-back repeats, the first one (daylight); in the hour a spring-forward
    /// skips, the standard-time reading.
    fn local_to_utc(&self, local: &Tm) -> Option<Tm> {
        let (std, dst) = zone(self.local)?;
        let wall = local.epoch_minutes();
        let as_daylight = wall - (std + 60) as i64;
        let utc = if daylight(dst, std, as_daylight) { as_daylight } else { wall - std as i64 };
        Some(from_minutes(utc, local.second, local.ms))
    }

    fn zone_keys(&self) -> Vec<String> {
        ZONE_TABLE.iter().map(|(k, ..)| k.to_string()).collect()
    }

    fn to_zone(&self, key: &str, utc: &Tm) -> Option<Tm> {
        let (std, dst) = zone(key)?;
        let at = utc.epoch_minutes();
        let off = std + if daylight(dst, std, at) { 60 } else { 0 };
        Some(from_minutes(at + off as i64, utc.second, utc.ms))
    }
}

/// A probe that plays back readings: each `read` gives the next one and the last repeats,
/// with its CPU counters moved on by `advance` after every repeat, so a steady load shows
/// as a delta between samples.
pub struct ScriptedProbe {
    script: Mutex<VecDeque<Reading>>,
    advance: (u64, u64),
    steady: Option<usize>,
}

impl ScriptedProbe {
    /// The readings in order, then the last one for ever. Panics on an empty script.
    pub fn new(script: impl IntoIterator<Item = Reading>) -> Self {
        let script: VecDeque<Reading> = script.into_iter().collect();
        assert!(!script.is_empty(), "a scripted probe needs a reading");
        Self { script: Mutex::new(script), advance: (0, 0), steady: None }
    }

    /// Adds `(idle, total)` CPU ticks to the repeated reading on every read.
    pub fn advancing(mut self, idle: u64, total: u64) -> Self {
        self.advance = (idle, total);
        self
    }

    /// `Reading::demo()` with the CPU at 37 % from the second sample on.
    pub fn demo() -> Self {
        Self::new([Reading::demo()]).advancing(6_300_000, 10_000_000)
    }
}

/// Readings the pinned desktop is rolled through before the first ask: the 60 samples the
/// graphs keep, and the one before them that the first delta is taken from.
pub const PREROLL: usize = 61;

/// `k` of `n` steps along a triangle wave of period 16 that is 0 at the last step, in -4 to 4.
fn wave(k: usize, n: usize) -> i32 {
    match (n - 1 - k) % 16 {
        p @ 0..=4 => p as i32,
        p @ 5..=12 => 8 - p as i32,
        p => p as i32 - 16,
    }
}

impl ScriptedProbe {
    /// A desktop that ends on the values in `pins` after `PREROLL` readings, each a second
    /// on from the last, with the CPU, memory, network and GPU loads swinging around them
    /// on the way so the graphs have a shape. Whoever reads it `steady` gets the end state.
    pub fn rolled(pins: &SysPins) -> Self {
        const GB: u64 = 1 << 30;
        const SECOND: u64 = 10_000_000; // in 100 ns ticks
        let pct = |v: u8, d: i32, step: i32| (i32::from(v) + d * step).clamp(0, 100) as u64;
        let (down, up) = ((pins.net_down * 1_048_576.0).round(), (pins.net_up * 1_048_576.0).round());
        let (mut idle, mut total, mut rx, mut tx) = (0u64, SECOND, 1_000_000_000u64, 200_000_000u64);
        let script = (0..PREROLL).map(|k| {
            let d = wave(k, PREROLL);
            if k > 0 {
                let cpu = pct(pins.cpu, d, 3);
                total += SECOND;
                idle += SECOND / 100 * (100 - cpu);
                rx += (down * (1.0 + f64::from(d) * 0.1)).round() as u64;
                tx += (up * (1.0 + f64::from(d) * 0.1)).round() as u64;
            }
            let ram = pct(pins.ram, d, 2);
            let phys = 16 * GB;
            let gpus = pins.gpus.iter().enumerate().map(|(i, g)| GpuLoad {
                label: if pins.gpus.len() > 1 { format!("GPU {}", i + 1) } else { "GPU".into() },
                id: "gpu",
                key: if i == 0 { "gpu".into() } else { format!("gpu{}", i + 1) },
                name: "8 GB VRAM".into(),
                load: pct(*g, d, 2) as f64,
            });
            Reading {
                cpu: (idle, total),
                mem: Memory { load_pct: ram as u32, total_phys: phys, avail_phys: phys - (u128::from(phys) * u128::from(ram) / 100) as u64, total_page: 32 * GB, avail_page: 18 * GB },
                drives: vec![("C:".into(), (u128::from(931 * GB) * u128::from(pins.disk) / 100) as u64, 931 * GB), ("D:".into(), 1100 * GB, 1800 * GB)],
                net: Some((rx, tx)),
                processes: pins.processes,
                battery: pins.battery.map(|percent| Battery { percent, charging: pins.charging }),
                uptime_ms: pins.uptime * 1000,
                gpus: gpus.collect(),
            }
        });
        let mut probe = Self::new(script);
        probe.steady = Some(PREROLL);
        probe
    }
}

impl SysProbe for ScriptedProbe {
    fn steady(&self) -> Option<usize> {
        self.steady
    }

    fn read(&self) -> Reading {
        let mut s = self.script.lock().unwrap_or_else(|e| e.into_inner());
        if s.len() > 1 {
            return s.pop_front().unwrap();
        }
        let out = s[0].clone();
        s[0].cpu = (out.cpu.0 + self.advance.0, out.cpu.1 + self.advance.1);
        out
    }
}

/// A transport with no network: every request fails, as a render with nothing to reach would.
pub struct OfflineFetch;

impl Fetch for OfflineFetch {
    fn fetch(&self, _to: &Target, _req: &Request) -> Result<Response, String> {
        Err("offline (render environment)".into())
    }
}

/// A transport that answers exactly the URLs it was given, and is offline for any other.
pub struct CannedFetch(pub Vec<Canned>);

impl Fetch for CannedFetch {
    fn fetch(&self, _to: &Target, req: &Request) -> Result<Response, String> {
        match self.0.iter().find(|c| c.url == req.url) {
            Some(c) => Ok(Response { status: c.status, content_type: c.content_type.clone(), location: None, body: c.body.clone() }),
            None => Err(format!("offline (render environment): no canned answer for {}", req.url)),
        }
    }
}

/// A media session that never changes: by default a paused, seekable "Song" by "Artist". It
/// asks nothing of the machine and records every `Control` it is sent.
pub struct FixedMedia {
    track: Track,
    log: Mutex<Vec<Control>>,
}

impl FixedMedia {
    pub fn new(track: Track) -> Self {
        Self { track, log: Mutex::new(Vec::new()) }
    }

    /// Every control sent so far, oldest first.
    pub fn controls(&self) -> Vec<Control> {
        self.log.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Default for FixedMedia {
    fn default() -> Self {
        Self::new(Track { active: true, title: "Song".into(), artist: "Artist".into(), album: "Album".into(), source: "Player".into(), can_seek: true, position: 60.0, duration: 200.0, ..Default::default() })
    }
}

impl MediaBackend for FixedMedia {
    fn track(&self) -> Track {
        self.track.clone()
    }

    fn control(&self, control: Control) {
        self.log.lock().unwrap_or_else(|e| e.into_inner()).push(control);
    }

    fn attach(&self, _notify: Arc<dyn Notify>) {}
}

/// A capture that plays a sum of steady sine tones, `amp` each, without a device: by default
/// 110, 440 and 1760 Hz at 48 kHz, a spectrum that never changes. It is read on demand, and
/// each read carries the tones on from where the last one stopped.
pub struct Tone {
    hz: Vec<f32>,
    amp: f32,
    played: Mutex<u64>,
}

impl Tone {
    pub const RATE: u32 = 48_000;

    pub fn new(hz: impl IntoIterator<Item = f32>, amp: f32) -> Self {
        Self { hz: hz.into_iter().collect(), amp, played: Mutex::new(0) }
    }
}

impl Default for Tone {
    fn default() -> Self {
        Self::new([110.0, 440.0, 1760.0], 0.3)
    }
}

impl Capture for Tone {
    fn open(&self) -> Result<u32, String> {
        Ok(Self::RATE)
    }

    /// The next 10 ms.
    fn read(&self, out: &mut Vec<f32>) -> Result<(), String> {
        let mut played = self.played.lock().unwrap_or_else(|e| e.into_inner());
        let rate = Self::RATE as f64;
        out.extend((*played..*played + u64::from(Self::RATE / 100)).map(|n| self.hz.iter().map(|hz| self.amp * (std::f64::consts::TAU * f64::from(*hz) * n as f64 / rate).sin() as f32).sum::<f32>()));
        *played += u64::from(Self::RATE / 100);
        Ok(())
    }

    fn realtime(&self) -> bool {
        false
    }
}

/// A capture that hears nothing, without a device.
pub struct Silence;

impl Capture for Silence {
    fn open(&self) -> Result<u32, String> {
        Ok(Tone::RATE)
    }

    fn read(&self, out: &mut Vec<f32>) -> Result<(), String> {
        out.extend(std::iter::repeat_n(0.0, (Tone::RATE / 100) as usize));
        Ok(())
    }

    fn realtime(&self) -> bool {
        false
    }
}

/// An icon source with no files and no shell: every file stem gets a 48 px rounded tile of
/// its own colour (so two apps look different), and a target resolves to itself.
pub struct TileIcons;

/// FNV-1a, so a stem's colour is the same on every machine and build.
fn stem_hash(stem: &str) -> u32 {
    stem.to_lowercase().bytes().fold(0x811c_9dc5, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

/// The tile for `stem`: a rounded square, opaque inside with an antialiased edge.
pub(super) fn tile(stem: &str) -> Decoded {
    let h = stem_hash(stem);
    let rgb = [(h & 0xff) as u8, (h >> 8 & 0xff) as u8, (h >> 16 & 0xff) as u8].map(|c| 64 + c / 2);
    let n = 48u32;
    let mut px = vec![0u8; (n * n * 4) as usize];
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 + 0.5 - 24.0, y as f32 + 0.5 - 24.0);
            let q = (fx.abs() - 19.0 + 8.0, fy.abs() - 19.0 + 8.0);
            let d = (q.0.max(0.0).powi(2) + q.1.max(0.0).powi(2)).sqrt() + q.0.max(q.1).min(0.0) - 8.0;
            let i = ((y * n + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], ((0.5 - d).clamp(0.0, 1.0) * 255.0) as u8]);
        }
    }
    Decoded { px, w: n, h: n, frames: None }
}

impl IconSource for TileIcons {
    fn shell_icon(&self, path: &Path) -> Option<Decoded> {
        Some(tile(&crate::shortcut::file_stem(&path.to_string_lossy())))
    }

    fn resolve_path(&self, target: &str) -> Option<PathBuf> {
        (!target.is_empty() && !target.contains("://")).then(|| PathBuf::from(target))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (month, day, hour, minute) in the zone `key` at the UTC moment 2026-`mo`-`d` `h`:`mi`.
    fn at(key: &str, mo: u32, d: u32, h: u32, mi: u32) -> (u32, u32, u32, u32) {
        let t = FixedCalendar::default().to_zone(key, &Tm::new(2026, mo, d, 0, h, mi, 0, 0)).expect(key);
        (t.month, t.day, t.hour, t.minute)
    }

    #[test]
    fn the_us_springs_forward_on_the_second_sunday_of_march_and_falls_back_on_the_first_of_november() {
        assert_eq!(at("Eastern Standard Time", 3, 8, 6, 59), (3, 8, 1, 59));
        assert_eq!(at("Eastern Standard Time", 3, 8, 7, 0), (3, 8, 3, 0), "02:00 EST becomes 03:00 EDT");
        assert_eq!(at("Eastern Standard Time", 11, 1, 5, 59), (11, 1, 1, 59));
        assert_eq!(at("Eastern Standard Time", 11, 1, 6, 0), (11, 1, 1, 0), "02:00 EDT becomes 01:00 EST");
        assert_eq!(at("Pacific Standard Time", 3, 8, 9, 59), (3, 8, 1, 59));
        assert_eq!(at("Pacific Standard Time", 3, 8, 10, 0), (3, 8, 3, 0));
        assert_eq!(at("US Mountain Standard Time", 7, 1, 12, 0), (7, 1, 5, 0), "Arizona keeps standard time");
    }

    #[test]
    fn europe_changes_at_one_utc_on_the_last_sundays_of_march_and_october() {
        assert_eq!(at("GMT Standard Time", 3, 29, 0, 59), (3, 29, 0, 59));
        assert_eq!(at("GMT Standard Time", 3, 29, 1, 0), (3, 29, 2, 0));
        assert_eq!(at("GMT Standard Time", 10, 25, 0, 59), (10, 25, 1, 59));
        assert_eq!(at("GMT Standard Time", 10, 25, 1, 0), (10, 25, 1, 0));
        assert_eq!(at("Romance Standard Time", 3, 29, 0, 59), (3, 29, 1, 59));
        assert_eq!(at("Romance Standard Time", 3, 29, 1, 0), (3, 29, 3, 0), "Paris jumps at the same instant as London");
        assert_eq!(at("Romance Standard Time", 10, 25, 0, 59), (10, 25, 2, 59));
        assert_eq!(at("Romance Standard Time", 10, 25, 1, 0), (10, 25, 2, 0));
        assert_eq!(at("Central European Standard Time", 1, 15, 12, 0), (1, 15, 13, 0));
    }

    #[test]
    fn the_south_is_on_daylight_time_over_new_year() {
        // Sydney: on at 02:00 standard on the first Sunday of October, off at 03:00 daylight on the first of April
        assert_eq!(at("AUS Eastern Standard Time", 10, 3, 15, 59), (10, 4, 1, 59));
        assert_eq!(at("AUS Eastern Standard Time", 10, 3, 16, 0), (10, 4, 3, 0));
        assert_eq!(at("AUS Eastern Standard Time", 4, 4, 15, 59), (4, 5, 2, 59));
        assert_eq!(at("AUS Eastern Standard Time", 4, 4, 16, 0), (4, 5, 2, 0));
        assert_eq!(at("AUS Eastern Standard Time", 12, 31, 13, 0), (1, 1, 0, 0), "Sydney enters the new year on daylight time");
        assert_eq!(at("AUS Eastern Standard Time", 6, 30, 14, 0), (7, 1, 0, 0), "and is an hour closer in winter");
        // Adelaide is half an hour off the hour, Brisbane never changes, Auckland switches in September
        assert_eq!(at("Cen. Australia Standard Time", 10, 3, 16, 29), (10, 4, 1, 59));
        assert_eq!(at("Cen. Australia Standard Time", 10, 3, 16, 30), (10, 4, 3, 0));
        assert_eq!(at("E. Australia Standard Time", 1, 15, 0, 0), (1, 15, 10, 0));
        assert_eq!(at("New Zealand Standard Time", 9, 26, 13, 59), (9, 27, 1, 59));
        assert_eq!(at("New Zealand Standard Time", 9, 26, 14, 0), (9, 27, 3, 0));
        assert_eq!(at("New Zealand Standard Time", 4, 4, 13, 59), (4, 5, 2, 59));
        assert_eq!(at("New Zealand Standard Time", 4, 4, 14, 0), (4, 5, 2, 0));
    }

    #[test]
    fn a_zone_at_a_year_change_rolls_the_date_over() {
        let t = FixedCalendar::default().to_zone("Tokyo Standard Time", &Tm::new(2026, 12, 31, 4, 15, 30, 7, 250)).unwrap();
        assert_eq!(t, Tm::new(2027, 1, 1, 5, 0, 30, 7, 250));
    }

    #[test]
    fn an_unknown_zone_is_none_never_a_guess() {
        assert_eq!(FixedCalendar::default().to_zone("Atlantis Standard Time", &Tm::new(2026, 1, 1, 4, 0, 0, 0, 0)), None);
        assert!(!FixedCalendar::default().zone_keys().iter().any(|k| k == "Egypt Standard Time"));
    }

    #[test]
    fn local_time_reads_back_to_utc_across_the_change() {
        let ny = |mo, d, h, mi| {
            let local = Tm::new(2026, mo, d, 0, h, mi, 0, 0);
            FixedCalendar::in_zone(local, "Eastern Standard Time").local_to_utc(&local).map(|t| (t.month, t.day, t.hour, t.minute))
        };
        assert_eq!(ny(3, 8, 1, 59), Some((3, 8, 6, 59)));
        assert_eq!(ny(3, 8, 3, 0), Some((3, 8, 7, 0)));
        assert_eq!(ny(7, 1, 12, 0), Some((7, 1, 16, 0)));
        assert_eq!(ny(11, 1, 1, 30), Some((11, 1, 5, 30)), "the repeated hour reads as the first");
        assert_eq!(ny(11, 1, 2, 0), Some((11, 1, 7, 0)));
        // the default calendar is UTC, with a Unix time to match
        let c = FixedCalendar::default();
        assert_eq!((c.now(), c.unix_ms()), (Tm::new(2026, 9, 21, 1, 12, 0, 0, 0), 1_789_992_000_000));
    }

    #[test]
    fn a_scripted_probe_plays_its_readings_then_repeats_the_last() {
        let (a, b) = (Reading { processes: 1, ..Reading::demo() }, Reading { processes: 2, ..Reading::demo() });
        let p = ScriptedProbe::new([a, b]);
        assert_eq!([p.read().processes, p.read().processes, p.read().processes], [1, 2, 2]);
        let d = ScriptedProbe::demo();
        let (first, second) = (d.read(), d.read());
        assert_eq!(first, Reading::demo());
        assert_eq!((second.cpu.0 - first.cpu.0, second.cpu.1 - first.cpu.1), (6_300_000, 10_000_000), "the counters move on");
    }

    #[test]
    fn fixed_media_is_a_paused_seekable_track_that_remembers_its_controls() {
        let m = FixedMedia::default();
        let t = m.track();
        assert_eq!((t.active, t.playing, t.can_seek, t.title.as_str(), t.at), (true, false, true, "Song", None));
        assert!(m.controls().is_empty());
        m.control(Control::PlayPause);
        m.control(Control::Seek(0.5));
        assert_eq!(m.controls(), [Control::PlayPause, Control::Seek(0.5)]);
        assert_eq!(m.track(), t, "controls change nothing on the fixed track");
    }

    #[test]
    fn names_are_english_and_follow_the_day_given() {
        let c = FixedCalendar::default();
        let t = Tm::new(2026, 9, 21, 1, 0, 0, 0, 0);
        let all = [DateStyle::Weekday, DateStyle::WeekdayShort, DateStyle::Month, DateStyle::Date, DateStyle::DateShort].map(|s| c.date_text(&t, s));
        assert_eq!(all, ["Monday", "Mon", "September", "Monday, 21 September", "21 Sep"]);
    }

    #[test]
    fn the_tone_is_steady_and_carries_on_between_reads() {
        let t = Tone::new([480.0], 0.5);
        assert_eq!(t.open(), Ok(48_000));
        let (mut a, mut b) = (vec![], vec![]);
        t.read(&mut a).unwrap();
        t.read(&mut b).unwrap();
        assert_eq!((a.len(), b.len()), (480, 480), "10 ms a read");
        assert!(a.iter().all(|s| s.abs() <= 0.5) && a.iter().any(|s| s.abs() > 0.49));
        // 480 Hz at 48 kHz repeats every 100 samples, so the second read (from sample 480) starts where sample 80 was
        assert!((b[0] - a[80]).abs() < 1e-5, "{} {}", b[0], a[80]);
        let fresh = Tone::new([480.0], 0.5);
        let mut c = vec![];
        fresh.read(&mut c).unwrap();
        assert_eq!(a, c, "a new tone starts the same way");
        assert!(!t.realtime() && !Silence.realtime());
        let mut s = vec![];
        Silence.read(&mut s).unwrap();
        assert!(s.len() == 480 && s.iter().all(|x| *x == 0.0));
    }

    #[test]
    fn a_tile_is_a_rounded_square_coloured_by_the_file_stem() {
        let icons = TileIcons;
        let a = icons.shell_icon(Path::new(r"C:\Apps\Chrome.exe")).unwrap();
        assert_eq!((a.w, a.h, a.px.len()), (48, 48, 48 * 48 * 4));
        let alpha = |d: &Decoded, x: usize, y: usize| d.px[(y * 48 + x) * 4 + 3];
        assert_eq!((alpha(&a, 0, 0), alpha(&a, 24, 24)), (0, 255), "transparent corner, opaque middle");
        let same = icons.shell_icon(Path::new("/elsewhere/chrome.lnk")).unwrap();
        let other = icons.shell_icon(Path::new(r"C:\Apps\Firefox.exe")).unwrap();
        assert_eq!(a.px, same.px, "the stem decides, not the folder, extension or case");
        assert_ne!(a.px[..3], other.px[..3], "two apps get two colours");
        assert_eq!(icons.resolve_path("chrome"), Some(PathBuf::from("chrome")), "a target resolves to itself");
        assert_eq!((icons.resolve_path("https://example.com"), icons.resolve_path("")), (None, None));
    }
}
