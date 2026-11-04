use anyhow::Result;
use chrono::{Duration, NaiveDate};

use crate::ctx::Ctx;
use crate::dates::parse_date;
use crate::output::{LinkStyle, emit};
use crate::repos::repo_qualifiers;
use crate::{config, gh, state};

pub fn run_individual(
    ctx: &Ctx,
    since: Option<String>,
    until: Option<String>,
    no_state: bool,
    username: bool,
    full_username: bool,
) -> Result<()> {
    let today = ctx.today;
    let link_style = ctx.link_style;
    let cfg = config::Config::load(&ctx.config_path)?;
    let st = if no_state {
        state::State::default()
    } else {
        state::State::load(&ctx.state_path)?
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
        repo_qualifiers(&ctx.repos)
    );
    let items = gh::all_items(&query, ctx.token())?;

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
        let opened = gh::created_date(item).unwrap_or(today);

        // The name shown in place of the login, wherever the reporter column
        // has a display text separate from the raw url (a markdown/hyperlink
        // link text, or the bare --username text). One lookup per unique
        // login per run.
        let display_name = if full_username && (username || link_style != LinkStyle::Plain) {
            full_names
                .entry(login.to_string())
                .or_insert_with(|| gh::user_full_name(login, ctx.token()).unwrap_or(None))
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
        let row = format!("{url}\t{title}\t{user}\t{opened}\t\t");
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
        st.save(&ctx.state_path)?;
    }

    Ok(())
}
