//! A character's YAML (design §7.3, with the combat and rest sections of §6.4 and §6.6).

use mp_config::{Amount, CharacterConfig, LoginSteps, Melee, PartyRole, Require, Rule};

const ERMIAS: &str = r#"
schema: 1
name: Ermias
class: Assassin
autostart: false
login:
  account: ermias
  steps: default-silvermere
  takeover: true
statline: "[HP=%h/%H,MA=%m/%M]:%r"
party:
  leader: Raijin
  invite_by_injection: true
remote:
  allow: [Raijin]
  channels: [telepath, gangpath]
  warn_denied: false
automation: { combat: true, nuke: true, rest: true }
combat:
  targeting:
    - kill_order: true
    - priority: ["*shaman*", "*mage*", "acolyte"]
    - assist_leader: true
    - fight_back: true
  tiers:
    - spell: lbol
      min_mp: 60
    - spell: mmis
      min_mp: 5
  room_tiers:
    - spell: fbal
      min_mp: 40
      min_enemies: 3
  melee: a
  stealth_opener:
    spell: mmis
    require: sneaking
rest:
  start_when_any: { hp: 50%, mp: 30% }
  stop_when_all:  { hp: 95%, mp: 100% }
  command: rest
  tell_leader: true
"#;

/// The main character's smaller file.
const RAIJIN: &str = r#"
schema: 1
name: Raijin
statline: "[HP=%h/MA=%m]:%r"
party:
  role: leader
"#;

#[test]
fn a_bots_file_reads_whole() {
    let c = CharacterConfig::from_yaml(ERMIAS).unwrap();
    assert_eq!(c.name, "Ermias");
    assert_eq!(c.class.as_deref(), Some("Assassin"));
    assert!(!c.autostart);
    let login = c.login.as_ref().unwrap();
    assert_eq!(login.account, "ermias");
    assert_eq!(login.steps, LoginSteps::DefaultSilvermere);
    assert!(login.takeover);
    assert_eq!(c.party.leader.as_deref(), Some("Raijin"));
    assert!(c.party.invite_by_injection);
    assert_eq!(c.party.role, PartyRole::Member);
    assert_eq!(c.remote.allow, ["Raijin"]);
    assert!(c.automation.combat && c.automation.nuke && c.automation.rest);
    assert_eq!(
        c.combat.targeting,
        [
            Rule::KillOrder(true),
            Rule::Priority(vec!["*shaman*".into(), "*mage*".into(), "acolyte".into()]),
            Rule::AssistLeader(true),
            Rule::FightBack(true),
        ]
    );
    assert_eq!(c.combat.tiers.len(), 2);
    assert_eq!(
        (c.combat.tiers[0].spell.as_str(), c.combat.tiers[0].min_mp),
        ("lbol", 60)
    );
    assert_eq!(c.combat.room_tiers[0].min_enemies, 3);
    assert_eq!(c.combat.melee, Melee::Command("a".into()));
    let opener = c.combat.stealth_opener.as_ref().unwrap();
    assert_eq!(
        (opener.spell.as_str(), opener.require),
        ("mmis", Require::Sneaking)
    );
    assert_eq!(c.rest.start_when_any.hp, Some(Amount::Percent(50)));
    assert_eq!(c.rest.stop_when_all.mp, Some(Amount::Percent(100)));
    assert_eq!(c.rest.command, "rest");
    assert!(c.rest.tell_leader);
}

#[test]
fn the_main_characters_short_file_reads_with_defaults() {
    let c = CharacterConfig::from_yaml(RAIJIN).unwrap();
    assert_eq!(c.name, "Raijin");
    assert_eq!(c.party.role, PartyRole::Leader);
    assert!(c.login.is_none());
    assert!(c.combat.tiers.is_empty());
    assert_eq!(c.combat.melee, Melee::None);
    assert_eq!(c.rest.command, "rest");
}

#[test]
fn a_bots_statline_defaults_to_the_bot_template() {
    let c = CharacterConfig::from_yaml("schema: 1\nname: Ermias\n").unwrap();
    assert_eq!(c.statline, "[HP=%h/%H,MA=%m/%M]:%r");
}

#[test]
fn thresholds_may_be_points_as_well_as_percentages() {
    let yaml = ERMIAS.replace(
        "start_when_any: { hp: 50%, mp: 30% }",
        "start_when_any: { hp: 120 }",
    );
    let c = CharacterConfig::from_yaml(&yaml).unwrap();
    assert_eq!(c.rest.start_when_any.hp, Some(Amount::Points(120)));
    assert_eq!(c.rest.start_when_any.mp, None);
}

#[test]
fn melee_none_means_no_melee() {
    let c = CharacterConfig::from_yaml(&ERMIAS.replace("melee: a", "melee: none")).unwrap();
    assert_eq!(c.combat.melee, Melee::None);
}

#[test]
fn explicit_login_steps_read() {
    let yaml = ERMIAS.replace(
        "steps: default-silvermere",
        "steps:\n    - { wait: \"Username:\", send: \"{account}\" }\n    - { wait: \"Password:\", send: \"{password}\" }",
    );
    let c = CharacterConfig::from_yaml(&yaml).unwrap();
    let LoginSteps::Steps(steps) = &c.login.as_ref().unwrap().steps else {
        panic!("explicit steps");
    };
    assert_eq!(
        (steps[1].wait.as_str(), steps[1].send.as_str()),
        ("Password:", "{password}")
    );
}

fn err(yaml: &str) -> String {
    CharacterConfig::from_yaml(yaml).unwrap_err().to_string()
}

#[test]
fn a_statline_the_matcher_cant_read_is_refused() {
    let e = err(&ERMIAS.replace("\"[HP=%h/%H,MA=%m/%M]:%r\"", "\"[MA=%m]:\""));
    assert!(e.contains("statline"), "{e}");
}

#[test]
fn a_percentage_over_100_is_refused_naming_its_field() {
    let e = err(&ERMIAS.replace("hp: 95%", "hp: 195%"));
    assert!(e.contains("rest.stop_when_all.hp"), "{e}");
}

#[test]
fn a_negative_tier_mp_is_refused_naming_its_field() {
    let e = err(&ERMIAS.replace("min_mp: 5", "min_mp: -5"));
    assert!(e.contains("combat.tiers[1].min_mp"), "{e}");
}

#[test]
fn a_rest_that_stops_before_it_starts_is_refused() {
    let e = err(&ERMIAS.replace("hp: 95%", "hp: 40%"));
    assert!(e.contains("rest.stop_when_all.hp"), "{e}");
}

#[test]
fn an_empty_priority_list_is_refused() {
    let e = err(&ERMIAS.replace(
        "- priority: [\"*shaman*\", \"*mage*\", \"acolyte\"]",
        "- priority: []",
    ));
    assert!(e.contains("combat.targeting[1]"), "{e}");
}

#[test]
fn an_unknown_field_is_refused() {
    let e = err(&format!("{ERMIAS}\ncolour: blue\n"));
    assert!(e.contains("colour"), "{e}");
}

#[test]
fn an_unknown_targeting_rule_is_refused() {
    let e = err(&ERMIAS.replace("- fight_back: true", "- flee: true"));
    assert!(e.contains("flee"), "{e}");
}
