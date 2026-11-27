//! A long session of walks (#78): one game, the real clock, and one walk after another
//! through Newhaven's arena, as a routine's would be. The old walk test ran its walks back
//! to back like this and stalled now and then, waiting for a walk that never ended.

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

    /// Sets Ermias's HP (and its maximum) in the game.
    async fn set_hp(&self, hp: i16, max_hp: i16) {
        self.sm
            .on_world(move |w| {
                for id in w.session_ids() {
                    if let Some(p) = w.session_mut(id).and_then(|s| s.character.as_mut())
                        && p.firstname == "Ermias"
                    {
                        p.maxhp = max_hp;
                        p.hp = hp;
                    }
                }
            })
            .await;
    }

    async fn stop(self) {
        self.server.shutdown().await;
        self.sm.stop().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_long_session_of_walks_never_stalls() {
    let mut g = Game::start(ARENA).await;
    for round in 0..4 {
        for (to, room) in [
            ("1/2141", WEAPONS_SHOP),
            ("1/2150", ARENA),
            ("1/2156", SMALL_CAVERN),
            ("1/2150", ARENA),
        ] {
            // The cavern is the cave bear's lair, and fills as the bot walks in; with combat
            // off it never fights back. Enough HP that the bear can't wound it, so this is
            // about walking. silvermere works the maximum out again now and then, so it's
            // set before every walk.
            g.set_hp(20_000, 20_000).await;
            let began = Instant::now();
            let end = g.walk(to).await;
            assert_eq!(end, WalkState::Arrived, "round {round}, to {to}");
            assert_eq!(g.card_room().0, Some(room), "round {round}, to {to}");
            eprintln!("round {round}: {to} in {:?}", began.elapsed());
        }
    }
    g.stop().await;
}

// What stalled the old walk test: in the cavern with the bear, combat off, Ermias was
// mortally wounded, and every step out was refused. The walk ends, saying why.
#[tokio::test(flavor = "multi_thread")]
async fn a_mortally_wounded_bot_s_walk_ends_saying_why() {
    let mut g = Game::start(SMALL_CAVERN).await;
    g.set_hp(-5, 30).await;
    match g.walk("1/2150").await {
        WalkState::Failed(why) => assert!(why.contains("mortally wounded"), "{why}"),
        other => panic!("{other:?}"),
    }
    g.stop().await;
}
