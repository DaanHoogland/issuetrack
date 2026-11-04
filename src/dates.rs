use anyhow::{Context, Result};
use chrono::{Datelike, Duration, NaiveDate};

use crate::cli::PeriodOverride;

pub fn parse_date(s: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .with_context(|| format!("invalid date '{s}', expected YYYY-MM-DD"))
}

/// Parses yyyy/mm/dd or yy/mm/dd (2-digit year -> 20XX, this century).
pub fn parse_slash_date(s: &str) -> Result<NaiveDate> {
    let bad = || anyhow::anyhow!("invalid date '{s}', expected yyyy/mm/dd or yy/mm/dd");
    let parts: Vec<&str> = s.split('/').collect();
    let [year_part, month_part, day_part] = parts[..] else {
        return Err(bad());
    };
    let year_num: i32 = year_part.parse().map_err(|_| bad())?;
    let year = match year_part.len() {
        1 | 2 => 2000 + year_num,
        4 => year_num,
        _ => return Err(bad()),
    };
    let month: u32 = month_part.parse().map_err(|_| bad())?;
    let day: u32 = day_part.parse().map_err(|_| bad())?;
    NaiveDate::from_ymd_opt(year, month, day).ok_or_else(bad)
}

pub fn resolve_period(
    over: PeriodOverride,
    today: NaiveDate,
    default_fn: impl Fn(NaiveDate) -> Result<(NaiveDate, NaiveDate)>,
) -> Result<(NaiveDate, NaiveDate)> {
    match (over.since, over.until) {
        (Some(s), Some(u)) => Ok((parse_date(&s)?, parse_date(&u)?)),
        _ => default_fn(today),
    }
}

/// Most recently completed Wednesday-through-Tuesday window, ending on the
/// last Tuesday strictly before `today`.
pub fn last_completed_week(today: NaiveDate) -> (NaiveDate, NaiveDate) {
    let reference = today - Duration::days(1);
    let tuesday = 1; // Weekday::num_days_from_monday(): Monday=0, Tuesday=1
    let offset = (reference.weekday().num_days_from_monday() as i64 - tuesday).rem_euclid(7);
    let end = reference - Duration::days(offset);
    let begin = end - Duration::days(6);
    (begin, end)
}

/// The Wednesday-through-Tuesday week that contains `date`.
pub fn week_containing(date: NaiveDate) -> (NaiveDate, NaiveDate) {
    let wednesday = 2; // Weekday::num_days_from_monday(): Monday=0, ..., Wednesday=2
    let offset = (date.weekday().num_days_from_monday() as i64 - wednesday).rem_euclid(7);
    let begin = date - Duration::days(offset);
    let end = begin + Duration::days(6);
    (begin, end)
}

pub fn last_day_of_month(year: i32, month: u32) -> NaiveDate {
    let (next_year, next_month) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .expect("valid next-month date")
        - Duration::days(1)
}

/// Calendar month `ago` months before `today`'s month (0 = current month).
fn month_with_offset(today: NaiveDate, ago: u32) -> (NaiveDate, NaiveDate) {
    let total_months = today.year() as i64 * 12 + (today.month() as i64 - 1) - ago as i64;
    let year = total_months.div_euclid(12) as i32;
    let month = (total_months.rem_euclid(12) + 1) as u32;
    let begin = NaiveDate::from_ymd_opt(year, month, 1).expect("valid first-of-month date");
    let end = last_day_of_month(year, month);
    (begin, end)
}

