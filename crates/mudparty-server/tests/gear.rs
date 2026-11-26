//! Gear through the server (#55) in the stock world's Newhaven: a new character runs a
//! routine whose first rule keeps its gear. It finds nothing in its inventory, walks to the
//! shops the compiled room files say sell a club and padded armour, buys and puts each on,
//! looks again, and goes back to the arena to camp.

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
  - gear
  - camp: { at: arena, look_every: 1s }
";

const LOCAL: &str = "schema: 1
area: newhaven
aliases:
  arena: arena-2150
";

const ERMIAS: &str = "schema: 1
name: Ermias
login:
  account: Ermias
automation:
  combat: false
gear:
  - { item: club, use: wield }
  - { item: padded vest, use: wear }
  - { item: padded pants, use: wear }
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

#[tokio::test(flavor = "multi_thread")]
async fn a_new_character_buys_and_puts_on_its_gear_and_goes_to_camp() {
    let sm = StockSilvermere::start(
        &[StockChar {
            room: 2150,
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

    until(&sm, &mut rx, &mut seen, "camping in the arena", |r| {
        *r == Report::Routine(RoutinePart::Camping)
    })
    .await;
    let sent: Vec<&str> = seen
        .iter()
        .filter_map(|r| match r {
            Report::Sent(l) => Some(l.as_str()),
            _ => None,
        })
        .collect();
    for line in [
        "i",
        "buy club",
        "wield club",
        "buy padded vest",
        "wear padded vest",
        "buy padded pants",
        "wear padded pants",
    ] {
        assert!(sent.contains(&line), "never sent {line:?}: {sent:?}");
    }
    assert!(
        !seen.iter().any(|r| matches!(r, Report::RoutineNote(_))),
        "{seen:?}"
    );
    let card = server
        .party()
        .into_iter()
        .find(|c| c.name == "Ermias")
        .unwrap();
    assert_eq!(card.room.map(|r| r.room), Some(2150));

    // The game agrees: the club in hand, the vest and pants worn.
    let held: Arc<Mutex<(i32, Vec<i32>)>> = Arc::default();
    let out = Arc::clone(&held);
    sm.on_world(move |w| {
        for id in w.session_ids() {
            if let Some(p) = w.session(id).and_then(|s| s.player())
                && p.firstname == "Ermias"
            {
                *out.lock().unwrap() = (p.weapon.0, p.worn.iter().map(|i| i.0).collect());
            }
        }
    })
    .await;
    let (weapon, worn) = held.lock().unwrap().clone();
    assert_eq!(weapon, 90, "the club");
    assert!(worn.contains(&332), "the vest: {worn:?}");
    assert!(worn.contains(&333), "the pants: {worn:?}");

    server.shutdown().await;
    sm.stop().await.unwrap();
}
