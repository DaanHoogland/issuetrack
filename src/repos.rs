use anyhow::Result;
use serde_json::Value;

const DEFAULT_REPO: &str = "apache/cloudstack";

/// The repos to query: `--repo` values if any were given, else `repos` from
/// config.toml, else `DEFAULT_REPO`. Each must look like `owner/name` —
/// anything else would be spliced into the search query as extra
/// qualifiers. Duplicates (case-insensitively) are dropped, keeping the
/// first spelling.
pub fn resolve_repos(cli_repos: &[String], config_repos: &[String]) -> Result<Vec<String>> {
    let source = if !cli_repos.is_empty() { cli_repos } else { config_repos };
    let mut repos: Vec<String> = Vec::new();
    for raw in source {
        let repo = raw.trim();
        if repo.is_empty() {
            continue;
        }
        let valid = repo.split('/').count() == 2
            && repo.split('/').all(|part| {
                !part.is_empty()
                    && part.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
            });
        if !valid {
            anyhow::bail!("invalid repo '{repo}', expected owner/name (e.g. apache/cloudstack)");
        }
        if !repos.iter().any(|r| r.eq_ignore_ascii_case(repo)) {
            repos.push(repo.to_string());
        }
    }
    if repos.is_empty() {
        repos.push(DEFAULT_REPO.to_string());
    }
    Ok(repos)
}

/// `repo:a/b repo:c/d` — GitHub search ORs repeated `repo:` qualifiers, so
/// one query (and one paginated fetch) covers every repo at once.
pub fn repo_qualifiers(repos: &[String]) -> String {
    repos
        .iter()
        .map(|r| format!("repo:{r}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The `owner/name` an issue-search item belongs to, from its
/// `repository_url` (`https://api.github.com/repos/owner/name`).
pub fn item_repo(item: &Value) -> &str {
    item.get("repository_url")
        .and_then(|v| v.as_str())
        .and_then(|u| u.strip_prefix("https://api.github.com/repos/"))
        .unwrap_or("")
}
