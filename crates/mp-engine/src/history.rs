//! The text history (the card's History modal), as pure logic: what a character saw, less
//! what the page shows elsewhere.
//!
//! - **Kept:** chat (telepaths, gangpaths, and lines that say, yell, gossip and so on),
//!   monsters coming in and going, fight lines, experience gained, and any text nothing
//!   else reads.
//! - **Left out:** what mudparty reads for the page (the status display, the inventory,
//!   `par`, `/room`, `pro`, `exp`, telepath acknowledgements); a room display (its name and
//!   description, up to its `Obvious exits:`, whatever brought it on: a step, a look, a
//!   party drag); the output of an informational command (`help`, `look`, `list`, `who`,
//!   ...), from its echo to the next prompt; and the echo of every line the bot sent,
//!   which the page shows as `> line` already.
//!
//! An event's line is kept at once. Unread text waits for its prompt, since a room
//! display is only known as one when its `Obvious exits:` comes.

use std::collections::VecDeque;

use mp_parse::{Event, Item};

/// A kept line, with what kind it is, for the page to colour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistLine {
    /// `chat`, `move`, `combat` or `text`.
    pub kind: &'static str,
    pub text: String,
}

/// The words of informational commands: their output is the page's to show, not history.
const INFO: &[&str] = &[
    "st",
    "stat",
    "status",
    "i",
    "inv",
    "inventory",
    "par",
    "party",
    "exp",
    "experience",
    "/room",
    "pro",
    "profile",
    "list",
    "look",
    "l",
    "help",
    "spells",
    "who",
    "bankbook",
    "health",
    "set",
];

/// The verbs of talk, in `<name> <verb> ...` (a colon after the verb is allowed:
/// `Bob gossips: ...`).
const TALK: &[&str] = &[
    "says",
    "yells",
    "gossips",
    "auctions",
    "broadcasts",
    "whispers",
    "telepaths",
    "gangpaths",
    "shouts",
];

/// Reads what a session sees into its history.
#[derive(Debug, Default)]
pub struct HistoryFilter {
    /// Lines the bot sent whose echo hasn't come yet.
    sent: VecDeque<String>,
    /// An informational command's output is coming, until the prompt.
    info: bool,
    /// Unread text since the prompt, held until it's known not to be a room display.
    held: Vec<String>,
}

impl HistoryFilter {
    /// The bot sent `line`: its echo is left out.
    pub fn sent(&mut self, line: &str) {
        self.sent.push_back(line.trim().to_ascii_lowercase());
        if self.sent.len() > 20 {
            self.sent.pop_front();
        }
    }

    /// The next item the game sent, with its plain text; the lines it adds to the history.
    pub fn on_item(&mut self, plain: &str, item: &Item) -> Vec<HistLine> {
        let line = |kind, text: &str| HistLine {
            kind,
            text: text.trim_end().to_owned(),
        };
        match item {
            Item::Prompt(_) => {
                self.info = false;
                self.held.drain(..).map(|t| line("text", &t)).collect()
            }
            Item::Event(e) => match kind(e) {
                Kind::Keep(k) => vec![line(k, plain)],
                Kind::RoomEnd => {
                    // What came before it was the room's name and description.
                    self.held.clear();
                    Vec::new()
                }
                Kind::Drop => Vec::new(),
            },
            Item::Text(t) => self.on_text(t),
        }
    }

    fn on_text(&mut self, t: &str) -> Vec<HistLine> {
        let trimmed = t.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        let low = trimmed.to_ascii_lowercase();
        // The echo of a line the bot sent.
        if let Some(i) = self.sent.iter().position(|s| *s == low) {
            self.sent.remove(i);
            if is_info(&low) {
                self.info = true;
            }
            return Vec::new();
        }
        // A player's informational command, typed in MudPlay.
        if is_info(&low) && low.split_whitespace().count() <= 4 {
            self.info = true;
            return Vec::new();
        }
        if low.contains(" into the room") {
            return vec![HistLine {
                kind: "move",
                text: t.trim_end().to_owned(),
            }];
        }
        // Talk's verb is lower case as the game writes it (`Bob telepaths: ...`), unlike a
        // display's label (`Allow Telepaths:    Yes`).
        if trimmed
            .split_whitespace()
            .nth(1)
            .is_some_and(|verb| TALK.contains(&verb.trim_end_matches(':')))
        {
            return vec![HistLine {
                kind: "chat",
                text: t.trim_end().to_owned(),
            }];
        }
        if !self.info {
            self.held.push(t.to_owned());
        }
        Vec::new()
    }
}

/// Whether a line is an informational command: its first word is one.
fn is_info(low: &str) -> bool {
    low.split_whitespace()
        .next()
        .is_some_and(|w| INFO.contains(&w))
}

enum Kind {
    Keep(&'static str),
    /// `Obvious exits:`: a room display's end.
    RoomEnd,
    Drop,
}

/// What an event's line is to the history.
fn kind(e: &Event) -> Kind {
    use Event as E;
    match e {
        E::Telepath { .. } | E::Gangpath { .. } => Kind::Keep("chat"),
        E::Left { .. } => Kind::Keep("move"),
        E::CombatEngaged
        | E::CombatOff
        | E::MovesToAttack { .. }
        | E::MovesToCast { .. }
        | E::AttacksRoom { .. }
        | E::AttacksYou { .. }
        | E::SpellNoEffect { .. }
        | E::WeaponNoEffect
        | E::ExpGain { .. }
        | E::ExpCapped
        | E::StrikeFromShadows { .. }
        | E::StrikesFromShadows { .. }
        | E::SpottedCasting
        | E::MortallyWounded => Kind::Keep("combat"),
        E::ObviousExits => Kind::RoomEnd,
        // Read for the page: the sheets, the room, the party, the profile, `exp`, and the
        // acknowledgements of telepaths.
        E::AlsoHere { .. }
        | E::RoomKey { .. }
        | E::StName { .. }
        | E::StRace { .. }
        | E::StClass { .. }
        | E::StHits { .. }
        | E::StMagic { .. }
        | E::StTraps { .. }
        | E::StPicklocks { .. }
        | E::StStrength { .. }
        | E::StIntellect { .. }
        | E::StWillpower { .. }
        | E::StPoisoned
        | E::Carrying { .. }
        | E::CarryingNothing
        | E::KeysLine
        | E::Wealth { .. }
        | E::Encumbrance { .. }
        | E::PartyHeader
        | E::PartyFollowing { .. }
        | E::PartyRow(_)
        | E::PartyRowInvited { .. }
        | E::PartyNone
        | E::ProfileStatline { .. }
        | E::ExpLine { .. }
        | E::TelepathSent { .. }
        | E::TelepathNotSent => Kind::Drop,
        _ => Kind::Keep("text"),
    }
}
