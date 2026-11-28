//! Healing spells planned by level (#126): `heals` in a `combat.by_level` block, each cast
//! by whom it's for, the target's HP and the healer's own MP.

use mp_config::{Amount, CharacterConfig, Span, Whom};

const CLERIC: &str = r#"
schema: 1
name: Ermias
combat:
  melee: a
  by_level:
    - levels: 8-
      heals:
        - cast: mrai
          when: { hp_below: 70%, allies: 2-, min_mp: 30% }
        - cast: mahe
          on: [self, party]
          when: { hp_below: 50%, min_mp: 10 }
        - cast: mihe
          on: [Raijin]
          when: { hp_below: 80%, enemies: 0, routine: [arena] }
      spells:
        - cast: mmis
"#;

fn err(yaml: &str) -> String {
    CharacterConfig::from_yaml(yaml).unwrap_err().to_string()
}

#[test]
fn heals_read_with_whom_they_are_for() {
    let c = CharacterConfig::from_yaml(CLERIC).unwrap();
    let h = &c.combat.by_level[0].heals;
    assert_eq!(h.len(), 3);
    assert_eq!(h[0].cast, "mrai");
    assert_eq!(h[0].when.hp_below, Amount::Percent(70));
    assert_eq!(h[0].when.allies, Some(Span::new(Some(2), None)));
    assert_eq!(h[0].when.min_mp, Some(Amount::Percent(30)));
    assert_eq!(h[1].on, [Whom::Me, Whom::Party]);
    assert_eq!(h[1].when.min_mp, Some(Amount::Points(10)));
    assert_eq!(h[2].on, [Whom::Named("Raijin".into())]);
    assert_eq!(h[2].when.enemies, Some(Span::new(Some(0), Some(0))));
    assert_eq!(h[2].when.routine, ["arena"]);
    assert_eq!(c.combat.by_level[0].spells[0].cast, "mmis");
}

#[test]
fn a_heal_is_for_the_healer_unless_told() {
    let c = CharacterConfig::from_yaml(CLERIC).unwrap();
    assert_eq!(c.combat.by_level[0].heals[0].on, [Whom::Me]);
}

#[test]
fn a_file_with_heals_writes_out_and_reads_back_the_same() {
    let c = CharacterConfig::from_yaml(CLERIC).unwrap();
    let again = CharacterConfig::from_yaml(&c.to_yaml()).unwrap();
    assert_eq!(again, c);
}

#[test]
fn a_heal_needs_its_hp_line() {
    let e = err(&CLERIC.replace("hp_below: 50%, min_mp: 10", "min_mp: 10"));
    assert!(e.contains("hp_below"), "{e}");
}

#[test]
fn a_heal_for_nobody_is_refused() {
    let e = err(&CLERIC.replace("on: [self, party]", "on: []"));
    assert!(e.contains("combat.by_level[0].heals[1].on"), "{e}");
}

#[test]
fn a_heal_with_no_spell_is_refused() {
    let e = err(&CLERIC.replace("- cast: mahe", "- cast: \"\""));
    assert!(e.contains("combat.by_level[0].heals[1].cast"), "{e}");
}

#[test]
fn an_hp_line_over_100_percent_is_refused() {
    let e = err(&CLERIC.replace("hp_below: 80%", "hp_below: 180%"));
    assert!(
        e.contains("combat.by_level[0].heals[2].when.hp_below"),
        "{e}"
    );
}

#[test]
fn a_backwards_allies_range_is_refused() {
    let e = err(&CLERIC.replace("allies: 2-", "allies: 4-1"));
    assert!(e.contains("combat.by_level[0].heals[0].when.allies"), "{e}");
}
