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

pub struct Clock;

impl DataSource for Clock {
    fn name(&self) -> &'static str {
        "clock"
    }

    fn value(&self, cx: &SourceCx) -> Value {
        clock_value(&cx.tm)
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
