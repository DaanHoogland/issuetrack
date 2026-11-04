use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Parses a `--hyperlink-separator` value: exactly one character, and not a
/// double quote (which would break the formula's own quoting).
fn parse_hyperlink_separator(s: &str) -> std::result::Result<char, String> {
    let mut chars = s.chars();
    let c = chars.next().ok_or("must be a single character, not empty")?;
    if chars.next().is_some() {
        return Err(format!("must be a single character, got '{s}'"));
    }
    if c == '"' {
        return Err("cannot be a double quote — that would break the formula".to_string());
    }
    Ok(c)
}

/// Refresh helper for the ACSissues Google Sheet: pulls data from GitHub and
/// prints TSV rows ready to paste into the sheet. Does not touch the sheet
/// itself — no Google credentials involved.
#[derive(Parser)]
#[command(name = "issuetrack", version, about, infer_subcommands = true)]
pub struct Cli {
    /// GitHub repo(s) to query, as owner/name. Repeatable (`-r a/b -r c/d`)
    /// or comma-separated (`-r a/b,c/d`). Falls back to `repos` in
    /// config.toml, then to apache/cloudstack.
    #[arg(short = 'r', long, global = true, value_delimiter = ',')]
    pub repo: Vec<String>,

    /// Path to config.toml (excluded ShapeBlue users / extra bots)
    #[arg(short = 'c', long, global = true)]
    pub config: Option<PathBuf>,

    /// Path to the state file tracking the last "individual" run date
    #[arg(short = 'S', long, global = true)]
    pub state: Option<PathBuf>,

    /// GitHub personal access token to use instead of `gh`'s own stored
    /// login. Falls back to `github_token` in config.toml, then to `gh`'s
    /// normal auth (GH_TOKEN/GITHUB_TOKEN env vars or `gh auth login`).
    #[arg(short = 't', long, global = true)]
    pub token: Option<String>,

    /// Format url columns as markdown links instead of plain urls: the
    /// weekly/monthly/yearly period url (`[begin..end](url)`) and the
    /// `individual` reporter's GitHub profile url (`[login](url)`).
    /// Mutually exclusive with --hyperlink-url.
    #[arg(short = 'm', long, global = true, conflicts_with = "hyperlink_url")]
    pub markdown_url: bool,

    /// Format url columns as Google Sheets `=HYPERLINK("url", "text")`
    /// formulas instead of plain urls — same columns as --markdown-url, and
    /// ready to paste straight into a sheet cell. Mutually exclusive with
    /// --markdown-url.
    #[arg(short = 'H', long, global = true)]
    pub hyperlink_url: bool,

    /// Single-character argument separator inside `=HYPERLINK()` formulas
    /// (only matters with --hyperlink-url). Google Sheets requires `;`
    /// instead of `,` in locales with a comma decimal separator (Dutch,
    /// German, French, ...); Sheets rejects the formula if this doesn't
    /// match the sheet's locale. Quote it in most shells, e.g. `-P ';'`.
    /// Defaults to `,`, or `hyperlink_separator` in config.toml.
    #[arg(short = 'P', long, global = true, value_parser = parse_hyperlink_separator)]
    pub hyperlink_separator: Option<char>,

    /// Copy the output to the clipboard (macOS `pbcopy`) instead of printing
    /// it for you to select and copy by hand. Selecting tab-separated output
    /// from a terminal window is fragile — some terminals render tabs as
    /// spaces on copy, which then paste as one merged Sheets cell instead of
    /// several; piping through the real clipboard preserves the tab bytes.
    #[arg(short = 'y', long, global = true)]
    pub copy: bool,

    #[command(subcommand)]
    pub command: Option<Command>,

    /// Implicit shortcut when no subcommand is given: a month name/abbreviation
    /// and/or a year resolve to `monthly`/`yearly` automatically (in either
    /// order), e.g. `issuetrack aug 2021` or `issuetrack 2021`.
    #[arg(allow_hyphen_values = true)]
    pub first: Option<String>,
    #[arg(allow_hyphen_values = true)]
    pub second: Option<String>,
}

/// Manual override of a period's begin/end date, for backfilling.
#[derive(clap::Args)]
pub struct PeriodOverride {
    /// Override the period's begin date (YYYY-MM-DD). Requires --until too.
    #[arg(short = 's', long, requires = "until")]
    pub since: Option<String>,
    /// Override the period's end date (YYYY-MM-DD). Requires --since too.
    #[arg(short = 'u', long, requires = "since")]
    pub until: Option<String>,
    /// Print multi-row output oldest-first (chronological order) instead of
    /// the default newest-first.
    #[arg(short = 'C', long)]
    pub chronological: bool,
}

#[derive(Subcommand)]
pub enum Command {
    /// TSV rows (url, title, user profile url, opened date) for new external-party issues,
    /// for the ACSissues tab. Blank status/action columns are left for you to fill in.
    Individual {
        /// Start date (YYYY-MM-DD). Defaults to the last recorded run, or 7 days ago.
        #[arg(short = 's', long)]
        since: Option<String>,
        /// End date (YYYY-MM-DD). Defaults to today.
        #[arg(short = 'u', long)]
        until: Option<String>,
        /// Don't read or update the state file.
        #[arg(short = 'n', long)]
        no_state: bool,
        /// Print the reporter as their bare GitHub username instead of a
        /// profile url (overrides --markdown-url/--hyperlink-url for that
        /// column).
        #[arg(short = 'U', long)]
        username: bool,
        /// Look up each reporter's full GitHub display name and show it
        /// (instead of their login) wherever the reporter column has a
        /// separate display text: as the link text with --markdown-url or
        /// --hyperlink-url, or as the displayed name with --username. Falls
        /// back to the login for accounts with no display name set, and has
        /// no effect on the plain profile-url output (none of those flags given).
        #[arg(short = 'F', long)]
        full_username: bool,
    },
    /// One summary TSV row (begin, end, open, closed, total, date, duration,
    /// url, rate) for the Weekly tab, covering the most recently completed
    /// Wednesday-through-Tuesday window.
    Weekly {
        #[command(flatten)]
        over: PeriodOverride,
        /// Print one row per completed week from the week containing this
        /// date through the most recently completed week. Format: yyyy/mm/dd
        /// or yy/mm/dd (2-digit year, this century).
        #[arg(value_name = "DATE")]
        since_date: Option<String>,
    },
    /// Same shape as `weekly`, covering the current calendar month by default.
    #[command(allow_negative_numbers = true)]
    Monthly {
        #[command(flatten)]
        over: PeriodOverride,
        /// Months back (e.g. -3 = 3 months ago), or a month name/abbreviation
        /// (e.g. Aug, August), optionally paired with a year (in either order).
        #[arg(allow_hyphen_values = true)]
        first: Option<String>,
        /// A year (2-digit or 4-digit) to go with a month name in `first`.
        /// May be given before or after the month name.
        #[arg(allow_hyphen_values = true)]
        second: Option<String>,
    },
    /// Same shape as `weekly`, covering the current calendar year by default.
    #[command(allow_negative_numbers = true)]
    Yearly {
        #[command(flatten)]
        over: PeriodOverride,
        /// 0 = current year; negative = N years back (e.g. -3 = 3 years ago);
        /// positive = an absolute year, 2-digit (23 -> 2023) or 4-digit (2023).
        #[arg(allow_hyphen_values = true, default_value_t = 0)]
        n: i64,
    },
    /// Write a starter config.toml if one doesn't already exist.
    InitConfig,
}
