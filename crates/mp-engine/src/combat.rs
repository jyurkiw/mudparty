//! The combat engine (design §6.4), as pure logic: parsed items and a clock in, commands
//! out.
//!
//! Game behaviour it relies on (MudPlay's GAME_MECHANICS, from Stock 1.11p, which
//! silvermere recreates):
//! - **Attacks**: melee is `<command> <target>` (`a practice dummy`), a single-target spell
//!   is its cast code and the target (`mmis practice dummy`), a room spell its bare code.
//!   A spell repeats every round by itself, so a command is sent only when the choice
//!   changes, and at most one change a round: two different attack spells never go out in
//!   one round.
//! - **Kills**: while engaged, `You gain N experience.` means the target just died. It
//!   comes before `*Combat Off*`, so the target is dropped on the exp line and nothing is
//!   cast at the corpse. Death lines are per-monster flavour and are not read.
//! - **`*Combat Off*` alone is no death**: a `break` or an interrupted round prints it too,
//!   so the fight is taken up again while the target is still here.
//! - **An attack's own break-off is no stop**: an attack sent while already fighting is
//!   answered `*Combat Off*` (the old fight broken off, with its own prompt), then
//!   `*Combat Engaged*`. An off that comes while the bot's attack is unanswered is that,
//!   and the attack is still running. Read as a stop, the attack went out again at the
//!   off's prompt, and every answer asked for the next: two bots, each line of one
//!   redrawing the other's prompt, kept it up ten times a second (#119). A kill can set it
//!   off: the party's experience line doesn't say which of two same-named monsters died.
//! - **Shared names**: an attack on a shared name goes to the first monster of that name.
//!
//! The room's monsters come from the `Also here:` list. A name counts as a monster only if
//! it is, or ends with, a monster name from the game data (`large kobold` is a `kobold`), so
//! a player is never attacked by mistake.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use mp_config::{Melee, RoomTier, Rule, Tier};
use mp_parse::{Event, Item};

/// What the engine needs to fight.
#[derive(Debug, Clone)]
pub struct CombatSettings {
    /// The bot's own first name.
    pub name: String,
    /// The party leader whose target `assist_leader` takes.
    pub leader: Option<String>,
    pub targeting: Vec<Rule>,
    pub tiers: Vec<Tier>,
    pub room_tiers: Vec<RoomTier>,
    pub melee: Melee,
    /// Mana cost by cast code, from the game data.
    pub spell_costs: HashMap<String, i32>,
    /// Monster names, from the game data.
    pub monsters: Vec<String>,
    /// How old the leader's target may be and still be assisted (design §5.3).
    pub leader_target_stale: Duration,
    /// A combat round: 5 s on a stock game (MudPlay GAME_MECHANICS "Combat round (5s)").
    /// At most one change of attack is sent in a round.
    pub round: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Attack {
    Spell(String),
    Room(String),
    Melee(String),
}

impl Attack {
    fn command(&self, target: &str) -> String {
        match self {
            Self::Spell(s) | Self::Melee(s) => format!("{s} {target}"),
            Self::Room(s) => s.clone(),
        }
    }

