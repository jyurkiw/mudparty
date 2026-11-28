//! Character sheets (#10, design §6.9, §15.3), as pure logic: the status display (`st`) and
//! the inventory (`i`), read whenever their output arrives, whoever asked for it.
//!
//! Game behaviour it relies on (silvermere `display/status.rs`, `display/inventory.rs`):
//! - STATUS prints `Name:` first, then race and experience, class and level, hits and armour,
//!   the magery line (`Mana:`, `Kai:`, or `Traps:` alone for a class without magic),
//!   `Picklocks:`, and three lines of stats, each stat with a red `*` when it differs from
//!   its base. `You are Poisoned!` may follow. The prompt ends it.
//! - INVENTORY prints `You are carrying ` and the coins, highest first (runic, platinum,
//!   gold, silver, copper), then the items, joined by `, ` and wrapped at 79 columns (or
//!   `Nothing!`). Then the keys line, `Wealth: <copper> copper farthings` and
//!   `Encumbrance: <carried>/<most> - <level> [<pct>%]`, coins counted in the weight.
//!   A worn item is named with its place (`padded vest (Torso)`), the weapon with
//!   `(Weapon Hand)` or `(Two handed)`, a lit light with `(Readied/<uses>)`, and duplicates
//!   with a count in front (`2 torch`).
//! - Three coins of a type weigh 1, each type counted on its own (`economy::coin_weight`).

use mp_parse::{Event, Item};
use serde::Serialize;

/// A stat, and whether the game marks it as raised or lowered from its base.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Stat {
    pub value: i64,
    pub marked: bool,
}

/// The magery line of a class with magic: mana, or kai.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Magic {
    /// `Mana` or `Kai`.
    pub kind: String,
    /// The Mana ability's red star.
    pub marked: bool,
    pub mp: i64,
    pub max_mp: i64,
    /// Not shown for kai.
    pub spellcasting: Option<i64>,
}

/// The status display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Status {
    /// First and last name.
    pub name: String,
    pub lives: i64,
    pub cp: i64,
    pub race: String,
    pub exp: i64,
    pub perception: i64,
    pub class: String,
    pub level: i64,
    pub stealth: i64,
    pub hp: i64,
    pub max_hp: i64,
    /// Armour class and damage resistance, as shown.
    pub ac: i64,
    pub dr: i64,
    pub thievery: i64,
    /// `None` for a class without magic.
    pub magic: Option<Magic>,
    pub traps: i64,
    pub picklocks: i64,
    pub strength: Stat,
    pub agility: Stat,
    pub intellect: Stat,
    pub health: Stat,
    pub willpower: Stat,
    pub charm: Stat,
    pub tracking: i64,
    pub martial_arts: i64,
    pub magic_res: i64,
    pub poisoned: bool,
}

/// Coins by type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Coins {
    pub runic: u64,
    pub platinum: u64,
    pub gold: u64,
    pub silver: u64,
    pub copper: u64,
}

impl Coins {
    /// Their value in copper: a runic coin is 100 platinum, a platinum 100 gold, a gold 10
    /// silver, a silver 10 copper (silvermere's defaults).
    pub fn copper_value(&self) -> u64 {
        ((self.runic * 100 + self.platinum) * 100 + self.gold) * 100
            + self.silver * 10
            + self.copper
    }

    /// Their weight: 3 coins of a type weigh 1, each type on its own.
    pub fn weight(&self) -> u64 {
        self.runic / 3 + self.platinum / 3 + self.gold / 3 + self.silver / 3 + self.copper / 3
    }

    /// The coin a name in the carried list names, by its singular or plural: `copper
    /// farthing(s)`, `silver noble(s)`, `gold crown(s)`, `platinum piece(s)`, `runic
    /// coin(s)`. A realm that renames its top coin isn't recognised.
    fn slot(&mut self, name: &str) -> Option<&mut u64> {
        let n = name.trim().to_ascii_lowercase();
        let n = n.strip_suffix('s').unwrap_or(&n);
        match n {
            "copper farthing" => Some(&mut self.copper),
            "silver noble" => Some(&mut self.silver),
            "gold crown" => Some(&mut self.gold),
            "platinum piece" => Some(&mut self.platinum),
            "runic coin" => Some(&mut self.runic),
            _ => None,
        }
    }
}

/// One entry of the carried list, coins left out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SheetItem {
    pub name: String,
    pub count: u32,
    /// Where it's worn or held (`Torso`, `Weapon Hand`, `Readied/80`), if it is.
    pub place: Option<String>,
}

/// The inventory's weight line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Encumbrance {
    /// Weight carried, coins included.
    pub cur: i64,
    /// The most that can be carried.
    pub max: i64,
    /// `None`, `Light`, `Medium`, `Heavy`: as the game words it.
    pub level: String,
    pub pct: i64,
}

