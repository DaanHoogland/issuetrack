# issuetrack

Refresh helper for the ACSissues Google Sheet. Pulls data from GitHub via the
`gh` CLI (reuses your existing `gh auth login`) and prints TSV rows ready to
paste into the sheet. Never touches the sheet itself — no Google credentials
involved, nothing runs on a schedule.

## Setup

```
cargo build --release
./target/release/issuetrack init-config
```

Edit `~/.config/issuetrack/config.toml` and add the GitHub usernames of
ShapeBlue employees who are *not* using a `@shapeblue.com`-domain-detectable
address (i.e. all of them — this list is by username, not email). Bot accounts
ending in `[bot]` (dependabot, github-actions, ...) are excluded automatically;
add any others to `extra_bots`. See [`config.toml.example`](config.toml.example)
for the full set of fields.

## Usage

### ACSissues tab (individual, external-party issues only)

```
issuetrack individual
```

Prints one TSV row per new issue opened by a non-ShapeBlue, non-bot author:

```
url    title    author-profile-url    opened-date    (blank status)    (blank action)
```

The status and action columns are left blank for you to fill in by hand, same
as today. Paste the rows in under the existing header, newest first.

Defaults to picking up from the last run (tracked in
`~/.config/issuetrack/state.json`) through today; first run defaults to the
last 7 days. Override with `--since YYYY-MM-DD` / `--until YYYY-MM-DD`, and
add `--no-state` to skip reading/writing the state file.

`author-profile-url` is the reporter's GitHub profile url
(`https://github.com/<login>`) by default, or `[<login>](https://github.com/<login>)`
with `--markdown-url`, or `=HYPERLINK("https://github.com/<login>", "<login>")`
with `--hyperlink-url` (ready to paste straight into a sheet cell). Add
`--username` to print the bare login instead (takes precedence over
`--markdown-url`/`--hyperlink-url` for this column).

Add `--full-username` to look up each reporter's public GitHub display name
(one API call per unique login) and show it in place of the login wherever
the reporter column has a separate display text — the link text with
`--markdown-url`/`--hyperlink-url`, or the printed name with `--username`.
Falls back to the login for accounts with no display name set, and has no
effect if none of those flags is given (the plain profile-url output has no
separate text to replace).

### Weekly / Monthly / Yearly tabs

```
issuetrack weekly
issuetrack monthly
issuetrack yearly
```

Each prints one TSV row for the most recently *completed* period, covering
the sheet's input columns:

```
url    observed    begin    end    open    closed
```

e.g.
`https://github.com/apache/cloudstack/issues?q=is%3Aissue%20created%3A2026-08-12..2026-08-18    2026-08-19    2026-08-12    2026-08-18    28    25`.
The remaining columns (`total`, `duration`, `rate`) are sheet formulas —
paste this row into H/F/A/B/C/D and drag the existing formulas down from the
row above.

Add `--markdown-url` to format the url column as a markdown link instead
(`[2026-08-12..2026-08-18](https://github.com/apache/cloudstack/issues?q=is%3Aissue%20created%3A2026-08-12..2026-08-18)`),
or `--hyperlink-url` to format it as a Google Sheets formula instead
(`=HYPERLINK("https://github.com/apache/cloudstack/issues?q=is%3Aissue%20created%3A2026-08-12..2026-08-18", "2026-08-12..2026-08-18")`) —
paste that straight into the sheet's url cell and it renders as a link. The
two are mutually exclusive; set `markdown_url = true` or `hyperlink_url =
true` in `config.toml` to make one the default.

Google Sheets rejects that formula if built with the wrong argument
separator for the sheet's locale: English-like locales expect `,` (the
default), but locales with a comma decimal separator (Dutch, German,
French, ...) expect `;` instead. Pass `--hyperlink-separator` a single
literal character to use instead — quote it in most shells, e.g.
`--hyperlink-separator ';'` or `-P ';'` (or set `hyperlink_separator = ";"`
in `config.toml`).

Period boundaries:
- **weekly**: the most recently completed Wednesday→Tuesday window (matches
  the existing sheet's cadence). Give a date (`yyyy/mm/dd` or `yy/mm/dd`, e.g.
  `issuetrack weekly 2026/07/20` or `issuetrack weekly 26/07/20`) to instead
  print one row per completed week, from the week containing that date
  through the most recently completed week — handy for backfilling a gap.
  Whenever the resolved period covers more than one week (this positional
  form, or a `--since`/`--until` override spanning more than 7 days), one row
  per Wednesday→Tuesday week is printed instead of a single aggregate row;
  a period that resolves to exactly one week still prints a single row.
