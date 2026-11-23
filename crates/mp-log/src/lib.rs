//! mudparty's logs (design §9): log everything, all the time.
//!
//! - [`RollingFile`]: one stream's files, rolled at a size on a record boundary, the newest
//!   kept plain, older ones zipped on a background thread, the oldest deleted.
//! - [`CharacterLogs`]: a character's wire capture, transcript, decisions and events, with
//!   one sequence shared by all four so they line up.
//! - [`ServerLog`]: the server's own stream.

mod clock;
mod rolling;
mod streams;

pub use clock::{Clock, FixedClock, SystemClock};
pub use rolling::{RollConfig, RollingFile};
pub use streams::{CharacterLogs, Header, ServerLog, Source};
