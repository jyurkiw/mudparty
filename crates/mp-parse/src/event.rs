//! What a line of game text means to mudparty.
//!
//! Events are built from a pattern's named regex groups through serde (`patterns.toml`):
//! the `event` field names the variant and each group fills the field of the same name.
//! The few fields whose text needs reading (padding, a class in brackets, a flag letter)
//! say how with a `deserialize_with` below.

use serde::{Deserialize, Deserializer};

use crate::statline::Rest;

/// A party member's rank, from `par`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rank {
    Front,
    Middle,
    Back,
}

/// One member's row of `par`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PartyRow {
    /// First and last name.
    #[serde(deserialize_with = "padded")]
    pub name: String,
    #[serde(deserialize_with = "class_name")]
    pub class: String,
    /// MP (or kai) as a percentage; `None` when the row shows none.
    #[serde(default)]
    pub mp_pct: Option<i32>,
    pub hp_pct: i32,
    #[serde(deserialize_with = "poison_flag")]
    pub poisoned: bool,
    #[serde(deserialize_with = "rest_flag")]
    pub rest: Rest,
    #[serde(deserialize_with = "rank_word")]
    pub rank: Rank,
}

/// A field padded to its width: the text without the trailing spaces.
fn padded<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(String::deserialize(d)?.trim_end().to_owned())
}

/// `(Warrior)   `, or a class cut to 12 (`(Necromancer`): the class name.
fn class_name<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let raw = String::deserialize(d)?;
    Ok(raw
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .to_owned())
}

/// `P` or a space.
fn poison_flag<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(String::deserialize(d)? == "P")
}

/// `R` resting, `M` meditating, or a space.
fn rest_flag<'de, D: Deserializer<'de>>(d: D) -> Result<Rest, D::Error> {
    Ok(match String::deserialize(d)?.as_str() {
        "R" => Rest::Resting,
        "M" => Rest::Meditating,
        _ => Rest::Standing,
    })
}

/// `Frontrank`, `Backrank` or `Midrank`.
fn rank_word<'de, D: Deserializer<'de>>(d: D) -> Result<Rank, D::Error> {
    match String::deserialize(d)?.as_str() {
        "Frontrank" => Ok(Rank::Front),
        "Backrank" => Ok(Rank::Back),
        "Midrank" => Ok(Rank::Middle),
        other => Err(serde::de::Error::custom(format!("unknown rank {other:?}"))),
    }
}

/// A line mudparty acts on. Each comes from one or more silvermere catalogue entries, named
/// in `patterns.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "event", deny_unknown_fields)]
pub enum Event {
    // ----- login
    UsernamePrompt,
    PasswordPrompt,
    InvalidCredentials,
    AccountLocked {
        minutes: u32,
    },
    DuplicateLogin,
    TakeoverQuestion,
    TakeoverFailed,
    TakenOver,
    NoPassword,
    /// The BBS main menu's `Make your selection (X to exit):`.
    MainMenu,
    /// `[MAJORMUD]:`.
    MudMenu,

    // ----- combat
    CombatEngaged,
    CombatOff,
    /// Someone else's melee or spell attack announce. `target` may be a monster or a player.
    MovesToAttack {
        who: String,
        target: String,
    },
    MovesToCast {
        who: String,
        spell: String,
        target: String,
    },
    /// A room spell: `<who> moves to attack everyone in the room.`
    AttacksRoom {
        who: String,
    },
    /// `<who> moves to attack you!`
    AttacksYou {
        who: String,
    },
    SpellNoEffect {
        target: String,
    },
    WeaponNoEffect,
    ExpGain {
        exp: i64,
    },
    /// The room's `Also here:` list, players and monsters.
    AlsoHere {
        names: Vec<String>,
    },
    /// The fork's spell backstab, stealth kept: `You strike from the shadows with <spell>!`
    StrikeFromShadows {
        spell: String,
    },
    /// Someone else's spell backstab.
    StrikesFromShadows {
        who: String,
        spell: String,
    },
    /// The fork's spell backstab, stealth lost: `Your foe spots you as you begin to cast!`
    SpottedCasting,

    // ----- talk and pacing
    Telepath {
        who: String,
        text: String,
    },
    Gangpath {
        who: String,
        text: String,
    },
    TelepathSent {
        who: String,
    },
    TelepathNotSent,
    /// `You are typing too quickly - command ignored`: the last command was lost.
    TooFast,
    /// `Why don't you slow down for a few seconds?`
    SlowDown,

    // ----- party
    InvitedYouFollow {
        who: String,
    },
    YouInvitedFollow {
        who: String,
    },
    NowFollowing {
        who: String,
    },
    StartedFollow {
        who: String,
    },
    NoLongerFollowing {
        who: String,
    },
    FollowerRemoved {
        who: String,
    },
    InviteFirst,
    /// `follow` found nobody of that name here.
    FollowNothing,
    /// `invite` found nobody of that name here.
    NotHere {
        who: String,
    },
    CannotInvite {
        who: String,
    },
    PartyHeader,
    PartyFollowing {
        who: String,
    },
    PartyRow(PartyRow),
    PartyRowInvited {
        #[serde(deserialize_with = "padded")]
        name: String,
        #[serde(deserialize_with = "class_name")]
        class: String,
    },
    PartyNone,
}
