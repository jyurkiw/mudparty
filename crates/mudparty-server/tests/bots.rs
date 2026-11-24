//! The server shell against a real silvermere (design §3.1, §4.4, §5.1, §10.4 scenario 1):
//! headless bots started and stopped by name, one session per character, config changes
//! pushed to a running bot, and an orderly stop that logs every bot off.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use mp_engine::{Offline, Report, SessionState};
use mp_silvermere::{Player, Silvermere, WAIT};
use mudparty_server::{BotEvent, Server, StartError};
use tokio::sync::broadcast;
use tokio::time::timeout;

fn character(name: &str, statline: &str, autostart: bool) -> String {
    format!(
        "schema: 1\nname: {name}\nautostart: {autostart}\nlogin:\n  account: {name}\nstatline: \"{statline}\"\n"
    )
}

const BOT: &str = "[HP=%h/%H,MA=%m/%M]:%r";

/// A data root for `sm`, with a character file and password for each of `names`.
fn root(sm: &Silvermere, names: &[&str], autostart: bool) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let port = sm.telnet_addr().port();
    fs::write(
        dir.path().join("mudparty.toml"),
        format!(
            "schema = 1\nlogoff_timeout = \"20s\"\n[game]\nhost = \"127.0.0.1\"\nport = {port}\n[gamedata]\nsilvermere_db = \"unused.db\"\n"
        ),
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("characters")).unwrap();
    let mut secrets = String::from("accounts:\n");
    for n in names {
        fs::write(
            dir.path().join(format!("characters/{n}.yaml")),
            character(n, BOT, autostart),
        )
        .unwrap();
        secrets.push_str(&format!("  {n}: \"{}\"\n", n.to_lowercase() + "1"));
    }
    fs::write(dir.path().join("secrets.yaml"), secrets).unwrap();
    dir
}

