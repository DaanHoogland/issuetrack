//! Configurable output columns. Each command defines its own `Field` enum;
//! a column list picks fields (and blanks) by name or one-letter shorthand,
//! from `--fields`, the config's `[fields]` table, or the command's default.

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

pub const BLANK_NAME: &str = "blank";
pub const BLANK_LETTER: char = '_';

/// A field a command can print, with its spec name and one-letter shorthand.
pub trait Field: Copy + PartialEq + 'static {
    /// Every field, in the command's default output order.
    const ALL: &'static [Self];
    /// The commands these fields belong to, for error messages.
    const COMMANDS: &'static str;
    /// The key in the config's `[fields]` table.
    const CONFIG_KEY: &'static str;

    fn name(self) -> &'static str;
    fn letter(self) -> char;
    fn configured_spec(fields_config: &FieldsConfig) -> Option<&FieldSpec>;
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Column<F> {
    Blank,
    Field(F),
}

/// A column list, as given in config.toml: either one string in `--fields`
/// syntax, or an array of names.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum FieldSpec {
    Text(String),
    Names(Vec<String>),
}

/// The config's `[fields]` table.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct FieldsConfig {
    #[serde(default)]
    pub individual: Option<FieldSpec>,
    #[serde(default)]
    pub period: Option<FieldSpec>,
}

/// The columns to print: `--fields` if given, else the config's entry for
/// this command, else every field in default order.
pub fn resolve_columns<F: Field>(
    command_line_spec: Option<&str>,
    fields_config: &FieldsConfig,
) -> Result<Vec<Column<F>>> {
    if let Some(spec_text) = command_line_spec {
        return parse_spec_text(spec_text).map_err(|error| anyhow!("--fields: {error}"));
    }
    if let Some(spec) = F::configured_spec(fields_config) {
        return parse_spec(spec)
            .map_err(|error| anyhow!("config.toml [fields] {}: {error}", F::CONFIG_KEY));
    }
    Ok(default_columns())
}

pub fn default_columns<F: Field>() -> Vec<Column<F>> {
    F::ALL.iter().map(|&field| Column::Field(field)).collect()
}

pub fn shows_field<F: Field>(columns: &[Column<F>], field: F) -> bool {
    columns.contains(&Column::Field(field))
}

fn parse_spec<F: Field>(spec: &FieldSpec) -> Result<Vec<Column<F>>> {
    match spec {
        FieldSpec::Text(spec_text) => parse_spec_text(spec_text),
        FieldSpec::Names(names) => {
            let columns = names
                .iter()
                .map(|name| column_named(name))
                .collect::<Result<Vec<_>>>()?;
            require_some_columns(columns)
        }
    }
}

/// `url,comment,blank,action` (names, comma-separated; a single name works
/// too) or `uc_a` (one letter per column).
fn parse_spec_text<F: Field>(spec_text: &str) -> Result<Vec<Column<F>>> {
    let spec_text = spec_text.trim();
    let is_name_list = spec_text.contains(',') || column_named::<F>(spec_text).is_ok();
    let columns = if is_name_list {
        spec_text
            .split(',')
            .map(|name| column_named(name.trim()))
            .collect::<Result<Vec<_>>>()?
    } else {
        spec_text
            .chars()
            .filter(|letter| !letter.is_whitespace())
            .map(column_lettered)
            .collect::<Result<Vec<_>>>()?
    };
    require_some_columns(columns)
}

fn require_some_columns<F: Field>(columns: Vec<Column<F>>) -> Result<Vec<Column<F>>> {
    if columns.is_empty() {
        bail!(
            "no columns given — valid for {}: {}",
            F::COMMANDS,
            valid_columns_listing::<F>()
        );
    }
    Ok(columns)
}

fn column_named<F: Field>(name: &str) -> Result<Column<F>> {
    if name.eq_ignore_ascii_case(BLANK_NAME) {
        return Ok(Column::Blank);
    }
    F::ALL
        .iter()
        .find(|field| field.name().eq_ignore_ascii_case(name))
        .map(|&field| Column::Field(field))
        .with_context(|| {
            format!(
                "unknown field '{name}' for {} — valid: {}",
                F::COMMANDS,
                valid_columns_listing::<F>()
            )
        })
}

fn column_lettered<F: Field>(letter: char) -> Result<Column<F>> {
    if letter == BLANK_LETTER {
        return Ok(Column::Blank);
    }
    F::ALL
        .iter()
        .find(|field| field.letter() == letter.to_ascii_lowercase())
        .map(|&field| Column::Field(field))
        .with_context(|| {
            format!(
                "unknown field letter '{letter}' for {} — valid: {}",
                F::COMMANDS,
                valid_columns_listing::<F>()
            )
        })
}

/// `url (u), comment (c), ..., blank (_)`
fn valid_columns_listing<F: Field>() -> String {
    F::ALL
        .iter()
        .map(|field| format!("{} ({})", field.name(), field.letter()))
        .chain(std::iter::once(format!("{BLANK_NAME} ({BLANK_LETTER})")))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum TestField {
        Url,
        Open,
    }

    impl Field for TestField {
        const ALL: &'static [Self] = &[TestField::Url, TestField::Open];
        const COMMANDS: &'static str = "test";
        const CONFIG_KEY: &'static str = "individual";

        fn name(self) -> &'static str {
            match self {
                TestField::Url => "url",
                TestField::Open => "open",
            }
        }
        fn letter(self) -> char {
            match self {
                TestField::Url => 'u',
                TestField::Open => 'o',
            }
        }
        fn configured_spec(fields_config: &FieldsConfig) -> Option<&FieldSpec> {
            fields_config.individual.as_ref()
        }
    }

    use Column::{Blank, Field as F};
    use TestField::{Open, Url};

    #[test]
    fn names_and_letters_give_the_same_columns() {
        let by_names = parse_spec_text::<TestField>("url, blank,open").unwrap();
        let by_letters = parse_spec_text::<TestField>("u_o").unwrap();
        assert_eq!(by_names, vec![F(Url), Blank, F(Open)]);
        assert_eq!(by_letters, by_names);
    }

    #[test]
    fn a_single_name_is_not_read_as_letters() {
        assert_eq!(parse_spec_text::<TestField>("open").unwrap(), vec![F(Open)]);
    }

    #[test]
    fn unknown_fields_are_rejected_with_the_valid_list() {
        let error = parse_spec_text::<TestField>("u_x").unwrap_err().to_string();
        assert!(
            error.contains("'x'") && error.contains("url (u)"),
            "{error}"
        );
        assert!(parse_spec_text::<TestField>("url,nope").is_err());
        assert!(parse_spec_text::<TestField>("  ").is_err());
    }

    #[test]
    fn command_line_beats_config_beats_default() {
        let fields_config = FieldsConfig {
            individual: Some(FieldSpec::Names(vec!["open".into()])),
            period: None,
        };
        assert_eq!(
            resolve_columns::<TestField>(Some("u"), &fields_config).unwrap(),
            vec![F(Url)]
        );
        assert_eq!(
            resolve_columns::<TestField>(None, &fields_config).unwrap(),
            vec![F(Open)]
        );
        assert_eq!(
            resolve_columns::<TestField>(None, &FieldsConfig::default()).unwrap(),
            vec![F(Url), F(Open)]
        );
    }
}
