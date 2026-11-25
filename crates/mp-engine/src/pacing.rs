//! Command pacing and telepath acknowledgements (design §6.1), as pure logic.
//!
//! Game behaviour it relies on (silvermere's `add_delayed_command`, after Stock 1.11p, and
//! MudPlay's GAME_MECHANICS "Wire → Command rate limit"):
//! - Lines typed faster than the game runs them wait in a per-player queue. The tenth line
//!   waiting gets `Why don't you slow down for a few seconds?`, and from the fourteenth
//!   each line is dropped with `You are typing too quickly - command ignored`. So a client
//!   that keeps only a few lines unanswered never loses one; a drop means the last line
//!   sent is sent again.
//! - Each command is answered by a prompt, so "one more line per prompt" keeps the game's
//!   queue short. Prompts also come unasked (rounds, regeneration), which only lets a line
//!   go a little sooner.
//! - A telepath is `/<name> <text>`. The game answers each one, in order, with
//!   `--- Telepath Sent to <Name> ---` or `--- Telepath Not Sent ---` (out of pages,
//!   blocked, or forgotten; it doesn't say which).
//! - **Pages:** a character receives at most `pagemax` + 1 telepaths (`SET MAXPAGES`,
//!   default 3) in each 3 s medium cycle, and silvermere's `cycles::medium` gives them back
//!   (`comms/allowance.rs`). A telepath sent again inside the same cycle fails again, so
//!   one not sent waits a cycle before its next try, and lines behind it go on meanwhile.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use mp_parse::{Event, Item};

/// A line or a telepath one of the engines wants sent; the session paces it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Send(String),
    Telepath { to: String, text: String },
}

/// How fast lines may go.
#[derive(Debug, Clone)]
pub struct PacingSettings {
    /// The least time between two lines.
    pub floor: Duration,
    /// The least time between a telepath and the line before it.
    pub telepath_floor: Duration,
    /// Lines sent with no prompt since; one more may go per prompt.
    pub max_unanswered: u32,
    /// With no prompt for this long, the unanswered lines are taken as answered.
    pub prompt_timeout: Duration,
    /// How many times a telepath is sent before it is given up.
    pub telepath_tries: u32,
    /// How long a telepath not sent waits before it is tried again: the game's page cycle.
    pub telepath_retry: Duration,
}

