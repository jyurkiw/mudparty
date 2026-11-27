//! The routine (#53, #65): a list of rules read top-down, as pure logic. What the bot looks
//! like and a clock go in; one [`Goal`] at a time comes out.
//!
//! **Rules.** The first rule that applies wins, and the last is what the bot does when
//! nothing else is due:
//! - `rest`: below its `when` line, walk to its room and rest to its `until`;
//! - `circuit` (#67): lairs visited as each fills again, as MudPlay's Auto-Lair. A lair's
//!   clock starts at its last kill (or a visit that found it empty), and it's ready its
//!   respawn later; one not seen yet is ready now. The circuit applies when its `when`
//!   holds and a lair will be ready by the time the bot gets there, waiting up to `wait`.
//!   It takes the nearest such lair (or, with none ready on arrival, the one whose fight
//!   can start soonest; `throughput` takes the nearest whatever the wait), walks to the
//!   lair's wait-room, steps in once it's ready, and kills all it finds. Then it plans
//!   again from where it stands. Travel is reckoned from the route's steps
//!   ([`RoutineSettings::step_time`] each);
//! - `camp`: walk to its room and stay, fighting what comes; always applies;
//! - `gear` (#55): when the routine starts and after each rest, check the inventory, put
//!   on what's carried but not in use, and buy what's missing at a shop that sells it. An
//!   item it can't get (no money, can't use it, no shop, no way there) is skipped, and
//!   said ([`Routine::take_notes`]).
//!
//! - `train` (#56): once the bot has the experience for the next level, and the next is
//!   no higher than `up_to`, walk to a trainer and `train`; then wait there for the new
//!   level's experience (the session asks for `exp`), and train again or go on. No money
//!   (or no experience, the game says) is said and tried again in 5 minutes; a trainer
//!   that won't train the bot gives way to the next, and with none left training stops
//!   until the bot restarts.
//!
//! **Taking over.** A rule's `when` says when it starts; once started it runs until it's
//! done. A rule higher up that starts to apply takes over at a safe point, never mid-fight
//! (fleeing is #16). A lair whose fight was cut short isn't cleared, so it's still ready
//! once the circuit's `when` holds again.
//!
//! The routine decides only *what* to do. The session's engines carry each goal out (the
//! walker walks, the rest engine rests, the combat engine fights), and the session tells
//! the routine how a walk or a rest ended. In 1.1 the party director (#59) hands out the
//! same goals for several characters, so the goals stay few and plain.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use mp_config::{GearUse, Pick, Pools, When};
use mp_parse::Vitals;

use crate::remote::{ExpKnown, RoomBook};
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
    /// The game data's routes, to reckon how long a walk takes (#67); without it every
    /// walk is reckoned as none.
    pub rooms: Option<Arc<dyn RoomBook>>,
    /// How long a step is reckoned to take.
    pub step_time: Duration,
    /// Where lairs' clocks are kept between runs (`mudparty.sqlite`).
    pub lair_clocks: Option<Arc<dyn LairClocks>>,
}

impl RoutineSettings {
    /// Every circuit's lairs.
    pub fn lairs(&self) -> impl Iterator<Item = &LairSettings> {
        self.rules.iter().flat_map(|r| match r {
            RuleSettings::Circuit(c) => c.lairs.as_slice(),
            _ => &[],
        })
    }
}

/// Keeps when each lair was last cleared, so a circuit's clocks outlive a restart or a
/// reconnect (#67). Lairs are the world's, so any bot's kill counts.
pub trait LairClocks: Send + Sync + fmt::Debug {
    fn last_cleared(&self, lair: (u32, u32)) -> Option<SystemTime>;
    fn cleared(&self, lair: (u32, u32), at: SystemTime);
}

/// One rule, its rooms looked up.
#[derive(Debug, Clone)]
pub enum RuleSettings {
    Rest {
        at: (u32, u32),
        when: When,
        until: Pools,
    },
    Circuit(CircuitSettings),
    Camp {
        at: (u32, u32),
        look_every: Duration,
    },
    Gear {
        items: Vec<GearSettings>,
        when: When,
    },
    Train {
        /// Best first.
        trainers: Vec<(u32, u32)>,
        /// The highest level to train to; none for as high as a trainer goes.
        up_to: Option<u32>,
        when: When,
    },
}

/// How long after a training refused for want of money (or experience) it's tried again.
const TRAIN_RETRY: Duration = Duration::from_secs(300);

