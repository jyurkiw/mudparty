//! The compiled room file (#63, design §14.2-14.4): one `places/<area>.yaml` per area,
//! compiled from silvermere's database and never edited, with a hand-written
//! `<area>.local.yaml` beside it for aliases. Scripts name rooms against these files
//! ([`Places::resolve`]).
//!
//! Areas follow MajorMUD's `Area, Room` names. Most rooms have no such prefix (20,000 of
//! the stock world's 26,720), so a room without one joins the area of the nearest room
//! that has one, by walking distance; a room no named room reaches goes in `map-<n>`.
//!
//! Silvermere's records are read as they are, codes and all (see [`crate::RoomExit`] for
//! the exits' parameter slots). The few fields whose meaning depends on the room are
//! explained where they are read.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fmt;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

use crate::DataError;

/// The first line of every compiled file.
const HEADER: &str = "# Compiled from silvermere.db by `mudparty-server places compile`. Don't edit:\n# names and notes go in the area's .local.yaml beside it.\n";

/// One area's rooms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Area {
    pub schema: u32,
    /// The area's slug: `newhaven`, `stone-tunnel`, `map-3`.
    pub area: String,
    pub source: Source,
    /// By slug.
    pub rooms: BTreeMap<String, Room>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub silvermere_db: String,
}

/// A room, as scripts need to know it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Room {
    /// `1/2150`.
    pub key: String,
    /// The game's own name: `Newhaven, Arena`.
    pub name: String,
    /// `arena`, `lair`, `shop`, `hotel`, `colliseum` (sic), `jail` or `library`; none for
    /// a normal room.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// An arena's player levels, `1-3`: a character above them may not enter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub levels: Option<String>,
    /// By direction word: `north` … `down`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub exits: BTreeMap<String, Exit>,
    /// What turns up in a normal room or an arena now and then, while players are there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawns: Option<Spawns>,
    /// A lair: it fills when a player walks in and nobody else is there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lair: Option<Lair>,
    /// The room's own monster, made when a player walks in and it isn't alive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boss: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop: Option<Shop>,
}

/// Where an exit leads: a place (a slug in this area, `area:slug` in another, or a key
/// for a room no area has), plainly or with what stands in the way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Exit {
    To(String),
    /// Boxed: most exits are plain, and a special one is large.
    Special(Box<SpecialExit>),
}

/// An exit that isn't a plain way through.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpecialExit {
    pub to: String,
    /// silvermere's exit kind: `door`, `gate`, `key`, `level`, `block_guard`, `hidden`, …
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock: Option<Lock>,
    /// A door's or gate's bash (and pick) bonus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bash: Option<i64>,
    /// The key item that opens it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// A level exit's range, `1-3`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub levels: Option<String>,
    /// An alignment exit's range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alignment: Option<String>,
    /// A toll's gold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toll: Option<i64>,
    /// The item an item exit needs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    /// A class or race exit's one class or race allowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub only: Option<String>,
    /// A class or race exit's one class or race barred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub barred: Option<String>,
    /// silvermere's four parameter slots, kept as they are, for the kinds read no further
    /// here (text, remote actions, traps, timed exits, …); see [`crate::RoomExit`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<[i64; 4]>,
}

