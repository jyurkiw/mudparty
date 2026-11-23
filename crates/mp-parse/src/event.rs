//! What a line of game text means to mudparty.

use crate::statline::Rest;

/// A party member's rank, from `par`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rank {
    Front,
    Middle,
    Back,
}

/// One member's row of `par`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartyRow {
    /// First and last name.
    pub name: String,
    pub class: String,
    /// MP (or kai) as a percentage; `None` when the row shows none.
    pub mp_pct: Option<i32>,
    pub hp_pct: i32,
    pub poisoned: bool,
    pub rest: Rest,
    pub rank: Rank,
}

/// A line mudparty acts on. Each comes from one or more silvermere catalogue entries, named
/// in `patterns.rs`.
#[derive(Debug, Clone, PartialEq, Eq)]
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
        name: String,
        class: String,
    },
    PartyNone,
}
