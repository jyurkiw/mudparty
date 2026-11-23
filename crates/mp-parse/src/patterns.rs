//! The lines mudparty knows, each keyed to the silvermere catalogue entry it reads
//! (`silvermere-text/messages/catalogue.toml`, or `auth:` for
//! `silvermere-auth/messages/auth.toml`). The catalogue test checks every pattern against
//! its entry as silvermere renders it (design §10.2). Patterns match plain text, a whole
//! line. Order matters where shapes overlap: the first match wins.

use std::sync::LazyLock;

use regex::{Captures, Regex};

use crate::event::{Event, PartyRow, Rank};
use crate::statline::Rest;

struct Def {
    keys: &'static [&'static str],
    re: &'static str,
    build: fn(&Captures) -> Event,
}

fn g(c: &Captures, name: &str) -> String {
    c.name(name)
        .map_or_else(String::new, |m| m.as_str().to_owned())
}

fn n<T: std::str::FromStr + Default>(c: &Captures, name: &str) -> T {
    c.name(name)
        .and_then(|m| m.as_str().trim().parse().ok())
        .unwrap_or_default()
}

/// `(Warrior)   ` or a class cut to 12 (`(Necromancer`): the class name.
fn class(c: &Captures) -> String {
    let raw = g(c, "class");
    raw.trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .to_owned()
}

