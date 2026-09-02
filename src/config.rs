use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Default argument separator for `=HYPERLINK()` formulas. Google Sheets
/// picks this per document locale: English-like locales use `,`; locales
/// with a comma decimal separator (Dutch, German, French, ...) require `;`
/// instead, and reject a formula built with the wrong one.
pub const DEFAULT_HYPERLINK_SEPARATOR: char = ',';

fn default_hyperlink_separator() -> char {
    DEFAULT_HYPERLINK_SEPARATOR
}

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub excluded_users: Vec<String>,
    #[serde(default)]
    pub extra_bots: Vec<String>,
    #[serde(default)]
    pub github_token: Option<String>,
    #[serde(default, alias = "markdown-url")]
    pub markdown_url: bool,
    #[serde(default, alias = "hyperlink-url")]
    pub hyperlink_url: bool,
    #[serde(default = "default_hyperlink_separator", alias = "hyperlink-separator")]
    pub hyperlink_separator: char,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            excluded_users: Vec::new(),
            extra_bots: Vec::new(),
            github_token: None,
            markdown_url: false,
            hyperlink_url: false,
            hyperlink_separator: default_hyperlink_separator(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        if !path.exists() {
            anyhow::bail!(
                "config file not found at {}\nrun `issuetrack init-config` to create a starter one, then add ShapeBlue GitHub usernames to it",
                path.display()
            );
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config file {}", path.display()))?;
        let cfg: Config = toml::from_str(&text)
            .with_context(|| format!("parsing config file {}", path.display()))?;
        Ok(cfg)
    }

    /// True if this GitHub login should be treated as an external contributor:
    /// not a known bot and not in the excluded (ShapeBlue) list.
    pub fn is_external(&self, login: &str) -> bool {
        let lower = login.to_lowercase();
        if lower.ends_with("[bot]") {
            return false;
        }
        if self.extra_bots.iter().any(|b| b.to_lowercase() == lower) {
            return false;
        }
        if self.excluded_users.iter().any(|u| u.to_lowercase() == lower) {
            return false;
        }
        true
    }
}

pub fn default_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/issuetrack/config.toml")
}

/// Reads the config file for its optional defaults (`github_token`,
/// `markdown_url`, `hyperlink_url`, `hyperlink_separator`), falling back to
/// `Config::default()` if the file is missing or unreadable. Unlike
/// `Config::load`, that's not an error here — most commands don't require a
/// config file at all.
pub fn load_defaults(path: &Path) -> Config {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default()
}

pub const STARTER: &str = r#"# ShapeBlue GitHub usernames to exclude from the "external party" issue list.
# Add one entry per known ShapeBlue employee GitHub account. Some employees use
# a personal, apache.org, or noreply address instead of @shapeblue.com, so this
# list is maintained by GitHub username, not by email domain.
excluded_users = [
  # "someusername",
]

# Extra bot accounts to exclude, for bots whose username doesn't end in "[bot]"
# (accounts ending in "[bot]" — e.g. dependabot[bot] — are excluded automatically).
extra_bots = [
  # "some-custom-bot",
]

# Optional GitHub personal access token, used instead of `gh`'s own stored
# login when talking to the GitHub API. Overridden by --token on the command
# line. Leave unset to keep using `gh auth login`.
# github_token = "ghp_..."

# Format the weekly/monthly/yearly url column (and the `individual` reporter
# column) as a markdown link instead of a plain url. Also settable per run
# with --markdown-url. Mutually exclusive with hyperlink_url.
# markdown_url = false

# Same, but as a Google Sheets =HYPERLINK("url", "text") formula instead of
# a markdown link. Also settable per run with --hyperlink-url. Mutually
# exclusive with markdown_url.
# hyperlink_url = false

# Single-character argument separator inside =HYPERLINK() formulas. Most
# locales use "," (the default); locales with a comma decimal separator
# (Dutch, German, French, ...) reject a formula built with "," and need ";"
# instead. Also settable per run with --hyperlink-separator. Only matters
# with hyperlink_url.
# hyperlink_separator = ","
"#;
