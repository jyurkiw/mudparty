//! A party bot assisting its leader through a run of monsters (#105), on the stock world.
//! The arena's monsters creep in one by one, each in its own words, long after the bot last
//! saw the room drawn; a bot that follows its leader never looks round. It must still join
//! the leader against each of them, not only the ones it saw as it came in.

use std::fs;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mp_engine::{Report, SessionState};
use mp_silvermere::{RawClient, Stats, StockChar, StockSilvermere, WAIT};
use mudparty_server::{BotEvent, Server};
use tokio::time::{Instant, timeout};

const ERMIAS: &str = "schema: 1
name: Ermias
class: Warrior
login:
  account: Ermias
party:
  leader: Raijin
combat:
  targeting: [kill_order, assist_leader]
  melee: a
";

const ARENA: u32 = 2150;

fn strong(name: &str) -> StockChar {
    StockChar {
        room: ARENA,
        stats: Some(Stats {
            strength: 90,
            health: 90,
            agility: 60,
        }),
        ..StockChar::new(name, "Warrior")
    }
}

/// The monsters in the arena now, by the game's names for them, with their ids.
async fn arena_monsters(sm: &StockSilvermere) -> Vec<(i32, String)> {
    let out: Arc<Mutex<Vec<(i32, String)>>> = Arc::default();
    let o = Arc::clone(&out);
    sm.on_world(move |w| {
        let names = w
            .room_state
            .iter()
            .find(|(k, _)| k.map.0 == 1 && k.room.0 == ARENA)
            .map(|(_, st)| {
                st.monsters
                    .iter()
                    .filter(|m| m.0 != 0)
                    .filter_map(|&m| w.active_monsters.peek(m).map(|a| (m.0, a.name.clone())))
                    .collect()
            })
            .unwrap_or_default();
        *o.lock().unwrap() = names;
    })
    .await;
    out.lock().unwrap().clone()
}

/// What the game says each fighter is doing: its name, whether it's in auto-combat, and the
/// target of its last fight.
async fn game_fights(sm: &StockSilvermere) -> Vec<(String, bool, String)> {
    let out: Arc<Mutex<Vec<(String, bool, String)>>> = Arc::default();
    let o = Arc::clone(&out);
    sm.on_world(move |w| {
        let fights = w
            .session_ids()
            .into_iter()
            .filter_map(|id| {
                Some((
                    w.session(id)?.character.as_ref()?.firstname.clone(),
                    w.autocombat.queued(id),
                    format!("{:?}", w.autocombat.entry(id).target),
                ))
            })
            .collect();
        *o.lock().unwrap() = fights;
    })
    .await;
    out.lock().unwrap().clone()
}

/// Both fighters' HP topped up, so a long fight never wounds them.
async fn heal(sm: &StockSilvermere) {
    sm.on_world(|w| {
        for id in w.session_ids() {
            if let Some(p) = w.session_mut(id).and_then(|s| s.character.as_mut()) {
                p.hp = p.maxhp;
            }
        }
    })
    .await;
}

/// Game cycles run each time round the test's loop. A combat round is several, and the loop
/// waits on the bot only briefly, so the fight moves on while the bot's lines are on their way.
const TICKS: u32 = 8;

/// The run, timed, for a failure message. A line the game repeats (a monster's swing each
/// round) is kept only the first time in a while, so the log shows what happened.
#[derive(Default)]
struct Log(Vec<String>);

impl Log {
    fn push(&mut self, at: Duration, what: String) {
        self.0.push(format!("{:7.2} {what}", at.as_secs_f32()));
    }

    fn line(&mut self, at: Duration, kind: &str, text: &str) {
        let what = format!("{kind} {text}");
        let recent = &self.0[self.0.len().saturating_sub(40)..];
        if !recent.iter().any(|l| l.ends_with(&what)) {
            self.push(at, what);
        }
    }
}

