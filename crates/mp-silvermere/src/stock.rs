//! silvermere's stock world for test fights (design §10.4), the way silvermere's own tests
//! build it (`bin/silvermere/tests/common/stock.rs`, `m3.rs`):
//!
//! - the stock package (downloaded once into `~/.cache/silvermere/stock` by
//!   `silvermere-testkit`) imported into a SQLite template, once per pinned silvermere and
//!   shared by every test process;
//! - characters created through the game's own creation dialogue, so their records are
//!   ones the game made, then edited: spells known, mana, the room;
//! - an optional hand-driven clock, so a combat round takes no time.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, anyhow};
use regex::Regex;
use silvermere::config::Config;
use silvermere_auth::Role;
use silvermere_auth::cli::create_with_cost;
use silvermere_auth::password::Cost;
use silvermere_db::Db;
use silvermere_engine::{EngineInput, SeededRng, SystemClock, World};
use silvermere_import::{ImportOptions, Importer};
use silvermere_testkit::TelnetClient;
use silvermere_types::ids::{MapId, RoomId, SpellNo};

use crate::client::WAIT;

/// The silvermere commit the template is built from; keep in step with the pin in the
/// workspace `Cargo.toml`, or a stale template is reused.
const SILVERMERE_REV: &str = "8aebd91b52af66a1c4f77ab7c0458d1d1965bb7a";

/// The arena practice room on map 1, with its 9999-HP practice dummy.
pub const ARENA: u32 = 206;

/// A character for the stock world.
#[derive(Debug, Clone)]
pub struct StockChar {
    pub name: String,
    pub password: String,
    /// The class, by its name in the creation list (`Warrior`, `Mage`, …).
    pub class: String,
    /// Spells known, by cast code (`mmis`).
    pub spells: Vec<String>,
    /// The room on map 1 to start in.
    pub room: u32,
    /// Experience, for a character that has earned levels it hasn't trained (#56).
    pub exp: u32,
    /// Silver nobles carried, as well as what creation gave it.
    pub silver: u32,
    /// Strength, health and agility, as a player spends creation points (#57); none keeps
    /// what creation gave it, with no points spent.
    pub stats: Option<Stats>,
}

/// Base stats a character is given in place of creation's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub strength: u16,
    pub health: u16,
    pub agility: u16,
}

impl StockChar {
    /// `name`, password `<name in lower case>1`, a `class` in the arena.
    pub fn new(name: &str, class: &str) -> Self {
        Self {
            name: name.to_owned(),
            password: format!("{}1", name.to_lowercase()),
            class: class.to_owned(),
            spells: Vec::new(),
            room: ARENA,
            exp: 0,
            silver: 0,
            stats: None,
        }
    }

    pub fn knows(mut self, spells: &[&str]) -> Self {
        self.spells = spells.iter().map(|s| (*s).to_owned()).collect();
        self
    }
}

fn block_on<T: Send + 'static>(f: impl std::future::Future<Output = T> + Send + 'static) -> T {
    // Tests run inside a tokio runtime, which can't block on another, so the work gets a
    // thread and a runtime of its own.
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime")
            .block_on(f)
    })
    .join()
    .expect("the template thread")
}

/// The imported stock world: a SQLite file built once and then only copied.
pub fn stock_template() -> PathBuf {
    static T: OnceLock<PathBuf> = OnceLock::new();
    T.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("mudparty-stock-{}", &SILVERMERE_REV[..12]));
        let path = dir.join("stock.db");
        if path.exists() {
            return path;
        }
        std::fs::create_dir_all(&dir).expect("the template folder");
        let building = dir.join(format!("stock.db.building-{}", std::process::id()));
        let _ = std::fs::remove_file(&building);
        let from = silvermere_testkit::stock::dir();
        let url = format!("sqlite://{}", building.display());
        block_on(async move {
            let db = Db::connect_and_migrate(&url)
                .await
                .expect("a template database");
            Importer::new(ImportOptions::new(&from), Some(&db))
                .run()
                .await
                .expect("the stock import");
            db.close().await;
        });
        // Another process may have finished first; either file is the same world.
        if std::fs::rename(&building, &path).is_err() {
            let _ = std::fs::remove_file(&building);
        }
        path
    })
    .clone()
}

/// A config for an in-process server on `url`, with the game's long text installed.
fn config(url: &str, data_dir: &Path, manual_clock: bool) -> Config {
    let mut cfg = Config::default();
    cfg.server.telnet_bind = "127.0.0.1:0".into();
    cfg.server.admin_bind = "127.0.0.1:0".into();
    cfg.database.url = url.to_owned();
    cfg.data_dir = data_dir.to_owned();
    if manual_clock {
        // The real-time fast ticker at a day: cycles run only when a test sends a tick.
        cfg.game.sec_fast = 86_400;
    }
    silvermere_text::package::load_and_install(&silvermere_testkit::stock::assets_dir())
        .expect("the stock package's text");
    cfg
}