/// A door's, gate's or key exit's lock state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lock {
    Open,
    Closed,
    Locked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spawns {
    pub monsters: Vec<String>,
    /// At most this many monsters in the room.
    pub max: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lair {
    pub monsters: Vec<String>,
    pub max: i64,
    /// How long after the room's last kill it can fill again: `2m`, or `server default`
    /// (the server's `regen_mins`) when the room sets none.
    pub respawn: String,
    /// A unique monster's (`maxalive` 1) wait after it was killed, in hours.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unique_hours: BTreeMap<String, i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shop {
    pub name: String,
    /// `general`, `weapons`, `armour`, `items`, `spells`, `hospital`, `tavern`, `bank`,
    /// `training`, `inn`, `specific`, `gang_shop` or `deed_shop`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The items it sells, in its own order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sells: Vec<String>,
    /// A training shop's class: `any class`, or the class's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trains: Option<String>,
}

// ----- silvermere's codes (silvermere-types `enums.rs`)

const DIRS: [&str; 10] = [
    "north",
    "south",
    "east",
    "west",
    "northeast",
    "northwest",
    "southeast",
    "southwest",
    "up",
    "down",
];

/// `EXIT_` codes, 0 to 24.
const EXIT_KINDS: [&str; 25] = [
    "normal",
    "spell",
    "key",
    "item",
    "toll",
    "action",
    "hidden",
    "door",
    "map_change",
    "trap",
    "text",
    "gate",
    "remote_action",
    "class",
    "race",
    "level",
    "timed",
    "ticket",
    "user_count",
    "block_guard",
    "alignment",
    "delay",
    "cast",
    "ability",
    "spell_trap",
];

/// `ROOM_` codes, 0 to 7; a normal room (0) has no `type:`.
const ROOM_KINDS: [&str; 8] = [
    "normal",
    "shop",
    "arena",
    "lair",
    "hotel",
    "colliseum",
    "jail",
    "library",
];

/// `SHOP_` codes, 0 to 12.
const SHOP_KINDS: [&str; 13] = [
    "general",
    "weapons",
    "armour",
    "items",
    "spells",
    "hospital",
    "tavern",
    "bank",
    "training",
    "inn",
    "specific",
    "gang_shop",
    "deed_shop",
];

/// An `EXIT_` code's name as the room file writes it: `door`, `block_guard`, ...
pub fn exit_kind_name(code: i64) -> String {
    code_name(&EXIT_KINDS, code)
}

/// Every exit kind's name, as the room file writes them, in `EXIT_` code order.
pub fn exit_kinds() -> &'static [&'static str] {
    &EXIT_KINDS
}

/// The `EXIT_` code of an exit kind's name, as the room file writes it.
pub fn exit_kind_code(name: &str) -> Option<i64> {
    EXIT_KINDS
        .iter()
        .position(|k| k.eq_ignore_ascii_case(name.trim()))
        .and_then(|i| i64::try_from(i).ok())
}

fn code_name(table: &[&str], code: i64) -> String {
    usize::try_from(code)
        .ok()
        .and_then(|i| table.get(i))
        .map_or_else(|| format!("code {code}"), |s| (*s).to_owned())
}

/// `Stone Tunnel` → `stone-tunnel`, `Adventurer's Guild` → `adventurers-guild`.
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if c == '\'' {
            // dropped: `adventurers`, not `adventurer-s`
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}

/// The `Area` of an `Area, Room` name.
fn prefix(name: &str) -> Option<&str> {
    name.split_once(", ").map(|(a, _)| a)
}

type Key = (i64, i64);

fn key_text((m, r): Key) -> String {
    format!("{m}/{r}")
}

struct RawRoom {
    name: String,
    room_type: i64,
    shopnum: i64,
    minlevel: i64,
    maxlevel: i64,
    mongroup: i64,
    genmonster: i64,
    maxmonsters: i64,
    regendelay: i64,
    specmon: i64,
    /// (direction, target, exit_type, par1..par4)
    exits: Vec<(usize, Key, i64, [i64; 4])>,
}

struct RawMonster {
    number: i64,
    name: String,
    montype: i64,
    genindex: i64,
    maxalive: i64,
    regenhrs: i64,
}

