//! Healing spells planned by level (#126), as pure logic fed with silvermere's own rendered
//! text: whom a heal is for, the target's HP and the healer's own MP, the most hurt first,
//! party spells by how many are hurt, and a heal in a fight with the attack taken up again.

use std::time::{Duration, Instant};

use mp_config::{Amount, HealRule, HealWhen, LevelBlock, Melee, Rule, Span, SpellRule, Whom};
use mp_engine::{AllyHp, Combat, CombatSettings, SpellFacts};
use mp_parse::{Reader, Statline};
use silvermere_text::{Msg, Palette, render};

const BOT: &str = "[HP=%h/%H,MA=%m/%M]:%r";

fn s(v: &str) -> String {
    v.to_owned()
}

fn heal(cast: &str, on: Vec<Whom>, below: i32) -> HealRule {
    HealRule {
        cast: s(cast),
        on,
        when: HealWhen {
            hp_below: Amount::Percent(below),
            ..HealWhen::default()
        },
    }
}

fn facts(name: &str, party: bool) -> SpellFacts {
    SpellFacts {
        name: s(name),
        party,
        ..SpellFacts::default()
    }
}

/// A cleric that fights anything with `mmis`, healing by `heals`.
fn cleric(heals: Vec<HealRule>) -> CombatSettings {
    CombatSettings {
        name: s("Ermias"),
        leader: None,
        targeting: vec![Rule::Priority(vec![s("*")])],
        tiers: vec![],
        room_tiers: vec![],
        by_level: vec![LevelBlock {
            levels: Span::new(None, None),
            spells: vec![SpellRule {
                cast: s("mmis"),
                when: Default::default(),
                max_casts: None,
            }],
            heals,
        }],
        melee: Melee::Command(s("a")),
        spell_costs: [
            (s("mihe"), 2),
            (s("mahe"), 6),
            (s("mrai"), 15),
            (s("mmis"), 1),
        ]
        .into(),
        spell_facts: [
            (s("mihe"), facts("minor healing", false)),
            (s("mahe"), facts("major healing", false)),
            (s("mrai"), facts("major healing rain", true)),
            (s("mmis"), facts("magic missile", false)),
        ]
        .into(),
        monsters: vec![s("kobold")],
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
    fn new(settings: CombatSettings) -> Self {
        let mut combat = Combat::new(settings);
        combat.set_level(Some(10));
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

    /// The healer's prompt: its HP of 100, and MP of 80.
    fn prompt(&mut self, hp: i32, mp: i32) -> &mut Self {
        let b = format!("\x1b[79D\x1b[K\x1b[0;37m[HP={hp}/100,MA={mp}/80]:").into_bytes();
        self.bytes(&b)
    }

    fn round(&mut self) -> &mut Self {
        self.now += Duration::from_secs(5);
        self
    }

    fn here(&mut self, who: &str) -> &mut Self {
        self.msg(Msg::RoomAlsoHere { who: s(who) })
    }

    /// What the server says of another character it runs: HP out of 100.
    fn ally(&mut self, name: &str, hp: i32) -> &mut Self {
        self.combat.set_ally(
            name,
            Some(AllyHp {
                hp,
                max_hp: Some(100),
            }),
        );
        self
    }

    fn take_sent(&mut self) -> Vec<String> {
        std::mem::take(&mut self.sent)
    }
}

#[test]
fn the_healer_heals_itself_below_its_line() {
    let mut f = Fight::new(cleric(vec![heal("mihe", vec![Whom::Me], 50)]));
    f.prompt(51, 80);
    assert!(f.take_sent().is_empty());
    f.prompt(49, 80);
    assert_eq!(
        f.take_sent(),
        ["mihe"],
        "a heal at nobody is the healer's own"
    );
}

#[test]
fn the_most_hurt_in_the_party_here_is_healed_first() {
    let mut f = Fight::new(cleric(vec![heal("mahe", vec![Whom::Me, Whom::Party], 50)]));
    f.ally("Raijin", 40).ally("Grond", 20).here("Raijin, Grond");
    f.prompt(30, 80);
    assert_eq!(f.take_sent(), ["mahe Grond"]);
}

#[test]
fn an_ally_who_isn_t_here_isn_t_healed() {
    let mut f = Fight::new(cleric(vec![heal("mahe", vec![Whom::Party], 50)]));
    f.ally("Raijin", 10).here("Grond");
    f.prompt(100, 80);
    assert!(f.take_sent().is_empty());
}

#[test]
fn an_ally_who_leaves_isn_t_healed() {
    let mut f = Fight::new(cleric(vec![heal("mahe", vec![Whom::Party], 50)]));
    f.ally("Raijin", 10).here("Raijin");
    f.msg(Msg::LeftDir1 {
        who: s("Raijin"),
        dir: s("north"),
        drag: s(""),
    })
    .prompt(100, 80);
    assert!(f.take_sent().is_empty());
}

#[test]
fn a_named_heal_is_for_them_alone() {
    let mut f = Fight::new(cleric(vec![heal(
        "mahe",
        vec![Whom::Named(s("Raijin"))],
        50,
    )]));
    f.ally("Raijin", 45).ally("Grond", 10).here("Raijin, Grond");
    f.prompt(100, 80);
    assert_eq!(f.take_sent(), ["mahe Raijin"]);
}

// The healer's own MP held in reserve: below it, the next rule.
#[test]
fn a_heal_waits_for_the_healer_s_mp() {
    let mut big = heal("mahe", vec![Whom::Party], 50);
    big.when.min_mp = Some(Amount::Percent(50));
    let small = heal("mihe", vec![Whom::Party], 50);
    let mut f = Fight::new(cleric(vec![big, small]));
    f.ally("Raijin", 30).here("Raijin");
    f.prompt(100, 40);
    assert_eq!(f.take_sent(), ["mahe Raijin"]);
    let mut big = heal("mahe", vec![Whom::Party], 50);
    big.when.min_mp = Some(Amount::Percent(50));
    let mut f = Fight::new(cleric(vec![big, heal("mihe", vec![Whom::Party], 50)]));
    f.ally("Raijin", 30).here("Raijin");
    f.prompt(100, 39);
    assert_eq!(f.take_sent(), ["mihe Raijin"]);
}

// A party spell goes out bare, once enough of the party here (the healer too) is hurt.
#[test]
fn a_party_heal_waits_for_enough_of_the_party_hurt() {
    let mut rain = heal("mrai", vec![], 70);
    rain.when.allies = Some(Span::new(Some(2), None));
    let mut f = Fight::new(cleric(vec![rain.clone()]));
    f.ally("Raijin", 60).ally("Grond", 90).here("Raijin, Grond");
    f.prompt(100, 80);
    assert!(f.take_sent().is_empty(), "one hurt of three");
    let mut f = Fight::new(cleric(vec![rain]));
    f.ally("Raijin", 60).ally("Grond", 90).here("Raijin, Grond");
    f.prompt(65, 80);
    assert_eq!(f.take_sent(), ["mrai"]);
}

// A heal comes before the attack. silvermere breaks off the fight for it (`*Combat Off*`),
// and the attack is taken up again the round after.
#[test]
fn a_heal_in_a_fight_breaks_off_and_the_attack_comes_back() {
    let mut f = Fight::new(cleric(vec![heal("mahe", vec![Whom::Party], 50)]));
    f.here("Raijin, kobold").prompt(100, 80);
    assert_eq!(f.take_sent(), ["mmis kobold"]);
    f.msg(Msg::CombatEngaged)
        .ally("Raijin", 30)
        .round()
        .prompt(100, 77);
    assert_eq!(f.take_sent(), ["mahe Raijin"]);
    f.msg(Msg::CombatOff).prompt(100, 71);
    assert!(f.take_sent().is_empty(), "the heal took the round");
    f.ally("Raijin", 80).round().prompt(100, 71);
    assert_eq!(f.take_sent(), ["mmis kobold"]);
}

// At most one heal a round, though someone is still hurt.
#[test]
fn one_heal_a_round() {
    let mut f = Fight::new(cleric(vec![heal("mihe", vec![Whom::Party], 50)]));
    f.ally("Raijin", 30).here("Raijin").prompt(100, 80);
    assert_eq!(f.take_sent(), ["mihe Raijin"]);
    f.prompt(100, 78);
    assert!(f.take_sent().is_empty());
    f.round().prompt(100, 78);
    assert_eq!(f.take_sent(), ["mihe Raijin"]);
}

// An ally whose maximum isn't known is healed by points only.
#[test]
fn a_share_of_an_unknown_maximum_is_no_line() {
    let mut f = Fight::new(cleric(vec![heal("mihe", vec![Whom::Party], 50)]));
    f.combat.set_ally(
        "Raijin",
        Some(AllyHp {
            hp: 5,
            max_hp: None,
        }),
    );
    f.here("Raijin").prompt(100, 80);
    assert!(f.take_sent().is_empty());
}

// The server says when a character it ran has gone.
#[test]
fn an_ally_the_server_lets_go_isn_t_healed() {
    let mut f = Fight::new(cleric(vec![heal("mihe", vec![Whom::Party], 50)]));
    f.ally("Raijin", 30).here("Raijin");
    f.combat.set_ally("Raijin", None);
    f.prompt(100, 80);
    assert!(f.take_sent().is_empty());
}

#[test]
fn a_heal_for_some_enemies_or_a_routine_holds_only_then() {
    let mut calm = heal("mihe", vec![Whom::Me], 90);
    calm.when.enemies = Some(Span::new(Some(0), Some(0)));
    let mut f = Fight::new(cleric(vec![calm.clone()]));
    f.here("kobold").prompt(80, 80);
    assert_eq!(
        f.take_sent(),
        ["mmis kobold"],
        "not while a monster is here"
    );
    let mut f = Fight::new(cleric(vec![calm]));
    f.prompt(80, 80);
    assert_eq!(f.take_sent(), ["mihe"]);
    let mut arena = heal("mihe", vec![Whom::Me], 90);
    arena.when.routine = vec![s("arena")];
    let mut f = Fight::new(cleric(vec![arena]));
    f.prompt(80, 80);
    assert!(f.take_sent().is_empty(), "no routine running");
}

#[test]
fn heals_follow_the_level_block() {
    let mut set = cleric(vec![heal("mihe", vec![Whom::Me], 50)]);
    set.by_level[0].levels = Span::new(Some(1), Some(5));
    set.by_level.push(LevelBlock {
        levels: Span::new(Some(6), None),
        spells: vec![],
        heals: vec![heal("mahe", vec![Whom::Me], 50)],
    });
    let mut f = Fight::new(set);
    f.prompt(40, 80);
    assert_eq!(f.take_sent(), ["mahe"]);
}