/// How a `train` the routine asked for ended (#56).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrainEnd {
    /// Up a level, to this one.
    Trained(u32),
    NoExp,
    NoMoney,
    /// This trainer won't train the bot (another class's, a level outside its own), as
    /// the game worded it.
    Refused(String),
    /// No answer, sent twice.
    NoAnswer,
}

/// One item the character carries, and the shops that sell it, best first.
#[derive(Debug, Clone)]
pub struct GearSettings {
    pub item: String,
    pub how: GearUse,
    pub shops: Vec<(u32, u32)>,
}

/// Lairs visited as each fills again (#67).
#[derive(Debug, Clone)]
pub struct CircuitSettings {
    pub lairs: Vec<LairSettings>,
    pub when: When,
    /// How long the bot will wait outside a lair for it to fill.
    pub wait: Duration,
    pub pick: Pick,
}

/// One lair of a circuit.
#[derive(Debug, Clone)]
pub struct LairSettings {
    pub at: (u32, u32),
    /// Its name, for the card.
    pub name: String,
    /// Where the bot waits for it to fill: the nearest room that isn't a lair.
    pub wait_at: (u32, u32),
    /// How long after its last kill it fills again.
    pub respawn: Duration,
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
    /// Train a level here; tell the routine how it went ([`Routine::train_ended`]).
    Train,
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
    /// Walking to a lair, or to the room outside it.
    ToLair(String),
    /// Outside a lair, for it to fill.
    WaitingFor(String),
    Killing(String),
    Stuck(String),
    Paused,
    /// No rule applies (a routine with no camp at the end).
    Idle,
    CheckingGear,
    ToShop(String),
    Equipping(String),
    ToTrainer,
    Training,
}

impl fmt::Display for RoutinePart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ToCamp => f.write_str("going back to camp"),
            Self::Camping => f.write_str("camping"),
            Self::ToRest => f.write_str("going to rest"),
            Self::Resting => f.write_str("resting"),
            Self::ToLair(l) => write!(f, "going to {l}"),
            Self::WaitingFor(l) => write!(f, "waiting for {l} to fill"),
            Self::Killing(m) => write!(f, "killing {m}"),
            Self::Stuck(why) => write!(f, "stuck ({why}); trying again soon"),
            Self::Paused => f.write_str("paused"),
            Self::Idle => f.write_str("nothing to do"),
            Self::CheckingGear => f.write_str("checking gear"),
            Self::ToShop(item) => write!(f, "going to get {item}"),
            Self::Equipping(item) => write!(f, "getting {item}"),
            Self::ToTrainer => f.write_str("going to train"),
            Self::Training => f.write_str("training"),
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
    /// The level, and the experience still needed for the next, as `exp` last showed it
    /// less what's been gained since; none until it's been asked (#56).
    pub exp: Option<ExpKnown>,
}

