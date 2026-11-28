//! mudparty's game data (design §7.5): rooms, exits, spells and classes, imported from
//! silvermere's database into `gamedata.sqlite`, which clients open read-only.
//!
//! The numbering is silvermere's, so a `map/room` from `/room` is a direct key. Two columns
//! are mudparty's own, worked out at import: each exit's `shows_as` (how `Obvious exits:`
//! prints it) and each room's `mm_hash` (MegaMud's room hash, design §8.1).

mod megamud;
pub mod places;

use std::collections::{HashMap, HashSet};
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
const EXIT_LEVEL: i64 = 15;

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
    pub monsters: usize,
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

/// One exit of a room, as silvermere stores it.
///
/// # `par1`..`par4`: legacy fields, kept exactly as they are
///
/// Every exit in a MajorMUD room record carries four parameter slots (`exitpar1s` to
/// `exitpar4s`), and silvermere keeps them as `par1` to `par4` in its `room_exits` table.
/// The slots have no fixed meaning: what each one holds depends on `exit_type`. That is
/// awkward, but it is the original game's design, and silvermere and MajorMUD's data
/// depend on it. mudparty copies the four values unchanged and decodes them only where it
/// needs to, so the data stays compatible with both. Don't reshape or reinterpret them in
/// the import.
///
/// How silvermere reads them (`ExitParams` in silvermere-types `src/records/room.rs`,
/// the source of truth if this table ever disagrees):
///
/// | `exit_type` (code) | `par1` | `par2` | `par3` | `par4` |
/// |---|---|---|---|---|
/// | Door (7), Gate (11) | lock state | bash/pick bonus | steps it stays open | key item |
/// | Key (2) | key item | lock state | pick bonus | steps it stays open |
/// | Toll (4) | gold | | | |
/// | Item (3) | item needed | refusal message | message | |
/// | Hidden (6) | `EXF_` flags (searched, blocked, …) | blocks | "opened" message | name message |
/// | Map Change (8) | the map it leads into | | | |
/// | Trap (9) | damage | trap state | message | disarm-failed message |
/// | Text (10) | the words to type (message) | | message | |
/// | Remote Action (12) | the words (message) | code | message | item |
/// | Level (15), Alignment (20) | minimum | maximum | | |
/// | Class (13), Race (14) | the only class/race allowed | the one barred | | |
/// | Timed (16) | message | steps | open/closed state | |
/// | Cast (22) | spell | after | message | look message |
/// | Ability (23) | ability | minimum | maximum | refusal message |
/// | Spell Trap (24) | spell | trap state | disarmed message | message |
/// | Normal (0), Spell (1), Ticket (17), User Count (18), Block Guard (19), Delay (21) | unused | | | |
///
/// "Message" means a message number in the game's message table, and "item" an item
/// number. Today mudparty reads only Map Change's `par1`, to set `to_map`. The rest wait
/// for the features that need them (doors, keys, tolls and level ranges when walking
/// paths).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomExit {
    /// 0..9: N S E W NE NW SE SW U D.
    pub dir: u8,
    /// The map the exit leads into: the room's own map, or a Map Change's `par1`.
    pub to_map: i64,
    /// The room it leads to, on `to_map`; 0 for none.
    pub to_room: i64,
    /// silvermere's `EXIT_` code; it says what `par1`..`par4` mean (see the table above).
    pub exit_type: i64,
    /// The first parameter; meaning by `exit_type`, in the table above.
    pub par1: i64,
    /// The second parameter; meaning by `exit_type`, in the table above.
    pub par2: i64,
    /// The third parameter; meaning by `exit_type`, in the table above.
    pub par3: i64,
    /// The fourth parameter; meaning by `exit_type`, in the table above.
    pub par4: i64,
    /// How `Obvious exits:` prints it, worked out by mudparty at import.
    pub shows_as: ShowsAs,
}

/// One step of a walk: the direction to go, the room it reaches, and whether a door or
/// gate stands in the way (which may need opening first).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    /// 0..9: N S E W NE NW SE SW U D.
    pub dir: u8,
    pub to_map: i64,
    pub to_room: i64,
    pub door: bool,
}

/// Exits a walker passes by walking: plain ones, doors and gates (opened if closed), map
/// changes, level exits (the game refuses a character outside the range), and the types
/// silvermere passes like a plain exit (ticket, user count, block guard, delay). Not:
/// key, spell, item, toll, action, hidden, trap, text, remote, class, race, timed or
/// alignment exits, which need more than a walk.
fn walkable(exit_type: i64) -> bool {
    matches!(exit_type, 0 | 7 | 8 | 11 | 15 | 17 | 18 | 19 | 21)
}

