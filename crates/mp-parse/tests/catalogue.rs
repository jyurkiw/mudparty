//! The text guard (design §10.2): every pattern names the silvermere catalogue entry it
//! reads, and is checked here against that entry as silvermere renders it.

use std::collections::BTreeSet;

use mp_parse::{Event, PartyRow, Rank, Rest, classify, pattern_keys};
use silvermere_auth::login::AuthMsg;
use silvermere_text::msg::CATALOGUE_TOML;
use silvermere_text::{Msg, Palette, render};

/// A catalogue key, its renderings, and the events each must parse to.
type Case = (&'static str, Vec<Vec<u8>>, Vec<Event>);

/// The events `bytes` produce, line by line, the unfinished last line included.
fn events(bytes: &[u8]) -> Vec<Event> {
    let mut x = mp_text::Extractor::new();
    let mut lines = x.feed(bytes);
    lines.extend(x.pending());
    lines.iter().filter_map(|l| classify(&l.plain)).collect()
}

fn s(v: &str) -> String {
    v.to_owned()
}

/// A game message, with what it must parse to in every palette.
fn msg(key: &'static str, m: Msg, want: Vec<Event>) -> Case {
    let bytes = (0..4)
        .map(|p| render(&m, Palette::new(p).unwrap()).bytes)
        .collect();
    (key, bytes, want)
}

/// A login message from silvermere-auth's catalogue.
fn auth(key: &'static str, m: AuthMsg, want: Vec<Event>) -> Case {
    assert_eq!(format!("auth:{}", m.key()), key);
    (key, vec![m.text().into_bytes()], want)
}

fn cases() -> Vec<Case> {
    use Event as E;
    vec![
        // login (silvermere-auth/messages/auth.toml)
        auth(
            "auth:username_prompt",
            AuthMsg::UsernamePrompt,
            vec![E::UsernamePrompt],
        ),
        auth(
            "auth:password_prompt",
            AuthMsg::PasswordPrompt,
            vec![E::PasswordPrompt],
        ),
        auth(
            "auth:invalid_credentials",
            AuthMsg::InvalidCredentials,
            vec![E::InvalidCredentials],
        ),
        auth(
            "auth:account_locked",
            AuthMsg::AccountLocked { minutes: 12 },
            vec![E::AccountLocked { minutes: 12 }],
        ),
        auth(
            "auth:duplicate_login",
            AuthMsg::DuplicateLogin,
            vec![E::DuplicateLogin],
        ),
        auth(
            "auth:takeover_prompt",
            AuthMsg::TakeoverPrompt,
            vec![E::DuplicateLogin, E::TakeoverQuestion],
        ),
        auth(
            "auth:takeover_question",
            AuthMsg::TakeoverQuestion,
            vec![E::TakeoverQuestion],
        ),
        auth(
            "auth:takeover_failed",
            AuthMsg::TakeoverFailed,
            vec![E::TakeoverFailed],
        ),
        auth("auth:taken_over", AuthMsg::TakenOver, vec![E::TakenOver]),
        auth("auth:no_password", AuthMsg::NoPassword, vec![E::NoPassword]),
        // the menus
        msg("bbs_menu", Msg::BbsMenu, vec![E::MainMenu]),
        msg("mud_menu_prompt", Msg::MudMenuPrompt, vec![E::MudMenu]),
        // combat
        msg("combat_engaged", Msg::CombatEngaged, vec![E::CombatEngaged]),
        msg(
            "combat_engaged_split",
            Msg::CombatEngagedSplit,
            vec![E::CombatEngaged],
        ),
        msg("combat_off", Msg::CombatOff, vec![E::CombatOff]),
        msg(
            "combat_moves_to_attack_mon",
            Msg::CombatMovesToAttackMon {
                who: s("Raijin"),
                mon: s("small kobold"),
            },
            vec![E::MovesToAttack {
                who: s("Raijin"),
                target: s("small kobold"),
            }],
        ),
        msg(
            "cast_moves_to_attack",
            Msg::CastMovesToAttack {
                who: s("Raijin"),
                target: s("Bob"),
            },
            vec![E::MovesToAttack {
                who: s("Raijin"),
                target: s("Bob"),
            }],
        ),
        msg(
            "combat_moves_to_attack_other",
            Msg::CombatMovesToAttackOther {
                who: s("Raijin"),
                target: s("Bob"),
            },
            vec![E::MovesToAttack {
                who: s("Raijin"),
                target: s("Bob"),
            }],
        ),
        msg(
            "cast_moves_to_cast",
            Msg::CastMovesToCast {
                who: s("Ermias"),
                spell: s("magic missile"),
                target: s("small kobold"),
            },
            vec![E::MovesToCast {
                who: s("Ermias"),
                spell: s("magic missile"),
                target: s("small kobold"),
            }],
        ),
        msg(
            "cast_attacks_room",
            Msg::CastAttacksRoom { who: s("Ermias") },
            vec![E::AttacksRoom { who: s("Ermias") }],
        ),
        msg(
            "combat_moves_to_attack_you",
            Msg::CombatMovesToAttackYou { who: s("Bob") },
            vec![E::AttacksYou { who: s("Bob") }],
        ),
        msg(
            "cast_moves_to_attack_you",
            Msg::CastMovesToAttackYou { who: s("Bob") },
            vec![E::AttacksYou { who: s("Bob") }],
        ),
        msg(
            "cast_no_effect_on",
            Msg::CastNoEffectOn {
                target: s("acid slime"),
            },
            vec![E::SpellNoEffect {
                target: s("acid slime"),
            }],
        ),
        msg(
            "combat_no_effect_weapon",
            Msg::CombatNoEffectWeapon,
            vec![E::WeaponNoEffect],
        ),
        msg(
            "exp_gain",
            Msg::ExpGain { exp: 1250 },
            vec![E::ExpGain { exp: 1250 }],
        ),
        msg(
            "room_also_here",
            Msg::RoomAlsoHere {
                who: s("Raijin, small kobold, giant rat"),
            },
            vec![E::AlsoHere {
                names: vec![s("Raijin"), s("small kobold"), s("giant rat")],
            }],
        ),
        // the fork's stealth casters (design/stealth-casters.md)
        msg(
            "spell_backstab_lead_in",
            Msg::SpellBackstabLeadIn {
                spell: s("magic missile"),
            },
            vec![E::StrikeFromShadows {
                spell: s("magic missile"),
            }],
        ),
        msg(
            "spell_backstab_lead_in_room",
            Msg::SpellBackstabLeadInRoom {
                who: s("Ermias"),
                spell: s("godstrike"),
            },
            vec![E::StrikesFromShadows {
                who: s("Ermias"),
                spell: s("godstrike"),
            }],
        ),
        msg(
            "spell_backstab_spotted",
            Msg::SpellBackstabSpotted,
            vec![E::SpottedCasting],
        ),
        // leaving the realm
        msg("exit_meditate", Msg::ExitMeditate, vec![E::ExitStarted]),
        msg(
            "exit_interrupted",
            Msg::ExitInterrupted,
            vec![E::ExitInterrupted],
        ),
        msg("exit_waiting", Msg::ExitWaiting, vec![E::ExitWaiting]),
        msg("exit_wounded", Msg::ExitWounded, vec![E::MortallyWounded]),
        msg(
            "mortally_wounded",
            Msg::MortallyWounded,
            vec![E::MortallyWounded],
        ),
        // telepaths and pacing
        msg(
            "telepath",
            Msg::Telepath {
                who: s("Raijin"),
                text: s("@health"),
            },
            vec![E::Telepath {
                who: s("Raijin"),
                text: s("@health"),
            }],
        ),
        msg(
            "gang_paths",
            Msg::GangPaths {
                who: s("Raijin"),
                msg: s("@wait"),
            },
            vec![E::Gangpath {
                who: s("Raijin"),
                text: s("@wait"),
            }],
        ),
        msg(
            "telepath_sent",
            Msg::TelepathSent { who: s("Raijin") },
            vec![E::TelepathSent { who: s("Raijin") }],
        ),
        msg(
            "telepath_not_sent",
            Msg::TelepathNotSent,
            vec![E::TelepathNotSent],
        ),
        msg("user_unknown", Msg::UserUnknown, vec![E::UserUnknown]),
        msg("too_fast", Msg::TooFast, vec![E::TooFast]),
        msg("slow_down", Msg::SlowDown, vec![E::SlowDown]),
        // party
        msg(
            "invited_you_follow",
            Msg::InvitedYouFollow {
                who: s("Raijin"),
                whom: s("him"),
            },
            vec![E::InvitedYouFollow { who: s("Raijin") }],
        ),
        msg(
            "you_invited_follow",
            Msg::YouInvitedFollow { who: s("Ermias") },
            vec![E::YouInvitedFollow { who: s("Ermias") }],
        ),
        msg(
            "now_following",
            Msg::NowFollowing { who: s("Raijin") },
            vec![E::NowFollowing { who: s("Raijin") }],
        ),
        msg(
            "started_follow",
            Msg::StartedFollow { who: s("Ermias") },
            vec![E::StartedFollow { who: s("Ermias") }],
        ),
        msg(
            "no_longer_following",
            Msg::NoLongerFollowing { who: s("Raijin") },
            vec![E::NoLongerFollowing { who: s("Raijin") }],
        ),
        msg(
            "follower_removed",
            Msg::FollowerRemoved { who: s("Ermias") },
            vec![E::FollowerRemoved { who: s("Ermias") }],
        ),
        msg("invite_first", Msg::InviteFirst, vec![E::InviteFirst]),
        msg("follow_nothing", Msg::FollowNothing, vec![E::FollowNothing]),
        msg(
            "gang_not_here",
            Msg::GangNotHere { who: s("Raijin") },
            vec![E::NotHere { who: s("Raijin") }],
        ),
        msg(
            "gang_invite_forgotten",
            Msg::GangInviteForgotten { who: s("Ermias") },
            vec![E::CannotInvite { who: s("Ermias") }],
        ),
        msg("party_header", Msg::PartyHeader, vec![E::PartyHeader]),
        msg(
            "party_following",
            Msg::PartyFollowing { who: s("Raijin") },
            vec![E::PartyFollowing { who: s("Raijin") }],
        ),
        msg(
            "party_row",
            Msg::PartyRow {
                name: format!("{:<30}", "Ermias Vale"),
                class: format!("{:<12}", "(Assassin)"),
                mana: s(" [M: 45%]"),
                hp: s(" 85"),
                poison: s("P"),
                rest: s("R"),
                rank: s(" - Backrank"),
            },
            vec![E::PartyRow(PartyRow {
                name: s("Ermias Vale"),
                class: s("Assassin"),
                mp_pct: Some(45),
                hp_pct: 85,
                poisoned: true,
                rest: Rest::Resting,
                rank: Rank::Back,
            })],
        ),
        msg(
            "party_row",
            Msg::PartyRow {
                name: format!("{:<30}", "Bob Smith"),
                class: format!("{:<12}", "(Warrior)"),
                mana: " ".repeat(9),
                hp: s("100"),
                poison: s(" "),
                rest: s(" "),
                rank: s(" - Frontrank"),
            },
            vec![E::PartyRow(PartyRow {
                name: s("Bob Smith"),
                class: s("Warrior"),
                mp_pct: None,
                hp_pct: 100,
                poisoned: false,
                rest: Rest::Standing,
                rank: Rank::Front,
            })],
        ),
        msg(
            "party_row_invited",
            Msg::PartyRowInvited {
                name: format!("{:<30}", "Bob Smith"),
                class: format!("{:<12}", "(Warrior)"),
            },
            vec![E::PartyRowInvited {
                name: s("Bob Smith"),
                class: s("Warrior"),
            }],
        ),
        msg("party_none", Msg::PartyNone, vec![E::PartyNone]),
        // resting
        msg("rest_start", Msg::RestStart, vec![E::RestStarted]),
        msg("meditate_start", Msg::MeditateStart, vec![E::RestStarted]),
        msg("rest_sick", Msg::RestSick, vec![E::TooSick]),
        msg("meditate_sick", Msg::MeditateSick, vec![E::TooSick]),
        msg(
            "meditate_useless",
            Msg::MeditateUseless,
            vec![E::MeditateUseless],
        ),
        msg("poison_tick", Msg::PoisonTick, vec![E::PoisonTick]),
    ]
}

#[test]
fn every_pattern_reads_its_catalogue_entry_as_silvermere_renders_it() {
    for (key, renderings, want) in cases() {
        for (palette, bytes) in renderings.iter().enumerate() {
            assert_eq!(
                events(bytes),
                want,
                "{key}, rendering {palette}: {:?}",
                String::from_utf8_lossy(bytes)
            );
        }
    }
}

#[test]
fn every_pattern_key_is_a_real_catalogue_entry() {
    for key in pattern_keys() {
        if key.starts_with("auth:") {
            continue; // checked by building the AuthMsg in cases()
        }
        assert!(
            CATALOGUE_TOML.contains(&format!("[messages.{key}]")),
            "{key} isn't in silvermere's catalogue"
        );
    }
}

#[test]
fn every_pattern_key_has_a_case_here() {
    let tested: BTreeSet<&str> = cases().iter().map(|(k, _, _)| *k).collect();
    let keys: BTreeSet<&str> = pattern_keys().into_iter().collect();
    let untested: Vec<_> = keys.difference(&tested).collect();
    let unknown: Vec<_> = tested.difference(&keys).collect();
    assert!(
        untested.is_empty(),
        "patterns with no catalogue case: {untested:?}"
    );
    assert!(
        unknown.is_empty(),
        "cases for keys no pattern names: {unknown:?}"
    );
}

#[test]
fn ordinary_text_is_no_event() {
    assert_eq!(
        classify("You notice a small kobold lurking in the shadows."),
        None
    );
    assert_eq!(classify(""), None);
}
