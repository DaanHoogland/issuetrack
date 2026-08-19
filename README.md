# acsissues

Refresh helper for the ACSissues Google Sheet. Pulls data from GitHub via the
`gh` CLI (reuses your existing `gh auth login`) and prints TSV rows ready to
paste into the sheet. Never touches the sheet itself — no Google credentials
involved, nothing runs on a schedule.

## Setup

```
cargo build --release
./target/release/acsissues init-config
```

Edit `~/.config/acsissues/config.toml` and add the GitHub usernames of
ShapeBlue employees who are *not* using a `@shapeblue.com`-domain-detectable
address (i.e. all of them — this list is by username, not email). Bot accounts
ending in `[bot]` (dependabot, github-actions, ...) are excluded automatically;
add any others to `extra_bots`.

## Usage

### ACSissues tab (individual, external-party issues only)

```
acsissues individual
```

Prints one TSV row per new issue opened by a non-ShapeBlue, non-bot author:

```
url    title    author    opened-date    (blank status)    (blank action)
```

The status and action columns are left blank for you to fill in by hand, same
as today. Paste the rows in under the existing header, newest first.

Defaults to picking up from the last run (tracked in
`~/.config/acsissues/state.json`) through today; first run defaults to the
last 7 days. Override with `--since YYYY-MM-DD` / `--until YYYY-MM-DD`, and
add `--no-state` to skip reading/writing the state file.

### Weekly / Monthly / Yearly tabs

```
acsissues weekly
acsissues monthly
acsissues yearly
```

Each prints one TSV row for the most recently *completed* period, matching
the sheet's columns exactly:

```
begin    end    open    closed    total    date    duration    url    rate
```

e.g. `2026-08-12    2026-08-18    28    25    53    2026-08-19    6    2026-08-12..2026-08-18    0.4717`
(`total`, `duration`, `url`, `rate` are computed here rather than left as
sheet formulas, so the row pastes correctly even into a cell range with no
prior formula).

Period boundaries:
- **weekly**: the most recently completed Wednesday→Tuesday window (matches
  the existing sheet's cadence).
- **monthly**: the most recently completed calendar month.
- **yearly**: the most recently completed calendar year — this one is an
  assumption; say so if you actually want a running year-to-date row instead.

Use `--since YYYY-MM-DD --until YYYY-MM-DD` together to override the
computed begin/end (e.g. for backfilling a specific range).

## Notes

- Uses the GitHub search API (`is:issue`, so PRs are excluded automatically).
  Search results cap at 1000 per query — fine for weekly/monthly windows,
  should be checked if a yearly backfill query gets large.
- Repo defaults to `apache/cloudstack`; override with `--repo owner/name`.
