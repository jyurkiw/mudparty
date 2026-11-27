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

/// A stat's mark: the game's red `*` for a value that differs from its base. The group is
/// there only when the star is.
fn star<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(String::deserialize(d)? == "*")
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
    /// `You have progressed too far without training to acquire anymore experience!`: a
    /// kill that gained nothing, in place of `ExpGain`.
    ExpCapped,
    /// `<who> just left to the <dir>.` (or upwards, downwards): gone from the room.
    Left {
        who: String,
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

    // ----- leaving the realm
    /// `You will exit after a period of silent meditation.` (`quit` or `x`).
    ExitStarted,
    /// `Your meditation has been interrupted - you may not exit now!`: an attack (or
    /// `break`) called the exit off.
    ExitInterrupted,
    /// `You may not perform any commands while waiting to exit!`
    ExitWaiting,
    /// At 0 HP or below: `x` and `quit` are both refused.
    MortallyWounded,

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
    /// `Cannot find user!`: a telepath (among others) to nobody in the game; no
    /// acknowledgement follows.
    UserUnknown,
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

    // ----- moving
    /// A room display's `Obvious exits:` line: the move it follows has arrived.
    ObviousExits,
    /// `There is no exit in that direction!`
    NoExit,
    /// A closed door or gate in the way.
    DoorClosed,
    /// `The door is now open.`, or it was already.
    DoorOpened,
    /// `The door is locked.`
    DoorLocked,
    /// `You bashed the door open.`
    BashedOpen,
    /// `Your attempts to bash through fail!`
    BashFailed,
    /// Any other refusal of an exit (level, alignment, a spell or item it needs, timed),
    /// as the game words it.
    ExitRefused {
        text: String,
    },

    // ----- shops and gear (#55)
    /// `You just bought <item> for <coins>.`
    Bought {
        item: String,
    },
    /// `You cannot afford <item>.`
    CannotAfford {
        item: String,
    },
    /// Any other refusal of a `buy` (not sold here, not known, too heavy), as worded.
    BuyRefused {
        text: String,
    },
    /// `You are now holding <item>.` (wield) or `You are now wearing <item>.` (wear).
    Equipped {
        item: String,
    },
    /// A refused `wield` or `wear`, as worded.
    EquipRefused {
        text: String,
    },
    /// The inventory's `You are carrying …` line. A long list wraps onto the lines after
    /// it, up to the keys line.
    Carrying {
        items: String,
    },
    /// `You are carrying Nothing!`
    CarryingNothing,
    /// The inventory's keys line, `You have no keys.` or the list: the carried list is over.
    KeysLine,
    /// The inventory's `Wealth:` line, in copper farthings.
    Wealth {
        copper: i64,
    },
    /// The inventory's last line: `Encumbrance: 520/3792 - None [13%]`, the weight carried
    /// (coins included), the most that can be, and how heavy that is.
    Encumbrance {
        cur: i64,
        max: i64,
        level: String,
        pct: i64,
    },

    // ----- the status display, `st` (#10)
    /// `Name: <first last>    Lives/CP:  <lives>/<cp>`, its first line.
    StName {
        name: String,
        lives: i64,
        cp: i64,
    },
    /// `Race: <race> Exp: <exp> Perception: <n>`.
    StRace {
        race: String,
        exp: i64,
        perception: i64,
    },
    /// `Class: <class> Level: <n> Stealth: <n>`.
    StClass {
        class: String,
        level: i64,
        stealth: i64,
    },
    /// `Hits: <hp>/<max> Armour Class: <ac>/<dr> Thievery: <n>`.
    StHits {
        hp: i64,
        max_hp: i64,
        ac: i64,
        dr: i64,
        thievery: i64,
    },
    /// `Mana: <mp>/<max> Spellcasting: <n> Traps: <n>`, or `Kai: <mp>/<max> Traps: <n>`.
    /// `marked`: the red star of a character with the Mana ability.
    StMagic {
        kind: String,
        #[serde(default, deserialize_with = "star")]
        marked: bool,
        mp: i64,
        max_mp: i64,
        #[serde(default)]
        spellcasting: Option<i64>,
        traps: i64,
    },
    /// `Traps: <n>` alone: the magery line of a class without magic.
    StTraps {
        traps: i64,
    },
    /// `Picklocks: <n>`.
    StPicklocks {
        picklocks: i64,
    },
    /// `Strength: <n> Agility: <n> Tracking: <n>`. A stat is `marked` (the game's red star)
    /// when it differs from its base value.
    StStrength {
        strength: i64,
        #[serde(default, deserialize_with = "star")]
        strength_marked: bool,
        agility: i64,
        #[serde(default, deserialize_with = "star")]
        agility_marked: bool,
        tracking: i64,
    },
    /// `Intellect: <n> Health: <n> Martial Arts: <n>`.
    StIntellect {
        intellect: i64,
        #[serde(default, deserialize_with = "star")]
        intellect_marked: bool,
        health: i64,
        #[serde(default, deserialize_with = "star")]
        health_marked: bool,
        martial_arts: i64,
    },
    /// `Willpower: <n> Charm: <n> MagicRes: <n>`: the status display's last stats line.
    StWillpower {
        willpower: i64,
        #[serde(default, deserialize_with = "star")]
        willpower_marked: bool,
        charm: i64,
        #[serde(default, deserialize_with = "star")]
        charm_marked: bool,
        magic_res: i64,
    },
    /// `You are Poisoned!`, in the status display.
    StPoisoned,

    // ----- training (#56)
    /// `You hand over <coins> and you receive training to attain level <n>.`
    Trained {
        level: u32,
    },
    /// `You do not have the required experience to train yet!`
    TrainNoExp,
    /// `You do not have the money required for your training.`
    TrainNoMoney,
    /// Any other refusal of `train` (not a training room, another class's trainer, a level
    /// outside the trainer's, tournament mode), as worded.
    TrainRefused {
        text: String,
    },

    // ----- where and how far
    /// `/room`'s answer, `1/206` (a silvermere fork command).
    RoomKey {
        map: u32,
        room: u32,
    },
    /// PROFILE's `Statusline:` row: `On`, `Full`, `Off`, or the custom template itself.
    ProfileStatline {
        mode: String,
    },
    /// The `exp` command's line.
    ExpLine {
        exp: i64,
        level: u32,
        /// Experience still needed for the next level.
        left: i64,
        /// The next level's total.
        need: i64,
        pct: u32,
    },

    // ----- resting
    /// `You are now resting.` or `You are now meditating.`
    RestStarted,
    /// `You are too sick to rest!` (or meditate): poison stops both.
    TooSick,
    /// `Meditation will not help at this time.`
    MeditateUseless,
    /// `You feel ill.`: a poison tick, each slow tick while poisoned.
    PoisonTick,
}
