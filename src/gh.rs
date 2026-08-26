use anyhow::{Context, Result};
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
