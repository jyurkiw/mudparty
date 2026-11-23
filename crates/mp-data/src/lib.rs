//! mudparty's game data (design §7.5): rooms, exits, spells and classes, imported from
//! silvermere's database into `gamedata.sqlite`, which clients open read-only.
//!
//! The numbering is silvermere's, so a `map/room` from `/room` is a direct key. Two columns
//! are mudparty's own, worked out at import: each exit's `shows_as` (how `Obvious exits:`
//! prints it) and each room's `mm_hash` (MegaMud's room hash, design §8.1).

mod megamud;

use std::fmt;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use sha2::{Digest, Sha256};

pub use megamud::{exits_code, mm_hash, name_hash};

/// How `Obvious exits:` prints an exit (silvermere `display/exits.rs`), which decides its
/// weight in the MegaMud hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShowsAs {
    /// The direction word; weight 1.
    Plain,
    /// `closed door north`, `open gate south`, a trap door; weight 2.
    Door,
    /// Shown only once searched, sprung or open; left out of the hash.
    Hidden,
    /// Never shown (remote actions, text exits); left out.
    None,
}

impl ShowsAs {
    fn as_str(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Door => "door",
            Self::Hidden => "hidden",
            Self::None => "none",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "door" => Self::Door,
            "hidden" => Self::Hidden,
            "none" => Self::None,
            _ => Self::Plain,
        }
    }
}

// silvermere's EXIT_ codes (silvermere-types `ExitType`).
const EXIT_KEY: i64 = 2;
const EXIT_HIDDEN: i64 = 6;
const EXIT_DOOR: i64 = 7;
const EXIT_MAPCHANGE: i64 = 8;
const EXIT_TRAP: i64 = 9;
const EXIT_TEXT: i64 = 10;
const EXIT_GATE: i64 = 11;
const EXIT_REMOTE: i64 = 12;
const EXIT_TIMED: i64 = 16;

/// How an exit of silvermere type `exit_type` shows, as the world stands before anyone
/// searches, springs a trap or opens a timed exit.
pub fn shows_as(exit_type: i64) -> ShowsAs {
    match exit_type {
        EXIT_DOOR | EXIT_KEY | EXIT_GATE => ShowsAs::Door,
        EXIT_HIDDEN | EXIT_TRAP | EXIT_TIMED => ShowsAs::Hidden,
        EXIT_REMOTE | EXIT_TEXT => ShowsAs::None,
        _ => ShowsAs::Plain,
    }
}

/// Something went wrong reading or writing game data.
#[derive(Debug)]
pub struct DataError {
    pub path: PathBuf,
    pub message: String,
}

impl fmt::Display for DataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}

impl std::error::Error for DataError {}

fn err(path: &Path) -> impl Fn(rusqlite::Error) -> DataError + '_ {
    move |e| DataError {
        path: path.to_owned(),
        message: e.to_string(),
    }
}

