//! The combat engine (design §6.4), as pure logic, fed with silvermere's own rendered text:
//! the room's monsters, the targeting rules, spell tiers re-picked by MP, room tiers, the
//! melee fallback, kills, and "no effect".

use std::time::{Duration, Instant};

use mp_config::{Melee, RoomTier, Rule, Tier};
use mp_engine::{Combat, CombatSettings};
use mp_parse::{Item, Reader, Statline};
use silvermere_text::{Msg, Palette, render};

const BOT: &str = "[HP=%h/%H,MA=%m/%M]:%r";

fn s(v: &str) -> String {
    v.to_owned()
}

fn tier(spell: &str, min_mp: i32) -> Tier {
    Tier {
        spell: s(spell),
        min_mp,
    }
}

/// A mage bot: `lbol` from 60 MP, `mmis` from 5, a room spell with three enemies, `a` to
/// melee, assisting the leader Raijin.
fn settings() -> CombatSettings {
    CombatSettings {
        name: s("Ermias"),
        leader: Some(s("Raijin")),
        targeting: vec![
            Rule::KillOrder(true),
            Rule::AssistLeader(true),
            Rule::FightBack(true),
        ],
        tiers: vec![tier("lbol", 60), tier("mmis", 5)],
        room_tiers: vec![],
        melee: Melee::Command(s("a")),
        spell_costs: [(s("lbol"), 10), (s("mmis"), 3), (s("fbal"), 20)].into(),
        monsters: vec![
            s("practice dummy"),
            s("orc shaman"),
            s("kobold"),
            s("giant rat"),
        ],
        leader_target_stale: Duration::from_secs(6),
    }
}

/// Plays rendered game bytes into the engine, round by round.
struct Fight {
    combat: Combat,
    reader: Reader,
    now: Instant,
    sent: Vec<String>,
}

