use std::sync::Arc;

use super::{Cadence, DataSource, SourceCx};
use crate::ambient::{Calendar, DateStyle, days_from_civil};
use crate::value::Value;

pub use crate::ambient::Tm;

pub fn clock_value(tm: &Tm, cal: &dyn Calendar) -> Value {
    let h12 = if tm.hour % 12 == 0 { 12 } else { tm.hour % 12 };
    let (h, m, s) = (tm.hour as f64, tm.minute as f64, tm.second as f64);
    Value::obj([
        ("hour", (tm.hour as i32).into()),
        ("hour12", (h12 as i32).into()),
        ("minute", (tm.minute as i32).into()),
        ("second", (tm.second as i32).into()),
        ("ms", (tm.ms as i32).into()),
        ("is_pm", (tm.hour >= 12).into()),
        ("ampm", (if tm.hour >= 12 { "PM" } else { "AM" }).into()),
        ("day", (tm.day as i32).into()),
        ("month", (tm.month as i32).into()),
        ("year", tm.year.into()),
        ("weekday", cal.date_text(tm, DateStyle::Weekday).into()),
        ("weekday_short", cal.date_text(tm, DateStyle::WeekdayShort).into()),
        ("month_name", cal.date_text(tm, DateStyle::Month).into()),
        ("date", cal.date_text(tm, DateStyle::Date).into()),
        ("date_short", cal.date_text(tm, DateStyle::DateShort).into()),
        // angles, degrees clockwise from 12
        ("hour_angle", ((h % 12.0) * 30.0 + m * 0.5).into()),
        // steps every 10 s (0.1 deg/s * 10): smooth to the eye, 6x cheaper than per-second
        ("minute_angle", (m * 6.0 + (s / 10.0).floor()).into()),
        ("second_angle", (s * 6.0).into()),
        ("second_smooth", (s * 6.0 + tm.ms as f64 * 0.006).into()),
    ])
}

// ---- world clocks --------------------------------------------------------------

/// Windows names time zones, not cities ("Eastern Standard Time"), so common cities
/// are mapped here. Anything else may still match part of a zone's key ("Korea").
const CITY_ZONES: &[(&str, &str)] = &[
    ("new york", "Eastern Standard Time"), ("toronto", "Eastern Standard Time"), ("washington", "Eastern Standard Time"),
    ("boston", "Eastern Standard Time"), ("miami", "Eastern Standard Time"), ("montreal", "Eastern Standard Time"),
    ("chicago", "Central Standard Time"), ("dallas", "Central Standard Time"), ("houston", "Central Standard Time"),
    ("denver", "Mountain Standard Time"), ("phoenix", "US Mountain Standard Time"),
    ("los angeles", "Pacific Standard Time"), ("san francisco", "Pacific Standard Time"), ("seattle", "Pacific Standard Time"),
    ("vancouver", "Pacific Standard Time"), ("anchorage", "Alaskan Standard Time"), ("honolulu", "Hawaiian Standard Time"),
    ("mexico city", "Central Standard Time (Mexico)"), ("sao paulo", "E. South America Standard Time"),
    ("são paulo", "E. South America Standard Time"), ("rio de janeiro", "E. South America Standard Time"),
    ("buenos aires", "Argentina Standard Time"), ("london", "GMT Standard Time"), ("dublin", "GMT Standard Time"),
    ("lisbon", "GMT Standard Time"), ("reykjavik", "Greenwich Standard Time"), ("paris", "Romance Standard Time"),
    ("madrid", "Romance Standard Time"), ("brussels", "Romance Standard Time"), ("amsterdam", "W. Europe Standard Time"),
    ("berlin", "W. Europe Standard Time"), ("rome", "W. Europe Standard Time"), ("vienna", "W. Europe Standard Time"),
    ("zurich", "W. Europe Standard Time"), ("stockholm", "W. Europe Standard Time"), ("oslo", "W. Europe Standard Time"),
    ("warsaw", "Central European Standard Time"), ("warszawa", "Central European Standard Time"),
    ("krakow", "Central European Standard Time"), ("kraków", "Central European Standard Time"),
    ("prague", "Central Europe Standard Time"), ("budapest", "Central Europe Standard Time"),
    ("athens", "GTB Standard Time"), ("bucharest", "GTB Standard Time"), ("helsinki", "FLE Standard Time"),
    ("kyiv", "FLE Standard Time"), ("kiev", "FLE Standard Time"), ("vilnius", "FLE Standard Time"),
    ("istanbul", "Turkey Standard Time"), ("moscow", "Russian Standard Time"), ("cairo", "Egypt Standard Time"),
    ("johannesburg", "South Africa Standard Time"), ("lagos", "W. Central Africa Standard Time"),
    ("nairobi", "E. Africa Standard Time"), ("jerusalem", "Israel Standard Time"), ("dubai", "Arabian Standard Time"),
    ("tehran", "Iran Standard Time"), ("karachi", "Pakistan Standard Time"), ("mumbai", "India Standard Time"),
    ("delhi", "India Standard Time"), ("new delhi", "India Standard Time"), ("bangalore", "India Standard Time"),
    ("kolkata", "India Standard Time"), ("dhaka", "Bangladesh Standard Time"), ("bangkok", "SE Asia Standard Time"),
    ("jakarta", "SE Asia Standard Time"), ("hanoi", "SE Asia Standard Time"), ("singapore", "Singapore Standard Time"),
    ("kuala lumpur", "Singapore Standard Time"), ("manila", "Singapore Standard Time"), ("beijing", "China Standard Time"),
    ("shanghai", "China Standard Time"), ("hong kong", "China Standard Time"), ("taipei", "Taipei Standard Time"),
    ("seoul", "Korea Standard Time"), ("tokyo", "Tokyo Standard Time"), ("osaka", "Tokyo Standard Time"),
    ("sydney", "AUS Eastern Standard Time"), ("melbourne", "AUS Eastern Standard Time"), ("canberra", "AUS Eastern Standard Time"),
    ("brisbane", "E. Australia Standard Time"), ("perth", "W. Australia Standard Time"), ("adelaide", "Cen. Australia Standard Time"),
    ("auckland", "New Zealand Standard Time"), ("wellington", "New Zealand Standard Time"), ("utc", "UTC"), ("gmt", "UTC"),
];

