use anyhow::Result;
use chrono::{Duration, NaiveDate};
use serde_json::Value;
use std::collections::HashMap;

use crate::cli::IndividualArgs;
use crate::config::Config;
use crate::ctx::Ctx;
use crate::dates::parse_date;
use crate::fields::{Column, Field, FieldSpec, FieldsConfig, shows_field};
use crate::gh;
use crate::gh::LinkedPr;
use crate::output::{Cell, LinkStyle, emit_cells, html_escape};
use crate::repos::{item_repo, repo_qualifiers};
use crate::state::State;

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

const DEFAULT_DAYS_BACK_WITHOUT_STATE: i64 = 7;
const NO_PR_TEXT: &str = "no code yet";
const PR_REFERENCE_SEPARATOR: &str = "; ";

struct IssueRow {
    opened: NaiveDate,
    url: String,
    title: String,
    /// The login, or the full name with --full-username.
    reporter_name: String,
    /// `None` with --username.
    reporter_profile_url: Option<String>,
    graphql_node_id: String,
    repo_owner_and_name: String,
}

/// A linked PR as shown in the action column, where only `id` is linked:
/// `PR by jmsperu: #14815` for a PR set to close the issue, `mentioned in
/// #14640` for one that only mentions it (and may solve it unannounced).
struct PrReference {
    label_before_id: String,
    id: String,
    url: String,
}

