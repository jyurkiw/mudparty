//! Reading config files, with a revision of each one's content (design §5.1), and
//! `secrets.yaml` (§7.4).

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::character::CharacterConfig;
use crate::error::ConfigError;
use crate::server::ServerConfig;

/// A loaded file and the revision of the bytes it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded<T> {
    pub value: T,
    /// The first 12 hex digits of the content's SHA-256: `config_rev`.
    pub rev: String,
}

/// The revision of `bytes`.
pub fn revision(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn read(path: &Path) -> Result<String, ConfigError> {
    std::fs::read_to_string(path).map_err(|e| ConfigError::new(e.to_string()).in_file(path))
}

/// Reads `mudparty.toml`.
pub fn load_server(path: &Path) -> Result<Loaded<ServerConfig>, ConfigError> {
    let text = read(path)?;
    let value = ServerConfig::from_toml(&text).map_err(|e| e.in_file(path))?;
    Ok(Loaded {
        value,
        rev: revision(text.as_bytes()),
    })
}

/// Reads `characters/<name>.yaml`. The file's name must be the character's.
pub fn load_character(path: &Path) -> Result<Loaded<CharacterConfig>, ConfigError> {
    let text = read(path)?;
    let value = character_from_text(path, &text)?;
    Ok(Loaded {
        value,
        rev: revision(text.as_bytes()),
    })
}

/// Reads a character's YAML as if it were the file at `path`, whose name must be the
/// character's: what the page's editor checks before it saves (#110).
pub fn character_from_text(path: &Path, text: &str) -> Result<CharacterConfig, ConfigError> {
    let value = CharacterConfig::from_yaml(text).map_err(|e| e.in_file(path))?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    if !stem.eq_ignore_ascii_case(&value.name) {
        return Err(ConfigError::at(
            "name",
            format!("{:?} is in a file named for {stem:?}", value.name),
        )
        .in_file(path));
    }
    Ok(value)
}

/// `secrets.yaml`: each account's password. Printing it never shows one.
#[derive(Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Secrets {
    #[serde(default)]
    accounts: BTreeMap<String, String>,
}

impl fmt::Debug for Secrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map()
            .entries(self.accounts.keys().map(|k| (k, "********")))
            .finish()
    }
}

impl Secrets {
    /// Reads a `secrets.yaml`. An error says where, never what the text was.
    pub fn from_yaml(text: &str) -> Result<Self, ConfigError> {
        serde_norway::from_str(text).map_err(|e| {
            let at = e
                .location()
                .map(|l| format!(" at line {} column {}", l.line(), l.column()))
                .unwrap_or_default();
            ConfigError::new(format!("not readable as secrets{at}"))
        })
    }

    /// Reads `secrets.yaml` from `path`.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        Self::from_yaml(&read(path)?).map_err(|e| e.in_file(path))
    }

    /// The password for `account`.
    pub fn password(&self, account: &str) -> Option<&str> {
        self.accounts.get(account).map(String::as_str)
    }

    /// Sets `account`'s password, replacing any it had (#110).
    pub fn set_password(&mut self, account: &str, password: &str) {
        self.accounts
            .insert(account.to_owned(), password.to_owned());
    }

    /// As `secrets.yaml` writes it: every account, in order.
    pub fn to_yaml(&self) -> String {
        let mut out = String::from("accounts:\n");
        for (account, password) in &self.accounts {
            let quote = |s: &str| serde_json::to_string(s).expect("a string always writes");
            out.push_str(&format!("  {}: {}\n", quote(account), quote(password)));
        }
        out
    }
}
