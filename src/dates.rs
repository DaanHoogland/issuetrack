use anyhow::{Context, Result};
use anyhow::{anyhow, bail};
use chrono::{Datelike, Duration, NaiveDate, Weekday};

use crate::cli::PeriodOverride;

pub fn parse_date(text: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .with_context(|| format!("invalid date '{text}', expected YYYY-MM-DD"))
}

/// 2-digit years mean this century: 21 -> 2021. Others are taken as-is.
fn expand_two_digit_year(year: i64) -> i32 {
    if (0..100).contains(&year) {
        2000 + year as i32
    } else {
        year as i32
    }
}

/// Parses yyyy/mm/dd or yy/mm/dd.
pub fn parse_slash_date(text: &str) -> Result<NaiveDate> {
    let invalid = || anyhow!("invalid date '{text}', expected yyyy/mm/dd or yy/mm/dd");
    let parts: Vec<&str> = text.split('/').collect();
    let [year_part, month_part, day_part] = parts[..] else {
        return Err(invalid());
    };
    if ![1, 2, 4].contains(&year_part.len()) {
        return Err(invalid());
    }
    let year = expand_two_digit_year(year_part.parse().map_err(|_| invalid())?);
    let month: u32 = month_part.parse().map_err(|_| invalid())?;
    let day: u32 = day_part.parse().map_err(|_| invalid())?;
    NaiveDate::from_ymd_opt(year, month, day).ok_or_else(invalid)
}

/// The `--since`/`--until` override when given, else `default_period(today)`.
pub fn resolve_period(
    over: PeriodOverride,
    today: NaiveDate,
    default_period: impl Fn(NaiveDate) -> Result<(NaiveDate, NaiveDate)>,
) -> Result<(NaiveDate, NaiveDate)> {
    match (over.since, over.until) {
        (Some(since), Some(until)) => Ok((parse_date(&since)?, parse_date(&until)?)),
        _ => default_period(today),
    }
}

fn days_since_last(weekday: Weekday, date: NaiveDate) -> i64 {
    (date.weekday().num_days_from_monday() as i64 - weekday.num_days_from_monday() as i64)
        .rem_euclid(7)
}

/// Most recently completed Wednesday-through-Tuesday window, ending on the
/// last Tuesday strictly before `today`.
pub fn last_completed_week(today: NaiveDate) -> (NaiveDate, NaiveDate) {
    let yesterday = today - Duration::days(1);
    let end = yesterday - Duration::days(days_since_last(Weekday::Tue, yesterday));
    (end - Duration::days(6), end)
}

/// The Wednesday-through-Tuesday week that contains `date`.
pub fn week_containing(date: NaiveDate) -> (NaiveDate, NaiveDate) {
    let begin = date - Duration::days(days_since_last(Weekday::Wed, date));
    (begin, begin + Duration::days(6))
}

pub fn last_day_of_month(year: i32, month: u32) -> NaiveDate {
    let (year_after, month_after) = next_month(year, month);
    NaiveDate::from_ymd_opt(year_after, month_after, 1).expect("valid first-of-month date")
        - Duration::days(1)
}

fn month_months_back(today: NaiveDate, months_back: u32) -> (NaiveDate, NaiveDate) {
    let months_since_year_zero =
        today.year() as i64 * 12 + (today.month() as i64 - 1) - months_back as i64;
    let year = months_since_year_zero.div_euclid(12) as i32;
    let month = (months_since_year_zero.rem_euclid(12) + 1) as u32;
    month_containing(NaiveDate::from_ymd_opt(year, month, 1).expect("valid first-of-month date"))
}

/// Resolves the `monthly` command's positional arguments:
/// - none: the current month.
/// - a number: negative = that many months back, positive = that year
///   (keeping the current month).
/// - a month name/abbreviation: that month this year.
/// - a month name and a year, in either order.
pub fn resolve_month(
    today: NaiveDate,
    first: Option<String>,
    second: Option<String>,
) -> Result<(NaiveDate, NaiveDate)> {
    match (first, second) {
        (None, None) => Ok(month_months_back(today, 0)),
        (Some(only), None) => match only.parse::<i64>() {
            Ok(months_back) if months_back <= 0 => {
                Ok(month_months_back(today, months_back.unsigned_abs() as u32))
            }
            Ok(year) => month_in_year(expand_two_digit_year(year), today.month()),
            Err(_) => month_in_year(today.year(), parse_month_name(&only)?),
        },
        (Some(first), Some(second)) => {
            let (month_text, year_text) = if parse_month_name(&first).is_ok() {
                (first, second)
            } else if parse_month_name(&second).is_ok() {
                (second, first)
            } else {
                bail!(
                    "give a month name/abbreviation and a year, e.g. `monthly aug 2021` \
                     or `monthly 2021 aug`"
                );
            };
            let year: i64 = year_text
                .parse()
                .with_context(|| format!("'{year_text}' is not a valid year"))?;
            month_in_year(expand_two_digit_year(year), parse_month_name(&month_text)?)
        }
        (None, Some(_)) => bail!("give a month and/or year"),
    }
}

