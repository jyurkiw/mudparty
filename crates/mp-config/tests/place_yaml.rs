//! Places as the files write them (#66, design §14.3): a name (an alias, a slug, the game's
//! name or a key), or the long form `{ room: 1/2150, name: "Newhaven, Arena" }` that names
//! the room by number and keeps its name for `check` to compare.

use std::fs;

use mp_config::{CharacterConfig, Place, RoutineRef, RuleSpec, load_routine};

fn routine(text: &str) -> Result<mp_config::Routine, mp_config::ConfigError> {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("r.yaml"), text).unwrap();
    load_routine(
        dir.path(),
        "Ermias",
        &RoutineRef::Named {
            name: "r".into(),
            with: Default::default(),
        },
    )
}

#[test]
fn a_place_is_a_name_or_a_room_by_number() {
    let r = routine(
        "do:\n  - rest: { at: road, when: { hp_below: 50% } }\n  - camp: { at: { room: 1/2150, name: \"Newhaven, Arena\" } }\n",
    )
    .unwrap();
    let RuleSpec::Rest(rest) = &r.rules[0] else {
        panic!("{:?}", r.rules[0])
    };
    assert_eq!(rest.at, Place::Named("road".into()));
    let RuleSpec::Camp(camp) = &r.rules[1] else {
        panic!("{:?}", r.rules[1])
    };
    assert_eq!(
        camp.at,
        Place::Room {
            map: 1,
            room: 2150,
            name: Some("Newhaven, Arena".into()),
        }
    );
    assert_eq!(camp.at.to_string(), "1/2150 (Newhaven, Arena)");
    assert_eq!(rest.at.to_string(), "road");
}

#[test]
fn the_long_form_needs_a_room_number() {
    let e = routine("do:\n  - camp: { at: { room: arena } }\n").unwrap_err();
    assert!(e.to_string().contains("1/2150"), "{e}");
    let e = routine("do:\n  - camp: { at: { room: 1/2150, nmae: x } }\n").unwrap_err();
    assert!(e.to_string().contains("nmae"), "{e}");
}

#[test]
fn gear_and_avoided_rooms_take_either_form() {
    let c = CharacterConfig::from_yaml(
        "schema: 1\nname: Ermias\ngear:\n  - { item: club, use: wield, at: { room: 1/2141 } }\n",
    )
    .unwrap();
    assert_eq!(
        c.gear[0].at,
        Some(Place::Room {
            map: 1,
            room: 2141,
            name: None,
        })
    );
    let r = routine("avoid:\n  rooms: [healer, { room: 1/9 }]\ndo:\n  - camp: { at: arena }\n")
        .unwrap();
    assert_eq!(r.avoid.rooms[0], Place::Named("healer".into()));
    assert_eq!(
        r.avoid.rooms[1],
        Place::Room {
            map: 1,
            room: 9,
            name: None,
        }
    );
}

// ----- written back (what `check` prints)

const ERMIAS: &str = r#"
schema: 1
name: Ermias
class: Warrior
login:
  account: Ermias
  steps:
    - { wait: "name:", send: "{account}" }
combat:
  targeting: [kill_order, { priority: ["cave bear"] }, fight_back]
  tiers: [{ spell: mmis, min_mp: 10 }]
  melee: a
rest:
  start_when_any: { hp: 40%, mp: 10 }
unattended:
  hang_up_below: none
gear:
  - { item: club, use: wield, at: { room: 1/2141, name: "Newhaven, Weapons Shop" } }
routine:
  use: newhaven-arena
  with: { rest_below: 60% }
"#;

const ARENA: &str = r#"
schema: 1
name: newhaven-arena
places: newhaven
avoid:
  rooms: [healer]
  exits: [trap]
prefer_not:
  narrow-path: 5
do:
  - gear
  - rest: { at: road, when: { hp_below: 50% }, until: { hp: 90% } }
  - hunt: { monster: cave bear, at: { room: 1/2156 }, every: 4m, when: { hp_at_least: 80% } }
  - camp: { at: arena, look_every: 1500ms }
"#;

#[test]
fn a_character_written_back_reads_the_same() {
    let c = CharacterConfig::from_yaml(ERMIAS).unwrap();
    let text = c.to_yaml();
    assert_eq!(CharacterConfig::from_yaml(&text).unwrap(), c, "{text}");
    // Defaults are written out.
    assert!(text.contains("statline:"), "{text}");
    assert!(text.contains("stop_when_all:"), "{text}");
    assert!(text.contains("hang_up_below: none"), "{text}");
}

#[test]
fn a_routine_written_back_reads_the_same() {
    let r = routine(ARENA).unwrap();
    let text = r.to_yaml();
    assert_eq!(routine(&text).unwrap(), r, "{text}");
    assert!(text.contains("look_every: 1500ms"), "{text}");
    assert!(text.contains("every: 4m"), "{text}");
    // Written inline in a character's file, it reads the same too.
    let c = CharacterConfig::from_yaml(&format!(
        "schema: 1\nname: Ermias\nroutine:\n{}",
        text.lines().map(|l| format!("  {l}\n")).collect::<String>()
    ))
    .unwrap();
    assert_eq!(
        load_routine(
            std::path::Path::new("."),
            "Ermias",
            c.routine.as_ref().unwrap()
        )
        .unwrap(),
        r
    );
}
