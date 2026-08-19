use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub excluded_users: Vec<String>,
    #[serde(default)]
    pub extra_bots: Vec<String>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        if !path.exists() {
            anyhow::bail!(
                "config file not found at {}\nrun `acsissues init-config` to create a starter one, then add ShapeBlue GitHub usernames to it",
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
        .join(".config/acsissues/config.toml")
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
"#;