fn players(names: &[&str]) -> Vec<Player> {
    names
        .iter()
        .map(|n| Player::new(n, &(n.to_lowercase() + "1")))
        .collect()
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

fn state(name: &'static str, s: SessionState) -> impl Fn(&BotEvent) -> bool {
    move |e| {
        *e == BotEvent::Session {
            name: name.into(),
            report: Report::State(s.clone()),
        }
    }
}

fn offline(name: &'static str) -> impl Fn(&BotEvent) -> bool {
    move |e| matches!(e, BotEvent::Session { name: n, report: Report::State(SessionState::Offline(_)) } if n == name)
}

async fn start(dir: &Path) -> Arc<Server> {
    Server::start(dir).await.expect("the server starts")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bot_started_by_name_logs_in() {
    let sm = Silvermere::start(&players(&["Aldric"])).await.unwrap();
    let dir = root(&sm, &["Aldric"], false);
    let server = start(dir.path()).await;
    let mut rx = server.subscribe();
    server.start_bot("Aldric").unwrap();
    until(&mut rx, state("Aldric", SessionState::InGame)).await;
    assert_eq!(server.running(), ["Aldric"]);
    server.shutdown().await;
    assert!(dir.path().join("logs/Aldric/transcript.log").exists());
    let log = fs::read_to_string(dir.path().join("logs/server.jsonl")).unwrap();
    assert!(
        log.contains("\"bot_start\"") && log.contains("\"shutdown\""),
        "{log}"
    );
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_session_for_a_running_character_is_refused() {
    let sm = Silvermere::start(&players(&["Aldric"])).await.unwrap();
    let dir = root(&sm, &["Aldric"], false);
    let server = start(dir.path()).await;
    server.start_bot("Aldric").unwrap();
    assert_eq!(
        server.start_bot("Aldric"),
        Err(StartError::Running("Aldric".into()))
    );
    server.shutdown().await;
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bot_without_a_file_a_login_or_a_password_is_refused() {
    let sm = Silvermere::start(&players(&["Aldric"])).await.unwrap();
    let dir = root(&sm, &["Aldric"], false);
    fs::write(
        dir.path().join("characters/Raijin.yaml"),
        "schema: 1\nname: Raijin\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("characters/Brenna.yaml"),
        "schema: 1\nname: Brenna\nlogin:\n  account: brenna\n",
    )
    .unwrap();
    let server = start(dir.path()).await;
    assert!(matches!(
        server.start_bot("Nobody"),
        Err(StartError::Config(_))
    ));
    assert_eq!(
        server.start_bot("Raijin"),
        Err(StartError::NoLogin("Raijin".into()))
    );
    assert_eq!(
        server.start_bot("Brenna"),
        Err(StartError::NoPassword("brenna".into()))
    );
    assert!(server.running().is_empty());
    server.shutdown().await;
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stopped_bot_can_be_started_again() {
    let sm = Silvermere::start(&players(&["Aldric"])).await.unwrap();
    let dir = root(&sm, &["Aldric"], false);
    let server = start(dir.path()).await;
    let mut rx = server.subscribe();
    server.start_bot("Aldric").unwrap();
    until(&mut rx, state("Aldric", SessionState::InGame)).await;
    assert_eq!(server.stop_bot("Aldric").await, Some(Offline::LoggedOff));
    assert!(server.running().is_empty());
    server.start_bot("Aldric").unwrap();
    until(&mut rx, state("Aldric", SessionState::InGame)).await;
    server.shutdown().await;
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_edited_character_file_reaches_the_running_bot() {
    let sm = Silvermere::start(&players(&["Aldric"])).await.unwrap();
    let dir = root(&sm, &["Aldric"], false);
    let server = start(dir.path()).await;
    let mut rx = server.subscribe();
    server.start_bot("Aldric").unwrap();
    until(&mut rx, state("Aldric", SessionState::InGame)).await;
    let path = dir.path().join("characters/Aldric.yaml");
    fs::write(&path, character("Aldric", "[%h/%H HP]:%r", false)).unwrap();
    let e = until(&mut rx, |e| {
        matches!(
            e,
            BotEvent::Session {
                report: Report::ConfigApplied { .. },
                ..
            }
        )
    })
    .await;
    let BotEvent::Session {
        report: Report::ConfigApplied { rev },
        ..
    } = e
    else {
        unreachable!()
    };
    assert_eq!(rev, mp_config::load_character(&path).unwrap().rev);
    server.shutdown().await;
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_broken_edit_keeps_the_old_config_and_says_why() {
    let sm = Silvermere::start(&players(&["Aldric"])).await.unwrap();
    let dir = root(&sm, &["Aldric"], false);
    let server = start(dir.path()).await;
    let mut rx = server.subscribe();
    server.start_bot("Aldric").unwrap();
    until(&mut rx, state("Aldric", SessionState::InGame)).await;
    fs::write(
        dir.path().join("characters/Aldric.yaml"),
        character("Aldric", "[MA=%m]:", false),
    )
    .unwrap();
    let e = until(&mut rx, |e| matches!(e, BotEvent::ConfigError { .. })).await;
    let BotEvent::ConfigError { name, message } = e else {
        unreachable!()
    };
    assert_eq!(name, "Aldric");
    assert!(message.contains("statline"), "{message}");
    assert_eq!(server.running(), ["Aldric"]);
    server.shutdown().await;
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_orderly_stop_logs_every_bot_off() {
    let sm = Silvermere::start(&players(&["Aldric", "Brenna"]))
        .await
        .unwrap();
    let dir = root(&sm, &["Aldric", "Brenna"], false);
    let server = start(dir.path()).await;
    let mut rx = server.subscribe();
    server.start_bot("Aldric").unwrap();
    server.start_bot("Brenna").unwrap();
    // Both log in at once, in either order.
    let mut in_game = std::collections::BTreeSet::new();
    while in_game.len() < 2 {
        let e = until(&mut rx, |e| {
            matches!(
                e,
                BotEvent::Session {
                    report: Report::State(SessionState::InGame),
                    ..
                }
            )
        })
        .await;
        if let BotEvent::Session { name, .. } = e {
            in_game.insert(name);
        }
    }
    let ended = server.shutdown().await;
    assert_eq!(
        ended,
        [
            ("Aldric".to_owned(), Offline::LoggedOff),
            ("Brenna".to_owned(), Offline::LoggedOff)
        ]
    );
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn bots_marked_autostart_start_with_the_server() {
    let sm = Silvermere::start(&players(&["Aldric"])).await.unwrap();
    let dir = root(&sm, &["Aldric"], true);
    let server = start(dir.path()).await;
    assert_eq!(server.running(), ["Aldric"]);
    let mut rx = server.subscribe();
    until(&mut rx, state("Aldric", SessionState::InGame)).await;
    server.shutdown().await;
    sm.stop().await.unwrap();
}

#[test]
fn offline_is_recognised() {
    assert!(offline("Aldric")(&BotEvent::Session {
        name: "Aldric".into(),
        report: Report::State(SessionState::Offline(Offline::LoggedOff)),
    }));
}