    /// The key a "no effect" is remembered under.
    fn key(&self) -> String {
        match self {
            Self::Spell(s) | Self::Room(s) => format!("spell:{s}"),
            Self::Melee(_) => "melee".into(),
        }
    }
}

fn lower(s: &str) -> String {
    s.to_lowercase()
}

/// `*` matches any run of characters; everything else matches itself, case-blind.
fn glob(pattern: &str, name: &str) -> bool {
    fn go(p: &[char], n: &[char]) -> bool {
        match p.split_first() {
            None => n.is_empty(),
            Some(('*', rest)) => (0..=n.len()).any(|i| go(rest, &n[i..])),
            Some((c, rest)) => n.first() == Some(c) && go(rest, &n[1..]),
        }
    }
    let p: Vec<char> = lower(pattern).chars().collect();
    let n: Vec<char> = lower(name).chars().collect();
    go(&p, &n)
}

/// One bot's fighting.
#[derive(Debug)]
pub struct Combat {
    set: CombatSettings,
    /// The monsters in the room, in the order the game lists them.
    roster: Vec<String>,
    target: Option<String>,
    /// The attack last sent, while it is running.
    current: Option<Attack>,
    engaged: bool,
    /// When the last attack went out, until the game answers it with `*Combat Engaged*`
    /// (#119).
    unanswered: Option<Instant>,
    kill_order: Option<String>,
    leader_target: Option<(String, Instant)>,
    attackers: Vec<String>,
    /// (attack key, monster, lower case) pairs that had no effect.
    useless: HashSet<(String, String)>,
    last_change: Option<Instant>,
    mp: i32,
    /// `@auto-nuke`: attack spells may be cast.
    nuke: bool,
    /// A `@wait` hold: no new fight is picked, though one going on goes on.
    hold: bool,
    /// An `Also here:` line came since the last prompt: the room display being read has
    /// its list.
    also_here_seen: bool,
}

impl Combat {
    pub fn new(set: CombatSettings) -> Self {
        Self {
            set,
            roster: Vec::new(),
            target: None,
            current: None,
            engaged: false,
            unanswered: None,
            kill_order: None,
            leader_target: None,
            attackers: Vec::new(),
            useless: HashSet::new(),
            last_change: None,
            mp: 0,
            nuke: true,
            hold: false,
            also_here_seen: false,
        }
    }

    /// The monsters in the room, as the game last listed them.
    pub fn monsters(&self) -> &[String] {
        &self.roster
    }

    /// `@auto-nuke on|off`: with it off, the bot melees whatever its MP.
    pub fn set_nuke(&mut self, on: bool) {
        self.nuke = on;
    }

    /// Holds (or frees) the picking of new fights, for a `@wait`.
    pub fn set_hold(&mut self, hold: bool) {
        self.hold = hold;
    }

    /// The monster being fought.
    pub fn target(&self) -> Option<&str> {
        self.target.as_deref()
    }

    /// The attack going at the target: a spell's cast code, or `melee`.
    pub fn tier(&self) -> Option<String> {
        self.target.as_ref()?;
        Some(match self.current.as_ref()? {
            Attack::Spell(s) | Attack::Room(s) => s.clone(),
            Attack::Melee(_) => "melee".into(),
        })
    }

    /// `@kill <name>`: this monster first, until it dies.
    pub fn order_kill(&mut self, name: &str) {
        self.kill_order = Some(name.to_owned());
    }

    /// Decides now, from the last prompt's MP, as after a kill order.
    pub fn act(&mut self, now: Instant) -> Vec<String> {
        self.decide(now)
    }

    /// `name` attacked the bot (for `fight_back`). Only a monster counts.
    pub fn attacked_by(&mut self, name: &str, _now: Instant) {
        if self.is_monster(name) && !self.attackers.iter().any(|a| a.eq_ignore_ascii_case(name)) {
            self.attackers.push(name.to_owned());
        }
    }

    fn is_monster(&self, name: &str) -> bool {
        let n = lower(name);
        self.set.monsters.iter().any(|m| {
            let m = lower(m);
            n == m || n.ends_with(&format!(" {m}"))
        })
    }

    /// The monster a line says came in: `A angry giant rat creeps into the room from
    /// nowhere.`, `kobold moves into the room from above.`. Arrivals print each monster's own
    /// words (silvermere's `monster_arrival_text`, from the game data's `arrivemsg`), so the
    /// name is read off the front: an `A`, `An` or `The`, then the longest run of words that
    /// is, or ends with, a monster's name. Whatever follows `into the room` (nowhere, a
    /// direction, above, below) doesn't matter.
    fn arrival(&self, line: &str) -> Option<String> {
        let line = line.trim();
        let before = &line[..line.find(" into the room")?];
        let mut words: Vec<&str> = before.split_whitespace().collect();
        if words
            .first()
            .is_some_and(|w| ["a", "an", "the"].contains(&lower(w).as_str()))
        {
            words.remove(0);
        }
        (1..=words.len())
            .rev()
            .map(|k| words[..k].join(" "))
            .find(|name| self.is_monster(name))
    }

