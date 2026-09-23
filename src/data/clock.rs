use super::{Cadence, DataSource, SourceCx};
use crate::value::Value;

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

pub fn now_local() -> Tm {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let t = unsafe { GetLocalTime() };
    Tm {
        year: t.wYear as i32,
        month: t.wMonth as u32,
        day: t.wDay as u32,
        dow: t.wDayOfWeek as u32,
        hour: t.wHour as u32,
        minute: t.wMinute as u32,
        second: t.wSecond as u32,
        ms: t.wMilliseconds as u32,
    }
}

fn localized_date(tm: &Tm, pattern: &str) -> String {
    use windows::Win32::Foundation::SYSTEMTIME;
    use windows::Win32::Globalization::{ENUM_DATE_FORMATS_FLAGS, GetDateFormatEx};
    use windows::core::{HSTRING, PCWSTR};
    let st = SYSTEMTIME {
        wYear: tm.year as u16,
        wMonth: tm.month as u16,
        wDayOfWeek: tm.dow as u16,
        wDay: tm.day as u16,
        ..Default::default()
    };
    let mut buf = [0u16; 96];
    let n = unsafe { GetDateFormatEx(PCWSTR::null(), ENUM_DATE_FORMATS_FLAGS(0), Some(&st as *const _), &HSTRING::from(pattern), Some(&mut buf), PCWSTR::null()) };
    if n <= 1 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..(n - 1) as usize])
}

pub fn clock_value(tm: &Tm) -> Value {
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
        ("weekday", localized_date(tm, "dddd").into()),
        ("weekday_short", localized_date(tm, "ddd").into()),
        ("month_name", localized_date(tm, "MMMM").into()),
        ("date", localized_date(tm, "dddd, d MMMM").into()),
        ("date_short", localized_date(tm, "d MMM").into()),
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

type Zone = windows::Win32::System::Time::DYNAMIC_TIME_ZONE_INFORMATION;

/// Every zone Windows knows, by key name; enumerated once.
fn zones() -> &'static [(String, Zone)] {
    static ZONES: std::sync::OnceLock<Vec<(String, Zone)>> = std::sync::OnceLock::new();
    ZONES.get_or_init(|| {
        use windows::Win32::System::Time::EnumDynamicTimeZoneInformation;
        let mut out = Vec::new();
        for i in 0.. {
            let mut z = Zone::default();
            if unsafe { EnumDynamicTimeZoneInformation(i, &mut z) } != 0 {
                break; // ERROR_NO_MORE_ITEMS
            }
            let key = String::from_utf16_lossy(&z.TimeZoneKeyName).trim_end_matches('\0').to_string();
            out.push((key, z));
        }
        out
    })
}

fn zone_of(city: &str) -> Option<&'static Zone> {
    let c = city.trim().to_lowercase();
    let key = CITY_ZONES.iter().find(|(n, _)| *n == c).map(|(_, k)| k.to_lowercase());
    let all = zones();
    let by_key = |k: &str| all.iter().find(|(n, _)| n.to_lowercase() == k).map(|(_, z)| z);
    key.as_deref().and_then(by_key).or_else(|| by_key(&c)).or_else(|| (c.len() >= 3).then(|| all.iter().find(|(n, _)| n.to_lowercase().contains(&c)).map(|(_, z)| z)).flatten())
}

/// Days since 1970-01-01 (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

/// "+9h", "-5h 30m", "same time".
pub fn offset_text(minutes: i64) -> String {
    if minutes == 0 {
        return "same time".into();
    }
    let (sign, m) = (if minutes < 0 { '-' } else { '+' }, minutes.abs());
    if m % 60 == 0 { format!("{sign}{}h", m / 60) } else { format!("{sign}{}h {}m", m / 60, m % 60) }
}