/// The inventory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Inventory {
    pub coins: Coins,
    pub items: Vec<SheetItem>,
    /// All the coins' value in copper, as the game says it.
    pub wealth: Option<i64>,
    pub encumbrance: Option<Encumbrance>,
}

impl Inventory {
    /// How many more coins could be carried: 3 to each unit of weight left.
    pub fn coins_room(&self) -> Option<i64> {
        self.encumbrance
            .as_ref()
            .map(|e| ((e.max - e.cur) * 3).max(0))
    }
}

/// Experience, as `exp` shows it, and kept up as kills add to it: for the card's XP bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Experience {
    /// Experience held.
    pub exp: i64,
    pub level: u32,
    /// Still needed for the next level; 0 when it can train.
    pub left: i64,
    /// The next level's total.
    pub need: i64,
}

impl Experience {
    /// How far to the next level's total, as `exp`'s own `[pct%]` reckons it: held over
    /// needed, 0 to 100.
    pub fn pct(&self) -> u32 {
        if self.need <= 0 {
            return 100;
        }
        u32::try_from((self.exp.max(0) * 100 / self.need).clamp(0, 100)).unwrap_or(100)
    }

    /// After a kill that gained `gained`.
    pub fn gain(&mut self, gained: i64) {
        self.exp += gained;
        self.left = (self.left - gained).max(0);
    }
}

/// A sheet part, read whole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "part", rename_all = "snake_case")]
pub enum SheetPart {
    Status(Box<Status>),
    Inventory(Box<Inventory>),
}

/// The status display as far as it's come.
#[derive(Debug, Default)]
struct StatusSoFar {
    name: Option<(String, i64, i64)>,
    race: Option<(String, i64, i64)>,
    class: Option<(String, i64, i64)>,
    hits: Option<(i64, i64, i64, i64, i64)>,
    magic: Option<Magic>,
    traps: i64,
    picklocks: i64,
    strength: Option<(Stat, Stat, i64)>,
    intellect: Option<(Stat, Stat, i64)>,
    willpower: Option<(Stat, Stat, i64)>,
    poisoned: bool,
}

impl StatusSoFar {
    /// The whole display, if every line came.
    fn finish(self) -> Option<Status> {
        let (name, lives, cp) = self.name?;
        let (race, exp, perception) = self.race?;
        let (class, level, stealth) = self.class?;
        let (hp, max_hp, ac, dr, thievery) = self.hits?;
        let (strength, agility, tracking) = self.strength?;
        let (intellect, health, martial_arts) = self.intellect?;
        let (willpower, charm, magic_res) = self.willpower?;
        Some(Status {
            name,
            lives,
            cp,
            race,
            exp,
            perception,
            class,
            level,
            stealth,
            hp,
            max_hp,
            ac,
            dr,
            thievery,
            magic: self.magic,
            traps: self.traps,
            picklocks: self.picklocks,
            strength,
            agility,
            intellect,
            health,
            willpower,
            charm,
            tracking,
            martial_arts,
            magic_res,
            poisoned: self.poisoned,
        })
    }
}

/// The inventory as far as it's come.
#[derive(Debug, Default)]
struct InventorySoFar {
    /// The carried list's words, while it's still coming (until the keys line).
    list: Option<String>,
    inv: Inventory,
}

/// Reads sheet parts out of the game's output.
#[derive(Debug, Default)]
pub struct SheetReader {
    status: Option<StatusSoFar>,
    inventory: Option<InventorySoFar>,
}

impl SheetReader {
    /// The next item the game sent; a part, once it's whole.
    pub fn on_item(&mut self, item: &Item) -> Option<SheetPart> {
        if let Some(part) = self.on_inventory(item) {
            return Some(part);
        }
        self.on_status(item)
    }

