//! A bot fights through the server (design §6.4, §7.5; MVP 4b): the server imports the game
//! data from silvermere's database, builds the bot's combat from its YAML and that data, and
//! the bot casts at the arena's practice dummy.

use std::fs;
use std::time::Duration;

use mp_engine::{Report, SessionState};
use mp_silvermere::{StockChar, StockSilvermere, WAIT};
use mudparty_server::{BotEvent, Server};
use tokio::time::{Instant, timeout};

const ERMIAS: &str = r#"schema: 1
name: Ermias
login:
  account: Ermias
combat:
  targeting:
    - priority: ["practice dummy"]
  tiers:
    - spell: mmis
      min_mp: 1
  melee: a
"#;

#[tokio::test(flavor = "multi_thread")]
async fn a_bot_fights_with_the_combat_its_file_gives_it() {
    let sm = StockSilvermere::start(&[StockChar::new("Ermias", "Mage").knows(&["mmis"])], true)
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let port = sm.telnet_addr().port();
    let db = sm.db_path().display().to_string().replace('\\', "/");
    fs::write(
        dir.path().join("mudparty.toml"),
        format!(
            "schema = 1\n[game]\nhost = \"127.0.0.1\"\nport = {port}\n[gamedata]\nsilvermere_db = \"{db}\"\n"
        ),
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("characters")).unwrap();
    fs::write(dir.path().join("characters/Ermias.yaml"), ERMIAS).unwrap();
    fs::write(
        dir.path().join("secrets.yaml"),
        "accounts:\n  Ermias: \"ermias1\"\n",
    )
    .unwrap();

    let server = Server::start(dir.path()).await.unwrap();
    assert!(
        dir.path().join("gamedata/gamedata.sqlite").exists(),
        "the game data was imported"
    );
    let mut rx = server.subscribe();
    server.start_bot("Ermias").unwrap();
    let in_game = BotEvent::Session {
        name: "Ermias".into(),
        report: Report::State(SessionState::InGame),
    };
    loop {
        if timeout(WAIT, rx.recv()).await.unwrap().unwrap() == in_game {
            break;
        }
    }
    let want = BotEvent::Session {
        name: "Ermias".into(),
        report: Report::Sent("mmis practice dummy".into()),
    };
    let deadline = Instant::now() + WAIT;
    'fight: loop {
        assert!(Instant::now() < deadline, "the bot never cast at the dummy");
        sm.tick(5).await;
        while let Ok(Ok(e)) = timeout(Duration::from_millis(150), rx.recv()).await {
            if e == want {
                break 'fight;
            }
        }
    }
    server.shutdown().await;
    sm.stop().await.unwrap();
}

/// A data root for the stock world `sm`, with Ermias's file `yaml` and password.
fn data_root(dir: &std::path::Path, sm: &StockSilvermere, yaml: &str) {
    let port = sm.telnet_addr().port();
    let db = sm.db_path().display().to_string().replace('\\', "/");
    fs::write(
        dir.join("mudparty.toml"),
        format!(
            "schema = 1
[game]
host = \"127.0.0.1\"
port = {port}
[gamedata]
silvermere_db = \"{db}\"
"
        ),
    )
    .unwrap();
    fs::create_dir_all(dir.join("characters")).unwrap();
    fs::write(dir.join("characters/Ermias.yaml"), yaml).unwrap();
    fs::write(
        dir.join("secrets.yaml"),
        "accounts:
  Ermias: \"ermias1\"
",
    )
    .unwrap();
}

const PLANNED: &str = r#"schema: 1
name: Ermias
login:
  account: Ermias
combat:
  targeting:
    - priority: ["practice dummy"]
  by_level:
    - levels: 1-
      spells:
        - cast: mmis
          max_casts: 2
  melee: a
"#;

// The whole way (#125): the level from `exp`, the cast line from the game data, the casts
// counted from what the game prints, then melee.
#[tokio::test(flavor = "multi_thread")]
async fn a_bot_melees_once_its_spell_s_casts_are_spent() {
    let sm = StockSilvermere::start(&[StockChar::new("Ermias", "Mage").knows(&["mmis"])], true)
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    data_root(dir.path(), &sm, PLANNED);
    let server = Server::start(dir.path()).await.unwrap();
    let mut rx = server.subscribe();
    server.start_bot("Ermias").unwrap();
    let sent = |line: &str| BotEvent::Session {
        name: "Ermias".into(),
        report: Report::Sent(line.into()),
    };
    let (cast, melee) = (sent("mmis practice dummy"), sent("a practice dummy"));
    let mut casts_sent = 0;
    let deadline = Instant::now() + WAIT * 2;
    'fight: loop {
        assert!(Instant::now() < deadline, "the bot never went to melee");
        sm.tick(5).await;
        while let Ok(Ok(e)) = timeout(Duration::from_millis(150), rx.recv()).await {
            if e == cast {
                casts_sent += 1;
            }
            if e == melee {
                assert!(casts_sent > 0, "the bot cast first");
                break 'fight;
            }
        }
    }
    server.shutdown().await;
    sm.stop().await.unwrap();
}

// A game data file from before the cast lines (#125) is imported again.
#[tokio::test(flavor = "multi_thread")]
async fn an_old_game_data_file_is_imported_again() {
    let sm = StockSilvermere::start(&[StockChar::new("Ermias", "Mage")], false)
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    data_root(dir.path(), &sm, PLANNED);
    let old = dir.path().join("gamedata/gamedata.sqlite");
    fs::create_dir_all(old.parent().unwrap()).unwrap();
    let conn = rusqlite::Connection::open(&old).unwrap();
    conn.execute_batch(
        "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         INSERT INTO meta VALUES ('schema', '1'), ('gamedata_rev', 'old');",
    )
    .unwrap();
    drop(conn);
    let server = Server::start(dir.path()).await.unwrap();
    let gd = mp_data::GameData::open(&old).unwrap();
    assert_eq!(gd.schema().unwrap(), mp_data::SCHEMA_VERSION);
    assert!(gd.spell_by_short("mmis").unwrap().is_some());
    server.shutdown().await;
    sm.stop().await.unwrap();
}
