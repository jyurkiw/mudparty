//! The 1.0 goal (#57): one bot runs a new character through Newhaven's arena loop by
//! itself, from the hand-written files in `examples/newhaven`, on the stock world. It logs
//! in by the steps its file writes out, buys and puts on its gear, trains to level 3 at
//! the Adventurer's Guild and no further, kills the cave bear in the Small Cavern, fights
//! in the arena, and rests on Narrow Road when hurt before going back.
//!
//! `NEWHAVEN_TRACE=1` prints the bot's reports as they come (HP lines left out) and keeps
//! the data root, logs and all, saying where it is.

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mp_engine::{Report, RoutinePart, SessionState};
use mp_silvermere::{Stats, StockChar, StockSilvermere, WAIT, stock_template};
use mudparty_server::{BotEvent, Server, check, compile_places};
use tokio::sync::broadcast;
use tokio::time::{Instant, timeout};

/// The example files, as a user would copy them into a data root.
const EXAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/newhaven");

fn tracing() -> bool {
    std::env::var_os("NEWHAVEN_TRACE").is_some()
}

/// A data root for the game at `port` on the world in `db`: the example's files copied in,
/// the room files compiled, and Ermias's password.
fn data_root(port: u16, db: &Path) -> tempfile::TempDir {
    let mut dir = tempfile::tempdir().unwrap();
    if tracing() {
        dir.disable_cleanup(true);
        eprintln!("data root: {}", dir.path().display());
    }
    let root = dir.path();
    let db = db.display().to_string().replace('\\', "/");
    fs::write(
        root.join("mudparty.toml"),
        format!(
            "schema = 1\nlogoff_timeout = \"5s\"\n[game]\nhost = \"127.0.0.1\"\nport = {port}\n[gamedata]\nsilvermere_db = \"{db}\"\n"
        ),
    )
    .unwrap();
    let example = Path::new(EXAMPLE);
    for sub in ["characters", "routines", "places"] {
        fs::create_dir_all(root.join(sub)).unwrap();
        for f in fs::read_dir(example.join(sub)).unwrap() {
            let f = f.unwrap();
            fs::copy(f.path(), root.join(sub).join(f.file_name())).unwrap();
        }
    }
    compile_places(root, None).unwrap();
    fs::write(
        root.join("secrets.yaml"),
        "accounts:\n  Ermias: \"ermias1\"\n",
    )
    .unwrap();
    dir
}

/// A game, and the server running the example's bot on it.
struct Game {
    sm: StockSilvermere,
    server: Arc<Server>,
    rx: broadcast::Receiver<BotEvent>,
    seen: Vec<Report>,
    _dir: tempfile::TempDir,
}

impl Game {
    async fn start(ch: StockChar) -> Self {
        let sm = StockSilvermere::start(&[ch], true).await.unwrap();
        let dir = data_root(sm.telnet_addr().port(), &sm.db_path());
        let server = Server::start(dir.path()).await.unwrap();
        let rx = server.subscribe();
        server.start_bot("Ermias").unwrap();
        Self {
            sm,
            server,
            rx,
            seen: Vec::new(),
            _dir: dir,
        }
    }

    /// Takes the reports waiting, running the game's clock once.
    async fn step(&mut self) -> Vec<Report> {
        let mut got = Vec::new();
        while let Ok(Ok(ev)) = timeout(Duration::from_millis(100), self.rx.recv()).await {
            if let BotEvent::Session { report, .. } = ev {
                if tracing() && !matches!(report, Report::Vitals(_)) {
                    eprintln!("{report:?}");
                }
                got.push(report);
            }
        }
        self.sm.tick(2).await;
        self.seen.extend(got.iter().cloned());
        got
    }

    /// Waits for a report `want` picks, within `within`.
    async fn until(&mut self, what: &str, within: Duration, want: impl Fn(&Report) -> bool) {
        let deadline = Instant::now() + within;
        loop {
            let tail: Vec<_> = self.seen.iter().rev().take(40).rev().collect();
            assert!(
                Instant::now() < deadline,
                "never saw {what}; last saw {tail:?}"
            );
            if self.step().await.iter().any(&want) {
                return;
            }
        }
    }

    /// Ermias's level and room, as the game has them.
    async fn ermias(&self) -> (u16, u32) {
        let found: Arc<Mutex<(u16, u32)>> = Arc::default();
        let out = Arc::clone(&found);
        self.sm
            .on_world(move |w| {
                for id in w.session_ids() {
                    if let Some(p) = w.session(id).and_then(|s| s.player())
                        && p.firstname == "Ermias"
                    {
                        *out.lock().unwrap() = (p.level, p.room.0);
                    }
                }
            })
            .await;
        *found.lock().unwrap()
    }

