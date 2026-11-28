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
//! - **Refusals**: an attack at a name nobody here has is no attack. A melee line is taken as
//!   speech: `You say "a kobold"` with fast talk on (a new character's default), else
//!   `Your command had no effect.`; a spell is `You do not see kobold here!`. That is how the
//!   bot learns that a monster died unseen (#118): its attack went out after the leader's
//!   kill, so it had no share of the kill and no `*Combat Off*`.
//!
//! - **Spell plans by level** (#125): the first `by_level` block whose levels hold the bot's
//!   level gives the attack spells, top-down; the step-1 `tiers` and `room_tiers` are a block
//!   for every level after them. A bot with level blocks starts no fight until it knows its
//!   level: the session asks `exp` at once (a caster that went in with melee instead would
//!   never stand out of the fight for the sheets, which ask it too). A rule is
//!   passed over when one of its conditions fails, when MP can't pay for it, when it has used
//!   up its casts, or when it had no effect on the target; then melee.
//! - **Counting casts**: a spell repeats every round by itself, so its casts are counted
//!   from the game's lines, never from what the bot sent. A round prints the caster's line,
//!   the spell's message from the game data (`You fire a magic missile at kobold for 4
//!   damage!`), or `You attempt to cast magic missile at kobold, but fail.`, or `..., but the
//!   spell is resisted.` (silvermere's `cast_fail_at`, `cast_resisted`; the spell's name, not
//!   its code). A single-target spell's casts are counted per monster, and a kill gives the
//!   next of that name its own; a room spell's per room, until the room is clear.
//!
//! - **Heals** (#126): a level block's `heals` come before its attacks, one cast a round.
//!   Whom a heal is for comes from the healer's own prompt, and from the HP the server
//!   passes on for each other character it runs (`set_ally`), read off that character's
//!   prompts. Another character is healed only while the room display lists it
//!   (`Also here:`), and not once it has left. A cast breaks off silvermere's auto-combat
//!   (`*Combat Off*`, `commands/cast.rs` `stop_autocombat`), so no attack goes out in the
//!   heal's round, and the attack is taken up again after it. A heal with no target is the
//!   healer's own (silvermere's cast with no target, for a Self or Single spell); a party
//!   spell goes out bare.
//!
//! The room's monsters come from the `Also here:` list. A name counts as a monster only if
//! it is, or ends with, a monster name from the game data (`large kobold` is a `kobold`), so
//! a player is never attacked by mistake.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use mp_config::{
    Amount, HealRule, LevelBlock, Melee, RoomTier, Rule, Span, SpellRule, SpellWhen, Tier, Whom,
};
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
    /// Attack spells planned by level (#125).
    pub by_level: Vec<LevelBlock>,
    /// What the game data says of the spells the plan names, by cast code.
    pub spell_facts: HashMap<String, SpellFacts>,
    /// The routine the bot runs, for the rules that name one.
    pub routine: Option<String>,
}

/// A spell as the game data has it (#125).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpellFacts {
    /// Its name, as the fail lines print it: `magic missile`.
    pub name: String,
    /// It goes at the whole room, cast with its bare code.
    pub room: bool,
    /// It heals the whole party here, cast with its bare code (#126).
    pub party: bool,
    /// The caster's line of a cast, `%s` and `%d` in it.
    pub cast_line: String,
}

/// Another character's HP, as the server last read it off that character's prompt (#126).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllyHp {
    pub hp: i32,
    pub max_hp: Option<i32>,
}

/// A heal rule as the engine runs it: whether it's a party spell.
#[derive(Debug, Clone)]
struct Healing {
    rule: HealRule,
    party: bool,
}

/// A spell rule as the engine runs it: whether it's cast at the room.
#[derive(Debug, Clone)]
struct Planned {
    rule: SpellRule,
    room: bool,
}