/// How many rooms a route search looks at before giving up.
const ROUTE_LIMIT: usize = 20_000;

/// What a walk may not use, and what it would rather not (#64, design §14.5). Every plan a
/// walk makes, its re-plans included, is made with the same rules, as MudPlay's route
/// filters (`IRoomFilter`) are held for a whole walk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteRules {
    /// Rooms never walked into (the room a walk starts in is left as usual).
    pub avoid_rooms: HashSet<(i64, i64)>,
    /// silvermere `EXIT_` codes never taken (7 door, 9 trap, 4 toll, ...).
    pub avoid_kinds: HashSet<i64>,
    /// Exits the game refused on this walk: `(room, direction)`.
    pub refused: HashSet<((i64, i64), u8)>,
    /// Rooms walked through only when the way round is longer than this many steps more.
    pub prefer_not: HashMap<(i64, i64), u32>,
    /// The character's level, when known: a level exit (`par1` the lowest level, `par2`
    /// the highest, 0 for none) that doesn't let it through isn't taken. Unknown, level
    /// exits are walked and the game decides, as it does for a walker (#52).
    pub level: Option<u32>,
}

/// Whether a level exit lets a character of `level` through, as silvermere decides it
/// (`EXIT_LEVEL`: at least `par1`, and at most `par2` unless `par2` is 0).
fn level_lets_through(e: &RoomExit, level: u32) -> bool {
    let level = i64::from(level);
    level >= e.par1 && (e.par2 == 0 || level <= e.par2)
}

/// A level exit's range in words: `levels 2 to 3`, `level 5 and up`.
fn level_range(e: &RoomExit) -> String {
    if e.par2 == 0 {
        format!("level {} and up", e.par1)
    } else {
        format!("levels {} to {}", e.par1, e.par2)
    }
}

/// Whether `rules` keep a walk from taking `e`, for its kind or the character's level.
fn kept_off(e: &RoomExit, rules: &RouteRules) -> bool {
    rules.avoid_kinds.contains(&e.exit_type)
        || (e.exit_type == EXIT_LEVEL && rules.level.is_some_and(|l| !level_lets_through(e, l)))
}

/// Why [`GameData::route_with`] found no walk.
#[derive(Debug)]
pub enum RouteError {
    Data(DataError),
    /// No walk keeps to the rules; what stands in the way, in words.
    NoWay(String),
}

impl fmt::Display for RouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Data(e) => e.fmt(f),
            Self::NoWay(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for RouteError {}

impl From<DataError> for RouteError {
    fn from(e: DataError) -> Self {
        Self::Data(e)
    }
}

const DIR_WORDS: [&str; 10] = [
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
    /// silvermere's `TGT_` value: 3, 5, 9, 11 and 12 go at the whole room.
    pub target: i64,
    pub spell_type: i64,
    pub energy: i64,
    /// The caster's line of a cast (#125), the spell message's first text, `%s` and `%d`
    /// still in it: `You fire a %s at %s for %d damage!`. Empty when the spell has none.
    pub cast_line: String,
}

impl Spell {
    /// Whether the spell goes at everyone in the room rather than one target: silvermere's
    /// Divided Area (3, 5), Divided Attack Area (9), Full Area (11) and Full Attack Area
    /// (12). It is cast with its bare code.
    pub fn hits_room(&self) -> bool {
        matches!(self.target, 3 | 5 | 9 | 11 | 12)
    }

    /// Whether the spell goes at the caster's whole party (#126): silvermere's Divided Party
    /// Area (10) and Full Party Area (13), healing rain. It is cast with its bare code.
    pub fn hits_party(&self) -> bool {
        matches!(self.target, 10 | 13)
    }
}

/// What this mudparty writes into `meta.schema`: a file of another one is imported again.
/// 2 added the spells' cast lines (#125).
pub const SCHEMA_VERSION: u32 = 2;

const SPELL_COLUMNS: &str =
    "number, name, short, mana, level, magery, magelvl, target, spell_type, energy, cast_line";

fn spell_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Spell> {
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
        cast_line: r.get(10)?,
    })
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
  target INTEGER NOT NULL, spell_type INTEGER NOT NULL, energy INTEGER NOT NULL,
  cast_line TEXT NOT NULL);
CREATE INDEX spells_short ON spells (short COLLATE NOCASE);
CREATE TABLE classes (
  number INTEGER PRIMARY KEY, name TEXT NOT NULL, magery INTEGER NOT NULL,
  magelvl INTEGER NOT NULL);