/// The key of the zone `city` names, among the calendar's `keys`.
fn zone_of<'a>(city: &str, keys: &'a [String]) -> Option<&'a str> {
    let c = city.trim().to_lowercase();
    let key = CITY_ZONES.iter().find(|(n, _)| *n == c).map(|(_, k)| k.to_lowercase());
    let by_key = |k: &str| keys.iter().find(|n| n.to_lowercase() == k).map(String::as_str);
    key.as_deref().and_then(by_key).or_else(|| by_key(&c)).or_else(|| (c.len() >= 3).then(|| keys.iter().find(|n| n.to_lowercase().contains(&c)).map(String::as_str)).flatten())
}

/// "+9h", "-5h 30m", "same time".
pub fn offset_text(minutes: i64) -> String {
    if minutes == 0 {
        return "same time".into();
    }
    let (sign, m) = (if minutes < 0 { '-' } else { '+' }, minutes.abs());
    if m % 60 == 0 { format!("{sign}{}h", m / 60) } else { format!("{sign}{}h {}m", m / 60, m % 60) }
}

/// The time in each of a comma-separated list of cities, relative to `here`; `utc` is the
/// moment `here` shows.
pub fn zone_times(cities: &str, utc: &Tm, here: &Tm, cal: &dyn Calendar) -> Vec<Value> {
    let here_day = days_from_civil(here.year, here.month, here.day);
    let here_min = here_day * 1440 + here.hour as i64 * 60 + here.minute as i64;
    let cities: Vec<&str> = cities.split(',').map(str::trim).filter(|c| !c.is_empty()).collect();
    let keys = if cities.is_empty() { Vec::new() } else { cal.zone_keys() };
    cities
        .into_iter()
        .map(|city| {
            let Some(t) = zone_of(city, &keys).and_then(|k| cal.to_zone(k, utc)) else {
                return Value::obj([("city", city.into()), ("time", "?".into()), ("known", false.into())]);
            };
            let (h, m) = (t.hour, t.minute);
            let day = days_from_civil(t.year, t.month, t.day);
            let offset = day * 1440 + h as i64 * 60 + m as i64 - here_min;
            let h12 = if h % 12 == 0 { 12 } else { h % 12 };
            Value::obj([
                ("city", city.into()),
                ("known", true.into()),
                ("time", format!("{h:02}:{m:02}").into()),
                ("hour", (h as i32).into()),
                ("hour12", (h12 as i32).into()),
                ("minute", (m as i32).into()),
                ("ampm", (if h >= 12 { "PM" } else { "AM" }).into()),
                ("night", Value::Bool(!(6..18).contains(&h))),
                ("day", (match day - here_day { 0 => "", d if d > 0 => "+1", _ => "-1" }).into()),
                ("offset", offset_text(offset).into()),
                ("hour_angle", ((h % 12) as f64 * 30.0 + m as f64 * 0.5).into()),
                ("minute_angle", (m as f64 * 6.0).into()),
            ])
        })
        .collect()
}

