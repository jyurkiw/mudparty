//! mudparty's automation (design §6): the login state machine and the session runner.

mod login;
mod session;

pub use login::{Login, LoginAction, LoginFailure, LoginSettings};
pub use session::{Command, Offline, Report, SessionHandle, SessionSpec, SessionState, spawn};
