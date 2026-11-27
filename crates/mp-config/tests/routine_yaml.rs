//! Routines as rule lists in their own files (#65, design §14.6): `routines/<name>.yaml`,
//! read top-down, named from a character's file with parameters of its own, or written
//! inline.

use std::fs;
use std::time::Duration;

use mp_config::{Amount, CharacterConfig, Pick, RoutineRef, RuleSpec, load_routine};

const ARENA: &str = r#"
schema: 1
name: newhaven-arena
places: newhaven
params:
  rest_below: 50%
  bear_hp: 80%
avoid:
  rooms: [healer]
  exits: [trap]
prefer_not:
  narrow-path: 5
do:
  - rest:
      at: road
      when: { hp_below: "${rest_below}" }
      until: { hp: 100% }
  - circuit:
      lairs: [dungeon:small-cavern-2156]
      respawn:
        dungeon:small-cavern-2156: 4m
      wait: 30s
      when: { hp_at_least: "${bear_hp}" }
  - camp:
      at: arena
      look_every: 4s
"#;

/// A data root's `routines/` folder holding `file` as `newhaven-arena.yaml`.
fn routines(file: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("newhaven-arena.yaml"), file).unwrap();
    dir
}

fn character(routine: &str) -> CharacterConfig {
    CharacterConfig::from_yaml(&format!(
        "schema: 1\nname: Ermias\nlogin:\n  account: Ermias\n{routine}"
    ))
    .unwrap()
}

#[test]
fn a_character_names_its_routine() {
    let c = character("routine: newhaven-arena\n");
    assert_eq!(
        c.routine,
        Some(RoutineRef::Named {
            name: "newhaven-arena".into(),
            with: Default::default(),
        })
    );
}

#[test]
fn a_routine_file_reads_top_down_with_its_params_filled_in() {
    let dir = routines(ARENA);
    let c = character("routine: newhaven-arena\n");
    let r = load_routine(dir.path(), "Ermias", c.routine.as_ref().unwrap()).unwrap();
    assert_eq!(r.name, "newhaven-arena");
    assert_eq!(r.places.as_deref(), Some("newhaven"));
    assert_eq!(r.avoid.rooms, ["healer"]);
    assert_eq!(r.prefer_not.get("narrow-path"), Some(&5));
    assert_eq!(r.rules.len(), 3);
    let RuleSpec::Rest(rest) = &r.rules[0] else {
        panic!("{:?}", r.rules[0])
    };
    assert_eq!(rest.at, "road");
    assert_eq!(rest.when.hp_below, Some(Amount::Percent(50)));
    assert_eq!(rest.until.and_then(|u| u.hp), Some(Amount::Percent(100)));
    let RuleSpec::Circuit(circuit) = &r.rules[1] else {
        panic!("{:?}", r.rules[1])
    };
    assert_eq!(circuit.lairs, ["dungeon:small-cavern-2156"]);
    assert_eq!(
        circuit.respawn.get("dungeon:small-cavern-2156"),
        Some(&Duration::from_secs(240))
    );
    assert_eq!(circuit.wait, Duration::from_secs(30));
    assert_eq!(circuit.pick, Pick::Soonest);
    assert_eq!(circuit.when.hp_at_least, Some(Amount::Percent(80)));
    let RuleSpec::Camp(camp) = &r.rules[2] else {
        panic!("{:?}", r.rules[2])
    };
    assert_eq!(camp.at, "arena");
    assert_eq!(camp.look_every, Duration::from_secs(4));
}

#[test]
fn a_character_may_change_a_routines_params() {
    let dir = routines(ARENA);
    let c = character("routine:\n  use: newhaven-arena\n  with: { rest_below: 60% }\n");
    let r = load_routine(dir.path(), "Ermias", c.routine.as_ref().unwrap()).unwrap();
    let RuleSpec::Rest(rest) = &r.rules[0] else {
        panic!()
    };
    assert_eq!(rest.when.hp_below, Some(Amount::Percent(60)));
}

#[test]
fn a_param_the_routine_doesnt_have_is_refused() {
    let dir = routines(ARENA);
    let c = character("routine:\n  use: newhaven-arena\n  with: { flee_below: 10% }\n");
    let e = load_routine(dir.path(), "Ermias", c.routine.as_ref().unwrap())
        .unwrap_err()
        .to_string();
    assert!(e.contains("flee_below"), "{e}");
}

#[test]
fn a_param_used_but_not_declared_is_refused() {
    let dir = routines(&ARENA.replace("\"${bear_hp}\"", "\"${bear_health}\""));
    let c = character("routine: newhaven-arena\n");
    let e = load_routine(dir.path(), "Ermias", c.routine.as_ref().unwrap())
        .unwrap_err()
        .to_string();
    assert!(e.contains("bear_health"), "{e}");
}

