//! Attack spells planned by level (#125): `combat.by_level` blocks, each a level range and
//! its spell rules, with their conditions and cast limits.

use mp_config::{Amount, CharacterConfig, Span};

const MAGE: &str = r#"
schema: 1
name: Ermias
combat:
  melee: a
  by_level:
    - levels: 1-7
      spells:
        - cast: mmis
    - levels: 8-
      spells:
        - cast: fbal
          when: { enemies: 3-, min_mp: 40% }
        - cast: lbol
          when: { min_mp: 25, routine: [kobold-loop], monsters: ["*kobold*"] }
          max_casts: 4
        - cast: mmis
"#;

fn err(yaml: &str) -> String {
    CharacterConfig::from_yaml(yaml).unwrap_err().to_string()
}

#[test]
fn level_blocks_read_with_their_rules() {
    let c = CharacterConfig::from_yaml(MAGE).unwrap();
    let b = &c.combat.by_level;
    assert_eq!(b.len(), 2);
    assert_eq!(b[0].levels, Span::new(Some(1), Some(7)));
    assert_eq!(b[1].levels, Span::new(Some(8), None));
    assert_eq!(b[0].spells[0].cast, "mmis");
    assert_eq!(b[0].spells[0].max_casts, None);
    let fbal = &b[1].spells[0];
    assert_eq!(fbal.when.enemies, Some(Span::new(Some(3), None)));
    assert_eq!(fbal.when.min_mp, Some(Amount::Percent(40)));
    let lbol = &b[1].spells[1];
    assert_eq!(lbol.when.min_mp, Some(Amount::Points(25)));
    assert_eq!(lbol.when.routine, ["kobold-loop"]);
    assert_eq!(lbol.when.monsters, ["*kobold*"]);
    assert_eq!(lbol.max_casts, Some(4));
}

#[test]
fn a_span_is_a_range_an_open_end_or_one_number() {
    for (text, want) in [
        ("3-5", Span::new(Some(3), Some(5))),
        ("8-", Span::new(Some(8), None)),
        ("-7", Span::new(None, Some(7))),
        ("3+", Span::new(Some(3), None)),
        ("12", Span::new(Some(12), Some(12))),
    ] {
        let yaml = MAGE.replace("levels: 1-7", &format!("levels: \"{text}\""));
        let c = CharacterConfig::from_yaml(&yaml).unwrap();
        assert_eq!(c.combat.by_level[0].levels, want, "{text}");
    }
    // A bare number, unquoted, is one level too.
    let c = CharacterConfig::from_yaml(&MAGE.replace("levels: 1-7", "levels: 4")).unwrap();
    assert_eq!(c.combat.by_level[0].levels, Span::new(Some(4), Some(4)));
}

#[test]
fn a_span_holds_its_ends() {
    let s = Span::new(Some(3), Some(5));
    assert!(!s.holds(2) && s.holds(3) && s.holds(5) && !s.holds(6));
    assert!(Span::new(Some(8), None).holds(50));
    assert!(Span::new(None, Some(7)).holds(1));
}

#[test]
fn a_file_with_level_blocks_writes_out_and_reads_back_the_same() {
    let c = CharacterConfig::from_yaml(MAGE).unwrap();
    let again = CharacterConfig::from_yaml(&c.to_yaml()).unwrap();
    assert_eq!(again, c);
}

#[test]
fn a_backwards_span_is_refused_naming_its_field() {
    let e = err(&MAGE.replace("levels: 1-7", "levels: 7-1"));
    assert!(e.contains("combat.by_level[0].levels"), "{e}");
    let e = err(&MAGE.replace("enemies: 3-", "enemies: 5-2"));
    assert!(
        e.contains("combat.by_level[1].spells[0].when.enemies"),
        "{e}"
    );
}

#[test]
fn a_span_that_isn_t_one_is_refused() {
    let e = err(&MAGE.replace("levels: 1-7", "levels: lots"));
    assert!(e.contains("lots"), "{e}");
}

#[test]
fn a_rule_with_no_spell_is_refused() {
    let e = err(&MAGE.replace("- cast: mmis\n    - levels", "- cast: \"\"\n    - levels"));
    assert!(e.contains("combat.by_level[0].spells[0].cast"), "{e}");
}

#[test]
fn a_cast_limit_of_none_is_refused() {
    let e = err(&MAGE.replace("max_casts: 4", "max_casts: 0"));
    assert!(e.contains("combat.by_level[1].spells[1].max_casts"), "{e}");
}

#[test]
fn a_percentage_of_mp_over_100_is_refused() {
    let e = err(&MAGE.replace("min_mp: 40%", "min_mp: 140%"));
    assert!(
        e.contains("combat.by_level[1].spells[0].when.min_mp"),
        "{e}"
    );
}

#[test]
fn an_unknown_condition_is_refused() {
    let e = err(&MAGE.replace("enemies: 3-", "foes: 3-"));
    assert!(e.contains("foes"), "{e}");
}
