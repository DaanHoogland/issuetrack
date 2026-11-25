use anyhow::Result;
use chrono::{Duration, NaiveDate};
use std::collections::HashMap;

use crate::ctx::Ctx;
use crate::dates::parse_date;
use crate::fields::{Column, Field, FieldSpec, FieldsConfig, shows_field};
use crate::gh::LinkedPr;
use crate::output::{Cell, LinkStyle, emit_cells, html_escape};
use crate::repos::{item_repo, repo_qualifiers};
use crate::{config, gh, state};

/// The `individual` command's columns, in default order — matching the
/// ACSissues sheet's columns A–E.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IssueField {
    Url,
    Comment,
    User,
    Opened,
    Action,
}

impl Field for IssueField {
    const ALL: &'static [Self] = &[
        IssueField::Url,
        IssueField::Comment,
        IssueField::User,
        IssueField::Opened,
        IssueField::Action,
    ];
    const COMMANDS: &'static str = "individual";
    const CONFIG_KEY: &'static str = "individual";

    fn name(self) -> &'static str {
        match self {
            IssueField::Url => "url",
            IssueField::Comment => "comment",
            IssueField::User => "user",
            IssueField::Opened => "opened",
            IssueField::Action => "action",
        }
    }

    fn letter(self) -> char {
        match self {
            IssueField::Url => 'u',
            IssueField::Comment => 'c',
            IssueField::User => 'r',
            IssueField::Opened => 'o',
            IssueField::Action => 'a',
        }
    }

    fn configured_spec(fields_config: &FieldsConfig) -> Option<&FieldSpec> {
        fields_config.individual.as_ref()
    }
}

/// One listed issue, kept in parts until output so each column can render
/// it as TSV text and as HTML.
struct IssueRow {
    opened: NaiveDate,
    url: String,
    title: String,
    /// The reporter's login, or full name with --full-username.
    reporter_name: String,
    /// `None` with --username, which shows the bare name.
    reporter_profile_url: Option<String>,
    graphql_node_id: String,
    /// `owner/name`
    repo: String,
}

/// A linked PR as shown in the action column: `PR by jmsperu: #14815` or
/// `mentioned in #14640`, where only the PR id is linked.
struct PrReference {
    label_before_id: String,
    id: String,
    url: String,
}

impl PrReference {
    fn for_issue(pr: &LinkedPr, issue_repo: &str) -> PrReference {
        let id = if pr.repo.eq_ignore_ascii_case(issue_repo) {
            format!("#{}", pr.number)
        } else {
            format!("{}#{}", pr.repo, pr.number)
        };
        // A PR that only mentions the issue may still solve it without being
        // linked as such — "mentioned in" flags it for checking.
        let label_before_id = if pr.closing {
            format!("PR by {}: ", pr.author)
        } else {
            "mentioned in ".to_string()
        };
        PrReference {
            label_before_id,
            id,
            url: pr.url.clone(),
        }
    }

    fn plain_text(&self) -> String {
        format!("{}{}", self.label_before_id, self.id)
    }

    fn markdown(&self) -> String {
        format!("{}[{}]({})", self.label_before_id, self.id, self.url)
    }

    fn html(&self) -> String {
        format!(
            "{}<a href=\"{}\">{}</a>",
            html_escape(&self.label_before_id),
            html_escape(&self.url),
            html_escape(&self.id)
        )
    }
}

