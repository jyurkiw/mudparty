//! Where log times come from, so tests can fix them.

use jiff::Timestamp;

/// A source of the current time.
pub trait Clock: Send {
    fn now(&self) -> Timestamp;
}

/// The real clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }
}

/// A clock that always reads the same time.
#[derive(Debug, Clone, Copy)]
pub struct FixedClock(Timestamp);

impl FixedClock {
    /// A clock stopped at `rfc3339`, e.g. `2026-11-23T10:00:00.250Z`.
    pub fn at(rfc3339: &str) -> Self {
        Self(rfc3339.parse().expect("an RFC 3339 time"))
    }
}

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        self.0
    }
}

/// A log time: UTC to the millisecond.
pub(crate) fn stamp(ts: Timestamp) -> String {
    format!("{ts:.3}")
}
