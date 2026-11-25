//! The party as the web page shows it (design §6.8): one card per character, kept up to
//! date from the bots' reports.

use std::collections::{BTreeMap, VecDeque};

use mp_config::{Automation, CharacterConfig};
use mp_engine::{AutoState, Offline, Report, SessionState};
use mp_parse::{Rest, Vitals};
use serde::Serialize;

/// How many of a bot's last events its card shows.
const EVENTS: usize = 8;

/// One character's card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Card {
    pub name: String,
    pub class: Option<String>,
    /// The main character: played in MudPlay, never started as a bot.
    pub main: bool,
    /// It has a login, so it can run as a headless bot.
    pub bot: bool,
    /// It has a session now.
    pub running: bool,
    /// `offline`, `connecting`, `logging_in`, `reconnecting`, `logging_off`, or in the game
    /// `fighting`, `resting`, `waiting` (held by a `@wait`) or `idle`.
    pub mode: &'static str,
    pub vitals: Option<CardVitals>,
    pub room: Option<CardRoom>,
    pub target: Option<String>,
    /// The attack going at the target: a cast code, or `melee`.
    pub tier: Option<String>,
    pub automation: CardAutomation,
    /// Where joining the leader's party stands.
    pub party: Option<String>,
    /// Why the bot last went offline.
    pub offline: Option<String>,
    /// The file was edited and doesn't read; the bot keeps its old config.
    pub config_error: Option<String>,
    /// The last few things it did, oldest first. A line sent to the game starts `> `.
    pub events: VecDeque<String>,
    #[serde(skip)]
    state: Option<SessionState>,
    #[serde(skip)]
    resting: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CardVitals {
    pub hp: i32,
    pub max_hp: Option<i32>,
    pub mp: Option<i32>,
    pub max_mp: Option<i32>,
    /// `standing`, `resting` or `meditating`.
    pub rest: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CardRoom {
    pub map: u32,
    pub room: u32,
    /// From the game data; `None` for a room it doesn't have.
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CardAutomation {
    pub combat: bool,
    pub nuke: bool,
    pub rest: bool,
    pub all_off: bool,
    pub stopped: bool,
    pub held_by: Vec<String>,
}

impl From<Automation> for CardAutomation {
    fn from(a: Automation) -> Self {
        Self {
            combat: a.combat,
            nuke: a.nuke,
            rest: a.rest,
            all_off: false,
            stopped: false,
            held_by: Vec::new(),
        }
    }
}

impl From<AutoState> for CardAutomation {
    fn from(a: AutoState) -> Self {
        Self {
            combat: a.combat,
            nuke: a.nuke,
            rest: a.rest,
            all_off: a.all_off,
            stopped: a.stopped,
            held_by: a.held_by,
        }
    }
}

impl Card {
    pub fn new(c: &CharacterConfig, main: bool) -> Self {
        let mut card = Self {
            name: c.name.clone(),
            class: None,
            main,
            bot: false,
            running: false,
            mode: "offline",
            vitals: None,
            room: None,
            target: None,
            tier: None,
            automation: c.automation.into(),
            party: None,
            offline: None,
            config_error: None,
            events: VecDeque::new(),
            state: None,
            resting: false,
        };
        card.configure(c);
        card
    }

    /// What the card shows from the character's file.
    pub fn configure(&mut self, c: &CharacterConfig) {
        self.class = c.class.clone();
        self.bot = c.login.is_some();
        if !self.running {
            self.automation = c.automation.into();
        }
        self.config_error = None;
    }

    /// The bot was started: a fresh session, so nothing of the last one is current.
    pub fn started(&mut self, automation: Automation) {
        self.running = true;
        self.state = Some(SessionState::Connecting);
        self.vitals = None;
        self.room = None;
        self.target = None;
        self.tier = None;
        self.party = None;
        self.offline = None;
        self.resting = false;
        self.automation = automation.into();
        self.event("started".into());
        self.refresh();
    }

    pub fn event(&mut self, e: String) {
        if self.events.len() == EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(e);
    }

    /// One report from the bot. `room_name` looks a room up in the game data.
    pub fn report(&mut self, r: &Report, room_name: impl Fn(u32, u32) -> Option<String>) {
        match r {
            Report::State(s) => {
                match s {
                    SessionState::InGame => self.event("in the game".into()),
                    SessionState::Reconnecting { after } => {
                        self.event(format!("link lost; again in {}s", after.as_secs()));
                    }
                    SessionState::Offline(off) => {
                        self.running = false;
                        self.target = None;
                        self.tier = None;
                        self.resting = false;
                        let why = offline_text(off);
                        self.event(format!("offline: {why}"));
                        self.offline = Some(why);
                    }
                    _ => {}
                }
                self.state = Some(s.clone());
            }
            Report::Vitals(v) => self.vitals = Some(vitals(v)),
            Report::ConfigApplied { rev } => {
                self.config_error = None;
                self.event(format!("config {}", &rev[..rev.len().min(8)]));
            }
            Report::Sent(line) => self.event(format!("> {line}")),
            Report::Party(p) => {
                let p = format!("{p:?}");
                self.event(format!("party: {p}"));
                self.party = Some(p);
            }
            Report::Resting(on) => {
                self.resting = *on;
                self.event(if *on { "resting" } else { "rested" }.into());
            }
            Report::Automation(a) => self.automation = a.clone().into(),
            Report::Fight { target, tier } => {
                if target.is_some() && *target != self.target {
                    self.event(format!("fighting {}", target.as_deref().unwrap_or("")));
                }
                self.target = target.clone();
                self.tier = tier.clone();
            }
            Report::Room { map, room } => {
                self.room = Some(CardRoom {
                    map: *map,
                    room: *room,
                    name: room_name(*map, *room),
                });
            }
        }
        self.refresh();
    }

    fn refresh(&mut self) {
        self.mode = match (&self.state, self.running) {
            (_, false) => "offline",
            (Some(SessionState::InGame), true) => {
                if self.target.is_some() {
                    "fighting"
                } else if self.resting {
                    "resting"
                } else if !self.automation.held_by.is_empty() {
                    "waiting"
                } else {
                    "idle"
                }
            }
            (Some(SessionState::LoggingIn), true) => "logging_in",
            (Some(SessionState::LoggingOff), true) => "logging_off",
            (Some(SessionState::Reconnecting { .. }), true) => "reconnecting",
            _ => "connecting",
        };
    }
}

fn vitals(v: &Vitals) -> CardVitals {
    CardVitals {
        hp: v.hp,
        max_hp: v.max_hp,
        mp: v.mp,
        max_mp: v.max_mp,
        rest: match v.rest {
            Rest::Standing => "standing",
            Rest::Resting => "resting",
            Rest::Meditating => "meditating",
        },
    }
}

fn offline_text(off: &Offline) -> String {
    match off {
        Offline::LoggedOff => "logged off".into(),
        Offline::LoginFailed(f) => format!("login failed: {f}"),
        Offline::TakenOver => "taken over by another login".into(),
        Offline::LogoffTimedOut => "the logoff timed out".into(),
        Offline::Dropped => "dropped".into(),
    }
}

/// Every card, the main character first, then by name.
pub fn ordered(cards: &BTreeMap<String, Card>) -> Vec<Card> {
    let mut out: Vec<Card> = cards.values().cloned().collect();
    out.sort_by_key(|c| !c.main);
    out
}