/// Resolves the `monthly` command's positional parameters, in either order:
/// - both absent: current month.
/// - one number only: that many months back (existing behavior).
/// - one month name/abbreviation only: that month in the current year.
/// - a month name and a year (2-digit -> 20XX, 4-digit as-is), in either order.
/// - one bare number only: negative = that many months back; positive = an
///   absolute year (2-digit -> 20XX, 4-digit as-is), keeping the current month.
pub fn resolve_month(
    today: NaiveDate,
    first: Option<String>,
    second: Option<String>,
) -> Result<(NaiveDate, NaiveDate)> {
    match (first, second) {
        (None, None) => Ok(month_with_offset(today, 0)),
        (Some(a), None) => {
            if let Ok(n) = a.parse::<i64>() {
                if n < 0 {
                    Ok(month_with_offset(today, n.unsigned_abs() as u32))
                } else if n == 0 {
                    Ok(month_with_offset(today, 0))
                } else {
                    let y = if n < 100 { 2000 + n as i32 } else { n as i32 };
                    month_in_year(y, today.month())
                }
            } else {
                let month_num = parse_month_name(&a)?;
                month_in_year(today.year(), month_num)
            }
        }
        (Some(a), Some(b)) => {
            let (month_str, year_str) = if parse_month_name(&a).is_ok() {
                (a, b)
            } else if parse_month_name(&b).is_ok() {
                (b, a)
            } else {
                anyhow::bail!(
                    "give a month name/abbreviation and a year, e.g. `monthly aug 2021` \
                     or `monthly 2021 aug`"
                );
            };
            let month_num = parse_month_name(&month_str)?;
            let year_num: i64 = year_str
                .parse()
                .with_context(|| format!("'{year_str}' is not a valid year"))?;
            let y = if year_num < 100 { 2000 + year_num as i32 } else { year_num as i32 };
            month_in_year(y, month_num)
        }
        (None, Some(_)) => anyhow::bail!("give a month and/or year"),
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
        anyhow::bail!(
            "no command given — run with --help, or provide a month and/or year \
             (e.g. `issuetrack aug 2021` or `issuetrack 2021`)"
        );
    }
    let has_month = [first.as_deref(), second.as_deref()]
        .into_iter()
        .flatten()
        .any(|s| parse_month_name(s).is_ok());
    if has_month {
        return resolve_month(today, first, second);
    }
    if second.is_some() {
        anyhow::bail!(
            "give a month name/abbreviation and a year, or use an explicit subcommand \
             (monthly/yearly) for other combinations"
        );
    }
    let s = first.expect("checked above");
    let n: i64 = s
        .parse()
        .with_context(|| format!("'{s}' is not a recognized month name, year, or offset"))?;
    Ok(resolve_year(today, n))
}

fn month_in_year(year: i32, month_num: u32) -> Result<(NaiveDate, NaiveDate)> {
    let begin = NaiveDate::from_ymd_opt(year, month_num, 1)
        .with_context(|| format!("invalid year {year}"))?;
    let end = last_day_of_month(year, month_num);
    Ok((begin, end))
}

fn parse_month_name(s: &str) -> Result<u32> {
    let month = match s.trim().to_lowercase().as_str() {
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
        _ => anyhow::bail!(
            "'{s}' is not a recognized month name/abbreviation, nor a number of months back"
        ),
    };
    Ok(month)
}

/// Resolves the `yearly` command's `n` parameter: 0 = current year, negative
/// = N years back, positive = an absolute year (2-digit -> 20XX, 4-digit as-is).
pub fn resolve_year(today: NaiveDate, n: i64) -> (NaiveDate, NaiveDate) {
    let year = if n < 0 {
        today.year() - (-n) as i32
    } else if n == 0 {
        today.year()
    } else if n < 100 {
        2000 + n as i32
    } else {
        n as i32
    };
    let begin = NaiveDate::from_ymd_opt(year, 1, 1).expect("valid Jan 1 date");
    let end = NaiveDate::from_ymd_opt(year, 12, 31).expect("valid Dec 31 date");
    (begin, end)
}

/// Apache CloudStack's repo predates this; nothing legitimate resolves outside it.
const MIN_SANE_YEAR: i32 = 2008;

/// Rejects a `begin..end` reaching before `MIN_SANE_YEAR` or past next year —
/// a sign the period arguments were misread. `hint` is appended to the error.
pub fn check_range(begin: NaiveDate, end: NaiveDate, today: NaiveDate, hint: &str) -> Result<()> {
    let max_year = today.year() + 1;
    if begin.year() < MIN_SANE_YEAR || end.year() > max_year {
        anyhow::bail!(
            "computed period {begin}..{end} is out of range ({MIN_SANE_YEAR}-{max_year}){hint}"
        );
    }
    Ok(())
}

