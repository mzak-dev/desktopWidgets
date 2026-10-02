//! The month the clock shows, laid out as a calendar: weeks of seven days that start on the
//! user's first day of the week, today marked, with ISO 8601 week numbers. Its own source,
//! so clock widgets never build a month on every tick.
//!
//! Fields: `weeks` (each `week`, the ISO week number, and `days`: `day`, `this_month`,
//! `today`, `weekend`), `day_names` and `day_letters` in the week's order, `month_name`,
//! `year`, `day`, `weekday`, `week`.

use std::sync::Arc;

use crate::ambient::{Calendar as Time, DateStyle, WinCalendar, days_from_civil};
use super::{Cadence, DataSource, SourceCx, Tm};
use crate::value::Value;

/// Monday is 0, Sunday 6, as Windows numbers `LOCALE_IFIRSTDAYOFWEEK`.
pub fn weekday(y: i32, m: u32, d: u32) -> u32 {
    // 1970-01-01 was a Thursday
    (days_from_civil(y, m, d) + 3).rem_euclid(7) as u32
}

pub fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// ISO 8601: weeks start on Monday, and week 1 holds the year's first Thursday.
pub fn iso_week(y: i32, m: u32, d: u32) -> u32 {
    let days = days_from_civil(y, m, d);
    let thursday = days - weekday(y, m, d) as i64 + 3;
    // the week belongs to its Thursday's year
    let (ty, _, _) = civil_from_days(thursday);
    ((thursday - days_from_civil(ty, 1, 1)) / 7 + 1) as u32
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub y: i32,
    pub m: u32,
    pub day: u32,
    pub this_month: bool,
    pub today: bool,
    pub weekend: bool,
}

/// The weeks that hold month `m` of year `y`, each seven days from `first` (0 Monday ... 6
/// Sunday), with the neighbouring months' days filling the first and last week. Four to
/// six weeks, as the month needs.
pub fn month_grid(y: i32, m: u32, today: Option<u32>, first: u32) -> Vec<[Cell; 7]> {
    let lead = (weekday(y, m, 1) + 7 - first % 7) % 7;
    let start = days_from_civil(y, m, 1) - lead as i64;
    let len = lead + days_in_month(y, m);
    let rows = len.div_ceil(7);
    (0..rows)
        .map(|r| {
            std::array::from_fn(|c| {
                let (cy, cm, cd) = civil_from_days(start + (r * 7 + c as u32) as i64);
                let this_month = cy == y && cm == m;
                let wd = weekday(cy, cm, cd);
                Cell { y: cy, m: cm, day: cd, this_month, today: this_month && today == Some(cd), weekend: wd >= 5 }
            })
        })
        .collect()
}

/// The inverse of `days_from_civil` (Howard Hinnant's `civil_from_days`).
pub fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = (yoe + era * 400 + if m <= 2 { 1 } else { 0 }) as i32;
    (y, m, d)
}

/// The month around `tm`, from `first` (0 Monday ... 6 Sunday), with day names Monday first.
pub fn calendar_value(tm: &Tm, first: u32, names: &[String; 7]) -> Value {
    calendar_value_in(tm, first, names, &WinCalendar)
}

/// `calendar_value` with the month and weekday names of `time` (the ambient calendar).
pub fn calendar_value_in(tm: &Tm, first: u32, names: &[String; 7], time: &dyn Time) -> Value {
    let weeks = month_grid(tm.year, tm.month, Some(tm.day), first)
        .iter()
        .map(|w| {
            // a week's number is its Thursday's, wherever the week starts
            let thu = w.iter().find(|c| weekday(c.y, c.m, c.day) == 3).copied().unwrap_or(w[0]);
            let days = w.iter().map(|c| Value::obj([("day", (c.day as i32).into()), ("this_month", c.this_month.into()), ("today", c.today.into()), ("weekend", c.weekend.into())])).collect();
            Value::obj([("week", (iso_week(thu.y, thu.m, thu.day) as i32).into()), ("days", Value::List(days))])
        })
        .collect();
    let order: Vec<&String> = (0..7).map(|i| &names[(first as usize + i) % 7]).collect();
    Value::obj([
        ("weeks", Value::List(weeks)),
        ("day_names", Value::List(order.iter().map(|n| Value::Str((*n).clone())).collect())),
        ("day_letters", Value::List(order.iter().map(|n| Value::Str(n.chars().next().map(String::from).unwrap_or_default())).collect())),
        ("month_name", time.date_text(tm, DateStyle::Month).into()),
        ("year", tm.year.into()),
        ("day", (tm.day as i32).into()),
        ("weekday", time.date_text(tm, DateStyle::Weekday).into()),
        ("week", (iso_week(tm.year, tm.month, tm.day) as i32).into()),
    ])
}

