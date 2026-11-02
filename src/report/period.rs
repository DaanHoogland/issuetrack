use anyhow::Result;
use chrono::{Datelike, Duration, NaiveDate};

use super::{fetch_span, print_period_row, print_rows};
use crate::ctx::Ctx;
use crate::dates::{
    check_range, last_completed_week, last_day_of_month, month_containing, next_month,
    week_containing,
};
use crate::gh;

/// Appended to the out-of-range error for periods resolved from monthly/yearly
/// arguments, where a misread bare number is the likely cause.
const BARE_NUMBER_HINT: &str = " — a bare number is interpreted as \"N months/years back\", \
     not an absolute year; for an absolute year use e.g. `yearly 2021` or `monthly aug 2021`";

/// Prints one weekly row per completed Wednesday-through-Tuesday week, from
/// the week containing `since_date` through the most recently completed week.
pub fn run_weekly_since(ctx: &Ctx, since_date: NaiveDate, chronological: bool) -> Result<()> {
    let today = ctx.today;
    let (_, last_end) = last_completed_week(today);
    let (span_begin, _) = week_containing(since_date);
    if span_begin > last_end {
        eprintln!("no completed weeks between {since_date} and {today}");
        return Ok(());
    }
    weekly_rows(ctx, span_begin, last_end, chronological)
}

/// Prints one weekly row per Wednesday-through-Tuesday week overlapping
/// `begin..end` — a single row when the period is exactly one week (the
/// `weekly` command's default), several when `--since`/`--until` cover a
/// wider range.
pub fn run_weekly_period(
    ctx: &Ctx,
    begin: NaiveDate,
    end: NaiveDate,
    chronological: bool,
) -> Result<()> {
    check_range(begin, end, ctx.today, "")?;
    let (span_begin, _) = week_containing(begin);
    let (_, span_end) = week_containing(end);
    weekly_rows(ctx, span_begin, span_end, chronological)
}

/// Prints one row per completed Wednesday-through-Tuesday week from
/// `span_begin` through `span_end` (both assumed already week-aligned),
/// fetching every matching issue for the whole span in a single (paginated)
/// search rather than one search per week — the search API's own rate limit
/// is easy to trip otherwise on a multi-week backfill. Rows print
/// newest-first by default; pass `chronological` to print oldest-first.
fn weekly_rows(
    ctx: &Ctx,
    span_begin: NaiveDate,
    span_end: NaiveDate,
    chronological: bool,
) -> Result<()> {
    let items = fetch_span(ctx, span_begin, span_end, "weeks")?;

    let mut weeks = Vec::new();
    let (mut begin, mut end) = (span_begin, span_begin + Duration::days(6));
    while end <= span_end {
        weeks.push((begin, end));
        begin += Duration::days(7);
        end += Duration::days(7);
    }
    print_rows(ctx, &items, weeks, chronological);
    Ok(())
}

/// Prints one row per repo for `begin..end`, using two count-only searches
/// per repo (a combined query can't split counts by repo). `closed` means
/// created in the window and closed *now*, not closed during the window.
pub fn run_period(ctx: &Ctx, begin: NaiveDate, end: NaiveDate) -> Result<()> {
    check_range(begin, end, ctx.today, BARE_NUMBER_HINT)?;

    for repo in &ctx.repos {
        let open_q = format!("repo:{repo} is:issue is:open created:{begin}..{end}");
        let closed_q = format!("repo:{repo} is:issue is:closed created:{begin}..{end}");

        let open = gh::total_count(&open_q, ctx.token())?;
        let closed = gh::total_count(&closed_q, ctx.token())?;
        print_period_row(ctx, repo, begin, end, open, closed);
    }

    Ok(())
}

/// Prints one row per calendar month overlapping `begin..end` — a single row
/// when the period is exactly one month (the `monthly` command's default),
/// several when `--since`/`--until` cover a wider range.
pub fn run_monthly_period(
    ctx: &Ctx,
    begin: NaiveDate,
    end: NaiveDate,
    chronological: bool,
) -> Result<()> {
    check_range(begin, end, ctx.today, BARE_NUMBER_HINT)?;
    let (span_begin, _) = month_containing(begin);
    let (_, span_end) = month_containing(end);

    let items = fetch_span(ctx, span_begin, span_end, "months")?;

    let mut months = Vec::new();
    let (mut year, mut month) = (span_begin.year(), span_begin.month());
    loop {
        let (month_begin, month_end) = (
            NaiveDate::from_ymd_opt(year, month, 1).expect("valid first-of-month date"),
            last_day_of_month(year, month),
        );
        months.push((month_begin, month_end));
        if month_end >= span_end {
            break;
        }
        (year, month) = next_month(year, month);
    }
    print_rows(ctx, &items, months, chronological);
    Ok(())
}

/// Prints one row per calendar year overlapping `begin..end` — a single row
/// when the period is exactly one year (the `yearly` command's default),
/// several when `--since`/`--until` cover a wider range.
pub fn run_yearly_period(
    ctx: &Ctx,
    begin: NaiveDate,
    end: NaiveDate,
    chronological: bool,
) -> Result<()> {
    check_range(begin, end, ctx.today, BARE_NUMBER_HINT)?;
    let span_begin_year = begin.year();
    let span_end_year = end.year();
    let span_begin = NaiveDate::from_ymd_opt(span_begin_year, 1, 1).expect("valid Jan 1 date");
    let span_end = NaiveDate::from_ymd_opt(span_end_year, 12, 31).expect("valid Dec 31 date");

    let items = fetch_span(ctx, span_begin, span_end, "years")?;

    let mut years = Vec::new();
    for year in span_begin_year..=span_end_year {
        let year_begin = NaiveDate::from_ymd_opt(year, 1, 1).expect("valid Jan 1 date");
        let year_end = NaiveDate::from_ymd_opt(year, 12, 31).expect("valid Dec 31 date");
        years.push((year_begin, year_end));
    }
    print_rows(ctx, &items, years, chronological);
    Ok(())
}
