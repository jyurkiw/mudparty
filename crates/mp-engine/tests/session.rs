//! The session runner against a real silvermere (design §6.2, §4.4, §10.4): connect, log
//! in, set the statline, take config changes, log off, reconnect.

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use mp_engine::{
    Command, LoginFailure, LoginSettings, Offline, Report, SessionHandle, SessionSpec,
    SessionState, spawn,
};
use mp_parse::{Event, Item};
use mp_silvermere::{Player, RawClient, Relay, Silvermere, WAIT};
use tokio::sync::mpsc;
use tokio::time::timeout;

const BOT: &str = "[HP=%h/%H,MA=%m/%M]:%r";

fn spec(game: SocketAddr, password: &str, logs: Option<&Path>) -> SessionSpec {
    SessionSpec {
        name: "Aldric".into(),
        game: game.to_string(),
        login: LoginSettings {
            account: "Aldric".into(),
            password: password.into(),
            takeover: true,
            step_timeout: Duration::from_secs(10),
        },
        statline: BOT.into(),
        config_rev: "c1".into(),
        logoff_timeout: Duration::from_secs(20),
        reconnect_delays: vec![Duration::from_millis(100)],
        logs: logs.map(Path::to_owned),
        combat: None,
        pacing: Default::default(),
        party: None,
        rest: None,
        remote: None,
        automation: Default::default(),
    }
}

fn start(spec: SessionSpec) -> (SessionHandle, mpsc::UnboundedReceiver<Report>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (spawn(spec, tx), rx)
}

/// Waits for a report that satisfies `want`, returning every report seen up to it.
async fn until(
    rx: &mut mpsc::UnboundedReceiver<Report>,
    want: impl Fn(&Report) -> bool,
) -> Vec<Report> {
    let mut seen = Vec::new();
    loop {
        let r = timeout(WAIT, rx.recv())
            .await
            .unwrap_or_else(|_| panic!("no wanted report in time; saw {seen:#?}"))
            .expect("the session reports until it ends");
        let done = want(&r);
        seen.push(r);
        if done {
            return seen;
        }
    }
}

fn is(state: SessionState) -> impl Fn(&Report) -> bool {
    move |r| *r == Report::State(state.clone())
}