/// The calendar source over the ambient calendar: the real machine's locale in the app, en-US in a hermetic run.
pub struct Calendar {
    time: Arc<dyn Time>,
}

impl Calendar {
    pub fn new(time: Arc<dyn Time>) -> Self {
        Self { time }
    }
}

impl DataSource for Calendar {
    fn name(&self) -> &str {
        "calendar"
    }

    fn value(&self, cx: &SourceCx) -> Value {
        let w = self.time.week();
        calendar_value_in(&cx.tm, w.first, &w.names, self.time.as_ref())
    }

    /// Only the date matters; a minute is the coarsest wake there is.
    fn cadence(&self, _field: &str, _cx: &SourceCx) -> Option<Cadence> {
        Some(Cadence::Minute)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn days(g: &[[Cell; 7]]) -> Vec<Vec<u32>> {
        g.iter().map(|w| w.iter().map(|c| c.day).collect()).collect()
    }

    #[test]
    fn weekdays_and_month_lengths() {
        assert_eq!(weekday(1970, 1, 1), 3, "a Thursday");
        assert_eq!(weekday(2026, 10, 1), 3);
        assert_eq!((days_in_month(2024, 2), days_in_month(2026, 2), days_in_month(1900, 2), days_in_month(2000, 2)), (29, 28, 28, 29));
        for z in [-800_000i64, -1, 0, 1, 20_000, 1_000_000] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
    }

    #[test]
    fn iso_weeks_follow_the_years_first_thursday() {
        assert_eq!(iso_week(2026, 10, 1), 40);
        assert_eq!(iso_week(2021, 1, 3), 53, "early January can be the last week of the year before");
        assert_eq!(iso_week(2024, 12, 30), 1, "and late December the first of the next");
        assert_eq!(iso_week(2026, 1, 1), 1);
    }

    #[test]
    fn a_month_fills_whole_weeks_from_the_first_day_of_the_week() {
        // October 2026 starts on a Thursday
        let mon = month_grid(2026, 10, Some(1), 0);
        assert_eq!(days(&mon)[0], [28, 29, 30, 1, 2, 3, 4]);
        assert_eq!(mon.len(), 5);
        assert!(mon[0][3].today && mon[0][3].this_month && !mon[0][0].this_month);
        assert!(mon[0][5].weekend && mon[0][6].weekend && !mon[0][4].weekend);
        let sun = month_grid(2026, 10, None, 6);
        assert_eq!(days(&sun)[0], [27, 28, 29, 30, 1, 2, 3], "a Sunday-first week");
        assert!(sun[0][0].weekend, "Sunday is still the weekend");
    }

    #[test]
    fn four_five_or_six_weeks_as_the_month_needs() {
        assert_eq!(month_grid(2021, 2, None, 0).len(), 4, "February 2021 starts on a Monday: four weeks");
        assert_eq!(month_grid(2026, 8, None, 0).len(), 6, "August 2026 starts on a Saturday: six weeks");
        let leap = month_grid(2024, 2, None, 0);
        assert_eq!(leap.iter().flatten().filter(|c| c.this_month).count(), 29, "February in a leap year");
        let first_is_first = month_grid(2026, 6, None, 0);
        assert_eq!(first_is_first[0][0].day, 1, "June 2026 starts on a Monday: no days before it");
    }

    #[test]
    fn the_value_names_the_days_in_the_weeks_order_and_numbers_each_week() {
        let names: [String; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"].map(String::from);
        let tm = Tm { year: 2026, month: 10, day: 1, dow: 4, hour: 9, minute: 0, second: 0, ms: 0 };
        let v = calendar_value(&tm, 6, &names);
        let list = |k: &str| match v.get(k) {
            Some(Value::List(l)) => l.iter().map(|x| x.to_string()).collect::<Vec<_>>(),
            _ => panic!("{k}"),
        };
        assert_eq!(list("day_names")[..2], ["Su".to_string(), "Mo".to_string()]);
        assert_eq!(list("day_letters")[0], "S");
        let Some(Value::List(weeks)) = v.get("weeks") else { panic!("weeks") };
        assert_eq!(weeks[0].get("week"), Some(&Value::Num(40.0)), "the week of Thursday 1 October");
        assert_eq!(v.get("week"), Some(&Value::Num(40.0)));
    }
}
