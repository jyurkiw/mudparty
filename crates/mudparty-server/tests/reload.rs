//! Reloading a character file from its card (#94): the file is checked as `check` does,
//! and a running bot is restarted on it, logging off and on. Anything wrong (a file that
//! doesn't read or has a problem, a bot that couldn't log in again) refuses the reload,
//! and the bot runs on as it was. On the stock world.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use mp_engine::{Report, SessionState};
use mp_silvermere::{StockChar, StockSilvermere, WAIT};
use mudparty_server::{BotEvent, Order, Server};
use serde_json::json;
use tokio::sync::broadcast;
use tokio::time::timeout;

const ERMIAS: &str = "schema: 1
name: Ermias
class: Warrior
login:
  account: Ermias
combat:
  targeting: [kill_order, assist_leader]
  melee: a
";

/// A data root for the stock world `sm`, with Ermias's file and password.
fn data_root(dir: &Path, sm: &StockSilvermere) {
    let db = sm.db_path().display().to_string().replace('\\', "/");
    let port = sm.telnet_addr().port();
    fs::write(
        dir.join("mudparty.toml"),
        format!(
            "schema = 1\nlogoff_timeout = \"10s\"\n[game]\nhost = \"127.0.0.1\"\nport = {port}\n[gamedata]\nsilvermere_db = \"{db}\"\n"
        ),
    )
    .unwrap();
    fs::create_dir_all(dir.join("characters")).unwrap();
    fs::write(dir.join("characters/Ermias.yaml"), ERMIAS).unwrap();
    fs::write(
        dir.join("secrets.yaml"),
        "accounts:\n  Ermias: \"ermias1\"\n",
    )
    .unwrap();
}

async fn server() -> (StockSilvermere, tempfile::TempDir, Arc<Server>) {
    let sm = StockSilvermere::start(&[StockChar::new("Ermias", "Warrior")], false)
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    data_root(dir.path(), &sm);
    let server = Server::start(dir.path()).await.unwrap();
    (sm, dir, server)
}

/// Waits for an event that satisfies `want`.
async fn until(
    rx: &mut broadcast::Receiver<BotEvent>,
    want: impl Fn(&BotEvent) -> bool,
) -> BotEvent {
    loop {
        let e = timeout(WAIT, rx.recv())
            .await
            .expect("the event came in time")
            .expect("events flow");
        if want(&e) {
            return e;
        }
    }
}

fn state(s: SessionState) -> impl Fn(&BotEvent) -> bool {
    move |e| {
        *e == BotEvent::Session {
            name: "Ermias".into(),
            report: Report::State(s.clone()),
        }
    }
}

fn offline(e: &BotEvent) -> bool {
    matches!(
        e,
        BotEvent::Session {
            report: Report::State(SessionState::Offline(_)),
            ..
        }
    )
}

/// No logoff for a while: the bot ran on.
async fn runs_on(rx: &mut broadcast::Receiver<BotEvent>) {
    let quiet = timeout(Duration::from_secs(3), async {
        loop {
            match rx.recv().await {
                Ok(e) if offline(&e) => return e,
                Ok(_) => {}
                Err(_) => std::future::pending::<()>().await,
            }
        }
    })
    .await;
    assert!(quiet.is_err(), "the bot went offline: {quiet:?}");
}

async fn in_game(server: &Arc<Server>) -> broadcast::Receiver<BotEvent> {
    let mut rx = server.subscribe();
    server.start_bot("Ermias").unwrap();
    until(&mut rx, state(SessionState::InGame)).await;
    rx
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reload_restarts_a_running_bot_on_its_file() {
    let (sm, dir, server) = server().await;
    let mut rx = in_game(&server).await;
    fs::write(
        dir.path().join("characters/Ermias.yaml"),
        ERMIAS.replace("[kill_order, assist_leader]", "[assist_leader, kill_order]"),
    )
    .unwrap();
    let said = server.order("Ermias", Order::Reload).unwrap();
    assert!(said.contains("restart"), "{said}");
    until(&mut rx, offline).await;
    until(&mut rx, state(SessionState::InGame)).await;
    assert_eq!(server.running(), ["Ermias"]);
    let log = fs::read_to_string(dir.path().join("logs/server.jsonl")).unwrap();
    assert_eq!(log.matches("\"bot_start\"").count(), 2, "{log}");
    server.shutdown().await;
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_with_a_problem_is_not_reloaded_and_the_bot_runs_on() {
    let (sm, dir, server) = server().await;
    let mut rx = in_game(&server).await;
    fs::write(
        dir.path().join("characters/Ermias.yaml"),
        format!("{ERMIAS}statline: \"[MA=%m]:\"\n"),
    )
    .unwrap();
    let why = server.order("Ermias", Order::Reload).unwrap_err();
    assert!(why.contains("statline"), "{why}");
    runs_on(&mut rx).await;
    assert_eq!(server.running(), ["Ermias"]);
    server.shutdown().await;
    sm.stop().await.unwrap();
}

// A bot that couldn't log in again isn't logged off for it.
#[tokio::test(flavor = "multi_thread")]
async fn a_file_the_bot_couldn_t_start_on_is_not_reloaded() {
    let (sm, dir, server) = server().await;
    let mut rx = in_game(&server).await;
    fs::write(
        dir.path().join("characters/Ermias.yaml"),
        ERMIAS.replace("account: Ermias", "account: Nobody"),
    )
    .unwrap();
    let why = server.order("Ermias", Order::Reload).unwrap_err();
    assert!(why.contains("Nobody"), "{why}");
    runs_on(&mut rx).await;
    assert_eq!(server.running(), ["Ermias"]);
    server.shutdown().await;
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_character_not_running_has_its_file_read_again() {
    let (sm, dir, server) = server().await;
    let said = server.order("Ermias", Order::Reload).unwrap();
    assert!(said.contains("reloaded"), "{said}");
    assert!(server.running().is_empty());
    fs::write(
        dir.path().join("characters/Ermias.yaml"),
        "schema: 1\nname: [\n",
    )
    .unwrap();
    assert!(server.order("Ermias", Order::Reload).is_err());
    fs::remove_file(dir.path().join("characters/Ermias.yaml")).unwrap();
    let why = server.order("Ermias", Order::Reload).unwrap_err();
    assert!(why.contains("no file"), "{why}");
    server.shutdown().await;
    sm.stop().await.unwrap();
}

// The page's feed: the result says which order it answers, so the page can show a failed
// reload on its card.
#[tokio::test(flavor = "multi_thread")]
async fn the_feed_s_result_names_the_reload() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    let (sm, dir, server) = server().await;
    let addr = mudparty_server::web::serve(server.clone(), "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let (mut ws, _) = connect_async(format!("ws://{addr}/ws/ui")).await.unwrap();
    fs::write(
        dir.path().join("characters/Ermias.yaml"),
        format!("{ERMIAS}statline: \"[MA=%m]:\"\n"),
    )
    .unwrap();
    let ask = json!({"v": 1, "type": "order", "name": "Ermias", "order": "reload"});
    ws.send(Message::Text(ask.to_string().into()))
        .await
        .unwrap();
    let r = loop {
        let Some(Ok(Message::Text(t))) = ws.next().await else {
            panic!("the feed closed");
        };
        let v: serde_json::Value = serde_json::from_str(&t).unwrap();
        if v["type"] == "result" {
            break v;
        }
    };
    assert_eq!(r["order"], "reload", "{r}");
    assert_eq!(r["ok"], false, "{r}");
    assert!(r["message"].as_str().unwrap().contains("statline"), "{r}");
    server.shutdown().await;
    sm.stop().await.unwrap();
}
