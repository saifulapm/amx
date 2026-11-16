//! When a usage limit lifts, read from the words claude ends a limited turn
//! with.
//!
//! claude says `You've hit your limit · resets 3pm`, sometimes with a date
//! (`resets Nov 20 at 9am`, `resets Aug 24, 10am`) and a zone in brackets
//! (`(Asia/Dhaka)`). A text that does not read whole gives nothing, so the
//! row says plain `limited` rather than a wrong time.

use jiff::Timestamp;
use jiff::tz::TimeZone;

/// The epoch second the limit in `text` lifts, as said at `now`, with
/// `local` as the zone of a time that names none.
///
/// A bare time is its next occurrence after `now`. A date takes the year that
/// puts it nearest `now`, so a January date said in December is next year's.
pub fn resets_at(text: &str, now: u64, local: &TimeZone) -> Option<u64> {
    let (_, said) = text.rsplit_once("resets ")?;
    // The reset ends its line, or a ` · ` starts the next part.
    let said = said.lines().next()?;
    let said = said.split(" · ").next()?.trim();
    let (when, zone) = match said.split_once(" (") {
        Some((when, zone)) => (when, TimeZone::get(zone.strip_suffix(')')?).ok()?),
        None => (said, local.clone()),
    };
    let now_at = Timestamp::from_second(i64::try_from(now).ok()?).ok()?;
    let today = now_at.to_zoned(zone.clone()).date();
    let moment = |day: jiff::civil::Date, (hour, minute): (i8, i8)| -> Option<i64> {
        let zoned = day.at(hour, minute, 0, 0).to_zoned(zone.clone()).ok()?;
        Some(zoned.timestamp().as_second())
    };
    let words: Vec<&str> = when.split_whitespace().collect();
    let secs = match words[..] {
        [time] => {
            let time = clock(time)?;
            let first = moment(today, time)?;
            match first > now_at.as_second() {
                true => first,
                false => moment(today.tomorrow().ok()?, time)?,
            }
        }
        [month, day, "at", time] | [month, day, time] => {
            let day = match words.len() {
                4 => day,
                _ => day.strip_suffix(',')?,
            };
            let month = MONTHS.iter().position(|name| *name == month)? as i8 + 1;
            let day: i8 = day.parse().ok()?;
            let time = clock(time)?;
            [today.year() - 1, today.year(), today.year() + 1]
                .into_iter()
                .filter_map(|year| moment(jiff::civil::Date::new(year, month, day).ok()?, time))
                .min_by_key(|secs| secs.abs_diff(now_at.as_second()))?
        }
        _ => return None,
    };
    u64::try_from(secs).ok()
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// The hour and minute of `3pm` or `10:30am`.
fn clock(word: &str) -> Option<(i8, i8)> {
    let (time, pm) = match word.strip_suffix("pm") {
        Some(time) => (time, true),
        None => (word.strip_suffix("am")?, false),
    };
    let (hour, minute) = match time.split_once(':') {
        Some((hour, minute)) if minute.len() == 2 => (hour, minute.parse().ok()?),
        Some(_) => return None,
        None => (time, 0),
    };
    let hour: i8 = hour.parse().ok()?;
    if !(1..=12).contains(&hour) || !(0..=59).contains(&minute) {
        return None;
    }
    Some((hour % 12 + if pm { 12 } else { 0 }, minute))
}

/// The words for when a limit that lifts at `at` lifts, as seen at `now`:
/// `15:00` within a day, `Nov 20 09:00` past that, and nothing once it has
/// passed.
pub fn resumes_words(at: u64, now: u64, local: &TimeZone) -> Option<String> {
    if at <= now {
        return None;
    }
    let zoned = Timestamp::from_second(i64::try_from(at).ok()?)
        .ok()?
        .to_zoned(local.clone());
    let format = match at - now < 86_400 {
        true => "%H:%M",
        false => "%b %-d %H:%M",
    };
    Some(zoned.strftime(format).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::{DateTime, date};

    fn dhaka() -> TimeZone {
        TimeZone::get("Asia/Dhaka").unwrap()
    }

    fn new_york() -> TimeZone {
        TimeZone::posix("EST5EDT,M3.2.0,M11.1.0").unwrap()
    }

    fn at(dt: DateTime, zone: &TimeZone) -> u64 {
        dt.to_zoned(zone.clone()).unwrap().timestamp().as_second() as u64
    }

    /// The reset `text` names, read at `now` in Dhaka, as Dhaka's clock.
    fn reset(text: &str, now: DateTime) -> Option<DateTime> {
        let secs = resets_at(text, at(now, &dhaka()), &dhaka())?;
        Some(
            Timestamp::from_second(secs as i64)
                .unwrap()
                .to_zoned(dhaka())
                .datetime(),
        )
    }

    #[test]
    fn a_bare_hour_is_its_next_time_today() {
        let now = date(2026, 11, 16).at(11, 20, 0, 0);
        assert_eq!(
            reset("You have hit your limit · resets 3pm", now),
            Some(date(2026, 11, 16).at(15, 0, 0, 0))
        );
        assert_eq!(
            reset("You've hit your limit · resets 10:30am", now),
            Some(date(2026, 11, 17).at(10, 30, 0, 0)),
            "an hour already past today is tomorrow's"
        );
        assert_eq!(
            reset("resets 12am", now),
            Some(date(2026, 11, 17).at(0, 0, 0, 0))
        );
        assert_eq!(
            reset("resets 12pm", now),
            Some(date(2026, 11, 16).at(12, 0, 0, 0))
        );
    }

    #[test]
    fn a_named_zone_is_read_in_that_zone() {
        let now = date(2026, 11, 16).at(8, 0, 0, 0);
        assert_eq!(
            reset("You've hit your limit · resets 10am (Asia/Dhaka)", now),
            Some(date(2026, 11, 16).at(10, 0, 0, 0))
        );
        // 10am in Tokyo is 7am in Dhaka, already past at 8am, so tomorrow's.
        assert_eq!(
            reset("resets 10am (Asia/Tokyo)", now),
            Some(date(2026, 11, 17).at(7, 0, 0, 0))
        );
        // A zone this machine cannot name gives nothing, not the local hour.
        assert_eq!(reset("resets 10am (Mars/Olympus)", now), None);
    }

    #[test]
    fn a_date_is_that_day_and_never_today() {
        let now = date(2026, 11, 16).at(8, 0, 0, 0);
        assert_eq!(
            reset("You've hit your weekly limit · resets Nov 20 at 9am", now),
            Some(date(2026, 11, 20).at(9, 0, 0, 0))
        );
        assert_eq!(
            reset(
                "You've hit your weekly limit · resets Aug 24, 10am (Asia/Dhaka)",
                date(2026, 8, 20).at(8, 0, 0, 0)
            ),
            Some(date(2026, 8, 24).at(10, 0, 0, 0))
        );
        // Said in late December, a January date is next year's.
        assert_eq!(
            reset("resets Jan 2 at 9am", date(2026, 12, 30).at(8, 0, 0, 0)),
            Some(date(2027, 1, 2).at(9, 0, 0, 0))
        );
    }

    #[test]
    fn words_that_do_not_read_whole_give_nothing() {
        let now = date(2026, 11, 16).at(8, 0, 0, 0);
        for text in [
            "You have hit your limit",
            "resets soon",
            "resets 3",
            "resets 13pm",
            "resets 3:75pm",
            "resets Nov 31 at 9am",
            "resets Novem 20 at 9am",
            "resets Nov 20 at 9",
            "resets 3pm tomorrow",
            "resets 10am (Asia/Dhaka",
            "",
        ] {
            assert_eq!(reset(text, now), None, "{text:?}");
        }
    }

    #[test]
    fn a_line_after_the_reset_is_not_part_of_it() {
        let now = date(2026, 11, 16).at(8, 0, 0, 0);
        assert_eq!(
            reset(
                "You've hit your limit · resets 3pm\n/upgrade to increase your usage limit.",
                now
            ),
            Some(date(2026, 11, 16).at(15, 0, 0, 0))
        );
        assert_eq!(
            reset("You've hit your limit · resets 3pm · /upgrade", now),
            Some(date(2026, 11, 16).at(15, 0, 0, 0))
        );
    }

    #[test]
    fn the_resume_reads_as_a_clock_within_a_day_and_a_date_past_it() {
        let zone = new_york();
        let now = at(date(2026, 11, 16).at(11, 20, 0, 0), &zone);
        let words = |dt: DateTime| resumes_words(at(dt, &zone), now, &zone);
        assert_eq!(
            words(date(2026, 11, 16).at(15, 0, 0, 0)).as_deref(),
            Some("15:00")
        );
        assert_eq!(
            words(date(2026, 11, 17).at(9, 5, 0, 0)).as_deref(),
            Some("09:05")
        );
        assert_eq!(
            words(date(2026, 11, 20).at(9, 0, 0, 0)).as_deref(),
            Some("Nov 20 09:00")
        );
        assert_eq!(words(date(2026, 11, 16).at(11, 20, 0, 0)), None);
        assert_eq!(words(date(2026, 11, 16).at(9, 0, 0, 0)), None);
    }
}
