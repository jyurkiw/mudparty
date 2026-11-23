//! `mudparty.toml` (design §7.2).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use crate::error::ConfigError;
use crate::units;

/// The server's config.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub schema: u32,
    /// Whose stream the proxy carries.
    #[serde(default)]
    pub main_character: Option<String>,
    /// How long each bot gets to log off on an orderly stop.
    #[serde(default = "d_logoff", deserialize_with = "units::duration")]
    pub logoff_timeout: Duration,
    pub game: Game,
    #[serde(default)]
    pub listen: Listen,
    pub gamedata: GameData,
    #[serde(default)]
    pub stale: Stale,
    #[serde(default)]
    pub logging: Logging,
}

/// The silvermere server.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Game {
    pub host: String,
    pub port: u16,
}

/// Where mudparty listens. Neither may use the user's silvermere ports.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Listen {
    /// `/ws/client`, `/ws/ui` and the web page.
    #[serde(default = "d_hub")]
    pub client_hub: SocketAddr,
    /// Where MudPlay connects.
    #[serde(default = "d_proxy")]
    pub proxy: SocketAddr,
}

impl Default for Listen {
    fn default() -> Self {
        Self {
            client_hub: d_hub(),
            proxy: d_proxy(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameData {
    /// silvermere's database, read for the import.
    pub silvermere_db: PathBuf,
}

/// When party state counts as stale (design §5.3).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stale {
    #[serde(default = "d_vitals", deserialize_with = "units::duration")]
    pub vitals: Duration,
    #[serde(default = "d_room", deserialize_with = "units::duration")]
    pub room: Duration,
    #[serde(default = "d_target", deserialize_with = "units::duration")]
    pub target: Duration,
}

impl Default for Stale {
    fn default() -> Self {
        Self {
            vitals: d_vitals(),
            room: d_room(),
            target: d_target(),
        }
    }
}

/// How the log streams roll (design §9.3), with per-stream overrides.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Logging {
    #[serde(default = "d_size", deserialize_with = "size")]
    pub file_size: u64,
    #[serde(default = "d_plain")]
    pub plain_files: usize,
    #[serde(default = "d_zips")]
    pub zip_archives: usize,
    #[serde(default)]
    pub wire: Option<RollOverride>,
    #[serde(default)]
    pub transcript: Option<RollOverride>,
    #[serde(default)]
    pub decisions: Option<RollOverride>,
    #[serde(default)]
    pub events: Option<RollOverride>,
    #[serde(default)]
    pub server: Option<RollOverride>,
}

impl Default for Logging {
    fn default() -> Self {
        Self {
            file_size: d_size(),
            plain_files: d_plain(),
            zip_archives: d_zips(),
            wire: None,
            transcript: None,
            decisions: None,
            events: None,
            server: None,
        }
    }
}

/// One stream's changes to the rolling.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RollOverride {
    #[serde(default, deserialize_with = "units::opt_size")]
    pub file_size: Option<u64>,
    #[serde(default)]
    pub plain_files: Option<usize>,
    #[serde(default)]
    pub zip_archives: Option<usize>,
}

/// The rolling one stream uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamRoll {
    pub file_size: u64,
    pub plain_files: usize,
    pub zip_archives: usize,
}

impl Logging {
    /// The rolling for `stream` (`wire`, `transcript`, `decisions`, `events`, `server`).
    pub fn roll_for(&self, stream: &str) -> StreamRoll {
        let o = match stream {
            "wire" => self.wire.as_ref(),
            "transcript" => self.transcript.as_ref(),
            "decisions" => self.decisions.as_ref(),
            "events" => self.events.as_ref(),
            "server" => self.server.as_ref(),
            _ => None,
        };
        let o = o.cloned().unwrap_or_default();
        StreamRoll {
            file_size: o.file_size.unwrap_or(self.file_size),
            plain_files: o.plain_files.unwrap_or(self.plain_files),
            zip_archives: o.zip_archives.unwrap_or(self.zip_archives),
        }
    }
}

fn size<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    units::opt_size(d)?.ok_or_else(|| serde::de::Error::custom("a size is required"))
}

fn d_logoff() -> Duration {
    Duration::from_secs(30)
}
fn d_hub() -> SocketAddr {
    "127.0.0.1:7171".parse().expect("a literal address")
}
fn d_proxy() -> SocketAddr {
    "127.0.0.1:2424".parse().expect("a literal address")
}
fn d_vitals() -> Duration {
    Duration::from_secs(15)
}
fn d_room() -> Duration {
    Duration::from_secs(30)
}
fn d_target() -> Duration {
    Duration::from_secs(6)
}
fn d_size() -> u64 {
    1024 * 1024
}
fn d_plain() -> usize {
    2
}
fn d_zips() -> usize {
    3
}

impl ServerConfig {
    /// Reads and checks a `mudparty.toml`.
    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        let c: Self = toml::from_str(text).map_err(|e| ConfigError::new(e.to_string()))?;
        c.validate()?;
        Ok(c)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.schema != 1 {
            return Err(ConfigError::at(
                "schema",
                format!(
                    "schema {} isn't supported (this mudparty reads schema 1)",
                    self.schema
                ),
            ));
        }
        if self.listen.proxy == self.listen.client_hub {
            return Err(ConfigError::at(
                "listen.proxy",
                "the proxy and the client hub need ports of their own",
            ));
        }
        if self.logging.plain_files == 0 {
            return Err(ConfigError::at(
                "logging.plain_files",
                "at least 1, the live file",
            ));
        }
        Ok(())
    }
}
