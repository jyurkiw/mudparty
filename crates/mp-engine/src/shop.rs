//! Gear (#55), as pure logic: what the bot carries, read from the inventory display, and
//! buying an item and putting it to use.
//!
//! Game behaviour it relies on (silvermere `display/inventory.rs`, `items/shop.rs`):
//! - INVENTORY prints `You are carrying ` and the items joined by `, ` (or `Nothing!`),
//!   wrapped at 79 columns, then the keys line, `Wealth:` and `Encumbrance:`. A worn item
//!   is named with its place (`padded vest (Torso)`), the weapon with `(Weapon Hand)` or
//!   `(Two handed)`, a readied item with `(Readied/n)`, and duplicates with a count in front
//!   (`2 torch`).
//! - `buy <item>` answers `You just bought <item> for <coins>.`, or `You cannot afford
//!   <item>.`, or another refusal (not sold here, not known, too heavy).
//! - `wield <item>` answers `You are now holding <item>.`, `wear <item>` `You are now
//!   wearing <item>.`, or a refusal (not allowed, no room, not carried).

use std::time::{Duration, Instant};

use mp_config::GearUse;
use mp_parse::{Event, Item};

/// One entry of the carried list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Carried {
    pub name: String,
    pub count: u32,
    /// Worn, wielded or readied.
    pub equipped: bool,
}

/// The words after `You are carrying `, as entries.
pub fn parse_carried(text: &str) -> Vec<Carried> {
    text.split(", ")
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(|e| {
            // `padded vest (Torso)`: in use.
            let (name, equipped) = match e.rfind(" (") {
                Some(i) if e.ends_with(')') => (&e[..i], true),
                _ => (e, false),
            };
            // `2 torch`: a count in front.
            let (count, name) = match name.split_once(' ') {
                Some((n, rest)) if n.chars().all(|c| c.is_ascii_digit()) => {
                    (n.parse().unwrap_or(1), rest)
                }
                _ => (1, name),
            };
            Carried {
                name: name.trim().to_owned(),
                count,
                equipped,
            }
        })
        .collect()
}

/// Reads the carried list out of an inventory display, wrapped lines and all.
#[derive(Debug, Default)]
pub struct InventoryReader {
    /// The list so far, while it's being read.
    reading: Option<String>,
}

impl InventoryReader {
    /// The next item the game sent; the carried list, once it's whole.
    pub fn on_item(&mut self, item: &Item) -> Option<Vec<Carried>> {
        match item {
            Item::Event(Event::CarryingNothing) => {
                self.reading = None;
                Some(Vec::new())
            }
            Item::Event(Event::Carrying { items }) => {
                self.reading = Some(items.clone());
                None
            }
            // A wrapped line of the list.
            Item::Text(t) if self.reading.is_some() => {
                if let Some(r) = &mut self.reading {
                    r.push(' ');
                    r.push_str(t.trim());
                }
                None
            }
            // The keys line comes next; a prompt or `Wealth:` ends it all the same.
            Item::Event(Event::KeysLine | Event::Wealth { .. }) | Item::Prompt(_) => {
                self.reading.take().map(|r| parse_carried(&r))
            }
            _ => None,
        }
    }
}

/// Where buying and equipping an item stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EquipState {
    Buying,
    Equipping,
    Done,
    Failed(String),
}

/// Buys an item (or not, when it's carried already) and puts it to use.
#[derive(Debug)]
pub struct Equipper {
    item: String,
    how: GearUse,
    wait: Duration,
    state: EquipState,
    since: Instant,
    /// Lines sent again after no answer.
    again: u32,
}

impl Equipper {
    /// `buy`: buy it first. `wait`: how long a line may go unanswered.
    pub fn new(item: &str, how: GearUse, buy: bool, wait: Duration) -> Self {
        Self {
            item: item.to_owned(),
            how,
            wait,
            state: if buy {
                EquipState::Buying
            } else {
                EquipState::Equipping
            },
            since: Instant::now(),
            again: 0,
        }
    }

    pub fn state(&self) -> EquipState {
        self.state.clone()
    }

    pub fn done(&self) -> bool {
        matches!(self.state, EquipState::Done | EquipState::Failed(_))
    }

    fn line(&self) -> Option<String> {
        match self.state {
            EquipState::Buying => Some(format!("buy {}", self.item)),
            EquipState::Equipping => Some(match self.how {
                GearUse::Wield => format!("wield {}", self.item),
                GearUse::Wear => format!("wear {}", self.item),
            }),
            _ => None,
        }
    }

    /// The first line.
    pub fn start(&mut self, now: Instant) -> Vec<String> {
        self.since = now;
        self.line().into_iter().collect()
    }

    fn fail(&mut self, why: impl Into<String>) -> Vec<String> {
        self.state = EquipState::Failed(why.into());
        Vec::new()
    }

    /// The next item the game sent.
    pub fn on_item(&mut self, item: &Item, now: Instant) -> Vec<String> {
        let Item::Event(e) = item else {
            return Vec::new();
        };
        match (&self.state, e) {
            (EquipState::Buying, Event::Bought { .. }) => {
                self.state = EquipState::Equipping;
                self.since = now;
                self.again = 0;
                self.line().into_iter().collect()
            }
            (EquipState::Buying, Event::CannotAfford { .. }) => {
                let why = format!("can't afford {}", self.item);
                self.fail(why)
            }
            (EquipState::Buying, Event::BuyRefused { text })
            | (EquipState::Equipping, Event::EquipRefused { text }) => self.fail(text.clone()),
            (EquipState::Equipping, Event::Equipped { .. }) => {
                self.state = EquipState::Done;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// Time passing: a line with no answer is sent once more, then given up.
    pub fn on_tick(&mut self, now: Instant) -> Vec<String> {
        if self.done() || now.duration_since(self.since) < self.wait {
            return Vec::new();
        }
        if self.again >= 1 {
            return self.fail("the game gave no answer");
        }
        self.again += 1;
        self.since = now;
        self.line().into_iter().collect()
    }
}
