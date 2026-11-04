use chrono::NaiveDate;
use std::path::PathBuf;

use crate::output::LinkStyle;

/// Run-wide settings, resolved once in `main` from the command line and
/// config.toml, and handed to every command.
pub struct Ctx {
    pub repos: Vec<String>,
    pub config_path: PathBuf,
    pub state_path: PathBuf,
    pub today: NaiveDate,
    pub token: Option<String>,
    pub link_style: LinkStyle,
}

impl Ctx {
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }
}
