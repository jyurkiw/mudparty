//! mudparty's configuration (design §7): the server's `mudparty.toml`, each character's
//! YAML, and `secrets.yaml`. Every file is checked when read, and an error names the file
//! and the field.

mod character;
mod error;
mod files;
mod server;
mod units;

pub use character::{
    Amount, Automation, Avoid, BOT_STATLINE, Channel, CharacterConfig, Combat, Hunt, Login,
    LoginStep, LoginSteps, Melee, Party, PartyRole, Pools, Remote, Require, Rest, RoomTier,
    Routine, Rule, StealthOpener, Tier, Unattended,
};
pub use error::ConfigError;
pub use files::{Loaded, Secrets, load_character, load_server, revision};
pub use server::{Game, GameData, Listen, Logging, RollOverride, ServerConfig, Stale, StreamRoll};
pub use units::{parse_duration, parse_size};