/// Resolves the top-level implicit shortcut (no subcommand given): a month
/// name/abbreviation and/or a year, in either order, dispatch to the same
/// logic as `monthly`/`yearly` respectively.
pub fn resolve_implicit(
    today: NaiveDate,
    first: Option<String>,
    second: Option<String>,
) -> Result<(NaiveDate, NaiveDate)> {
    if first.is_none() && second.is_none() {
        bail!(
            "no command given — run with --help, or provide a month and/or year \
             (e.g. `issuetrack aug 2021` or `issuetrack 2021`)"
        );
    }
    let has_month = [first.as_deref(), second.as_deref()]
        .into_iter()
        .flatten()
        .any(|argument| parse_month_name(argument).is_ok());
    if has_month {
        return resolve_month(today, first, second);
    }
    if second.is_some() {
        bail!(
            "give a month name/abbreviation and a year, or use an explicit subcommand \
             (monthly/yearly) for other combinations"
        );
    }
    let only = first.expect("checked above");
    let year_or_years_back: i64 = only
        .parse()
        .with_context(|| format!("'{only}' is not a recognized month name, year, or offset"))?;
    Ok(resolve_year(today, year_or_years_back))
}

fn month_in_year(year: i32, month: u32) -> Result<(NaiveDate, NaiveDate)> {
    let first_day =
        NaiveDate::from_ymd_opt(year, month, 1).with_context(|| format!("invalid year {year}"))?;
    Ok(month_containing(first_day))
}

fn parse_month_name(text: &str) -> Result<u32> {
    let month = match text.trim().to_lowercase().as_str() {
        "jan" | "january" => 1,
        "feb" | "february" => 2,
        "mar" | "march" => 3,
        "apr" | "april" => 4,
        "may" => 5,
        "jun" | "june" => 6,
        "jul" | "july" => 7,
        "aug" | "august" => 8,
        "sep" | "sept" | "september" => 9,
        "oct" | "october" => 10,
        "nov" | "november" => 11,
        "dec" | "december" => 12,
        _ => bail!(
            "'{text}' is not a recognized month name/abbreviation, nor a number of months back"
        ),
    };
    Ok(month)
}

/// Resolves the `yearly` command's argument: 0 = the current year,
/// negative = that many years back, positive = that year.
pub fn resolve_year(today: NaiveDate, year_or_years_back: i64) -> (NaiveDate, NaiveDate) {
    let year = if year_or_years_back <= 0 {
        today.year() + year_or_years_back as i32
    } else {
        expand_two_digit_year(year_or_years_back)
    };
    calendar_year(year)
}

/// Apache CloudStack's repo predates this; nothing legitimate resolves outside it.
const MIN_SANE_YEAR: i32 = 2008;

/// Rejects a `begin..end` reaching before `MIN_SANE_YEAR` or past next year —
/// a sign the period arguments were misread. `hint` is appended to the error.
pub fn check_range(begin: NaiveDate, end: NaiveDate, today: NaiveDate, hint: &str) -> Result<()> {
    let max_year = today.year() + 1;
    if begin.year() < MIN_SANE_YEAR || end.year() > max_year {
        bail!("computed period {begin}..{end} is out of range ({MIN_SANE_YEAR}-{max_year}){hint}");
    }
    Ok(())
}

pub fn calendar_year(year: i32) -> (NaiveDate, NaiveDate) {
    (
        NaiveDate::from_ymd_opt(year, 1, 1).expect("valid Jan 1 date"),
        NaiveDate::from_ymd_opt(year, 12, 31).expect("valid Dec 31 date"),
    )
}

pub fn month_containing(date: NaiveDate) -> (NaiveDate, NaiveDate) {
    (
        NaiveDate::from_ymd_opt(date.year(), date.month(), 1).expect("valid first-of-month date"),
        last_day_of_month(date.year(), date.month()),
    )
}

