use anyhow::Result;
use chrono::{Duration, NaiveDate};

use crate::ctx::Ctx;
use crate::dates::parse_date;
use crate::output::{LinkStyle, emit, emit_rich, html_escape};
use crate::repos::{item_repo, repo_qualifiers};
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
    let mut rows: Vec<Row> = Vec::new();
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
        // `None` for --username, which shows the bare name with no link.
        let user_link = (!username).then(|| format!("https://github.com/{login}"));
        rows.push(Row {
            opened,
            url: url.to_string(),
            title: title.to_string(),
            display_name,
            user_link,
            node_id: item.get("node_id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            repo: item_repo(item).to_string(),
        });
    }
    rows.sort_by(|a, b| b.opened.cmp(&a.opened));

    // One batched GraphQL lookup for every listed issue's linked PRs.
    let node_ids: Vec<&str> = rows
        .iter()
        .map(|r| r.node_id.as_str())
        .filter(|id| !id.is_empty())
        .collect();
    let prs = gh::linked_prs(&node_ids, ctx.token())?;

    let mut multi_pr_rows = 0;
    for row in &rows {
        let user = match &row.user_link {
            None => row.display_name.clone(),
            Some(link) => link_style.format(&row.display_name, link),
        };
        // Sheet columns: A url, B comment (title), C user, D opened, E action
        // (the PR references), F symbol — F is left for filling in by hand,
        // so the row stops at E.
        let prefix = format!("{}\t{}\t{user}\t{}", row.url, row.title, row.opened);

        // Each linked PR as (text before the PR id, PR id, PR url), e.g.
        // ("PR by jmsperu: ", "#14815", ...). A PR that only mentions the
        // issue may still solve it without being linked as such — it's
        // flagged as "mentioned in" for checking.
        let entries: Vec<(String, String, &str)> = prs
            .get(&row.node_id)
            .into_iter()
            .flatten()
            .map(|pr| {
                // `#123` for a PR in the issue's own repo, else
                // `owner/name#123`.
                let id = if pr.repo.eq_ignore_ascii_case(&row.repo) {
                    format!("#{}", pr.number)
                } else {
                    format!("{}#{}", pr.repo, pr.number)
                };
                let lead = if pr.closing {
                    format!("PR by {}: ", pr.author)
                } else {
                    "mentioned in ".to_string()
                };
                (lead, id, pr.url.as_str())
            })
            .collect();

        let pr_field = match (link_style, entries.as_slice()) {
            (_, []) => "no code yet".to_string(),
            (LinkStyle::Plain, _) => entries
                .iter()
                .map(|(lead, id, _)| format!("{lead}{id}"))
                .collect::<Vec<_>>()
                .join("; "),
            (LinkStyle::Markdown, _) => entries
                .iter()
                .map(|(lead, id, url)| format!("{lead}[{id}]({url})"))
                .collect::<Vec<_>>()
                .join("; "),
            // One PR: a single =HYPERLINK() formula covers it.
            (LinkStyle::Hyperlink(_), [(lead, id, url)]) => {
                link_style.format(&format!("{lead}{id}"), url)
            }
            // Several PRs: a cell holds one formula at most, so the TSV gets
            // plain text; the links survive only in the --copy HTML version.
            (LinkStyle::Hyperlink(_), _) => {
                multi_pr_rows += 1;
                entries
                    .iter()
                    .map(|(lead, id, _)| format!("{lead}{id}"))
                    .collect::<Vec<_>>()
                    .join("; ")
            }
        };
        let line = format!("{prefix}\t{pr_field}");

        if let LinkStyle::Hyperlink(_) = link_style {
            // HTML version for the clipboard: real links instead of formulas,
            // and every PR id linked inside the one PR cell (rich text, like
            // a cell edited by hand in Sheets).
            let td = |inner: String| format!("<td>{inner}</td>");
            let user_html = match &row.user_link {
                None => html_escape(&row.display_name),
                Some(link) => format!(
                    "<a href=\"{}\">{}</a>",
                    html_escape(link),
                    html_escape(&row.display_name)
                ),
            };
            let pr_html = if entries.is_empty() {
                "no code yet".to_string()
            } else {
                entries
                    .iter()
                    .map(|(lead, id, url)| {
                        format!(
                            "{}<a href=\"{}\">{}</a>",
                            html_escape(lead),
                            html_escape(url),
                            html_escape(id)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            };
            let html = [
                td(html_escape(&row.url)),
                td(html_escape(&row.title)),
                td(user_html),
                td(row.opened.to_string()),
                td(pr_html),
            ]
            .concat();
            emit_rich(&line, &html);
        } else {
            emit(&line);
        }
    }
    if multi_pr_rows > 0 && !ctx.copy {
        eprintln!(
            "{multi_pr_rows} row(s) have several PRs; a formula can link only one, so their \
             PR cell is plain text — use --copy to paste them with every PR linked"
        );
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

/// One listed issue, kept in parts until output so the TSV and the --copy
/// HTML version can each render it their own way.
struct Row {
    opened: NaiveDate,
    url: String,
    title: String,
    /// The reporter's login, or full name with --full-username.
    display_name: String,
    /// The reporter's profile url; `None` with --username.
    user_link: Option<String>,
    /// The issue's GraphQL node id, for the linked-PR lookup.
    node_id: String,
    /// The issue's `owner/name`.
    repo: String,
}