    /// Sets Ermias's HP to `of_max` of its maximum.
    async fn set_hp(&self, of_max: f32) {
        self.sm
            .on_world(move |w| {
                for id in w.session_ids() {
                    if let Some(p) = w.session_mut(id).and_then(|s| s.character.as_mut())
                        && p.firstname == "Ermias"
                    {
                        p.hp = (f32::from(p.maxhp) * of_max) as i16;
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
async fn a_bot_runs_newhaven_s_arena_loop_alone() {
    let mut g = Game::start(StockChar {
        // A new character, put in the arena.
        room: 2150,
        // Experience for many levels, as if it had fought for them, and the fees.
        exp: 500_000_000,
        silver: 100,
        // Its creation points in strength, health and agility, as a warrior's would be.
        stats: Some(Stats {
            strength: 90,
            health: 80,
            agility: 50,
        }),
        ..StockChar::new("Ermias", "Warrior")
    })
    .await;
    let stage = WAIT * 4;

    // In by the steps the file writes out, then the shops: a club, padded armour and a
    // torch, bought and put to use.
    g.until("in the game", stage, |r| {
        *r == Report::State(SessionState::InGame)
    })
    .await;
    g.until("the torch", stage, |r| {
        *r == Report::Routine(RoutinePart::Equipping("torch".into()))
    })
    .await;
    for line in [
        "buy club",
        "wield club",
        "buy padded vest",
        "wear padded vest",
        "buy padded pants",
        "wear padded pants",
        "buy padded helm",
        "buy padded gloves",
        "buy padded boots",
    ] {
        assert!(g.seen.contains(&Report::Sent(line.into())), "{line}");
    }
    // Up to level 3 at the guild, as far as the arena lets it.
    g.until("trained to level 3", stage, |r| {
        *r == Report::RoutineNote("trained to level 3".into())
    })
    .await;
    assert!(
        g.seen
            .contains(&Report::RoutineNote("trained to level 2".into()))
    );
    // The cave bear: to its cavern, through the arena's locked door, and killed by the
    // torch's light.
    g.until("the cavern cleared", stage, |r| {
        *r == Report::RoutineNote("cleared Small Cavern".into())
    })
    .await;
    assert!(
        g.seen
            .contains(&Report::Routine(RoutinePart::ToLair("Small Cavern".into())))
    );
    assert!(
        g.seen
            .iter()
            .any(|r| matches!(r, Report::Fight { target: Some(t), .. } if t == "cave bear")),
    );
    assert!(g.seen.contains(&Report::Sent("buy torch".into())));
    assert!(g.seen.contains(&Report::Sent("light torch".into())));
    g.until("back in the arena", stage, |r| {
        *r == Report::Routine(RoutinePart::Camping)
    })
    .await;
    // Hurt below half its HP: up to Narrow Road to rest, and back down. The arena's
    // monsters wear it down slowly, so the test does it at once.
    g.set_hp(0.4).await;
    g.until("going to rest", stage, |r| {
        *r == Report::Routine(RoutinePart::ToRest)
    })
    .await;
    g.until("resting", stage, |r| {
        *r == Report::Routine(RoutinePart::Resting)
    })
    .await;
    assert_eq!(g.ermias().await.1, 2146);
    g.until("camping again", stage, |r| {
        *r == Report::Routine(RoutinePart::Camping)
    })
    .await;
    assert_eq!(g.ermias().await, (3, 2150));
    let card = g
        .server
        .party()
        .into_iter()
        .find(|c| c.name == "Ermias")
        .unwrap();
    assert_eq!(card.routine.as_deref(), Some("camping"));
    // It had the experience for more, but trained only twice.
    let trains = g
        .seen
        .iter()
        .filter(|r| matches!(r, Report::Sent(l) if l == "train"))
        .count();
    assert_eq!(trains, 2);
    g.stop().await;
}

// What `mudparty-server check` says of the example: nothing wrong, for a new character.
#[test]
fn the_example_checks_clean() {
    let dir = data_root(2323, &stock_template());
    let root = dir.path();
    for file in ["characters/Ermias.yaml", "routines/newhaven-arena.yaml"] {
        let c = check(root, &root.join(file), Some(1)).unwrap();
        assert_eq!(c.problems, Vec::<String>::new(), "{file}:\n{}", c.text);
    }
}