- **monthly**: the current calendar month by default.
  - `-N` = N months back (e.g. `issuetrack monthly -3`).
  - a bare positive number = an absolute year, keeping the current month
    (2-digit -> 20XX, 4-digit as-is), e.g. `issuetrack monthly 2021` or
    `issuetrack monthly 23`.
  - a month name/abbreviation alone = that month in the current year
    (`issuetrack monthly aug` / `issuetrack monthly august`).
  - a month name plus a year, in either order = that month in that year
    (`issuetrack monthly aug 2021` or `issuetrack monthly 2021 aug`).
- **yearly**: the current calendar year by default. Add `-N` to go back N
  years instead (e.g. `issuetrack yearly -2` = 2 years ago), or give an
  absolute year directly, 2-digit or 4-digit (`issuetrack yearly 23` or
  `issuetrack yearly 2023`).

Use `--since YYYY-MM-DD --until YYYY-MM-DD` together to override the
computed begin/end directly (e.g. for backfilling an arbitrary range) —
takes precedence over `-N`. As with `weekly`, when the overridden period
covers more than one calendar month (`monthly`) or more than one calendar
year (`yearly`), one row per month/year is printed instead of a single
aggregate row; a period that resolves to exactly one month/year still
prints a single row.

Multi-row output from `weekly`/`monthly`/`yearly` prints newest period first
by default. Add `--chronological` (`-C`) to print oldest-first instead. Has
no visible effect when the period resolves to a single row.

**The `monthly`/`yearly` subcommand names are optional** whenever a month
name and/or a year is given directly — it's inferred: a month name (with or
without a year, either order) implies `monthly`; a bare year alone implies
`yearly`.

```
issuetrack aug          # same as: issuetrack monthly aug
issuetrack aug 2021     # same as: issuetrack monthly aug 2021
issuetrack 2021 aug     # same as: issuetrack monthly 2021 aug
issuetrack 2021         # same as: issuetrack yearly 2021
```

The `-N` back-offset forms and `--since`/`--until` overrides still need the
explicit subcommand, since a bare `-3` alone is ambiguous between months and
years back.

## Notes

- Uses the GitHub search API (`is:issue`, so PRs are excluded automatically).
  Search results cap at 1000 per query — fine for weekly/monthly windows,
  should be checked if a yearly backfill query gets large.
- `weekly <date>` backfills fetch the whole span in one paginated search and
  bucket locally by week, instead of two searches per week — keeps a
  multi-month backfill well under the search API's own 30-requests/minute
  limit.
- Repo defaults to `apache/cloudstack`; override with `--repo owner/name`.
- By default, GitHub requests use `gh`'s own stored login (`gh auth login`).
  To use a personal access token instead, pass `--token <TOKEN>` or set
  `github_token = "ghp_..."` in `config.toml` (the flag takes precedence).

### Pasting into Sheets