#[test]
fn a_routine_file_that_isnt_there_is_named() {
    let dir = tempfile::tempdir().unwrap();
    let c = character("routine: newhaven-arena\n");
    let e = load_routine(dir.path(), "Ermias", c.routine.as_ref().unwrap())
        .unwrap_err()
        .to_string();
    assert!(e.contains("newhaven-arena"), "{e}");
}

#[test]
fn a_routine_may_be_written_inline() {
    let c = character(
        "routine:\n  do:\n    - rest: { at: 1/2146, when: { hp_below: 50% } }\n    - camp: { at: 1/2150 }\n",
    );
    let dir = tempfile::tempdir().unwrap();
    let r = load_routine(dir.path(), "Ermias", c.routine.as_ref().unwrap()).unwrap();
    assert_eq!(r.name, "Ermias");
    assert_eq!(r.rules.len(), 2);
    let RuleSpec::Camp(camp) = &r.rules[1] else {
        panic!()
    };
    assert_eq!(camp.look_every, Duration::from_secs(5));
}

#[test]
fn an_inline_routine_is_checked_with_the_character() {
    let e = CharacterConfig::from_yaml(
        "schema: 1\nname: Ermias\nroutine:\n  do:\n    - rest: { at: road }\n",
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("routine.do[0]"), "{e}");
}

fn error_in(file: &str) -> String {
    let dir = routines(file);
    let c = character("routine: newhaven-arena\n");
    load_routine(dir.path(), "Ermias", c.routine.as_ref().unwrap())
        .unwrap_err()
        .to_string()
}

#[test]
fn a_rule_after_one_that_always_applies_would_never_run() {
    let e = error_in(
        "schema: 1\nname: x\ndo:\n  - camp: { at: arena }\n  - rest: { at: road, when: { hp_below: 50% } }\n",
    );
    assert!(e.contains("do[1]"), "{e}");
    assert!(e.contains("never"), "{e}");
}

#[test]
fn a_rest_rule_says_when() {
    let e = error_in("schema: 1\nname: x\ndo:\n  - rest: { at: road }\n  - camp: { at: arena }\n");
    assert!(e.contains("do[0]"), "{e}");
    assert!(e.contains("when"), "{e}");
}

#[test]
fn an_unknown_rule_or_condition_is_named() {
    let e = error_in("schema: 1\nname: x\ndo:\n  - juggle: { at: road }\n");
    assert!(e.contains("juggle"), "{e}");
    let e =
        error_in("schema: 1\nname: x\ndo:\n  - rest: { at: road, when: { feeling_low: true } }\n");
    assert!(e.contains("feeling_low"), "{e}");
}

#[test]
fn a_circuit_needs_lairs_and_times_for_them() {
    let e = error_in(&ARENA.replace("lairs: [dungeon:small-cavern-2156]", "lairs: []"));
    assert!(e.contains("do[1].lairs"), "{e}");
    let e = error_in(&ARENA.replace("small-cavern-2156: 4m", "small-cavern-2156: 0s"));
    assert!(e.contains("do[1].respawn"), "{e}");
    // A respawn time for a room that isn't one of its lairs is a slip.
    let e = error_in(&ARENA.replace(
        "        dungeon:small-cavern-2156: 4m",
        "        dungeon:small-cavern-2156: 4m\n        arena: 2m",
    ));
    assert!(e.contains("do[1].respawn.arena"), "{e}");
    assert!(e.contains("isn't one of the circuit's lairs"), "{e}");
}

#[test]
fn a_circuit_may_count_waiting_as_free() {
    let dir = routines(&ARENA.replace("wait: 30s", "wait: 30s\n      pick: throughput"));
    let c = character("routine: newhaven-arena\n");
    let r = load_routine(dir.path(), "Ermias", c.routine.as_ref().unwrap()).unwrap();
    let RuleSpec::Circuit(circuit) = &r.rules[1] else {
        panic!()
    };
    assert_eq!(circuit.pick, Pick::Throughput);
}

#[test]
fn a_hunt_is_now_a_circuit_of_one_lair() {
    let e = error_in(
        "schema: 1\nname: x\ndo:\n  - hunt: { monster: cave bear, at: cavern, every: 4m }\n",
    );
    assert!(e.contains("circuit"), "{e}");
}

#[test]
fn an_exit_kind_must_be_one_the_game_has() {
    let e = error_in(&ARENA.replace("exits: [trap]", "exits: [trapdoor]"));
    assert!(e.contains("avoid.exits[0]"), "{e}");
}

#[test]
fn targeting_rules_may_be_bare_words() {
    let c = CharacterConfig::from_yaml(
        "schema: 1\nname: Ermias\ncombat:\n  targeting: [kill_order, fight_back, { priority: [\"*\"] }]\n",
    )
    .unwrap();
    assert_eq!(c.combat.targeting.len(), 3);
}
