//! Attack spells planned by level (#125), as pure logic fed with silvermere's own rendered
//! text: the block for the bot's level, each rule's conditions, cast limits counted from the
//! game's cast lines (per monster for a single-target spell, per room for a room spell), and
//! the fall to the next rule and to melee.

use std::time::{Duration, Instant};

use mp_config::{Amount, LevelBlock, Melee, Rule, Span, SpellRule, SpellWhen, Tier};
use mp_engine::{Combat, CombatSettings, SpellFacts};
use mp_parse::{Reader, Statline};
use silvermere_text::{Msg, Palette, render};

const BOT: &str = "[HP=%h/%H,MA=%m/%M]:%r";
const FIRE: &str = "You fire a %s at %s for %d damage!";
const BOOM: &str = "The fireball explodes, %scausing %d damage!";

fn s(v: &str) -> String {
    v.to_owned()
}

fn rule(cast: &str) -> SpellRule {
    SpellRule {
        cast: s(cast),
        when: SpellWhen::default(),
        max_casts: None,
    }
}

fn block(from: Option<u32>, to: Option<u32>, spells: Vec<SpellRule>) -> LevelBlock {
    LevelBlock {
        levels: Span::new(from, to),
        spells,
    }
}

fn facts(name: &str, room: bool, line: &str) -> SpellFacts {
    SpellFacts {
        name: s(name),
        room,
        cast_line: s(line),
    }
}

/// A mage fighting anything, with no plan yet: melee `a`.
fn mage() -> CombatSettings {
    CombatSettings {
        name: s("Ermias"),
        leader: None,
        targeting: vec![Rule::Priority(vec![s("*")])],
        tiers: vec![],
        room_tiers: vec![],
        by_level: vec![],
        melee: Melee::Command(s("a")),
        spell_costs: [(s("lbol"), 10), (s("mmis"), 3), (s("fbal"), 20)].into(),
        spell_facts: [
            (s("mmis"), facts("magic missile", false, FIRE)),
            (s("lbol"), facts("lightning bolt", false, FIRE)),
            (s("fbal"), facts("fireball", true, BOOM)),
        ]
        .into(),
        monsters: vec![s("kobold"), s("giant rat"), s("orc shaman")],
        leader_target_stale: Duration::from_secs(6),
        round: Duration::from_secs(5),
        routine: None,
    }
}

struct Fight {
    combat: Combat,
    reader: Reader,
    now: Instant,
    sent: Vec<String>,
}

impl Fight {
    fn new(settings: CombatSettings, level: Option<u32>) -> Self {
        let mut combat = Combat::new(settings);
        combat.set_level(level);
        Self {
            combat,
            reader: Reader::new(Some(Statline::compile(BOT).unwrap())),
            now: Instant::now(),
            sent: Vec::new(),
        }
    }

    fn bytes(&mut self, b: &[u8]) -> &mut Self {
        for item in self.reader.feed(b) {
            self.sent.extend(self.combat.on_item(&item, self.now));
        }
        self
    }

    fn msg(&mut self, m: Msg) -> &mut Self {
        let b = render(&m, Palette::new(0).unwrap()).bytes;
        self.bytes(&b)
    }

    fn prompt(&mut self, mp: i32) -> &mut Self {
        let b = format!("\x1b[79D\x1b[K\x1b[0;37m[HP=120/120,MA={mp}/80]:").into_bytes();
        self.bytes(&b)
    }

    /// A combat round later.
    fn round(&mut self) -> &mut Self {
        self.now += Duration::from_secs(5);
        self
    }

    fn here(&mut self, who: &str) -> &mut Self {
        self.msg(Msg::RoomAlsoHere { who: s(who) })
    }

    /// The caster's own line of a round of a spell.
    fn cast_line(&mut self, text: &str) -> &mut Self {
        self.msg(Msg::SpellLineOth {
            text: format!("{text}\r"),
        })
    }

    fn engaged(&mut self) -> &mut Self {
        self.msg(Msg::CombatEngaged)
    }

    fn killed(&mut self) -> &mut Self {
        self.msg(Msg::ExpGain { exp: 10 })
    }

    fn take_sent(&mut self) -> Vec<String> {
        std::mem::take(&mut self.sent)
    }
}

/// Levels 1-7 cast `mmis`; 8 and up `lbol`.
fn planned() -> CombatSettings {
    let mut set = mage();
    set.by_level = vec![
        block(Some(1), Some(7), vec![rule("mmis")]),
        block(Some(8), None, vec![rule("lbol")]),
    ];
    set
}

#[test]
fn the_block_for_the_bot_s_level_is_cast() {
    let mut f = Fight::new(planned(), Some(3));
    f.here("kobold").prompt(80);
    assert_eq!(f.take_sent(), ["mmis kobold"]);
    let mut f = Fight::new(planned(), Some(9));
    f.here("kobold").prompt(80);
    assert_eq!(f.take_sent(), ["lbol kobold"]);
}

