mod config;
mod gh;
mod state;

use anyhow::{Context, Result};
use chrono::{Datelike, Duration, Local, NaiveDate};
use clap::{Parser, Subcommand};
use serde_json::Value;
use std::path::PathBuf;

/// Refresh helper for the ACSissues Google Sheet: pulls data from GitHub and
/// prints TSV rows ready to paste into the sheet. Does not touch the sheet
/// itself — no Google credentials involved.
#[derive(Parser)]
#[command(name = "issuetrack", version, about, infer_subcommands = true)]
struct Cli {
    /// GitHub repo to query
    #[arg(short = 'r', long, global = true, default_value = "apache/cloudstack")]
    repo: String,

    /// Path to config.toml (excluded ShapeBlue users / extra bots)
    #[arg(short = 'c', long, global = true)]
    config: Option<PathBuf>,

    /// Path to the state file tracking the last "individual" run date
    #[arg(short = 'S', long, global = true)]
    state: Option<PathBuf>,

    /// GitHub personal access token to use instead of `gh`'s own stored
    /// login. Falls back to `github_token` in config.toml, then to `gh`'s
    /// normal auth (GH_TOKEN/GITHUB_TOKEN env vars or `gh auth login`).
    #[arg(short = 't', long, global = true)]
    token: Option<String>,

    /// Format url columns as markdown links instead of plain urls: the
    /// weekly/monthly/yearly period url (`[begin..end](url)`) and the
    /// `individual` reporter's GitHub profile url (`[login](url)`).
    #[arg(short = 'm', long, global = true)]
    markdown_url: bool,

    #[command(subcommand)]
    command: Option<Command>,

    /// Implicit shortcut when no subcommand is given: a month name/abbreviation
    /// and/or a year resolve to `monthly`/`yearly` automatically (in either
    /// order), e.g. `issuetrack aug 2021` or `issuetrack 2021`.
    #[arg(allow_hyphen_values = true)]
    first: Option<String>,
    #[arg(allow_hyphen_values = true)]
    second: Option<String>,
}

/// Manual override of a period's begin/end date, for backfilling.
#[derive(clap::Args)]
struct PeriodOverride {
    /// Override the period's begin date (YYYY-MM-DD). Requires --until too.
    #[arg(short = 's', long, requires = "until")]
    since: Option<String>,
    /// Override the period's end date (YYYY-MM-DD). Requires --since too.
    #[arg(short = 'u', long, requires = "since")]
    until: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// TSV rows (url, title, user profile url, opened date) for new external-party issues,
    /// for the ACSissues tab. Blank status/action columns are left for you to fill in.
    Individual {
        /// Start date (YYYY-MM-DD). Defaults to the last recorded run, or 7 days ago.
        #[arg(short = 's', long)]
        since: Option<String>,
        /// End date (YYYY-MM-DD). Defaults to today.
        #[arg(short = 'u', long)]
        until: Option<String>,
        /// Don't read or update the state file.
        #[arg(short = 'n', long)]
        no_state: bool,
        /// Print the reporter as their bare GitHub username instead of a
        /// profile url (overrides --markdown-url for that column).
        #[arg(short = 'U', long)]
        username: bool,
        /// Look up each reporter's full GitHub display name and show it
        /// (instead of their login) wherever the reporter column has a
        /// separate display text: as the markdown link text with
        /// --markdown-url, or as the displayed name with --username. Falls
        /// back to the login for accounts with no display name set, and has
        /// no effect on the plain profile-url output (neither flag given).
        #[arg(short = 'F', long)]
        full_username: bool,
    },
    /// One summary TSV row (begin, end, open, closed, total, date, duration,
    /// url, rate) for the Weekly tab, covering the most recently completed
    /// Wednesday-through-Tuesday window.
    Weekly {
        #[command(flatten)]
        over: PeriodOverride,
        /// Print one row per completed week from the week containing this
        /// date through the most recently completed week. Format: yyyy/mm/dd
        /// or yy/mm/dd (2-digit year, this century).
        #[arg(value_name = "DATE")]
        since_date: Option<String>,
    },
    /// Same shape as `weekly`, covering the current calendar month by default.
    #[command(allow_negative_numbers = true)]
    Monthly {
        #[command(flatten)]
        over: PeriodOverride,
        /// Months back (e.g. -3 = 3 months ago), or a month name/abbreviation
        /// (e.g. Aug, August), optionally paired with a year (in either order).
        #[arg(allow_hyphen_values = true)]
        first: Option<String>,
        /// A year (2-digit or 4-digit) to go with a month name in `first`.
        /// May be given before or after the month name.
        #[arg(allow_hyphen_values = true)]
        second: Option<String>,
    },
    /// Same shape as `weekly`, covering the current calendar year by default.
    #[command(allow_negative_numbers = true)]
    Yearly {
        #[command(flatten)]
        over: PeriodOverride,
        /// 0 = current year; negative = N years back (e.g. -3 = 3 years ago);
        /// positive = an absolute year, 2-digit (23 -> 2023) or 4-digit (2023).
        #[arg(allow_hyphen_values = true, default_value_t = 0)]
        n: i64,
    },
    /// Write a starter config.toml if one doesn't already exist.
    InitConfig,
}

