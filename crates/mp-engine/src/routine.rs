//! The routine (#53): a solo bot's loop, as pure logic. What the bot looks like and a clock
//! go in; one [`Goal`] at a time comes out.
//!
//! The loop: walk to the camp and stay there, fighting what comes. When a rest is due
//! (`rest.start_when_any`) and no fight is going on, walk to the rest room, rest to the
//! stop thresholds, and walk back. When a hunt is due and HP allows, walk to the monster's
//! spawn room, kill it if it's there, and walk back; it's due again `every` after the kill,
//! or after the visit that found it gone.
//!
//! The routine decides only *what* to do. The session's engines carry each goal out (the
//! walker walks, the rest engine rests, the combat engine fights), and the session tells
//! the routine how a walk or a rest ended. In 1.1 the party director (#59) hands out the
//! same goals for several characters, so the goals stay few and plain.

use std::fmt;
use std::time::{Duration, Instant};

use mp_config::{Amount, Pools};
use mp_parse::Vitals;

use crate::rest::below;
use crate::walk::{RouteRules, WalkState};

/// A routine's rooms and thresholds, from the character's `routine:` and `rest:`.
#[derive(Debug, Clone)]
pub struct RoutineSettings {
    /// Where the bot stays and fights.
    pub camp: (u32, u32),
    /// Where it rests.
    pub rest_at: (u32, u32),
    /// Leave the camp to rest when either falls below (`rest.start_when_any`).
    pub retreat: Pools,
    /// How often the bot in the camp looks around.
    pub look_every: Duration,
    pub hunts: Vec<HuntSettings>,
    /// How long after a failed walk the routine tries again.
    pub retry: Duration,
    /// How long a hunted monster may stand there unfought before the hunt gives up on it.
    pub kill_wait: Duration,
    /// What the routine's walks keep out of (#64).
    pub route: RouteRules,
}

/// A monster with a known spawn room and respawn time.
#[derive(Debug, Clone)]
pub struct HuntSettings {
    pub monster: String,
    pub at: (u32, u32),
    pub every: Duration,
    pub min_hp: Option<Amount>,
}

/// What the routine wants done now. The session's engines carry it out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Goal {
    /// Walk to this room.
    Walk((u32, u32)),
    /// Rest here, to the rest engine's stop thresholds.
    Rest,
    /// Stay here, look around now and then, and fight what comes.
    Camp,
    /// Attack this monster (the name as the room shows it) until it's gone.
    Kill(String),
    /// Nothing: waiting to try again, or paused.
    Wait,
}

/// Where the routine is, for the web page's card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutinePart {
    ToCamp,
    Camping,
    ToRest,
    Resting,
    Hunting(String),
    Killing(String),
    Stuck(String),
    Paused,
}

impl fmt::Display for RoutinePart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ToCamp => f.write_str("going back to camp"),
            Self::Camping => f.write_str("camping"),
            Self::ToRest => f.write_str("going to rest"),
            Self::Resting => f.write_str("resting"),
            Self::Hunting(m) => write!(f, "hunting {m}"),
            Self::Killing(m) => write!(f, "killing {m}"),
            Self::Stuck(why) => write!(f, "stuck ({why}); trying again soon"),
            Self::Paused => f.write_str("paused"),
        }
    }
}

/// What the bot looks like now, as the session knows it.
#[derive(Debug, Clone, Default)]
pub struct Seen {
    /// The last prompt's values.
    pub vitals: Option<Vitals>,
    /// The combat engine has a target.
    pub fighting: bool,
    /// The monsters the room last showed.
    pub monsters: Vec<String>,
    /// The room the game last said the bot is in.
    pub room: Option<(u32, u32)>,
    /// A walk is under way.
    pub walking: bool,
}

#[derive(Debug, Clone)]
enum Phase {
    ToCamp,
    Camp,
    ToRest,
    Rest,
    /// Walking to hunt `hunts[i]`.
    Hunt(usize),
    /// At the spawn room, the monster in it.
    Kill {
        hunt: usize,
        target: String,
        since: Instant,
        /// A fight has started since the bot got here.
        started: bool,
    },
    Stuck {
        why: String,
        until: Instant,
    },
}

#[derive(Debug)]
pub struct Routine {
    set: RoutineSettings,
    phase: Phase,
    /// When each hunt is due.
    due: Vec<Instant>,
    paused: bool,
}

/// The room's name for `monster`: the same name, or one that ends with it (`big cave
/// bear` for `cave bear`), case-blind.
fn present(monster: &str, here: &[String]) -> Option<String> {
    let m = monster.trim().to_lowercase();
    here.iter()
        .find(|n| {
            let n = n.to_lowercase();
            n == m || n.ends_with(&format!(" {m}"))
        })
        .cloned()
}

impl Routine {
    /// A routine starting now: every hunt is due at once.
    pub fn new(set: RoutineSettings, now: Instant) -> Self {
        let due = vec![now; set.hunts.len()];
        Self {
            set,
            phase: Phase::ToCamp,
            due,
            paused: false,
        }
    }

