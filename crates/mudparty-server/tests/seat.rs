//! What a MudPlay seat adds to plain pass-through (design §2, §2.1, §10.4 scenario 8;
//! MVP 9b): `#` lines held at the seat, and injection that waits for the user's half-typed
//! line.

use std::fs;
use std::path::Path;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use mp_engine::{Report, SessionState};
use mp_parse::Item;
use mp_silvermere::{RawClient, StockChar, StockSilvermere, WAIT};
use mudparty_server::{BotEvent, Order, Server, proxy};
use serde_json::Value;
use tokio::sync::broadcast;
use tokio::time::{Instant, timeout};

const RAIJIN: &str = "schema: 1\nname: Raijin\nclass: Warrior\n";
const ERMIAS: &str = "schema: 1\nname: Ermias\nclass: Mage\nlogin:\n  account: Ermias\n";

async fn until(rx: &mut broadcast::Receiver<BotEvent>, who: &str, want: impl Fn(&Report) -> bool) {
    let deadline = Instant::now() + WAIT;
    loop {
        let wait = deadline.saturating_duration_since(Instant::now());
        match timeout(wait, rx.recv()).await {
            Ok(Ok(BotEvent::Session { name, report })) if name == who && want(&report) => return,
            Ok(Ok(_)) => {}
            _ => panic!("never saw the report from {who}"),
        }
    }
}

/// Everything the session wrote to the game, in order: (source, bytes).
fn outgoing(root: &Path, name: &str) -> Vec<(String, String)> {
    let text = fs::read_to_string(root.join("logs").join(name).join("wire.jsonl")).unwrap();
    text.lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|r| r["kind"] == "wire" && r["dir"] == "out" && r.get("b64").is_some())
        .map(|r| {
            let b = STANDARD.decode(r["b64"].as_str().unwrap()).unwrap();
            (
                r["src"].as_str().unwrap().to_owned(),
                String::from_utf8_lossy(&b).into_owned(),
            )
        })
        .collect()
}

fn text_of(seen: &[Item]) -> String {
    seen.iter()
        .filter_map(|i| match i {
            Item::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test(flavor = "multi_thread")]
async fn hash_lines_stay_at_the_seat_and_injection_waits_for_the_user() {
    let sm = StockSilvermere::start(
        &[
            StockChar::new("Raijin", "Warrior"),
            StockChar::new("Ermias", "Mage"),
        ],
        false,
    )
    .await
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let port = sm.telnet_addr().port();
    fs::write(
        dir.path().join("mudparty.toml"),
        format!(
            "schema = 1\nmain_character = \"Raijin\"\nlogoff_timeout = \"5s\"\n[game]\nhost = \"127.0.0.1\"\nport = {port}\n[gamedata]\nsilvermere_db = \"none.db\"\n"
        ),
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("characters")).unwrap();
    fs::write(dir.path().join("characters/Raijin.yaml"), RAIJIN).unwrap();
    fs::write(dir.path().join("characters/Ermias.yaml"), ERMIAS).unwrap();
    fs::write(
        dir.path().join("secrets.yaml"),
        "accounts:\n  Ermias: \"ermias1\"\n",
    )
    .unwrap();
    let server = Server::start(dir.path()).await.unwrap();
    let addr = proxy::serve(server.clone(), "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let mut rx = server.subscribe();
    server.start_bot("Ermias").unwrap();
    until(&mut rx, "Ermias", |r| {
        *r == Report::State(SessionState::InGame)
    })
    .await;
    let mut mudplay = RawClient::log_in(addr, "Raijin", "raijin1").await.unwrap();
    until(&mut rx, "Raijin", |r| {
        *r == Report::State(SessionState::InGame)
    })
    .await;

    // `#who` is answered at the seat, with the game's prompt drawn again after it.
    mudplay.seen.clear();
    mudplay.send("#who").await.unwrap();
    mudplay
        .read_until(|i| matches!(i, Item::Prompt(_)))
        .await
        .unwrap();
    let said = text_of(&mudplay.seen);
    assert!(said.contains("Raijin (MudPlay)"), "{said}");
    assert!(said.contains("Ermias (bot)"), "{said}");
    // `#help` lists the commands.
    mudplay.seen.clear();
    mudplay.send("#help").await.unwrap();
    mudplay
        .read_until(|i| matches!(i, Item::Prompt(_)))
        .await
        .unwrap();
    let said = text_of(&mudplay.seen);
    assert!(said.contains("#who"), "{said}");
    // A `#` line mudparty doesn't know goes to the game whole.
    mudplay.send("#nosuch thing").await.unwrap();
    mudplay
        .read_until(|i| matches!(i, Item::Prompt(_)))
        .await
        .unwrap();

    // Injection: the user is half-way through typing `look`, so an injected line waits.
    mudplay.type_bytes(b"lo").await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        server.order(
            "Raijin",
            Order::Command {
                text: "say injected".into()
            }
        ),
        Ok("queued for Raijin".into())
    );
    until(&mut rx, "Raijin", |r| {
        *r == Report::Queued(vec!["say injected".into()])
    })
    .await;
    // Nothing goes until the line is finished.
    let early = timeout(Duration::from_millis(700), async {
        until(&mut rx, "Raijin", |r| {
            *r == Report::Sent("say injected".into())
        })
        .await;
    })
    .await;
    assert!(early.is_err(), "injected into a half-typed line");
    let card = server
        .party()
        .into_iter()
        .find(|c| c.name == "Raijin")
        .unwrap();
    assert_eq!(card.queued, ["say injected"]);
    // The user finishes the line; once its prompt is back, the injected line goes.
    mudplay.type_bytes(b"ok\r\n").await.unwrap();
    until(&mut rx, "Raijin", |r| {
        *r == Report::Sent("say injected".into())
    })
    .await;
    until(&mut rx, "Raijin", |r| *r == Report::Queued(vec![])).await;
    mudplay
        .read_until(|i| matches!(i, Item::Text(t) if t.contains("injected")))
        .await
        .unwrap();

    server.shutdown().await;
    let out = outgoing(dir.path(), "Raijin");
    let user: String = out
        .iter()
        .filter(|(s, _)| s == "user")
        .map(|(_, b)| b.as_str())
        .collect();
    assert!(!user.contains("#who"), "#who reached the game: {user:?}");
    assert!(!user.contains("#help"), "#help reached the game: {user:?}");
    assert!(user.contains("#nosuch thing\r\n"), "{user:?}");
    // The user's line went first, then the injection.
    let look = out
        .iter()
        .position(|(s, b)| s == "user" && b.contains("ok\r\n"));
    let inject = out
        .iter()
        .position(|(s, b)| s == "inject" && b == "say injected\r\n");
    assert!(
        look.is_some() && inject.is_some() && look < inject,
        "{out:?}"
    );
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn only_a_seated_character_takes_injection() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("mudparty.toml"),
        "schema = 1\nmain_character = \"Raijin\"\n[game]\nhost = \"127.0.0.1\"\nport = 1\n[gamedata]\nsilvermere_db = \"none.db\"\n",
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("characters")).unwrap();
    fs::write(dir.path().join("characters/Raijin.yaml"), RAIJIN).unwrap();
    let server = Server::start(dir.path()).await.unwrap();
    let r = server.inject("Raijin", "say hi");
    assert!(r.is_err(), "{r:?}");
    server.shutdown().await;
}
