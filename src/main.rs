mod cli;
mod config;
mod ctx;
mod dates;
mod fields;
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
use config::Config;
use ctx::Ctx;
use dates::{
    last_completed_week, parse_slash_date, resolve_implicit, resolve_month, resolve_period,
    resolve_year,
};
use output::{LinkStyle, finish_copy_capture, start_copy_capture};
use repos::resolve_repos;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let ctx = build_context(&cli)?;

    if ctx.copy {
        start_copy_capture();
    }
    let command_result = run(cli, &ctx);
    if ctx.copy {
        finish_copy_capture()?;
    }
    command_result
}

/// Resolves run-wide settings: command-line options first, then
/// config.toml, then built-in defaults.
fn build_context(cli: &Cli) -> Result<Ctx> {
    let config_path = cli.config.clone().unwrap_or_else(config::default_path);
    let file_defaults = config::load_defaults(&config_path);
    Ok(Ctx {
        repos: resolve_repos(&cli.repo, &file_defaults.repos)?,
        state_path: cli.state.clone().unwrap_or_else(state::default_path),
        today: Local::now().date_naive(),
        token: cli.token.clone().or(file_defaults.github_token.clone()),
        link_style: resolve_link_style(cli, &file_defaults),
        copy: cli.copy,
        command_line_fields: cli.fields.clone(),
        configured_fields: file_defaults.fields.clone(),
        config_path,
    })
}

fn resolve_link_style(cli: &Cli, file_defaults: &Config) -> LinkStyle {
    if cli.hyperlink_url || file_defaults.hyperlink_url {
        let separator = cli
            .hyperlink_separator
            .unwrap_or(file_defaults.hyperlink_separator);
        LinkStyle::Hyperlink(separator)
    } else if cli.markdown_url || file_defaults.markdown_url {
        LinkStyle::Markdown
    } else {
        LinkStyle::Plain
    }
}

fn run(cli: Cli, ctx: &Ctx) -> Result<()> {
    let today = ctx.today;
    match cli.command {
        Some(Command::InitConfig) => config::init_config(&ctx.config_path),
        Some(Command::Individual(args)) => individual::run_individual(ctx, args),
        Some(Command::Weekly {
            over,
            since_date: Some(since_date),
        }) => {
            let since_date = parse_slash_date(&since_date)?;
            report::run_weekly_since(ctx, since_date, over.chronological)
        }
        Some(Command::Weekly {
            over,
            since_date: None,
        }) => {
            let chronological = over.chronological;
            let (begin, end) = resolve_period(over, today, |today| Ok(last_completed_week(today)))?;
            report::run_weekly_period(ctx, begin, end, chronological)
        }
        Some(Command::Monthly {
            over,
            first,
            second,
        }) => {
            let chronological = over.chronological;
            let (begin, end) = resolve_period(over, today, move |today| {
                resolve_month(today, first.clone(), second.clone())
            })?;
            report::run_monthly_period(ctx, begin, end, chronological)
        }
        Some(Command::Yearly {
            over,
            year_or_years_back,
        }) => {
            let chronological = over.chronological;
            let (begin, end) = resolve_period(over, today, move |today| {
                Ok(resolve_year(today, year_or_years_back))
            })?;
            report::run_yearly_period(ctx, begin, end, chronological)
        }
        None => {
            let (begin, end) = resolve_implicit(today, cli.first, cli.second)?;
            report::run_period(ctx, begin, end)
        }
    }
}
