//! The pattern table: which lines mudparty knows and the [`Event`] each one builds.
//!
//! The table is data, `patterns.toml` in this folder, read once. Each entry's named regex
//! groups become the event's fields through serde (see `event.rs`), so a new pattern is a
//! new entry. A wrong event or field name is not a compile error; it fails the catalogue
//! test (design §10.2), which reads every entry against silvermere's own text.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::event::Event;

const BUILTIN: &str = include_str!("patterns.toml");

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Table {
    pattern: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    event: String,
    keys: Vec<String>,
    re: String,
    #[serde(default)]
    numbers: Vec<String>,
    #[serde(default)]
    split: BTreeMap<String, String>,
}

/// A table that can't be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternError(String);

impl fmt::Display for PatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PatternError {}

#[derive(Debug)]
struct Pattern {
    entry: Entry,
    re: Regex,
}

/// A loaded pattern table.
#[derive(Debug)]
pub struct Patterns {
    patterns: Vec<Pattern>,
}

static BUILT_IN: LazyLock<Patterns> =
    LazyLock::new(|| Patterns::from_toml(BUILTIN).expect("src/patterns.toml loads"));

impl Patterns {
    /// The table in `src/patterns.toml`.
    pub fn builtin() -> &'static Patterns {
        &BUILT_IN
    }

    /// Loads a table. Every regex must compile and every entry name a catalogue key.
    pub fn from_toml(text: &str) -> Result<Self, PatternError> {
        let table: Table = toml::from_str(text).map_err(|e| PatternError(e.to_string()))?;
        let patterns = table
            .pattern
            .into_iter()
            .map(|entry| {
                if entry.keys.is_empty() {
                    return Err(PatternError(format!(
                        "{}: names no catalogue key",
                        entry.event
                    )));
                }
                let re = Regex::new(&entry.re)
                    .map_err(|e| PatternError(format!("{}: {e}", entry.event)))?;
                Ok(Pattern { entry, re })
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { patterns })
    }

    /// What a whole plain-text line means, if the table knows it. The first entry that
    /// matches wins.
    pub fn classify(&self, plain: &str) -> Option<Event> {
        self.patterns.iter().find_map(|p| p.build(plain))
    }

    /// Every catalogue key the table reads, sorted, once each.
    pub fn keys(&self) -> Vec<&str> {
        let mut keys: Vec<&str> = self
            .patterns
            .iter()
            .flat_map(|p| p.entry.keys.iter().map(String::as_str))
            .collect();
        keys.sort_unstable();
        keys.dedup();
        keys
    }
}

impl Pattern {
    fn build(&self, plain: &str) -> Option<Event> {
        let caps = self.re.captures(plain)?;
        let mut fields = Map::new();
        fields.insert("event".into(), Value::String(self.entry.event.clone()));
        for name in self.re.capture_names().flatten() {
            let Some(m) = caps.name(name) else { continue };
            let text = m.as_str();
            let value = if self.entry.numbers.iter().any(|n| n == name) {
                Value::Number(text.trim().parse::<i64>().ok()?.into())
            } else if let Some(sep) = self.entry.split.get(name) {
                Value::Array(
                    text.split(sep.as_str())
                        .map(|s| Value::String(s.to_owned()))
                        .collect(),
                )
            } else {
                Value::String(text.to_owned())
            };
            fields.insert(name.to_owned(), value);
        }
        match serde_json::from_value(Value::Object(fields)) {
            Ok(event) => Some(event),
            Err(e) => {
                // The table and the Event type disagree: a bug the catalogue test exists to
                // catch, never a property of the game's text.
                debug_assert!(false, "pattern {} builds no event: {e}", self.entry.event);
                None
            }
        }
    }
}

/// What a whole plain-text line means, by the built-in table.
pub fn classify(plain: &str) -> Option<Event> {
    Patterns::builtin().classify(plain)
}

/// Every catalogue key the built-in table reads, for the catalogue test.
pub fn pattern_keys() -> Vec<&'static str> {
    Patterns::builtin().keys()
}
