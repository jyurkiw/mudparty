//! Test support (design §10.4): a real silvermere, started in-process on a temp SQLite with
//! a small fixture world and the characters a test asks for.
//!
//! The fixture follows silvermere's own Layer-3 tests (`bin/silvermere/tests/common`): the
//! four special rooms of the default config on map 1, blank records for two items, two
//! monsters, one class and one race, built from the engine's zeroed records.

mod client;
mod relay;
mod stock;

pub use client::{RawClient, WAIT};
pub use relay::Relay;
pub use stock::{ARENA, StockChar, StockSilvermere, stock_template};

use std::net::SocketAddr;

use silvermere::config::Config;
use silvermere_auth::Role;
use silvermere_auth::cli::create_with_cost;
use silvermere_auth::password::Cost;
use silvermere_db::{Db, TestDb};
use silvermere_engine::world::blank;
use silvermere_engine::{SeededRng, SystemClock, clear_player};
use silvermere_types::config::GameConfig;
use silvermere_types::enums::StatlineMode;
use silvermere_types::flags::PlayerSettings;
use silvermere_types::ids::{ClassNo, ItemNo, MapId, MonsterNo, RaceNo, RoomId, RoomKey};

/// A character to create, with its account.
#[derive(Debug, Clone)]
pub struct Player {
    first_name: String,
    password: String,
    hp: i16,
}

impl Player {
    /// A level 1 character `first_name` in the start room, whose account has the same name
    /// and `password`.
    pub fn new(first_name: &str, password: &str) -> Self {
        Self {
            first_name: first_name.to_owned(),
            password: password.to_owned(),
            hp: 35,
        }
    }

    /// Current and maximum hit points.
    pub fn hp(mut self, hp: i16) -> Self {
        self.hp = hp;
        self
    }
}

/// A running in-process silvermere. Stop it with [`Silvermere::stop`].
pub struct Silvermere {
    running: silvermere::Running,
    _db: TestDb,
}

impl Silvermere {
    /// Starts silvermere on a fresh temp SQLite holding the fixture world and `players`.
    pub async fn start(players: &[Player]) -> anyhow::Result<Self> {
        let db = TestDb::sqlite_temp().await?;
        seed_world(db.db()).await?;
        for p in players {
            seed_player(db.db(), p).await?;
        }
        let mut cfg = Config::default();
        cfg.server.telnet_bind = "127.0.0.1:0".into();
        cfg.server.admin_bind = "127.0.0.1:0".into();
        cfg.database.url = db.db().url().to_string();
        let running = silvermere::serve_in_process(cfg, SystemClock, SeededRng::new(7)).await?;
        Ok(Self { running, _db: db })
    }

    /// Where its telnet listener is.
    pub fn telnet_addr(&self) -> SocketAddr {
        self.running.telnet_addr
    }

    /// Stops it and waits for the engine to finish.
    pub async fn stop(self) -> anyhow::Result<()> {
        self.running.stop().await.map(drop)
    }
}

/// The four special rooms of the default config.
fn special_rooms() -> [u32; 4] {
    let g = GameConfig::default();
    [g.start_room, g.reinc_room, g.evil_reinc_room, g.jail_room]
}

async fn seed_world(db: &Db) -> anyhow::Result<()> {
    for (i, n) in special_rooms().into_iter().enumerate() {
        let mut r = blank::clear_room(RoomKey::new(1, n));
        r.name = format!("Room {}", i + 1);
        db.rooms().insert(&r).await?;
    }
    for n in [1, 2] {
        db.items().insert(&blank::clear_item(ItemNo(n))).await?;
        db.monsters()
            .insert(&blank::clear_known_monster(MonsterNo(n)))
            .await?;
    }
    db.classes().insert(&blank::clear_class(ClassNo(1))).await?;
    db.races().insert(&blank::clear_race(RaceNo(1))).await?;
    Ok(())
}

/// A stored character as silvermere's loader accepts it (its `login_e2e` test's Aldric):
/// record version 22, level 1, stats inside the blank race's range, the name validated.
async fn seed_player(db: &Db, p: &Player) -> anyhow::Result<()> {
    let account =
        create_with_cost(db, &p.first_name, &p.password, &[Role::Player], Cost::Low).await?;
    let mut c = clear_player();
    c.userid = p.first_name.to_uppercase();
    c.firstname = p.first_name.clone();
    c.map = MapId(1);
    c.room = RoomId(special_rooms()[0]);
    c.hp = p.hp;
    c.maxhp = p.hp;
    c.class = ClassNo(1);
    c.race = RaceNo(1);
    c.statline = StatlineMode::Full;
    c.version = 22;
    c.level = 1;
    (c.str, c.intel, c.wil, c.agi, c.hea, c.cha) = (50, 50, 50, 50, 50, 50);
    (c.basstr, c.basint, c.baswil, c.basagi, c.bashea, c.bascha) = (50, 50, 50, 50, 50, 50);
    // The recomputed maximum is health/2 + hpbonus: 25 + hpbonus.
    c.hpbonus = i32::from(p.hp) - 25;
    c.settings |= PlayerSettings::VALIDATED;
    db.characters().insert(account.id, &c).await?;
    Ok(())
}
