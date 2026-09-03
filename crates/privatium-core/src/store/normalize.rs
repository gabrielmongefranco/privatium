// Project:  Privatium™  |  File: crates/privatium-core/src/store/normalize.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-03
// Summary:  Typed writes (spec/data-dictionary.md §2.1, spec/lua-api.md §3.3): the values of
//           a `d` about to be appended, checked and normalized against the column each
//           names — BIGINT and DECIMAL as digit strings at the declared scale, BOOLEAN as
//           true/false, and DATE, TIME and TIMESTAMPTZ parsed from the forms people type
//           into the ISO spelling the cache stores. A value that is not its type refuses
//           the append before anything is written, so the log stays clean and nothing has
//           to materialize as NULL later.

use serde_json::{Map, Value};

use crate::store::Decimal;
use crate::store::schema::{Column, Kind, Table};

/// Which way round `3/9/2026` reads when neither number is past twelve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DateOrder {
    /// `3/9/2026` is the ninth of March — `ui.date_format = "us"`, and the default.
    #[default]
    MonthFirst,
    /// `3/9/2026` is the third of September — `ui.date_format = "eu"`.
    DayFirst,
}

impl DateOrder {
    /// From `ui.date_format` (`spec/data-dictionary.md §3.6`): `eu` reads the day first;
    /// `iso` and `us` — and anything unrecognised — the month first.
    #[must_use]
    pub fn from_setting(format: &str) -> Self {
        if format.trim().eq_ignore_ascii_case("eu") {
            Self::DayFirst
        } else {
            Self::MonthFirst
        }
    }
}

/// A value the append was refused for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    /// The column.
    pub column: String,
    /// What was wrong, naming the value and the spellings that would have worked.
    pub problem: String,
}

/// Normalize every value of `d` that names a declared column, in place. Keys no column
/// matches pass through untouched (`spec/protocol.md §4.5`); a JSON `null` is left alone,
/// since it is the same as an absent key (`§2.1`).
pub fn normalize_row(
    table: &Table,
    d: &mut Map<String, Value>,
    order: DateOrder,
) -> Result<(), Refused> {
    for column in &table.columns {
        let Some(value) = d.get_mut(&column.name) else {
            continue;
        };
        if value.is_null() {
            continue;
        }
        let normalized = normalize_value(column, value, order).map_err(|problem| Refused {
            column: column.name.clone(),
            problem,
        })?;
        *value = normalized;
    }
    Ok(())
}

fn normalize_value(column: &Column, value: &Value, order: DateOrder) -> Result<Value, String> {
    let declared = column.ty.trim().to_ascii_uppercase();
    match column.kind {
        Kind::Integer => integer(value).map(Value::String),
        Kind::Decimal { scale } => decimal(value, scale).map(Value::String),
        Kind::Boolean => boolean(value).map(Value::Bool),
        Kind::Json => Ok(value.clone()),
        Kind::Text if declared.starts_with("TIMESTAMP") || declared.starts_with("DATETIME") => {
            timestamp(value, order).map(Value::String)
        }
        Kind::Text if declared.starts_with("DATE") => date(value, order).map(Value::String),
        Kind::Text if declared.starts_with("TIME") => time(value).map(Value::String),
        Kind::Text => Ok(value.clone()),
    }
}

/// `BIGINT`: a JSON integer or a string of digits, as the digits (`§2.1`).
fn integer(value: &Value) -> Result<String, String> {
    match value {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                return Ok(i.to_string());
            }
            if let Some(u) = n.as_u64() {
                return Ok(u.to_string());
            }
            Err(format!("{n} is not an integer; a BIGINT is a whole number"))
        }
        Value::String(text) => text
            .trim()
            .parse::<i64>()
            .map(|i| i.to_string())
            .map_err(|_| format!("{text:?} is not an integer; a BIGINT is a whole number")),
        other => Err(format!(
            "{other} is not an integer; a BIGINT is a whole number"
        )),
    }
}

/// `DECIMAL(p,s)`: a string, an integer or a number, as digits at the declared scale. More
/// fractional digits than the scale holds is refused rather than rounded: the author
/// wrote a value the column cannot keep, and silently rounding money is how cents go
/// missing.
fn decimal(value: &Value, scale: u8) -> Result<String, String> {
    let text = match value {
        Value::String(text) => text.trim().to_owned(),
        // serde prints the shortest round-trip form, so a literal `12.5` arrives as
        // `12.5`; a computed float that is not exact arrives with its noise and is refused
        // below for having too many places, which is the right answer for it.
        Value::Number(n) => n.to_string(),
        other => return Err(format!("{other} is not a decimal")),
    };
    let parsed = Decimal::parse(&text)
        .ok_or_else(|| format!("{text:?} is not a decimal ([-]digits[.digits])"))?;
    let scaled = parsed.with_scale(scale);
    if scaled.compare(parsed) != std::cmp::Ordering::Equal {
        return Err(format!(
            "{text:?} has more than {scale} decimal place(s), which this column cannot hold"
        ));
    }
    Ok(scaled.to_string())
}