fn parse_date(s: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .with_context(|| format!("invalid date '{s}', expected YYYY-MM-DD"))
}

/// Parses yyyy/mm/dd or yy/mm/dd (2-digit year -> 20XX, this century).
fn parse_slash_date(s: &str) -> Result<NaiveDate> {
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config_path = cli.config.clone().unwrap_or_else(config::default_path);
    let state_path = cli.state.clone().unwrap_or_else(state::default_path);
    let today = Local::now().date_naive();
    let file_defaults = config::load_defaults(&config_path);
    let token = cli.token.clone().or(file_defaults.github_token);
    let markdown_url = cli.markdown_url || file_defaults.markdown_url;

    match cli.command {
        Some(Command::InitConfig) => init_config(&config_path),
        Some(Command::Individual {
            since,
            until,
            no_state,
            username,
            full_username,
        }) => run_individual(
            &cli.repo,
            &config_path,
            &state_path,
            since,
            until,
            no_state,
            today,
            token.as_deref(),
            markdown_url,
            username,
            full_username,
        ),
        Some(Command::Weekly { over: _, since_date: Some(s) }) => {
            let date = parse_slash_date(&s)?;
            run_weekly_since(&cli.repo, date, today, token.as_deref(), markdown_url)
        }
        Some(Command::Weekly { over, since_date: None }) => {
            let (begin, end) = resolve_period(over, today, |t| Ok(last_completed_week(t)))?;
            run_weekly_period(&cli.repo, begin, end, today, token.as_deref(), markdown_url)
        }
        Some(Command::Monthly { over, first, second }) => {
            let (begin, end) = resolve_period(over, today, move |t| {
                resolve_month(t, first.clone(), second.clone())
            })?;
            run_monthly_period(&cli.repo, begin, end, today, token.as_deref(), markdown_url)
        }
        Some(Command::Yearly { over, n }) => {
            let (begin, end) = resolve_period(over, today, move |t| Ok(resolve_year(t, n)))?;
            run_yearly_period(&cli.repo, begin, end, today, token.as_deref(), markdown_url)
        }
        None => {
            let (begin, end) = resolve_implicit(today, cli.first, cli.second)?;
            run_period(&cli.repo, begin, end, today, token.as_deref(), markdown_url)
        }
    }
}

