//! Resting (design §6.6), as pure logic: parsed items and a clock in, lines and telepaths
//! out.
//!
//! A *rest* here is the whole stretch from falling below a start threshold to meeting every
//! stop threshold. It may hold several `rest` commands: a fight breaks the game's rest off,
//! and the bot rests again afterwards, while the leader goes on waiting.
//!
//! Game behaviour it relies on (MudPlay's GAME_MECHANICS "Health, resting & recovery", from
//! Stock 1.11p, which silvermere recreates):
//! - The prompt's `%r` shows whether the player is resting (`(Resting)`) or meditating.
//! - Being attacked, moving, or casting stands the player up.
//! - Poison stops resting: `You are too sick to rest!`. `You feel ill.` comes each slow tick
//!   while poisoned, so its stopping is the only sign the poison has worn off.

use std::time::{Duration, Instant};

use mp_config::{Amount, Pools};
use mp_parse::{Event, Item, Rest, Vitals};

use crate::pacing::Action;

/// When and how to rest.
#[derive(Debug, Clone)]
pub struct RestSettings {
    /// Start when either falls below.
    pub start_when_any: Pools,
    /// Stop when both reach these.
    pub stop_when_all: Pools,
    /// `rest`, or `meditate`.
    pub command: String,
    /// Whom to tell `@wait` at the start and `@ok` at the end, if anyone.
    pub tell: Option<String>,
    /// The least time between two rest commands, and how long after an attack the bot
    /// waits before resting.
    pub retry: Duration,
    /// How long after the last sign of poison the bot waits before resting.
    pub sick_wait: Duration,
}

#[derive(Debug)]
pub struct Rester {
    set: RestSettings,
    /// In a rest, as above.
    resting: bool,
    told: bool,
    engaged: bool,
    last_attacked: Option<Instant>,
    last_try: Option<Instant>,
    sick_until: Option<Instant>,
}

/// Below `amount`, out of `max`.
fn below(amount: Option<Amount>, cur: Option<i32>, max: Option<i32>) -> bool {
    match (amount, cur) {
        (Some(Amount::Points(n)), Some(c)) => c < n,
        (Some(Amount::Percent(p)), Some(c)) => max.is_some_and(|m| m > 0 && c * 100 < p * m),
        _ => false,
    }
}

/// At or above `amount`, or nothing to reach.
fn meets(amount: Option<Amount>, cur: Option<i32>, max: Option<i32>) -> bool {
    match (amount, cur) {
        (Some(Amount::Points(n)), Some(c)) => c >= n,
        (Some(Amount::Percent(p)), Some(c)) => max.is_none_or(|m| m <= 0 || c * 100 >= p * m),
        _ => true,
    }
}

impl Rester {
    pub fn new(set: RestSettings) -> Self {
        Self {
            set,
            resting: false,
            told: false,
            engaged: false,
            last_attacked: None,
            last_try: None,
            sick_until: None,
        }
    }

    /// Whether the bot is in a rest (the leader told to wait, if it is told).
    pub fn resting(&self) -> bool {
        self.resting
    }

    fn can_rest(&self, now: Instant, fighting: bool) -> bool {
        !fighting
            && !self.engaged
            && self
                .last_attacked
                .is_none_or(|at| now.duration_since(at) >= self.set.retry)
            && self.sick_until.is_none_or(|at| now >= at)
    }

    fn try_rest(&mut self, now: Instant) -> Action {
        self.last_try = Some(now);
        Action::Send(self.set.command.clone())
    }

    /// The next item the game sent. `fighting`: the combat engine has a target.
    pub fn on_item(&mut self, item: &Item, now: Instant, fighting: bool) -> Vec<Action> {
        match item {
            Item::Prompt(v) => return self.on_prompt(v, now, fighting),
            Item::Event(Event::CombatEngaged) => self.engaged = true,
            Item::Event(Event::CombatOff) => self.engaged = false,
            Item::Event(Event::AttacksYou { .. }) => self.last_attacked = Some(now),
            Item::Event(Event::TooSick | Event::PoisonTick) => {
                self.sick_until = Some(now + self.set.sick_wait);
            }
            _ => {}
        }
        Vec::new()
    }

    fn on_prompt(&mut self, v: &Vitals, now: Instant, fighting: bool) -> Vec<Action> {
        let (start, stop) = (self.set.start_when_any, self.set.stop_when_all);
        if self.resting {
            if meets(stop.hp, Some(v.hp), v.max_hp) && meets(stop.mp, v.mp, v.max_mp) {
                self.resting = false;
                self.last_try = None;
                return match (self.told, &self.set.tell) {
                    (true, Some(to)) => {
                        self.told = false;
                        vec![Action::Telepath {
                            to: to.clone(),
                            text: "@ok".into(),
                        }]
                    }
                    _ => Vec::new(),
                };
            }
            // Stood up by a fight: rest again once it's over.
            let since = self.last_try.map(|at| now.duration_since(at));
            if v.rest == Rest::Standing
                && self.can_rest(now, fighting)
                && since.is_none_or(|d| d >= self.set.retry)
            {
                return vec![self.try_rest(now)];
            }
            return Vec::new();
        }
        let needs = below(start.hp, Some(v.hp), v.max_hp) || below(start.mp, v.mp, v.max_mp);
        // A rest starts only where one can be taken, so the leader isn't told to wait
        // through a fight.
        if !needs || !self.can_rest(now, fighting) {
            return Vec::new();
        }
        self.resting = true;
        let mut out = Vec::new();
        if let Some(to) = &self.set.tell {
            self.told = true;
            out.push(Action::Telepath {
                to: to.clone(),
                text: "@wait".into(),
            });
        }
        // Already resting by hand: nothing to send but the word to the leader.
        if v.rest == Rest::Standing {
            out.push(self.try_rest(now));
        }
        out
    }
}
