//! Walking through the server (#52) in the stock world's Newhaven: a bot told to go to a
//! room plans its route over the imported game data and walks it, through the arena's
//! door as well. Each step is a test of its own, on a game of its own, with Ermias
//! starting where the step starts.

use std::fs;
use std::sync::Arc;

use mp_engine::{Report, SessionState, WalkState};
use mp_silvermere::{StockChar, StockSilvermere, WAIT};
use mudparty_server::{BotEvent, Order, Server, compile_places};
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

const ARENA: u32 = 2150;
const WEAPONS_SHOP: u32 = 2141;
const SMALL_CAVERN: u32 = 2156;

/// Waits for a report `want` picks; failing, says what it saw last.
async fn until(rx: &mut broadcast::Receiver<BotEvent>, want: impl Fn(&Report) -> bool) -> Report {
    let deadline = Instant::now() + WAIT;
    let mut seen: Vec<String> = Vec::new();
    loop {
        let wait = deadline.saturating_duration_since(Instant::now());
        match timeout(wait, rx.recv()).await {
            Ok(Ok(BotEvent::Session { report, .. })) if want(&report) => return report,
            Ok(Ok(ev)) => seen.push(format!("{ev:?}")),
            other => {
                let tail = &seen[seen.len().saturating_sub(40)..];
                panic!(
                    "never saw the report ({other:?}); last seen:\n{}",
                    tail.join("\n")
                )
            }
        }
    }
}

/// A game with Ermias in Newhaven's room `room`, a server on it with the compiled
/// Newhaven room file, and Ermias logged in and in the game.
struct Game {
    sm: StockSilvermere,
    server: Arc<Server>,
    rx: broadcast::Receiver<BotEvent>,
    _dir: tempfile::TempDir,
}

impl Game {
    async fn start(room: u32) -> Self {
        let sm = StockSilvermere::start(
            &[StockChar {
                room,
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
        // The compiled room file, so rooms can be named.
        compile_places(dir.path(), Some("newhaven")).unwrap();
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
        Self {
            sm,
            server,
            rx,
            _dir: dir,
        }
    }

    /// Orders a walk that keeps out of `avoid`, and waits for its end.
    async fn walk_avoiding(&mut self, room: &str, avoid: &[&str]) -> WalkState {
        let r = self.server.order(
            "Ermias",
            Order::Goto {
                room: room.into(),
                avoid: avoid.iter().map(|a| (*a).to_owned()).collect(),
            },
        );
        assert!(r.is_ok(), "{r:?}");
        let Report::Walk(end) = until(&mut self.rx, |r| {
            matches!(r, Report::Walk(WalkState::Arrived | WalkState::Failed(_)))
        })
        .await
        else {
            unreachable!()
        };
        end
    }

    async fn walk(&mut self, room: &str) -> WalkState {
        self.walk_avoiding(room, &[]).await
    }

    /// The room the party card shows Ermias in, and its name.
    fn card_room(&self) -> (Option<u32>, Option<String>) {
        let card = self
            .server
            .party()
            .into_iter()
            .find(|c| c.name == "Ermias")
            .unwrap();
        (
            card.room.as_ref().map(|r| r.room),
            card.room.and_then(|r| r.name),
        )
    }

    async fn stop(self) {
        self.server.shutdown().await;
        self.sm.stop().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn step1_up_the_stairs_to_the_weapons_shop() {
    let mut g = Game::start(ARENA).await;
    assert_eq!(g.walk("1/2141").await, WalkState::Arrived);
    assert_eq!(
        g.card_room(),
        (Some(WEAPONS_SHOP), Some("Newhaven, Weapons Shop".into()))
    );
    g.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn step2_from_the_weapons_shop_down_to_the_arena() {
    let mut g = Game::start(WEAPONS_SHOP).await;
    assert_eq!(g.walk("1/2150").await, WalkState::Arrived);
    assert_eq!(g.card_room().0, Some(ARENA));
    g.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn step3_north_through_the_arena_door_to_the_cavern() {
    let mut g = Game::start(ARENA).await;
    assert_eq!(g.walk("1/2156").await, WalkState::Arrived);
    assert_eq!(g.card_room().0, Some(SMALL_CAVERN));
    g.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn step4_from_the_cavern_back_to_the_arena() {
    let mut g = Game::start(SMALL_CAVERN).await;
    assert_eq!(g.walk("1/2150").await, WalkState::Arrived);
    assert_eq!(g.card_room().0, Some(ARENA));
    g.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn step5_to_the_weapons_shop_by_its_compiled_name() {
    // By the compiled room file's names (#63).
    let mut g = Game::start(ARENA).await;
    assert_eq!(g.walk("weapons-shop").await, WalkState::Arrived);
    assert_eq!(g.card_room().0, Some(WEAPONS_SHOP));
    g.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn step6_back_to_the_arena_by_its_compiled_name() {
    let mut g = Game::start(WEAPONS_SHOP).await;
    assert_eq!(g.walk("arena-2150").await, WalkState::Arrived);
    assert_eq!(g.card_room().0, Some(ARENA));
    g.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn step7_an_avoided_room_with_no_way_round_fails_saying_why() {
    // Round what is avoided (#64): there's no way to the shops but along the Narrow Path,
    // so the walk says what's in the way, and the bot stays put.
    let mut g = Game::start(ARENA).await;
    match g.walk_avoiding("weapons-shop", &["narrow-path"]).await {
        WalkState::Failed(why) => {
            assert!(why.contains("avoided"), "{why}");
            assert!(why.contains("Narrow Path"), "{why}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(g.card_room().0, Some(ARENA));
    g.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn step8_orders_it_cant_carry_out_are_answered() {
    let g = Game::start(ARENA).await;
    for room in ["the moon", "arena"] {
        let r = g.server.order(
            "Ermias",
            Order::Goto {
                room: room.into(),
                avoid: Vec::new(),
            },
        );
        assert!(r.is_err(), "{room}: {r:?}");
    }
    g.stop().await;
}
