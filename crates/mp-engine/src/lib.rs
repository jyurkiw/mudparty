//! mudparty's automation (design §6): the login state machine, the combat
//! engine and the session runner.

mod combat;
mod login;
mod session;

pub use combat::{Combat, CombatSettings};
pub use login::{Login, LoginAction, LoginFailure, LoginSettings};
pub use session::{Command, Offline, Report, SessionHandle, SessionSpec, SessionState, spawn};