    pub fn look_every(&self) -> Duration {
        self.set.look_every
    }

    /// What the routine's walks keep out of.
    pub fn route_rules(&self) -> &RouteRules {
        &self.set.route
    }

    /// Pauses (`@stop`) or resumes the routine; it resumes where it was.
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// What the routine wants done now.
    pub fn goal(&self) -> Goal {
        if self.paused {
            return Goal::Wait;
        }
        match &self.phase {
            Phase::ToCamp => Goal::Walk(self.set.camp),
            Phase::Camp => Goal::Camp,
            Phase::ToRest => Goal::Walk(self.set.rest_at),
            Phase::Rest => Goal::Rest,
            Phase::Hunt(i) => Goal::Walk(self.set.hunts[*i].at),
            Phase::Kill { target, .. } => Goal::Kill(target.clone()),
            Phase::Stuck { .. } => Goal::Wait,
        }
    }

    pub fn part(&self) -> RoutinePart {
        if self.paused {
            return RoutinePart::Paused;
        }
        match &self.phase {
            Phase::ToCamp => RoutinePart::ToCamp,
            Phase::Camp => RoutinePart::Camping,
            Phase::ToRest => RoutinePart::ToRest,
            Phase::Rest => RoutinePart::Resting,
            Phase::Hunt(i) => RoutinePart::Hunting(self.set.hunts[*i].monster.clone()),
            Phase::Kill { target, .. } => RoutinePart::Killing(target.clone()),
            Phase::Stuck { why, .. } => RoutinePart::Stuck(why.clone()),
        }
    }

    fn needs_rest(&self, s: &Seen) -> bool {
        let Some(v) = s.vitals else { return false };
        let r = self.set.retreat;
        below(r.hp, Some(v.hp), v.max_hp) || below(r.mp, v.mp, v.max_mp)
    }

    /// The first hunt that is due and that the bot's HP allows.
    fn hunt_due(&self, s: &Seen, now: Instant) -> Option<usize> {
        let v = s.vitals?;
        self.set.hunts.iter().enumerate().find_map(|(i, h)| {
            let hp_ok = h.min_hp.is_none() || !below(h.min_hp, Some(v.hp), v.max_hp);
            (now >= self.due[i] && hp_ok).then_some(i)
        })
    }

    /// Done with `hunt` for now: due again after its time.
    fn hunted(&mut self, hunt: usize, now: Instant) {
        self.due[hunt] = now + self.set.hunts[hunt].every;
        self.phase = Phase::ToCamp;
    }

    /// Looks at the bot and moves the loop on.
    pub fn update(&mut self, s: &Seen, now: Instant) {
        if self.paused {
            return;
        }
        match &mut self.phase {
            Phase::Stuck { until, .. } => {
                if now >= *until {
                    self.phase = if self.needs_rest(s) {
                        Phase::ToRest
                    } else {
                        Phase::ToCamp
                    };
                }
                return;
            }
            Phase::ToRest | Phase::Rest => return,
            Phase::Kill { started, .. } if s.fighting => {
                *started = true;
            }
            _ => {}
        }
        // A rest comes first, once no fight is going on (fleeing mid-fight is #16).
        if !s.fighting && self.needs_rest(s) {
            self.phase = Phase::ToRest;
            return;
        }
        match self.phase.clone() {
            Phase::Camp => {
                if !s.walking && s.room.is_some_and(|r| r != self.set.camp) {
                    self.phase = Phase::ToCamp;
                } else if !s.fighting
                    && let Some(i) = self.hunt_due(s, now)
                {
                    self.phase = Phase::Hunt(i);
                }
            }
            Phase::Kill {
                hunt,
                target,
                since,
                started,
            } if !s.fighting => match present(&target, &s.monsters) {
                // Dead, or gone.
                None => self.hunted(hunt, now),
                Some(_) if !started && now.duration_since(since) >= self.set.kill_wait => {
                    self.hunted(hunt, now);
                }
                Some(_) => {}
            },
            _ => {}
        }
    }

    /// The walk the routine asked for ended.
    pub fn walk_ended(&mut self, end: &WalkState, s: &Seen, now: Instant) {
        match end {
            WalkState::Failed(why) => {
                self.phase = Phase::Stuck {
                    why: why.clone(),
                    until: now + self.set.retry,
                };
            }
            WalkState::Arrived => match self.phase {
                Phase::ToCamp => self.phase = Phase::Camp,
                Phase::ToRest => self.phase = Phase::Rest,
                Phase::Hunt(i) => match present(&self.set.hunts[i].monster, &s.monsters) {
                    Some(target) => {
                        self.phase = Phase::Kill {
                            hunt: i,
                            target,
                            since: now,
                            started: s.fighting,
                        };
                    }
                    None => self.hunted(i, now),
                },
                _ => {}
            },
            _ => {}
        }
    }

    /// The rest the routine asked for ended: back to the camp.
    pub fn rest_ended(&mut self, _now: Instant) {
        if matches!(self.phase, Phase::Rest) {
            self.phase = Phase::ToCamp;
        }
    }
}
