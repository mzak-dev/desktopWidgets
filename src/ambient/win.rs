//! The real calendar: Win32 local time, locale date names and the system's time zones.

use windows::Win32::Foundation::SYSTEMTIME;
use windows::Win32::System::Time::DYNAMIC_TIME_ZONE_INFORMATION;

use super::{Calendar, DateStyle, Tm};

type Zone = DYNAMIC_TIME_ZONE_INFORMATION;

pub struct WinCalendar;

/// A C string in a fixed buffer: Windows may leave garbage after the terminator.
fn wide_until_nul(buf: &[u16]) -> String {
    String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(buf.len())])
}

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
            let key = wide_until_nul(&z.TimeZoneKeyName);
            out.push((key, z));
        }
        out
    })
}

fn tm_of(t: &SYSTEMTIME) -> Tm {
    Tm::new(t.wYear as i32, t.wMonth as u32, t.wDay as u32, t.wDayOfWeek as u32, t.wHour as u32, t.wMinute as u32, t.wSecond as u32, t.wMilliseconds as u32)
}

fn systime_of(tm: &Tm) -> SYSTEMTIME {
    SYSTEMTIME {
        wYear: tm.year as u16,
        wMonth: tm.month as u16,
        wDayOfWeek: tm.dow as u16,
        wDay: tm.day as u16,
        wHour: tm.hour as u16,
        wMinute: tm.minute as u16,
        wSecond: tm.second as u16,
        wMilliseconds: tm.ms as u16,
    }
}

fn localized_date(tm: &Tm, pattern: &str) -> String {
    use windows::Win32::Globalization::{ENUM_DATE_FORMATS_FLAGS, GetDateFormatEx};
    use windows::core::{HSTRING, PCWSTR};
    let st = SYSTEMTIME { wYear: tm.year as u16, wMonth: tm.month as u16, wDayOfWeek: tm.dow as u16, wDay: tm.day as u16, ..Default::default() };
    let mut buf = [0u16; 96];
    let n = unsafe { GetDateFormatEx(PCWSTR::null(), ENUM_DATE_FORMATS_FLAGS(0), Some(&st as *const _), &HSTRING::from(pattern), Some(&mut buf), PCWSTR::null()) };
    if n <= 1 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..(n - 1) as usize])
}

impl Calendar for WinCalendar {
    fn now(&self) -> Tm {
        use windows::Win32::System::SystemInformation::GetLocalTime;
        tm_of(&unsafe { GetLocalTime() })
    }

    fn unix_ms(&self) -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
    }

    fn date_text(&self, tm: &Tm, style: DateStyle) -> String {
        localized_date(
            tm,
            match style {
                DateStyle::Weekday => "dddd",
                DateStyle::WeekdayShort => "ddd",
                DateStyle::Month => "MMMM",
                DateStyle::Date => "dddd, d MMMM",
                DateStyle::DateShort => "d MMM",
            },
        )
    }

    fn local_to_utc(&self, local: &Tm) -> Option<Tm> {
        use windows::Win32::System::Time::TzSpecificLocalTimeToSystemTimeEx;
        // the wall time to the second, as the clock shows it
        let local = SYSTEMTIME { wYear: local.year as u16, wMonth: local.month as u16, wDay: local.day as u16, wHour: local.hour as u16, wMinute: local.minute as u16, wSecond: local.second as u16, ..Default::default() };
        let mut utc = SYSTEMTIME::default();
        unsafe { TzSpecificLocalTimeToSystemTimeEx(None, &local, &mut utc) }.ok().map(|_| tm_of(&utc))
    }

    fn zone_keys(&self) -> Vec<String> {
        zones().iter().map(|(k, _)| k.clone()).collect()
    }

    fn to_zone(&self, key: &str, utc: &Tm) -> Option<Tm> {
        use windows::Win32::System::Time::SystemTimeToTzSpecificLocalTimeEx;
        let zone = zones().iter().find(|(k, _)| k == key).map(|(_, z)| z)?;
        let mut t = SYSTEMTIME::default();
        unsafe { SystemTimeToTzSpecificLocalTimeEx(Some(zone), &systime_of(utc), &mut t) }.ok().map(|_| tm_of(&t))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zone_key_ends_at_its_terminator_not_at_the_buffer() {
        let mut buf = [0u16; 16];
        buf[..3].copy_from_slice(&[b'G' as u16, b'M' as u16, b'T' as u16]);
        buf[4..6].copy_from_slice(&[0x5b70, 0x87b3]); // what a release build found after the NUL
        assert_eq!(wide_until_nul(&buf), "GMT");
    }

    #[test]
    fn the_machines_zones_agree_with_the_fixed_table_at_the_dst_edges() {
        let (win, fixed) = (WinCalendar, super::super::FixedCalendar::default());
        let keys = win.zone_keys();
        for (key, ..) in super::super::fixed::ZONE_TABLE {
            if !keys.iter().any(|k| k == *key) {
                continue; // a Windows without this zone (none is expected)
            }
            // the minutes around every transition in 2026, and mid-season days
            for (mo, d) in [(1, 15), (3, 7), (3, 8), (3, 9), (3, 28), (3, 29), (3, 30), (4, 3), (4, 4), (4, 5), (7, 1), (9, 25), (9, 26), (9, 27), (10, 3), (10, 4), (10, 24), (10, 25), (10, 26), (11, 1), (11, 2), (12, 31)] {
                for (h, mi) in [(0, 0), (1, 0), (2, 0), (6, 59), (7, 0), (13, 59), (14, 0), (15, 59), (16, 0), (16, 29), (16, 30), (23, 59)] {
                    let utc = Tm::new(2026, mo, d, 0, h, mi, 0, 0);
                    let (w, f) = (win.to_zone(key, &utc), fixed.to_zone(key, &utc));
                    assert_eq!(w.map(|t| (t.year, t.month, t.day, t.hour, t.minute)), f.map(|t| (t.year, t.month, t.day, t.hour, t.minute)), "{key} at 2026-{mo:02}-{d:02} {h:02}:{mi:02} UTC");
                }
            }
        }
    }
}