async fn run_on(
    to_engine: &std::sync::mpsc::Sender<EngineInput>,
    f: impl FnOnce(&mut World) + Send + 'static,
) {
    let (tx, rx) = std::sync::mpsc::channel();
    to_engine
        .send(EngineInput::Custom {
            session: None,
            run: Box::new(move |w| {
                f(w);
                let _ = tx.send(());
            }),
        })
        .expect("the engine is running");
    tokio::task::spawn_blocking(move || rx.recv())
        .await
        .expect("the wait")
        .expect("the engine ran it");
}

/// Logs in to the `[MAJORMUD]:` menu.
async fn to_menu(c: &mut TelnetClient, user: &str, password: &str) -> anyhow::Result<()> {
    c.until("Username:", WAIT).await?;
    c.send_line(user).await?;
    c.until("Password:", WAIT).await?;
    c.send_line(password).await?;
    c.until(r"Make your selection \(X to exit\):", WAIT).await?;
    c.send_line("M").await?;
    c.until(r"\[MAJORMUD\]:", WAIT).await?;
    Ok(())
}

/// Creates `ch` through the game's line-mode creation (a Human; Strength and Health 50, as
/// silvermere's own test characters), then leaves the realm cleanly so the record is saved.
async fn create(
    addr: SocketAddr,
    to_engine: &std::sync::mpsc::Sender<EngineInput>,
    ch: &StockChar,
) -> anyhow::Result<()> {
    let mut c = TelnetClient::connect(addr).await?.with_naws(79, 24);
    to_menu(&mut c, &ch.name, &ch.password).await?;
    c.send_line("E").await?;
    c.until("choose your race", WAIT).await?;
    c.send_line("1").await?;
    let list = c.until("choose your class", WAIT).await?.plain;
    let re = Regex::new(&format!(r"(?m)(\d+)\W+{}\b", regex::escape(&ch.class)))?;
    let number = re
        .captures(&list)
        .map(|m| m[1].to_owned())
        .ok_or_else(|| anyhow!("no class {:?} in the creation list:\n{list}", ch.class))?;
    c.send_line(&number).await?;
    // The lawful question is asked of some classes only.
    let got = c.until(r"Lawful\?|Given Name", WAIT).await?.plain;
    if got.contains("Lawful?") {
        c.send_line("N").await?;
        c.until("Given Name", WAIT).await?;
    }
    for (line, next) in [
        ("", "Family Name"),
        ("Test", "Strength"),
        ("50", "Intellect"),
        ("", "Willpower"),
        ("", "Agility"),
        ("", "Health"),
        ("50", "Charm"),
        ("", "Hair Length"),
        ("", "Hair Colour"),
        ("", "Eye Colour"),
        ("", "Exit: SAVE"),
        ("", r"\[HP=-?\d+[^\]\n]*\]:"),
    ] {
        c.send_line(line).await?;
        c.until(next, WAIT)
            .await
            .with_context(|| format!("creating {}: waiting for {next}", ch.name))?;
    }
    c.send_line("x").await?;
    c.until("silent meditation", WAIT).await?;
    let name = ch.name.clone();
    run_on(to_engine, move |w| {
        use silvermere_engine::state::login::leave::{exit_countdown, is_exiting};
        let id = w.session_ids().into_iter().find(|&id| {
            w.session(id)
                .and_then(|s| s.character.as_ref())
                .is_some_and(|p| p.firstname == name)
        });
        if let Some(id) = id {
            for _ in 0..1000 {
                if !is_exiting(w, id) {
                    break;
                }
                exit_countdown(w, id);
            }
        }
    })
    .await;
    c.until(r"\[MAJORMUD\]:", WAIT).await?;
    c.disconnect().await;
    Ok(())
}

/// Makes `dir` an empty folder for a world. A folder of the same name may be left from an
/// earlier run: one whose test panicked never reached [`StockSilvermere::stop`], and
/// Windows reuses process ids. Its write-ahead log would replay onto the fresh copy, so it
/// goes first (#120). No live process shares the name, as the id is in it.
fn fresh_dir(dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    std::fs::create_dir_all(dir)
}

/// A running silvermere on a copy of the stock world.
pub struct StockSilvermere {
    running: silvermere::Running,
    dir: PathBuf,
}