Output is tab-separated so it splits into columns on paste — but only if the
tabs actually survive the trip to your clipboard. Selecting output by hand in
a terminal window is fragile: some terminals render a tab as spaces and copy
what's rendered, not the original byte, so what lands in Sheets is one column
per row instead of several (with no visible warning — the paste just looks
wrong, cells run together, and a `=HYPERLINK(...)` formula from one row can
swallow the value that should've landed in the cell after it). Use `--copy`
to skip the terminal entirely — it pipes the output straight through
`pbcopy` (macOS), so the real tab bytes reach the clipboard: `issuetrack
--copy weekly`, then paste directly into the sheet.

## Command reference

Subcommand names can be abbreviated to any unambiguous prefix (`issuetrack
ind`, `issuetrack week`, `issuetrack mo aug`, ...). Full `--help`/`-h` is
available at every level (`issuetrack --help`, `issuetrack individual --help`, ...).

### Global options

Apply to every subcommand (and to the implicit month/year shortcut).

| Option | Short | Description |
|---|---|---|
| `--repo <REPO>` | `-r` | GitHub repo to query (default: `apache/cloudstack`) |
| `--config <PATH>` | `-c` | Path to `config.toml` (default: `~/.config/issuetrack/config.toml`) |
| `--state <PATH>` | `-S` | Path to the state file (default: `~/.config/issuetrack/state.json`) |
| `--token <TOKEN>` | `-t` | GitHub personal access token, instead of `gh`'s own stored login |
| `--markdown-url` | `-m` | Format url columns as markdown links: the weekly/monthly/yearly period url as `[begin..end](url)`, and the `individual` reporter's profile url as `[login](url)`. Mutually exclusive with `--hyperlink-url` |
| `--hyperlink-url` | `-H` | Format url columns as Google Sheets formulas instead: `=HYPERLINK(url, "begin..end")` / `=HYPERLINK(url, "login")` — paste straight into a sheet cell. Mutually exclusive with `--markdown-url` |
| `--hyperlink-separator <CHAR>` | `-P` | Single literal character to use as the argument separator inside `=HYPERLINK()` formulas (only matters with `--hyperlink-url`); default `,` — e.g. `-P ';'` |
| `--copy` | `-y` | Copy the output to the clipboard (macOS `pbcopy`) instead of relying on selecting it from the terminal — see [Pasting into Sheets](#pasting-into-sheets) below |
| `--help` | `-h` | Print help |
| `--version` | `-V` | Print version |

### `individual`

TSV rows (url, title, user profile url, opened date) of new external-party
issues, for the ACSissues tab. Issues GitHub itself has closed as a duplicate
(`state_reason: duplicate`) are skipped.

| Option | Short | Description |
|---|---|---|
| `--since <YYYY-MM-DD>` | `-s` | Start date (default: last recorded state run, or 7 days ago) |
| `--until <YYYY-MM-DD>` | `-u` | End date (default: today) |
| `--no-state` | `-n` | Don't read or update the state file |
| `--username` | `-U` | Print the reporter as their bare GitHub username instead of a profile url (overrides `--markdown-url`/`--hyperlink-url` for this column) |
| `--full-username` | `-F` | Look up each reporter's GitHub display name and use it in place of the login as the link text (`--markdown-url`/`--hyperlink-url`) or `--username` text; no effect otherwise |

### `weekly`

One summary row per completed Wed→Tue week, for the Weekly tab.

| Option | Short | Description |
|---|---|---|
| `--since <YYYY-MM-DD>` / `--until <YYYY-MM-DD>` | `-s` / `-u` | Manual override of the period (both required together) |
| `[DATE]` positional (`yyyy/mm/dd` or `yy/mm/dd`) | | Print one row per completed week from the week containing `DATE` through the most recently completed week |
| `--chronological` | `-C` | Print multi-row output oldest-first instead of the default newest-first |

Default (no args): the single most recently completed Wed→Tue window. Any
resolved period longer than one week (via `[DATE]` or a wide `--since`/
`--until` override) prints one row per week instead of a single row.

### `monthly`

Same row shape, for a calendar month.

| Option | Short | Description |
|---|---|---|
| `--since` / `--until` | `-s` / `-u` | Manual override (same as `weekly`) |
| `[FIRST]` | | Months back (negative number), or a month name/abbreviation, or a year |
| `[SECOND]` | | A year, paired with a month name in `FIRST` (either order) |
| `--chronological` | `-C` | Print multi-row output oldest-first instead of the default newest-first |

Examples: `monthly`, `monthly -3`, `monthly aug`, `monthly aug 2021`, `monthly 2021 aug`.

A `--since`/`--until` override spanning more than one calendar month prints
one row per month instead of a single row.

### `yearly`

Same row shape, for a calendar year.

| Option | Short | Description |
|---|---|---|
| `--since` / `--until` | `-s` / `-u` | Manual override (same as `weekly`) |
| `[N]` | | `0` = current year (default); negative = N years back; positive = absolute year (2- or 4-digit) |
| `--chronological` | `-C` | Print multi-row output oldest-first instead of the default newest-first |

A `--since`/`--until` override spanning more than one calendar year prints
one row per year instead of a single row.

### `init-config`

No options. Writes a starter `config.toml` if one doesn't already exist.

### Implicit shortcut (no subcommand)

`issuetrack [FIRST] [SECOND]` — same resolution rules as `monthly`'s two
positionals, but dispatches to `monthly` or `yearly` depending on whether a
month name is present:

- `issuetrack aug 2021` → `monthly aug 2021`
- `issuetrack 2021` → `yearly 2021`
- `issuetrack` alone with no args is an error (must pass a subcommand, or a month/year)