/// `BOOLEAN`: `true`/`false`, the words people type, or `1`/`0`.
fn boolean(value: &Value) -> Result<bool, String> {
    match value {
        Value::Bool(b) => Ok(*b),
        Value::Number(n) => match n.as_i64() {
            Some(1) => Ok(true),
            Some(0) => Ok(false),
            _ => Err(format!("{n} is not a boolean; write true or false")),
        },
        Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" | "y" => Ok(true),
            "false" | "0" | "no" | "off" | "n" | "" => Ok(false),
            _ => Err(format!("{text:?} is not a boolean; write true or false")),
        },
        other => Err(format!("{other} is not a boolean; write true or false")),
    }
}

/// `DATE`: any of the spellings [`parse_date`] accepts, as `YYYY-MM-DD`.
fn date(value: &Value, order: DateOrder) -> Result<String, String> {
    let parsed = match value {
        Value::String(text) => parse_date(text, order),
        Value::Number(n) => n
            .as_i64()
            .and_then(epoch)
            .map(|ts| ts.to_zoned(utc()).date()),
        _ => None,
    };
    parsed.map(|d| d.to_string()).ok_or_else(|| {
        format!("{value} is not a date; write 2026-09-03, 3/9/2026, 3 September 2026 or 09-SEP-26")
    })
}

/// `TIME`: `HH:MM`, `HH:MM:SS`, or a 12-hour clock with am/pm, as `HH:MM:SS`.
fn time(value: &Value) -> Result<String, String> {
    let Value::String(text) = value else {
        return Err(format!(
            "{value} is not a time; write 14:03, 14:03:11 or 2:03 pm"
        ));
    };
    parse_time(text)
        .map(|t| format!("{:02}:{:02}:{:02}", t.hour(), t.minute(), t.second()))
        .ok_or_else(|| format!("{text:?} is not a time; write 14:03, 14:03:11 or 2:03 pm"))
}

/// `TIMESTAMPTZ`: RFC 3339 with `Z` or an offset, a date and time without a zone (read as
/// UTC), a date alone (midnight UTC), or an epoch — as RFC 3339 UTC to the millisecond,
/// the log's own `ts` spelling.
fn timestamp(value: &Value, order: DateOrder) -> Result<String, String> {
    let parsed = match value {
        Value::String(text) => parse_timestamp(text, order),
        Value::Number(n) => n.as_i64().and_then(epoch),
        _ => None,
    };
    parsed.map(crate::log::format_ts).ok_or_else(|| {
        format!(
            "{value} is not a timestamp; write 2026-09-03T14:03:11Z, 2026-09-03 14:03, \
                 or a date"
        )
    })
}

fn utc() -> jiff::tz::TimeZone {
    jiff::tz::TimeZone::UTC
}

/// An epoch, in seconds (ten digits) or milliseconds (thirteen).
fn epoch(n: i64) -> Option<jiff::Timestamp> {
    let digits = n.unsigned_abs().to_string().len();
    match digits {
        10 => jiff::Timestamp::from_second(n).ok(),
        13 => jiff::Timestamp::from_millisecond(n).ok(),
        _ => None,
    }
}