/// Compiles every area of `silvermere_db`, opened read-only.
pub fn compile(silvermere_db: &Path) -> Result<Vec<Area>, DataError> {
    let db = Connection::open_with_flags(silvermere_db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(crate::err(silvermere_db))?;
    let e = crate::err(silvermere_db);
    let mut ids: HashMap<i64, Key> = HashMap::new();
    let mut rooms: BTreeMap<Key, RawRoom> = BTreeMap::new();
    {
        let mut q = db
            .prepare(
                "SELECT id, map, room, name, room_type, shopnum, minlevel, maxlevel, mongroup,
                        genmonster, maxmonsters, regendelay, specmon
                 FROM rooms",
            )
            .map_err(&e)?;
        let rows = q
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    (r.get::<_, i64>(1)?, r.get::<_, i64>(2)?),
                    RawRoom {
                        name: r.get(3)?,
                        room_type: r.get(4)?,
                        shopnum: r.get(5)?,
                        minlevel: r.get(6)?,
                        maxlevel: r.get(7)?,
                        mongroup: r.get(8)?,
                        genmonster: r.get(9)?,
                        maxmonsters: r.get(10)?,
                        regendelay: r.get(11)?,
                        specmon: r.get(12)?,
                        exits: Vec::new(),
                    },
                ))
            })
            .map_err(&e)?;
        for row in rows {
            let (id, key, room) = row.map_err(&e)?;
            ids.insert(id, key);
            rooms.insert(key, room);
        }
    }
    {
        let mut q = db
            .prepare(
                "SELECT room_id, direction, target, exit_type, par1, par2, par3, par4
                 FROM room_exits WHERE target <> 0",
            )
            .map_err(&e)?;
        let rows = q
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    [r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?],
                ))
            })
            .map_err(&e)?;
        for row in rows {
            let (id, dir, target, exit_type, par) = row.map_err(&e)?;
            let Some(&from) = ids.get(&id) else { continue };
            // A Map Change keeps the map it leads into in `par1`.
            let to_map = if exit_type == 8 { par[0] } else { from.0 };
            if let (Some(room), Ok(dir)) = (rooms.get_mut(&from), usize::try_from(dir))
                && dir < DIRS.len()
            {
                room.exits.push((dir, (to_map, target), exit_type, par));
            }
        }
    }
    let monsters: Vec<RawMonster> = {
        let mut q = db
            .prepare(
                "SELECT number, name, montype, genindex, maxalive, regenhrs
                 FROM monsters ORDER BY number",
            )
            .map_err(&e)?;
        q.query_map([], |r| {
            Ok(RawMonster {
                number: r.get(0)?,
                name: r.get(1)?,
                montype: r.get(2)?,
                genindex: r.get(3)?,
                maxalive: r.get(4)?,
                regenhrs: r.get(5)?,
            })
        })
        .map_err(&e)?
        .collect::<Result<_, _>>()
        .map_err(&e)?
    };
    let names = |table: &str| -> Result<HashMap<i64, String>, DataError> {
        let mut q = db
            .prepare(&format!("SELECT number, name FROM {table}"))
            .map_err(&e)?;
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(&e)?
            .collect::<Result<_, _>>()
            .map_err(&e)
    };
    let items = names("items")?;
    let classes = names("classes")?;
    let races = names("races")?;
    let mut shops: HashMap<i64, Shop> = HashMap::new();
    {
        let mut q = db
            .prepare("SELECT number, name, shop_type, trainclass FROM shops")
            .map_err(&e)?;
        let rows = q
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })
            .map_err(&e)?;
        for row in rows {
            let (n, name, shop_type, trainclass) = row.map_err(&e)?;
            let kind = code_name(&SHOP_KINDS, shop_type);
            let trains = (kind == "training").then(|| match trainclass {
                0 => "any class".to_owned(),
                c => classes
                    .get(&c)
                    .cloned()
                    .unwrap_or_else(|| format!("class {c}")),
            });
            shops.insert(
                n,
                Shop {
                    name,
                    kind,
                    sells: Vec::new(),
                    trains,
                },
            );
        }
        let mut q = db
            .prepare("SELECT shop, item FROM shop_items ORDER BY shop, slot")
            .map_err(&e)?;
        let rows = q
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
            .map_err(&e)?;
        for row in rows {
            let (shop, item) = row.map_err(&e)?;
            if let (Some(s), Some(name)) = (shops.get_mut(&shop), items.get(&item)) {
                s.sells.push(name.clone());
            }
        }
    }
    drop(db);

    let area_of = areas(&rooms);
    // Slugs, one area at a time: a name the area repeats takes its room number.
    let mut slugs: HashMap<Key, String> = HashMap::new();
    let mut members: BTreeMap<&str, Vec<Key>> = BTreeMap::new();
    for (k, a) in &area_of {
        members.entry(a.as_str()).or_default().push(*k);
    }
    for (area, keys) in &members {
        let base = |k: &Key| {
            let name = &rooms[k].name;
            let s = match name.split_once(", ") {
                Some((p, rest)) if slug(p) == *area => slug(rest),
                _ => slug(name),
            };
            // A name with no letters or digits in it.
            if s.is_empty() { "room".to_owned() } else { s }
        };
        let mut count: HashMap<String, usize> = HashMap::new();
        for k in keys {
            *count.entry(base(k)).or_default() += 1;
        }
        let mut taken: HashMap<String, usize> = HashMap::new();
        for k in keys {
            let b = base(k);
            let s = if count[&b] > 1 {
                format!("{b}-{}", k.1)
            } else {
                b.clone()
            };
            *taken.entry(s.clone()).or_default() += 1;
            slugs.insert(*k, s);
        }
        // The same room number on two maps of one area: the map too.
        for k in keys {
            if taken[&slugs[k]] > 1 {
                let s = format!("{}-{}-{}", base(k), k.0, k.1);
                slugs.insert(*k, s);
            }
        }
    }
    let place = |from_area: &str, to: Key| -> String {
        match (area_of.get(&to), slugs.get(&to)) {
            (Some(a), Some(s)) if a == from_area => s.clone(),
            (Some(a), Some(s)) => format!("{a}:{s}"),
            _ => key_text(to),
        }
    };
    let source = Source {
        silvermere_db: silvermere_db.display().to_string().replace('\\', "/"),
    };
    let mut out: BTreeMap<String, Area> = BTreeMap::new();
    for (key, raw) in &rooms {
        let area = &area_of[key];
        let room = compile_room(
            *key, raw, area, &place, &monsters, &shops, &items, &classes, &races,
        );
        out.entry(area.clone())
            .or_insert_with(|| Area {
                schema: 1,
                area: area.clone(),
                source: source.clone(),
                rooms: BTreeMap::new(),
            })
            .rooms
            .insert(slugs[key].clone(), room);
    }
    Ok(out.into_values().collect())
}