    fn on_status(&mut self, item: &Item) -> Option<SheetPart> {
        let stat = |value: &i64, marked: &bool| Stat {
            value: *value,
            marked: *marked,
        };
        match item {
            Item::Event(Event::StName { name, lives, cp }) => {
                self.status = Some(StatusSoFar {
                    name: Some((name.clone(), *lives, *cp)),
                    ..StatusSoFar::default()
                });
                None
            }
            Item::Event(e) => {
                let st = self.status.as_mut()?;
                match e {
                    Event::StRace {
                        race,
                        exp,
                        perception,
                    } => st.race = Some((race.clone(), *exp, *perception)),
                    Event::StClass {
                        class,
                        level,
                        stealth,
                    } => st.class = Some((class.clone(), *level, *stealth)),
                    Event::StHits {
                        hp,
                        max_hp,
                        ac,
                        dr,
                        thievery,
                    } => st.hits = Some((*hp, *max_hp, *ac, *dr, *thievery)),
                    Event::StMagic {
                        kind,
                        marked,
                        mp,
                        max_mp,
                        spellcasting,
                        traps,
                    } => {
                        st.magic = Some(Magic {
                            kind: kind.clone(),
                            marked: *marked,
                            mp: *mp,
                            max_mp: *max_mp,
                            spellcasting: *spellcasting,
                        });
                        st.traps = *traps;
                    }
                    Event::StTraps { traps } => st.traps = *traps,
                    Event::StPicklocks { picklocks } => st.picklocks = *picklocks,
                    Event::StStrength {
                        strength,
                        strength_marked,
                        agility,
                        agility_marked,
                        tracking,
                    } => {
                        st.strength = Some((
                            stat(strength, strength_marked),
                            stat(agility, agility_marked),
                            *tracking,
                        ));
                    }
                    Event::StIntellect {
                        intellect,
                        intellect_marked,
                        health,
                        health_marked,
                        martial_arts,
                    } => {
                        st.intellect = Some((
                            stat(intellect, intellect_marked),
                            stat(health, health_marked),
                            *martial_arts,
                        ));
                    }
                    Event::StWillpower {
                        willpower,
                        willpower_marked,
                        charm,
                        charm_marked,
                        magic_res,
                    } => {
                        st.willpower = Some((
                            stat(willpower, willpower_marked),
                            stat(charm, charm_marked),
                            *magic_res,
                        ));
                    }
                    Event::StPoisoned => st.poisoned = true,
                    _ => {}
                }
                None
            }
            // The prompt ends the display: whole, or dropped.
            Item::Prompt(_) => self
                .status
                .take()
                .and_then(StatusSoFar::finish)
                .map(|s| SheetPart::Status(Box::new(s))),
            _ => None,
        }
    }

    fn on_inventory(&mut self, item: &Item) -> Option<SheetPart> {
        match item {
            Item::Event(Event::Carrying { items }) => {
                self.inventory = Some(InventorySoFar {
                    list: Some(items.clone()),
                    inv: Inventory::default(),
                });
                None
            }
            Item::Event(Event::CarryingNothing) => {
                self.inventory = Some(InventorySoFar::default());
                None
            }
            // A wrapped line of the list.
            Item::Text(t) => {
                if let Some(list) = self.inventory.as_mut().and_then(|i| i.list.as_mut()) {
                    list.push(' ');
                    list.push_str(t.trim());
                }
                None
            }
            Item::Event(Event::KeysLine) => {
                let so_far = self.inventory.as_mut()?;
                if let Some(list) = so_far.list.take() {
                    read_list(&list, &mut so_far.inv);
                }
                None
            }
            Item::Event(Event::Wealth { copper }) => {
                let so_far = self.inventory.as_mut()?;
                if let Some(list) = so_far.list.take() {
                    read_list(&list, &mut so_far.inv);
                }
                so_far.inv.wealth = Some(*copper);
                None
            }
            Item::Event(Event::Encumbrance {
                cur,
                max,
                level,
                pct,
            }) => {
                let mut so_far = self.inventory.take()?;
                if let Some(list) = so_far.list.take() {
                    read_list(&list, &mut so_far.inv);
                }
                so_far.inv.encumbrance = Some(Encumbrance {
                    cur: *cur,
                    max: *max,
                    level: level.clone(),
                    pct: *pct,
                });
                Some(SheetPart::Inventory(Box::new(so_far.inv)))
            }
            // A prompt ends it too, once its wealth has come (an older game with no weight
            // line); before that, it was cut short.
            Item::Prompt(_) => {
                let so_far = self.inventory.take()?;
                so_far
                    .inv
                    .wealth
                    .is_some()
                    .then_some(SheetPart::Inventory(Box::new(so_far.inv)))
            }
            _ => None,
        }
    }
}

/// The carried list's words into coins and items.
fn read_list(list: &str, inv: &mut Inventory) {
    for entry in list.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        // `padded vest (Torso)`: worn there.
        let (rest, place) = match entry.rfind(" (") {
            Some(i) if entry.ends_with(')') => {
                (&entry[..i], Some(entry[i + 2..entry.len() - 1].to_owned()))
            }
            _ => (entry, None),
        };
        // `2 torch`: a count in front.
        let (count, name) = match rest.split_once(' ') {
            Some((n, name)) if n.chars().all(|c| c.is_ascii_digit()) => {
                (n.parse::<u64>().unwrap_or(1), name.trim())
            }
            _ => (1, rest.trim()),
        };
        if place.is_none()
            && let Some(slot) = inv.coins.slot(name)
        {
            *slot += count;
            continue;
        }
        inv.items.push(SheetItem {
            name: name.to_owned(),
            count: u32::try_from(count).unwrap_or(u32::MAX),
            place,
        });
    }
}
