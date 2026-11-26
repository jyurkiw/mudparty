//! Walking to a room (#52), as pure logic: parsed items and a clock in, lines out.
//!
//! The walker finds where it is with `/room` (a silvermere fork command: `1/206`), plans
//! the route over the game data's exits ([`RoomBook::route`]), and walks it a step at a
//! time. After each move it waits for the room display's `Obvious exits:` line, then
//! confirms the room with `/room`, so a wrong turn is caught at once and the route is
//! planned again from wherever the bot is.
//!
//! Game behaviour it relies on (silvermere's movement and exits, from Stock 1.11p):
//! - A move that arrives prints the new room, ending with `Obvious exits:`.
//! - `There is no exit in that direction!` means the bot isn't where it thought.
//! - A closed door or gate says `The door is closed!` (or `Closed!` when locked).
//!   `open <dir>` answers `The door is now open.` (or `was already open`), or
//!   `The door is locked.`. A locked door is bashed (`bash <dir>`): strength plus the
//!   door's bonus against a 0-100 roll, two ticks a try, and one try in four does 1-4
//!   damage. It locks again after a while.
//! - Level, alignment, timed and other gated exits refuse with their own lines
//!   ([`Event::ExitRefused`]); a walk can't get past those.
//! - Moving mid-fight would flee, so the walker waits while the bot fights.

use std::sync::Arc;
use std::time::{Duration, Instant};

use mp_parse::{Event, Item};

use crate::remote::RoomBook;

/// One step of a route: the direction, the room it reaches, and whether a door or gate
/// stands in the way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalkStep {
    /// silvermere's direction number, 0 north … 9 down.
    pub dir: u8,
    pub to: (u32, u32),
    pub door: bool,
}

/// Where a walk stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalkState {
    /// Asking the game where the bot is.
    Locating,
    /// On step `step` of `of`.
    Walking {
        step: usize,
        of: usize,
    },
    /// Waiting out a fight.
    Paused,
    Arrived,
    Failed(String),
}

const DIRECTIONS: [&str; 10] = [
    "north",
    "south",
    "east",
    "west",
    "northeast",
    "northwest",
    "southeast",
    "southwest",
    "up",
    "down",
];

/// How many times a walk asks again after no answer, or plans again after a wrong room,
/// before it gives up.
const RETRIES: u32 = 3;
/// How many bashes at a locked door before the walk gives up.
const BASHES: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// `/room` sent.
    Locate,
    /// A direction sent.
    Move,
    /// Arrived somewhere; `/room` sent to see where.
    Confirm,
    /// `open <dir>` sent.
    Open,
    /// `bash <dir>` sent.
    Bash,
    Paused,
    Done,
}

#[derive(Debug)]
pub struct Walker {
    to: (u32, u32),
    book: Arc<dyn RoomBook>,
    step_wait: Duration,
    plan: Vec<WalkStep>,
    at: usize,
    phase: Phase,
    since: Instant,
    /// Asks with no answer, in a row.
    unanswered: u32,
    /// Wrong rooms, over the walk.
    replans: u32,
    /// Bashes at the door in the way.
    bashes: u32,
    state: WalkState,
}

impl Walker {
    /// A walk to `to`. `step_wait`: how long a line may go unanswered before the walker
    /// asks where it is again.
    pub fn new(to: (u32, u32), book: Arc<dyn RoomBook>, step_wait: Duration) -> Self {
        Self {
            to,
            book,
            step_wait,
            plan: Vec::new(),
            at: 0,
            phase: Phase::Locate,
            since: Instant::now(),
            unanswered: 0,
            replans: 0,
            bashes: 0,
            state: WalkState::Locating,
        }
    }

    pub fn state(&self) -> WalkState {
        self.state.clone()
    }

    pub fn destination(&self) -> (u32, u32) {
        self.to
    }

    /// Whether the walk is over, arrived or failed.
    pub fn done(&self) -> bool {
        self.phase == Phase::Done
    }

    /// The first line: where is the bot?
    pub fn start(&mut self, now: Instant) -> Vec<String> {
        self.locate(now)
    }

    fn locate(&mut self, now: Instant) -> Vec<String> {
        self.phase = Phase::Locate;
        self.since = now;
        self.state = WalkState::Locating;
        vec!["/room".into()]
    }

    fn fail(&mut self, why: impl Into<String>) -> Vec<String> {
        self.phase = Phase::Done;
        self.state = WalkState::Failed(why.into());
        Vec::new()
    }

