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
        round: Duration::from_secs(5),
        by_level: vec![],
        spell_facts: Default::default(),
        routine: None,
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

/// A bot fighting whatever its priority list names, as a camp's or a hunter's does.
fn hunter(globs: &[&str]) -> CombatSettings {
    let mut set = settings();
    set.targeting = vec![Rule::Priority(globs.iter().map(|g| s(g)).collect())];
    set
}

// A monster that walks in prints its own words from the game data (#105). The name is
// read off the front: the longest run of words that's a monster's name.
#[test]
fn a_monster_that_creeps_in_joins_the_room() {
    let mut f = Fight::new(hunter(&["*rat*"]));
    f.here("Raijin").prompt(80);
    assert!(f.take_sent().is_empty());
    f.msg(Msg::MonsterArrivalText {
        text: s("A angry giant rat creeps into the room from nowhere."),
    })
    .prompt(80);
    assert_eq!(f.take_sent(), ["lbol angry giant rat"]);
    assert_eq!(f.combat.monsters(), ["angry giant rat"]);
}

#[test]
fn arrivals_from_any_way_in_are_read() {
    let mut f = Fight::new(hunter(&["*"]));
    f.here("Raijin").prompt(80);
    f.msg(Msg::MonsterArrivalText {
        text: s("An orc shaman slithers quietly into the room from the north."),
    });
    f.msg(Msg::MonsterArrivesAbove { mon: s("kobold") });
    f.msg(Msg::MonsterArrivesDir {
        mon: s("giant rat"),
        dir: s("east"),
    });
    assert_eq!(f.combat.monsters(), ["orc shaman", "kobold", "giant rat"]);
}

#[test]
fn a_player_walking_in_is_not_a_monster() {
    let mut f = Fight::new(hunter(&["*"]));
    f.here("Raijin").prompt(80);
    f.msg(Msg::ArriveDir {
        who: s("Bob"),
        dir: s("south"),
        drag: s(""),
    });
    f.msg(Msg::MonsterArrivalText {
        text: s("A strange wind blows into the room from nowhere."),
    })
    .prompt(80);
    assert!(f.combat.monsters().is_empty());
    assert!(f.take_sent().is_empty());
}

// The leader's attack line says the monster is in the room, though the bot never saw it
// arrive (#105): assist it.
#[test]
fn the_leader_s_target_is_assisted_though_unseen_till_then() {
    let mut f = Fight::new(settings());
    f.here("Raijin").prompt(80);
    f.leader_attacks("kobold").prompt(80);
    assert_eq!(f.take_sent(), ["lbol kobold"]);
    // Not a monster's name: no fight.
    let mut f = Fight::new(settings());
    f.here("Raijin").prompt(80);
    f.leader_attacks("Bob").prompt(80);
    assert!(f.take_sent().is_empty());
}

// The bot's attack can reach the game after the leader has killed its target: the bot wasn't
// fighting it, so no share of the kill and no `*Combat Off*` tells it so (#118). The game
// refuses the attack instead; the monster is let go, and the leader's next is assisted.
#[test]
fn a_melee_attack_refused_by_speech_lets_the_monster_go() {
    let mut f = Fight::new(settings());
    f.here("Raijin").prompt(4);
    f.leader_attacks("kobold").prompt(4);
    assert_eq!(f.take_sent(), ["a kobold"]);
    // Fast talk on (a new character's default): the line is said, not swung.
    f.msg(Msg::YouSay {
        verb: s("say"),
        text: s("a kobold"),
    })
    .prompt(4);
    assert_eq!(f.combat.target(), None);
    assert!(f.combat.monsters().is_empty());
    f.leader_attacks("giant rat").prompt(4);
    assert_eq!(f.take_sent(), ["a giant rat"]);
}

#[test]
fn a_melee_attack_with_no_effect_lets_the_monster_go() {
    let mut f = Fight::new(settings());
    f.here("Raijin, kobold, kobold").prompt(4);
    f.leader_attacks("kobold").prompt(4);
    assert_eq!(f.take_sent(), ["a kobold"]);
    // Fast talk off. An attack goes to the first of a shared name, so none of them is here.
    f.msg(Msg::NoEffect).prompt(4);
    assert_eq!(f.combat.target(), None);
    assert!(f.combat.monsters().is_empty());
    f.leader_attacks("giant rat").prompt(4);
    assert_eq!(f.take_sent(), ["a giant rat"]);
}

#[test]
fn a_spell_at_a_monster_not_here_lets_it_go() {
    let mut f = Fight::new(settings());
    f.here("Raijin").prompt(80);
    f.leader_attacks("kobold").prompt(80);
    assert_eq!(f.take_sent(), ["lbol kobold"]);
    f.msg(Msg::CastNotHere { what: s("kobold") }).prompt(80);
    assert_eq!(f.combat.target(), None);
    f.leader_attacks("giant rat").prompt(80);
    assert_eq!(f.take_sent(), ["lbol giant rat"]);
}

