mod config;
mod gh;
mod state;

use anyhow::{Context, Result};
use chrono::{Datelike, Duration, Local, NaiveDate};
use clap::{Parser, Subcommand};
use serde_json::Value;
use std::path::PathBuf;

/// How to render a url column: as a plain url, a markdown link, or a Google
/// Sheets `=HYPERLINK()` formula. `--markdown-url` and `--hyperlink-url` are
/// mutually exclusive; neither given means `Plain`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LinkStyle {
    Plain,
    Markdown,
    Hyperlink(char),
}

impl LinkStyle {
    /// Renders `url` alone (`Plain`), or `url` described by `text`
    /// (`Markdown`/`Hyperlink`).
    fn format(self, text: &str, url: &str) -> String {
        match self {
            LinkStyle::Plain => url.to_string(),
            LinkStyle::Markdown => format!("[{text}]({url})"),
            LinkStyle::Hyperlink(sep) => format!("=HYPERLINK(\"{url}\"{sep} \"{text}\")"),
        }
    }
}

/// Parses a `--hyperlink-separator` value: exactly one character, and not a
/// double quote (which would break the formula's own quoting).
fn parse_hyperlink_separator(s: &str) -> std::result::Result<char, String> {
    let mut chars = s.chars();
    let c = chars.next().ok_or("must be a single character, not empty")?;
    if chars.next().is_some() {
        return Err(format!("must be a single character, got '{s}'"));
    }
    if c == '"' {
        return Err("cannot be a double quote — that would break the formula".to_string());
    }
    Ok(c)
}

/// Refresh helper for the ACSissues Google Sheet: pulls data from GitHub and
/// prints TSV rows ready to paste into the sheet. Does not touch the sheet
/// itself — no Google credentials involved.
#[derive(Parser)]
#[command(name = "issuetrack", version, about, infer_subcommands = true)]
struct Cli {
    /// GitHub repo(s) to query, as owner/name. Repeatable (`-r a/b -r c/d`)
    /// or comma-separated (`-r a/b,c/d`). Falls back to `repos` in
    /// config.toml, then to apache/cloudstack.
    #[arg(short = 'r', long, global = true, value_delimiter = ',')]
    repo: Vec<String>,

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
    /// Mutually exclusive with --hyperlink-url.
    #[arg(short = 'm', long, global = true, conflicts_with = "hyperlink_url")]
    markdown_url: bool,

    /// Format url columns as Google Sheets `=HYPERLINK("url", "text")`
    /// formulas instead of plain urls — same columns as --markdown-url, and
    /// ready to paste straight into a sheet cell. Mutually exclusive with
    /// --markdown-url.
    #[arg(short = 'H', long, global = true)]
    hyperlink_url: bool,

    /// Single-character argument separator inside `=HYPERLINK()` formulas
    /// (only matters with --hyperlink-url). Google Sheets requires `;`
    /// instead of `,` in locales with a comma decimal separator (Dutch,
    /// German, French, ...); Sheets rejects the formula if this doesn't
    /// match the sheet's locale. Quote it in most shells, e.g. `-P ';'`.
    /// Defaults to `,`, or `hyperlink_separator` in config.toml.
    #[arg(short = 'P', long, global = true, value_parser = parse_hyperlink_separator)]
    hyperlink_separator: Option<char>,