/// The time in each of a comma-separated list of cities, relative to `here`.
pub fn zone_times(cities: &str, utc: &windows::Win32::Foundation::SYSTEMTIME, here: &Tm) -> Vec<Value> {
    use windows::Win32::System::Time::SystemTimeToTzSpecificLocalTimeEx;
    let here_min = days_from_civil(here.year, here.month, here.day) * 1440 + here.hour as i64 * 60 + here.minute as i64;
    let here_day = days_from_civil(here.year, here.month, here.day);
    cities
        .split(',')
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(|city| {
            let mut t = windows::Win32::Foundation::SYSTEMTIME::default();
            let known = zone_of(city).is_some_and(|z| unsafe { SystemTimeToTzSpecificLocalTimeEx(Some(z), utc, &mut t) }.is_ok());
            if !known {
                return Value::obj([("city", city.into()), ("time", "?".into()), ("known", false.into())]);
            }
            let (h, m) = (t.wHour as u32, t.wMinute as u32);
            let day = days_from_civil(t.wYear as i32, t.wMonth as u32, t.wDay as u32);
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

pub struct Clock;

impl DataSource for Clock {
    fn name(&self) -> &'static str {
        "clock"
    }

    fn value(&self, cx: &SourceCx) -> Value {
        let mut v = clock_value(&cx.tm);
        // world clocks for the Instance's `cities` param, if its Widget has one
        let cities = cx.cfg.params.get("cities").and_then(|c| c.as_str()).unwrap_or("");
        if let Value::Obj(m) = &mut v {
            let utc = unsafe { windows::Win32::System::SystemInformation::GetSystemTime() };
            m.insert("zones".into(), Value::List(zone_times(cities, &utc, &cx.tm)));
        }
        v
    }

    fn cadence(&self, field: &str) -> Option<Cadence> {
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

    pub fn tm(h: u32, m: u32, s: u32, ms: u32) -> Tm {
        Tm { year: 2026, month: 9, day: 21, dow: 1, hour: h, minute: m, second: s, ms }
    }

    fn utc(y: u16, mo: u16, d: u16, h: u16, mi: u16) -> windows::Win32::Foundation::SYSTEMTIME {
        windows::Win32::Foundation::SYSTEMTIME { wYear: y, wMonth: mo, wDay: d, wHour: h, wMinute: mi, ..Default::default() }
    }

    #[test]
    fn world_clocks_show_each_city_relative_to_here() {
        // "here" is UTC, at noon on a January day (no daylight saving north of the equator)
        let here = Tm { year: 2026, month: 1, day: 15, dow: 4, hour: 12, minute: 0, second: 0, ms: 0 };
        let z = zone_times("Tokyo, new york,Sydney, Atlantis", &utc(2026, 1, 15, 12, 0), &here);
        let field = |i: usize, k: &str| z[i].get(k).map(|v| v.to_string()).unwrap_or_default();
        assert_eq!((field(0, "city"), field(0, "time"), field(0, "offset"), field(0, "day")), ("Tokyo".into(), "21:00".into(), "+9h".into(), "".into()));
        assert_eq!((field(1, "city"), field(1, "time"), field(1, "offset")), ("new york".into(), "07:00".into(), "-5h".into()));
        assert_eq!((field(2, "time"), field(2, "offset")), ("23:00".into(), "+11h".into()), "Sydney is on summer time in January");
        assert_eq!((field(3, "city"), field(3, "time")), ("Atlantis".into(), "?".into()), "an unknown city is marked, never guessed");

        let late = zone_times("Tokyo", &utc(2026, 1, 15, 20, 30), &Tm { hour: 20, minute: 30, ..here });
        assert_eq!((late[0].get("time").unwrap().to_string(), late[0].get("day").unwrap().to_string()), ("05:30".into(), "+1".into()));
        assert!(zone_times("", &utc(2026, 1, 15, 12, 0), &here).is_empty());
    }

    #[test]
    fn offsets_read_like_a_clock() {
        assert_eq!((offset_text(540), offset_text(-300), offset_text(330), offset_text(0), offset_text(-570)), ("+9h".into(), "-5h".into(), "+5h 30m".into(), "same time".into(), "-9h 30m".into()));
    }

    #[test]
    fn clock_fields_and_angles() {
        let v = clock_value(&tm(15, 30, 20, 500));
        assert_eq!(v.get("hour12"), Some(&Value::Num(3.0)));
        assert_eq!(v.get("ampm"), Some(&Value::Str("PM".into())));
        assert_eq!(v.get("hour_angle"), Some(&Value::Num(105.0))); // 3:30 -> 90 + 15
        assert_eq!(v.get("minute_angle"), Some(&Value::Num(182.0))); // 30*6 + floor(20/10)
        assert_eq!(v.get("second_angle"), Some(&Value::Num(120.0)));
        assert_eq!(clock_value(&tm(0, 0, 0, 0)).get("hour12"), Some(&Value::Num(12.0)));
    }
}