    fn here(&self, name: &str) -> Option<String> {
        self.roster
            .iter()
            .find(|r| r.eq_ignore_ascii_case(name))
            .cloned()
    }

    /// The next item the game sent; returns the commands to send.
    pub fn on_item(&mut self, item: &Item, now: Instant) -> Vec<String> {
        match item {
            Item::Event(Event::AlsoHere { names }) => {
                self.also_here_seen = true;
                self.roster = names
                    .iter()
                    .filter(|n| self.is_monster(n))
                    .cloned()
                    .collect();
            }
            // A room display lists who else is here before its exits, and leaves the line
            // out when nobody is: exits with no list since the last prompt mean an empty
            // room (a move, or a look after the last monster died or left).
            Item::Event(Event::ObviousExits) if !self.also_here_seen => self.roster.clear(),
            // Gone: off the roster, so a target that ran is let go at the next prompt.
            Item::Event(Event::Left { who }) => {
                if let Some(i) = self.roster.iter().position(|r| r.eq_ignore_ascii_case(who)) {
                    self.roster.remove(i);
                }
            }
            Item::Event(Event::MovesToAttack { who, target })
            | Item::Event(Event::MovesToCast { who, target, .. })
                if self
                    .set
                    .leader
                    .as_deref()
                    .is_some_and(|l| l.eq_ignore_ascii_case(who)) =>
            {
                // The leader's attack says the monster is here, though the bot may never
                // have seen it arrive (#105).
                if self.is_monster(target) && self.here(target).is_none() {
                    self.roster.push(target.clone());
                }
                self.leader_target = Some((target.clone(), now));
            }
            // A monster walking in, in its own words (#105).
            Item::Text(line) => {
                if let Some(name) = self.arrival(line) {
                    self.roster.push(name);
                }
            }
            Item::Event(Event::CombatEngaged) => {
                self.engaged = true;
                self.unanswered = None;
            }
            // The break-off of the bot's own attack, answered in a round, is no stop.
            Item::Event(Event::CombatOff)
                if self
                    .unanswered
                    .is_some_and(|at| now.duration_since(at) < self.set.round) => {}
            Item::Event(Event::CombatOff) => {
                self.engaged = false;
                self.current = None;
            }
            // A kill says what it gained, or that it gained nothing past the cap.
            Item::Event(Event::ExpGain { .. } | Event::ExpCapped) => self.killed(),
            Item::Event(Event::SpellNoEffect { target }) => {
                if let Some(a @ (Attack::Spell(_) | Attack::Room(_))) = self.current.take() {
                    self.useless.insert((a.key(), lower(target)));
                    // An immune target spends no round, so the next pick may go at once.
                    self.last_change = None;
                }
            }
            Item::Event(Event::WeaponNoEffect) => {
                if let (Some(Attack::Melee(_)), Some(t)) = (&self.current, &self.target) {
                    self.useless.insert(("melee".into(), lower(t)));
                    self.current = None;
                    self.last_change = None;
                }
            }
            Item::Prompt(v) => {
                self.also_here_seen = false;
                self.mp = v.mp.unwrap_or(0);
                return self.decide(now);
            }
            _ => {}
        }
        Vec::new()
    }