// Only the answer to the bot's own attack counts: other speech, or a command with no
// effect once the fight is under way, leaves the target alone.
#[test]
fn other_refusals_leave_the_target_alone() {
    let mut f = Fight::new(settings());
    f.here("Raijin, kobold").prompt(4);
    f.leader_attacks("kobold").prompt(4);
    assert_eq!(f.take_sent(), ["a kobold"]);
    f.msg(Msg::YouSay {
        verb: s("say"),
        text: s("a giant rat"),
    })
    .prompt(4);
    assert_eq!(f.combat.target(), Some("kobold"));
    f.msg(Msg::CombatEngaged).msg(Msg::NoEffect).prompt(4);
    assert_eq!(f.combat.target(), Some("kobold"));
    assert_eq!(f.combat.monsters(), ["kobold"]);
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

// Past the experience it may hold untrained, a kill gains nothing, and the game says so in
// place of the experience line: still a kill (#57). A bot kept at an arena's top level
// gets there.
#[test]
fn a_kill_past_the_experience_cap_is_still_a_kill() {
    let mut set = settings();
    set.targeting = vec![Rule::Priority(vec![s("*")])];
    let mut f = Fight::new(set);
    f.here("kobold, giant rat").prompt(80);
    f.msg(Msg::CombatEngaged)
        .wait(5)
        .msg(Msg::ExpCapped)
        .msg(Msg::CombatOff)
        .prompt(70);
    assert_eq!(f.take_sent(), ["lbol kobold", "lbol giant rat"]);
}

// A target that runs off is let go, and the next is taken (#57): kept, the bot would
// attack an empty room and never see the fight end.
#[test]
fn a_target_that_leaves_is_let_go() {
    let mut set = settings();
    set.targeting = vec![Rule::Priority(vec![s("*")])];
    let mut f = Fight::new(set);
    f.here("kobold, giant rat").prompt(80);
    f.msg(Msg::CombatEngaged).wait(5).msg(Msg::MonsterLeftDir {
        mon: s("kobold"),
        dir: s("north"),
    });
    f.msg(Msg::CombatOff).prompt(70);
    assert_eq!(f.take_sent(), ["lbol kobold", "lbol giant rat"]);
    assert_eq!(f.combat.monsters(), ["giant rat"]);
}

#[test]
fn a_lone_target_that_leaves_ends_the_fight() {
    let mut f = Fight::new(settings());
    f.here("Raijin, practice dummy")
        .leader_attacks("practice dummy")
        .prompt(80);
    f.msg(Msg::CombatEngaged).wait(5).msg(Msg::MonsterLeftUp {
        mon: s("practice dummy"),
    });
    f.msg(Msg::CombatOff).prompt(70);
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

#[test]
fn with_nuking_off_the_bot_melees_whatever_its_mp() {
    let mut f = Fight::new(settings());
    f.combat.set_nuke(false);
    f.here("Raijin, practice dummy")
        .leader_attacks("practice dummy")
        .prompt(80);
    assert_eq!(f.take_sent(), ["a practice dummy"]);
    f.combat.set_nuke(true);
    f.wait(5).prompt(80);
    assert_eq!(f.take_sent(), ["lbol practice dummy"], "nuking again");
}

#[test]
fn a_hold_picks_no_new_fight_but_keeps_the_one_going_and_kill_orders() {
    let mut f = Fight::new(settings());
    f.combat.set_hold(true);
    f.here("Raijin, practice dummy")
        .leader_attacks("practice dummy")
        .prompt(80);
    assert!(f.take_sent().is_empty(), "no new fight while held");
    f.combat.set_hold(false);
    f.prompt(80);
    assert_eq!(f.take_sent(), ["lbol practice dummy"]);
    f.combat.set_hold(true);
    f.wait(5).prompt(50);
    assert_eq!(
        f.take_sent(),
        ["mmis practice dummy"],
        "the fight going on goes on"
    );
    let mut f = Fight::new(settings());
    f.combat.set_hold(true);
    f.here("Raijin, practice dummy");
    f.combat.order_kill("practice dummy");
    f.prompt(80);
    assert_eq!(
        f.take_sent(),
        ["lbol practice dummy"],
        "a kill order is the leader's say"
    );
}

#[test]
fn the_attack_in_use_is_known_for_the_web_page() {
    let mut f = Fight::new(settings());
    assert_eq!(f.combat.tier(), None);
    f.here("Raijin, practice dummy")
        .leader_attacks("practice dummy")
        .prompt(80);
    assert_eq!(f.combat.target(), Some("practice dummy"));
    assert_eq!(f.combat.tier().as_deref(), Some("lbol"));
    f.msg(Msg::CombatEngaged).wait(5).prompt(4);
    assert_eq!(f.combat.tier().as_deref(), Some("melee"));
    f.msg(Msg::ExpGain { exp: 10 });
    assert_eq!((f.combat.target(), f.combat.tier()), (None, None));
}

#[test]
fn a_room_shown_with_nobody_else_here_empties_the_roster() {
    let mut f = Fight::new(settings());
    f.here("practice dummy").prompt(80);
    assert_eq!(f.combat.monsters(), ["practice dummy"]);
    // The next room's display has no `Also here:` line before its exits.
    f.msg(Msg::RoomExits {
        exits: s("north, up"),
    })
    .prompt(80);
    assert!(f.combat.monsters().is_empty());
    // An `Also here:` in the same display is the room's list.
    f.here("giant rat")
        .msg(Msg::RoomExits {
            exits: s("north, up"),
        })
        .prompt(80);
    assert_eq!(f.combat.monsters(), ["giant rat"]);
}

/// A warrior bot: melee only, assisting the leader Raijin.
fn warrior() -> CombatSettings {
    let mut set = settings();
    set.tiers = vec![];
    set
}

/// silvermere's answer to an attack command while the player is already fighting
/// (`commands/combat.rs`): the old fight broken off, `*Combat Off*` with its own prompt,
/// then the new one, `*Combat Engaged*`.
fn reengaged(f: &mut Fight) -> &mut Fight {
    f.msg(Msg::CombatOff).prompt(80).msg(Msg::CombatEngaged)
}

// An attack sent while already fighting is answered off, prompt, engaged: the fight goes
// on, so nothing more is sent. Read as a stop, the bot resent at that prompt, and every
// answer asked for the next; two bots, each line of one redrawing the other's prompt, kept
// it up ten times a second (#119).
#[test]
fn the_break_off_an_attack_of_the_bot_s_own_causes_is_no_stop() {
    let mut f = Fight::new(warrior());
    f.here("Raijin, giant rat")
        .leader_attacks("giant rat")
        .prompt(80);
    assert_eq!(f.take_sent(), ["a giant rat"]);
    reengaged(&mut f).prompt(80);
    f.leader_attacks("giant rat").prompt(80).prompt(80);
    assert!(f.take_sent().is_empty());
}

// The user's backscroll (#119): two giant rats, the bot fighting one, the leader's dies.
// The party's experience line can't say which died, so the bot takes up "giant rat" again,
// a fight it's already in. The answer must end it there.
#[test]
fn one_of_two_same_named_monsters_dying_sends_one_attack_at_most() {
    let mut f = Fight::new(warrior());
    f.here("Raijin, giant rat, giant rat")
        .leader_attacks("giant rat")
        .prompt(80);
    f.msg(Msg::CombatEngaged).wait(5);
    f.msg(Msg::ExpGain { exp: 3 }).prompt(80);
    assert_eq!(f.take_sent(), ["a giant rat", "a giant rat"]);
    // The game's answer, then the other bot's and the leader's lines redrawing the prompt.
    reengaged(&mut f).prompt(80);
    for _ in 0..20 {
        f.leader_attacks("giant rat").prompt(80);
    }
    assert!(f.take_sent().is_empty());
}

// A change of attack is a command too, answered the same way: the new one is running.
#[test]
fn a_change_of_attack_answered_off_then_engaged_is_not_sent_again() {
    let mut f = Fight::new(settings());
    f.here("Raijin, practice dummy")
        .leader_attacks("practice dummy")
        .prompt(40);
    assert_eq!(f.take_sent(), ["mmis practice dummy"]);
    f.msg(Msg::CombatEngaged).wait(5).prompt(70);
    assert_eq!(f.take_sent(), ["lbol practice dummy"]);
    f.msg(Msg::CombatOff).prompt(70).msg(Msg::CombatEngaged);
    f.wait(5).prompt(70);
    assert!(f.take_sent().is_empty());
}

// A break-off the bot didn't ask for still ends the attack, though one of its own came
// just before it was answered.
#[test]
fn an_off_once_its_attack_is_answered_ends_the_attack() {
    let mut f = Fight::new(warrior());
    f.here("Raijin, giant rat")
        .leader_attacks("giant rat")
        .prompt(80);
    f.msg(Msg::CombatEngaged).prompt(80);
    f.wait(1).msg(Msg::CombatOff).prompt(80);
    assert_eq!(f.take_sent(), ["a giant rat", "a giant rat"]);
}