fn resolve_period(
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
fn last_completed_week(today: NaiveDate) -> (NaiveDate, NaiveDate) {
    let reference = today - Duration::days(1);
    let tuesday = 1; // Weekday::num_days_from_monday(): Monday=0, Tuesday=1
    let offset = (reference.weekday().num_days_from_monday() as i64 - tuesday).rem_euclid(7);
    let end = reference - Duration::days(offset);
    let begin = end - Duration::days(6);
    (begin, end)
}

/// The Wednesday-through-Tuesday week that contains `date`.
fn week_containing(date: NaiveDate) -> (NaiveDate, NaiveDate) {
    let wednesday = 2; // Weekday::num_days_from_monday(): Monday=0, ..., Wednesday=2
    let offset = (date.weekday().num_days_from_monday() as i64 - wednesday).rem_euclid(7);
    let begin = date - Duration::days(offset);
    let end = begin + Duration::days(6);
    (begin, end)
}

/// Prints one weekly row per completed Wednesday-through-Tuesday week, from
/// the week containing `since_date` through the most recently completed week.
fn run_weekly_since(
    repo: &str,
    since_date: NaiveDate,
    today: NaiveDate,
    token: Option<&str>,
    markdown_url: bool,
) -> Result<()> {
    let (_, last_end) = last_completed_week(today);
    let (span_begin, _) = week_containing(since_date);
    if span_begin > last_end {
        eprintln!("no completed weeks between {since_date} and {today}");
        return Ok(());
    }
    weekly_rows(repo, span_begin, last_end, today, token, markdown_url)
}

/// Prints one weekly row per Wednesday-through-Tuesday week overlapping
/// `begin..end` — a single row when the period is exactly one week (the
/// `weekly` command's default), several when `--since`/`--until` cover a
/// wider range.
fn run_weekly_period(
    repo: &str,
    begin: NaiveDate,
    end: NaiveDate,
    today: NaiveDate,
    token: Option<&str>,
    markdown_url: bool,
) -> Result<()> {
    let max_year = today.year() + 1;
    if begin.year() < MIN_SANE_YEAR || end.year() > max_year {
        anyhow::bail!(
            "computed period {begin}..{end} is out of range ({MIN_SANE_YEAR}-{max_year})"
        );
    }
    let (span_begin, _) = week_containing(begin);
    let (_, span_end) = week_containing(end);
    weekly_rows(repo, span_begin, span_end, today, token, markdown_url)
}

/// Prints one row per completed Wednesday-through-Tuesday week from
/// `span_begin` through `span_end` (both assumed already week-aligned),
/// fetching every matching issue for the whole span in a single (paginated)
/// search rather than one search per week — the search API's own rate limit
/// is easy to trip otherwise on a multi-week backfill.
fn weekly_rows(
    repo: &str,
    span_begin: NaiveDate,
    span_end: NaiveDate,
    today: NaiveDate,
    token: Option<&str>,
    markdown_url: bool,
) -> Result<()> {
    let query = format!("repo:{repo} is:issue created:{span_begin}..{span_end}");
    let items = gh::all_items(&query, token)?;
    if items.len() >= 1000 {
        eprintln!(
            "warning: {span_begin}..{span_end} hit the search API's 1000-result cap — \
             some weeks' counts may be undercounted; narrow the range if needed"
        );
    }

    let (mut begin, mut end) = (span_begin, span_begin + Duration::days(6));
    while end <= span_end {
        let (open, closed) = counts_in_range(&items, begin, end);
        print_period_row(repo, begin, end, today, open, closed, markdown_url);
        begin += Duration::days(7);
        end += Duration::days(7);
    }
    Ok(())
}

/// Counts, among already-fetched issue-search items, how many were created
/// within `begin..end` and are currently open vs. closed.
fn counts_in_range(items: &[Value], begin: NaiveDate, end: NaiveDate) -> (u64, u64) {
    let (mut open, mut closed) = (0u64, 0u64);
    for item in items {
        let created_at = item
            .get("created_at")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let date_str = created_at.split('T').next().unwrap_or(created_at);
        let Ok(created) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") else {
            continue;
        };
        if created < begin || created > end {
            continue;
        }
        match item.get("state").and_then(|v| v.as_str()) {
            Some("open") => open += 1,
            Some("closed") => closed += 1,
            _ => {}
        }
    }
    (open, closed)
}