impl std::fmt::Display for Log {
    /// The first and last of it, when it's long.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = self.0.len();
        if n <= 300 {
            return f.write_str(&self.0.join("\n"));
        }
        write!(
            f,
            "{}\n... {} lines ...\n{}",
            self.0[..150].join("\n"),
            n - 300,
            self.0[n - 150..].join("\n")
        )
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_party_bot_assists_against_monsters_that_came_in_later() {
    let sm = StockSilvermere::start(&[strong("Raijin"), strong("Ermias")], true)
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

    let mut raijin = RawClient::log_in(sm.telnet_addr(), "Raijin", "raijin1")
        .await
        .unwrap();
    let server = Server::start(root).await.unwrap();
    let mut rx = server.subscribe();
    server.start_bot("Ermias").unwrap();

    // The leader attacks whatever is in the arena, as each one comes; the bot's fights are
    // counted by the targets it takes up: each report naming a target after one that named
    // another or none, so a run of same-named monsters counts each.
    let start = Instant::now();
    let mut in_game = false;
    let mut fought: Vec<String> = Vec::new();
    let mut fighting: Option<String> = None;
    let mut led: Vec<String> = Vec::new();
    // When the leader last attacked, so it isn't sent again before the game takes it up.
    let mut sent_at: Option<Instant> = None;
    // For the failure message: the bot's reports, what it sent and the game's lines to it,
    // timed.
    let mut log = Log::default();
    let deadline = start + WAIT * 4;
    while fought.len() < 4 {
        if Instant::now() >= deadline {
            let here = arena_monsters(&sm).await;
            let game = game_fights(&sm).await;
            panic!(
                "the bot took up {fought:?}; the leader attacked {} monsters, the first {:?}\n\
                 the arena now: {here:?}\nthe game's fights: {game:?}\n\
                 the bot's reports and lines:\n{log}",
                led.len(),
                &led[..led.len().min(10)]
            );
        }
        while let Ok(Ok(ev)) = timeout(Duration::from_millis(50), rx.recv()).await {
            let BotEvent::Session { report, .. } = ev else {
                continue;
            };
            let at = start.elapsed();
            match report {
                Report::State(SessionState::InGame) => in_game = true,
                Report::Fight { target, tier } => {
                    log.push(at, format!("FIGHT {target:?} {tier:?}"));
                    if let Some(t) = &target
                        && fighting.as_ref() != Some(t)
                    {
                        fought.push(t.clone());
                    }
                    fighting = target;
                }
                Report::Sent(line) => log.push(at, format!("SENT {line}")),
                Report::History(lines) => {
                    for l in lines {
                        log.line(at, l.kind, &l.text);
                    }
                }
                _ => {}
            }
        }
        // The leader's own output, drained so it never backs up.
        let _ = timeout(Duration::from_millis(20), raijin.read_until(|_| false)).await;
        if in_game {
            // The leader takes on the next monster whenever the game has it in no fight: a
            // monster's number is used again once it dies, so it can't be told by that.
            let idle = !game_fights(&sm)
                .await
                .iter()
                .any(|(name, fighting, _)| name == "Raijin" && *fighting);
            let here = arena_monsters(&sm).await;
            if idle
                && sent_at.is_none_or(|at| at.elapsed() >= Duration::from_millis(300))
                && let Some(next) = here.first()
            {
                raijin.send(&format!("a {}", next.1)).await.unwrap();
                log.push(
                    start.elapsed(),
                    format!("LEADER a {} (#{})", next.1, next.0),
                );
                led.push(next.1.clone());
                sent_at = Some(Instant::now());
            }
            heal(&sm).await;
        }
        // The game runs well ahead of the bot, as on a slow runner, so the bot's attack
        // often reaches it after the leader's kill (#118).
        sm.tick(TICKS).await;
    }
    // Most of them came in after the bot's last look at the room, yet it fought each.
    assert!(fought.len() >= 4, "{fought:?}");

    server.shutdown().await;
    sm.stop().await.unwrap();
}