/// What an import wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    /// `gamedata_rev`: the first 12 hex digits of a SHA-256 over everything imported.
    pub rev: String,
    pub rooms: usize,
    pub exits: usize,
    pub spells: usize,
    pub classes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Room {
    pub map: i64,
    pub room: i64,
    pub name: String,
    pub light: i64,
    pub room_type: i64,
    pub shopnum: i64,
    pub flags: i64,
    pub roomspell: i64,
    /// MegaMud's 8-character hashExits.
    pub mm_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomExit {
    /// 0..9: N S E W NE NW SE SW U D.
    pub dir: u8,
    pub to_map: i64,
    pub to_room: i64,
    pub exit_type: i64,
    pub par1: i64,
    pub par2: i64,
    pub par3: i64,
    pub par4: i64,
    pub shows_as: ShowsAs,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spell {
    pub number: i64,
    pub name: String,
    /// The command name, e.g. `mmis`.
    pub short: String,
    pub mana: i64,
    pub level: i64,
    pub magery: i64,
    pub magelvl: i64,
    pub target: i64,
    pub spell_type: i64,
    pub energy: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Class {
    pub number: i64,
    pub name: String,
    pub magery: i64,
    pub magelvl: i64,
}

const SCHEMA: &str = "
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE rooms (
  map INTEGER NOT NULL, room INTEGER NOT NULL, name TEXT NOT NULL,
  light INTEGER NOT NULL, room_type INTEGER NOT NULL, shopnum INTEGER NOT NULL,
  flags INTEGER NOT NULL, roomspell INTEGER NOT NULL, mm_hash TEXT NOT NULL,
  PRIMARY KEY (map, room));
CREATE INDEX rooms_mm_hash ON rooms (mm_hash);
CREATE TABLE exits (
  map INTEGER NOT NULL, room INTEGER NOT NULL, dir INTEGER NOT NULL,
  to_map INTEGER NOT NULL, to_room INTEGER NOT NULL, exit_type INTEGER NOT NULL,
  par1 INTEGER NOT NULL, par2 INTEGER NOT NULL, par3 INTEGER NOT NULL, par4 INTEGER NOT NULL,
  shows_as TEXT NOT NULL,
  PRIMARY KEY (map, room, dir));
CREATE TABLE spells (
  number INTEGER PRIMARY KEY, name TEXT NOT NULL, short TEXT NOT NULL, mana INTEGER NOT NULL,
  level INTEGER NOT NULL, magery INTEGER NOT NULL, magelvl INTEGER NOT NULL,
  target INTEGER NOT NULL, spell_type INTEGER NOT NULL, energy INTEGER NOT NULL);
CREATE INDEX spells_short ON spells (short COLLATE NOCASE);
CREATE TABLE classes (
  number INTEGER PRIMARY KEY, name TEXT NOT NULL, magery INTEGER NOT NULL,
  magelvl INTEGER NOT NULL);
";

/// Imports `silvermere_db` into `out`, replacing it whole: the new file is written beside
/// it and renamed into place, so a reader never sees half an import.
pub fn import(silvermere_db: &Path, out: &Path) -> Result<ImportReport, DataError> {
    let src = Connection::open_with_flags(silvermere_db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(err(silvermere_db))?;
    let e = err(silvermere_db);

    let mut exits_by_room: std::collections::BTreeMap<(i64, i64), Vec<RoomExit>> =
        Default::default();
    {
        let mut q = src
            .prepare(
                "SELECT r.map, r.room, x.direction, x.target, x.exit_type, x.par1, x.par2,
                        x.par3, x.par4
                 FROM room_exits x JOIN rooms r ON r.id = x.room_id
                 ORDER BY r.map, r.room, x.direction",
            )
            .map_err(&e)?;
        let rows = q
            .query_map([], |row| {
                let map: i64 = row.get(0)?;
                let exit_type: i64 = row.get(4)?;
                let par1: i64 = row.get(5)?;
                Ok((
                    (map, row.get::<_, i64>(1)?),
                    RoomExit {
                        dir: row.get(2)?,
                        to_map: if exit_type == EXIT_MAPCHANGE {
                            par1
                        } else {
                            map
                        },
                        to_room: row.get(3)?,
                        exit_type,
                        par1,
                        par2: row.get(6)?,
                        par3: row.get(7)?,
                        par4: row.get(8)?,
                        shows_as: shows_as(exit_type),
                    },
                ))
            })
            .map_err(&e)?;
        for r in rows {
            let (key, exit) = r.map_err(&e)?;
            exits_by_room.entry(key).or_default().push(exit);
        }
    }

    let mut rooms = Vec::new();
    {
        let mut q = src
            .prepare(
                "SELECT map, room, name, light, room_type, shopnum, flags, roomspell
                 FROM rooms ORDER BY map, room",
            )
            .map_err(&e)?;
        let rows = q
            .query_map([], |row| {
                Ok(Room {
                    map: row.get(0)?,
                    room: row.get(1)?,
                    name: row.get(2)?,
                    light: row.get(3)?,
                    room_type: row.get(4)?,
                    shopnum: row.get(5)?,
                    flags: row.get(6)?,
                    roomspell: row.get(7)?,
                    mm_hash: String::new(),
                })
            })
            .map_err(&e)?;
        for r in rows {
            let mut room = r.map_err(&e)?;
            let exits = exits_by_room
                .get(&(room.map, room.room))
                .map(Vec::as_slice)
                .unwrap_or_default();
            room.mm_hash = mm_hash(&room.name, exits);
            rooms.push(room);
        }
    }

    let spells: Vec<Spell> = {
        let mut q = src
            .prepare(
                "SELECT number, name, shortnm, mana, level, magery, magelvl, target, spell_type,
                        energy
                 FROM spells ORDER BY number",
            )
            .map_err(&e)?;
        q.query_map([], |row| {
            Ok(Spell {
                number: row.get(0)?,
                name: row.get(1)?,
                short: row.get(2)?,
                mana: row.get(3)?,
                level: row.get(4)?,
                magery: row.get(5)?,
                magelvl: row.get(6)?,
                target: row.get(7)?,
                spell_type: row.get(8)?,
                energy: row.get(9)?,
            })
        })
        .map_err(&e)?
        .collect::<Result<_, _>>()
        .map_err(&e)?
    };

    let classes: Vec<Class> = {
        let mut q = src
            .prepare("SELECT number, name, magery, magelvl FROM classes ORDER BY number")
            .map_err(&e)?;
        q.query_map([], |row| {
            Ok(Class {
                number: row.get(0)?,
                name: row.get(1)?,
                magery: row.get(2)?,
                magelvl: row.get(3)?,
            })
        })
        .map_err(&e)?
        .collect::<Result<_, _>>()
        .map_err(&e)?
    };
    drop(src);

    // The revision is over the content, in key order, so the same world always gives the
    // same revision.
    let mut h = Sha256::new();
    for r in &rooms {
        h.update(format!("{r:?}\n"));
    }
    for (key, xs) in &exits_by_room {
        h.update(format!("{key:?}{xs:?}\n"));
    }
    for s in &spells {
        h.update(format!("{s:?}\n"));
    }
    for c in &classes {
        h.update(format!("{c:?}\n"));
    }
    let rev: String = h
        .finalize()
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect();

    let tmp = out.with_extension("sqlite.importing");
    let o = err(&tmp);
    if tmp.exists() {
        std::fs::remove_file(&tmp).map_err(|x| DataError {
            path: tmp.clone(),
            message: x.to_string(),
        })?;
    }
    let mut w = Connection::open(&tmp).map_err(&o)?;
    w.execute_batch(SCHEMA).map_err(&o)?;
    let txn = w.transaction().map_err(&o)?;
    for (k, v) in [
        ("gamedata_rev", rev.as_str()),
        ("source", "silvermere"),
        ("source_path", &silvermere_db.display().to_string()),
        ("schema", "1"),
    ] {
        txn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)",
            params![k, v],
        )
        .map_err(&o)?;
    }
    for r in &rooms {
        txn.execute(
            "INSERT INTO rooms VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                r.map,
                r.room,
                r.name,
                r.light,
                r.room_type,
                r.shopnum,
                r.flags,
                r.roomspell,
                r.mm_hash
            ],
        )
        .map_err(&o)?;
    }
    let mut exit_count = 0;
    for ((map, room), xs) in &exits_by_room {
        for x in xs {
            txn.execute(
                "INSERT INTO exits VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    map,
                    room,
                    x.dir,
                    x.to_map,
                    x.to_room,
                    x.exit_type,
                    x.par1,
                    x.par2,
                    x.par3,
                    x.par4,
                    x.shows_as.as_str()
                ],
            )
            .map_err(&o)?;
            exit_count += 1;
        }
    }
    for s in &spells {
        txn.execute(
            "INSERT INTO spells VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                s.number,
                s.name,
                s.short,
                s.mana,
                s.level,
                s.magery,
                s.magelvl,
                s.target,
                s.spell_type,
                s.energy
            ],
        )
        .map_err(&o)?;
    }
    for c in &classes {
        txn.execute(
            "INSERT INTO classes VALUES (?1, ?2, ?3, ?4)",
            params![c.number, c.name, c.magery, c.magelvl],
        )
        .map_err(&o)?;
    }
    txn.commit().map_err(&o)?;
    drop(w);
    std::fs::rename(&tmp, out).map_err(|x| DataError {
        path: out.to_owned(),
        message: x.to_string(),
    })?;
    Ok(ImportReport {
        rev,
        rooms: rooms.len(),
        exits: exit_count,
        spells: spells.len(),
        classes: classes.len(),
    })
}