/// The calendar month `date.year()`/`date.month()` falls in.
pub fn month_containing(date: NaiveDate) -> (NaiveDate, NaiveDate) {
    (
        NaiveDate::from_ymd_opt(date.year(), date.month(), 1).expect("valid first-of-month date"),
        last_day_of_month(date.year(), date.month()),
    )
}

/// The calendar month following `year`/`month`.
pub fn next_month(year: i32, month: u32) -> (i32, u32) {
    if month == 12 { (year + 1, 1) } else { (year, month + 1) }
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
        // On a Wednesday the week that ended yesterday is complete.
        assert_eq!(last_completed_week(d(2026, 10, 28)), wk);
        // On a Tuesday the current week isn't complete yet.
        assert_eq!(last_completed_week(d(2026, 10, 27)), (d(2026, 10, 14), d(2026, 10, 20)));
    }

    #[test]
    fn week_containing_is_wednesday_through_tuesday() {
        let wk = (d(2026, 10, 21), d(2026, 10, 27));
        assert_eq!(week_containing(d(2026, 10, 21)), wk);
        assert_eq!(week_containing(d(2026, 10, 27)), wk);
        assert_eq!(week_containing(d(2026, 10, 20)), (d(2026, 10, 14), d(2026, 10, 20)));
    }

    #[test]
    fn month_helpers() {
        assert_eq!(last_day_of_month(2024, 2), d(2024, 2, 29));
        assert_eq!(last_day_of_month(2025, 12), d(2025, 12, 31));
        assert_eq!(month_containing(d(2025, 2, 14)), (d(2025, 2, 1), d(2025, 2, 28)));
        assert_eq!(next_month(2025, 12), (2026, 1));
        assert_eq!(parse_month_name(" Sept ").unwrap(), 9);
        assert_eq!(parse_month_name("DECEMBER").unwrap(), 12);
        assert!(parse_month_name("foo").is_err());
    }

    #[test]
    fn monthly_arguments() {
        let t = today();
        let oct_2026 = (d(2026, 10, 1), d(2026, 10, 31));
        let oct_2021 = (d(2021, 10, 1), d(2021, 10, 31));
        let aug_2021 = (d(2021, 8, 1), d(2021, 8, 31));
        assert_eq!(resolve_month(t, None, None).unwrap(), oct_2026);
        assert_eq!(resolve_month(t, s("0"), None).unwrap(), oct_2026);
        // Negative = months back, crossing the year boundary.
        assert_eq!(resolve_month(t, s("-3"), None).unwrap(), (d(2026, 7, 1), d(2026, 7, 31)));
        assert_eq!(resolve_month(t, s("-10"), None).unwrap(), (d(2025, 12, 1), d(2025, 12, 31)));
        // Positive = an absolute year, keeping the current month.
        assert_eq!(resolve_month(t, s("2021"), None).unwrap(), oct_2021);
        assert_eq!(resolve_month(t, s("21"), None).unwrap(), oct_2021);
        // Month name alone = that month this year.
        assert_eq!(resolve_month(t, s("aug"), None).unwrap(), (d(2026, 8, 1), d(2026, 8, 31)));
        // Month name + year, in either order.
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
        assert_eq!(resolve_implicit(t, s("aug"), None).unwrap(), (d(2026, 8, 1), d(2026, 8, 31)));
        assert_eq!(resolve_implicit(t, s("2021"), s("aug")).unwrap(), aug_2021);
        assert_eq!(resolve_implicit(t, s("2021"), None).unwrap(), (d(2021, 1, 1), d(2021, 12, 31)));
        assert_eq!(resolve_implicit(t, s("-1"), None).unwrap(), (d(2025, 1, 1), d(2025, 12, 31)));
        assert!(resolve_implicit(t, s("2021"), s("3")).is_err());
        assert!(resolve_implicit(t, s("foo"), None).is_err());
    }
}