/// Each room's area: its name's prefix, or the nearest prefixed room's by walking
/// distance (a breadth-first walk from every prefixed room at once, over exits both
/// ways, in key order so the result never changes), or `map-<n>`.
fn areas(rooms: &BTreeMap<Key, RawRoom>) -> BTreeMap<Key, String> {
    let mut near: HashMap<Key, Vec<Key>> = HashMap::new();
    for (k, r) in rooms {
        for (_, to, _, _) in &r.exits {
            if rooms.contains_key(to) {
                near.entry(*k).or_default().push(*to);
                near.entry(*to).or_default().push(*k);
            }
        }
    }
    for v in near.values_mut() {
        v.sort_unstable();
        v.dedup();
    }
    let mut area: BTreeMap<Key, String> = BTreeMap::new();
    let mut queue = VecDeque::new();
    for (k, r) in rooms {
        if let Some(p) = prefix(&r.name).map(slug).filter(|s| !s.is_empty()) {
            area.insert(*k, p);
            queue.push_back(*k);
        }
    }
    while let Some(k) = queue.pop_front() {
        let a = area[&k].clone();
        for n in near.get(&k).into_iter().flatten() {
            if !area.contains_key(n) {
                area.insert(*n, a.clone());
                queue.push_back(*n);
            }
        }
    }
    for k in rooms.keys() {
        area.entry(*k).or_insert_with(|| format!("map-{}", k.0));
    }
    area
}