impl Planned {
    fn attack(&self) -> Attack {
        if self.room {
            Attack::Room(self.rule.cast.clone())
        } else {
            Attack::Spell(self.rule.cast.clone())
        }
    }
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
    /// The attack sent, its target and when it went, until the game takes it up
    /// (`*Combat Engaged*`) or refuses it (#118, #119).
    asked: Option<(Attack, String, Instant)>,
    /// The bot's level, once `exp` has said it.
    level: Option<u32>,
    /// The maximum MP, from the last prompt that showed it.
    max_mp: Option<i32>,
    /// The level blocks, then the step-1 tiers as a block for every level.
    blocks: Vec<(Span, Vec<Planned>)>,
    tiers: Vec<Planned>,
    /// Casts counted by (cast code, monster in lower case): a room spell's monster is "".
    casts: HashMap<(String, String), u32>,
    /// Each level block's heals, by the block's place in `blocks`.
    heals: Vec<Vec<Healing>>,
    /// The other characters the server runs, by first name in lower case.
    allies: HashMap<String, (String, AllyHp)>,
    /// The players in the room, as the game last listed them.
    players: Vec<String>,
    /// The healer's own HP and maximum, from the last prompt.
    hp: Option<i32>,
    max_hp: Option<i32>,
    /// When the last heal went: one a round, and no attack in its round.
    healed_at: Option<Instant>,
}

impl Combat {
    pub fn new(set: CombatSettings) -> Self {
        let room = |cast: &str| set.spell_facts.get(cast).is_some_and(|f| f.room);
        let heals = set
            .by_level
            .iter()
            .map(|b| {
                b.heals
                    .iter()
                    .map(|h| Healing {
                        rule: h.clone(),
                        party: set.spell_facts.get(&h.cast).is_some_and(|f| f.party),
                    })
                    .collect()
            })
            .collect();
        let blocks = set
            .by_level
            .iter()
            .map(|b| {
                let rules = b
                    .spells
                    .iter()
                    .map(|r| Planned {
                        rule: r.clone(),
                        room: room(&r.cast),
                    })
                    .collect();
                (b.levels, rules)
            })
            .collect();
        // Step 1's lists: room tiers first, cast bare; tiers at the target, whatever the
        // game data says.
        let tiers = set
            .room_tiers
            .iter()
            .map(|t| Planned {
                rule: SpellRule {
                    cast: t.spell.clone(),
                    when: SpellWhen {
                        min_mp: Some(Amount::Points(t.min_mp)),
                        enemies: Some(Span::new(Some(t.min_enemies), None)),
                        ..SpellWhen::default()
                    },
                    max_casts: None,
                },
                room: true,
            })
            .chain(set.tiers.iter().map(|t| Planned {
                rule: SpellRule {
                    cast: t.spell.clone(),
                    when: SpellWhen {
                        min_mp: Some(Amount::Points(t.min_mp)),
                        ..SpellWhen::default()
                    },
                    max_casts: None,
                },
                room: false,
            }))
            .collect();
        Self {
            set,
            roster: Vec::new(),
            target: None,
            current: None,
            engaged: false,
            kill_order: None,
            leader_target: None,
            attackers: Vec::new(),
            useless: HashSet::new(),
            last_change: None,
            mp: 0,
            nuke: true,
            hold: false,
            also_here_seen: false,
            asked: None,
            level: None,
            max_mp: None,
            blocks,
            tiers,
            casts: HashMap::new(),
            heals,
            allies: HashMap::new(),
            players: Vec::new(),
            hp: None,
            max_hp: None,
            healed_at: None,
        }
    }

    /// Another character the server runs, and its HP; `None` when it has gone (#126).
    pub fn set_ally(&mut self, name: &str, hp: Option<AllyHp>) {
        match hp {
            Some(hp) => {
                self.allies.insert(lower(name), (name.to_owned(), hp));
            }
            None => {
                self.allies.remove(&lower(name));
            }
        }
    }

    /// The bot's level, for its level blocks (#125); `None` while it isn't known.
    pub fn set_level(&mut self, level: Option<u32>) {
        self.level = level;
    }

