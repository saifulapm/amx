//! When a timed sleep wakes, for each of the picker's five choices.
//!
//! Evening, tomorrow and next week are built as a civil date at an hour and
//! converted through the zone, never as `now + 86400`, so a daylight-saving
//! change moves the length of the day but not the wall-clock hour.

// The picker consumes this next.
#![cfg_attr(not(test), allow(dead_code))]

use jiff::Timestamp;
use jiff::civil::{Date, Weekday};
use jiff::tz::TimeZone;

/// One of the picker's timed choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Hour,
    ThreeHours,
    Evening,
    Tomorrow,
    NextWeek,
}

impl Choice {
    /// The choice a picker digit stands for: `1` to `5`, in order.
    pub fn of(digit: char) -> Option<Choice> {
        match digit {
            '1' => Some(Choice::Hour),
            '2' => Some(Choice::ThreeHours),
            '3' => Some(Choice::Evening),
            '4' => Some(Choice::Tomorrow),
            '5' => Some(Choice::NextWeek),
            _ => None,
        }
    }
}

/// The epoch second of `hour`:00 local on `day`.
fn at_hour(day: Date, hour: i8, zone: &TimeZone) -> Option<u64> {
    let zoned = day.at(hour, 0, 0, 0).to_zoned(zone.clone()).ok()?;
    u64::try_from(zoned.timestamp().as_second()).ok()
}

/// When a sleep chosen at `now` (epoch seconds) wakes, in epoch seconds,
/// with `zone` as local time.
pub fn wake_at(choice: Choice, now: u64, zone: &TimeZone) -> u64 {
    // A real epoch time never leaves jiff's date range, so the calendar
    // choices only fall back to an hour from now if that ever stops holding.
    let calendar = || -> Option<u64> {
        let today = Timestamp::from_second(i64::try_from(now).ok()?)
            .ok()?
            .to_zoned(zone.clone())
            .date();
        match choice {
            Choice::Hour | Choice::ThreeHours => None,
            Choice::Evening => at_hour(today, 18, zone)
                .filter(|&wake| wake > now)
                .or_else(|| at_hour(today.tomorrow().ok()?, 18, zone)),
            Choice::Tomorrow => at_hour(today.tomorrow().ok()?, 9, zone),
            Choice::NextWeek => at_hour(today.nth_weekday(1, Weekday::Monday).ok()?, 9, zone),
        }
    };
    match choice {
        Choice::Hour => now + 3600,
        Choice::ThreeHours => now + 10800,
        _ => calendar().unwrap_or(now + 3600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::{DateTime, date};

    fn zone() -> TimeZone {
        TimeZone::posix("EST5EDT,M3.2.0,M11.1.0").unwrap()
    }

    fn at(dt: DateTime) -> u64 {
        dt.to_zoned(zone()).unwrap().timestamp().as_second() as u64
    }

    fn local(secs: u64) -> DateTime {
        jiff::Timestamp::from_second(secs as i64)
            .unwrap()
            .to_zoned(zone())
            .datetime()
    }

    fn wake(choice: Choice, now: DateTime) -> DateTime {
        local(wake_at(choice, at(now), &zone()))
    }

    #[test]
    fn the_hour_choices_add_one_and_three_hours() {
        let now = date(2026, 5, 6).at(14, 20, 0, 0);
        assert_eq!(wake(Choice::Hour, now), date(2026, 5, 6).at(15, 20, 0, 0));
        assert_eq!(
            wake(Choice::ThreeHours, now),
            date(2026, 5, 6).at(17, 20, 0, 0)
        );
    }

    #[test]
    fn evening_before_six_is_six_today() {
        let now = date(2026, 5, 6).at(14, 20, 0, 0);
        assert_eq!(wake(Choice::Evening, now), date(2026, 5, 6).at(18, 0, 0, 0));
    }

    #[test]
    fn evening_after_six_is_six_tomorrow() {
        let now = date(2026, 5, 6).at(19, 5, 0, 0);
        assert_eq!(wake(Choice::Evening, now), date(2026, 5, 7).at(18, 0, 0, 0));
    }

    #[test]
    fn evening_at_exactly_six_is_six_tomorrow() {
        let now = date(2026, 5, 6).at(18, 0, 0, 0);
        assert_eq!(wake(Choice::Evening, now), date(2026, 5, 7).at(18, 0, 0, 0));
    }

    #[test]
    fn tomorrow_is_nine_the_next_day() {
        let now = date(2026, 5, 6).at(14, 20, 0, 0);
        assert_eq!(wake(Choice::Tomorrow, now), date(2026, 5, 7).at(9, 0, 0, 0));
    }

    #[test]
    fn next_week_on_a_monday_is_nine_the_following_monday() {
        let now = date(2026, 5, 4).at(14, 20, 0, 0);
        assert_eq!(
            wake(Choice::NextWeek, now),
            date(2026, 5, 11).at(9, 0, 0, 0)
        );
    }

    #[test]
    fn next_week_on_a_sunday_is_nine_the_next_day() {
        let now = date(2026, 5, 3).at(14, 20, 0, 0);
        assert_eq!(wake(Choice::NextWeek, now), date(2026, 5, 4).at(9, 0, 0, 0));
    }

    #[test]
    fn tomorrow_across_the_spring_change_is_still_nine_local() {
        // 14:20 EST to 09:00 EDT: 18 h 40 min on the wall, one hour less on the clock.
        let now = date(2026, 3, 7).at(14, 20, 0, 0);
        let wakes = wake_at(Choice::Tomorrow, at(now), &zone());
        assert_eq!(local(wakes), date(2026, 3, 8).at(9, 0, 0, 0));
        assert_eq!(wakes - at(now), 17 * 3600 + 40 * 60);
    }

    #[test]
    fn tomorrow_across_the_autumn_change_is_still_nine_local() {
        // 14:20 EDT to 09:00 EST: 18 h 40 min on the wall, one hour more on the clock.
        let now = date(2026, 10, 31).at(14, 20, 0, 0);
        let wakes = wake_at(Choice::Tomorrow, at(now), &zone());
        assert_eq!(local(wakes), date(2026, 11, 1).at(9, 0, 0, 0));
        assert_eq!(wakes - at(now), 19 * 3600 + 40 * 60);
    }

    #[test]
    fn digits_one_to_five_pick_the_choices_in_order() {
        assert_eq!(Choice::of('1'), Some(Choice::Hour));
        assert_eq!(Choice::of('2'), Some(Choice::ThreeHours));
        assert_eq!(Choice::of('3'), Some(Choice::Evening));
        assert_eq!(Choice::of('4'), Some(Choice::Tomorrow));
        assert_eq!(Choice::of('5'), Some(Choice::NextWeek));
    }

    #[test]
    fn other_characters_pick_nothing() {
        assert_eq!(Choice::of('6'), None);
        assert_eq!(Choice::of('z'), None);
    }
}
