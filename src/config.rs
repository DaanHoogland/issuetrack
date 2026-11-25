use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::fields::FieldsConfig;

/// Default argument separator for `=HYPERLINK()` formulas. Google Sheets
/// picks this per document locale: English-like locales use `,`; locales
/// with a comma decimal separator (Dutch, German, French, ...) require `;`
/// instead, and reject a formula built with the wrong one.
pub const DEFAULT_HYPERLINK_SEPARATOR: char = ',';

/// config.toml. Every setting is optional; missing ones take their
/// `Default` value.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub repos: Vec<String>,
    pub excluded_users: Vec<String>,
    pub extra_bots: Vec<String>,
    pub github_token: Option<String>,
    #[serde(alias = "markdown-url")]
    pub markdown_url: bool,
    #[serde(alias = "hyperlink-url")]
    pub hyperlink_url: bool,
    #[serde(alias = "hyperlink-separator")]
    pub hyperlink_separator: char,
    pub fields: FieldsConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            repos: Vec::new(),
            excluded_users: Vec::new(),
            extra_bots: Vec::new(),
            github_token: None,
            markdown_url: false,
            hyperlink_url: false,
            hyperlink_separator: DEFAULT_HYPERLINK_SEPARATOR,
            fields: FieldsConfig::default(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        if !path.exists() {
            bail!(
                "config file not found at {}\nrun `issuetrack init-config` to create a starter one, then add ShapeBlue GitHub usernames to it",
                path.display()
            );
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config file {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing config file {}", path.display()))
    }

    /// Not a bot (`[bot]` suffix or `extra_bots`) and not an excluded
    /// (ShapeBlue) user.
    pub fn is_external_contributor(&self, login: &str) -> bool {
        let is_listed_in = |logins: &[String]| {
            logins
                .iter()
                .any(|listed| listed.eq_ignore_ascii_case(login))
        };
        let is_bot = login.to_lowercase().ends_with("[bot]") || is_listed_in(&self.extra_bots);
        !is_bot && !is_listed_in(&self.excluded_users)
    }
}

pub fn default_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/issuetrack/config.toml")
}

/// Reads the config file for its optional defaults (`repos`, `github_token`,
/// `markdown_url`, `hyperlink_url`, `hyperlink_separator`, `[fields]`), falling back to
/// `Config::default()` if the file is missing or unreadable. Unlike
/// `Config::load`, that's not an error here — most commands don't require a
/// config file at all.
pub fn load_defaults(path: &Path) -> Config {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default()
}

pub const STARTER: &str = r#"# GitHub repos (owner/name) to query. Overridden by --repo on the command
# line. Defaults to apache/cloudstack when empty or unset.
# repos = ["apache/cloudstack", "apache/cloudstack-terraform-provider"]

# ShapeBlue GitHub usernames to exclude from the "external party" issue list.
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

# Output columns, in order, per command. Each entry is either an array of
# field names or one string in --fields syntax: comma-separated names, or
# one letter per column. "blank" (letter _) inserts an empty column.
# Overridden by --fields on the command line. Defaults are shown.
#
#   individual: url (u), comment (c), user (r), opened (o), action (a)
#   period (weekly/monthly/yearly): url (u), date (d), begin (b), end (e),
#                                   open (o), closed (c)
#
# [fields]
# individual = ["url", "comment", "user", "opened", "action"]
# period = "udbeoc"
"#;

pub fn init_config(path: &PathBuf) -> Result<()> {
    if path.exists() {
        bail!(
            "config file already exists at {} — edit it directly, refusing to overwrite",
            path.display()
        );
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, STARTER)
        .with_context(|| format!("writing starter config to {}", path.display()))?;
    println!("wrote starter config to {}", path.display());
    println!("edit it to add ShapeBlue GitHub usernames before running `individual`");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_settings_take_their_defaults() {
        let config: Config = toml::from_str("repos = [\"a/b\"]").unwrap();
        assert_eq!(config.repos, ["a/b"]);
        assert_eq!(config.hyperlink_separator, DEFAULT_HYPERLINK_SEPARATOR);
        assert!(config.fields.individual.is_none());
    }

    #[test]
    fn the_starter_config_parses() {
        assert!(toml::from_str::<Config>(STARTER).is_ok());
    }

    #[test]
    fn bots_and_excluded_users_are_not_external() {
        let config: Config =
            toml::from_str("excluded_users = [\"DaanH\"]\nextra_bots = [\"ci-helper\"]").unwrap();
        assert!(config.is_external_contributor("someone"));
        assert!(!config.is_external_contributor("daanh"));
        assert!(!config.is_external_contributor("CI-Helper"));
        assert!(!config.is_external_contributor("dependabot[bot]"));
    }
}
