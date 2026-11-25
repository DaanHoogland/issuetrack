use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use serde_json::Value;
use std::collections::HashMap;
use std::process::Command;

/// GitHub search returns at most this many results per query.
pub const SEARCH_RESULT_CAP: usize = 1000;
const SEARCH_PAGE_SIZE: u32 = 100;
const SEARCH_MAX_PAGES: u32 = SEARCH_RESULT_CAP as u32 / SEARCH_PAGE_SIZE;
const LINKED_PR_LOOKUP_BATCH_SIZE: usize = 50;
const DELETED_ACCOUNT_LOGIN: &str = "ghost";

/// Runs `gh api <args>` (as `token` when given, else `gh`'s own login) and
/// parses its JSON output. `what` names the call in error messages.
fn gh_api(args: &[&str], token: Option<&str>, what: &str) -> Result<Value> {
    let mut command = Command::new("gh");
    command.arg("api").args(args);
    if let Some(token) = token {
        command.env("GH_TOKEN", token);
    }
    let output = command
        .output()
        .context("failed to run `gh api` — is the gh CLI installed and on PATH?")?;
    if !output.status.success() {
        bail!(
            "gh api {what} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    serde_json::from_slice(&output.stdout)
        .with_context(|| format!("failed to parse gh api {what} JSON output"))
}

struct SearchPage {
    total_count: u64,
    items: Vec<Value>,
}

fn search_issues(
    query: &str,
    page: u32,
    page_size: u32,
    token: Option<&str>,
) -> Result<SearchPage> {
    let response = gh_api(
        &[
            "-X",
            "GET",
            "search/issues",
            "-f",
            &format!("q={query}"),
            "-f",
            &format!("per_page={page_size}"),
            "-f",
            &format!("page={page}"),
        ],
        token,
        "search/issues",
    )?;
    Ok(SearchPage {
        total_count: response["total_count"].as_u64().unwrap_or(0),
        items: response["items"].as_array().cloned().unwrap_or_default(),
    })
}

/// Total match count for a search query — accurate even past the 1000
/// results search lets you retrieve.
pub fn total_count(query: &str, token: Option<&str>) -> Result<u64> {
    Ok(search_issues(query, 1, 1, token)?.total_count)
}

/// All matching items for a search query, up to search's 1000-result cap.
pub fn all_items(query: &str, token: Option<&str>) -> Result<Vec<Value>> {
    let mut items = Vec::new();
    for page in 1..=SEARCH_MAX_PAGES {
        let search_page = search_issues(query, page, SEARCH_PAGE_SIZE, token)?;
        let last_page_reached = (search_page.items.len() as u32) < SEARCH_PAGE_SIZE;
        items.extend(search_page.items);
        if last_page_reached || items.len() as u64 >= search_page.total_count {
            break;
        }
    }
    Ok(items)
}

/// The date part of a search item's `created_at` timestamp
/// (`2026-10-29T14:03:00Z` -> 2026-10-29).
pub fn created_date(item: &Value) -> Option<NaiveDate> {
    let created_at = item["created_at"].as_str()?;
    let date_part = created_at.split('T').next().unwrap_or(created_at);
    NaiveDate::parse_from_str(date_part, "%Y-%m-%d").ok()
}

/// A GitHub account's public display name ("Jane Doe" for login
/// "janedoe"), or `None` when the account has none set.
pub fn user_full_name(login: &str, token: Option<&str>) -> Result<Option<String>> {
    let endpoint = format!("users/{login}");
    let profile = gh_api(&[&endpoint], token, &endpoint)?;
    Ok(profile["name"]
        .as_str()
        .filter(|name| !name.is_empty())
        .map(str::to_string))
}

pub struct LinkedPr {
    pub repo_owner_and_name: String,
    pub number: u64,
    pub url: String,
    pub author_login: String,
    /// Set to close the issue (a "fixes #N" keyword or a Development-sidebar
    /// link), as opposed to only mentioning it.
    pub closes_issue: bool,
}

/// Pull requests linked to each issue, keyed by the issue's GraphQL
/// `node_id` (as found on search items): first the PRs set to close it, then
/// other PRs that mention it, without duplicates.
///
/// Looks up `LINKED_PR_LOOKUP_BATCH_SIZE` issues per GraphQL call, which
/// doesn't count against the search API's ~30 requests/minute limit.
pub fn linked_prs(
    issue_node_ids: &[&str],
    token: Option<&str>,
) -> Result<HashMap<String, Vec<LinkedPr>>> {
    let mut linked_prs_by_issue = HashMap::new();
    for batch in issue_node_ids.chunks(LINKED_PR_LOOKUP_BATCH_SIZE) {
        let query_field = format!("query={}", linked_prs_query(batch));
        let response = gh_api(
            &["graphql", "-f", &query_field],
            token,
            "graphql (linked PRs)",
        )?;
        for issue in response["data"]["nodes"].as_array().into_iter().flatten() {
            if let Some(issue_node_id) = issue["id"].as_str() {
                linked_prs_by_issue.insert(issue_node_id.to_string(), linked_prs_of_issue(issue));
            }
        }
    }
    Ok(linked_prs_by_issue)
}

fn linked_prs_query(issue_node_ids: &[&str]) -> String {
    let quoted_ids = issue_node_ids
        .iter()
        .map(|id| serde_json::to_string(id).expect("a string serializes"))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "query {{ nodes(ids: [{quoted_ids}]) {{ ... on Issue {{ id \
         closedByPullRequestsReferences(first: 10, includeClosedPrs: true) {{ nodes {{ ...pr }} }} \
         timelineItems(first: 50, itemTypes: [CROSS_REFERENCED_EVENT, CONNECTED_EVENT]) {{ nodes {{ \
           ... on CrossReferencedEvent {{ source {{ ... on PullRequest {{ ...pr }} }} }} \
           ... on ConnectedEvent {{ subject {{ ... on PullRequest {{ ...pr }} }} }} \
         }} }} }} }} }} \
         fragment pr on PullRequest {{ number url author {{ login }} repository {{ nameWithOwner }} }}"
    )
}

/// Closing PRs are listed first, so a PR that both closes and mentions the
/// issue is kept as closing. Timeline events whose source is an issue rather
/// than a PR come back as empty objects and are skipped.
fn linked_prs_of_issue(issue: &Value) -> Vec<LinkedPr> {
    let closing_prs = issue["closedByPullRequestsReferences"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|pr| (pr, true));
    let mentioning_prs = issue["timelineItems"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|event| event.get("source").or_else(|| event.get("subject")))
        .map(|pr| (pr, false));

    let mut linked_prs: Vec<LinkedPr> = Vec::new();
    for (pr, closes_issue) in closing_prs.chain(mentioning_prs) {
        let Some(linked_pr) = parse_linked_pr(pr, closes_issue) else {
            continue;
        };
        let already_listed = linked_prs.iter().any(|listed| {
            listed.number == linked_pr.number
                && listed.repo_owner_and_name == linked_pr.repo_owner_and_name
        });
        if !already_listed {
            linked_prs.push(linked_pr);
        }
    }
    linked_prs
}

fn parse_linked_pr(pr: &Value, closes_issue: bool) -> Option<LinkedPr> {
    let text_at = |pointer: &str| pr.pointer(pointer).and_then(Value::as_str);
    Some(LinkedPr {
        number: pr["number"].as_u64()?,
        repo_owner_and_name: text_at("/repository/nameWithOwner")
            .unwrap_or("")
            .to_string(),
        url: text_at("/url").unwrap_or("").to_string(),
        author_login: text_at("/author/login")
            .unwrap_or(DELETED_ACCOUNT_LOGIN)
            .to_string(),
        closes_issue,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pr_json(number: u64, author: &str) -> Value {
        json!({
            "number": number,
            "url": format!("https://github.com/o/r/pull/{number}"),
            "author": { "login": author },
            "repository": { "nameWithOwner": "o/r" }
        })
    }

    #[test]
    fn closing_prs_come_first_and_win_over_mentions() {
        let issue = json!({
            "id": "I_1",
            "closedByPullRequestsReferences": { "nodes": [pr_json(2, "closer")] },
            "timelineItems": { "nodes": [
                { "source": pr_json(1, "mentioner") },
                { "source": {} },
                { "subject": pr_json(2, "closer") }
            ]}
        });
        let linked: Vec<(u64, bool)> = linked_prs_of_issue(&issue)
            .iter()
            .map(|pr| (pr.number, pr.closes_issue))
            .collect();
        assert_eq!(linked, vec![(2, true), (1, false)]);
    }

    #[test]
    fn a_pr_without_author_belongs_to_the_deleted_account() {
        let pr = json!({ "number": 7, "author": null });
        assert_eq!(parse_linked_pr(&pr, false).unwrap().author_login, "ghost");
    }
}