pub fn next_month(year: i32, month: u32) -> (i32, u32) {
    if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn s(v: &str) -> Option<String> {
        Some(v.to_string())
    }

    /// A Thursday.
    fn today() -> NaiveDate {
        d(2026, 10, 29)
    }

    #[test]
    fn slash_dates() {
        assert_eq!(parse_slash_date("2024/03/05").unwrap(), d(2024, 3, 5));
        assert_eq!(parse_slash_date("24/3/5").unwrap(), d(2024, 3, 5));
        assert!(parse_slash_date("024/03/05").is_err());
        assert!(parse_slash_date("2024-03-05").is_err());
        assert!(parse_slash_date("2024/02/30").is_err());
    }

    #[test]
    fn completed_week_ends_on_the_previous_tuesday() {
        let wk = (d(2026, 10, 21), d(2026, 10, 27));
        assert_eq!(last_completed_week(today()), wk);
        assert_eq!(
            last_completed_week(d(2026, 10, 28)),
            wk,
            "on a Wednesday, the week that ended yesterday is complete"
        );
        assert_eq!(
            last_completed_week(d(2026, 10, 27)),
            (d(2026, 10, 14), d(2026, 10, 20)),
            "on a Tuesday, the current week isn't complete yet"
        );
    }

    #[test]
    fn week_containing_is_wednesday_through_tuesday() {
        let wk = (d(2026, 10, 21), d(2026, 10, 27));
        assert_eq!(week_containing(d(2026, 10, 21)), wk);
        assert_eq!(week_containing(d(2026, 10, 27)), wk);
        assert_eq!(
            week_containing(d(2026, 10, 20)),
            (d(2026, 10, 14), d(2026, 10, 20))
        );
    }

    #[test]
    fn month_helpers() {
        assert_eq!(last_day_of_month(2024, 2), d(2024, 2, 29));
        assert_eq!(last_day_of_month(2025, 12), d(2025, 12, 31));
        assert_eq!(
            month_containing(d(2025, 2, 14)),
            (d(2025, 2, 1), d(2025, 2, 28))
        );
        assert_eq!(next_month(2025, 12), (2026, 1));
        assert_eq!(parse_month_name(" Sept ").unwrap(), 9);
        assert_eq!(parse_month_name("DECEMBER").unwrap(), 12);
        assert!(parse_month_name("foo").is_err());
    }

    #[test]
    fn monthly_without_arguments_is_the_current_month() {
        let oct_2026 = (d(2026, 10, 1), d(2026, 10, 31));
        assert_eq!(resolve_month(today(), None, None).unwrap(), oct_2026);
        assert_eq!(resolve_month(today(), s("0"), None).unwrap(), oct_2026);
    }

    #[test]
    fn monthly_negative_number_is_months_back_across_years() {
        let t = today();
        assert_eq!(
            resolve_month(t, s("-3"), None).unwrap(),
            (d(2026, 7, 1), d(2026, 7, 31))
        );
        assert_eq!(
            resolve_month(t, s("-10"), None).unwrap(),
            (d(2025, 12, 1), d(2025, 12, 31))
        );
    }

    #[test]
    fn monthly_positive_number_is_a_year_keeping_the_current_month() {
        let oct_2021 = (d(2021, 10, 1), d(2021, 10, 31));
        assert_eq!(resolve_month(today(), s("2021"), None).unwrap(), oct_2021);
        assert_eq!(resolve_month(today(), s("21"), None).unwrap(), oct_2021);
    }

    #[test]
    fn monthly_month_name_alone_is_that_month_this_year() {
        assert_eq!(
            resolve_month(today(), s("aug"), None).unwrap(),
            (d(2026, 8, 1), d(2026, 8, 31))
        );
    }

    #[test]
    fn monthly_month_name_and_year_in_either_order() {
        let t = today();
        let aug_2021 = (d(2021, 8, 1), d(2021, 8, 31));
        assert_eq!(resolve_month(t, s("aug"), s("2021")).unwrap(), aug_2021);
        assert_eq!(resolve_month(t, s("21"), s("August")).unwrap(), aug_2021);
        assert!(resolve_month(t, s("2021"), s("3")).is_err());
        assert!(resolve_month(t, None, s("aug")).is_err());
    }

    #[test]
    fn range_check() {
        let t = today();
        assert!(check_range(d(2008, 1, 1), d(2027, 12, 31), t, "").is_ok());
        assert!(check_range(d(2007, 12, 31), d(2008, 1, 31), t, "").is_err());
        assert!(check_range(d(2027, 1, 1), d(2028, 1, 1), t, "").is_err());
        let err = check_range(d(1993, 7, 1), d(1993, 7, 31), t, " — hint").unwrap_err();
        assert_eq!(
            err.to_string(),
            "computed period 1993-07-01..1993-07-31 is out of range (2008-2027) — hint"
        );
    }

    #[test]
    fn yearly_argument() {
        let t = today();
        assert_eq!(resolve_year(t, 0), (d(2026, 1, 1), d(2026, 12, 31)));
        assert_eq!(resolve_year(t, -3), (d(2023, 1, 1), d(2023, 12, 31)));
        assert_eq!(resolve_year(t, 23), (d(2023, 1, 1), d(2023, 12, 31)));
        assert_eq!(resolve_year(t, 2019), (d(2019, 1, 1), d(2019, 12, 31)));
    }

    #[test]
    fn implicit_shortcut() {
        let t = today();
        let aug_2021 = (d(2021, 8, 1), d(2021, 8, 31));
        assert!(resolve_implicit(t, None, None).is_err());
        assert_eq!(
            resolve_implicit(t, s("aug"), None).unwrap(),
            (d(2026, 8, 1), d(2026, 8, 31))
        );
        assert_eq!(resolve_implicit(t, s("2021"), s("aug")).unwrap(), aug_2021);
        assert_eq!(
            resolve_implicit(t, s("2021"), None).unwrap(),
            (d(2021, 1, 1), d(2021, 12, 31))
        );
        assert_eq!(
            resolve_implicit(t, s("-1"), None).unwrap(),
            (d(2025, 1, 1), d(2025, 12, 31))
        );
        assert!(resolve_implicit(t, s("2021"), s("3")).is_err());
        assert!(resolve_implicit(t, s("foo"), None).is_err());
    }
}