impl StockSilvermere {
    /// Copies the stock world, creates `chars` in it, and starts the game. With
    /// `manual_clock`, rounds run only on [`StockSilvermere::tick`].
    pub async fn start(chars: &[StockChar], manual_clock: bool) -> anyhow::Result<Self> {
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let template = stock_template();
        let dir = template
            .parent()
            .expect("a folder")
            .join(format!("t{}-{n}", std::process::id()));
        fresh_dir(&dir)?;
        let db_path = dir.join("test.db");
        std::fs::copy(&template, &db_path)?;
        let url = format!("sqlite://{}", db_path.display());
        {
            let db = Db::connect_and_migrate(&url).await?;
            for ch in chars {
                create_with_cost(&db, &ch.name, &ch.password, &[Role::Player], Cost::Low).await?;
            }
            db.close().await;
        }
        // Create the characters on a server of their own, then edit the saved records.
        let creator =
            silvermere::serve_in_process(config(&url, &dir, false), SystemClock, SeededRng::new(7))
                .await?;
        for ch in chars {
            create(creator.telnet_addr, &creator.to_engine, ch).await?;
        }
        creator.stop().await?;
        let db = Db::connect_and_migrate(&url).await?;
        for ch in chars {
            let mut row = db
                .characters()
                .by_first_name(&ch.name)
                .await?
                .ok_or_else(|| anyhow!("{} was not saved", ch.name))?;
            let c = &mut row.character;
            c.map = MapId(1);
            c.room = RoomId(ch.room);
            c.trailmaps[0] = MapId(1);
            c.trailrooms[0] = RoomId(ch.room);
            if ch.exp > 0 {
                c.expbil = 0;
                c.exprem = ch.exp;
            }
            c.coins.silver += ch.silver;
            if let Some(s) = ch.stats {
                // Base and current alike: the game works the rest out (HP from health) as
                // it loads the character.
                (c.basstr, c.str) = (s.strength, s.strength);
                (c.bashea, c.hea) = (s.health, s.health);
                (c.basagi, c.agi) = (s.agility, s.agility);
            }
            let mut book = Vec::new();
            for short in &ch.spells {
                let spell = db
                    .spells()
                    .all()
                    .await?
                    .into_iter()
                    .find(|s| s.shortnm.eq_ignore_ascii_case(short))
                    .ok_or_else(|| anyhow!("no spell {short:?} in the stock world"))?;
                book.push(spell.number);
            }
            if !book.is_empty() {
                c.nspellslots = c.nspellslots.max(book.len() as u8);
                for (i, s) in book.into_iter().enumerate() {
                    if i < c.spellbook.len() {
                        c.spellbook[i] = s;
                    } else {
                        c.spellbook.push(s);
                    }
                }
            }
            db.characters().update(row.id, &row.character).await?;
        }
        db.close().await;
        let running = silvermere::serve_in_process(
            config(&url, &dir, manual_clock),
            SystemClock,
            SeededRng::new(7),
        )
        .await?;
        Ok(Self { running, dir })
    }

    pub fn telnet_addr(&self) -> SocketAddr {
        self.running.telnet_addr
    }

    /// The world's database file (silvermere's `silvermere.db` for this test), for
    /// mudparty's game-data import.
    pub fn db_path(&self) -> PathBuf {
        self.dir.join("test.db")
    }

    /// Runs `n` fast cycles (a combat round is several) and waits until they are done.
    pub async fn tick(&self, n: u32) {
        for _ in 0..n {
            self.running
                .to_engine
                .send(EngineInput::Tick)
                .expect("the engine is running");
        }
        run_on(&self.running.to_engine, |_| ()).await;
    }

    /// Runs `f` on the game's world, on the engine thread.
    pub async fn on_world(&self, f: impl FnOnce(&mut World) + Send + 'static) {
        run_on(&self.running.to_engine, f).await;
    }

    /// Takes the hit spells off every monster's attacks: the arena's acid slime burns with
    /// one. A monster's spell on a player recalculates the player's stats (silvermere's
    /// `monsters/cast.rs` calls `items::equip::recalc`), which puts a max HP a test raised
    /// back to the class's own, and the HP down to it. A test that boosts HP so the
    /// monsters can't matter calls this first (#129).
    pub async fn without_hit_spells(&self) {
        self.on_world(|w| {
            for m in w.monsters.values_mut() {
                for a in &mut m.attacks {
                    a.hitspell = SpellNo(0);
                }
            }
        })
        .await;
    }

    pub async fn stop(self) -> anyhow::Result<()> {
        self.running.stop().await?;
        let _ = std::fs::remove_dir_all(&self.dir);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // A test that panics never reaches `stop`, so its folder stays, write-ahead log and
    // all; Windows hands its process id to a later run, whose world then lands in the same
    // folder. SQLite would replay the old log onto the fresh copy ("an account named Ermias
    // exists already"), so the folder is emptied first (#120).
    #[test]
    fn a_world_s_folder_is_emptied_of_an_earlier_run() {
        let dir = std::env::temp_dir().join(format!(
            "mudparty-fresh-dir-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("test.db"), b"old").unwrap();
        std::fs::write(dir.join("test.db-wal"), b"old log").unwrap();
        std::fs::write(dir.join("assets").join("x"), b"old").unwrap();
        super::fresh_dir(&dir).unwrap();
        let left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(left.is_empty(), "{left:?}");
    }

    #[test]
    fn a_world_s_folder_is_made_when_missing() {
        let dir = std::env::temp_dir()
            .join(format!("mudparty-fresh-dir-new-{}", std::process::id()))
            .join("t1-0");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
        super::fresh_dir(&dir).unwrap();
        assert!(dir.is_dir());
        std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }
}
