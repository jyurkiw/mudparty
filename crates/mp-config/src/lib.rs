//! mudparty's configuration (design §7): the server's `mudparty.toml`, each character's
//! YAML, and `secrets.yaml`. Every file is checked when read, and an error names the file
//! and the field.

mod character;
mod error;
mod files;
mod place;
mod routine;
pub mod schema;
mod server;
mod units;

pub use character::{
    Amount, Automation, BOT_STATLINE, Channel, CharacterConfig, Combat, Gear, GearUse, Login,
    LoginStep, LoginSteps, Melee, Party, PartyRole, Pools, Remote, Require, Rest, RoomTier, Rule,
    StealthOpener, Tier, Unattended,
};
pub use error::ConfigError;
pub use files::{Loaded, Secrets, load_character, load_server, revision};
pub use place::Place;
pub use routine::{
    Avoid, CampRule, GearRule, HuntRule, RestRule, Routine, RoutineRef, RuleSpec, When,
    load_routine,
};
pub use server::{Game, GameData, Listen, Logging, RollOverride, ServerConfig, Stale, StreamRoll};
pub use units::{format_duration, parse_duration, parse_size};
