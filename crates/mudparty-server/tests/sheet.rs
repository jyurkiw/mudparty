//! Character sheets through the server (#10) on the stock world: a bot reads its status
//! display and inventory after logging in, with the values the game holds. The card carries
//! them, and the page's Refresh reads them again: here after the test changes its purse.

use std::fs;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mp_engine::{Report, SessionState, SheetPart};
use mp_silvermere::{Stats, StockChar, StockSilvermere, WAIT};
use mudparty_server::{BotEvent, Order, Server};
use tokio::sync::broadcast;
use tokio::time::{Instant, timeout};

const ERMIAS: &str = "schema: 1
name: Ermias
class: Warrior
login:
  account: Ermias
automation:
  combat: false
";

/// Waits for a report `want` picks, running the game's clock as it waits.
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

/// What the game holds of Ermias: charm, strength, max HP and silver.
async fn held(sm: &StockSilvermere) -> (u16, u16, i16, u32) {
    let found: Arc<Mutex<(u16, u16, i16, u32)>> = Arc::default();
    let out = Arc::clone(&found);
    sm.on_world(move |w| {
        for id in w.session_ids() {
            if let Some(p) = w.session(id).and_then(|s| s.player())
                && p.firstname == "Ermias"
            {
                *out.lock().unwrap() = (p.cha, p.str, p.maxhp, p.coins.silver);
            }
        }
    })
    .await;
    *found.lock().unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bot_reads_its_sheet_and_reads_it_again_on_refresh() {
    let sm = StockSilvermere::start(
        &[StockChar {
            silver: 25,
            stats: Some(Stats {
                strength: 90,
                health: 80,
                agility: 50,
            }),
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

    // After logging in: the status display, as the game holds the character.
    let Report::Sheet(SheetPart::Status(st)) = until(&sm, &mut rx, "the status sheet", |r| {
        matches!(r, Report::Sheet(SheetPart::Status(_)))
    })
    .await
    else {
        unreachable!()
    };
    let (cha, str, max_hp, silver) = held(&sm).await;
    assert_eq!(st.name.split(' ').next(), Some("Ermias"));
    assert_eq!(st.class, "Warrior");
    assert_eq!(st.level, 1);
    assert_eq!(st.strength.value, i64::from(str));
    assert_eq!(st.strength.value, 90);
    assert_eq!(st.charm.value, i64::from(cha));
    assert_eq!(st.max_hp, i64::from(max_hp));
    assert_eq!(st.magic, None, "a Warrior has no mana");

    // And the inventory: its coins, wealth and weight.
    let Report::Sheet(SheetPart::Inventory(inv)) =
        until(&sm, &mut rx, "the inventory sheet", |r| {
            matches!(r, Report::Sheet(SheetPart::Inventory(_)))
        })
        .await
    else {
        unreachable!()
    };
    assert_eq!(inv.coins.silver, u64::from(silver));
    assert_eq!(Some(inv.coins.copper_value() as i64), inv.wealth);
    let enc = inv.encumbrance.clone().expect("the weight line");
    assert!(enc.max > 0, "{enc:?}");
    assert_eq!(inv.coins_room(), Some((enc.max - enc.cur) * 3));

    // The card carries both.
    let card = server
        .party()
        .into_iter()
        .find(|c| c.name == "Ermias")
        .unwrap();
    assert_eq!(card.sheet.status.as_ref(), Some(&*st));
    assert_eq!(card.sheet.inventory.as_ref(), Some(&*inv));
    assert_eq!(card.sheet.coins_value, Some(inv.coins.copper_value()));

    // More silver in its purse; the page's Refresh reads the inventory again.
    sm.on_world(|w| {
        for id in w.session_ids() {
            if let Some(p) = w.session_mut(id).and_then(|s| s.character.as_mut())
                && p.firstname == "Ermias"
            {
                p.coins.silver += 300;
            }
        }
    })
    .await;
    server.order("Ermias", Order::Refresh).unwrap();
    let want = u64::from(silver) + 300;
    until(
        &sm,
        &mut rx,
        "the refreshed inventory",
        |r| matches!(r, Report::Sheet(SheetPart::Inventory(i)) if i.coins.silver == want),
    )
    .await;

    server.shutdown().await;
    sm.stop().await.unwrap();
}
