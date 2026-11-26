//! Resting (design §6.6), as pure logic: start below any start threshold, stop when every
//! stop threshold is met, tell the leader `@wait` and `@ok`, never in combat or while
//! poisoned, and rest again after a fight that broke the rest off.

use std::time::{Duration, Instant};

use mp_config::{Amount, Pools};
use mp_engine::{Action, RestSettings, Rester};
use mp_parse::{Event, Item, Rest, Vitals};

fn s(v: &str) -> String {
    v.to_owned()
}

fn settings() -> RestSettings {
    RestSettings {
        start_when_any: Pools {
            hp: Some(Amount::Percent(50)),
            mp: Some(Amount::Percent(30)),
        },
        stop_when_all: Pools {
            hp: Some(Amount::Percent(95)),
            mp: Some(Amount::Points(20)),
        },
        command: s("rest"),
        tell: Some(s("Raijin")),
        retry: Duration::from_secs(5),
        sick_wait: Duration::from_secs(30),
    }
}

fn prompt(hp: i32, mp: i32, rest: Rest) -> Item {
    Item::Prompt(Vitals {
        hp,
        max_hp: Some(100),
        mp: Some(mp),
        max_mp: Some(40),
        rest,
    })
}

fn wait() -> Action {
    Action::Telepath {
        to: s("Raijin"),
        text: s("@wait"),
    }
}

fn ok() -> Action {
    Action::Telepath {
        to: s("Raijin"),
        text: s("@ok"),
    }
}

fn rest() -> Action {
    Action::Send(s("rest"))
}

#[test]
fn above_every_start_threshold_it_does_not_rest() {
    let mut r = Rester::new(settings());
    assert!(
        r.on_item(&prompt(60, 20, Rest::Standing), Instant::now(), false)
            .is_empty()
    );
    assert!(!r.resting());
}

#[test]
fn below_any_start_threshold_it_tells_the_leader_and_rests() {
    let t = Instant::now();
    let mut r = Rester::new(settings());
    assert_eq!(
        r.on_item(&prompt(90, 10, Rest::Standing), t, false),
        vec![wait(), rest()]
    );
    assert!(r.resting());
    // Resting: nothing more is sent while the values climb.
    assert!(
        r.on_item(&prompt(92, 15, Rest::Resting), t, false)
            .is_empty()
    );
}

#[test]
fn it_stops_when_every_stop_threshold_is_met() {
    let t = Instant::now();
    let mut r = Rester::new(settings());
    r.on_item(&prompt(40, 30, Rest::Standing), t, false);
    assert!(
        r.on_item(&prompt(94, 40, Rest::Resting), t, false)
            .is_empty(),
        "HP isn't there yet"
    );
    assert_eq!(
        r.on_item(&prompt(95, 20, Rest::Resting), t, false),
        vec![ok()]
    );
    assert!(!r.resting());
}

#[test]
fn without_tell_leader_it_just_rests() {
    let t = Instant::now();
    let mut r = Rester::new(RestSettings {
        tell: None,
        ..settings()
    });
    assert_eq!(
        r.on_item(&prompt(10, 40, Rest::Standing), t, false),
        vec![rest()]
    );
    assert!(
        r.on_item(&prompt(100, 40, Rest::Resting), t, false)
            .is_empty()
    );
    assert!(!r.resting());
}

#[test]
fn it_does_not_start_while_fighting() {
    let t = Instant::now();
    let mut r = Rester::new(settings());
    assert!(
        r.on_item(&prompt(10, 0, Rest::Standing), t, true)
            .is_empty()
    );
    r.on_item(&Item::Event(Event::CombatEngaged), t, false);
    assert!(
        r.on_item(&prompt(10, 0, Rest::Standing), t, false)
            .is_empty()
    );
    r.on_item(&Item::Event(Event::CombatOff), t, false);
    assert_eq!(
        r.on_item(&prompt(10, 0, Rest::Standing), t, false),
        vec![wait(), rest()]
    );
}

#[test]
fn it_does_not_start_just_after_being_attacked() {
    let t = Instant::now();
    let mut r = Rester::new(settings());
    r.on_item(
        &Item::Event(Event::AttacksYou {
            who: s("practice dummy"),
        }),
        t,
        false,
    );
    assert!(
        r.on_item(&prompt(10, 0, Rest::Standing), t, false)
            .is_empty()
    );
    let later = t + Duration::from_secs(6);
    assert_eq!(
        r.on_item(&prompt(10, 0, Rest::Standing), later, false),
        vec![wait(), rest()]
    );
}