-- Monster names, to tell the room's monsters from its players.
CREATE TABLE monsters (number INTEGER PRIMARY KEY, name TEXT NOT NULL);
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
                "SELECT s.number, s.name, s.shortnm, s.mana, s.level, s.magery, s.magelvl,
                        s.target, s.spell_type, s.energy, COALESCE(m.text0, '')
                 FROM spells s LEFT JOIN messages m ON m.number = s.msgnum
                 ORDER BY s.number",
            )
            .map_err(&e)?;
        q.query_map([], spell_row)
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
    let monsters: Vec<(i64, String)> = {
        let mut q = src
            .prepare("SELECT number, name FROM monsters ORDER BY number")
            .map_err(&e)?;
        q.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
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
    for m in &monsters {
        h.update(format!("{m:?}\n"));
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
        ("schema", &SCHEMA_VERSION.to_string()),
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
            "INSERT INTO spells VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
                s.energy,
                s.cast_line
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
    for (number, name) in &monsters {
        txn.execute(
            "INSERT INTO monsters VALUES (?1, ?2)",
            params![number, name],
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
        monsters: monsters.len(),
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

    /// `meta.schema`: the [`SCHEMA_VERSION`] of the mudparty that wrote the file.
    pub fn schema(&self) -> Result<u32, DataError> {
        let v: String = self
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'schema'", [], |r| {
                r.get(0)
            })
            .map_err(err(&self.path))?;
        Ok(v.parse().unwrap_or(0))
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

    /// The shortest walk from `from` to `to`, both `(map, room)`, over walkable exits;
    /// `None` when there is none.
    pub fn route(&self, from: (i64, i64), to: (i64, i64)) -> Result<Option<Vec<Step>>, DataError> {
        match self.route_with(from, to, &RouteRules::default()) {
            Ok(steps) => Ok(Some(steps)),
            Err(RouteError::NoWay(_)) => Ok(None),
            Err(RouteError::Data(e)) => Err(e),
        }
    }

    /// The cheapest walk from `from` to `to` that keeps to `rules` (#64): each step costs
    /// one, plus a room's `prefer_not`. Ties go to the exit found first, in direction
    /// order, so the same world and rules always give the same walk. With no walk, the
    /// error says what stands in the way.
    pub fn route_with(
        &self,
        from: (i64, i64),
        to: (i64, i64),
        rules: &RouteRules,
    ) -> Result<Vec<Step>, RouteError> {
        if from == to {
            return Ok(Vec::new());
        }
        if rules.avoid_rooms.contains(&to) {
            return Err(RouteError::NoWay(format!("{} is avoided", self.named(to)?)));
        }
        if let Some(steps) = self.search(from, to, rules)? {
            return Ok(steps);
        }
        Err(RouteError::NoWay(self.in_the_way(from, to, rules)?))
    }

    /// Dijkstra over the walkable exits that `rules` allow.
    fn search(
        &self,
        from: (i64, i64),
        to: (i64, i64),
        rules: &RouteRules,
    ) -> Result<Option<Vec<Step>>, DataError> {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;
        let mut best: HashMap<(i64, i64), u64> = HashMap::from([(from, 0)]);
        let mut came: HashMap<(i64, i64), ((i64, i64), Step)> = HashMap::new();
        let mut queue = BinaryHeap::from([Reverse((0u64, 0u64, from))]);
        let mut seq = 0u64;
        let mut done: HashSet<(i64, i64)> = HashSet::new();
        while let Some(Reverse((cost, _, here))) = queue.pop() {
            if !done.insert(here) {
                continue;
            }
            if here == to {
                let mut steps = Vec::new();
                let mut at = to;
                while at != from {
                    let (prev, s) = came[&at];
                    steps.push(s);
                    at = prev;
                }
                steps.reverse();
                return Ok(Some(steps));
            }
            if done.len() > ROUTE_LIMIT {
                break;
            }
            for e in self.exits(here.0, here.1)? {
                let next = (e.to_map, e.to_room);
                if !walkable(e.exit_type)
                    || e.to_room == 0
                    || done.contains(&next)
                    || kept_off(&e, rules)
                    || rules.avoid_rooms.contains(&next)
                    || rules.refused.contains(&(here, e.dir))
                {
                    continue;
                }
                let step_cost = 1 + u64::from(rules.prefer_not.get(&next).copied().unwrap_or(0));
                let c = cost + step_cost;
                if best.get(&next).is_some_and(|&b| b <= c) {
                    continue;
                }
                best.insert(next, c);
                came.insert(
                    next,
                    (
                        here,
                        Step {
                            dir: e.dir,
                            to_map: e.to_map,
                            to_room: e.to_room,
                            door: matches!(e.exit_type, EXIT_DOOR | EXIT_GATE),
                        },
                    ),
                );
                seq += 1;
                queue.push(Reverse((c, seq, next)));
            }
        }
        Ok(None)
    }

    /// Why no walk keeps to `rules`: the first thing the rules forbid on the walk there
    /// would be without them, or that there's no walk at all.
    fn in_the_way(
        &self,
        from: (i64, i64),
        to: (i64, i64),
        rules: &RouteRules,
    ) -> Result<String, DataError> {
        let ends = format!("from {} to {}", self.named(from)?, self.named(to)?);
        let Some(free) = self.search(from, to, &RouteRules::default())? else {
            return Ok(format!("no way {ends} by walking"));
        };
        let mut at = from;
        for s in &free {
            let next = (s.to_map, s.to_room);
            let taken = self.exits(at.0, at.1)?.into_iter().find(|e| e.dir == s.dir);
            let kind = taken.as_ref().map_or(0, |e| e.exit_type);
            let dir = DIR_WORDS[usize::from(s.dir) % DIR_WORDS.len()];
            if rules.refused.contains(&(at, s.dir)) {
                return Ok(format!(
                    "every way {ends} is barred: the shortest goes {dir} from {}, which the game refused",
                    self.named(at)?
                ));
            }
            if let (Some(e), Some(level)) = (&taken, rules.level)
                && kind == EXIT_LEVEL
                && !level_lets_through(e, level)
            {
                return Ok(format!(
                    "every way {ends} is barred: the shortest goes {dir} from {} by an exit for {}, and the character is level {level}",
                    self.named(at)?,
                    level_range(e)
                ));
            }
            if rules.avoid_kinds.contains(&kind) {
                return Ok(format!(
                    "every way {ends} is barred: the shortest goes {dir} from {} by a {} exit, and those are avoided",
                    self.named(at)?,
                    places::exit_kind_name(kind)
                ));
            }
            if rules.avoid_rooms.contains(&next) {
                return Ok(format!(
                    "every way {ends} is barred: the shortest goes through {}, which is avoided",
                    self.named(next)?
                ));
            }
            at = next;
        }
        Ok(format!("no way {ends} keeps to the rules"))
    }

    /// `Square (1/1)`, or the bare key for a room the data hasn't.
    fn named(&self, key: (i64, i64)) -> Result<String, DataError> {
        Ok(match self.room(key.0, key.1)? {
            Some(r) => format!("{} ({}/{})", r.name, key.0, key.1),
            None => format!("{}/{}", key.0, key.1),
        })
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
                &format!("SELECT {SPELL_COLUMNS} FROM spells WHERE short = ?1 COLLATE NOCASE"),
                params![short],
                spell_row,
            )
            .optional()
            .map_err(err(&self.path))
    }

    /// Every class, by number.
    pub fn classes(&self) -> Result<Vec<Class>, DataError> {
        let e = err(&self.path);
        let mut q = self
            .conn
            .prepare("SELECT number, name, magery, magelvl FROM classes ORDER BY number")
            .map_err(&e)?;
        q.query_map([], |r| {
            Ok(Class {
                number: r.get(0)?,
                name: r.get(1)?,
                magery: r.get(2)?,
                magelvl: r.get(3)?,
            })
        })
        .map_err(&e)?
        .collect::<Result<_, _>>()
        .map_err(&e)
    }

    /// Every spell with a cast code, by number.
    pub fn spells(&self) -> Result<Vec<Spell>, DataError> {
        let e = err(&self.path);
        let mut q = self
            .conn
            .prepare(&format!(
                "SELECT {SPELL_COLUMNS} FROM spells WHERE short <> '' ORDER BY number"
            ))
            .map_err(&e)?;
        q.query_map([], spell_row)
            .map_err(&e)?
            .collect::<Result<_, _>>()
            .map_err(&e)
    }

    /// Every monster name, once each, sorted.
    pub fn monster_names(&self) -> Result<Vec<String>, DataError> {
        let e = err(&self.path);
        let mut q = self
            .conn
            .prepare("SELECT DISTINCT name FROM monsters WHERE name <> '' ORDER BY name")
            .map_err(&e)?;
        q.query_map([], |r| r.get(0))
            .map_err(&e)?
            .collect::<Result<_, _>>()
            .map_err(&e)
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