// The level comes from `exp`, which the session asks at once. Until it's known the bot
// can't tell which block to fight with, so it starts no fight: a caster that went in with
// melee would never stand out of the fight for its sheets.
#[test]
fn a_planned_bot_starts_no_fight_until_its_level_is_known() {
    let mut f = Fight::new(planned(), None);
    assert!(f.combat.wants_level());
    f.here("kobold").prompt(80);
    assert!(f.take_sent().is_empty());
    f.combat.set_level(Some(9));
    f.round().prompt(80);
    assert_eq!(f.take_sent(), ["lbol kobold"]);
}

// The step-1 tiers are a block for every level, after the level blocks.
#[test]
fn a_level_no_block_holds_falls_to_the_tiers_then_melee() {
    let mut set = mage();
    set.by_level = vec![block(Some(8), None, vec![rule("lbol")])];
    let mut f = Fight::new(set.clone(), Some(3));
    f.here("kobold").prompt(80);
    assert_eq!(f.take_sent(), ["a kobold"]);
    set.tiers = vec![Tier {
        spell: s("mmis"),
        min_mp: 0,
    }];
    let mut f = Fight::new(set, Some(3));
    f.here("kobold").prompt(80);
    assert_eq!(f.take_sent(), ["mmis kobold"]);
}

// A bot with no level blocks needs no level.
#[test]
fn a_bot_with_only_tiers_fights_without_a_level() {
    let mut set = mage();
    set.tiers = vec![Tier {
        spell: s("mmis"),
        min_mp: 0,
    }];
    let mut f = Fight::new(set, None);
    assert!(!f.combat.wants_level());
    f.here("kobold").prompt(80);
    assert_eq!(f.take_sent(), ["mmis kobold"]);
}

#[test]
fn min_mp_may_be_a_share_of_the_maximum() {
    let mut set = mage();
    let mut lbol = rule("lbol");
    lbol.when.min_mp = Some(Amount::Percent(50));
    set.by_level = vec![block(None, None, vec![lbol, rule("mmis")])];
    let mut f = Fight::new(set.clone(), Some(5));
    f.here("kobold").prompt(40);
    assert_eq!(f.take_sent(), ["lbol kobold"], "40 of 80 is 50%");
    let mut f = Fight::new(set, Some(5));
    f.here("kobold").prompt(39);
    assert_eq!(f.take_sent(), ["mmis kobold"]);
}

// A rule the bot can't pay for is passed over, and taken up again as MP comes back.
#[test]
fn a_rule_the_mp_can_t_pay_for_falls_to_the_next_and_climbs_back() {
    let mut set = mage();
    set.by_level = vec![block(None, None, vec![rule("lbol"), rule("mmis")])];
    let mut f = Fight::new(set, Some(5));
    f.here("kobold").prompt(80);
    assert_eq!(f.take_sent(), ["lbol kobold"]);
    f.engaged().round().prompt(9);
    assert_eq!(f.take_sent(), ["mmis kobold"]);
    f.round().prompt(2);
    assert_eq!(f.take_sent(), ["a kobold"]);
    f.round().prompt(60);
    assert_eq!(f.take_sent(), ["lbol kobold"]);
}

// Room or single target is the game data's: `fbal` goes out bare.
#[test]
fn a_room_spell_waits_for_enough_enemies() {
    let mut set = mage();
    let mut fbal = rule("fbal");
    fbal.when.enemies = Some(Span::new(Some(3), None));
    set.by_level = vec![block(None, None, vec![fbal, rule("lbol")])];
    let mut f = Fight::new(set.clone(), Some(20));
    f.here("kobold, giant rat").prompt(80);
    assert_eq!(f.take_sent(), ["lbol kobold"]);
    let mut f = Fight::new(set, Some(20));
    f.here("kobold, giant rat, orc shaman").prompt(80);
    assert_eq!(f.take_sent(), ["fbal"]);
}

#[test]
fn a_rule_for_some_monsters_goes_only_at_them() {
    let mut set = mage();
    let mut lbol = rule("lbol");
    lbol.when.monsters = vec![s("*shaman*")];
    set.by_level = vec![block(None, None, vec![lbol, rule("mmis")])];
    let mut f = Fight::new(set.clone(), Some(9));
    f.here("orc shaman").prompt(80);
    assert_eq!(f.take_sent(), ["lbol orc shaman"]);
    let mut f = Fight::new(set, Some(9));
    f.here("kobold").prompt(80);
    assert_eq!(f.take_sent(), ["mmis kobold"]);
}

