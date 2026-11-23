//! Config errors, naming the file and field where they can.

use std::fmt;
use std::path::{Path, PathBuf};

/// A config file mudparty can't use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub file: Option<PathBuf>,
    /// The field, as a path: `rest.stop_when_all.hp`, `combat.tiers[1].min_mp`.
    pub field: Option<String>,
    pub message: String,
}

impl ConfigError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            file: None,
            field: None,
            message: message.into(),
        }
    }

    pub(crate) fn at(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            file: None,
            field: Some(field.into()),
            message: message.into(),
        }
    }

    pub(crate) fn in_file(mut self, file: &Path) -> Self {
        self.file = Some(file.to_owned());
        self
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(file) = &self.file {
            write!(f, "{}: ", file.display())?;
        }
        if let Some(field) = &self.field {
            write!(f, "{field}: ")?;
        }
        f.write_str(&self.message)
    }
}

impl std::error::Error for ConfigError {}