/// What the rule under way is doing.
#[derive(Debug, Clone)]
enum Doing {
    /// Walking to its room.
    Walking,
    /// In its room: resting, or camping.
    Here,
    /// Walking to lair `lair`'s wait-room.
    ToWait { lair: usize },
    /// In the wait-room, for lair `lair` to fill.
    Waiting { lair: usize },
    /// Walking into lair `lair`.
    ToLair { lair: usize },
    /// In lair `lair`, killing what's there, `target` first.
    Kill {
        lair: usize,
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
    /// Walking to `trainers[t]`.
    ToTrainer { t: usize },
    /// At `trainers[t]`, training.
    Training { t: usize },
    /// Trained: waiting where it is for the new level's experience.
    AfterTrain,
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
    /// When each lair was last cleared.
    clocks: BTreeMap<(u32, u32), Instant>,
    /// Lairs cleared since the session last asked ([`Routine::take_cleared`]).
    cleared: Vec<((u32, u32), Instant)>,
    /// Reckoned walks, `(from, to)`: how long, or none for no way.
    travel: RefCell<BTreeMap<Leg, Option<Duration>>>,
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
    /// Trainers that wouldn't train the bot, not tried again.
    refused_trainers: BTreeSet<(u32, u32)>,
    /// No training before this: refused for want of money or experience.
    train_after: Option<Instant>,
}

/// A walk, `(from, to)`.
type Leg = ((u32, u32), (u32, u32));

/// Whether the room shows `monster`, case-blind.
fn present(monster: &str, here: &[String]) -> bool {
    here.iter().any(|n| n.eq_ignore_ascii_case(monster))
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
    /// A routine starting now: every lair not seen yet is ready, and the first rule that
    /// applies before the first prompt (usually the camp) is under way.
    pub fn new(set: RoutineSettings, now: Instant) -> Self {
        Self::with_clocks(set, BTreeMap::new(), now)
    }

    /// A routine starting now, with when its lairs were last cleared, as kept.
    pub fn with_clocks(
        set: RoutineSettings,
        clocks: BTreeMap<(u32, u32), Instant>,
        now: Instant,
    ) -> Self {
        let mut r = Self {
            set,
            active: None,
            clocks,
            cleared: Vec::new(),
            travel: RefCell::new(BTreeMap::new()),
            stuck: None,
            paused: false,
            last: Seen::default(),
            gear_due: true,
            skipped: BTreeSet::new(),
            notes: Vec::new(),
            refused_trainers: BTreeSet::new(),
            train_after: None,
        };
        r.pick(&Seen::default(), now, None);
        r
    }

    /// The lairs cleared since last asked, and when, oldest first: for keeping.
    pub fn take_cleared(&mut self) -> Vec<((u32, u32), Instant)> {
        std::mem::take(&mut self.cleared)
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

    pub fn settings(&self) -> &RoutineSettings {
        &self.set
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
            RuleSettings::Circuit(c) => holds(&c.when, s) && self.next_lair(c, s, now).is_some(),
            RuleSettings::Camp { .. } => true,
            RuleSettings::Gear { when, .. } => self.gear_due && holds(when, s),
            RuleSettings::Train {
                trainers,
                up_to,
                when,
            } => {
                holds(when, s)
                    && s.exp
                        .is_some_and(|e| e.left <= 0 && up_to.is_none_or(|u| e.level < u))
                    && self.train_after.is_none_or(|t| now >= t)
                    && self.next_trainer(trainers, 0).is_some()
            }
        }
    }

    /// How long a walk from `from` to `to` is reckoned to take; none when there's no way.
    /// Unknown where the bot is, or without the game data, it's reckoned as none.
    fn travel(&self, from: Option<(u32, u32)>, to: (u32, u32)) -> Option<Duration> {
        let (Some(from), Some(book)) = (from, &self.set.rooms) else {
            return Some(Duration::ZERO);
        };
        if from == to {
            return Some(Duration::ZERO);
        }
        *self
            .travel
            .borrow_mut()
            .entry((from, to))
            .or_insert_with(|| {
                let steps = book.route(from, to, &self.set.route).ok()?;
                Some(self.set.step_time * u32::try_from(steps.len()).unwrap_or(u32::MAX))
            })
    }

    /// When lair `l` fills again; none if it's ready now.
    fn ready_at(&self, l: &LairSettings, now: Instant) -> Option<Instant> {
        self.clocks
            .get(&l.at)
            .map(|c| *c + l.respawn)
            .filter(|r| *r > now)
    }

    /// The lair the circuit goes for next from where the bot is, if any will be ready by
    /// the time it gets there, waiting up to the circuit's `wait`.
    fn next_lair(&self, c: &CircuitSettings, s: &Seen, now: Instant) -> Option<usize> {
        let mut best: Option<((bool, Duration), usize)> = None;
        for (i, l) in c.lairs.iter().enumerate() {
            let Some(travel) = self.travel(s.room, l.wait_at) else {
                continue;
            };
            let arrive = now + travel;
            let ready = self.ready_at(l, now).unwrap_or(now);
            if ready > arrive + c.wait {
                continue;
            }
            // Slack: arriving after it's ready wastes respawn time; before, the bot waits.
            let on_time = ready <= arrive;
            let key = match c.pick {
                Pick::Soonest if on_time => (false, travel),
                // None on time: the one whose fight can start soonest.
                Pick::Soonest => (true, ready.max(arrive) - now),
                Pick::Throughput => (false, travel),
            };
            if best.is_none_or(|(k, _)| key < k) {
                best = Some((key, i));
            }
        }
        best.map(|(_, i)| i)
    }

    /// How rule `rule` starts: a circuit with the lair it goes for, straight in if it's
    /// ready, else to the room outside.
    fn begin(&self, rule: usize, s: &Seen, now: Instant) -> Doing {
        match &self.set.rules[rule] {
            RuleSettings::Gear { .. } => Doing::Checking,
            RuleSettings::Train { trainers, .. } => match self.next_trainer(trainers, 0) {
                Some(t) if s.room == Some(trainers[t]) => Doing::Training { t },
                Some(t) => Doing::ToTrainer { t },
                None => Doing::Walking,
            },
            RuleSettings::Circuit(c) => match self.next_lair(c, s, now) {
                Some(lair) if self.ready_at(&c.lairs[lair], now).is_none() => {
                    Doing::ToLair { lair }
                }
                Some(lair) => Doing::ToWait { lair },
                None => Doing::Walking,
            },
            _ => Doing::Walking,
        }
    }

    /// The first rule that applies (but `skip`), from the top, set under way.
    fn pick(&mut self, s: &Seen, now: Instant, skip: Option<usize>) {
        self.active = (0..self.set.rules.len())
            .filter(|&i| Some(i) != skip)
            .find(|&i| self.applies(i, s, now))
            .map(|rule| Active {
                rule,
                doing: self.begin(rule, s, now),
            });
    }

    /// Lair `lair` of circuit `rule` is clear (or given up on): its clock starts now, and
    /// the routine plans again from where the bot stands.
    /// Done with a lair: cleared, or `given_up` on a monster there that never fought. Its
    /// clock starts now either way, and the card is told.
    fn lair_done(
        &mut self,
        rule: usize,
        lair: usize,
        given_up: Option<&str>,
        s: &Seen,
        now: Instant,
    ) {
        if let RuleSettings::Circuit(c) = &self.set.rules[rule] {
            let l = &c.lairs[lair];
            self.clocks.insert(l.at, now);
            self.cleared.push((l.at, now));
            self.notes.push(match given_up {
                Some(m) => format!("gave up on {m} in {}", l.name),
                None => format!("cleared {}", l.name),
            });
        }
        self.pick(s, now, None);
    }

    fn lair(&self, rule: usize, lair: usize) -> &LairSettings {
        match &self.set.rules[rule] {
            RuleSettings::Circuit(c) => &c.lairs[lair],
            _ => unreachable!("a lair of a rule that isn't a circuit"),
        }
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
            (RuleSettings::Circuit(_), Doing::Kill { target, .. }) => Goal::Kill(target.clone()),
            (RuleSettings::Circuit(c), Doing::ToWait { lair }) => {
                Goal::Walk(c.lairs[*lair].wait_at)
            }
            (RuleSettings::Circuit(c), Doing::ToLair { lair }) => Goal::Walk(c.lairs[*lair].at),
            (RuleSettings::Circuit(_), _) => Goal::Wait,
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
            (RuleSettings::Train { trainers, .. }, Doing::ToTrainer { t }) => {
                Goal::Walk(trainers[*t])
            }
            (RuleSettings::Train { .. }, Doing::Training { .. }) => Goal::Train,
            (RuleSettings::Train { .. }, _) => Goal::Wait,
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
            (RuleSettings::Circuit(_), Doing::Kill { target, .. }) => {
                RoutinePart::Killing(target.clone())
            }
            (RuleSettings::Circuit(c), Doing::ToWait { lair } | Doing::ToLair { lair }) => {
                RoutinePart::ToLair(c.lairs[*lair].name.clone())
            }
            (RuleSettings::Circuit(c), Doing::Waiting { lair }) => {
                RoutinePart::WaitingFor(c.lairs[*lair].name.clone())
            }
            (RuleSettings::Circuit(_), _) => RoutinePart::Idle,
            (RuleSettings::Gear { items, .. }, Doing::ToShop { item, .. }) => {
                RoutinePart::ToShop(items[*item].item.clone())
            }
            (RuleSettings::Gear { items, .. }, Doing::Equip { item, .. }) => {
                RoutinePart::Equipping(items[*item].item.clone())
            }
            (RuleSettings::Gear { .. }, _) => RoutinePart::CheckingGear,
            (RuleSettings::Train { .. }, Doing::ToTrainer { .. }) => RoutinePart::ToTrainer,
            (RuleSettings::Train { .. }, _) => RoutinePart::Training,
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
                doing: self.begin(i, s, now),
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
            // Trained: on once the new level's experience is known.
            (RuleSettings::Train { .. }, Doing::AfterTrain) if s.exp.is_some() => {
                self.pick(s, now, None);
            }
            (RuleSettings::Circuit(c), Doing::Waiting { lair }) => {
                if self.ready_at(&c.lairs[*lair], now).is_none() {
                    self.active = Some(Active {
                        rule: a.rule,
                        doing: Doing::ToLair { lair: *lair },
                    });
                }
            }
            (
                RuleSettings::Circuit(_),
                Doing::Kill {
                    lair,
                    target,
                    since,
                    started,
                },
            ) if !s.fighting => {
                if present(target, &s.monsters) {
                    // Standing there unfought: given up on.
                    if !started && now.duration_since(*since) >= self.set.kill_wait {
                        let target = target.clone();
                        self.lair_done(a.rule, *lair, Some(&target), s, now);
                    }
                } else if let Some(next) = s.monsters.first() {
                    // Dead, or gone; the next.
                    self.active = Some(Active {
                        rule: a.rule,
                        doing: Doing::Kill {
                            lair: *lair,
                            target: next.clone(),
                            since: now,
                            started: false,
                        },
                    });
                } else {
                    self.lair_done(a.rule, *lair, None, s, now);
                }
            }
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
                RuleSettings::Circuit(_) => match a.doing {
                    Doing::ToWait { lair } => {
                        let ready = self.ready_at(self.lair(a.rule, lair), now).is_none();
                        self.active = Some(Active {
                            rule: a.rule,
                            doing: if ready {
                                Doing::ToLair { lair }
                            } else {
                                Doing::Waiting { lair }
                            },
                        });
                    }
                    // In the lair: it filled as the bot walked in, or it's empty.
                    Doing::ToLair { lair } => match s.monsters.first() {
                        Some(target) => {
                            self.active = Some(Active {
                                rule: a.rule,
                                doing: Doing::Kill {
                                    lair,
                                    target: target.clone(),
                                    since: now,
                                    started: s.fighting,
                                },
                            });
                        }
                        None => self.lair_done(a.rule, lair, None, s, now),
                    },
                    _ => {}
                },
                RuleSettings::Gear { .. } => {
                    if let Doing::ToShop { item, .. } = a.doing {
                        self.active = Some(Active {
                            rule: a.rule,
                            doing: Doing::Equip { item, buy: true },
                        });
                    }
                }
                RuleSettings::Train { .. } => {
                    if let Doing::ToTrainer { t } = a.doing {
                        self.active = Some(Active {
                            rule: a.rule,
                            doing: Doing::Training { t },
                        });
                    }
                }
            },
            _ => {}
        }
    }

    /// Whether the routine wants to know the bot's experience: it trains (#56). The
    /// session asks with `exp` while it isn't known.
    pub fn wants_exp(&self) -> bool {
        self.set
            .rules
            .iter()
            .any(|r| matches!(r, RuleSettings::Train { .. }))
    }

    /// The first trainer from `from` on that hasn't refused the bot.
    fn next_trainer(&self, trainers: &[(u32, u32)], from: usize) -> Option<usize> {
        (from..trainers.len()).find(|&t| !self.refused_trainers.contains(&trainers[t]))
    }

    /// The `train` the routine asked for ended.
    pub fn train_ended(&mut self, end: TrainEnd, now: Instant) {
        let Some(Active {
            rule,
            doing: Doing::Training { t },
        }) = self.active.clone()
        else {
            return;
        };
        let RuleSettings::Train { trainers, .. } = &self.set.rules[rule] else {
            return;
        };
        let trainers = trainers.clone();
        let last = self.last.clone();
        match end {
            TrainEnd::Trained(level) => {
                self.notes.push(format!("trained to level {level}"));
                self.active = Some(Active {
                    rule,
                    doing: Doing::AfterTrain,
                });
            }
            TrainEnd::Refused(why) => {
                self.refused_trainers.insert(trainers[t]);
                match self.next_trainer(&trainers, t + 1) {
                    Some(next) => {
                        self.active = Some(Active {
                            rule,
                            doing: Doing::ToTrainer { t: next },
                        });
                    }
                    None => {
                        self.notes.push(format!(
                            "no trainer will train it ({why}); training stopped"
                        ));
                        self.pick(&last, now, Some(rule));
                    }
                }
            }
            other => {
                let why = match other {
                    TrainEnd::NoMoney => "no money for the training fee",
                    TrainEnd::NoExp => "the game says there isn't the experience yet",
                    _ => "the trainer gave no answer",
                };
                self.notes
                    .push(format!("couldn't train: {why}; trying again in 5m"));
                self.train_after = Some(now + TRAIN_RETRY);
                self.pick(&last, now, Some(rule));
            }
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
