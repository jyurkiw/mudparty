//! The routine (#53, #65): a list of rules read top-down, as pure logic. What the bot looks
//! like and a clock go in; one [`Goal`] at a time comes out.
//!
//! **Rules.** The first rule that applies wins, and the last is what the bot does when
//! nothing else is due:
//! - `rest`: below its `when` line, walk to its room and rest to its `until`;
//! - `hunt`: when due and its `when` holds, walk to the monster's spawn room, kill it if
//!   it's there, and be done; due again `every` after the kill, or after the visit that
//!   found it gone;
//! - `camp`: walk to its room and stay, fighting what comes; always applies;
//! - `gear` (#55): when the routine starts and after each rest, check the inventory, put
//!   on what's carried but not in use, and buy what's missing at a shop that sells it. An
//!   item it can't get (no money, can't use it, no shop, no way there) is skipped, and
//!   said ([`Routine::take_notes`]).
//!
//! **Taking over.** A rule's `when` says when it starts; once started it runs until it's
//! done. A rule higher up that starts to apply takes over at a safe point, never mid-fight
//! (fleeing is #16). A hunt that was interrupted keeps its timer, so it applies again once
//! its `when` holds.
//!
//! The routine decides only *what* to do. The session's engines carry each goal out (the
//! walker walks, the rest engine rests, the combat engine fights), and the session tells
//! the routine how a walk or a rest ended. In 1.1 the party director (#59) hands out the
//! same goals for several characters, so the goals stay few and plain.

use std::collections::BTreeSet;
use std::fmt;
use std::time::{Duration, Instant};

use mp_config::{GearUse, Pools, When};
use mp_parse::Vitals;

use crate::rest::below;
use crate::shop::Carried;
use crate::walk::{RouteRules, WalkState};

/// A routine's rules, with their rooms looked up.
#[derive(Debug, Clone)]
pub struct RoutineSettings {
    /// Top-down: the first that applies wins.
    pub rules: Vec<RuleSettings>,
    /// How long after a failed walk the routine tries again.
    pub retry: Duration,
    /// How long a hunted monster may stand there unfought before the hunt gives up on it.
    pub kill_wait: Duration,
    /// What the routine's walks keep out of (#64).
    pub route: RouteRules,
}

/// One rule, its rooms looked up.
#[derive(Debug, Clone)]
pub enum RuleSettings {
    Rest {
        at: (u32, u32),
        when: When,
        until: Pools,
    },
    Hunt(HuntSettings),
    Camp {
        at: (u32, u32),
        look_every: Duration,
    },
    Gear {
        items: Vec<GearSettings>,
        when: When,
    },
}

/// One item the character carries, and the shops that sell it, best first.
#[derive(Debug, Clone)]
pub struct GearSettings {
    pub item: String,
    pub how: GearUse,
    pub shops: Vec<(u32, u32)>,
}

/// A monster with a known spawn room and respawn time.
#[derive(Debug, Clone)]
pub struct HuntSettings {
    pub monster: String,
    pub at: (u32, u32),
    pub every: Duration,
    pub when: When,
}

/// What the routine wants done now. The session's engines carry it out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Goal {
    /// Walk to this room.
    Walk((u32, u32)),
    /// Rest here until both reach these.
    Rest(Pools),
    /// Stay here, look around this often, and fight what comes.
    Camp(Duration),
    /// Attack this monster (the name as the room shows it) until it's gone.
    Kill(String),
    /// Look at the inventory, and tell the routine what's carried ([`Routine::inventory`]).
    Inventory,
    /// Put this item to use, buying it first if `buy`; tell the routine how it went
    /// ([`Routine::equip_ended`]).
    Equip {
        item: String,
        how: GearUse,
        buy: bool,
    },
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
    /// No rule applies (a routine with no camp at the end).
    Idle,
    CheckingGear,
    ToShop(String),
    Equipping(String),
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
            Self::Idle => f.write_str("nothing to do"),
            Self::CheckingGear => f.write_str("checking gear"),
            Self::ToShop(item) => write!(f, "going to get {item}"),
            Self::Equipping(item) => write!(f, "getting {item}"),
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

