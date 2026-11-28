//! A party bot assisting its leader through a run of monsters (#105), on the stock world.
//! The arena's monsters creep in one by one, each in its own words, long after the bot last
//! saw the room drawn; a bot that follows its leader never looks round. It must still join
//! the leader against each of them, not only the ones it saw as it came in.

use std::collections::BTreeSet;
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

/// The monsters in the arena now, by the game's names for them.
async fn arena_monsters(sm: &StockSilvermere) -> Vec<String> {
    let out: Arc<Mutex<Vec<String>>> = Arc::default();
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
                    .filter_map(|&m| w.active_monsters.peek(m).map(|a| a.name.clone()))
                    .collect()
            })
            .unwrap_or_default();
        *o.lock().unwrap() = names;
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
    // counted by the targets it takes up.
    let mut in_game = false;
    let mut fought: Vec<String> = Vec::new();
    let mut led: BTreeSet<String> = BTreeSet::new();
    let mut leading: Option<String> = None;
    let deadline = Instant::now() + WAIT * 4;
    while fought.len() < 4 {
        assert!(
            Instant::now() < deadline,
            "the bot took up {fought:?}; the leader attacked {led:?}"
        );
        while let Ok(Ok(ev)) = timeout(Duration::from_millis(50), rx.recv()).await {
            if let BotEvent::Session { report, .. } = ev {
                match report {
                    Report::State(SessionState::InGame) => in_game = true,
                    Report::Fight {
                        target: Some(t), ..
                    } if fought.last() != Some(&t) => fought.push(t),
                    _ => {}
                }
            }
        }
        // The leader's own output, drained so it never backs up.
        let _ = timeout(Duration::from_millis(20), raijin.read_until(|_| false)).await;
        if in_game {
            let here = arena_monsters(&sm).await;
            if leading.as_ref().is_none_or(|l| !here.contains(l))
                && let Some(next) = here.first()
            {
                raijin.send(&format!("a {next}")).await.unwrap();
                led.insert(next.clone());
                leading = Some(next.clone());
            }
            heal(&sm).await;
        }
        sm.tick(2).await;
    }
    // Most of them came in after the bot's last look at the room, yet it fought each.
    assert!(fought.len() >= 4, "{fought:?}");

    server.shutdown().await;
    sm.stop().await.unwrap();
}
