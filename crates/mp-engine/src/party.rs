//! Joining the leader's party (design §6.5), as pure logic: parsed items and a clock in,
//! lines and telepaths out.
//!
//! Game behaviour it relies on (silvermere's `display_party` and the follow and invite
//! commands, after Stock 1.11p):
//! - `set follow normal` gives full room displays while the leader drags the bot along.
//! - `par` prints `You are following <leader>.` first when the player follows someone, then
//!   the player's followers (or `You are not in a party at the present time.`), and **always
//!   ends with the player's own row**. That row marks the end of the answer.
//! - Following needs an invitation, and both inviting and following need the two players in
//!   the same room: `You don't see that anywhere!` from `follow` means the leader isn't here.
//! - `You are now following <leader>` (no full stop) means the bot is in.

use std::time::{Duration, Instant};

use mp_parse::{Event, Item};

use crate::pacing::Action;

/// Whom to follow.
#[derive(Debug, Clone)]
pub struct PartySettings {
    /// The bot's own first name.
    pub name: String,
    /// The leader's first name.
    pub leader: String,
    /// How long an ask may go unanswered, or the bot wait away from the leader, before it
    /// checks `par` again.
    pub retry: Duration,
}

/// Where the join stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartyState {
    /// `par` sent; reading the answer.
    Checking,
    /// `@invite` sent to the leader; waiting for the invitation.
    Asking,
    /// Invited; `follow <leader>` sent.
    Invited,
    /// In the leader's party.
    Following,
    /// The leader isn't in this room; waiting before checking again.
    NotWithLeader,
}

#[derive(Debug)]
pub struct PartyJoin {
    set: PartySettings,
    state: PartyState,
    /// While checking: whether `par` said the bot follows the leader.
    follows_leader: bool,
    /// When the bot checks again, while asking, invited, or away from the leader.
    retry_at: Option<Instant>,
}

impl PartyJoin {
    pub fn new(set: PartySettings) -> Self {
        Self {
            set,
            state: PartyState::Checking,
            follows_leader: false,
            retry_at: None,
        }
    }

    pub fn state(&self) -> PartyState {
        self.state
    }

    /// The leader being joined.
    pub fn leader(&self) -> &str {
        &self.set.leader
    }

    fn is_leader(&self, who: &str) -> bool {
        who.eq_ignore_ascii_case(&self.set.leader)
    }

    /// In the game: ask for full room displays, then look at the party.
    pub fn start(&mut self, _now: Instant) -> Vec<Action> {
        let mut out = vec![Action::Send("set follow normal".into())];
        out.extend(self.check());
        out
    }

    fn check(&mut self) -> Vec<Action> {
        self.state = PartyState::Checking;
        self.follows_leader = false;
        self.retry_at = None;
        vec![Action::Send("par".into())]
    }

    fn ask(&mut self, now: Instant) -> Vec<Action> {
        self.state = PartyState::Asking;
        self.retry_at = Some(now + self.set.retry);
        vec![Action::Telepath {
            to: self.set.leader.clone(),
            text: "@invite".into(),
        }]
    }

    pub fn on_item(&mut self, item: &Item, now: Instant) -> Vec<Action> {
        let Item::Event(e) = item else {
            return Vec::new();
        };
        match (self.state, e) {
            (_, Event::NowFollowing { who }) if self.is_leader(who) => {
                self.state = PartyState::Following;
                self.retry_at = None;
            }
            (PartyState::Checking, Event::PartyFollowing { who }) => {
                self.follows_leader = self.is_leader(who);
            }
            // The own row ends the answer.
            (PartyState::Checking, Event::PartyRow(row))
                if row
                    .name
                    .split_whitespace()
                    .next()
                    .is_some_and(|f| f.eq_ignore_ascii_case(&self.set.name)) =>
            {
                if self.follows_leader {
                    self.state = PartyState::Following;
                } else {
                    return self.ask(now);
                }
            }
            (PartyState::Following, _) => {}
            (_, Event::InvitedYouFollow { who }) if self.is_leader(who) => {
                self.state = PartyState::Invited;
                self.retry_at = Some(now + self.set.retry);
                return vec![Action::Send(format!("follow {}", self.set.leader))];
            }
            (PartyState::Invited, Event::InviteFirst) => return self.ask(now),
            (PartyState::Invited, Event::FollowNothing) => {
                self.state = PartyState::NotWithLeader;
                self.retry_at = Some(now + self.set.retry);
            }
            _ => {}
        }
        if self.state == PartyState::Following {
            let lost = match e {
                Event::NoLongerFollowing { who } => self.is_leader(who),
                Event::FollowerRemoved { who } => who.eq_ignore_ascii_case(&self.set.name),
                _ => false,
            };
            if lost {
                return self.check();
            }
        }
        Vec::new()
    }

    /// Checks `par` again once an ask or a wait has gone on too long.
    pub fn on_tick(&mut self, now: Instant) -> Vec<Action> {
        match self.retry_at {
            Some(at) if now >= at && self.state != PartyState::Following => self.check(),
            _ => Vec::new(),
        }
    }
}