#[allow(clippy::too_many_arguments)]
fn compile_room(
    key: Key,
    raw: &RawRoom,
    area: &str,
    place: &dyn Fn(&str, Key) -> String,
    monsters: &[RawMonster],
    shops: &HashMap<i64, Shop>,
    items: &HashMap<i64, String>,
    classes: &HashMap<i64, String>,
    races: &HashMap<i64, String>,
) -> Room {
    let name_of = |table: &HashMap<i64, String>, n: i64, what: &str| {
        table
            .get(&n)
            .cloned()
            .unwrap_or_else(|| format!("{what} {n}"))
    };
    let mut exits = BTreeMap::new();
    for &(dir, to, exit_type, par) in &raw.exits {
        let to = place(area, to);
        let kind = code_name(&EXIT_KINDS, exit_type);
        let plain = |kind: String| SpecialExit {
            to: to.clone(),
            kind,
            lock: None,
            bash: None,
            key: None,
            levels: None,
            alignment: None,
            toll: None,
            item: None,
            only: None,
            barred: None,
            params: None,
        };
        let lock = |code: i64| match code {
            0 => Some(Lock::Open),
            1 => Some(Lock::Closed),
            2 => Some(Lock::Locked),
            _ => None,
        };
        let special = |x: SpecialExit| Exit::Special(Box::new(x));
        let exit = match exit_type {
            // Normal, and a Map Change, which walks like one (its `to` names the map).
            0 | 8 => Exit::To(to.clone()),
            // Door, Gate: lock state, bash/pick bonus, steps it stays open, key item.
            7 | 11 => special(SpecialExit {
                lock: lock(par[0]),
                bash: (par[1] != 0).then_some(par[1]),
                key: (par[3] != 0).then(|| name_of(items, par[3], "item")),
                params: lock(par[0]).is_none().then_some(par),
                ..plain(kind)
            }),
            // Key: key item, lock state, pick bonus, steps it stays open.
            2 => special(SpecialExit {
                key: Some(name_of(items, par[0], "item")),
                lock: lock(par[1]),
                ..plain(kind)
            }),
            // Toll: the gold.
            4 => special(SpecialExit {
                toll: Some(par[0]),
                ..plain(kind)
            }),
            // Item: the item needed.
            3 => special(SpecialExit {
                item: Some(name_of(items, par[0], "item")),
                ..plain(kind)
            }),
            // Level, Alignment: minimum, maximum.
            15 => special(SpecialExit {
                levels: Some(format!("{}-{}", par[0], par[1])),
                ..plain(kind)
            }),
            20 => special(SpecialExit {
                alignment: Some(format!("{}-{}", par[0], par[1])),
                ..plain(kind)
            }),
            // Class, Race: the only one allowed, the one barred.
            13 | 14 => {
                let table = if exit_type == 13 { classes } else { races };
                let what = if exit_type == 13 { "class" } else { "race" };
                special(SpecialExit {
                    only: (par[0] != 0).then(|| name_of(table, par[0], what)),
                    barred: (par[1] != 0).then(|| name_of(table, par[1], what)),
                    ..plain(kind)
                })
            }
            // Ticket, User Count, Block Guard, Delay: walked like a plain exit.
            17 | 18 | 19 | 21 => special(plain(kind)),
            // The rest, read no further here: the slots as they are.
            _ => special(SpecialExit {
                params: Some(par),
                ..plain(kind)
            }),
        };
        exits.insert(DIRS[dir].to_owned(), exit);
    }
    // The spawn fields (silvermere `generate.rs`, `Request::for_room`): `mongroup` is the
    // `montype` the pool is drawn from (-1 any), and `minlevel`..`maxlevel` the monsters'
    // `genindex` range (0 no limit; a pool needs a non-zero maximum). `genmonster` names
    // one monster instead. These are the same `minlevel` / `maxlevel` an arena reads as its
    // players' levels: one pair of legacy fields read two ways, kept as they are.
    let pool: Vec<&RawMonster> = if raw.genmonster != 0 {
        monsters
            .iter()
            .filter(|m| m.number == raw.genmonster)
            .collect()
    } else if raw.maxlevel == 0 {
        Vec::new()
    } else {
        monsters
            .iter()
            .filter(|m| {
                (raw.mongroup == -1 || m.montype == raw.mongroup)
                    && (raw.minlevel == 0 || m.genindex >= raw.minlevel)
                    && m.genindex <= raw.maxlevel
            })
            .collect()
    };
    // Each name once: two monsters may share one (the stock world has two giant rats).
    let mut pool_names: Vec<String> = Vec::new();
    for m in &pool {
        if !pool_names.contains(&m.name) {
            pool_names.push(m.name.clone());
        }
    }
    let room_kind = code_name(&ROOM_KINDS, raw.room_type);
    let lair = (raw.room_type == 3 && !pool.is_empty()).then(|| Lair {
        monsters: pool_names.clone(),
        max: raw.maxmonsters,
        // The delay is minutes since the room's last kill (`regen_blocked`).
        respawn: if raw.regendelay != 0 {
            format!("{}m", raw.regendelay)
        } else {
            "server default".to_owned()
        },
        unique_hours: pool
            .iter()
            .filter(|m| m.maxalive == 1 && m.regenhrs > 0)
            .map(|m| (m.name.clone(), m.regenhrs))
            .collect(),
    });
    let spawns = (matches!(raw.room_type, 0 | 2) && !pool.is_empty()).then_some(Spawns {
        monsters: pool_names,
        max: raw.maxmonsters,
    });
    Room {
        key: key_text(key),
        name: raw.name.clone(),
        kind: (raw.room_type != 0).then_some(room_kind),
        levels: (raw.room_type == 2).then(|| format!("{}-{}", raw.minlevel, raw.maxlevel)),
        exits,
        spawns,
        lair,
        boss: (raw.specmon != 0).then(|| {
            monsters
                .iter()
                .find(|m| m.number == raw.specmon)
                .map_or_else(|| format!("monster {}", raw.specmon), |m| m.name.clone())
        }),
        shop: (raw.shopnum != 0)
            .then(|| shops.get(&raw.shopnum).cloned())
            .flatten(),
    }
}

