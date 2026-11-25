//! @ commands through the server (design §6.3, §10.4 scenario 7): the bot's YAML says who
//! may command it and which engines start on; an allowed player's command is answered,
//! and a switch it flips reaches the server's feed.

use std::fs;

use mp_engine::{AutoState, Report, SessionState};
use mp_parse::{Event, Item};
use mp_silvermere::{RawClient, StockChar, StockSilvermere, WAIT};
use mudparty_server::{BotEvent, Server};
use tokio::sync::broadcast;
use tokio::time::{Instant, timeout};

const ERMIAS: &str = "schema: 1
name: Ermias
login:
  account: Ermias
remote:
  allow: [Raijin]
  warn_denied: true
automation:
  rest: false
";

async fn until(rx: &mut broadcast::Receiver<BotEvent>, want: impl Fn(&Report) -> bool) {
    let deadline = Instant::now() + WAIT;
    loop {
        let wait = deadline.saturating_duration_since(Instant::now());
        match timeout(wait, rx.recv()).await {
            Ok(Ok(BotEvent::Session { report, .. })) if want(&report) => return,
            Ok(Ok(_)) => {}
            _ => panic!("never saw the report"),
        }
    }
}

async fn ask(player: &mut RawClient, cmd: &str) -> String {
    player.send(&format!("/Ermias {cmd}")).await.unwrap();
    let Item::Event(Event::Telepath { text, .. }) = player
        .read_until(|i| matches!(i, Item::Event(Event::Telepath { who, .. }) if who == "Ermias"))
        .await
        .expect("a reply")
    else {
        unreachable!()
    };
    text
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bots_file_says_who_may_command_it() {
    let sm = StockSilvermere::start(
        &[
            StockChar::new("Raijin", "Warrior"),
            StockChar::new("Ermias", "Mage"),
        ],
        false,
    )
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

    let mut raijin = RawClient::log_in(sm.telnet_addr(), "Raijin", "raijin1")
        .await
        .unwrap();
    let server = Server::start(dir.path()).await.unwrap();
    let mut rx = server.subscribe();
    server.start_bot("Ermias").unwrap();
    until(&mut rx, |r| *r == Report::State(SessionState::InGame)).await;
    until(&mut rx, |r| matches!(r, Report::Vitals(_))).await;

    // More telepaths than one 3 s page cycle takes, so both get room for them.
    sm.on_world(|w| {
        for id in w.session_ids() {
            if let Some(p) = w.session_mut(id).and_then(|s| s.character.as_mut()) {
                p.pagemax = 50;
            }
        }
    })
    .await;
    // The room's name and exits come from the game data the server imported.
    assert_eq!(
        ask(&mut raijin, "@where").await,
        "{Arena Practice Room (map 1, room 206); exits: north, up}"
    );
    assert_eq!(
        ask(&mut raijin, "@version").await,
        format!("{{mudparty {}}}", env!("CARGO_PKG_VERSION"))
    );
    // The file starts auto-rest off, so a bare toggle turns it on.
    assert_eq!(ask(&mut raijin, "@auto-rest").await, "{@auto-rest: on}");
    until(&mut rx, |r| {
        matches!(r, Report::Automation(AutoState { rest: true, .. }))
    })
    .await;
    // Combat starts on, as the file leaves it, so a bare toggle turns it off.
    assert_eq!(
        ask(&mut raijin, "@auto-combat").await,
        "{@auto-combat: off}"
    );
    server.shutdown().await;
    sm.stop().await.unwrap();
}
