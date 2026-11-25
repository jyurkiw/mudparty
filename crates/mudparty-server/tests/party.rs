//! Party join and resting through the server (design §10.4 scenarios 2 and 6, without the
//! proxy): bots from their YAML ask the leader, join, and follow where it goes; a bot
//! below its rest thresholds tells the leader to wait and rests. The test plays the leader,
//! Raijin, as MudPlay would: it invites whoever asks.

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mp_engine::{PartyState, Report, SessionState};
use mp_parse::{Event, Item};
use mp_silvermere::{ARENA, RawClient, StockChar, StockSilvermere, WAIT};
use mudparty_server::{BotEvent, Server};
use tokio::sync::broadcast;
use tokio::time::{Instant, timeout};

fn bot_yaml(name: &str, extra: &str) -> String {
    format!(
        "schema: 1\nname: {name}\nlogin:\n  account: {name}\nautomation:\n  combat: false\nparty:\n  leader: Raijin\n{extra}"
    )
}

fn data_root(dir: &Path, sm: &StockSilvermere, bots: &[(&str, String)]) {
    let port = sm.telnet_addr().port();
    let db = sm.db_path().display().to_string().replace('\\', "/");
    fs::write(
        dir.join("mudparty.toml"),
        format!(
            "schema = 1\n[game]\nhost = \"127.0.0.1\"\nport = {port}\n[gamedata]\nsilvermere_db = \"{db}\"\n"
        ),
    )
    .unwrap();
    fs::create_dir_all(dir.join("characters")).unwrap();
    let mut secrets = String::from("accounts:\n");
    for (name, yaml) in bots {
        fs::write(dir.join(format!("characters/{name}.yaml")), yaml).unwrap();
        secrets.push_str(&format!("  {name}: \"{}1\"\n", name.to_lowercase()));
    }
    fs::write(dir.join("secrets.yaml"), secrets).unwrap();
}

/// Waits until every bot in `names` has sent a report passing `want`.
async fn all(
    rx: &mut broadcast::Receiver<BotEvent>,
    names: &[&str],
    want: impl Fn(&Report) -> bool,
    what: &str,
) {
    let mut left: Vec<&str> = names.to_vec();
    let deadline = Instant::now() + WAIT;
    while !left.is_empty() {
        let wait = deadline.saturating_duration_since(Instant::now());
        let Ok(Ok(BotEvent::Session { name, report })) = timeout(wait, rx.recv()).await else {
            panic!("{left:?} never {what}");
        };
        if want(&report) {
            left.retain(|n| *n != name);
        }
    }
}

async fn room_of(sm: &StockSilvermere, name: &'static str) -> u32 {
    let out = Arc::new(Mutex::new(0));
    let o = out.clone();
    sm.on_world(move |w| {
        for id in w.session_ids() {
            if let Some(p) = w.session(id).and_then(|s| s.player())
                && p.firstname == name
            {
                *o.lock().unwrap() = p.room.0;
            }
        }
    })
    .await;
    *out.lock().unwrap()
}

/// The leader invites each bot that telepaths `@invite`, until `n` of them follow.
fn invite_askers(mut leader: RawClient, n: usize) -> tokio::task::JoinHandle<RawClient> {
    tokio::spawn(async move {
        let mut following = 0;
        leader
            .answer_until(
                |i| match i {
                    Item::Event(Event::Telepath { who, text }) if text == "@invite" => {
                        Some(format!("invite {who}"))
                    }
                    _ => None,
                },
                |i| {
                    if matches!(i, Item::Event(Event::StartedFollow { .. })) {
                        following += 1;
                    }
                    following == n
                },
            )
            .await
            .expect("both bots ask and follow");
        leader
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn two_bots_join_the_leader_and_follow_it() {
    let sm = StockSilvermere::start(
        &[
            StockChar::new("Raijin", "Warrior"),
            StockChar::new("Ermias", "Mage"),
            StockChar::new("Kaelen", "Warrior"),
        ],
        false,
    )
    .await
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    data_root(
        dir.path(),
        &sm,
        &[
            ("Ermias", bot_yaml("Ermias", "")),
            ("Kaelen", bot_yaml("Kaelen", "")),
        ],
    );
    let leader = RawClient::log_in(sm.telnet_addr(), "Raijin", "raijin1")
        .await
        .unwrap();
    let leader = invite_askers(leader, 2);
    let server = Server::start(dir.path()).await.unwrap();
    let mut rx = server.subscribe();
    server.start_bot("Ermias").unwrap();
    server.start_bot("Kaelen").unwrap();
    all(
        &mut rx,
        &["Ermias", "Kaelen"],
        |r| *r == Report::Party(PartyState::Following),
        "followed",
    )
    .await;
    let mut leader = timeout(WAIT, leader).await.unwrap().unwrap();
    assert_eq!(room_of(&sm, "Ermias").await, ARENA);
    leader.send("n").await.unwrap();
    let deadline = Instant::now() + WAIT;
    while room_of(&sm, "Ermias").await != 205 || room_of(&sm, "Kaelen").await != 205 {
        assert!(Instant::now() < deadline, "the bots never followed north");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    server.shutdown().await;
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bot_rests_by_its_file_and_tells_the_leader() {
    let sm = StockSilvermere::start(
        &[
            StockChar::new("Raijin", "Warrior"),
            StockChar::new("Ermias", "Mage"),
        ],
        true,
    )
    .await
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let rest = "rest:\n  start_when_any: { mp: 5 }\n  stop_when_all: { hp: 1, mp: 12 }\n";
    data_root(dir.path(), &sm, &[("Ermias", bot_yaml("Ermias", rest))]);
    let mut leader = RawClient::log_in(sm.telnet_addr(), "Raijin", "raijin1")
        .await
        .unwrap();
    let server = Server::start(dir.path()).await.unwrap();
    let mut rx = server.subscribe();
    server.start_bot("Ermias").unwrap();
    all(
        &mut rx,
        &["Ermias"],
        |r| *r == Report::State(SessionState::InGame),
        "came in",
    )
    .await;
    sm.on_world(|w| {
        for id in w.session_ids() {
            if let Some(p) = w.session_mut(id).and_then(|s| s.character.as_mut())
                && p.firstname == "Ermias"
            {
                p.maxmana = 40;
                p.mana = 2;
            }
        }
    })
    .await;
    let deadline = Instant::now() + WAIT;
    'rest: loop {
        assert!(Instant::now() < deadline, "the bot never rested");
        sm.tick(5).await;
        while let Ok(Ok(BotEvent::Session { report, .. })) =
            timeout(Duration::from_millis(100), rx.recv()).await
        {
            if report == Report::Sent("rest".into()) {
                break 'rest;
            }
        }
    }
    leader
        .read_until(|i| {
            matches!(i, Item::Event(Event::Telepath { who, text }) if who == "Ermias" && text == "@wait")
        })
        .await
        .expect("the leader is told to wait");
    server.shutdown().await;
    sm.stop().await.unwrap();
}
