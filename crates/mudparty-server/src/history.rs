//! `mudparty.sqlite`, the server's own history (design §7.6). For now it keeps where each
//! character was last known to be, and why that was recorded: when MudPlay drops and the
//! character is left in the game unattended, and when it hangs up or goes offline after.
//! That is where to look for a downed or nearly dead character to rescue.
//!
//! It also keeps each character's own statline while a bot has its own set (#34), so it
//! survives a crash and is put back on the next clean logoff, and when each lair was last
//! cleared (#67), so a circuit's clocks survive a restart.

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mp_engine::{LairClocks, StatlineStore};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

/// Where a character was, and why it was recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Position {
    pub map: u32,
    pub room: u32,
    /// From the game data, when it has the room.
    pub room_name: Option<String>,
    pub hp: Option<i32>,
    pub max_hp: Option<i32>,
    /// `MudPlay left`, `hung up: HP 12/40`, …
    pub why: String,
    /// When, as an RFC 3339 time in UTC.
    pub at: String,
}

pub struct History(Mutex<Connection>);

impl std::fmt::Debug for History {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("History")
    }
}

impl History {
    /// Opens (or makes) the file.
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let db = Connection::open(path)?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS positions (
                id INTEGER PRIMARY KEY,
                character TEXT NOT NULL,
                at TEXT NOT NULL,
                why TEXT NOT NULL,
                map INTEGER NOT NULL,
                room INTEGER NOT NULL,
                room_name TEXT,
                hp INTEGER,
                max_hp INTEGER
            );
            CREATE INDEX IF NOT EXISTS positions_by_character ON positions (character, id);
            CREATE TABLE IF NOT EXISTS statlines (
                character TEXT PRIMARY KEY,
                original TEXT NOT NULL,
                saved_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS lairs (
                map INTEGER NOT NULL,
                room INTEGER NOT NULL,
                cleared_ms INTEGER NOT NULL,
                PRIMARY KEY (map, room)
            );",
        )?;
        Ok(Self(Mutex::new(db)))
    }

    /// Adds a position to `character`'s record.
    pub fn record(&self, character: &str, p: &Position) -> rusqlite::Result<()> {
        self.0.lock().expect("history lock").execute(
            "INSERT INTO positions (character, at, why, map, room, room_name, hp, max_hp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                character,
                p.at,
                p.why,
                p.map,
                p.room,
                p.room_name,
                p.hp,
                p.max_hp
            ],
        )?;
        Ok(())
    }

    /// The last position on `character`'s record.
    pub fn last(&self, character: &str) -> Option<Position> {
        self.0
            .lock()
            .expect("history lock")
            .query_row(
                "SELECT map, room, room_name, hp, max_hp, why, at FROM positions
                 WHERE character = ?1 ORDER BY id DESC LIMIT 1",
                [character],
                |r| {
                    Ok(Position {
                        map: r.get(0)?,
                        room: r.get(1)?,
                        room_name: r.get(2)?,
                        hp: r.get(3)?,
                        max_hp: r.get(4)?,
                        why: r.get(5)?,
                        at: r.get(6)?,
                    })
                },
            )
            .optional()
            .ok()
            .flatten()
    }
}

/// When each lair was last cleared (#67), by any bot: lairs are the world's. Kept as
/// milliseconds since 1970, UTC.
impl LairClocks for History {
    fn last_cleared(&self, lair: (u32, u32)) -> Option<SystemTime> {
        let ms: i64 = self
            .0
            .lock()
            .expect("history lock")
            .query_row(
                "SELECT cleared_ms FROM lairs WHERE map = ?1 AND room = ?2",
                params![lair.0, lair.1],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()?;
        Some(UNIX_EPOCH + Duration::from_millis(u64::try_from(ms).ok()?))
    }

    // A failed write can't be reported from here; the session logs each lair cleared.
    fn cleared(&self, lair: (u32, u32), at: SystemTime) {
        let ms = at
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
        let _ = self.0.lock().expect("history lock").execute(
            "INSERT INTO lairs (map, room, cleared_ms) VALUES (?1, ?2, ?3)
             ON CONFLICT (map, room) DO UPDATE SET cleared_ms = ?3",
            params![lair.0, lair.1, ms],
        );
    }
}

impl StatlineStore for History {
    fn saved(&self, name: &str) -> Option<String> {
        self.0
            .lock()
            .expect("history lock")
            .query_row(
                "SELECT original FROM statlines WHERE character = ?1",
                [name],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    // A failed write can't be reported from here; the session logs what it saved.
    fn save(&self, name: &str, original: &str) {
        let _ = self.0.lock().expect("history lock").execute(
            "INSERT INTO statlines (character, original, saved_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (character) DO UPDATE SET original = ?2, saved_at = ?3",
            params![name, original, jiff::Timestamp::now().to_string()],
        );
    }

    fn clear(&self, name: &str) {
        let _ = self
            .0
            .lock()
            .expect("history lock")
            .execute("DELETE FROM statlines WHERE character = ?1", [name]);
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use mp_engine::{LairClocks, StatlineStore};

    use super::History;

    #[test]
    fn a_saved_statline_outlives_the_server_until_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mudparty.sqlite");
        let h = History::open(&path).unwrap();
        assert_eq!(h.saved("Ermias"), None);
        h.save("Ermias", "Full");
        h.save("Ermias", "[HP=%h]:");
        drop(h);
        let h = History::open(&path).unwrap();
        assert_eq!(h.saved("Ermias").as_deref(), Some("[HP=%h]:"));
        h.clear("Ermias");
        assert_eq!(h.saved("Ermias"), None);
    }

    #[test]
    fn a_lairs_clock_outlives_the_server() {
        // Lairs are the world's, not a character's: any bot's kill starts the clock (#67).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mudparty.sqlite");
        let h = History::open(&path).unwrap();
        assert_eq!(h.last_cleared((1, 2156)), None);
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        h.cleared((1, 2156), at - Duration::from_secs(60));
        h.cleared((1, 2156), at);
        drop(h);
        let h = History::open(&path).unwrap();
        assert_eq!(h.last_cleared((1, 2156)), Some(at));
        assert_eq!(h.last_cleared((1, 2150)), None);
    }
}