/// An imported `gamedata.sqlite`, open read-only.
#[derive(Debug)]
pub struct GameData {
    conn: Connection,
    path: PathBuf,
}

impl GameData {
    /// Opens `path` read-only; a missing file is an error, never created.
    pub fn open(path: &Path) -> Result<Self, DataError> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(err(path))?;
        Ok(Self {
            conn,
            path: path.to_owned(),
        })
    }

    /// `gamedata_rev`.
    pub fn rev(&self) -> Result<String, DataError> {
        self.conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'gamedata_rev'",
                [],
                |r| r.get(0),
            )
            .map_err(err(&self.path))
    }

    pub fn room(&self, map: i64, room: i64) -> Result<Option<Room>, DataError> {
        self.conn
            .query_row(
                "SELECT map, room, name, light, room_type, shopnum, flags, roomspell, mm_hash
                 FROM rooms WHERE map = ?1 AND room = ?2",
                params![map, room],
                |r| {
                    Ok(Room {
                        map: r.get(0)?,
                        room: r.get(1)?,
                        name: r.get(2)?,
                        light: r.get(3)?,
                        room_type: r.get(4)?,
                        shopnum: r.get(5)?,
                        flags: r.get(6)?,
                        roomspell: r.get(7)?,
                        mm_hash: r.get(8)?,
                    })
                },
            )
            .optional()
            .map_err(err(&self.path))
    }

    pub fn exits(&self, map: i64, room: i64) -> Result<Vec<RoomExit>, DataError> {
        let e = err(&self.path);
        let mut q = self
            .conn
            .prepare(
                "SELECT dir, to_map, to_room, exit_type, par1, par2, par3, par4, shows_as
                 FROM exits WHERE map = ?1 AND room = ?2 ORDER BY dir",
            )
            .map_err(&e)?;
        q.query_map(params![map, room], |r| {
            Ok(RoomExit {
                dir: r.get(0)?,
                to_map: r.get(1)?,
                to_room: r.get(2)?,
                exit_type: r.get(3)?,
                par1: r.get(4)?,
                par2: r.get(5)?,
                par3: r.get(6)?,
                par4: r.get(7)?,
                shows_as: ShowsAs::parse(&r.get::<_, String>(8)?),
            })
        })
        .map_err(&e)?
        .collect::<Result<_, _>>()
        .map_err(&e)
    }

    /// The rooms with MegaMud hash `hash`, as `(map, room)`.
    pub fn rooms_by_mm_hash(&self, hash: &str) -> Result<Vec<(i64, i64)>, DataError> {
        let e = err(&self.path);
        let mut q = self
            .conn
            .prepare("SELECT map, room FROM rooms WHERE mm_hash = ?1 ORDER BY map, room")
            .map_err(&e)?;
        q.query_map(params![hash.to_ascii_uppercase()], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .map_err(&e)?
        .collect::<Result<_, _>>()
        .map_err(&e)
    }

    /// A spell by its command name, case-blind.
    pub fn spell_by_short(&self, short: &str) -> Result<Option<Spell>, DataError> {
        self.conn
            .query_row(
                "SELECT number, name, short, mana, level, magery, magelvl, target, spell_type,
                        energy
                 FROM spells WHERE short = ?1 COLLATE NOCASE",
                params![short],
                |r| {
                    Ok(Spell {
                        number: r.get(0)?,
                        name: r.get(1)?,
                        short: r.get(2)?,
                        mana: r.get(3)?,
                        level: r.get(4)?,
                        magery: r.get(5)?,
                        magelvl: r.get(6)?,
                        target: r.get(7)?,
                        spell_type: r.get(8)?,
                        energy: r.get(9)?,
                    })
                },
            )
            .optional()
            .map_err(err(&self.path))
    }

    /// A class by name, case-blind.
    pub fn class_by_name(&self, name: &str) -> Result<Option<Class>, DataError> {
        self.conn
            .query_row(
                "SELECT number, name, magery, magelvl FROM classes WHERE name = ?1 COLLATE NOCASE",
                params![name],
                |r| {
                    Ok(Class {
                        number: r.get(0)?,
                        name: r.get(1)?,
                        magery: r.get(2)?,
                        magelvl: r.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(err(&self.path))
    }
}