pub struct Clock {
    calendar: Arc<dyn Calendar>,
}

impl Clock {
    pub fn new(calendar: Arc<dyn Calendar>) -> Self {
        Self { calendar }
    }
}

impl DataSource for Clock {
    fn name(&self) -> &str {
        "clock"
    }

    fn value(&self, cx: &SourceCx) -> Value {
        let cal = &*self.calendar;
        let mut v = clock_value(cx.tm(), cal);
        // world clocks for the Instance's `cities` param, if its Widget has one
        let cities = cx.params().get("cities").map(|c| c.to_string()).unwrap_or_default();
        if let Value::Obj(m) = &mut v {
            // the instant the clock shows, so city times always agree with the face
            let utc = cal.local_to_utc(cx.tm()).unwrap_or_else(|| Tm::from_unix_ms(cal.unix_ms()));
            m.insert("zones".into(), Value::List(zone_times(&cities, &utc, cx.tm(), cal)));
        }
        v
    }

    fn cadence(&self, field: &str, _cx: &SourceCx) -> Option<Cadence> {
        Some(match field {
            // the whole object changes as often as its fastest field
            "" | "ms" | "second_smooth" => Cadence::Frame,
            "second" | "second_angle" => Cadence::Second,
            "minute_angle" => Cadence::TenSecond,
            _ => Cadence::Minute,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ambient::{Ambient, FixedCalendar, WinCalendar};

    pub fn tm(h: u32, m: u32, s: u32, ms: u32) -> Tm {
        Tm { year: 2026, month: 9, day: 21, dow: 1, hour: h, minute: m, second: s, ms }
    }

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> Tm {
        Tm { year: y, month: mo, day: d, dow: 0, hour: h, minute: mi, second: 0, ms: 0 }
    }

    /// The cities of the first world clock test, on whichever calendar `cal` is.
    fn world_clocks(cal: &dyn Calendar) {
        // "here" is UTC, at noon on a January day (no daylight saving north of the equator)
        let here = Tm { year: 2026, month: 1, day: 15, dow: 4, hour: 12, minute: 0, second: 0, ms: 0 };
        let z = zone_times("Tokyo, new york,Sydney, Atlantis", &utc(2026, 1, 15, 12, 0), &here, cal);
        let field = |i: usize, k: &str| z[i].get(k).map(|v| v.to_string()).unwrap_or_default();
        assert_eq!((field(0, "city"), field(0, "time"), field(0, "offset"), field(0, "day")), ("Tokyo".into(), "21:00".into(), "+9h".into(), "".into()));
        assert_eq!((field(1, "city"), field(1, "time"), field(1, "offset")), ("new york".into(), "07:00".into(), "-5h".into()));
        assert_eq!((field(2, "time"), field(2, "offset")), ("23:00".into(), "+11h".into()), "Sydney is on summer time in January");
        assert_eq!((field(3, "city"), field(3, "time")), ("Atlantis".into(), "?".into()), "an unknown city is marked, never guessed");

        let late = zone_times("Tokyo", &utc(2026, 1, 15, 20, 30), &Tm { hour: 20, minute: 30, ..here }, cal);
        assert_eq!((late[0].get("time").unwrap().to_string(), late[0].get("day").unwrap().to_string()), ("05:30".into(), "+1".into()));
        assert!(zone_times("", &utc(2026, 1, 15, 12, 0), &here, cal).is_empty());
    }

    #[test]
    fn world_clocks_show_each_city_relative_to_here() {
        world_clocks(&FixedCalendar::default());
    }

    #[test]
    fn the_machines_calendar_gives_the_same_world_clocks() {
        world_clocks(&WinCalendar);
    }

    #[test]
    fn a_city_is_found_by_name_by_zone_key_or_by_part_of_a_key() {
        let keys = FixedCalendar::default().zone_keys();
        assert_eq!(zone_of(" Sao Paulo ", &keys), Some("E. South America Standard Time"));
        assert_eq!(zone_of("pacific standard time", &keys), Some("Pacific Standard Time"));
        assert_eq!(zone_of("korea", &keys), Some("Korea Standard Time"));
        assert_eq!((zone_of("xx", &keys), zone_of("Atlantis", &keys)), (None, None));
    }

    #[test]
    fn every_city_names_a_zone_the_fixed_table_knows_but_two() {
        let keys = FixedCalendar::default().zone_keys();
        let missing: std::collections::BTreeSet<_> = CITY_ZONES.iter().map(|(_, k)| *k).filter(|k| !keys.iter().any(|n| n == k)).collect();
        assert_eq!(missing, std::collections::BTreeSet::from(["Egypt Standard Time", "Israel Standard Time"]));
    }

    #[test]
    fn a_city_time_across_a_daylight_change_moves_an_hour_on_the_fixed_calendar() {
        // London and New York on the morning the US has changed and the EU has not
        let cal = FixedCalendar::default();
        let here = Tm { year: 2026, month: 3, day: 9, dow: 1, hour: 12, minute: 0, second: 0, ms: 0 };
        let z = zone_times("London, New York", &utc(2026, 3, 9, 12, 0), &here, &cal);
        let offset = |i: usize| z[i].get("offset").unwrap().to_string();
        assert_eq!((offset(0), offset(1)), ("same time".to_string(), "-4h".to_string()));
    }

    #[test]
    fn world_clocks_follow_the_time_the_clock_shows_not_the_system_clock() {
        // a moment far from now: the zones must be relative to it, so offsets stay within a day
        let saved = std::collections::BTreeMap::new();
        let params = std::collections::BTreeMap::from([("cities".to_string(), Value::Str("Tokyo, New York".into()))]);
        let cx = SourceCx::new(crate::data::InstanceRef::new("", &saved), &params, tm(15, 42, 0, 0), "Default");
        let Some(Value::List(z)) = Clock::new(Ambient::fixed().calendar).value(&cx).get("zones").cloned() else { panic!("no zones") };
        for zone in &z {
            let off = zone.get("offset").unwrap().to_string();
            let hours: i32 = off.trim_start_matches(['+', '-']).split('h').next().unwrap_or("0").parse().unwrap_or(0);
            assert!(off == "same time" || hours <= 14, "{off}");
        }
        // on the fixed calendar (local time is UTC) the offsets are exact
        let off = |i: usize| z[i].get("offset").unwrap().to_string();
        assert_eq!((off(0), off(1)), ("+9h".to_string(), "-4h".to_string()), "New York is on daylight time in September");
    }

    #[test]
    fn offsets_read_like_a_clock() {
        assert_eq!((offset_text(540), offset_text(-300), offset_text(330), offset_text(0), offset_text(-570)), ("+9h".into(), "-5h".into(), "+5h 30m".into(), "same time".into(), "-9h 30m".into()));
    }

    #[test]
    fn clock_fields_and_angles() {
        let cal = FixedCalendar::default();
        let v = clock_value(&tm(15, 30, 20, 500), &cal);
        assert_eq!(v.get("hour12"), Some(&Value::Num(3.0)));
        assert_eq!(v.get("ampm"), Some(&Value::Str("PM".into())));
        assert_eq!(v.get("hour_angle"), Some(&Value::Num(105.0))); // 3:30 -> 90 + 15
        assert_eq!(v.get("minute_angle"), Some(&Value::Num(182.0))); // 30*6 + floor(20/10)
        assert_eq!(v.get("second_angle"), Some(&Value::Num(120.0)));
        assert_eq!(clock_value(&tm(0, 0, 0, 0), &cal).get("hour12"), Some(&Value::Num(12.0)));
        // the names come from the calendar, here English
        assert_eq!((v.get("weekday"), v.get("date")), (Some(&Value::Str("Monday".into())), Some(&Value::Str("Monday, 21 September".into()))));
    }
}
