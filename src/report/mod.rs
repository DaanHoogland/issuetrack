mod period;

pub use period::{
    run_monthly_period, run_period, run_weekly_period, run_weekly_since, run_yearly_period,
};

use anyhow::Result;
use chrono::NaiveDate;
use serde_json::Value;

use crate::ctx::Ctx;
use crate::fields::{Column, Field, FieldSpec, FieldsConfig};
use crate::gh;
use crate::output::{Cell, emit_cells};
use crate::repos::{item_repo, repo_qualifiers};

/// The weekly/monthly/yearly commands' columns, in default order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PeriodField {
    Url,
    Date,
    Begin,
    End,
    Open,
    Closed,
}

impl Field for PeriodField {
    const ALL: &'static [Self] = &[
        PeriodField::Url,
        PeriodField::Date,
        PeriodField::Begin,
        PeriodField::End,
        PeriodField::Open,
        PeriodField::Closed,
    ];
    const COMMANDS: &'static str = "weekly/monthly/yearly";
    const CONFIG_KEY: &'static str = "period";

    fn name(self) -> &'static str {
        match self {
            PeriodField::Url => "url",
            PeriodField::Date => "date",
            PeriodField::Begin => "begin",
            PeriodField::End => "end",
            PeriodField::Open => "open",
            PeriodField::Closed => "closed",
        }
    }

    fn letter(self) -> char {
        match self {
            PeriodField::Url => 'u',
            PeriodField::Date => 'd',
            PeriodField::Begin => 'b',
            PeriodField::End => 'e',
            PeriodField::Open => 'o',
            PeriodField::Closed => 'c',
        }
    }

    fn configured_spec(fields_config: &FieldsConfig) -> Option<&FieldSpec> {
        fields_config.period.as_ref()
    }
}

/// One repo's issue counts for one period. `closed` means created in the
/// period and closed *now*, not closed during the period.
struct PeriodRow<'a> {
    repo: &'a str,
    begin: NaiveDate,
    end: NaiveDate,
    open: u64,
    closed: u64,
}

/// Fetches every issue created in `span_begin..span_end` across all
/// repos in a single (paginated) search, warning when the combined result
/// hits the search API's 1000-result cap. `unit` names the sub-periods the
/// caller will bucket into, for the warning text.
fn fetch_span(
    ctx: &Ctx,
    span_begin: NaiveDate,
    span_end: NaiveDate,
    unit: &str,
) -> Result<Vec<Value>> {
    let query = format!(
        "{} is:issue created:{span_begin}..{span_end}",
        repo_qualifiers(&ctx.repos)
    );
    let items = gh::all_items(&query, ctx.token())?;
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
    ctx: &Ctx,
    columns: &[Column<PeriodField>],
    items: &[Value],
    mut periods: Vec<(NaiveDate, NaiveDate)>,
    chronological: bool,
) {
    if !chronological {
        periods.reverse();
    }
    for (begin, end) in periods {
        for repo in &ctx.repos {
            let (open, closed) = counts_in_range(items, repo, begin, end);
            let row = PeriodRow { repo, begin, end, open, closed };
            print_period_row(ctx, columns, &row);
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
        let Some(created) = gh::created_date(item) else {
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

fn print_period_row(ctx: &Ctx, columns: &[Column<PeriodField>], row: &PeriodRow) {
    let cells: Vec<Cell> = columns
        .iter()
        .map(|column| match column {
            Column::Blank => Cell::empty(),
            Column::Field(field) => Cell::plain(period_cell_text(ctx, *field, row)),
        })
        .collect();
    emit_cells(&cells, false);
}

fn period_cell_text(ctx: &Ctx, field: PeriodField, row: &PeriodRow) -> String {
    match field {
        PeriodField::Url => period_link(ctx, row),
        PeriodField::Date => ctx.today.to_string(),
        PeriodField::Begin => row.begin.to_string(),
        PeriodField::End => row.end.to_string(),
        PeriodField::Open => row.open.to_string(),
        PeriodField::Closed => row.closed.to_string(),
    }
}

/// The period's GitHub issue search, labelled `begin..end` — prefixed with
/// the repo when several are listed, since a markdown/hyperlink label
/// otherwise wouldn't say which repo the row is for.
fn period_link(ctx: &Ctx, row: &PeriodRow) -> String {
    let PeriodRow { repo, begin, end, .. } = row;
    let search_url =
        format!("https://github.com/{repo}/issues?q=is%3Aissue%20created%3A{begin}..{end}");
    let label = if ctx.repos.len() > 1 {
        format!("{repo} {begin}..{end}")
    } else {
        format!("{begin}..{end}")
    };
    ctx.link_style.format(&label, &search_url)
}