/// Every date spelling accepted on write. Returns `None` for anything else.
#[must_use]
pub fn parse_date(text: &str, order: DateOrder) -> Option<jiff::civil::Date> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    // `YYYY-MM-DD`, alone or as the start of a timestamp.
    if let Some(date) = iso_date(text) {
        return Some(date);
    }
    // All digits: `YYYYMMDD`, or an epoch.
    if text.bytes().all(|b| b.is_ascii_digit()) {
        return match text.len() {
            8 => ymd(&text[..4], &text[4..6], &text[6..]),
            10 | 13 => text
                .parse::<i64>()
                .ok()
                .and_then(epoch)
                .map(|ts| ts.to_zoned(utc()).date()),
            _ => None,
        };
    }
    // Three numbers with one kind of separator: `2026/09/03`, `3/9/2026`, `3-9-26`,
    // `03.09.2026`.
    let numeric: Vec<&str> = text
        .split(['/', '-', '.'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    if numeric.len() == 3
        && numeric
            .iter()
            .all(|p| p.bytes().all(|b| b.is_ascii_digit()))
    {
        if numeric[0].len() == 4 {
            return ymd(numeric[0], numeric[1], numeric[2]);
        }
        let (a, b) = (
            numeric[0].parse::<u32>().ok()?,
            numeric[1].parse::<u32>().ok()?,
        );
        let year = expand_year(numeric[2])?;
        let (month, day) = if a > 12 {
            (b, a)
        } else if b > 12 {
            (a, b)
        } else {
            match order {
                DateOrder::MonthFirst => (a, b),
                DateOrder::DayFirst => (b, a),
            }
        };
        return civil(year, month, day);
    }
    // A month by name: `March 9, 2026`, `9 March 2026`, `Mar 9 2026`, `9-Mar-2026`,
    // `09-MAR-26`.
    let tokens: Vec<&str> = text
        .split([' ', ',', '-', '.', '/'])
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.len() == 3 {
        let mut month = None;
        let mut numbers = Vec::new();
        for token in &tokens {
            if token.bytes().all(|b| b.is_ascii_digit()) {
                numbers.push(*token);
            } else if let Some(m) = month_named(token) {
                month = Some(m);
            } else {
                return None;
            }
        }
        if let (Some(month), [first, second]) = (month, numbers.as_slice()) {
            // The year is the four-digit number, or the last one.
            let (day, year) = if first.len() == 4 {
                (*second, *first)
            } else {
                (*first, *second)
            };
            return civil(expand_year(year)?, month, day.parse().ok()?);
        }
    }
    None
}

fn iso_date(text: &str) -> Option<jiff::civil::Date> {
    let bytes = text.as_bytes();
    if bytes.len() < 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    if bytes.len() > 10 && !matches!(bytes[10], b'T' | b't' | b' ') {
        return None;
    }
    ymd(&text[..4], &text[5..7], &text[8..10])
}

fn ymd(year: &str, month: &str, day: &str) -> Option<jiff::civil::Date> {
    civil(year.parse().ok()?, month.parse().ok()?, day.parse().ok()?)
}

fn civil(year: i32, month: u32, day: u32) -> Option<jiff::civil::Date> {
    jiff::civil::Date::new(
        i16::try_from(year).ok()?,
        i8::try_from(month).ok()?,
        i8::try_from(day).ok()?,
    )
    .ok()
}

/// A two-digit year: `00`–`69` is this century, `70`–`99` the last. Four digits stand.
fn expand_year(text: &str) -> Option<i32> {
    let year: i32 = text.parse().ok()?;
    Some(match text.len() {
        2 => {
            if year <= 69 {
                2000 + year
            } else {
                1900 + year
            }
        }
        4 => year,
        _ => return None,
    })
}

/// The month a token names, by its first three letters.
fn month_named(token: &str) -> Option<u32> {
    let lower = token.to_ascii_lowercase();
    if lower.len() < 3 || !lower.bytes().all(|b| b.is_ascii_lowercase()) {
        return None;
    }
    const MONTHS: [&str; 12] = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    MONTHS
        .iter()
        .position(|name| {
            name.starts_with(&lower)
                || lower.starts_with(&name[..3])
                    && lower.len() <= name.len()
                    && name.starts_with(&lower)
        })
        .map(|i| i as u32 + 1)
}

/// `HH:MM[:SS[.fff]]` on a 24-hour clock, or `H[:MM[:SS]] am|pm`.
#[must_use]
pub fn parse_time(text: &str) -> Option<jiff::civil::Time> {
    let lower = text.trim().to_ascii_lowercase();
    let (clock, meridiem) = if let Some(rest) = lower
        .strip_suffix("pm")
        .or_else(|| lower.strip_suffix("p.m."))
    {
        (rest.trim(), Some(true))
    } else if let Some(rest) = lower
        .strip_suffix("am")
        .or_else(|| lower.strip_suffix("a.m."))
    {
        (rest.trim(), Some(false))
    } else {
        (lower.as_str(), None)
    };
    let mut parts = clock.split(':');
    let hour: u32 = parts.next()?.trim().parse().ok()?;
    let minute: u32 = match parts.next() {
        Some(m) => m.trim().parse().ok()?,
        None if meridiem.is_some() => 0,
        None => return None,
    };
    let second: u32 = match parts.next() {
        Some(s) => s.trim().split('.').next()?.parse().ok()?,
        None => 0,
    };
    if parts.next().is_some() {
        return None;
    }
    let hour = match meridiem {
        Some(_) if !(1..=12).contains(&hour) => return None,
        Some(true) => {
            if hour == 12 {
                12
            } else {
                hour + 12
            }
        }
        Some(false) => {
            if hour == 12 {
                0
            } else {
                hour
            }
        }
        None => hour,
    };
    jiff::civil::Time::new(
        i8::try_from(hour).ok()?,
        i8::try_from(minute).ok()?,
        i8::try_from(second).ok()?,
        0,
    )
    .ok()
}

/// Every timestamp spelling accepted on write.
#[must_use]
pub fn parse_timestamp(text: &str, order: DateOrder) -> Option<jiff::Timestamp> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    // RFC 3339 with a zone: `2026-09-03T14:03:11Z`, `2026-09-03T16:03:11+02:00`.
    if let Ok(ts) = text.parse::<jiff::Timestamp>() {
        return Some(ts);
    }
    // ISO without a zone, `T` or a space: read as UTC.
    let iso = text.replacen(' ', "T", 1);
    if let Ok(dt) = iso.parse::<jiff::civil::DateTime>() {
        return dt.to_zoned(utc()).ok().map(|z| z.timestamp());
    }
    // An epoch.
    if text.bytes().all(|b| b.is_ascii_digit()) && matches!(text.len(), 10 | 13) {
        return text.parse::<i64>().ok().and_then(epoch);
    }
    // A date alone: midnight UTC.
    if let Some(date) = parse_date(text, order) {
        return date.to_zoned(utc()).ok().map(|z| z.timestamp());
    }
    // A date, a space, a time — `3/9/2026 2:30 pm`. Try every split.
    for (index, _) in text.match_indices(' ') {
        if let (Some(date), Some(time)) = (
            parse_date(&text[..index], order),
            parse_time(&text[index + 1..]),
        ) {
            return date
                .to_datetime(time)
                .to_zoned(utc())
                .ok()
                .map(|z| z.timestamp());
        }
    }
    None
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    fn d(text: &str, order: DateOrder) -> String {
        parse_date(text, order)
            .map(|d| d.to_string())
            .unwrap_or_else(|| "none".to_owned())
    }

    #[test]
    fn dates_in_the_forms_people_type() {
        let us = DateOrder::MonthFirst;
        let eu = DateOrder::DayFirst;
        assert_eq!(d("2026-09-03", us), "2026-09-03");
        assert_eq!(d("2026-09-03T14:03:11.412Z", us), "2026-09-03");
        assert_eq!(d("2026/09/03", us), "2026-09-03");
        assert_eq!(d("20260903", us), "2026-09-03");
        assert_eq!(d("3/9/2026", us), "2026-03-09");
        assert_eq!(d("3/9/2026", eu), "2026-09-03");
        assert_eq!(d("31/12/2026", us), "2026-12-31", "the range decides");
        assert_eq!(d("12/31/2026", eu), "2026-12-31");
        assert_eq!(d("3-9-26", us), "2026-03-09");
        assert_eq!(d("03.09.1985", eu), "1985-09-03");
        assert_eq!(d("3/9/85", us), "1985-03-09", "70-99 is the last century");
        assert_eq!(d("March 9, 2026", us), "2026-03-09");
        assert_eq!(d("9 March 2026", us), "2026-03-09");
        assert_eq!(d("Mar 9 2026", us), "2026-03-09");
        assert_eq!(d("9-Mar-2026", us), "2026-03-09");
        assert_eq!(d("09-SEP-26", us), "2026-09-09");
        assert_eq!(d("Sept 9, 2026", us), "2026-09-09");
        assert_eq!(d("1756000000", us), "2025-08-24", "an epoch in seconds");
        for bad in [
            "",
            "yesterday",
            "2026-13-01",
            "31/31/2026",
            "Marchember 9 2026",
            "3/9",
            "abc",
        ] {
            assert_eq!(d(bad, us), "none", "{bad}");
        }
    }

    #[test]
    fn times_on_both_clocks() {
        let t = |text: &str| {
            parse_time(text)
                .map(|t| format!("{:02}:{:02}:{:02}", t.hour(), t.minute(), t.second()))
                .unwrap_or_else(|| "none".to_owned())
        };
        assert_eq!(t("14:03"), "14:03:00");
        assert_eq!(t("14:03:11"), "14:03:11");
        assert_eq!(t("14:03:11.5"), "14:03:11");
        assert_eq!(t("2:03 pm"), "14:03:00");
        assert_eq!(t("2:03PM"), "14:03:00");
        assert_eq!(t("12:30 am"), "00:30:00");
        assert_eq!(t("12 pm"), "12:00:00");
        assert_eq!(t("9am"), "09:00:00");
        for bad in ["", "25:00", "14", "13 pm", "1:2:3:4", "noon"] {
            assert_eq!(t(bad), "none", "{bad}");
        }
    }

    #[test]
    fn timestamps_land_in_utc_to_the_millisecond() {
        let ts = |text: &str| {
            parse_timestamp(text, DateOrder::MonthFirst)
                .map(crate::log::format_ts)
                .unwrap_or_else(|| "none".to_owned())
        };
        assert_eq!(ts("2026-09-03T14:03:11.412Z"), "2026-09-03T14:03:11.412Z");
        assert_eq!(ts("2026-09-03T16:03:11+02:00"), "2026-09-03T14:03:11.000Z");
        assert_eq!(ts("2026-09-03 14:03"), "2026-09-03T14:03:00.000Z");
        assert_eq!(ts("2026-09-03T14:03:11"), "2026-09-03T14:03:11.000Z");
        assert_eq!(ts("2026-09-03"), "2026-09-03T00:00:00.000Z");
        assert_eq!(ts("3/9/2026 2:30 pm"), "2026-03-09T14:30:00.000Z");
        assert_eq!(ts("1756000000"), "2025-08-24T01:46:40.000Z");
        assert_eq!(ts("1756000000412"), "2025-08-24T01:46:40.412Z");
        assert_eq!(ts("soon"), "none");
    }

    fn column(name: &str, ty: &str) -> Column {
        Column {
            name: name.to_owned(),
            ty: ty.to_owned(),
            kind: Kind::of(ty),
            not_null: false,
        }
    }

    #[test]
    fn a_row_is_normalized_column_by_column_and_refused_by_name() {
        let table = Table {
            name: "thing".to_owned(),
            columns: vec![
                column("name", "VARCHAR"),
                column("amount", "DECIMAL(18,2)"),
                column("big", "BIGINT"),
                column("ok", "BOOLEAN"),
                column("tags", "VARCHAR[]"),
                column("filled_on", "DATE"),
                column("seen_at", "TIMESTAMPTZ"),
                column("at_time", "TIME"),
            ],
        };
        let mut d: Map<String, Value> = serde_json::from_str(
            r#"{"name":"a","amount":12.5,"big":42,"ok":"yes","tags":["x"],"filled_on":"3/9/2026","seen_at":"2026-09-03 14:03","at_time":"2:30 pm","extra":"kept","nothing":null}"#,
        )
        .unwrap();
        normalize_row(&table, &mut d, DateOrder::MonthFirst).unwrap();
        assert_eq!(d["amount"], "12.50");
        assert_eq!(d["big"], "42");
        assert_eq!(d["ok"], true);
        assert_eq!(d["tags"], serde_json::json!(["x"]));
        assert_eq!(d["filled_on"], "2026-03-09");
        assert_eq!(d["seen_at"], "2026-09-03T14:03:00.000Z");
        assert_eq!(d["at_time"], "14:30:00");
        assert_eq!(d["extra"], "kept", "an undeclared key passes through");
        assert_eq!(d["nothing"], Value::Null);

        for (key, value, expected) in [
            ("amount", serde_json::json!("abc"), "not a decimal"),
            (
                "amount",
                serde_json::json!("12.345"),
                "more than 2 decimal place",
            ),
            (
                "amount",
                serde_json::json!(0.1 + 0.2),
                "more than 2 decimal place",
            ),
            ("big", serde_json::json!("1.5"), "not an integer"),
            ("big", serde_json::json!(true), "not an integer"),
            ("ok", serde_json::json!("maybe"), "not a boolean"),
            ("filled_on", serde_json::json!("yesterday"), "not a date"),
            ("seen_at", serde_json::json!("soon"), "not a timestamp"),
            ("at_time", serde_json::json!("noon"), "not a time"),
        ] {
            let mut d = Map::new();
            d.insert(key.to_owned(), value.clone());
            let refused = normalize_row(&table, &mut d, DateOrder::MonthFirst).unwrap_err();
            assert_eq!(refused.column, key, "{value}");
            assert!(
                refused.problem.contains(expected),
                "{key} {value}: {}",
                refused.problem
            );
        }
        // Exact trailing zeros past the scale are fine; only lost digits are refused.
        let mut d = Map::new();
        d.insert("amount".to_owned(), serde_json::json!("12.300"));
        normalize_row(&table, &mut d, DateOrder::MonthFirst).unwrap();
        assert_eq!(d["amount"], "12.30");
    }
}
