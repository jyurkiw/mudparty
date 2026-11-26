//! A character's `places:` and `routine:` (#53): the solo loop of camp, retreat, rest,
//! return and timed hunts, with rooms named once and used by name.

use std::time::Duration;

use mp_config::{Amount, CharacterConfig};

const NEWHAVEN: &str = r#"
schema: 1
name: Ermias
login:
  account: Ermias
rest:
  start_when_any: { hp: 50% }
  stop_when_all: { hp: 100% }
places:
  arena: 1/2150
  road: 1/2146
  cavern: 1/2156
routine:
  camp: arena
  rest_at: road
  look_every: 4s
  hunts:
    - monster: cave bear
      at: cavern
      every: 4m
      min_hp: 80%
"#;

#[test]
fn a_routine_reads_with_its_places_resolved() {
    let c = CharacterConfig::from_yaml(NEWHAVEN).unwrap();
    assert_eq!(c.place("arena"), Some((1, 2150)));
    // A room may be written straight out, too.
    assert_eq!(c.place("1/2141"), Some((1, 2141)));
    assert_eq!(c.place("nowhere"), None);
    let r = c.routine.as_ref().unwrap();
    assert_eq!(r.camp, "arena");
    assert_eq!(r.rest_at, "road");
    assert_eq!(r.look_every, Duration::from_secs(4));
    assert_eq!(r.hunts.len(), 1);
    let h = &r.hunts[0];
    assert_eq!(h.monster, "cave bear");
    assert_eq!(c.place(&h.at), Some((1, 2156)));
    assert_eq!(h.every, Duration::from_secs(240));
    assert_eq!(h.min_hp, Some(Amount::Percent(80)));
}

#[test]
fn a_file_without_a_routine_has_none() {
    let c = CharacterConfig::from_yaml("schema: 1\nname: Ermias\n").unwrap();
    assert!(c.routine.is_none());
    assert!(c.places.is_empty());
}

#[test]
fn the_defaults_fill_in() {
    let c = CharacterConfig::from_yaml(
        "schema: 1\nname: Ermias\nrest:\n  start_when_any: { hp: 50% }\nroutine:\n  camp: 1/2150\n  rest_at: 1/2146\n",
    )
    .unwrap();
    let r = c.routine.unwrap();
    assert_eq!(r.look_every, Duration::from_secs(5));
    assert!(r.hunts.is_empty());
}

fn error(yaml: &str) -> String {
    CharacterConfig::from_yaml(yaml).unwrap_err().to_string()
}

#[test]
fn an_unknown_place_is_named() {
    let e = error(&NEWHAVEN.replace("rest_at: road", "rest_at: tavern"));
    assert!(e.contains("routine.rest_at"), "{e}");
    assert!(e.contains("tavern"), "{e}");
    let e = error(&NEWHAVEN.replace("at: cavern", "at: lair"));
    assert!(e.contains("routine.hunts[0].at"), "{e}");
}

#[test]
fn a_place_must_be_a_room() {
    let e = error(&NEWHAVEN.replace("arena: 1/2150", "arena: the arena"));
    assert!(e.contains("places.arena"), "{e}");
}

#[test]
fn a_routine_retreats_by_the_rest_thresholds() {
    // Without a line to fall below, the bot would never leave the camp to rest.
    let e = error(&NEWHAVEN.replace("start_when_any: { hp: 50% }", "command: rest"));
    assert!(e.contains("rest.start_when_any"), "{e}");
}

#[test]
fn a_hunt_needs_a_monster_and_a_time() {
    let e = error(&NEWHAVEN.replace("monster: cave bear", "monster: \"\""));
    assert!(e.contains("routine.hunts[0].monster"), "{e}");
    let e = error(&NEWHAVEN.replace("every: 4m", "every: 0s"));
    assert!(e.contains("routine.hunts[0].every"), "{e}");
    let e = error(&NEWHAVEN.replace("min_hp: 80%", "min_hp: 180%"));
    assert!(e.contains("routine.hunts[0].min_hp"), "{e}");
}
