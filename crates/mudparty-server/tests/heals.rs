//! A healer bot heals another bot through the server (#126): the server shares each
//! character's vitals, read off its own prompts, with the others it runs. On the stock world.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use mp_engine::{Report, SessionState};
use mp_silvermere::{StockChar, StockSilvermere, WAIT};
use mudparty_server::{BotEvent, Order, Server};
use tokio::sync::broadcast;
use tokio::time::{Instant, timeout};

const ERMIAS: &str = r#"schema: 1
name: Ermias
login:
  account: Ermias
combat:
  by_level:
    - levels: 1-
      heals:
        - cast: mihe
          on: [party]
          when: { hp_below: 80% }
"#;

const GROND: &str = "schema: 1\nname: Grond\nlogin:\n  account: Grond\n";

fn data_root(dir: &Path, sm: &StockSilvermere) {
    let port = sm.telnet_addr().port();
    let db = sm.db_path().display().to_string().replace('\\', "/");
    fs::write(
        dir.join("mudparty.toml"),
        format!(
            "schema = 1\n[game]\nhost = \"127.0.0.1\"\nport = {port}\n[gamedata]\nsilvermere_db = \"{db}\"\n"
        ),
    )
    .unwrap();
    fs::create_dir_all(dir.join("characters")).unwrap();
    fs::write(dir.join("characters/Ermias.yaml"), ERMIAS).unwrap();
    fs::write(dir.join("characters/Grond.yaml"), GROND).unwrap();
    fs::write(
        dir.join("secrets.yaml"),
        "accounts:\n  Ermias: \"ermias1\"\n  Grond: \"grond1\"\n",
    )
    .unwrap();
}

async fn in_game(rx: &mut broadcast::Receiver<BotEvent>, name: &str) {
    let want = BotEvent::Session {
        name: name.into(),
        report: Report::State(SessionState::InGame),
    };
    loop {
        if timeout(WAIT, rx.recv()).await.unwrap().unwrap() == want {
            return;
        }
    }
}

async fn set_hp(sm: &StockSilvermere, name: &'static str, hp: i16) {
    sm.on_world(move |w| {
        for id in w.session_ids() {
            if let Some(p) = w.session_mut(id).and_then(|s| s.character.as_mut())
                && p.firstname == name
            {
                p.hp = hp;
            }
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_healer_heals_a_hurt_bot_beside_it() {
    let sm = StockSilvermere::start(
        &[
            StockChar::new("Ermias", "Cleric").knows(&["mihe"]),
            StockChar::new("Grond", "Warrior"),
        ],
        true,
    )
    .await
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    data_root(dir.path(), &sm);
    let server: Arc<Server> = Server::start(dir.path()).await.unwrap();
    let mut rx = server.subscribe();
    // Grond first, so Ermias's room display lists him.
    server.start_bot("Grond").unwrap();
    in_game(&mut rx, "Grond").await;
    server.start_bot("Ermias").unwrap();
    in_game(&mut rx, "Ermias").await;
    set_hp(&sm, "Grond", 3).await;
    // A line for Grond: its prompt after shows the wound, and the server passes it on.
    server
        .order("Grond", Order::Command { text: "st".into() })
        .unwrap();
    let want = BotEvent::Session {
        name: "Ermias".into(),
        report: Report::Sent("mihe Grond".into()),
    };
    let deadline = Instant::now() + WAIT;
    'heal: loop {
        assert!(Instant::now() < deadline, "Ermias never healed Grond");
        sm.tick(5).await;
        while let Ok(Ok(e)) = timeout(Duration::from_millis(150), rx.recv()).await {
            if e == want {
                break 'heal;
            }
        }
    }
    server.shutdown().await;
    sm.stop().await.unwrap();
}