#[test]
fn a_rule_for_a_routine_holds_only_under_it() {
    let mut set = mage();
    let mut lbol = rule("lbol");
    lbol.when.routine = vec![s("kobold-*")];
    set.by_level = vec![block(None, None, vec![lbol, rule("mmis")])];
    set.routine = Some(s("kobold-loop"));
    let mut f = Fight::new(set.clone(), Some(9));
    f.here("kobold").prompt(80);
    assert_eq!(f.take_sent(), ["lbol kobold"]);
    set.routine = Some(s("arena"));
    let mut f = Fight::new(set.clone(), Some(9));
    f.here("kobold").prompt(80);
    assert_eq!(f.take_sent(), ["mmis kobold"]);
    set.routine = None;
    let mut f = Fight::new(set, Some(9));
    f.here("kobold").prompt(80);
    assert_eq!(f.take_sent(), ["mmis kobold"]);
}

// A spell repeats by itself every round, so the casts are counted from the game's lines:
// after two at this kobold, the bot melees it. The next monster gets two of its own.
#[test]
fn max_casts_counts_each_monster_s_casts_from_the_game_s_lines() {
    let mut set = mage();
    let mut mmis = rule("mmis");
    mmis.max_casts = Some(2);
    set.by_level = vec![block(None, None, vec![mmis])];
    let mut f = Fight::new(set, Some(3));
    f.here("kobold, giant rat").prompt(80);
    assert_eq!(f.take_sent(), ["mmis kobold"]);
    f.engaged()
        .cast_line("You fire a magic missile at kobold for 4 damage!")
        .round()
        .prompt(77);
    assert!(f.take_sent().is_empty(), "one cast of two");
    f.cast_line("You fire a magic missile at kobold for 3 damage!")
        .round()
        .prompt(74);
    assert_eq!(f.take_sent(), ["a kobold"]);
    f.killed().msg(Msg::CombatOff).round().prompt(74);
    assert_eq!(f.take_sent(), ["mmis giant rat"]);
}

#[test]
fn a_failed_or_resisted_cast_counts_as_a_cast() {
    let mut set = mage();
    let mut mmis = rule("mmis");
    mmis.max_casts = Some(2);
    set.by_level = vec![block(None, None, vec![mmis])];
    let mut f = Fight::new(set, Some(3));
    f.here("kobold").prompt(80);
    assert_eq!(f.take_sent(), ["mmis kobold"]);
    f.engaged()
        .msg(Msg::CastFailAt {
            spell: s("magic missile"),
            target: s("kobold"),
        })
        .round()
        .prompt(77);
    assert!(f.take_sent().is_empty());
    f.msg(Msg::CastResisted {
        spell: s("magic missile"),
        target: s("kobold"),
    })
    .round()
    .prompt(74);
    assert_eq!(f.take_sent(), ["a kobold"]);
}

// Another spell's line is no cast of this one.
#[test]
fn only_the_spell_going_out_is_counted() {
    let mut set = mage();
    let mut mmis = rule("mmis");
    mmis.max_casts = Some(1);
    set.by_level = vec![block(None, None, vec![mmis])];
    let mut f = Fight::new(set, Some(3));
    f.here("kobold").prompt(80);
    assert_eq!(f.take_sent(), ["mmis kobold"]);
    f.engaged()
        .msg(Msg::CastFailAt {
            spell: s("lightning bolt"),
            target: s("kobold"),
        })
        .round()
        .prompt(77);
    assert!(f.take_sent().is_empty());
}

// A room spell's limit is the room's: a kill doesn't give it back, an emptied room does.
#[test]
fn a_room_spell_s_casts_count_for_the_room() {
    let mut set = mage();
    let mut fbal = rule("fbal");
    fbal.max_casts = Some(1);
    set.by_level = vec![block(None, None, vec![fbal, rule("mmis")])];
    let mut f = Fight::new(set, Some(20));
    f.here("kobold, giant rat").prompt(80);
    assert_eq!(f.take_sent(), ["fbal"]);
    f.engaged()
        .cast_line("The fireball explodes, causing 12 damage!")
        .killed()
        .msg(Msg::CombatOff)
        .round()
        .prompt(60);
    assert_eq!(f.take_sent(), ["mmis giant rat"]);
    f.engaged().killed().msg(Msg::CombatOff).round().prompt(60);
    assert!(f.take_sent().is_empty(), "the room is clear");
    f.here("orc shaman").round().prompt(60);
    assert_eq!(f.take_sent(), ["fbal"]);
}

// `@auto-nuke off` melees whatever the plan says.
#[test]
fn with_nuking_off_the_plan_gives_way_to_melee() {
    let mut f = Fight::new(planned(), Some(3));
    f.combat.set_nuke(false);
    f.here("kobold").prompt(80);
    assert_eq!(f.take_sent(), ["a kobold"]);
}