const NO_PR_TEXT: &str = "no code yet";
const PR_REFERENCE_SEPARATOR: &str = "; ";

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
    let columns = ctx.columns::<IssueField>()?;
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

    // Full names only show where the reporter column has display text apart
    // from the raw profile url.
    let look_up_full_names = full_username
        && shows_field(&columns, IssueField::User)
        && (username || link_style != LinkStyle::Plain);
    let mut full_names_by_login: HashMap<String, Option<String>> = HashMap::new();

    let mut rows: Vec<IssueRow> = Vec::new();
    for item in &items {
        let login = item
            .get("user")
            .and_then(|u| u.get("login"))
            .and_then(|l| l.as_str())
            .unwrap_or("");
        if login.is_empty() || !cfg.is_external(login) {
            continue;
        }
        let closed_as_duplicate =
            item.get("state_reason").and_then(|v| v.as_str()) == Some("duplicate");
        if closed_as_duplicate {
            continue;
        }

        let reporter_name = if look_up_full_names {
            full_names_by_login
                .entry(login.to_string())
                .or_insert_with(|| gh::user_full_name(login, ctx.token()).unwrap_or(None))
                .clone()
                .unwrap_or_else(|| login.to_string())
        } else {
            login.to_string()
        };
        let text_of = |key: &str| item.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string();
        rows.push(IssueRow {
            opened: gh::created_date(item).unwrap_or(today),
            url: text_of("html_url"),
            title: text_of("title"),
            reporter_name,
            reporter_profile_url: (!username).then(|| format!("https://github.com/{login}")),
            graphql_node_id: text_of("node_id"),
            repo: item_repo(item).to_string(),
        });
    }
    rows.sort_by_key(|row| std::cmp::Reverse(row.opened));

    let linked_prs_by_node_id = if shows_field(&columns, IssueField::Action) {
        let node_ids: Vec<&str> = rows
            .iter()
            .map(|row| row.graphql_node_id.as_str())
            .filter(|id| !id.is_empty())
            .collect();
        gh::linked_prs(&node_ids, ctx.token())?
    } else {
        HashMap::new()
    };

    let clipboard_needs_html = matches!(link_style, LinkStyle::Hyperlink(_));
    let mut rows_with_unlinked_prs = 0;
    for row in &rows {
        let pr_references: Vec<PrReference> = linked_prs_by_node_id
            .get(&row.graphql_node_id)
            .into_iter()
            .flatten()
            .map(|pr| PrReference::for_issue(pr, &row.repo))
            .collect();
        let pr_links_lost_in_tsv =
            clipboard_needs_html && pr_references.len() > 1 && shows_field(&columns, IssueField::Action);
        if pr_links_lost_in_tsv {
            rows_with_unlinked_prs += 1;
        }

        let cells: Vec<Cell> = columns
            .iter()
            .map(|column| match column {
                Column::Blank => Cell::empty(),
                Column::Field(field) => issue_cell(*field, row, &pr_references, link_style),
            })
            .collect();
        emit_cells(&cells, clipboard_needs_html);
    }
    if rows_with_unlinked_prs > 0 && !ctx.copy {
        eprintln!(
            "{rows_with_unlinked_prs} row(s) have several PRs; a formula can link only one, so \
             their PR cell is plain text — use --copy to paste them with every PR linked"
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

fn issue_cell(
    field: IssueField,
    row: &IssueRow,
    pr_references: &[PrReference],
    link_style: LinkStyle,
) -> Cell {
    match field {
        IssueField::Url => Cell::plain(&row.url),
        IssueField::Comment => Cell::plain(&row.title),
        IssueField::Opened => Cell::plain(row.opened.to_string()),
        IssueField::User => reporter_cell(row, link_style),
        IssueField::Action => pr_references_cell(pr_references, link_style),
    }
}

fn reporter_cell(row: &IssueRow, link_style: LinkStyle) -> Cell {
    match &row.reporter_profile_url {
        None => Cell::plain(&row.reporter_name),
        Some(profile_url) => Cell {
            text: link_style.format(&row.reporter_name, profile_url),
            html: format!(
                "<a href=\"{}\">{}</a>",
                html_escape(profile_url),
                html_escape(&row.reporter_name)
            ),
        },
    }
}

/// The TSV text links what it can: every PR id as markdown, or — since a
/// cell holds one `=HYPERLINK()` formula at most — a lone PR as a formula
/// and several as plain text. The HTML links every PR id.
fn pr_references_cell(pr_references: &[PrReference], link_style: LinkStyle) -> Cell {
    let joined = |render: fn(&PrReference) -> String| {
        pr_references
            .iter()
            .map(render)
            .collect::<Vec<_>>()
            .join(PR_REFERENCE_SEPARATOR)
    };
    let text = match (link_style, pr_references) {
        (_, []) => NO_PR_TEXT.to_string(),
        (LinkStyle::Markdown, _) => joined(PrReference::markdown),
        (LinkStyle::Hyperlink(_), [only_pr]) => link_style.format(&only_pr.plain_text(), &only_pr.url),
        (LinkStyle::Plain | LinkStyle::Hyperlink(_), _) => joined(PrReference::plain_text),
    };
    let html = if pr_references.is_empty() {
        NO_PR_TEXT.to_string()
    } else {
        joined(PrReference::html)
    };
    Cell { text, html }
}