    /// Copy the output to the clipboard (macOS `pbcopy`) instead of printing
    /// it for you to select and copy by hand. Selecting tab-separated output
    /// from a terminal window is fragile — some terminals render tabs as
    /// spaces on copy, which then paste as one merged Sheets cell instead of
    /// several; piping through the real clipboard preserves the tab bytes.
    #[arg(short = 'y', long, global = true)]
    copy: bool,

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
    /// Print multi-row output oldest-first (chronological order) instead of
    /// the default newest-first.
    #[arg(short = 'C', long)]
    chronological: bool,
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
        /// profile url (overrides --markdown-url/--hyperlink-url for that
        /// column).
        #[arg(short = 'U', long)]
        username: bool,
        /// Look up each reporter's full GitHub display name and show it
        /// (instead of their login) wherever the reporter column has a
        /// separate display text: as the link text with --markdown-url or
        /// --hyperlink-url, or as the displayed name with --username. Falls
        /// back to the login for accounts with no display name set, and has
        /// no effect on the plain profile-url output (none of those flags given).
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

thread_local! {
    static COPY_BUFFER: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Prints a data row to stdout, and — when `--copy` is active — also appends
/// it to the in-memory buffer that gets flushed to the clipboard at the end
/// of `main`, so both `--copy` and plain stdout output stay available.
fn emit(line: &str) {
    println!("{line}");
    COPY_BUFFER.with(|b| {
        if let Some(buf) = b.borrow_mut().as_mut() {
            buf.push_str(line);
            buf.push('\n');
        }
    });
}

/// Pipes `text` to `pbcopy`, replacing the current clipboard contents.
fn copy_to_clipboard(text: &str) -> Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new("pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .context("failed to run `pbcopy` — is this macOS?")?;
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(text.as_bytes())
        .context("failed to write to pbcopy")?;
    let status = child.wait().context("pbcopy did not exit cleanly")?;
    if !status.success() {
        anyhow::bail!("pbcopy exited with {status}");
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config_path = cli.config.clone().unwrap_or_else(config::default_path);
    let state_path = cli.state.clone().unwrap_or_else(state::default_path);
    let today = Local::now().date_naive();
    let file_defaults = config::load_defaults(&config_path);
    let repos = resolve_repos(&cli.repo, &file_defaults.repos)?;
    let token = cli.token.clone().or(file_defaults.github_token);
    let link_style = if cli.hyperlink_url || file_defaults.hyperlink_url {
        let separator = cli.hyperlink_separator.unwrap_or(file_defaults.hyperlink_separator);
        LinkStyle::Hyperlink(separator)
    } else if cli.markdown_url || file_defaults.markdown_url {
        LinkStyle::Markdown
    } else {
        LinkStyle::Plain
    };

    let copy = cli.copy;
    if copy {
        COPY_BUFFER.with(|b| *b.borrow_mut() = Some(String::new()));
    }

    let result = run(cli, &repos, &config_path, &state_path, today, token.as_deref(), link_style);

    if copy {
        let buf = COPY_BUFFER.with(|b| b.borrow_mut().take()).unwrap_or_default();
        if !buf.is_empty() {
            copy_to_clipboard(&buf)?;
            eprintln!("copied {} row(s) to the clipboard", buf.lines().count());
        }
    }

    result
}

const DEFAULT_REPO: &str = "apache/cloudstack";

/// The repos to query: `--repo` values if any were given, else `repos` from
/// config.toml, else `DEFAULT_REPO`. Each must look like `owner/name` —
/// anything else would be spliced into the search query as extra
/// qualifiers. Duplicates (case-insensitively) are dropped, keeping the
/// first spelling.
fn resolve_repos(cli_repos: &[String], config_repos: &[String]) -> Result<Vec<String>> {
    let source = if !cli_repos.is_empty() { cli_repos } else { config_repos };
    let mut repos: Vec<String> = Vec::new();
    for raw in source {
        let repo = raw.trim();
        if repo.is_empty() {
            continue;
        }
        let valid = repo.split('/').count() == 2
            && repo.split('/').all(|part| {
                !part.is_empty()
                    && part.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
            });
        if !valid {
            anyhow::bail!("invalid repo '{repo}', expected owner/name (e.g. apache/cloudstack)");
        }
        if !repos.iter().any(|r| r.eq_ignore_ascii_case(repo)) {
            repos.push(repo.to_string());
        }
    }
    if repos.is_empty() {
        repos.push(DEFAULT_REPO.to_string());
    }
    Ok(repos)
}

/// `repo:a/b repo:c/d` — GitHub search ORs repeated `repo:` qualifiers, so
/// one query (and one paginated fetch) covers every repo at once.
fn repo_qualifiers(repos: &[String]) -> String {
    repos
        .iter()
        .map(|r| format!("repo:{r}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The `owner/name` an issue-search item belongs to, from its
/// `repository_url` (`https://api.github.com/repos/owner/name`).
fn item_repo(item: &Value) -> &str {
    item.get("repository_url")
        .and_then(|v| v.as_str())
        .and_then(|u| u.strip_prefix("https://api.github.com/repos/"))
        .unwrap_or("")
}

#[allow(clippy::too_many_arguments)]
fn run(
    cli: Cli,
    repos: &[String],
    config_path: &PathBuf,
    state_path: &PathBuf,
    today: NaiveDate,
    token: Option<&str>,
    link_style: LinkStyle,
) -> Result<()> {
    match cli.command {
        Some(Command::InitConfig) => init_config(config_path),
        Some(Command::Individual {
            since,
            until,
            no_state,
            username,
            full_username,
        }) => run_individual(
            repos,
            config_path,
            state_path,
            since,
            until,
            no_state,
            today,
            token,
            link_style,
            username,
            full_username,
        ),
        Some(Command::Weekly { over, since_date: Some(s) }) => {
            let date = parse_slash_date(&s)?;
            run_weekly_since(repos, date, today, token, link_style, over.chronological)
        }
        Some(Command::Weekly { over, since_date: None }) => {
            let chronological = over.chronological;
            let (begin, end) = resolve_period(over, today, |t| Ok(last_completed_week(t)))?;
            run_weekly_period(repos, begin, end, today, token, link_style, chronological)
        }
        Some(Command::Monthly { over, first, second }) => {
            let chronological = over.chronological;
            let (begin, end) = resolve_period(over, today, move |t| {
                resolve_month(t, first.clone(), second.clone())
            })?;
            run_monthly_period(repos, begin, end, today, token, link_style, chronological)
        }
        Some(Command::Yearly { over, n }) => {
            let chronological = over.chronological;
            let (begin, end) = resolve_period(over, today, move |t| Ok(resolve_year(t, n)))?;
            run_yearly_period(repos, begin, end, today, token, link_style, chronological)
        }
        None => {
            let (begin, end) = resolve_implicit(today, cli.first, cli.second)?;
            run_period(repos, begin, end, today, token, link_style)
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
    repos: &[String],
    since_date: NaiveDate,
    today: NaiveDate,
    token: Option<&str>,
    link_style: LinkStyle,
    chronological: bool,
) -> Result<()> {
    let (_, last_end) = last_completed_week(today);
    let (span_begin, _) = week_containing(since_date);
    if span_begin > last_end {
        eprintln!("no completed weeks between {since_date} and {today}");
        return Ok(());
    }
    weekly_rows(repos, span_begin, last_end, today, token, link_style, chronological)
}

/// Prints one weekly row per Wednesday-through-Tuesday week overlapping
/// `begin..end` — a single row when the period is exactly one week (the
/// `weekly` command's default), several when `--since`/`--until` cover a
/// wider range.
#[allow(clippy::too_many_arguments)]
fn run_weekly_period(
    repos: &[String],
    begin: NaiveDate,
    end: NaiveDate,
    today: NaiveDate,
    token: Option<&str>,
    link_style: LinkStyle,
    chronological: bool,
) -> Result<()> {
    let max_year = today.year() + 1;
    if begin.year() < MIN_SANE_YEAR || end.year() > max_year {
        anyhow::bail!(
            "computed period {begin}..{end} is out of range ({MIN_SANE_YEAR}-{max_year})"
        );
    }
    let (span_begin, _) = week_containing(begin);
    let (_, span_end) = week_containing(end);
    weekly_rows(repos, span_begin, span_end, today, token, link_style, chronological)
}

/// Prints one row per completed Wednesday-through-Tuesday week from
/// `span_begin` through `span_end` (both assumed already week-aligned),
/// fetching every matching issue for the whole span in a single (paginated)
/// search rather than one search per week — the search API's own rate limit
/// is easy to trip otherwise on a multi-week backfill. Rows print
/// newest-first by default; pass `chronological` to print oldest-first.
#[allow(clippy::too_many_arguments)]
fn weekly_rows(
    repos: &[String],
    span_begin: NaiveDate,
    span_end: NaiveDate,
    today: NaiveDate,
    token: Option<&str>,
    link_style: LinkStyle,
    chronological: bool,
) -> Result<()> {
    let items = fetch_span(repos, span_begin, span_end, "weeks", token)?;

    let mut weeks = Vec::new();
    let (mut begin, mut end) = (span_begin, span_begin + Duration::days(6));
    while end <= span_end {
        weeks.push((begin, end));
        begin += Duration::days(7);
        end += Duration::days(7);
    }
    print_rows(repos, &items, weeks, today, link_style, chronological);
    Ok(())
}

/// Fetches every issue created in `span_begin..span_end` across all
/// `repos` in a single (paginated) search, warning when the combined result
/// hits the search API's 1000-result cap. `unit` names the sub-periods the
/// caller will bucket into, for the warning text.
fn fetch_span(
    repos: &[String],
    span_begin: NaiveDate,
    span_end: NaiveDate,
    unit: &str,
    token: Option<&str>,
) -> Result<Vec<Value>> {
    let query = format!(
        "{} is:issue created:{span_begin}..{span_end}",
        repo_qualifiers(repos)
    );
    let items = gh::all_items(&query, token)?;
    if items.len() >= 1000 {
        eprintln!(
            "warning: {span_begin}..{span_end} hit the search API's 1000-result cap — \
             some {unit}' counts may be undercounted; narrow the range (or the repo list) \
             if needed"
        );
    }
    Ok(items)
}

/// Prints one `print_period_row` line per period per repo — periods
/// newest-first unless `chronological` requests oldest-first, and within a
/// period the repos in the order given.
fn print_rows(
    repos: &[String],
    items: &[Value],
    mut periods: Vec<(NaiveDate, NaiveDate)>,
    today: NaiveDate,
    link_style: LinkStyle,
    chronological: bool,
) {
    if !chronological {
        periods.reverse();
    }
    let show_repo = repos.len() > 1;
    for (begin, end) in periods {
        for repo in repos {
            let (open, closed) = counts_in_range(items, repo, begin, end);
            print_period_row(repo, show_repo, begin, end, today, open, closed, link_style);
        }
    }
}

/// Counts, among already-fetched issue-search items, how many from `repo`
/// were created within `begin..end` and are currently open vs. closed.
fn counts_in_range(items: &[Value], repo: &str, begin: NaiveDate, end: NaiveDate) -> (u64, u64) {
    let (mut open, mut closed) = (0u64, 0u64);
    for item in items {
        if !item_repo(item).eq_ignore_ascii_case(repo) {
            continue;
        }
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
    repos: &[String],
    config_path: &PathBuf,
    state_path: &PathBuf,
    since: Option<String>,
    until: Option<String>,
    no_state: bool,
    today: NaiveDate,
    token: Option<&str>,
    link_style: LinkStyle,
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

    let query = format!(
        "{} is:issue created:{since_date}..{until_date}",
        repo_qualifiers(repos)
    );
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
        // Closed-as-duplicate issues aren't distinct tracking items — skip
        // them rather than listing a dupe alongside the issue it duplicates.
        if item.get("state_reason").and_then(|v| v.as_str()) == Some("duplicate") {
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
        // has a display text separate from the raw url (a markdown/hyperlink
        // link text, or the bare --username text). One lookup per unique
        // login per run.
        let display_name = if full_username && (username || link_style != LinkStyle::Plain) {
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
            link_style.format(&display_name, &user_link)
        };
        let row = format!("{url}\t{title}\t{user}\t{opened_date}\t\t");
        rows.push((opened, row));
    }
    rows.sort_by(|a, b| b.0.cmp(&a.0));

    for (_, row) in &rows {
        emit(row);
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

/// Prints one row per repo for `begin..end`, using two count-only searches
/// per repo (a combined query can't split counts by repo). `closed` means
/// created in the window and closed *now*, not closed during the window.
fn run_period(
    repos: &[String],
    begin: NaiveDate,
    end: NaiveDate,
    today: NaiveDate,
    token: Option<&str>,
    link_style: LinkStyle,
) -> Result<()> {
    let max_year = today.year() + 1;
    if begin.year() < MIN_SANE_YEAR || end.year() > max_year {
        anyhow::bail!(
            "computed period {begin}..{end} is out of range ({MIN_SANE_YEAR}-{max_year}) \
             — a bare number is interpreted as \"N months/years back\", not an absolute year; \
             for an absolute year use e.g. `yearly 2021` or `monthly aug 2021`"
        );
    }

    let show_repo = repos.len() > 1;
    for repo in repos {
        let open_q = format!("repo:{repo} is:issue is:open created:{begin}..{end}");
        let closed_q = format!("repo:{repo} is:issue is:closed created:{begin}..{end}");

        let open = gh::total_count(&open_q, token)?;
        let closed = gh::total_count(&closed_q, token)?;
        print_period_row(repo, show_repo, begin, end, today, open, closed, link_style);
    }

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
#[allow(clippy::too_many_arguments)]
fn run_monthly_period(
    repos: &[String],
    begin: NaiveDate,
    end: NaiveDate,
    today: NaiveDate,
    token: Option<&str>,
    link_style: LinkStyle,
    chronological: bool,
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

    let items = fetch_span(repos, span_begin, span_end, "months", token)?;

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
    print_rows(repos, &items, months, today, link_style, chronological);
    Ok(())
}

/// Prints one row per calendar year overlapping `begin..end` — a single row
/// when the period is exactly one year (the `yearly` command's default),
/// several when `--since`/`--until` cover a wider range.
#[allow(clippy::too_many_arguments)]
fn run_yearly_period(
    repos: &[String],
    begin: NaiveDate,
    end: NaiveDate,
    today: NaiveDate,
    token: Option<&str>,
    link_style: LinkStyle,
    chronological: bool,
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

    let items = fetch_span(repos, span_begin, span_end, "years", token)?;

    let mut years = Vec::new();
    for year in span_begin_year..=span_end_year {
        let year_begin = NaiveDate::from_ymd_opt(year, 1, 1).expect("valid Jan 1 date");
        let year_end = NaiveDate::from_ymd_opt(year, 12, 31).expect("valid Dec 31 date");
        years.push((year_begin, year_end));
    }
    print_rows(repos, &items, years, today, link_style, chronological);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn print_period_row(
    repo: &str,
    show_repo: bool,
    begin: NaiveDate,
    end: NaiveDate,
    today: NaiveDate,
    open: u64,
    closed: u64,
    link_style: LinkStyle,
) {
    let link_target =
        format!("https://github.com/{repo}/issues?q=is%3Aissue%20created%3A{begin}..{end}");
    // With several repos the plain url still names its repo, but a
    // markdown/hyperlink label wouldn't, so prefix it there.
    let label = if show_repo {
        format!("{repo} {begin}..{end}")
    } else {
        format!("{begin}..{end}")
    };
    let url = link_style.format(&label, &link_target);

    emit(&format!("{url}\t{today}\t{begin}\t{end}\t{open}\t{closed}"));
}