impl Area {
    /// Writes the file, with its header. The same world always writes the same text.
    pub fn write(&self, path: &Path) -> Result<(), DataError> {
        let body = serde_norway::to_string(self).map_err(|x| DataError {
            path: path.to_owned(),
            message: x.to_string(),
        })?;
        std::fs::write(path, format!("{HEADER}{body}")).map_err(|x| DataError {
            path: path.to_owned(),
            message: x.to_string(),
        })
    }

    pub fn read(path: &Path) -> Result<Self, DataError> {
        let text = std::fs::read_to_string(path).map_err(|x| DataError {
            path: path.to_owned(),
            message: x.to_string(),
        })?;
        serde_norway::from_str(&text).map_err(|x| DataError {
            path: path.to_owned(),
            message: x.to_string(),
        })
    }
}

/// An area's hand-written companion, `<area>.local.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(title = "mudparty places local file")]
#[serde(deny_unknown_fields)]
pub struct Local {
    pub schema: u32,
    pub area: String,
    /// Names of the user's own: `arena: 1/2150`, `road: narrow-road-2146`. The target is
    /// any way of naming a room except another alias.
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
}

/// Why a name didn't give one room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaceError {
    Unknown(String),
    /// More than one room answers to it: their keys and names.
    Ambiguous {
        name: String,
        rooms: Vec<(String, String)>,
    },
}

impl fmt::Display for PlaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(n) => write!(f, "no room {n:?} in places/"),
            Self::Ambiguous { name, rooms } => {
                let list: Vec<String> = rooms.iter().map(|(k, n)| format!("{k} {n}")).collect();
                write!(
                    f,
                    "{name:?} could be {}; name one by its slug or key",
                    list.join(", ")
                )
            }
        }
    }
}

impl std::error::Error for PlaceError {}

/// One room as [`Places`] finds it.
#[derive(Debug, Clone)]
struct Found {
    key: (u32, u32),
    area: String,
    slug: String,
    name: String,
    /// What its shop sells.
    sells: Vec<String>,
    /// A lair's respawn as the file writes it (`2m`, `server default`).
    lair_respawn: Option<String>,
    /// Where its exits lead, as the file writes them, by direction word.
    exits: Vec<String>,
    /// A training shop's class: `any class`, or the class's name.
    trains: Option<String>,
    /// Its shop's kind (`weapons`, `general`, `bank`, ...), if it's a shop.
    shop_kind: Option<String>,
}

/// Every area in a `places/` folder, with the local files' aliases, for naming rooms
/// (design §14.3).
#[derive(Debug, Clone, Default)]
pub struct Places {
    rooms: Vec<Found>,
    /// (area, alias) → the room.
    aliases: BTreeMap<(String, String), (u32, u32)>,
}

fn parse_key(text: &str) -> Option<(u32, u32)> {
    let (m, r) = text.trim().split_once('/')?;
    Some((m.trim().parse().ok()?, r.trim().parse().ok()?))
}

