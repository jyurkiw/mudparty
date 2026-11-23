//! The pattern table is data (`src/patterns.toml`): how an entry is read and turned into an
//! [`Event`].

use mp_parse::{Event, Patterns, pattern_keys};

#[test]
fn the_built_in_table_loads_and_names_its_keys() {
    let p = Patterns::builtin();
    assert_eq!(p.keys(), pattern_keys());
    assert!(p.keys().contains(&"combat_off"));
}

#[test]
fn an_entry_with_no_fields_builds_its_event() {
    let p = Patterns::from_toml(
        r#"
        [[pattern]]
        event = "CombatOff"
        keys = ["combat_off"]
        re = '^\*Combat Off\*$'
        "#,
    )
    .unwrap();
    assert_eq!(p.classify("*Combat Off*"), Some(Event::CombatOff));
    assert_eq!(p.classify("Combat Off"), None);
}

#[test]
fn named_groups_fill_the_events_fields() {
    let p = Patterns::from_toml(
        r#"
        [[pattern]]
        event = "MovesToAttack"
        keys = ["combat_moves_to_attack_mon"]
        re = '^(?P<who>\S+) moves to attack (?P<target>.+)\.$'
        "#,
    )
    .unwrap();
    assert_eq!(
        p.classify("Raijin moves to attack small kobold."),
        Some(Event::MovesToAttack {
            who: "Raijin".into(),
            target: "small kobold".into()
        })
    );
}

#[test]
fn numbers_are_read_as_numbers() {
    let p = Patterns::from_toml(
        r#"
        [[pattern]]
        event = "ExpGain"
        keys = ["exp_gain"]
        re = '^You gain (?P<exp>\d+) experience\.$'
        numbers = ["exp"]
        "#,
    )
    .unwrap();
    assert_eq!(
        p.classify("You gain 1250 experience."),
        Some(Event::ExpGain { exp: 1250 })
    );
}

#[test]
fn a_split_group_becomes_a_list() {
    let p = Patterns::from_toml(
        r#"
        [[pattern]]
        event = "AlsoHere"
        keys = ["room_also_here"]
        re = '^Also here: (?P<names>.+)\.$'
        split = { names = ", " }
        "#,
    )
    .unwrap();
    assert_eq!(
        p.classify("Also here: Raijin, giant rat."),
        Some(Event::AlsoHere {
            names: vec!["Raijin".into(), "giant rat".into()]
        })
    );
}

#[test]
fn the_first_matching_entry_wins() {
    let p = Patterns::from_toml(
        r#"
        [[pattern]]
        event = "AttacksRoom"
        keys = ["cast_attacks_room"]
        re = '^(?P<who>\S+) moves to attack everyone in the room\.$'

        [[pattern]]
        event = "MovesToAttack"
        keys = ["cast_moves_to_attack"]
        re = '^(?P<who>\S+) moves to attack (?P<target>.+)\.$'
        "#,
    )
    .unwrap();
    assert_eq!(
        p.classify("Ermias moves to attack everyone in the room."),
        Some(Event::AttacksRoom {
            who: "Ermias".into()
        })
    );
}

#[test]
fn a_bad_regex_is_refused_at_load_naming_the_entry() {
    let err = Patterns::from_toml(
        r#"
        [[pattern]]
        event = "CombatOff"
        keys = ["combat_off"]
        re = '^(unclosed$'
        "#,
    )
    .unwrap_err();
    assert!(err.to_string().contains("CombatOff"), "{err}");
}

#[test]
fn an_entry_missing_its_regex_is_refused() {
    assert!(
        Patterns::from_toml("[[pattern]]\nevent = \"CombatOff\"\nkeys = [\"combat_off\"]\n")
            .is_err()
    );
}

#[test]
fn an_entry_with_no_keys_is_refused() {
    assert!(
        Patterns::from_toml("[[pattern]]\nevent = \"CombatOff\"\nkeys = []\nre = 'x'\n").is_err()
    );
}
