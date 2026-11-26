//! mudparty's automation (design §6): the login state machine, the combat
//! engine and the session runner.

mod combat;
mod login;
mod pacing;
mod party;
mod remote;
mod rest;
mod routine;
mod session;
mod walk;

pub use combat::{Combat, CombatSettings};
pub use login::{Login, LoginAction, LoginFailure, LoginSettings};
pub use pacing::{Action, Pacer, PacingSettings};
pub use party::{PartyJoin, PartySettings, PartyState};
pub use remote::{
    AutoSwitch, BotView, Effect, ExpKnown, Query, Remote, RemoteSettings, RoomBook, RoomInfo,
    exp_reply, status_reply, where_reply,
};
pub use rest::{RestSettings, Rester};
pub use routine::{Goal, HuntSettings, Routine, RoutinePart, RoutineSettings, RuleSettings, Seen};
pub use session::{
    AutoState, Command, Inviter, Offline, Report, Seat, SeatInput, SessionHandle, SessionSpec,
    SessionState, StatlineStore, Upstream, spawn, spawn_seated,
};
pub use walk::{RouteRules, WalkState, WalkStep, Walker};
