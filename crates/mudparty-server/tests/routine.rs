//! A routine through the server (#53) in the stock world's Newhaven: a bot walks down to
//! its camp in the arena, leaves for Narrow Road to rest when its HP falls, comes back, and
//! pauses and goes on when told.

use std::fs;
use std::time::Duration;

use mp_engine::{Report, RoutinePart, SessionState, WalkState};
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
rest:
  start_when_any: { hp: 50% }
  stop_when_all: { hp: 100% }
places:
  arena: 1/2150
  road: 1/2146
routine:
  camp: arena
  rest_at: road
  look_every: 1s
";

/// Waits for a report `want` picks, running the game's clock (it runs only when told) as
/// it waits.
async fn until(
    sm: &StockSilvermere,
    rx: &mut broadcast::Receiver<BotEvent>,
    what: &str,
    want: impl Fn(&Report) -> bool,
) -> Report {
    let deadline = Instant::now() + WAIT;
    loop {
        assert!(Instant::now() < deadline, "never saw {what}");
        while let Ok(Ok(ev)) = timeout(Duration::from_millis(100), rx.recv()).await {
            if let BotEvent::Session { report, .. } = ev
                && want(&report)
            {
                return report;
            }
        }
        sm.tick(2).await;
    }
}

async fn set_hp(sm: &StockSilvermere, of_max: f32) {
    sm.on_world(move |w| {
        for id in w.session_ids() {
            if let Some(p) = w.session_mut(id).and_then(|s| s.character.as_mut())
                && p.firstname == "Ermias"
            {
                p.hp = (p.maxhp as f32 * of_max) as _;
            }
        }
    })
    .await;
}

fn card_room(server: &Server) -> Option<u32> {
    server
        .party()
        .into_iter()
        .find(|c| c.name == "Ermias")?
        .room
        .map(|r| r.room)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bot_camps_rests_up_the_stairs_and_comes_back() {
    // Ermias starts on Narrow Road, one room up from the arena.
    let sm = StockSilvermere::start(
        &[StockChar {
            room: 2146,
            ..StockChar::new("Ermias", "Mage")
        }],
        true,
    )
    .await
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let port = sm.telnet_addr().port();
    let db = sm.db_path().display().to_string().replace('\\', "/");
    fs::write(
        root.join("mudparty.toml"),
        format!(
            "schema = 1\nlogoff_timeout = \"5s\"\n[game]\nhost = \"127.0.0.1\"\nport = {port}\n[gamedata]\nsilvermere_db = \"{db}\"\n"
        ),
    )
    .unwrap();
    fs::create_dir_all(root.join("characters")).unwrap();
    fs::write(root.join("characters/Ermias.yaml"), ERMIAS).unwrap();
    fs::write(
        root.join("secrets.yaml"),
        "accounts:\n  Ermias: \"ermias1\"\n",
    )
    .unwrap();
    let server = Server::start(root).await.unwrap();
    let mut rx = server.subscribe();
    server.start_bot("Ermias").unwrap();
    until(&sm, &mut rx, "in the game", |r| {
        *r == Report::State(SessionState::InGame)
    })
    .await;
    // The game's clock runs as the test waits, and the arena's monsters (an acid slime)
    // come and hit a bot that doesn't fight back. Enough HP that they can't push it below
    // the retreat line keeps the camp a camp until the test hurts it.
    sm.on_world(|w| {
        for id in w.session_ids() {
            if let Some(p) = w.session_mut(id).and_then(|s| s.character.as_mut())
                && p.firstname == "Ermias"
            {
                p.maxhp = 20_000;
                p.hp = 20_000;
            }
        }
    })
    .await;

    // Down the stairs to the camp, and it stays there, looking around.
    until(&sm, &mut rx, "camping", |r| {
        *r == Report::Routine(RoutinePart::Camping)
    })
    .await;
    assert_eq!(card_room(&server), Some(2150));
    let card = server.party().into_iter().find(|c| c.name == "Ermias");
    assert_eq!(card.and_then(|c| c.routine).as_deref(), Some("camping"));
    until(&sm, &mut rx, "a look", |r| {
        *r == Report::Sent(String::new())
    })
    .await;
    // A walk by hand waits for the routine to be paused.
    assert!(
        server
            .order(
                "Ermias",
                Order::Goto {
                    room: "1/2141".into(),
                    avoid: Vec::new(),
                }
            )
            .is_ok()
    );
    let refused = until(&sm, &mut rx, "the goto refused", |r| {
        matches!(r, Report::Walk(WalkState::Failed(_)))
    })
    .await;
    assert!(format!("{refused:?}").contains("@stop"), "{refused:?}");

    // Hurt: up to Narrow Road to rest.
    set_hp(&sm, 0.3).await;
    until(&sm, &mut rx, "going to rest", |r| {
        *r == Report::Routine(RoutinePart::ToRest)
    })
    .await;
    until(&sm, &mut rx, "resting", |r| {
        *r == Report::Routine(RoutinePart::Resting)
    })
    .await;
    assert_eq!(card_room(&server), Some(2146));
    until(&sm, &mut rx, "the rest command", |r| {
        *r == Report::Sent("rest".into())
    })
    .await;
    // Rested: back down to the camp.
    set_hp(&sm, 1.0).await;
    until(&sm, &mut rx, "going back", |r| {
        *r == Report::Routine(RoutinePart::ToCamp)
    })
    .await;
    until(&sm, &mut rx, "camping again", |r| {
        *r == Report::Routine(RoutinePart::Camping)
    })
    .await;
    assert_eq!(card_room(&server), Some(2150));

    // Paused from the page, and on again.
    server.order("Ermias", Order::Pause).unwrap();
    until(&sm, &mut rx, "paused", |r| {
        *r == Report::Routine(RoutinePart::Paused)
    })
    .await;
    server.order("Ermias", Order::Resume).unwrap();
    until(&sm, &mut rx, "camping after the pause", |r| {
        *r == Report::Routine(RoutinePart::Camping)
    })
    .await;

    server.shutdown().await;
    sm.stop().await.unwrap();
}