impl Default for PacingSettings {
    fn default() -> Self {
        Self {
            floor: Duration::from_millis(100),
            telepath_floor: Duration::from_millis(100),
            max_unanswered: 3,
            prompt_timeout: Duration::from_secs(3),
            telepath_tries: 3,
            telepath_retry: Duration::from_secs(3),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Out {
    Line(String),
    Telepath {
        who: String,
        text: String,
        tries: u32,
    },
}

impl Out {
    fn text(&self) -> String {
        match self {
            Self::Line(l) => l.clone(),
            Self::Telepath { who, text, .. } => format!("/{who} {text}"),
        }
    }
}

/// The lines waiting to go to the game, and the telepaths waiting for their answer.
#[derive(Debug)]
pub struct Pacer {
    set: PacingSettings,
    queue: VecDeque<Out>,
    unanswered: u32,
    last_send: Option<Instant>,
    last_prompt: Option<Instant>,
    /// The last line sent, in case the game drops it.
    last: Option<Out>,
    /// Telepaths sent and not yet answered, oldest first.
    acks: VecDeque<Out>,
    /// Telepaths not sent, each with when it may be tried again, soonest first.
    retries: VecDeque<(Instant, Out)>,
}

impl Pacer {
    pub fn new(set: PacingSettings) -> Self {
        Self {
            set,
            queue: VecDeque::new(),
            unanswered: 0,
            last_send: None,
            last_prompt: None,
            last: None,
            acks: VecDeque::new(),
            retries: VecDeque::new(),
        }
    }

    /// Queues a line.
    pub fn push(&mut self, line: impl Into<String>) {
        self.queue.push_back(Out::Line(line.into()));
    }

    /// Queues a telepath to `who`.
    pub fn telepath(&mut self, who: &str, text: &str) {
        self.queue.push_back(Out::Telepath {
            who: who.to_owned(),
            text: text.to_owned(),
            tries: 0,
        });
    }

    /// Telepaths sent and not yet answered.
    pub fn pending_telepaths(&self) -> usize {
        self.acks.len()
    }

    /// Whether nothing is waiting to go.
    pub fn is_idle(&self) -> bool {
        self.queue.is_empty() && self.retries.is_empty()
    }

    fn floor_for(&self, o: &Out) -> Duration {
        match o {
            Out::Line(_) => self.set.floor,
            Out::Telepath { .. } => self.set.telepath_floor,
        }
    }

    /// When the next line may go, if one is waiting. While the prompt gate is shut that is
    /// when it would time out; a prompt before then opens it sooner.
    pub fn next_at(&self) -> Option<Instant> {
        let gate = (self.unanswered >= self.set.max_unanswered)
            .then(|| self.last_prompt.max(self.last_send))
            .flatten()
            .map(|at| at + self.set.prompt_timeout);
        let when = |o: &Out, due: Option<Instant>| {
            let floor = self.last_send.map(|at| at + self.floor_for(o));
            floor.max(gate).max(due).unwrap_or_else(Instant::now)
        };
        let queued = self.queue.front().map(|o| when(o, None));
        let retry = self.retries.front().map(|(at, o)| when(o, Some(*at)));
        match (queued, retry) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn gate_open(&mut self, now: Instant) -> bool {
        if self.unanswered < self.set.max_unanswered {
            return true;
        }
        let since = self.last_prompt.max(self.last_send);
        if since.is_some_and(|at| now.duration_since(at) >= self.set.prompt_timeout) {
            self.unanswered = 0;
            return true;
        }
        false
    }

    /// The lines that may go now; at most one, so the floor holds between them.
    pub fn ready(&mut self, now: Instant) -> Vec<String> {
        // A retry whose cycle has passed goes before the queue: it was sent first.
        let due = self.retries.front().is_some_and(|(at, _)| *at <= now);
        let next = if due {
            &self.retries.front().expect("a retry is due").1
        } else {
            match self.queue.front() {
                Some(o) => o,
                None => return Vec::new(),
            }
        };
        if self
            .last_send
            .is_some_and(|at| now.duration_since(at) < self.floor_for(next))
        {
            return Vec::new();
        }
        if !self.gate_open(now) {
            return Vec::new();
        }
        let mut out = if due {
            self.retries.pop_front().expect("a retry is due").1
        } else {
            self.queue.pop_front().expect("a line is waiting")
        };
        if let Out::Telepath { tries, .. } = &mut out {
            *tries += 1;
            self.acks.push_back(out.clone());
        }
        self.unanswered += 1;
        self.last_send = Some(now);
        let text = out.text();
        self.last = Some(out);
        vec![text]
    }

    /// The next item the game sent. Returns the telepaths given up, as (to, text).
    pub fn on_item(&mut self, item: &Item, now: Instant) -> Vec<(String, String)> {
        match item {
            Item::Prompt(_) => {
                self.unanswered = self.unanswered.saturating_sub(1);
                self.last_prompt = Some(now);
            }
            Item::Event(Event::TooFast) => {
                if let Some(mut o) = self.last.take() {
                    // A dropped telepath gets no answer, and the drop isn't a try.
                    if let Some(i) = self.acks.iter().rposition(|a| *a == o) {
                        self.acks.remove(i);
                    }
                    if let Out::Telepath { tries, .. } = &mut o {
                        *tries -= 1;
                    }
                    self.queue.push_front(o);
                }
                self.unanswered = self.unanswered.saturating_sub(1);
            }
            Item::Event(Event::TelepathSent { .. }) => {
                self.acks.pop_front();
            }
            // Nobody of that name is on, so trying again won't help.
            Item::Event(Event::UserUnknown) => {
                if let Some(Out::Telepath { who, text, .. }) = self.acks.pop_front() {
                    return vec![(who, text)];
                }
            }
            Item::Event(Event::TelepathNotSent) => {
                if let Some(Out::Telepath { who, text, tries }) = self.acks.pop_front() {
                    if tries >= self.set.telepath_tries {
                        return vec![(who, text)];
                    }
                    let at = now + self.set.telepath_retry;
                    self.retries
                        .push_back((at, Out::Telepath { who, text, tries }));
                }
            }
            _ => {}
        }
        Vec::new()
    }
}