impl Places {
    /// Reads `dir`: every `<area>.yaml`, then every `<area>.local.yaml`, whose aliases must
    /// each name a room.
    pub fn load(dir: &Path) -> Result<Self, DataError> {
        let listing = std::fs::read_dir(dir).map_err(|x| DataError {
            path: dir.to_owned(),
            message: x.to_string(),
        })?;
        let mut files: Vec<PathBuf> = listing
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "yaml"))
            .collect();
        files.sort();
        let is_local = |p: &Path| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".local.yaml"))
        };
        let mut places = Places::default();
        for p in files.iter().filter(|p| !is_local(p)) {
            let a = Area::read(p)?;
            for (slug, room) in a.rooms {
                let Some(key) = parse_key(&room.key) else {
                    continue;
                };
                places.rooms.push(Found {
                    key,
                    area: a.area.clone(),
                    slug,
                    name: room.name,
                    trains: room.shop.as_ref().and_then(|s| s.trains.clone()),
                    shop_kind: room.shop.as_ref().map(|s| s.kind.clone()),
                    sells: room.shop.map(|s| s.sells).unwrap_or_default(),
                    lair_respawn: room.lair.map(|l| l.respawn),
                    exits: room
                        .exits
                        .into_values()
                        .map(|e| match e {
                            Exit::To(to) => to,
                            Exit::Special(s) => s.to,
                        })
                        .collect(),
                });
            }
        }
        for p in files.iter().filter(|p| is_local(p)) {
            let text = std::fs::read_to_string(p).map_err(|x| DataError {
                path: p.clone(),
                message: x.to_string(),
            })?;
            let local: Local = serde_norway::from_str(&text).map_err(|x| DataError {
                path: p.clone(),
                message: x.to_string(),
            })?;
            for (alias, target) in &local.aliases {
                let key = places
                    .resolve_plain(target, Some(&local.area))
                    .map_err(|x| DataError {
                        path: p.clone(),
                        message: format!("aliases.{alias}: {x}"),
                    })?;
                places
                    .aliases
                    .insert((local.area.clone(), alias.clone()), key);
            }
        }
        Ok(places)
    }

    /// The game's name for the room `key`, if a compiled file has it.
    pub fn name_of(&self, key: (u32, u32)) -> Option<&str> {
        self.rooms
            .iter()
            .find(|f| f.key == key)
            .map(|f| f.name.as_str())
    }

    /// The kind of a room's shop (`weapons`, `general`, `bank`, `training`, ...); none for a
    /// room that isn't a shop.
    pub fn shop_kind(&self, key: (u32, u32)) -> Option<&str> {
        self.rooms
            .iter()
            .find(|f| f.key == key)
            .and_then(|f| f.shop_kind.as_deref())
    }

    /// A lair's respawn time as its file writes it (`2m`, or `server default` when the room
    /// sets none); none for a room that isn't a lair (#67).
    pub fn lair_respawn(&self, key: (u32, u32)) -> Option<&str> {
        self.rooms
            .iter()
            .find(|f| f.key == key)
            .and_then(|f| f.lair_respawn.as_deref())
    }

    /// Where a bot waits for the lair `key` to fill (#67, design §14.7): the nearest room by
    /// exits that isn't a lair itself, never passing through a room in `avoid`; of rooms as
    /// near, the first by direction word. A lair fills when a player walks in, so waiting
    /// inside would start it early. Lairs next to each other are walked through, as the bot
    /// would.
    pub fn wait_room(&self, key: (u32, u32), avoid: &BTreeSet<(u32, u32)>) -> Option<(u32, u32)> {
        let mut seen = BTreeSet::from([key]);
        let mut queue = VecDeque::from([key]);
        while let Some(k) = queue.pop_front() {
            let Some(room) = self.rooms.iter().find(|f| f.key == k) else {
                continue;
            };
            for to in &room.exits {
                let Ok(next) = self.resolve_plain(to, Some(&room.area)) else {
                    continue;
                };
                if avoid.contains(&next) || !seen.insert(next) {
                    continue;
                }
                if self.lair_respawn(next).is_none() {
                    return Some(next);
                }
                queue.push_back(next);
            }
        }
        None
    }

    /// The trainers for `class` (#56): training shops for any class or for that one
    /// (case-blind), those in `area` first, then by key. With no class, those for any class.
    pub fn trainers(&self, class: Option<&str>, area: Option<&str>) -> Vec<(u32, u32)> {
        let mut found: Vec<&Found> = self
            .rooms
            .iter()
            .filter(|f| {
                f.trains.as_deref().is_some_and(|t| {
                    t == "any class" || class.is_some_and(|c| t.eq_ignore_ascii_case(c.trim()))
                })
            })
            .collect();
        found.sort_by_key(|f| (area.is_none_or(|a| f.area != a), f.key));
        found.iter().map(|f| f.key).collect()
    }

    /// The shops that sell `item` (case-blind), those in `area` first, then by key.
    pub fn shops_selling(&self, item: &str, area: Option<&str>) -> Vec<(u32, u32)> {
        let mut found: Vec<&Found> = self
            .rooms
            .iter()
            .filter(|f| f.sells.iter().any(|s| s.eq_ignore_ascii_case(item.trim())))
            .collect();
        found.sort_by_key(|f| (area.is_none_or(|a| f.area != a), f.key));
        found.iter().map(|f| f.key).collect()
    }

    /// The rooms of `area`, or `None` for an area no file has.
    pub fn rooms_in(&self, area: &str) -> Option<Vec<(u32, u32)>> {
        let keys: Vec<(u32, u32)> = self
            .rooms
            .iter()
            .filter(|f| f.area == area.trim())
            .map(|f| f.key)
            .collect();
        (!keys.is_empty()).then_some(keys)
    }

    /// The room `text` names, looking first in `area` (design §14.3): an alias, a key
    /// (`1/2150`), `area:slug`, a slug, the game's name while one room has it, or the
    /// start of a slug the area repeats (`narrow-road` for `narrow-road-2146`), which is
    /// an error listing the rooms when there are several.
    pub fn resolve(&self, text: &str, area: Option<&str>) -> Result<(u32, u32), PlaceError> {
        let t = text.trim();
        if let Some(a) = area
            && let Some(k) = self.aliases.get(&(a.to_owned(), t.to_owned()))
        {
            return Ok(*k);
        }
        let other: Vec<(u32, u32)> = self
            .aliases
            .iter()
            .filter(|((_, n), _)| n == t)
            .map(|(_, k)| *k)
            .collect();
        if let [k] = other.as_slice() {
            return Ok(*k);
        }
        self.resolve_plain(t, area)
    }

    /// [`Places::resolve`] without the aliases.
    fn resolve_plain(&self, text: &str, area: Option<&str>) -> Result<(u32, u32), PlaceError> {
        let t = text.trim();
        let one = |found: Vec<&Found>| -> Option<Result<(u32, u32), PlaceError>> {
            match found.as_slice() {
                [] => None,
                [f] => Some(Ok(f.key)),
                many => Some(Err(PlaceError::Ambiguous {
                    name: t.to_owned(),
                    rooms: many
                        .iter()
                        .map(|f| (key_text((f.key.0.into(), f.key.1.into())), f.name.clone()))
                        .collect(),
                })),
            }
        };
        if let Some(k) = parse_key(t) {
            return if self.rooms.iter().any(|f| f.key == k) {
                Ok(k)
            } else {
                Err(PlaceError::Unknown(t.to_owned()))
            };
        }
        if let Some((a, s)) = t.split_once(':') {
            let found = self
                .rooms
                .iter()
                .filter(|f| f.area == a.trim() && f.slug == s.trim())
                .collect();
            return one(found).unwrap_or_else(|| Err(PlaceError::Unknown(t.to_owned())));
        }
        // A slug: in the area first, then anywhere.
        let in_area = |f: &&Found| area.is_none_or(|a| f.area == a);
        let exact: Vec<&Found> = self.rooms.iter().filter(|f| f.slug == t).collect();
        if let Some(r) = one(exact.iter().copied().filter(in_area).collect()) {
            return r;
        }
        if let Some(r) = one(exact) {
            return r;
        }
        // The game's own name.
        if let Some(r) = one(self
            .rooms
            .iter()
            .filter(|f| f.name.eq_ignore_ascii_case(t))
            .collect())
        {
            return r;
        }
        // The start of a slug the area repeats.
        let repeated = |f: &&Found| {
            f.slug
                .strip_prefix(t)
                .and_then(|rest| rest.strip_prefix('-'))
                .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit() || c == '-'))
        };
        let starts: Vec<&Found> = self.rooms.iter().filter(repeated).collect();
        if let Some(r) = one(starts.iter().copied().filter(in_area).collect()) {
            return r;
        }
        if let Some(r) = one(starts) {
            return r;
        }
        Err(PlaceError::Unknown(t.to_owned()))
    }
}
