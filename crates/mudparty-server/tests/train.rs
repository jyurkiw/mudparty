//! Training through the server (#56) in the stock world's Newhaven: a Warrior with the
//! experience for several levels runs a routine whose first rule trains it up to level 3.
//! It finds the Adventurer's Guild from the compiled room files, trains twice, and goes
//! back to the arena to camp, and doesn't train past level 3 though it has the experience.

use std::fs;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mp_engine::{Report, RoutinePart, SessionState};
use mp_silvermere::{StockChar, StockSilvermere, WAIT};
use mudparty_server::{BotEvent, Server, compile_places};
use tokio::sync::broadcast;
use tokio::time::{Instant, timeout};

const ARENA: &str = "schema: 1
name: newhaven-arena
places: newhaven
do:
  - train: { up_to: 3 }
  - camp: { at: arena, look_every: 1s }
";

const LOCAL: &str = "schema: 1
area: newhaven
aliases:
  arena: arena-2150
";

const ERMIAS: &str = "schema: 1
name: Ermias
class: Warrior
login:
  account: Ermias
automation:
  combat: false
routine: newhaven-arena
";

/// Waits for a report `want` picks, running the game's clock as it waits; every report
/// seen on the way is kept in `seen`.
async fn until(
    sm: &StockSilvermere,
    rx: &mut broadcast::Receiver<BotEvent>,
    seen: &mut Vec<Report>,
    what: &str,
    want: impl Fn(&Report) -> bool,
) {
    let deadline = Instant::now() + WAIT * 2;
    loop {
        assert!(Instant::now() < deadline, "never saw {what}; saw {seen:?}");
        while let Ok(Ok(ev)) = timeout(Duration::from_millis(100), rx.recv()).await {
            if let BotEvent::Session { report, .. } = ev {
                let done = want(&report);
                seen.push(report);
                if done {
                    return;
                }
            }
        }
        sm.tick(2).await;
    }
}

/// Ermias's level, as the game has it.
async fn level(sm: &StockSilvermere) -> u16 {
    let found: Arc<Mutex<u16>> = Arc::default();
    let out = Arc::clone(&found);
    sm.on_world(move |w| {
        for id in w.session_ids() {
            if let Some(p) = w.session(id).and_then(|s| s.player())
                && p.firstname == "Ermias"
            {
                *out.lock().unwrap() = p.level;
            }
        }
    })
    .await;
    *found.lock().unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bot_trains_up_to_its_level_and_no_further() {
    let sm = StockSilvermere::start(
        &[StockChar {
            room: 2150,
            // Enough for many levels, and the fees (5 silver a level).
            exp: 500_000_000,
            silver: 100,
            ..StockChar::new("Ermias", "Warrior")
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
    fs::create_dir_all(root.join("routines")).unwrap();
    fs::write(root.join("routines/newhaven-arena.yaml"), ARENA).unwrap();
    compile_places(root, Some("newhaven")).unwrap();
    fs::write(root.join("places/newhaven.local.yaml"), LOCAL).unwrap();
    fs::write(
        root.join("secrets.yaml"),
        "accounts:\n  Ermias: \"ermias1\"\n",
    )
    .unwrap();
    let server = Server::start(root).await.unwrap();
    let mut rx = server.subscribe();
    server.start_bot("Ermias").unwrap();
    let mut seen = Vec::new();
    until(&sm, &mut rx, &mut seen, "in the game", |r| {
        *r == Report::State(SessionState::InGame)
    })
    .await;
    // The arena's monsters come while the clock runs; enough HP that they don't matter.
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

    until(&sm, &mut rx, &mut seen, "trained to level 3", |r| {
        *r == Report::RoutineNote("trained to level 3".into())
    })
    .await;
    assert!(
        seen.contains(&Report::RoutineNote("trained to level 2".into())),
        "{seen:?}"
    );
    until(&sm, &mut rx, &mut seen, "camping in the arena", |r| {
        *r == Report::Routine(RoutinePart::Camping)
    })
    .await;
    let card = server
        .party()
        .into_iter()
        .find(|c| c.name == "Ermias")
        .unwrap();
    assert_eq!(card.room.map(|r| r.room), Some(2150));
    assert_eq!(level(&sm).await, 3);

    // It has the experience for level 4, but level 3 is as far as the routine goes.
    for _ in 0..20 {
        while let Ok(Ok(ev)) = timeout(Duration::from_millis(100), rx.recv()).await {
            if let BotEvent::Session { report, .. } = ev {
                seen.push(report);
            }
        }
        sm.tick(2).await;
    }
    let trains = seen
        .iter()
        .filter(|r| matches!(r, Report::Sent(l) if l == "train"))
        .count();
    assert_eq!(trains, 2, "{seen:?}");
    assert_eq!(level(&sm).await, 3);

    server.shutdown().await;
    sm.stop().await.unwrap();
}
