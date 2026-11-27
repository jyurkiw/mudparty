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
mod shop;
mod walk;

pub use combat::{Combat, CombatSettings};
pub use login::{Login, LoginAction, LoginFailure, LoginSettings, ScriptStep};
pub use pacing::{Action, Pacer, PacingSettings};
pub use party::{PartyJoin, PartySettings, PartyState};
pub use remote::{
    AutoSwitch, BotView, Effect, ExpKnown, Query, Remote, RemoteSettings, RoomBook, RoomInfo,
    exp_reply, status_reply, where_reply,
};
pub use rest::{RestSettings, Rester};
pub use routine::{
    CircuitSettings, GearSettings, Goal, LairClocks, LairSettings, Routine, RoutinePart,
    RoutineSettings, RuleSettings, Seen, TrainEnd,
};
pub use session::{
    AutoState, Command, Inviter, Offline, Report, Seat, SeatInput, SessionHandle, SessionSpec,
    SessionState, StatlineStore, Upstream, spawn, spawn_seated,
};
pub use shop::{Carried, EquipState, Equipper, InventoryReader, parse_carried};
pub use walk::{RouteRules, WalkState, WalkStep, Walker};