impl Fight {
    fn new(settings: CombatSettings) -> Self {
        Self {
            combat: Combat::new(settings),
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

    /// A prompt, as at the end of a combat round.
    fn prompt(&mut self, mp: i32) -> &mut Self {
        let b = format!("\x1b[79D\x1b[K\x1b[0;37m[HP=120/120,MA={mp}/80]:").into_bytes();
        self.bytes(&b)
    }

    fn wait(&mut self, secs: u64) -> &mut Self {
        self.now += Duration::from_secs(secs);
        self
    }

    fn here(&mut self, who: &str) -> &mut Self {
        self.msg(Msg::RoomAlsoHere { who: s(who) })
    }

    fn leader_attacks(&mut self, mon: &str) -> &mut Self {
        self.msg(Msg::CombatMovesToAttackMon {
            who: s("Raijin"),
            mon: s(mon),
        })
    }

    fn take_sent(&mut self) -> Vec<String> {
        std::mem::take(&mut self.sent)
    }
}

#[test]
fn an_empty_room_starts_no_fight() {
    let mut f = Fight::new(settings());
    f.prompt(80).here("Raijin").prompt(80);
    assert!(f.take_sent().is_empty());
}

#[test]
fn the_bot_assists_the_leader_with_its_top_tier() {
    let mut f = Fight::new(settings());
    f.here("Raijin, practice dummy")
        .leader_attacks("practice dummy")
        .prompt(80);
    assert_eq!(f.take_sent(), ["lbol practice dummy"]);
}

#[test]
fn a_spell_that_repeats_by_itself_is_not_sent_again() {
    let mut f = Fight::new(settings());
    f.here("Raijin, practice dummy")
        .leader_attacks("practice dummy")
        .prompt(80);
    f.msg(Msg::CombatEngaged)
        .wait(5)
        .prompt(70)
        .wait(5)
        .prompt(65);
    assert_eq!(f.take_sent(), ["lbol practice dummy"]);
}

#[test]
fn falling_mp_drops_a_tier_and_rising_mp_climbs_back() {
    let mut f = Fight::new(settings());
    f.here("Raijin, practice dummy")
        .leader_attacks("practice dummy")
        .prompt(80);
    f.msg(Msg::CombatEngaged).wait(5).prompt(55);
    f.wait(5).prompt(62);
    assert_eq!(
        f.take_sent(),
        [
            "lbol practice dummy",
            "mmis practice dummy",
            "lbol practice dummy"
        ]
    );
}

#[test]
fn below_every_tier_the_bot_falls_back_to_melee() {
    let mut f = Fight::new(settings());
    f.here("Raijin, practice dummy")
        .leader_attacks("practice dummy")
        .prompt(4);
    assert_eq!(f.take_sent(), ["a practice dummy"]);
}

#[test]
fn a_caster_with_no_melee_does_nothing_below_its_tiers() {
    let mut set = settings();
    set.melee = Melee::None;
    let mut f = Fight::new(set);
    f.here("Raijin, practice dummy")
        .leader_attacks("practice dummy")
        .prompt(4);
    assert!(f.take_sent().is_empty());
}

#[test]
fn a_tier_needs_the_spells_cost_as_well_as_its_floor() {
    let mut set = settings();
    set.tiers = vec![tier("fbal", 0), tier("mmis", 0)];
    let mut f = Fight::new(set);
    f.here("Raijin, practice dummy")
        .leader_attacks("practice dummy")
        .prompt(15);
    assert_eq!(f.take_sent(), ["mmis practice dummy"]);
}

#[test]
fn only_one_tier_change_happens_in_a_round() {
    let mut f = Fight::new(settings());
    f.here("Raijin, practice dummy")
        .leader_attacks("practice dummy")
        .prompt(80);
    f.msg(Msg::CombatEngaged)
        .wait(1)
        .prompt(55)
        .wait(1)
        .prompt(62)
        .wait(1)
        .prompt(50);
    assert_eq!(f.take_sent(), ["lbol practice dummy"]);
    f.wait(5).prompt(50);
    assert_eq!(f.take_sent(), ["mmis practice dummy"]);
}

#[test]
fn a_kill_drops_the_target_and_nothing_is_cast_at_the_corpse() {
    let mut f = Fight::new(settings());
    f.here("Raijin, practice dummy")
        .leader_attacks("practice dummy")
        .prompt(80);
    f.msg(Msg::CombatEngaged).wait(5);
    f.msg(Msg::ExpGain { exp: 40 })
        .msg(Msg::CombatOff)
        .prompt(55);
    assert_eq!(f.take_sent(), ["lbol practice dummy"]);
    assert_eq!(f.combat.target(), None);
}

#[test]
fn after_a_kill_the_next_monster_is_picked() {
    let mut set = settings();
    set.targeting = vec![Rule::Priority(vec![s("*")])];
    let mut f = Fight::new(set);
    f.here("kobold, giant rat").prompt(80);
    f.msg(Msg::CombatEngaged)
        .wait(5)
        .msg(Msg::ExpGain { exp: 10 })
        .msg(Msg::CombatOff)
        .prompt(70);
    assert_eq!(f.take_sent(), ["lbol kobold", "lbol giant rat"]);
}

#[test]
fn a_priority_monster_comes_before_the_leaders_target() {
    let mut set = settings();
    set.targeting = vec![
        Rule::Priority(vec![s("*shaman*")]),
        Rule::AssistLeader(true),
    ];
    let mut f = Fight::new(set);
    f.here("Raijin, practice dummy, orc shaman")
        .leader_attacks("practice dummy")
        .prompt(80);
    assert_eq!(f.take_sent(), ["lbol orc shaman"]);
}

#[test]
fn a_kill_order_comes_before_everything() {
    let mut f = Fight::new(settings());
    f.here("Raijin, practice dummy, kobold")
        .leader_attacks("practice dummy");
    f.combat.order_kill("kobold");
    f.prompt(80);
    assert_eq!(f.take_sent(), ["lbol kobold"]);
}

#[test]
fn a_stale_leader_target_is_not_assisted() {
    let mut f = Fight::new(settings());
    f.here("Raijin, practice dummy")
        .leader_attacks("practice dummy")
        .wait(7)
        .prompt(80);
    assert!(f.take_sent().is_empty());
}

#[test]
fn a_player_is_never_taken_for_a_monster() {
    let mut set = settings();
    set.targeting = vec![Rule::Priority(vec![s("*")])];
    let mut f = Fight::new(set);
    f.here("Raijin, Bob").prompt(80);
    assert!(f.take_sent().is_empty());
}

#[test]
fn a_monster_with_a_prefix_is_still_a_monster() {
    let mut set = settings();
    set.targeting = vec![Rule::Priority(vec![s("*kobold*")])];
    let mut f = Fight::new(set);
    f.here("Raijin, large kobold").prompt(80);
    assert_eq!(f.take_sent(), ["lbol large kobold"]);
}

#[test]
fn a_spell_with_no_effect_drops_to_the_next_tier() {
    let mut f = Fight::new(settings());
    f.here("Raijin, practice dummy")
        .leader_attacks("practice dummy")
        .prompt(80);
    f.msg(Msg::CastNoEffectOn {
        target: s("practice dummy"),
    })
    .wait(5)
    .prompt(80);
    assert_eq!(
        f.take_sent(),
        ["lbol practice dummy", "mmis practice dummy"]
    );
}

#[test]
fn a_weapon_with_no_effect_is_not_swung_again() {
    let mut f = Fight::new(settings());
    f.here("Raijin, practice dummy")
        .leader_attacks("practice dummy")
        .prompt(4);
    f.msg(Msg::CombatEngaged)
        .msg(Msg::CombatNoEffectWeapon)
        .wait(5)
        .prompt(4);
    assert_eq!(f.take_sent(), ["a practice dummy"]);
}

#[test]
fn a_room_tier_is_cast_bare_when_enough_monsters_are_here() {
    let mut set = settings();
    set.targeting = vec![Rule::Priority(vec![s("*")])];
    set.room_tiers = vec![RoomTier {
        spell: s("fbal"),
        min_mp: 40,
        min_enemies: 3,
    }];
    let mut f = Fight::new(set);
    f.here("kobold, kobold, giant rat").prompt(80);
    assert_eq!(f.take_sent(), ["fbal"]);
}

#[test]
fn a_room_tier_waits_for_its_count_of_enemies() {
    let mut set = settings();
    set.targeting = vec![Rule::Priority(vec![s("*")])];
    set.room_tiers = vec![RoomTier {
        spell: s("fbal"),
        min_mp: 40,
        min_enemies: 3,
    }];
    let mut f = Fight::new(set);
    f.here("kobold, giant rat").prompt(80);
    assert_eq!(f.take_sent(), ["lbol kobold"]);
}

// A monster's attack prints the monster's own attack text from the game data, which the
// parser doesn't read yet (MVP 4b); until then the session tells the engine who attacks.
#[test]
fn a_monster_attacking_the_bot_is_fought_back() {
    let mut set = settings();
    set.targeting = vec![Rule::FightBack(true)];
    let mut f = Fight::new(set);
    f.here("kobold, giant rat");
    f.combat.attacked_by("giant rat", f.now);
    f.prompt(80);
    assert_eq!(f.take_sent(), ["lbol giant rat"]);
}

#[test]
fn an_attacker_who_isnt_a_monster_is_not_fought_back() {
    let mut set = settings();
    set.targeting = vec![Rule::FightBack(true)];
    let mut f = Fight::new(set);
    f.here("Bob, kobold");
    f.combat.attacked_by("Bob", f.now);
    f.prompt(80);
    assert!(f.take_sent().is_empty());
}

#[test]
fn a_fight_broken_off_is_taken_up_again() {
    let mut f = Fight::new(settings());
    f.here("Raijin, practice dummy")
        .leader_attacks("practice dummy")
        .prompt(80);
    f.msg(Msg::CombatEngaged)
        .wait(5)
        .msg(Msg::CombatOff)
        .leader_attacks("practice dummy")
        .prompt(80);
    assert_eq!(
        f.take_sent(),
        ["lbol practice dummy", "lbol practice dummy"]
    );
}

#[test]
fn items_are_read_the_same_way_whatever_their_source() {
    // A guard that the helpers above feed the engine real parsed items.
    let mut r = Reader::new(None);
    let items = r.feed(&render(&Msg::CombatEngaged, Palette::new(0).unwrap()).bytes);
    assert_eq!(items, [Item::Event(mp_parse::Event::CombatEngaged)]);
}
