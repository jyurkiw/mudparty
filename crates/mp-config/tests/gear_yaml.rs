//! A character's `gear:` (#55, design §14.7): what it carries, and whether each item is
//! wielded or worn. A routine's `gear` rule keeps it so, buying what's missing at a shop
//! that sells it, or at the shop `at:` names.

use std::fs;

use mp_config::{CharacterConfig, GearUse, RoutineRef, RuleSpec, load_routine};

const ERMIAS: &str = r#"
schema: 1
name: Ermias
login:
  account: Ermias
gear:
  - item: club
    use: wield
  - { item: padded vest, use: wear, at: armour-shop }
"#;

#[test]
fn gear_reads_with_an_optional_shop() {
    let c = CharacterConfig::from_yaml(ERMIAS).unwrap();
    assert_eq!(c.gear.len(), 2);
    assert_eq!(c.gear[0].item, "club");
    assert_eq!(c.gear[0].how, GearUse::Wield);
    assert_eq!(c.gear[0].at, None);
    assert_eq!(c.gear[1].how, GearUse::Wear);
    assert_eq!(c.gear[1].at.as_deref(), Some("armour-shop"));
}

#[test]
fn no_gear_is_none() {
    let c = CharacterConfig::from_yaml("schema: 1\nname: Ermias\n").unwrap();
    assert!(c.gear.is_empty());
}

#[test]
fn an_item_needs_a_name_and_a_use() {
    let e = CharacterConfig::from_yaml(&ERMIAS.replace("item: club", "item: \"\""))
        .unwrap_err()
        .to_string();
    assert!(e.contains("gear[0].item"), "{e}");
    let e = CharacterConfig::from_yaml(&ERMIAS.replace("use: wield", "use: juggle"))
        .unwrap_err()
        .to_string();
    assert!(e.contains("juggle") || e.contains("use"), "{e}");
}

#[test]
fn a_routine_keeps_the_gear_with_a_gear_rule() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("arena.yaml"),
        "schema: 1\nname: arena\ndo:\n  - gear\n  - gear: { when: { hp_at_least: 50% } }\n  - camp: { at: arena }\n",
    )
    .unwrap();
    let r = load_routine(
        dir.path(),
        "Ermias",
        &RoutineRef::Named {
            name: "arena".into(),
            with: Default::default(),
        },
    )
    .unwrap();
    // A rule with nothing to say may be a bare word.
    assert!(matches!(&r.rules[0], RuleSpec::Gear(g) if g.when == Default::default()));
    assert!(matches!(&r.rules[1], RuleSpec::Gear(g) if g.when.hp_at_least.is_some()));
}