/// What the rule under way is doing.
#[derive(Debug, Clone)]
enum Doing {
    /// Walking to its room.
    Walking,
    /// In its room: resting, or camping.
    Here,
    /// In the spawn room, the monster in it.
    Kill {
        target: String,
        since: Instant,
        /// A fight has started since the bot got here.
        started: bool,
    },
    /// Waiting for the inventory.
    Checking,
    /// Walking to `shops[shop]` for `items[item]`.
    ToShop { item: usize, shop: usize },
    /// Buying (if `buy`) and putting on `items[item]`.
    Equip { item: usize, buy: bool },
}

#[derive(Debug, Clone)]
struct Active {
    rule: usize,
    doing: Doing,
}

#[derive(Debug)]
pub struct Routine {
    set: RoutineSettings,
    active: Option<Active>,
    /// When each rule (each hunt) is due next.
    due: Vec<Instant>,
    /// A failed walk, and when to try again.
    stuck: Option<(String, Instant)>,
    paused: bool,
    /// What the bot looked like last.
    last: Seen,
    /// The gear wants checking: at the start, and after each rest.
    gear_due: bool,
    /// Gear it couldn't get, not tried again.
    skipped: BTreeSet<String>,
    /// What the card should be told: items skipped, and why.
    notes: Vec<String>,
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

/// At or above `a`, out of `max`; never before a prompt.
fn at_least(a: mp_config::Amount, cur: Option<i32>, max: Option<i32>) -> bool {
    cur.is_some() && !below(Some(a), cur, max)
}

/// Whether every condition of `w` holds. One about HP or MP can't hold before a prompt.
fn holds(w: &When, s: &Seen) -> bool {
    if *w == When::default() {
        return true;
    }
    let Some(v) = s.vitals else { return false };
    let (hp, mp) = (Some(v.hp), v.mp);
    w.hp_below.is_none_or(|a| below(Some(a), hp, v.max_hp))
        && w.mp_below.is_none_or(|a| below(Some(a), mp, v.max_mp))
        && w.hp_at_least.is_none_or(|a| at_least(a, hp, v.max_hp))
        && w.mp_at_least.is_none_or(|a| at_least(a, mp, v.max_mp))
}

impl Routine {
    /// A routine starting now: every hunt is due at once, and the first rule that applies
    /// before the first prompt (usually the camp) is under way.
    pub fn new(set: RoutineSettings, now: Instant) -> Self {
        let due = vec![now; set.rules.len()];
        let mut r = Self {
            set,
            active: None,
            due,
            stuck: None,
            paused: false,
            last: Seen::default(),
            gear_due: true,
            skipped: BTreeSet::new(),
            notes: Vec::new(),
        };
        r.pick(&Seen::default(), now, None);
        r
    }

    /// What has happened that the card should be told, oldest first; each note once.
    pub fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }

    fn gear_items(&self, rule: usize) -> &[GearSettings] {
        match &self.set.rules[rule] {
            RuleSettings::Gear { items, .. } => items,
            _ => &[],
        }
    }

    fn skip(&mut self, rule: usize, item: usize, why: &str) {
        let name = self.gear_items(rule)[item].item.clone();
        self.notes.push(format!("skipped {name}: {why}"));
        self.skipped.insert(name);
    }

    fn check_gear(&mut self, rule: usize) {
        self.active = Some(Active {
            rule,
            doing: Doing::Checking,
        });
    }