impl PrReference {
    fn for_issue(pr: &LinkedPr, issue_repo: &str) -> PrReference {
        let id = if pr.repo_owner_and_name.eq_ignore_ascii_case(issue_repo) {
            format!("#{}", pr.number)
        } else {
            format!("{}#{}", pr.repo_owner_and_name, pr.number)
        };
        let label_before_id = if pr.closes_issue {
            format!("PR by {}: ", pr.author_login)
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

/// Prints the external-party issues opened since the last run (or
/// `--since`), newest first, and records the run in the state file.
pub fn run_individual(ctx: &Ctx, args: IndividualArgs) -> Result<()> {
    let columns = ctx.columns::<IssueField>()?;
    let config = Config::load(&ctx.config_path)?;
    let mut state = if args.no_state {
        State::default()
    } else {
        State::load(&ctx.state_path)?
    };
    let (since, until) = issue_window(&args, &state, ctx.today)?;

    let query = format!(
        "{} is:issue created:{since}..{until}",
        repo_qualifiers(&ctx.repos)
    );
    let issues = gh::all_items(&query, ctx.token())?;
    let rows = external_issue_rows(ctx, &args, &columns, &config, &issues);
    let linked_prs_by_node_id = if shows_field(&columns, IssueField::Action) {
        linked_prs_of_rows(ctx, &rows)?
    } else {
        HashMap::new()
    };
    print_issue_rows(ctx, &columns, &rows, &linked_prs_by_node_id);
    eprintln!(
        "{} external-party issue(s) from {since} to {until}",
        rows.len()
    );

    if !args.no_state {
        state.last_individual = Some(until);
        state.save(&ctx.state_path)?;
    }
    Ok(())
}

/// `--since`/`--until` when given; else from the last recorded run (or a
/// week back) through today.
fn issue_window(
    args: &IndividualArgs,
    state: &State,
    today: NaiveDate,
) -> Result<(NaiveDate, NaiveDate)> {
    let until = match &args.until {
        Some(until) => parse_date(until)?,
        None => today,
    };
    let since = match &args.since {
        Some(since) => parse_date(since)?,
        None => state
            .last_individual
            .unwrap_or_else(|| today - Duration::days(DEFAULT_DAYS_BACK_WITHOUT_STATE)),
    };
    Ok((since, until))
}

/// Issues by external contributors, newest first, skipping issues GitHub
/// closed as a duplicate of another.
fn external_issue_rows(
    ctx: &Ctx,
    args: &IndividualArgs,
    columns: &[Column<IssueField>],
    config: &Config,
    issues: &[Value],
) -> Vec<IssueRow> {
    let reporter_column_shows_a_name = shows_field(columns, IssueField::User)
        && (args.username || ctx.link_style != LinkStyle::Plain);
    let look_up_full_names = args.full_username && reporter_column_shows_a_name;
    let mut full_names_by_login: HashMap<String, Option<String>> = HashMap::new();

    let mut rows: Vec<IssueRow> = Vec::new();
    for issue in issues {
        let text_of = |key: &str| issue[key].as_str().unwrap_or("").to_string();
        let login = issue["user"]["login"].as_str().unwrap_or("");
        let closed_as_duplicate = issue["state_reason"].as_str() == Some("duplicate");
        if login.is_empty() || !config.is_external_contributor(login) || closed_as_duplicate {
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
        rows.push(IssueRow {
            opened: gh::created_date(issue).unwrap_or(ctx.today),
            url: text_of("html_url"),
            title: text_of("title"),
            reporter_name,
            reporter_profile_url: (!args.username).then(|| format!("https://github.com/{login}")),
            graphql_node_id: text_of("node_id"),
            repo_owner_and_name: item_repo(issue).to_string(),
        });
    }
    rows.sort_by_key(|row| std::cmp::Reverse(row.opened));
    rows
}

fn linked_prs_of_rows(ctx: &Ctx, rows: &[IssueRow]) -> Result<HashMap<String, Vec<LinkedPr>>> {
    let node_ids: Vec<&str> = rows
        .iter()
        .map(|row| row.graphql_node_id.as_str())
        .filter(|node_id| !node_id.is_empty())
        .collect();
    gh::linked_prs(&node_ids, ctx.token())
}

fn print_issue_rows(
    ctx: &Ctx,
    columns: &[Column<IssueField>],
    rows: &[IssueRow],
    linked_prs_by_node_id: &HashMap<String, Vec<LinkedPr>>,
) {
    let clipboard_needs_html = matches!(ctx.link_style, LinkStyle::Hyperlink(_));
    let mut rows_with_pr_links_lost_in_tsv = 0;
    for row in rows {
        let pr_references: Vec<PrReference> = linked_prs_by_node_id
            .get(&row.graphql_node_id)
            .into_iter()
            .flatten()
            .map(|pr| PrReference::for_issue(pr, &row.repo_owner_and_name))
            .collect();
        if clipboard_needs_html && pr_references.len() > 1 {
            rows_with_pr_links_lost_in_tsv += 1;
        }
        let cells: Vec<Cell> = columns
            .iter()
            .map(|column| match column {
                Column::Blank => Cell::empty(),
                Column::Field(field) => issue_cell(*field, row, &pr_references, ctx.link_style),
            })
            .collect();
        emit_cells(&cells, clipboard_needs_html);
    }
    if rows_with_pr_links_lost_in_tsv > 0 && !ctx.copy {
        eprintln!(
            "{rows_with_pr_links_lost_in_tsv} row(s) have several PRs; a formula can link only \
             one, so their PR cell is plain text — use --copy to paste them with every PR linked"
        );
    }
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

/// The TSV text links what it can: every PR id as markdown, or — a cell
/// holding one `=HYPERLINK()` formula at most — a lone PR as a formula and
/// several as plain text. The HTML links every PR id.
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
        (LinkStyle::Hyperlink(_), [only_pr]) => {
            link_style.format(&only_pr.plain_text(), &only_pr.url)
        }
        (LinkStyle::Plain | LinkStyle::Hyperlink(_), _) => joined(PrReference::plain_text),
    };
    let html = if pr_references.is_empty() {
        NO_PR_TEXT.to_string()
    } else {
        joined(PrReference::html)
    };
    Cell { text, html }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linked_pr(number: u64, repo: &str, closes_issue: bool) -> LinkedPr {
        LinkedPr {
            repo_owner_and_name: repo.to_string(),
            number,
            url: format!("https://github.com/{repo}/pull/{number}"),
            author_login: "dev".to_string(),
            closes_issue,
        }
    }

    fn references(prs: &[LinkedPr]) -> Vec<PrReference> {
        prs.iter()
            .map(|pr| PrReference::for_issue(pr, "o/r"))
            .collect()
    }

    #[test]
    fn closing_and_mentioning_prs_are_labelled_differently() {
        let prs = [linked_pr(1, "o/r", true), linked_pr(2, "other/repo", false)];
        let cell = pr_references_cell(&references(&prs), LinkStyle::Plain);
        assert_eq!(cell.text, "PR by dev: #1; mentioned in other/repo#2");
    }

    #[test]
    fn a_hyperlink_formula_is_used_only_for_a_lone_pr() {
        let style = LinkStyle::Hyperlink(',');
        let one = pr_references_cell(&references(&[linked_pr(1, "o/r", true)]), style);
        assert_eq!(
            one.text,
            "=HYPERLINK(\"https://github.com/o/r/pull/1\", \"PR by dev: #1\")"
        );
        let two = [linked_pr(1, "o/r", true), linked_pr(2, "o/r", false)];
        let cell = pr_references_cell(&references(&two), style);
        assert_eq!(cell.text, "PR by dev: #1; mentioned in #2");
        assert_eq!(
            cell.html,
            "PR by dev: <a href=\"https://github.com/o/r/pull/1\">#1</a>; \
             mentioned in <a href=\"https://github.com/o/r/pull/2\">#2</a>"
        );
    }

    #[test]
    fn no_linked_pr_reads_no_code_yet() {
        let cell = pr_references_cell(&[], LinkStyle::Markdown);
        assert_eq!(
            (cell.text.as_str(), cell.html.as_str()),
            (NO_PR_TEXT, NO_PR_TEXT)
        );
    }
}
