use anyhow::{Context, Result};
use chrono::NaiveDate;
use serde_json::Value;
use std::process::Command;

pub struct SearchResult {
    pub total_count: u64,
    pub items: Vec<Value>,
}

fn search_issues(query: &str, page: u32, per_page: u32, token: Option<&str>) -> Result<SearchResult> {
    let mut cmd = Command::new("gh");
    cmd.args([
        "api",
        "-X",
        "GET",
        "search/issues",
        "-f",
        &format!("q={query}"),
        "-f",
        &format!("per_page={per_page}"),
        "-f",
        &format!("page={page}"),
    ]);
    if let Some(t) = token {
        cmd.env("GH_TOKEN", t);
    }
    let output = cmd
        .output()
        .context("failed to run `gh api` — is the gh CLI installed and on PATH?")?;

    if !output.status.success() {
        anyhow::bail!(
            "gh api search/issues failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let v: Value =
        serde_json::from_slice(&output.stdout).context("failed to parse gh api JSON output")?;

    let total_count = v.get("total_count").and_then(|c| c.as_u64()).unwrap_or(0);
    let items = v
        .get("items")
        .and_then(|i| i.as_array())
        .cloned()
        .unwrap_or_default();

    Ok(SearchResult { total_count, items })
}

/// Total match count for a search query (GitHub search API caps this at 1000
/// for actual item retrieval, but total_count itself is accurate).
pub fn total_count(query: &str, token: Option<&str>) -> Result<u64> {
    Ok(search_issues(query, 1, 1, token)?.total_count)
}

/// All matching items for a search query, paginated up to GitHub search's
/// 1000-result cap (10 pages of 100).
pub fn all_items(query: &str, token: Option<&str>) -> Result<Vec<Value>> {
    let mut items = Vec::new();
    let per_page = 100;
    let mut page = 1;
    loop {
        let res = search_issues(query, page, per_page, token)?;
        let got = res.items.len() as u32;
        let total = res.total_count;
        items.extend(res.items);
        if got < per_page || items.len() as u64 >= total || page >= 10 {
            break;
        }
        page += 1;
    }
    Ok(items)
}

/// The date part of a search item's `created_at` timestamp
/// (`2026-10-29T14:03:00Z` -> 2026-10-29), or `None` if it's missing or
/// malformed.
pub fn created_date(item: &Value) -> Option<NaiveDate> {
    let created_at = item.get("created_at")?.as_str()?;
    let date_str = created_at.split('T').next().unwrap_or(created_at);
    NaiveDate::parse_from_str(date_str, "%Y-%m-%d").ok()
}

/// A GitHub account's public display name (the "Name" field on its profile),
/// e.g. "Jane Doe" for login "janedoe". `None` if the account has no display
/// name set (common for accounts that only ever set a username).
pub fn user_full_name(login: &str, token: Option<&str>) -> Result<Option<String>> {
    let mut cmd = Command::new("gh");
    cmd.args(["api", &format!("users/{login}")]);
    if let Some(t) = token {
        cmd.env("GH_TOKEN", t);
    }
    let output = cmd
        .output()
        .context("failed to run `gh api` — is the gh CLI installed and on PATH?")?;

    if !output.status.success() {
        anyhow::bail!(
            "gh api users/{login} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let v: Value =
        serde_json::from_slice(&output.stdout).context("failed to parse gh api JSON output")?;
    Ok(v.get("name")
        .and_then(|n| n.as_str())
        .filter(|n| !n.is_empty())
        .map(str::to_string))
}

/// A pull request linked to an issue.
pub struct LinkedPr {
    /// `owner/name` of the repo the PR lives in.
    pub repo: String,
    pub number: u64,
    pub url: String,
    /// The PR author's login (`ghost` for deleted accounts).
    pub author: String,
    /// Whether the PR is set to close the issue (a closing keyword or a
    /// Development-sidebar link), as opposed to only mentioning it.
    pub closing: bool,
}

/// Pull requests linked to each of the given issues, keyed by the issue's
/// GraphQL `node_id` (as found on search items). An issue's list holds the
/// PRs set to close it (a "fixes #N" keyword or a manual link in the
/// Development sidebar) first, then any other PR that cross-references it,
/// without duplicates. Issues with no linked PR map to an empty list.
///
/// Uses GraphQL `nodes(ids: ...)` in batches of 50, so a run costs one call
/// per 50 issues instead of one search per issue — the search API's
/// ~30 req/min cap would trip on a per-issue loop.
pub fn linked_prs(
    node_ids: &[&str],
    token: Option<&str>,
) -> Result<std::collections::HashMap<String, Vec<LinkedPr>>> {
    let mut out = std::collections::HashMap::new();
    for chunk in node_ids.chunks(50) {
        let ids = chunk
            .iter()
            .map(|id| serde_json::to_string(id).expect("string serializes"))
            .collect::<Vec<_>>()
            .join(",");
        let query = format!(
            "query {{ nodes(ids: [{ids}]) {{ ... on Issue {{ id \
             closedByPullRequestsReferences(first: 10, includeClosedPrs: true) {{ nodes {{ ...pr }} }} \
             timelineItems(first: 50, itemTypes: [CROSS_REFERENCED_EVENT, CONNECTED_EVENT]) {{ nodes {{ \
               ... on CrossReferencedEvent {{ source {{ ... on PullRequest {{ ...pr }} }} }} \
               ... on ConnectedEvent {{ subject {{ ... on PullRequest {{ ...pr }} }} }} \
             }} }} }} }} }} \
             fragment pr on PullRequest {{ number url author {{ login }} repository {{ nameWithOwner }} }}"
        );

        let mut cmd = Command::new("gh");
        cmd.args(["api", "graphql", "-f", &format!("query={query}")]);
        if let Some(t) = token {
            cmd.env("GH_TOKEN", t);
        }
        let output = cmd
            .output()
            .context("failed to run `gh api` — is the gh CLI installed and on PATH?")?;
        if !output.status.success() {
            anyhow::bail!(
                "gh api graphql (linked PRs) failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let v: Value = serde_json::from_slice(&output.stdout)
            .context("failed to parse gh api graphql JSON output")?;

        let nodes = v
            .pointer("/data/nodes")
            .and_then(|n| n.as_array())
            .cloned()
            .unwrap_or_default();
        for node in &nodes {
            let Some(id) = node.get("id").and_then(|i| i.as_str()) else {
                continue;
            };
            let closing = node
                .pointer("/closedByPullRequestsReferences/nodes")
                .and_then(|n| n.as_array())
                .into_iter()
                .flatten()
                .map(|pr| (pr, true));
            let referencing = node
                .pointer("/timelineItems/nodes")
                .and_then(|n| n.as_array())
                .into_iter()
                .flatten()
                .filter_map(|ev| ev.get("source").or_else(|| ev.get("subject")))
                .map(|pr| (pr, false));

            let mut prs: Vec<LinkedPr> = Vec::new();
            for (pr, closing) in closing.chain(referencing) {
                // Closing PRs come first, so a PR that both closes and
                // mentions the issue keeps `closing: true`.
                // Non-PR sources (issues cross-referencing this one) come
                // back as empty objects.
                let Some(number) = pr.get("number").and_then(|n| n.as_u64()) else {
                    continue;
                };
                let repo = pr
                    .pointer("/repository/nameWithOwner")
                    .and_then(|r| r.as_str())
                    .unwrap_or("")
                    .to_string();
                if prs.iter().any(|p| p.number == number && p.repo == repo) {
                    continue;
                }
                let url = pr
                    .get("url")
                    .and_then(|u| u.as_str())
                    .unwrap_or("")
                    .to_string();
                let author = pr
                    .pointer("/author/login")
                    .and_then(|l| l.as_str())
                    .unwrap_or("ghost")
                    .to_string();
                prs.push(LinkedPr { repo, number, url, author, closing });
            }
            out.insert(id.to_string(), prs);
        }
    }
    Ok(out)
}
