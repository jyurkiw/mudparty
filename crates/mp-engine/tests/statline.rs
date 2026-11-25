//! A bot leaves the character's own statline as it found it (#34, design §6.1): it reads
//! the statline with `pro`, saves it before setting its own, sets nothing when its own is
//! already there, and puts the saved one back on an orderly logoff.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mp_engine::{
    Command, LoginSettings, Report, SessionHandle, SessionSpec, SessionState, StatlineStore, spawn,
};
use mp_parse::{Event, Item};
use mp_silvermere::{Player, RawClient, Silvermere, WAIT};
use tokio::sync::mpsc;
use tokio::time::timeout;

const BOT: &str = "[HP=%h/%H,MA=%m/%M]:%r";

/// The saved originals, in memory.
#[derive(Debug, Default)]
struct Memory(Mutex<HashMap<String, String>>);

impl StatlineStore for Memory {
    fn saved(&self, name: &str) -> Option<String> {
        self.0.lock().unwrap().get(name).cloned()
    }
    fn save(&self, name: &str, original: &str) {
        self.0
            .lock()
            .unwrap()
            .insert(name.to_owned(), original.to_owned());
    }
    fn clear(&self, name: &str) {
        self.0.lock().unwrap().remove(name);
    }
}

fn spec(game: SocketAddr, store: Arc<Memory>) -> SessionSpec {
    SessionSpec {
        name: "Aldric".into(),
        game: game.to_string(),
        login: LoginSettings {
            account: "Aldric".into(),
            password: "aldric1".into(),
            takeover: true,
            step_timeout: Duration::from_secs(10),
        },
        statline: BOT.into(),
        config_rev: "c1".into(),
        logoff_timeout: Duration::from_secs(20),
        reconnect_delays: vec![Duration::from_millis(100)],
        logs: None,
        log_rolls: Default::default(),
        combat: None,
        pacing: Default::default(),
        party: None,
        rest: None,
        remote: None,
        automation: Default::default(),
        rooms: None,
        hang_up_below: None,
        inviter: None,
        statline_store: Some(store),
        statline_restore: true,
    }
}

fn start(spec: SessionSpec) -> (SessionHandle, mpsc::UnboundedReceiver<Report>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (spawn(spec, tx), rx)
}

/// Every report up to one that satisfies `want`.
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

fn offline(r: &Report) -> bool {
    matches!(r, Report::State(SessionState::Offline(_)))
}

fn sent(seen: &[Report]) -> Vec<String> {
    seen.iter()
        .filter_map(|r| match r {
            Report::Sent(l) => Some(l.clone()),
            _ => None,
        })
        .collect()
}

/// Logs Aldric in by hand, runs `lines`, reads `pro`'s statline, and quits the realm.
async fn by_hand(addr: SocketAddr, lines: &[&str]) -> String {
    let mut c = RawClient::log_in(addr, "Aldric", "aldric1").await.unwrap();
    for l in lines {
        c.send(l).await.unwrap();
        c.read_until(|i| matches!(i, Item::Prompt(_) | Item::Text(_)))
            .await
            .unwrap();
    }
    c.send("pro").await.unwrap();
    let Item::Event(Event::ProfileStatline { mode }) = c
        .read_until(|i| matches!(i, Item::Event(Event::ProfileStatline { .. })))
        .await
        .unwrap()
    else {
        unreachable!()
    };
    c.send("quit").await.unwrap();
    c.read_until(|i| matches!(i, Item::Event(Event::MudMenu)))
        .await
        .unwrap();
    mode
}

#[tokio::test(flavor = "multi_thread")]
async fn a_full_statline_is_saved_replaced_and_put_back() {
    let sm = Silvermere::start(&[Player::new("Aldric", "aldric1")])
        .await
        .unwrap();
    assert_eq!(
        by_hand(sm.telnet_addr(), &["set statline full"]).await,
        "Full"
    );
    let store = Arc::new(Memory::default());
    let (h, mut rx) = start(spec(sm.telnet_addr(), store.clone()));
    let seen = until(
        &mut rx,
        |r| matches!(r, Report::Sent(l) if l.starts_with("set statline full custom")),
    )
    .await;
    assert!(sent(&seen).contains(&"pro".to_owned()), "it read first");
    assert_eq!(store.saved("Aldric").as_deref(), Some("Full"));
    h.send(Command::Logoff);
    let seen = until(&mut rx, offline).await;
    let lines = sent(&seen);
    let restore = lines.iter().position(|l| l == "set statline full");
    let quit = lines.iter().position(|l| l == "quit");
    assert!(restore.is_some() && restore < quit, "{lines:?}");
    assert_eq!(store.saved("Aldric"), None, "restored, so nothing is kept");
    assert_eq!(by_hand(sm.telnet_addr(), &[]).await, "Full");
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_character_already_on_the_bot_statline_gets_no_set() {
    let sm = Silvermere::start(&[Player::new("Aldric", "aldric1")])
        .await
        .unwrap();
    by_hand(
        sm.telnet_addr(),
        &[&format!("set statline full custom {BOT}")],
    )
    .await;
    let store = Arc::new(Memory::default());
    let (h, mut rx) = start(spec(sm.telnet_addr(), store.clone()));
    until(&mut rx, |r| matches!(r, Report::Sent(l) if l == "pro")).await;
    // Past the answer to `pro`: a prompt after it.
    let seen = until(&mut rx, |r| matches!(r, Report::Vitals(_))).await;
    h.send(Command::Logoff);
    let mut seen2 = until(&mut rx, offline).await;
    seen2.splice(0..0, seen);
    let lines = sent(&seen2);
    assert!(
        !lines.iter().any(|l| l.starts_with("set statline")),
        "{lines:?}"
    );
    assert_eq!(store.saved("Aldric"), None);
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn after_a_crash_the_saved_original_is_kept_and_restored_later() {
    let sm = Silvermere::start(&[Player::new("Aldric", "aldric1")])
        .await
        .unwrap();
    by_hand(sm.telnet_addr(), &["set statline on"]).await;
    let store = Arc::new(Memory::default());
    let (h, mut rx) = start(spec(sm.telnet_addr(), store.clone()));
    until(
        &mut rx,
        |r| matches!(r, Report::Sent(l) if l.starts_with("set statline full custom")),
    )
    .await;
    // A crash: the session goes without logging off, so nothing is put back.
    drop(h);
    until(&mut rx, offline).await;
    assert_eq!(store.saved("Aldric").as_deref(), Some("On"));

    // The next login finds the bot's own statline still set: it sets nothing, and keeps the
    // original saved before the crash rather than taking its own for one.
    let (h, mut rx) = start(spec(sm.telnet_addr(), store.clone()));
    until(&mut rx, |r| matches!(r, Report::Sent(l) if l == "pro")).await;
    until(&mut rx, |r| matches!(r, Report::Vitals(_))).await;
    assert_eq!(store.saved("Aldric").as_deref(), Some("On"));
    h.send(Command::Logoff);
    let seen = until(&mut rx, offline).await;
    assert!(
        sent(&seen).contains(&"set statline on".to_owned()),
        "{seen:?}"
    );
    assert_eq!(by_hand(sm.telnet_addr(), &[]).await, "On");
    sm.stop().await.unwrap();
}