    /// The target died: it leaves the roster, and nothing more goes at it.
    fn killed(&mut self) {
        let Some(t) = self.target.take() else { return };
        if let Some(i) = self.roster.iter().position(|r| r.eq_ignore_ascii_case(&t)) {
            self.roster.remove(i);
        }
        if self
            .kill_order
            .as_deref()
            .is_some_and(|k| k.eq_ignore_ascii_case(&t))
        {
            self.kill_order = None;
        }
        if self
            .leader_target
            .as_ref()
            .is_some_and(|(l, _)| l.eq_ignore_ascii_case(&t))
            && self.here(&t).is_none()
        {
            self.leader_target = None;
        }
        self.attackers.retain(|a| !a.eq_ignore_ascii_case(&t));
        self.current = None;
        self.engaged = false;
        self.last_change = None;
    }

    /// The target by the rules, top-down: the first rule that names a monster here wins.
    fn pick(&self, now: Instant) -> Option<String> {
        for rule in &self.set.targeting {
            let found = match rule {
                Rule::KillOrder(true) => self.kill_order.as_deref().and_then(|k| self.here(k)),
                Rule::Priority(globs) => globs
                    .iter()
                    .find_map(|g| self.roster.iter().find(|m| glob(g, m)).cloned()),
                Rule::AssistLeader(true) => self
                    .leader_target
                    .as_ref()
                    .filter(|(_, at)| now.duration_since(*at) <= self.set.leader_target_stale)
                    .and_then(|(t, _)| self.here(t)),
                Rule::FightBack(true) => self.attackers.iter().find_map(|a| self.here(a)),
                _ => None,
            };
            if found.is_some() {
                return found;
            }
        }
        None
    }

    fn affordable(&self, spell: &str, min_mp: i32) -> bool {
        let cost = self.set.spell_costs.get(spell).copied().unwrap_or(0);
        self.mp >= min_mp.max(cost)
    }

    fn works(&self, a: &Attack, target: &str) -> bool {
        !self.useless.contains(&(a.key(), lower(target)))
    }

    /// The attack for `target` at the current MP: a room tier with enough enemies here, else
    /// the first single-target tier that fits, else melee, else none.
    fn choose(&self, target: &str) -> Option<Attack> {
        let room_tiers = if self.nuke {
            &self.set.room_tiers[..]
        } else {
            &[]
        };
        let tiers = if self.nuke { &self.set.tiers[..] } else { &[] };
        for t in room_tiers {
            let a = Attack::Room(t.spell.clone());
            if self.roster.len() >= t.min_enemies as usize
                && self.affordable(&t.spell, t.min_mp)
                && self.works(&a, target)
            {
                return Some(a);
            }
        }
        for t in tiers {
            let a = Attack::Spell(t.spell.clone());
            if self.affordable(&t.spell, t.min_mp) && self.works(&a, target) {
                return Some(a);
            }
        }
        match &self.set.melee {
            Melee::Command(c) => Some(Attack::Melee(c.clone())).filter(|a| self.works(a, target)),
            Melee::None => None,
        }
    }

    fn decide(&mut self, now: Instant) -> Vec<String> {
        // A kill order preempts the current target; otherwise a target is kept until it
        // dies or leaves, so the bot doesn't flip between monsters.
        let ordered = self.kill_order.as_deref().and_then(|k| self.here(k));
        let keep = self.target.as_deref().and_then(|t| self.here(t));
        let target = match (ordered, keep) {
            (Some(o), _) => Some(o),
            (None, Some(k)) => Some(k),
            (None, None) if self.hold => None,
            (None, None) => self.pick(now),
        };
        if target != self.target {
            self.current = None;
        }
        self.target = target;
        let Some(target) = self.target.clone() else {
            return Vec::new();
        };
        let Some(want) = self.choose(&target) else {
            return Vec::new();
        };
        if self.current.as_ref() == Some(&want) {
            return Vec::new();
        }
        if self.current.is_some()
            && self
                .last_change
                .is_some_and(|at| now.duration_since(at) < self.set.round)
        {
            return Vec::new();
        }
        let cmd = want.command(&target);
        self.current = Some(want);
        self.last_change = Some(now);
        self.unanswered = Some(now);
        vec![cmd]
    }
}
