//! Timestamps. Tessera stores milliseconds since the Unix epoch and formats them the way Jira does.

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Timestamp, Zoned};

/// The current time in milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    Timestamp::now().as_millisecond()
}

fn zoned(ms: i64) -> Zoned {
    Timestamp::from_millisecond(ms)
        .unwrap_or(Timestamp::UNIX_EPOCH)
        .to_zoned(TimeZone::UTC)
}

/// Formats a timestamp like Jira: `2026-10-13T14:43:15.245+0000`.
pub fn format(ms: i64) -> String {
    zoned(ms).strftime("%Y-%m-%dT%H:%M:%S%.3f%z").to_string()
}

/// Parses a Jira date (`2026-10-13`) used by fields such as `duedate`.
pub fn parse_date(text: &str) -> Option<Date> {
    Date::strptime("%Y-%m-%d", text).ok()
}

/// Parses an absolute date or date-time as JQL accepts them:
/// `yyyy-MM-dd`, `yyyy/MM/dd`, and either one followed by ` HH:mm`. Times are UTC.
pub fn parse_jql_datetime(text: &str) -> Option<i64> {
    let normalized = text.trim().replace('/', "-");
    let (date, time) = match normalized.split_once(' ') {
        Some((date, time)) => (date, Some(time)),
        None => (normalized.as_str(), None),
    };
    let date = parse_date(date)?;
    let (hour, minute) = match time {
        Some(time) => {
            let (h, m) = time.split_once(':')?;
            (h.parse().ok()?, m.parse().ok()?)
        }
        None => (0, 0),
    };
    let datetime = date.at(hour, minute, 0, 0);
    Some(
        datetime
            .to_zoned(TimeZone::UTC)
            .ok()?
            .timestamp()
            .as_millisecond(),
    )
}

/// Parses a relative JQL time such as `-7d`, `2w`, `-4h`, or `-30m` against `now`.
/// A combined form like `-1w 2d` is also accepted.
pub fn parse_relative(text: &str, now: i64) -> Option<i64> {
    let mut total: i64 = 0;
    let mut any = false;
    for part in text.split_whitespace() {
        let (sign, rest) = match part.strip_prefix('-') {
            Some(rest) => (-1, rest),
            None => (1, part.strip_prefix('+').unwrap_or(part)),
        };
        let unit = rest.chars().last()?;
        let amount: i64 = rest[..rest.len() - 1].parse().ok()?;
        let unit_ms = match unit {
            'w' => 7 * 24 * 3_600_000,
            'd' => 24 * 3_600_000,
            'h' => 3_600_000,
            'm' => 60_000,
            _ => return None,
        };
        total += sign * amount * unit_ms;
        any = true;
    }
    any.then_some(now + total)
}

/// The start of a calendar unit (`day`, `week`, `month`, `year`) that contains `now`, in UTC,
/// shifted by `offset` units. Weeks start on Sunday, as in Jira's default locale.
pub fn start_of(unit: &str, now: i64, offset: i64) -> Option<i64> {
    let today = zoned(now).date();
    let start = match unit {
        "day" => today.checked_add(jiff::Span::new().days(offset)).ok()?,
        "week" => {
            let back = today.weekday().to_sunday_zero_offset() as i64;
            today
                .checked_add(jiff::Span::new().days(offset * 7 - back))
                .ok()?
        }
        "month" => today
            .first_of_month()
            .checked_add(jiff::Span::new().months(offset))
            .ok()?,
        "year" => Date::new(today.year(), 1, 1)
            .ok()?
            .checked_add(jiff::Span::new().years(offset))
            .ok()?,
        _ => return None,
    };
    Some(
        start
            .to_zoned(TimeZone::UTC)
            .ok()?
            .timestamp()
            .as_millisecond(),
    )
}

/// The last millisecond of a calendar unit that contains `now`, shifted by `offset` units.
pub fn end_of(unit: &str, now: i64, offset: i64) -> Option<i64> {
    start_of(unit, now, offset + 1).map(|next| next - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-10-13 (a Tuesday) 14:43:15.245 UTC.
    const NOW: i64 = 1_791_902_595_245;

    #[test]
    fn formats_like_jira() {
        assert_eq!(format(NOW), "2026-10-13T14:43:15.245+0000");
    }

    #[test]
    fn parses_absolute_jql_dates() {
        let midnight = parse_jql_datetime("2026-10-13").unwrap();
        assert_eq!(format(midnight), "2026-10-13T00:00:00.000+0000");
        assert_eq!(parse_jql_datetime("2026/10/13"), Some(midnight));
        assert_eq!(
            format(parse_jql_datetime("2026/10/13 09:30").unwrap()),
            "2026-10-13T09:30:00.000+0000"
        );
        assert_eq!(parse_jql_datetime("13-10-2026"), None);
    }

    #[test]
    fn parses_relative_times() {
        assert_eq!(parse_relative("-1d", NOW), Some(NOW - 86_400_000));
        assert_eq!(parse_relative("2h", NOW), Some(NOW + 7_200_000));
        assert_eq!(
            parse_relative("-1w 2d", NOW),
            Some(NOW - 7 * 86_400_000 + 2 * 86_400_000)
        );
        assert_eq!(parse_relative("-1x", NOW), None);
    }

    #[test]
    fn calendar_boundaries() {
        let day = |ms| format(ms)[..10].to_string();
        assert_eq!(
            format(start_of("day", NOW, 0).unwrap()),
            "2026-10-13T00:00:00.000+0000"
        );
        assert_eq!(
            format(end_of("day", NOW, 0).unwrap()),
            "2026-10-13T23:59:59.999+0000"
        );
        assert_eq!(day(start_of("week", NOW, 0).unwrap()), "2026-10-11");
        assert_eq!(day(start_of("month", NOW, -1).unwrap()), "2026-09-01");
        assert_eq!(day(start_of("year", NOW, 0).unwrap()), "2026-01-01");
    }
}
