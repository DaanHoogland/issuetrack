mod cli;
mod config;
mod ctx;
mod dates;
mod gh;
mod individual;
mod output;
mod report;
mod repos;
mod state;

use anyhow::Result;
use chrono::Local;
use clap::Parser;

use cli::{Cli, Command};
use ctx::Ctx;
use dates::{
    last_completed_week, parse_slash_date, resolve_implicit, resolve_month, resolve_period,
    resolve_year,
};
use output::{COPY_BUFFER, LinkStyle, copy_to_clipboard};
use repos::resolve_repos;

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

    let ctx = Ctx {
        repos,
        config_path,
        state_path,
        today,
        token,
        link_style,
    };

    let copy = cli.copy;
    if copy {
        COPY_BUFFER.with(|b| *b.borrow_mut() = Some(String::new()));
    }

    let result = run(cli, &ctx);

    if copy {
        let buf = COPY_BUFFER.with(|b| b.borrow_mut().take()).unwrap_or_default();
        if !buf.is_empty() {
            copy_to_clipboard(&buf)?;
            eprintln!("copied {} row(s) to the clipboard", buf.lines().count());
        }
    }

    result
}

fn run(cli: Cli, ctx: &Ctx) -> Result<()> {
    let today = ctx.today;
    match cli.command {
        Some(Command::InitConfig) => config::init_config(&ctx.config_path),
        Some(Command::Individual {
            since,
            until,
            no_state,
            username,
            full_username,
        }) => individual::run_individual(ctx, since, until, no_state, username, full_username),
        Some(Command::Weekly { over, since_date: Some(s) }) => {
            let date = parse_slash_date(&s)?;
            report::run_weekly_since(ctx, date, over.chronological)
        }
        Some(Command::Weekly { over, since_date: None }) => {
            let chronological = over.chronological;
            let (begin, end) = resolve_period(over, today, |t| Ok(last_completed_week(t)))?;
            report::run_weekly_period(ctx, begin, end, chronological)
        }
        Some(Command::Monthly { over, first, second }) => {
            let chronological = over.chronological;
            let (begin, end) = resolve_period(over, today, move |t| {
                resolve_month(t, first.clone(), second.clone())
            })?;
            report::run_monthly_period(ctx, begin, end, chronological)
        }
        Some(Command::Yearly { over, n }) => {
            let chronological = over.chronological;
            let (begin, end) = resolve_period(over, today, move |t| Ok(resolve_year(t, n)))?;
            report::run_yearly_period(ctx, begin, end, chronological)
        }
        None => {
            let (begin, end) = resolve_implicit(today, cli.first, cli.second)?;
            report::run_period(ctx, begin, end)
        }
    }
}
