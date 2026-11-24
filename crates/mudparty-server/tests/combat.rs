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
