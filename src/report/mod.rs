mod period;

pub use period::{
    run_monthly_period, run_period, run_weekly_period, run_weekly_since, run_yearly_period,
};

use anyhow::Result;
use chrono::NaiveDate;
use serde_json::Value;

use crate::ctx::Ctx;
use crate::gh;
use crate::output::emit;
use crate::repos::{item_repo, repo_qualifiers};

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
            print_period_row(ctx, repo, begin, end, open, closed);
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

fn print_period_row(
    ctx: &Ctx,
    repo: &str,
    begin: NaiveDate,
    end: NaiveDate,
    open: u64,
    closed: u64,
) {
    let today = ctx.today;
    let link_target =
        format!("https://github.com/{repo}/issues?q=is%3Aissue%20created%3A{begin}..{end}");
    // With several repos the plain url still names its repo, but a
    // markdown/hyperlink label wouldn't, so prefix it there.
    let label = if ctx.repos.len() > 1 {
        format!("{repo} {begin}..{end}")
    } else {
        format!("{begin}..{end}")
    };
    let url = ctx.link_style.format(&label, &link_target);

    emit(&format!("{url}\t{today}\t{begin}\t{end}\t{open}\t{closed}"));
}