const DEFS: &[Def] = &[
    // ----- login
    Def {
        keys: &["auth:username_prompt"],
        re: r"^Username: ?$",
        build: |_| Event::UsernamePrompt,
    },
    Def {
        keys: &["auth:password_prompt"],
        re: r"^Password: ?$",
        build: |_| Event::PasswordPrompt,
    },
    Def {
        keys: &["auth:invalid_credentials"],
        re: r"^Invalid Credentials$",
        build: |_| Event::InvalidCredentials,
    },
    Def {
        keys: &["auth:account_locked"],
        re: r"^Too many failed logins: this account is locked for (?P<m>\d+) more minute\(s\)\.$",
        build: |c| Event::AccountLocked { minutes: n(c, "m") },
    },
    Def {
        keys: &["auth:duplicate_login", "auth:takeover_prompt"],
        re: r"^This account is already logged on\.$",
        build: |_| Event::DuplicateLogin,
    },
    Def {
        keys: &["auth:takeover_question", "auth:takeover_prompt"],
        re: r"^Disconnect the other connection and log in here\? \(Y/N\): ?$",
        build: |_| Event::TakeoverQuestion,
    },
    Def {
        keys: &["auth:takeover_failed"],
        re: r"^The other connection did not close in time\. Please try again in a minute\.$",
        build: |_| Event::TakeoverFailed,
    },
    Def {
        keys: &["auth:taken_over"],
        re: r"^This account has logged on from another connection\. Disconnecting\.$",
        build: |_| Event::TakenOver,
    },
    Def {
        keys: &["auth:no_password"],
        re: r"^This account has no password yet\. Ask the sysop to set one\.$",
        build: |_| Event::NoPassword,
    },
    Def {
        keys: &["bbs_menu"],
        re: r"^Make your selection \(X to exit\): ?$",
        build: |_| Event::MainMenu,
    },
    Def {
        keys: &["mud_menu_prompt"],
        re: r"^\[MAJORMUD\]: ?$",
        build: |_| Event::MudMenu,
    },
    // ----- combat
    Def {
        keys: &["combat_engaged", "combat_engaged_split"],
        re: r"^\*Combat Engaged\*$",
        build: |_| Event::CombatEngaged,
    },
    Def {
        keys: &["combat_off"],
        re: r"^\*Combat Off\*$",
        build: |_| Event::CombatOff,
    },
    // Before the general `moves to attack` shape, which these also fit.
    Def {
        keys: &["cast_attacks_room"],
        re: r"^(?P<who>\S+) moves to attack everyone in the room\.$",
        build: |c| Event::AttacksRoom { who: g(c, "who") },
    },
    Def {
        keys: &["combat_moves_to_attack_you", "cast_moves_to_attack_you"],
        re: r"^(?P<who>\S+) moves to attack you!$",
        build: |c| Event::AttacksYou { who: g(c, "who") },
    },
    Def {
        keys: &[
            "combat_moves_to_attack_mon",
            "cast_moves_to_attack",
            "combat_moves_to_attack_other",
        ],
        re: r"^(?P<who>\S+) moves to attack (?P<target>.+)[.!]$",
        build: |c| Event::MovesToAttack {
            who: g(c, "who"),
            target: g(c, "target"),
        },
    },
    Def {
        keys: &["cast_moves_to_cast"],
        re: r"^(?P<who>\S+) moves to cast (?P<spell>.+) upon (?P<target>.+)\.$",
        build: |c| Event::MovesToCast {
            who: g(c, "who"),
            spell: g(c, "spell"),
            target: g(c, "target"),
        },
    },
    Def {
        keys: &["cast_no_effect_on"],
        re: r"^Your spell has no effect on (?P<t>.+)\.$",
        build: |c| Event::SpellNoEffect { target: g(c, "t") },
    },
    Def {
        keys: &["combat_no_effect_weapon"],
        re: r"^Your weapon has no effect against this monster!$",
        build: |_| Event::WeaponNoEffect,
    },
    Def {
        keys: &["exp_gain"],
        re: r"^You gain (?P<exp>\d+) experience\.$",
        build: |c| Event::ExpGain { exp: n(c, "exp") },
    },
    Def {
        keys: &["room_also_here"],
        re: r"^Also here: (?P<who>.+)\.$",
        build: |c| Event::AlsoHere {
            names: g(c, "who").split(", ").map(str::to_owned).collect(),
        },
    },
    Def {
        keys: &["spell_backstab_lead_in"],
        re: r"^You strike from the shadows with (?P<spell>.+)!$",
        build: |c| Event::StrikeFromShadows {
            spell: g(c, "spell"),
        },
    },
    Def {
        keys: &["spell_backstab_lead_in_room"],
        re: r"^(?P<who>\S+) strikes from the shadows with (?P<spell>.+)!$",
        build: |c| Event::StrikesFromShadows {
            who: g(c, "who"),
            spell: g(c, "spell"),
        },
    },
    Def {
        keys: &["spell_backstab_spotted"],
        re: r"^Your foe spots you as you begin to cast!$",
        build: |_| Event::SpottedCasting,
    },
    // ----- talk and pacing
    Def {
        keys: &["telepath"],
        re: r"^(?P<who>\S+) telepaths: (?P<text>.*)$",
        build: |c| Event::Telepath {
            who: g(c, "who"),
            text: g(c, "text"),
        },
    },
    Def {
        keys: &["gang_paths"],
        re: r"^(?P<who>\S+) gangpaths: (?P<text>.*)$",
        build: |c| Event::Gangpath {
            who: g(c, "who"),
            text: g(c, "text"),
        },
    },
    Def {
        keys: &["telepath_sent"],
        re: r"^--- Telepath Sent to (?P<who>.+) ---$",
        build: |c| Event::TelepathSent { who: g(c, "who") },
    },
    Def {
        keys: &["telepath_not_sent"],
        re: r"^--- Telepath Not Sent ---$",
        build: |_| Event::TelepathNotSent,
    },
    Def {
        keys: &["too_fast"],
        re: r"^You are typing too quickly - command ignored$",
        build: |_| Event::TooFast,
    },
    Def {
        keys: &["slow_down"],
        re: r"^Why don't you slow down for a few seconds\?$",
        build: |_| Event::SlowDown,
    },
    // ----- party
    Def {
        keys: &["invited_you_follow"],
        re: r"^(?P<who>\S+) has invited you to follow \S+\.$",
        build: |c| Event::InvitedYouFollow { who: g(c, "who") },
    },
    Def {
        keys: &["you_invited_follow"],
        re: r"^You have invited (?P<who>.+) to follow you\.$",
        build: |c| Event::YouInvitedFollow { who: g(c, "who") },
    },
    Def {
        keys: &["now_following"],
        re: r"^You are now following (?P<who>\S+)$",
        build: |c| Event::NowFollowing { who: g(c, "who") },
    },
    Def {
        keys: &["started_follow"],
        re: r"^(?P<who>\S+) started to follow you\.$",
        build: |c| Event::StartedFollow { who: g(c, "who") },
    },
    Def {
        keys: &["no_longer_following"],
        re: r"^You are no longer following (?P<who>\S+)\.$",
        build: |c| Event::NoLongerFollowing { who: g(c, "who") },
    },
    Def {
        keys: &["follower_removed"],
        re: r"^(?P<who>\S+) has been removed from your followers\.$",
        build: |c| Event::FollowerRemoved { who: g(c, "who") },
    },
    Def {
        keys: &["invite_first"],
        re: r"^You must be invited first!$",
        build: |_| Event::InviteFirst,
    },
    Def {
        keys: &["follow_nothing"],
        re: r"^You don't see that anywhere!$",
        build: |_| Event::FollowNothing,
    },
    Def {
        keys: &["gang_not_here"],
        re: r"^You don't see (?P<who>.+) here!$",
        build: |c| Event::NotHere { who: g(c, "who") },
    },
    Def {
        keys: &["gang_invite_forgotten"],
        re: r"^You cannot invite (?P<who>.+)\.$",
        build: |c| Event::CannotInvite { who: g(c, "who") },
    },
    Def {
        keys: &["party_header"],
        re: r"^The following people are in your travel party:$",
        build: |_| Event::PartyHeader,
    },
    Def {
        keys: &["party_following"],
        re: r"^You are following (?P<who>\S+)\.$",
        build: |c| Event::PartyFollowing { who: g(c, "who") },
    },
    // `  %-30.30s %-12.12s` then ` [M:%3.3s%]` (K for kai) or nine spaces, ` [H:%3.3s%]`,
    // P or a space, R / M or a space, and the rank (silvermere display/party.rs).
    Def {
        keys: &["party_row"],
        re: r"^  (?P<name>.{30}) (?P<class>.{12})(?: \[[MK]:(?P<mp> *-?\d+)%\]| {9}) \[H:(?P<hp> *-?\d+)%\](?P<p>[P ])(?P<r>[RM ]) - (?P<rank>Frontrank|Backrank|Midrank)$",
        build: |c| {
            Event::PartyRow(PartyRow {
                name: g(c, "name").trim_end().to_owned(),
                class: class(c),
                mp_pct: c.name("mp").and_then(|m| m.as_str().trim().parse().ok()),
                hp_pct: n(c, "hp"),
                poisoned: &g(c, "p") == "P",
                rest: match g(c, "r").as_str() {
                    "R" => Rest::Resting,
                    "M" => Rest::Meditating,
                    _ => Rest::Standing,
                },
                rank: match g(c, "rank").as_str() {
                    "Frontrank" => Rank::Front,
                    "Backrank" => Rank::Back,
                    _ => Rank::Middle,
                },
            })
        },
    },
    Def {
        keys: &["party_row_invited"],
        re: r"^  (?P<name>.{30}) (?P<class>.{12}) \[Invited\]$",
        build: |c| Event::PartyRowInvited {
            name: g(c, "name").trim_end().to_owned(),
            class: class(c),
        },
    },
    Def {
        keys: &["party_none"],
        re: r"^You are not in a party at the present time\.$",
        build: |_| Event::PartyNone,
    },
];

static COMPILED: LazyLock<Vec<(Regex, &'static Def)>> = LazyLock::new(|| {
    DEFS.iter()
        .map(|d| (Regex::new(d.re).expect("the patterns compile"), d))
        .collect()
});

/// What a whole plain-text line means, if mudparty knows it.
pub fn classify(plain: &str) -> Option<Event> {
    COMPILED
        .iter()
        .find_map(|(re, d)| re.captures(plain).map(|c| (d.build)(&c)))
}

/// Every catalogue key a pattern reads, for the catalogue test.
pub fn pattern_keys() -> Vec<&'static str> {
    let mut keys: Vec<&str> = DEFS.iter().flat_map(|d| d.keys.iter().copied()).collect();
    keys.sort_unstable();
    keys.dedup();
    keys
}