fn offline(r: &Report) -> bool {
    matches!(r, Report::State(SessionState::Offline(_)))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bot_logs_in_and_sets_its_own_statline() {
    let sm = Silvermere::start(&[Player::new("Aldric", "aldric1").hp(35)])
        .await
        .unwrap();
    let (h, mut rx) = start(spec(sm.telnet_addr(), "aldric1", None));
    let seen = until(
        &mut rx,
        |r| matches!(r, Report::Vitals(v) if v.max_hp == Some(35)),
    )
    .await;
    // The order of a login.
    let states: Vec<_> = seen
        .iter()
        .filter_map(|r| match r {
            Report::State(s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        states,
        [
            SessionState::Connecting,
            SessionState::LoggingIn,
            SessionState::InGame
        ]
    );
    h.send(Command::Logoff);
    until(&mut rx, offline).await;
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_password_stops_the_session_without_retrying() {
    let sm = Silvermere::start(&[Player::new("Aldric", "aldric1")])
        .await
        .unwrap();
    let (h, mut rx) = start(spec(sm.telnet_addr(), "wrong", None));
    let seen = until(&mut rx, offline).await;
    assert_eq!(
        seen.last(),
        Some(&Report::State(SessionState::Offline(Offline::LoginFailed(
            LoginFailure::InvalidCredentials
        ))))
    );
    assert_eq!(
        h.finished().await,
        Offline::LoginFailed(LoginFailure::InvalidCredentials)
    );
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_logoff_quits_the_realm_and_leaves_the_account_free() {
    let sm = Silvermere::start(&[Player::new("Aldric", "aldric1")])
        .await
        .unwrap();
    let (h, mut rx) = start(spec(sm.telnet_addr(), "aldric1", None));
    until(&mut rx, is(SessionState::InGame)).await;
    h.send(Command::Logoff);
    let seen = until(&mut rx, offline).await;
    assert!(
        seen.contains(&Report::State(SessionState::LoggingOff)),
        "{seen:#?}"
    );
    assert_eq!(
        seen.last(),
        Some(&Report::State(SessionState::Offline(Offline::LoggedOff)))
    );
    // Logged off cleanly: a new login is not asked to take anything over.
    let c = RawClient::log_in(sm.telnet_addr(), "Aldric", "aldric1")
        .await
        .unwrap();
    assert!(
        !c.seen.contains(&Item::Event(Event::TakeoverQuestion)),
        "{:#?}",
        c.seen
    );
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bot_takes_over_a_session_left_logged_in() {
    let sm = Silvermere::start(&[Player::new("Aldric", "aldric1")])
        .await
        .unwrap();
    let mut old = RawClient::log_in(sm.telnet_addr(), "Aldric", "aldric1")
        .await
        .unwrap();
    let (h, mut rx) = start(spec(sm.telnet_addr(), "aldric1", None));
    until(&mut rx, is(SessionState::InGame)).await;
    old.read_until(|i| *i == Item::Event(Event::TakenOver))
        .await
        .unwrap();
    h.send(Command::Logoff);
    until(&mut rx, offline).await;
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bot_taken_over_by_another_login_stops_without_reconnecting() {
    let sm = Silvermere::start(&[Player::new("Aldric", "aldric1")])
        .await
        .unwrap();
    let (h, mut rx) = start(spec(sm.telnet_addr(), "aldric1", None));
    until(&mut rx, is(SessionState::InGame)).await;
    let _user = RawClient::log_in(sm.telnet_addr(), "Aldric", "aldric1")
        .await
        .unwrap();
    let seen = until(&mut rx, offline).await;
    assert_eq!(
        seen.last(),
        Some(&Report::State(SessionState::Offline(Offline::TakenOver)))
    );
    assert_eq!(h.finished().await, Offline::TakenOver);
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lost_link_is_reconnected() {
    let sm = Silvermere::start(&[Player::new("Aldric", "aldric1")])
        .await
        .unwrap();
    let relay = Relay::start(sm.telnet_addr()).await.unwrap();
    let (h, mut rx) = start(spec(relay.addr(), "aldric1", None));
    until(&mut rx, is(SessionState::InGame)).await;
    relay.cut();
    let seen = until(&mut rx, is(SessionState::InGame)).await;
    assert!(
        seen.iter()
            .any(|r| matches!(r, Report::State(SessionState::Reconnecting { .. }))),
        "{seen:#?}"
    );
    h.send(Command::Logoff);
    until(&mut rx, offline).await;
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_game_that_never_answers_times_the_login_out() {
    let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = silent.local_addr().unwrap();
    let _keep = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((s, _)) = silent.accept().await {
            held.push(s);
        }
    });
    let mut s = spec(addr, "aldric1", None);
    s.login.step_timeout = Duration::from_millis(300);
    let (_h, mut rx) = start(s);
    let seen = until(&mut rx, offline).await;
    assert_eq!(
        seen.last(),
        Some(&Report::State(SessionState::Offline(Offline::LoginFailed(
            LoginFailure::Timeout {
                waiting_for: "the username prompt"
            }
        ))))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_statline_is_applied_in_the_game() {
    let sm = Silvermere::start(&[Player::new("Aldric", "aldric1").hp(35)])
        .await
        .unwrap();
    let (h, mut rx) = start(spec(sm.telnet_addr(), "aldric1", None));
    until(&mut rx, is(SessionState::InGame)).await;
    h.send(Command::Config {
        statline: "[%h/%H HP]:%r".into(),
        rev: "c2".into(),
    });
    until(&mut rx, |r| {
        *r == Report::ConfigApplied { rev: "c2".into() }
    })
    .await;
    h.send(Command::Logoff);
    until(&mut rx, offline).await;
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_logs_everything_and_never_its_password() {
    let sm = Silvermere::start(&[Player::new("Aldric", "aldric1")])
        .await
        .unwrap();
    let logs = tempfile::tempdir().unwrap();
    let (h, mut rx) = start(spec(sm.telnet_addr(), "aldric1", Some(logs.path())));
    until(&mut rx, is(SessionState::InGame)).await;
    h.send(Command::Logoff);
    until(&mut rx, offline).await;
    h.finished().await;
    let dir = logs.path().join("Aldric");
    let transcript = std::fs::read_to_string(dir.join("transcript.log")).unwrap();
    assert!(transcript.contains("> ********  [engine]"), "{transcript}");
    assert!(
        transcript.contains(&format!("> set statline full custom {BOT}  [engine]")),
        "{transcript}"
    );
    assert!(transcript.contains("> quit  [engine]"), "{transcript}");
    let events = std::fs::read_to_string(dir.join("events.jsonl")).unwrap();
    for kind in ["\"connected\"", "\"in_game\"", "\"logged_off\""] {
        assert!(events.contains(kind), "{kind} in {events}");
    }
    for e in std::fs::read_dir(&dir).unwrap() {
        let text = String::from_utf8_lossy(&std::fs::read(e.unwrap().path()).unwrap()).into_owned();
        assert!(!text.contains("aldric1"), "a password in the logs");
    }
    sm.stop().await.unwrap();
}

// silvermere saves a character's statline with the character, so a character a bot has
// played comes back with the bot's statline on its next login.
#[tokio::test(flavor = "multi_thread")]
async fn a_character_a_bot_played_before_logs_in_again() {
    let sm = Silvermere::start(&[Player::new("Aldric", "aldric1")])
        .await
        .unwrap();
    for _ in 0..2 {
        let (h, mut rx) = start(spec(sm.telnet_addr(), "aldric1", None));
        until(&mut rx, is(SessionState::InGame)).await;
        h.send(Command::Logoff);
        until(&mut rx, offline).await;
    }
    sm.stop().await.unwrap();
}