    /// The inventory the routine asked for: what the bot carries. The gear rule puts on
    /// the first item carried but not in use, or goes for the first that's missing, or,
    /// with nothing to do, is done until the next rest.
    pub fn inventory(&mut self, carried: &[Carried], now: Instant) {
        let Some(Active {
            rule,
            doing: Doing::Checking,
        }) = self.active.clone()
        else {
            return;
        };
        for (i, g) in self.gear_items(rule).to_vec().iter().enumerate() {
            if self.skipped.contains(&g.item) {
                continue;
            }
            match carried
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(&g.item))
            {
                Some(c) if c.equipped => {}
                Some(_) => {
                    self.active = Some(Active {
                        rule,
                        doing: Doing::Equip {
                            item: i,
                            buy: false,
                        },
                    });
                    return;
                }
                None if g.shops.is_empty() => self.skip(rule, i, "no shop in places/ sells it"),
                None => {
                    self.active = Some(Active {
                        rule,
                        doing: Doing::ToShop { item: i, shop: 0 },
                    });
                    return;
                }
            }
        }
        self.gear_due = false;
        let last = self.last.clone();
        self.pick(&last, now, None);
    }

    /// Buying and putting on the item the routine asked for ended: look again either way.
    pub fn equip_ended(&mut self, result: Result<(), String>, _now: Instant) {
        let Some(Active {
            rule,
            doing: Doing::Equip { item, .. },
        }) = self.active.clone()
        else {
            return;
        };
        if let Err(why) = result {
            self.skip(rule, item, &why);
        }
        self.check_gear(rule);
    }

    /// What the routine's walks keep out of.
    pub fn route_rules(&self) -> &RouteRules {
        &self.set.route
    }

    /// Pauses (`@stop`) or resumes the routine; it resumes where it was.
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    fn applies(&self, i: usize, s: &Seen, now: Instant) -> bool {
        match &self.set.rules[i] {
            RuleSettings::Rest { when, .. } => holds(when, s),
            RuleSettings::Hunt(h) => now >= self.due[i] && holds(&h.when, s),
            RuleSettings::Camp { .. } => true,
            RuleSettings::Gear { when, .. } => self.gear_due && holds(when, s),
        }
    }

    /// The first rule that applies (but `skip`), from the top, set under way.
    fn pick(&mut self, s: &Seen, now: Instant, skip: Option<usize>) {
        self.active = (0..self.set.rules.len())
            .filter(|&i| Some(i) != skip)
            .find(|&i| self.applies(i, s, now))
            .map(|rule| Active {
                rule,
                doing: match self.set.rules[rule] {
                    RuleSettings::Gear { .. } => Doing::Checking,
                    _ => Doing::Walking,
                },
            });
    }

    /// Done with hunt `i` for now: due again after its time, and on to what applies.
    fn hunted(&mut self, i: usize, s: &Seen, now: Instant) {
        if let RuleSettings::Hunt(h) = &self.set.rules[i] {
            self.due[i] = now + h.every;
        }
        self.pick(s, now, None);
    }

    /// What the routine wants done now.
    pub fn goal(&self) -> Goal {
        if self.paused || self.stuck.is_some() {
            return Goal::Wait;
        }
        let Some(a) = &self.active else {
            return Goal::Wait;
        };
        match (&self.set.rules[a.rule], &a.doing) {
            (RuleSettings::Rest { at, .. } | RuleSettings::Camp { at, .. }, Doing::Walking) => {
                Goal::Walk(*at)
            }
            (RuleSettings::Hunt(_), Doing::Kill { target, .. }) => Goal::Kill(target.clone()),
            (RuleSettings::Hunt(h), _) => Goal::Walk(h.at),
            (RuleSettings::Rest { until, .. }, _) => Goal::Rest(*until),
            (RuleSettings::Camp { look_every, .. }, _) => Goal::Camp(*look_every),
            (RuleSettings::Gear { items, .. }, Doing::ToShop { item, shop }) => {
                Goal::Walk(items[*item].shops[*shop])
            }
            (RuleSettings::Gear { items, .. }, Doing::Equip { item, buy }) => Goal::Equip {
                item: items[*item].item.clone(),
                how: items[*item].how,
                buy: *buy,
            },
            (RuleSettings::Gear { .. }, _) => Goal::Inventory,
        }
    }

    pub fn part(&self) -> RoutinePart {
        if self.paused {
            return RoutinePart::Paused;
        }
        if let Some((why, _)) = &self.stuck {
            return RoutinePart::Stuck(why.clone());
        }
        let Some(a) = &self.active else {
            return RoutinePart::Idle;
        };
        match (&self.set.rules[a.rule], &a.doing) {
            (RuleSettings::Rest { .. }, Doing::Walking) => RoutinePart::ToRest,
            (RuleSettings::Rest { .. }, _) => RoutinePart::Resting,
            (RuleSettings::Camp { .. }, Doing::Walking) => RoutinePart::ToCamp,
            (RuleSettings::Camp { .. }, _) => RoutinePart::Camping,
            (RuleSettings::Hunt(_), Doing::Kill { target, .. }) => {
                RoutinePart::Killing(target.clone())
            }
            (RuleSettings::Hunt(h), _) => RoutinePart::Hunting(h.monster.clone()),
            (RuleSettings::Gear { items, .. }, Doing::ToShop { item, .. }) => {
                RoutinePart::ToShop(items[*item].item.clone())
            }
            (RuleSettings::Gear { items, .. }, Doing::Equip { item, .. }) => {
                RoutinePart::Equipping(items[*item].item.clone())
            }
            (RuleSettings::Gear { .. }, _) => RoutinePart::CheckingGear,
        }
    }

    /// Looks at the bot and moves the routine on.
    pub fn update(&mut self, s: &Seen, now: Instant) {
        self.last = s.clone();
        if self.paused {
            return;
        }
        if let Some((_, until)) = &self.stuck {
            if now < *until {
                return;
            }
            self.stuck = None;
            self.active = None;
        }
        let Some(a) = self.active.clone() else {
            self.pick(s, now, None);
            return;
        };
        if s.fighting
            && let Some(Active {
                doing: Doing::Kill { started, .. },
                ..
            }) = &mut self.active
        {
            *started = true;
        }
        // A rule above the one under way that now applies takes over, out of a fight.
        if !s.fighting
            && let Some(i) = (0..a.rule).find(|&i| self.applies(i, s, now))
        {
            self.active = Some(Active {
                rule: i,
                doing: match self.set.rules[i] {
                    RuleSettings::Gear { .. } => Doing::Checking,
                    _ => Doing::Walking,
                },
            });
            return;
        }
        match (&self.set.rules[a.rule], &a.doing) {
            (RuleSettings::Camp { at, .. }, Doing::Here) => {
                // Moved away (by hand, or dragged): back to the camp.
                if !s.walking && s.room.is_some_and(|r| r != *at) {
                    self.active = Some(Active {
                        rule: a.rule,
                        doing: Doing::Walking,
                    });
                }
            }
            (
                RuleSettings::Hunt(_),
                Doing::Kill {
                    target,
                    since,
                    started,
                },
            ) if !s.fighting => match present(target, &s.monsters) {
                // Dead, or gone.
                None => self.hunted(a.rule, s, now),
                Some(_) if !started && now.duration_since(*since) >= self.set.kill_wait => {
                    self.hunted(a.rule, s, now);
                }
                Some(_) => {}
            },
            _ => {}
        }
    }

    /// The walk the routine asked for ended.
    pub fn walk_ended(&mut self, end: &WalkState, s: &Seen, now: Instant) {
        self.last = s.clone();
        let Some(a) = self.active.clone() else { return };
        match end {
            // A shop it can't reach: the next that sells the item, or skip the item.
            WalkState::Failed(why) if matches!(a.doing, Doing::ToShop { .. }) => {
                let Doing::ToShop { item, shop } = a.doing else {
                    return;
                };
                if shop + 1 < self.gear_items(a.rule)[item].shops.len() {
                    self.active = Some(Active {
                        rule: a.rule,
                        doing: Doing::ToShop {
                            item,
                            shop: shop + 1,
                        },
                    });
                } else {
                    self.skip(a.rule, item, why);
                    self.check_gear(a.rule);
                }
            }
            WalkState::Failed(why) => {
                self.stuck = Some((why.clone(), now + self.set.retry));
                self.active = None;
            }
            WalkState::Arrived => match &self.set.rules[a.rule] {
                RuleSettings::Rest { .. } | RuleSettings::Camp { .. } => {
                    self.active = Some(Active {
                        rule: a.rule,
                        doing: Doing::Here,
                    });
                }
                RuleSettings::Hunt(h) => match present(&h.monster, &s.monsters) {
                    Some(target) => {
                        self.active = Some(Active {
                            rule: a.rule,
                            doing: Doing::Kill {
                                target,
                                since: now,
                                started: s.fighting,
                            },
                        });
                    }
                    None => self.hunted(a.rule, s, now),
                },
                RuleSettings::Gear { .. } => {
                    if let Doing::ToShop { item, .. } = a.doing {
                        self.active = Some(Active {
                            rule: a.rule,
                            doing: Doing::Equip { item, buy: true },
                        });
                    }
                }
            },
            _ => {}
        }
    }

    /// The rest the routine asked for ended: on to whatever else applies (the rest that
    /// just ended aside, since what the bot looked like is a prompt behind).
    pub fn rest_ended(&mut self, now: Instant) {
        if let Some(a) = &self.active
            && matches!(self.set.rules[a.rule], RuleSettings::Rest { .. })
        {
            let done = a.rule;
            let last = self.last.clone();
            // Rested: the gear is looked at again.
            self.gear_due = true;
            self.pick(&last, now, Some(done));
        }
    }
}