#[test]
fn poison_puts_resting_off() {
    let t = Instant::now();
    let mut r = Rester::new(settings());
    r.on_item(&prompt(10, 0, Rest::Standing), t, false);
    assert!(!r.poisoned(t));
    r.on_item(&Item::Event(Event::TooSick), t, false);
    assert!(r.poisoned(t), "for @status");
    let soon = t + Duration::from_secs(10);
    assert!(
        r.on_item(&prompt(10, 0, Rest::Standing), soon, false)
            .is_empty()
    );
    r.on_item(&Item::Event(Event::PoisonTick), soon, false);
    let still = soon + Duration::from_secs(25);
    assert!(
        r.on_item(&prompt(10, 0, Rest::Standing), still, false)
            .is_empty()
    );
    let well = soon + Duration::from_secs(31);
    assert_eq!(
        r.on_item(&prompt(10, 0, Rest::Standing), well, false),
        vec![rest()],
        "the leader was told already"
    );
}

#[test]
fn a_rest_broken_by_a_fight_is_taken_up_again_afterwards() {
    let t = Instant::now();
    let mut r = Rester::new(settings());
    r.on_item(&prompt(40, 10, Rest::Standing), t, false);
    r.on_item(&prompt(45, 12, Rest::Resting), t, false);
    // Attacked: the game stands the bot up and combat takes over.
    let fight = t + Duration::from_secs(1);
    r.on_item(
        &Item::Event(Event::AttacksYou {
            who: s("practice dummy"),
        }),
        fight,
        false,
    );
    assert!(
        r.on_item(&prompt(30, 10, Rest::Standing), fight, true)
            .is_empty()
    );
    assert!(r.resting(), "still needs its rest; the leader still waits");
    let after = fight + Duration::from_secs(6);
    assert_eq!(
        r.on_item(&prompt(30, 10, Rest::Standing), after, false),
        vec![rest()]
    );
}

#[test]
fn points_and_a_class_without_mana() {
    let t = Instant::now();
    let mut r = Rester::new(RestSettings {
        start_when_any: Pools {
            hp: Some(Amount::Points(20)),
            mp: Some(Amount::Percent(30)),
        },
        stop_when_all: Pools {
            hp: Some(Amount::Points(30)),
            mp: Some(Amount::Percent(100)),
        },
        tell: None,
        ..settings()
    });
    let warrior = |hp| {
        Item::Prompt(Vitals {
            hp,
            max_hp: Some(40),
            mp: None,
            max_mp: None,
            rest: Rest::Standing,
        })
    };
    assert!(
        r.on_item(&warrior(20), t, false).is_empty(),
        "20 isn't below 20"
    );
    assert_eq!(r.on_item(&warrior(19), t, false), vec![rest()]);
    assert!(r.on_item(&warrior(30), t, false).is_empty());
    assert!(!r.resting(), "no mana to wait for");
}

#[test]
fn with_auto_start_off_it_rests_only_when_told() {
    let t = Instant::now();
    let mut r = Rester::new(RestSettings {
        tell: None,
        ..settings()
    });
    r.set_auto_start(false);
    // Below the start thresholds, but a routine decides where to rest.
    assert!(
        r.on_item(&prompt(10, 5, Rest::Standing), t, false)
            .is_empty()
    );
    assert!(!r.resting());
    assert!(r.begin().is_empty());
    assert!(r.resting());
    assert_eq!(
        r.on_item(&prompt(10, 5, Rest::Standing), t, false),
        vec![rest()]
    );
    // It stops at the stop thresholds as ever.
    assert!(
        r.on_item(&prompt(95, 20, Rest::Resting), t, false)
            .is_empty()
    );
    assert!(!r.resting());
}

#[test]
fn begun_at_full_it_ends_at_the_next_prompt() {
    let t = Instant::now();
    let mut r = Rester::new(settings());
    // The leader is told, as when it starts by itself.
    assert_eq!(r.begin(), vec![wait()]);
    assert_eq!(
        r.on_item(&prompt(100, 40, Rest::Standing), t, false),
        vec![ok()]
    );
    assert!(!r.resting());
}
