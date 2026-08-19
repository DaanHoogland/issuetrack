use anyhow::{Context, Result};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    /// Date through which the "individual" (external-issues) command last ran,
    /// so the next run can default to picking up right where it left off.
    pub last_individual: Option<NaiveDate>,
}

impl State {
    pub fn load(path: &Path) -> Result<State> {
        if !path.exists() {
            return Ok(State::default());
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading state file {}", path.display()))?;
        serde_json::from_str(&text)
            .with_context(|| format!("parsing state file {}", path.display()))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("writing state file {}", path.display()))
    }
}

pub fn default_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/acsissues/state.json")
}