    /// Whether the bot fights by level blocks: it starts no fight until its level is known,
    /// and the session asks `exp` for it.
    pub fn wants_level(&self) -> bool {
        !self.blocks.is_empty()
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
                self.players = names
                    .iter()
                    .filter(|n| !self.is_monster(n) && !n.eq_ignore_ascii_case(&self.set.name))
                    .cloned()
                    .collect();
            }
            // A room display lists who else is here before its exits, and leaves the line
            // out when nobody is: exits with no list since the last prompt mean an empty
            // room (a move, or a look after the last monster died or left).
            Item::Event(Event::ObviousExits) if !self.also_here_seen => {
                self.roster.clear();
                self.players.clear();
            }
            // Gone: off the roster, so a target that ran is let go at the next prompt.
            Item::Event(Event::Left { who }) => {
                if let Some(i) = self.roster.iter().position(|r| r.eq_ignore_ascii_case(who)) {
                    self.roster.remove(i);
                }
                self.players.retain(|p| !p.eq_ignore_ascii_case(who));
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
                } else if let Some(name) = self.refused(line) {
                    self.gone(&name);
                } else {
                    self.count_cast(line);
                }
            }
            Item::Event(Event::CombatEngaged) => {
                self.engaged = true;
                self.asked = None;
            }
            // The break-off of the bot's own attack, answered in a round, is no stop.
            Item::Event(Event::CombatOff)
                if self
                    .asked
                    .as_ref()
                    .is_some_and(|(_, _, at)| now.duration_since(*at) < self.set.round) => {}
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
                if v.max_mp.is_some() {
                    self.max_mp = v.max_mp;
                }
                self.hp = Some(v.hp);
                if v.max_hp.is_some() {
                    self.max_hp = v.max_hp;
                }
                return self.decide(now);
            }
            _ => {}
        }
        Vec::new()
    }

    /// A round of the spell going out, by the game's line (see the module notes).
    fn count_cast(&mut self, line: &str) {
        let (Some(Attack::Spell(code) | Attack::Room(code)), Some(target)) =
            (&self.current, &self.target)
        else {
            return;
        };
        let Some(facts) = self.set.spell_facts.get(code) else {
            return;
        };
        let line = lower(line.trim());
        let name = lower(&facts.name);
        let cast = !facts.cast_line.is_empty()
            && glob(
                &facts.cast_line.replace("%s", "*").replace("%d", "*"),
                &line,
            );
        let tried = line
            .strip_prefix("you attempt to cast ")
            .is_some_and(|rest| {
                !name.is_empty()
                    && (rest.starts_with(&format!("{name} at "))
                        || rest.starts_with(&format!("{name},")))
            });
        if cast || tried {
            let who = if matches!(self.current, Some(Attack::Room(_))) {
                String::new()
            } else {
                lower(target)
            };
            *self.casts.entry((code.clone(), who)).or_default() += 1;
        }
    }

    /// The monster the game says the bot's attack found nobody by (see the module notes).
    fn refused(&self, line: &str) -> Option<String> {
        let (attack, target, _) = self.asked.as_ref()?;
        let line = line.trim();
        let said = format!("You say \"{}\"", attack.command(target));
        let refused = match attack {
            Attack::Melee(_) => {
                line.eq_ignore_ascii_case(&said) || line == "Your command had no effect."
            }
            Attack::Spell(_) => line
                .strip_prefix("You do not see ")
                .and_then(|l| l.strip_suffix(" here!"))
                .is_some_and(|t| t.eq_ignore_ascii_case(target)),
            Attack::Room(_) => false,
        };
        refused.then(|| target.clone())
    }

    /// Nobody here goes by `name` (an attack at a shared name goes to the first of them):
    /// all of that name leave the roster, and the fight with it ends.
    fn gone(&mut self, name: &str) {
        self.roster.retain(|r| !r.eq_ignore_ascii_case(name));
        self.attackers.retain(|a| !a.eq_ignore_ascii_case(name));
        let who = lower(name);
        self.casts.retain(|(_, m), _| *m != who);
        if self
            .leader_target
            .as_ref()
            .is_some_and(|(l, _)| l.eq_ignore_ascii_case(name))
        {
            self.leader_target = None;
        }
        if self
            .target
            .as_deref()
            .is_some_and(|t| t.eq_ignore_ascii_case(name))
        {
            self.target = None;
            self.current = None;
            self.last_change = None;
        }
        self.asked = None;
    }

    /// The target died: it leaves the roster, and nothing more goes at it.
    fn killed(&mut self) {
        let Some(t) = self.target.take() else { return };
        if let Some(i) = self.roster.iter().position(|r| r.eq_ignore_ascii_case(&t)) {
            self.roster.remove(i);
        }
        let who = lower(&t);
        self.casts.retain(|(_, m), _| *m != who);
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
        self.asked = None;
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

    /// MP covers the spell's cost and the rule's `min_mp`; a share of the maximum needs the
    /// maximum known.
    fn affordable(&self, spell: &str, min_mp: Option<Amount>) -> bool {
        let cost = self.set.spell_costs.get(spell).copied().unwrap_or(0);
        let min = match min_mp {
            None => true,
            Some(Amount::Points(n)) => self.mp >= n,
            Some(Amount::Percent(p)) => self
                .max_mp
                .is_some_and(|max| i64::from(self.mp) * 100 >= i64::from(p) * i64::from(max)),
        };
        self.mp >= cost && min
    }

    /// The level block for the bot's level: the first that holds it.
    fn block(&self) -> Option<usize> {
        let l = self.level?;
        self.blocks.iter().position(|(span, _)| span.holds(l))
    }

    /// The spells for the bot's level: the block's, else the tiers.
    fn plan(&self) -> &[Planned] {
        self.block()
            .map_or(&self.tiers[..], |i| &self.blocks[i].1[..])
    }

    /// A rule's `routine` holds: none given, or the routine running is one of them.
    fn under_routine(&self, globs: &[String]) -> bool {
        globs.is_empty()
            || self
                .set
                .routine
                .as_deref()
                .is_some_and(|r| globs.iter().any(|g| glob(g, r)))
    }

    /// The heal to cast now, if any (see the module notes): the first rule of the block
    /// with someone to heal that the healer's MP can pay for.
    fn healing(&self) -> Option<String> {
        let rules = &self.heals[self.block()?];
        let monsters = u32::try_from(self.roster.len()).unwrap_or(u32::MAX);
        // The others here, with their HP.
        let here: Vec<&(String, AllyHp)> = self
            .players
            .iter()
            .filter_map(|p| self.allies.get(&lower(p)))
            .collect();
        for h in rules {
            let w = &h.rule.when;
            if w.enemies.is_some_and(|e| !e.holds(monsters))
                || !self.under_routine(&w.routine)
                || !self.affordable(&h.rule.cast, w.min_mp)
            {
                continue;
            }
            let hurt =
                |hp: i32, max: Option<i32>| crate::rest::below(Some(w.hp_below), Some(hp), max);
            let me_hurt = self.hp.is_some_and(|hp| hurt(hp, self.max_hp));
            if h.party {
                let n = here.iter().filter(|(_, a)| hurt(a.hp, a.max_hp)).count()
                    + usize::from(me_hurt);
                let n = u32::try_from(n).unwrap_or(u32::MAX);
                if w.allies.map_or(n >= 1, |s| n >= 1 && s.holds(n)) {
                    return Some(h.rule.cast.clone());
                }
                continue;
            }
            // Those it's for that are hurt: `None` for the healer, with their share of
            // their maximum in thousandths (points alone count as whole).
            let share = |hp: i32, max: Option<i32>| {
                max.filter(|m| *m > 0)
                    .map_or(1000, |m| i64::from(hp) * 1000 / i64::from(m))
            };
            let mut hurt_ones: Vec<(Option<&str>, i64)> = Vec::new();
            for whom in &h.rule.on {
                match whom {
                    Whom::Me if me_hurt => {
                        hurt_ones.push((None, share(self.hp.unwrap_or(0), self.max_hp)));
                    }
                    Whom::Me => {}
                    Whom::Party => hurt_ones.extend(
                        here.iter()
                            .filter(|(_, a)| hurt(a.hp, a.max_hp))
                            .map(|(n, a)| (Some(n.as_str()), share(a.hp, a.max_hp))),
                    ),
                    Whom::Named(name) => hurt_ones.extend(
                        here.iter()
                            .filter(|(n, a)| n.eq_ignore_ascii_case(name) && hurt(a.hp, a.max_hp))
                            .map(|(n, a)| (Some(n.as_str()), share(a.hp, a.max_hp))),
                    ),
                }
            }
            hurt_ones.sort_by_key(|(n, s)| (*s, n.map(lower)));
            hurt_ones.dedup_by_key(|(n, _)| n.map(lower));
            let n = u32::try_from(hurt_ones.len()).unwrap_or(u32::MAX);
            if n == 0 || w.allies.is_some_and(|s| !s.holds(n)) {
                continue;
            }
            return Some(match hurt_ones[0].0 {
                None => h.rule.cast.clone(),
                Some(name) => format!("{} {name}", h.rule.cast),
            });
        }
        None
    }

    /// The rule's conditions hold, here, at this target.
    fn applies(&self, p: &Planned, target: &str) -> bool {
        let w = &p.rule.when;
        let enemies = w
            .enemies
            .is_none_or(|e| e.holds(u32::try_from(self.roster.len()).unwrap_or(u32::MAX)));
        let monsters = w.monsters.is_empty()
            || if p.room {
                self.roster
                    .iter()
                    .any(|m| w.monsters.iter().any(|g| glob(g, m)))
            } else {
                w.monsters.iter().any(|g| glob(g, target))
            };
        enemies && monsters && self.under_routine(&w.routine)
    }

    /// Casts left under `max_casts`: at this monster, or in this room for a room spell.
    fn casts_left(&self, p: &Planned, target: &str) -> bool {
        let Some(max) = p.rule.max_casts else {
            return true;
        };
        let who = if p.room { String::new() } else { lower(target) };
        self.casts
            .get(&(p.rule.cast.clone(), who))
            .is_none_or(|n| *n < max)
    }

    fn works(&self, a: &Attack, target: &str) -> bool {
        !self.useless.contains(&(a.key(), lower(target)))
    }

    /// The attack for `target` at the current MP: the first rule of the plan for the bot's
    /// level that applies, can be paid for, has casts left and works on it; else melee, else
    /// none.
    fn choose(&self, target: &str) -> Option<Attack> {
        let plan = if self.nuke { self.plan() } else { &[] };
        for p in plan {
            let a = p.attack();
            if self.applies(p, target)
                && self.affordable(&p.rule.cast, p.rule.when.min_mp)
                && self.casts_left(p, target)
                && self.works(&a, target)
            {
                return Some(a);
            }
        }
        match &self.set.melee {
            Melee::Command(c) => Some(Attack::Melee(c.clone())).filter(|a| self.works(a, target)),
            Melee::None => None,
        }
    }

    fn decide(&mut self, now: Instant) -> Vec<String> {
        // A room clear of monsters gives the room spells their casts back.
        if self.roster.is_empty() {
            self.casts.retain(|(_, m), _| !m.is_empty());
        }
        // A heal first, one a round; no attack goes out in its round.
        let heal_round = self
            .healed_at
            .is_some_and(|at| now.duration_since(at) < self.set.round);
        if !heal_round && let Some(cast) = self.healing() {
            self.healed_at = Some(now);
            return vec![cast];
        }
        // A kill order preempts the current target; otherwise a target is kept until it
        // dies or leaves, so the bot doesn't flip between monsters.
        let ordered = self.kill_order.as_deref().and_then(|k| self.here(k));
        let keep = self.target.as_deref().and_then(|t| self.here(t));
        let target = match (ordered, keep) {
            (Some(o), _) => Some(o),
            (None, Some(k)) => Some(k),
            (None, None) if self.hold => None,
            // Which block to fight with isn't known yet (#125).
            (None, None) if self.wants_level() && self.level.is_none() => None,
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
        if heal_round {
            return Vec::new();
        }
        let cmd = want.command(&target);
        self.asked = Some((want.clone(), target, now));
        self.current = Some(want);
        self.last_change = Some(now);
        vec![cmd]
    }
}
