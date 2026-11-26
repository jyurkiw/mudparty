//! Walking through the server (#52) in the stock world's Newhaven: a bot told to go to a
//! room plans its route over the imported game data and walks it, through the arena's
//! door as well.

use std::fs;

use mp_engine::{Report, SessionState, WalkState};
use mp_silvermere::{StockChar, StockSilvermere, WAIT};
use mudparty_server::{BotEvent, Order, Server};
use tokio::sync::broadcast;
use tokio::time::{Instant, timeout};

const ERMIAS: &str = "schema: 1
name: Ermias
login:
  account: Ermias
automation:
  combat: false
  rest: false
";

async fn until(rx: &mut broadcast::Receiver<BotEvent>, want: impl Fn(&Report) -> bool) -> Report {
    let deadline = Instant::now() + WAIT;
    loop {
        let wait = deadline.saturating_duration_since(Instant::now());
        match timeout(wait, rx.recv()).await {
            Ok(Ok(BotEvent::Session { report, .. })) if want(&report) => return report,
            Ok(Ok(_)) => {}
            _ => panic!("never saw the report"),
        }
    }
}

/// Orders a walk and waits for its end, returning how it ended.
async fn walk(
    server: &std::sync::Arc<Server>,
    rx: &mut broadcast::Receiver<BotEvent>,
    room: &str,
) -> WalkState {
    let r = server.order("Ermias", Order::Goto { room: room.into() });
    assert!(r.is_ok(), "{r:?}");
    let Report::Walk(end) = until(rx, |r| {
        matches!(r, Report::Walk(WalkState::Arrived | WalkState::Failed(_)))
    })
    .await
    else {
        unreachable!()
    };
    end
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bot_walks_newhaven_by_the_game_data() {
    // Ermias starts in Newhaven's arena.
    let sm = StockSilvermere::start(
        &[StockChar {
            room: 2150,
            ..StockChar::new("Ermias", "Mage")
        }],
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
            "schema = 1\nlogoff_timeout = \"5s\"\n[game]\nhost = \"127.0.0.1\"\nport = {port}\n[gamedata]\nsilvermere_db = \"{db}\"\n"
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
    let mut rx = server.subscribe();
    server.start_bot("Ermias").unwrap();
    until(&mut rx, |r| *r == Report::State(SessionState::InGame)).await;
    until(
        &mut rx,
        |r| matches!(r, Report::Vitals(v) if v.max_hp.is_some()),
    )
    .await;

    // Up the stairs and east to the Weapons Shop, and back down to the arena.
    assert_eq!(walk(&server, &mut rx, "1/2141").await, WalkState::Arrived);
    let card = server
        .party()
        .into_iter()
        .find(|c| c.name == "Ermias")
        .unwrap();
    assert_eq!(card.room.as_ref().map(|r| r.room), Some(2141));
    assert_eq!(
        card.room.and_then(|r| r.name).as_deref(),
        Some("Newhaven, Weapons Shop")
    );
    assert_eq!(walk(&server, &mut rx, "1/2150").await, WalkState::Arrived);
    // North through the arena's door to the cave bear's cavern, and back.
    assert_eq!(walk(&server, &mut rx, "1/2156").await, WalkState::Arrived);
    assert_eq!(walk(&server, &mut rx, "1/2150").await, WalkState::Arrived);

    // Orders it can't carry out are answered.
    assert!(
        server
            .order(
                "Ermias",
                Order::Goto {
                    room: "the moon".into()
                }
            )
            .is_err()
    );
    server.shutdown().await;
    sm.stop().await.unwrap();
}
