//! Reading silvermere's text: the prompt matcher, the game lines mudparty acts on, and the
//! reader that puts them together over a byte stream.

mod event;
mod patterns;
mod reader;
mod statline;

pub use event::{Event, PartyRow, Rank};
pub use patterns::{PatternError, Patterns, classify, pattern_keys};
pub use reader::{Item, Reader};
pub use statline::{Rest, Statline, StatlineError, Vitals};