fn last_day_of_month(year: i32, month: u32) -> NaiveDate {
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
fn resolve_month(
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
fn resolve_implicit(
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
fn resolve_year(today: NaiveDate, n: i64) -> (NaiveDate, NaiveDate) {
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

fn init_config(path: &PathBuf) -> Result<()> {
    if path.exists() {
        anyhow::bail!(
            "config file already exists at {} — edit it directly, refusing to overwrite",
            path.display()
        );
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, config::STARTER)
        .with_context(|| format!("writing starter config to {}", path.display()))?;
    println!("wrote starter config to {}", path.display());
    println!("edit it to add ShapeBlue GitHub usernames before running `individual`");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_individual(
    repo: &str,
    config_path: &PathBuf,
    state_path: &PathBuf,
    since: Option<String>,
    until: Option<String>,
    no_state: bool,
    today: NaiveDate,
    token: Option<&str>,
    markdown_url: bool,
    username: bool,
    full_username: bool,
) -> Result<()> {
    let cfg = config::Config::load(config_path)?;
    let st = if no_state {
        state::State::default()
    } else {
        state::State::load(state_path)?
    };

    let until_date = match until {
        Some(s) => parse_date(&s)?,
        None => today,
    };
    let since_date = match since {
        Some(s) => parse_date(&s)?,
        None => st
            .last_individual
            .unwrap_or_else(|| today - Duration::days(7)),
    };

    let query = format!("repo:{repo} is:issue created:{since_date}..{until_date}");
    let items = gh::all_items(&query, token)?;

    let mut full_names: std::collections::HashMap<String, Option<String>> = Default::default();
    let mut rows: Vec<(NaiveDate, String)> = Vec::new();
    for item in &items {
        let login = item
            .get("user")
            .and_then(|u| u.get("login"))
            .and_then(|l| l.as_str())
            .unwrap_or("");
        if login.is_empty() || !cfg.is_external(login) {
            continue;
        }
        let url = item.get("html_url").and_then(|v| v.as_str()).unwrap_or("");
        let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let created_at = item
            .get("created_at")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let opened_date = created_at.split('T').next().unwrap_or(created_at);
        let opened = NaiveDate::parse_from_str(opened_date, "%Y-%m-%d").unwrap_or(today);

        // The name shown in place of the login, wherever the reporter column
        // has a display text separate from the raw url (markdown link text,
        // or the bare --username text). One lookup per unique login per run.
        let display_name = if full_username && (username || markdown_url) {
            full_names
                .entry(login.to_string())
                .or_insert_with(|| gh::user_full_name(login, token).unwrap_or(None))
                .clone()
                .unwrap_or_else(|| login.to_string())
        } else {
            login.to_string()
        };
        let user = if username {
            display_name
        } else {
            let user_link = format!("https://github.com/{login}");
            if markdown_url {
                format!("[{display_name}]({user_link})")
            } else {
                user_link
            }
        };
        let row = format!("{url}\t{title}\t{user}\t{opened_date}\t\t");
        rows.push((opened, row));
    }
    rows.sort_by(|a, b| b.0.cmp(&a.0));

    for (_, row) in &rows {
        println!("{row}");
    }
    eprintln!(
        "{} external-party issue(s) from {since_date} to {until_date}",
        rows.len()
    );

    if !no_state {
        let mut st = st;
        st.last_individual = Some(until_date);
        st.save(state_path)?;
    }

    Ok(())
}

/// Apache CloudStack's repo predates this; nothing legitimate resolves outside it.
const MIN_SANE_YEAR: i32 = 2008;

fn run_period(
    repo: &str,
    begin: NaiveDate,
    end: NaiveDate,
    today: NaiveDate,
    token: Option<&str>,
    markdown_url: bool,
) -> Result<()> {
    let max_year = today.year() + 1;
    if begin.year() < MIN_SANE_YEAR || end.year() > max_year {
        anyhow::bail!(
            "computed period {begin}..{end} is out of range ({MIN_SANE_YEAR}-{max_year}) \
             — a bare number is interpreted as \"N months/years back\", not an absolute year; \
             for an absolute year use e.g. `yearly 2021` or `monthly aug 2021`"
        );
    }

    // open/closed reflect the *current* status of issues created in this
    // window (so open + closed always equals the window's total), not
    // "closed during the window" — an issue created in this period may have
    // been closed long after it ended.
    let open_q = format!("repo:{repo} is:issue is:open created:{begin}..{end}");
    let closed_q = format!("repo:{repo} is:issue is:closed created:{begin}..{end}");

    let open = gh::total_count(&open_q, token)?;
    let closed = gh::total_count(&closed_q, token)?;
    print_period_row(repo, begin, end, today, open, closed, markdown_url);

    Ok(())
}

/// The calendar month `date.year()`/`date.month()` falls in.
fn month_containing(date: NaiveDate) -> (NaiveDate, NaiveDate) {
    (
        NaiveDate::from_ymd_opt(date.year(), date.month(), 1).expect("valid first-of-month date"),
        last_day_of_month(date.year(), date.month()),
    )
}

/// The calendar month following `year`/`month`.
fn next_month(year: i32, month: u32) -> (i32, u32) {
    if month == 12 { (year + 1, 1) } else { (year, month + 1) }
}

/// Prints one row per calendar month overlapping `begin..end` — a single row
/// when the period is exactly one month (the `monthly` command's default),
/// several when `--since`/`--until` cover a wider range.
fn run_monthly_period(
    repo: &str,
    begin: NaiveDate,
    end: NaiveDate,
    today: NaiveDate,
    token: Option<&str>,
    markdown_url: bool,
) -> Result<()> {
    let max_year = today.year() + 1;
    if begin.year() < MIN_SANE_YEAR || end.year() > max_year {
        anyhow::bail!(
            "computed period {begin}..{end} is out of range ({MIN_SANE_YEAR}-{max_year}) \
             — a bare number is interpreted as \"N months/years back\", not an absolute year; \
             for an absolute year use e.g. `yearly 2021` or `monthly aug 2021`"
        );
    }
    let (span_begin, _) = month_containing(begin);
    let (_, span_end) = month_containing(end);

    let query = format!("repo:{repo} is:issue created:{span_begin}..{span_end}");
    let items = gh::all_items(&query, token)?;
    if items.len() >= 1000 {
        eprintln!(
            "warning: {span_begin}..{span_end} hit the search API's 1000-result cap — \
             some months' counts may be undercounted; narrow the range if needed"
        );
    }

    let (mut year, mut month) = (span_begin.year(), span_begin.month());
    loop {
        let (month_begin, month_end) = (
            NaiveDate::from_ymd_opt(year, month, 1).expect("valid first-of-month date"),
            last_day_of_month(year, month),
        );
        let (open, closed) = counts_in_range(&items, month_begin, month_end);
        print_period_row(repo, month_begin, month_end, today, open, closed, markdown_url);
        if month_end >= span_end {
            break;
        }
        (year, month) = next_month(year, month);
    }
    Ok(())
}

/// Prints one row per calendar year overlapping `begin..end` — a single row
/// when the period is exactly one year (the `yearly` command's default),
/// several when `--since`/`--until` cover a wider range.
fn run_yearly_period(
    repo: &str,
    begin: NaiveDate,
    end: NaiveDate,
    today: NaiveDate,
    token: Option<&str>,
    markdown_url: bool,
) -> Result<()> {
    let max_year = today.year() + 1;
    if begin.year() < MIN_SANE_YEAR || end.year() > max_year {
        anyhow::bail!(
            "computed period {begin}..{end} is out of range ({MIN_SANE_YEAR}-{max_year}) \
             — a bare number is interpreted as \"N months/years back\", not an absolute year; \
             for an absolute year use e.g. `yearly 2021` or `monthly aug 2021`"
        );
    }
    let span_begin_year = begin.year();
    let span_end_year = end.year();
    let span_begin = NaiveDate::from_ymd_opt(span_begin_year, 1, 1).expect("valid Jan 1 date");
    let span_end = NaiveDate::from_ymd_opt(span_end_year, 12, 31).expect("valid Dec 31 date");

    let query = format!("repo:{repo} is:issue created:{span_begin}..{span_end}");
    let items = gh::all_items(&query, token)?;
    if items.len() >= 1000 {
        eprintln!(
            "warning: {span_begin}..{span_end} hit the search API's 1000-result cap — \
             some years' counts may be undercounted; narrow the range if needed"
        );
    }

    for year in span_begin_year..=span_end_year {
        let year_begin = NaiveDate::from_ymd_opt(year, 1, 1).expect("valid Jan 1 date");
        let year_end = NaiveDate::from_ymd_opt(year, 12, 31).expect("valid Dec 31 date");
        let (open, closed) = counts_in_range(&items, year_begin, year_end);
        print_period_row(repo, year_begin, year_end, today, open, closed, markdown_url);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn print_period_row(
    repo: &str,
    begin: NaiveDate,
    end: NaiveDate,
    today: NaiveDate,
    open: u64,
    closed: u64,
    markdown_url: bool,
) {
    let link_target =
        format!("https://github.com/{repo}/issues?q=is%3Aissue%20created%3A{begin}..{end}");
    let url = if markdown_url {
        format!("[{begin}..{end}]({link_target})")
    } else {
        link_target
    };

    println!("{url}\t{today}\t{begin}\t{end}\t{open}\t{closed}");
}