    fn arrived(&mut self) -> Vec<String> {
        self.phase = Phase::Done;
        self.state = WalkState::Arrived;
        Vec::new()
    }

    /// Plans from `here` and takes the first step.
    fn plan_from(&mut self, here: (u32, u32), now: Instant) -> Vec<String> {
        if here == self.to {
            return self.arrived();
        }
        match self.book.route(here, self.to) {
            Some(plan) if !plan.is_empty() => {
                self.plan = plan;
                self.at = 0;
                self.step(now)
            }
            _ => {
                let (m, r) = (here, self.to);
                self.fail(format!(
                    "no way from {}/{} to {}/{} by walking",
                    m.0, m.1, r.0, r.1
                ))
            }
        }
    }

    fn step(&mut self, now: Instant) -> Vec<String> {
        let s = self.plan[self.at];
        self.phase = Phase::Move;
        self.since = now;
        self.state = WalkState::Walking {
            step: self.at + 1,
            of: self.plan.len(),
        };
        vec![DIRECTIONS[usize::from(s.dir) % 10].to_owned()]
    }

    /// The next item the game sent. `fighting`: the bot is in a fight.
    pub fn on_item(&mut self, item: &Item, now: Instant, fighting: bool) -> Vec<String> {
        if self.phase == Phase::Done {
            return Vec::new();
        }
        if fighting || matches!(item, Item::Event(Event::CombatEngaged)) {
            if self.phase != Phase::Paused {
                self.phase = Phase::Paused;
                self.state = WalkState::Paused;
            }
            return Vec::new();
        }
        let Item::Event(e) = item else {
            return Vec::new();
        };
        match (self.phase, e) {
            (Phase::Locate, Event::RoomKey { map, room }) => {
                self.unanswered = 0;
                self.plan_from((*map, *room), now)
            }
            (Phase::Confirm, Event::RoomKey { map, room }) => {
                self.unanswered = 0;
                let here = (*map, *room);
                if here == self.plan[self.at].to {
                    self.at += 1;
                    if self.at == self.plan.len() {
                        return self.arrived();
                    }
                    return self.step(now);
                }
                // Somewhere else: plan again from there, a few times.
                self.replans += 1;
                if self.replans > RETRIES {
                    return self.fail(format!("lost the way at {}/{}", here.0, here.1));
                }
                self.plan_from(here, now)
            }
            (Phase::Move, Event::ObviousExits) => {
                self.phase = Phase::Confirm;
                self.since = now;
                vec!["/room".into()]
            }
            (Phase::Move, Event::NoExit) => self.locate(now),
            (Phase::Move, Event::DoorClosed) => {
                self.phase = Phase::Open;
                self.since = now;
                let dir = DIRECTIONS[usize::from(self.plan[self.at].dir) % 10];
                vec![format!("open {dir}")]
            }
            (Phase::Move | Phase::Open, Event::ExitRefused { text }) => self.fail(text.clone()),
            (Phase::Open, Event::DoorOpened)
            | (Phase::Bash, Event::BashedOpen | Event::DoorOpened) => {
                self.bashes = 0;
                self.step(now)
            }
            (Phase::Open, Event::DoorLocked) | (Phase::Bash, Event::BashFailed) => {
                let dir = DIRECTIONS[usize::from(self.plan[self.at].dir) % 10];
                self.bashes += 1;
                if self.bashes >= BASHES {
                    return self.fail(format!(
                        "the door to the {dir} is locked, and wouldn't break"
                    ));
                }
                self.phase = Phase::Bash;
                self.since = now;
                vec![format!("bash {dir}")]
            }
            _ => Vec::new(),
        }
    }

    /// Time passing: a fight that's over lets the walk go on; a line with no answer is
    /// asked again.
    pub fn on_tick(&mut self, now: Instant, fighting: bool) -> Vec<String> {
        match self.phase {
            Phase::Done => Vec::new(),
            Phase::Paused if fighting => Vec::new(),
            // Over: find where the fight left the bot, and go on from there.
            Phase::Paused => {
                self.unanswered = 0;
                self.locate(now)
            }
            _ if fighting => {
                self.phase = Phase::Paused;
                self.state = WalkState::Paused;
                Vec::new()
            }
            _ if now.duration_since(self.since) >= self.step_wait => {
                self.unanswered += 1;
                if self.unanswered > RETRIES {
                    return self.fail("the game gave no answer");
                }
                self.locate(now)
            }
            _ => Vec::new(),
        }
    }
}
