//! mudparty's automation (design §6): the login state machine, the combat
//! engine and the session runner.

mod combat;
mod login;
mod pacing;
mod party;
mod remote;
mod rest;
mod session;

pub use combat::{Combat, CombatSettings};
pub use login::{Login, LoginAction, LoginFailure, LoginSettings};
pub use pacing::{Action, Pacer, PacingSettings};
pub use party::{PartyJoin, PartySettings, PartyState};
pub use remote::{
    AutoSwitch, BotView, Effect, ExpKnown, Query, Remote, RemoteSettings, RoomBook, RoomInfo,
    exp_reply, status_reply, where_reply,
};
pub use rest::{RestSettings, Rester};
pub use session::{
    AutoState, Command, Offline, Report, SessionHandle, SessionSpec, SessionState, spawn,
};
