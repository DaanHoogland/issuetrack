mod config;
mod gh;
mod state;

use anyhow::{Context, Result};
use chrono::{Datelike, Duration, Local, NaiveDate};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Refresh helper for the ACSissues Google Sheet: pulls data from GitHub and
/// prints TSV rows ready to paste into the sheet. Does not touch the sheet
/// itself — no Google credentials involved.
#[derive(Parser)]
#[command(name = "acsissues", version, about)]
struct Cli {
    /// GitHub repo to query
    #[arg(long, global = true, default_value = "apache/cloudstack")]
    repo: String,

    /// Path to config.toml (excluded ShapeBlue users / extra bots)
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Path to the state file tracking the last "individual" run date
    #[arg(long, global = true)]
    state: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

/// Manual override of a period's begin/end date, for backfilling.
#[derive(clap::Args)]
struct PeriodOverride {
    /// Override the period's begin date (YYYY-MM-DD). Requires --until too.
    #[arg(long, requires = "until")]
    since: Option<String>,
    /// Override the period's end date (YYYY-MM-DD). Requires --since too.
    #[arg(long, requires = "since")]
    until: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// TSV rows (url, title, user, opened date) for new external-party issues,
    /// for the ACSissues tab. Blank status/action columns are left for you to fill in.
    Individual {
        /// Start date (YYYY-MM-DD). Defaults to the last recorded run, or 7 days ago.
        #[arg(long)]
        since: Option<String>,
        /// End date (YYYY-MM-DD). Defaults to today.
        #[arg(long)]
        until: Option<String>,
        /// Don't read or update the state file.
        #[arg(long)]
        no_state: bool,
    },
    /// One summary TSV row (begin, end, open, closed, total, date, duration,
    /// url, rate) for the Weekly tab, covering the most recently completed
    /// Wednesday-through-Tuesday window.
    Weekly {
        #[command(flatten)]
        over: PeriodOverride,
    },
    /// Same shape as `weekly`, covering the most recently completed calendar month.
    Monthly {
        #[command(flatten)]
        over: PeriodOverride,
    },
    /// Same shape as `weekly`, covering the most recently completed calendar year.
    Yearly {
        #[command(flatten)]
        over: PeriodOverride,
    },
    /// Write a starter config.toml if one doesn't already exist.
    InitConfig,
}

fn parse_date(s: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .with_context(|| format!("invalid date '{s}', expected YYYY-MM-DD"))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config_path = cli.config.clone().unwrap_or_else(config::default_path);
    let state_path = cli.state.clone().unwrap_or_else(state::default_path);
    let today = Local::now().date_naive();

    match cli.command {
        Command::InitConfig => init_config(&config_path),
        Command::Individual {
            since,
            until,
            no_state,
        } => run_individual(&cli.repo, &config_path, &state_path, since, until, no_state, today),
        Command::Weekly { over } => {
            let (begin, end) = resolve_period(over, today, last_completed_week)?;
            run_period(&cli.repo, begin, end, today)
        }
        Command::Monthly { over } => {
            let (begin, end) = resolve_period(over, today, last_completed_month)?;
            run_period(&cli.repo, begin, end, today)
        }
        Command::Yearly { over } => {
            let (begin, end) = resolve_period(over, today, last_completed_year)?;
            run_period(&cli.repo, begin, end, today)
        }
    }
}

fn resolve_period(
    over: PeriodOverride,
    today: NaiveDate,
    default_fn: impl Fn(NaiveDate) -> (NaiveDate, NaiveDate),
) -> Result<(NaiveDate, NaiveDate)> {
    match (over.since, over.until) {
        (Some(s), Some(u)) => Ok((parse_date(&s)?, parse_date(&u)?)),
        _ => Ok(default_fn(today)),
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

fn last_day_of_month(year: i32, month: u32) -> NaiveDate {
    let (next_year, next_month) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .expect("valid next-month date")
        - Duration::days(1)
}

/// Most recently completed calendar month before `today`'s month.
fn last_completed_month(today: NaiveDate) -> (NaiveDate, NaiveDate) {
    let (year, month) = if today.month() == 1 {
        (today.year() - 1, 12)
    } else {
        (today.year(), today.month() - 1)
    };
    let begin = NaiveDate::from_ymd_opt(year, month, 1).expect("valid first-of-month date");
    let end = last_day_of_month(year, month);
    (begin, end)
}

/// Most recently completed calendar year before `today`'s year.
fn last_completed_year(today: NaiveDate) -> (NaiveDate, NaiveDate) {
    let year = today.year() - 1;
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
    let items = gh::all_items(&query)?;

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
        let row = format!("{url}\t{title}\t{login}\t{opened_date}\t\t");
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

fn run_period(repo: &str, begin: NaiveDate, end: NaiveDate, today: NaiveDate) -> Result<()> {
    let opened_q = format!("repo:{repo} is:issue created:{begin}..{end}");
    let closed_q = format!("repo:{repo} is:issue closed:{begin}..{end}");

    let open = gh::total_count(&opened_q)?;
    let closed = gh::total_count(&closed_q)?;
    let link_target =
        format!("https://github.com/{repo}/issues?q=is%3Aissue%20created%3A{begin}..{end}");
    let url = format!("[{begin}..{end}]({link_target})");

    println!("{url}\t{today}\t{begin}\t{end}\t{open}\t{closed}");

    Ok(())
}
