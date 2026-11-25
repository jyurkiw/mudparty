//! Party join and resting in the session, against a real silvermere on its stock world
//! (design §6.5, §6.6, §10.4 scenarios 2 and 6, without the proxy): the mage bot Ermias and
//! the warrior Raijin, played by the test as the leader, in the arena (room 206), whose
//! north exit leads to room 205.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mp_config::{Amount, Pools};
use mp_engine::{
    Command, LoginSettings, PartySettings, PartyState, Report, RestSettings, SessionHandle,
    SessionSpec, SessionState, spawn,
};
use mp_parse::{Event, Item, Rest};
use mp_silvermere::{ARENA, RawClient, StockChar, StockSilvermere, WAIT};
use tokio::sync::mpsc;
use tokio::time::{Instant, timeout};

fn s(v: &str) -> String {
    v.to_owned()
}

fn party() -> PartySettings {
    PartySettings {
        name: s("Ermias"),
        leader: s("Raijin"),
        retry: Duration::from_secs(30),
    }
}

/// Rest below 5 MP, up to 12, telling Raijin.
fn rest() -> RestSettings {
    RestSettings {
        start_when_any: Pools {
            hp: None,
            mp: Some(Amount::Points(5)),
        },
        stop_when_all: Pools {
            hp: None,
            mp: Some(Amount::Points(12)),
        },
        command: s("rest"),
        tell: Some(s("Raijin")),
        retry: Duration::from_millis(300),
        sick_wait: Duration::from_secs(30),
    }
}

fn spec(game: SocketAddr, party: Option<PartySettings>, rest: Option<RestSettings>) -> SessionSpec {
    SessionSpec {
        name: s("Ermias"),
        game: game.to_string(),
        login: LoginSettings {
            account: s("Ermias"),
            password: s("ermias1"),
            takeover: true,
            step_timeout: Duration::from_secs(20),
        },
        statline: s("[HP=%h/%H,MA=%m/%M]:%r"),
        config_rev: s("c1"),
        logoff_timeout: Duration::from_secs(20),
        reconnect_delays: vec![Duration::from_millis(200)],
        logs: None,
        log_rolls: Default::default(),
        combat: None,
        pacing: Default::default(),
        party,
        rest,
        remote: None,
        automation: Default::default(),
        rooms: None,
    }
}

async fn world(manual_clock: bool) -> StockSilvermere {
    StockSilvermere::start(
        &[
            StockChar::new("Ermias", "Mage"),
            StockChar::new("Raijin", "Warrior"),
        ],
        manual_clock,
    )
    .await
    .expect("the stock world starts")
}

async fn start(
    sm: &StockSilvermere,
    party: Option<PartySettings>,
    rest: Option<RestSettings>,
) -> (SessionHandle, mpsc::UnboundedReceiver<Report>) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let h = spawn(spec(sm.telnet_addr(), party, rest), tx);
    until(
        &mut rx,
        |r| *r == Report::State(SessionState::InGame),
        "in the game",
    )
    .await;
    (h, rx)
}

/// Reads reports until one passes `want`, or [`WAIT`] passes.
async fn until(
    rx: &mut mpsc::UnboundedReceiver<Report>,
    mut want: impl FnMut(&Report) -> bool,
    what: &str,
) -> Vec<Report> {
    let deadline = Instant::now() + WAIT;
    let mut seen = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match timeout(left, rx.recv()).await {
            Ok(Some(r)) => {
                let done = want(&r);
                seen.push(r);
                if done {
                    return seen;
                }
            }
            _ => panic!("never {what}; saw {seen:#?}"),
        }
    }
}

/// Steps rounds until a report passes `want`, or [`WAIT`] passes.
async fn until_ticking(
    sm: &StockSilvermere,
    rx: &mut mpsc::UnboundedReceiver<Report>,
    mut want: impl FnMut(&Report) -> bool,
    what: &str,
) -> Vec<Report> {
    let deadline = Instant::now() + WAIT;
    let mut seen = Vec::new();
    loop {
        assert!(Instant::now() < deadline, "never {what}; saw {seen:#?}");
        sm.tick(5).await;
        while let Ok(Some(r)) = timeout(Duration::from_millis(100), rx.recv()).await {
            let done = want(&r);
            seen.push(r);
            if done {
                return seen;
            }
        }
    }
}

/// The room a player is in.
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

async fn set_mana(sm: &StockSilvermere, mana: i16, max: u16) {
    sm.on_world(move |w| {
        for id in w.session_ids() {
            if let Some(p) = w.session_mut(id).and_then(|s| s.character.as_mut())
                && p.firstname == "Ermias"
            {
                p.maxmana = max;
                p.mana = mana;
            }
        }
    })
    .await;
}

fn telepath(item: &Item, text: &str) -> bool {
    matches!(item, Item::Event(Event::Telepath { who, text: t }) if who == "Ermias" && t == text)
}

/// The leader logs in, then answers the bot's `@invite` until it follows.
async fn leader_invites(sm: &StockSilvermere) -> tokio::task::JoinHandle<RawClient> {
    let mut leader = RawClient::log_in(sm.telnet_addr(), "Raijin", "raijin1")
        .await
        .unwrap();
    tokio::spawn(async move {
        leader
            .answer_until(
                |i| telepath(i, "@invite").then(|| s("invite Ermias")),
                |i| matches!(i, Item::Event(Event::StartedFollow { who }) if who == "Ermias"),
            )
            .await
            .expect("the bot asks and follows");
        leader
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bot_asks_its_leader_and_follows_where_it_goes() {
    let sm = world(false).await;
    let leader = leader_invites(&sm).await;
    let (h, mut rx) = start(&sm, Some(party()), None).await;
    let seen = until(
        &mut rx,
        |r| *r == Report::Party(PartyState::Following),
        "following",
    )
    .await;
    for line in [
        "set follow normal",
        "par",
        "/Raijin @invite",
        "follow Raijin",
    ] {
        assert!(seen.contains(&Report::Sent(s(line))), "{line}: {seen:#?}");
    }
    let mut leader = timeout(WAIT, leader).await.unwrap().unwrap();
    assert_eq!(room_of(&sm, "Ermias").await, ARENA);
    leader.send("n").await.unwrap();
    let deadline = Instant::now() + WAIT;
    while room_of(&sm, "Ermias").await != 205 {
        assert!(Instant::now() < deadline, "the bot never followed north");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    h.send(Command::Logoff);
    h.finished().await;
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn below_the_threshold_the_bot_tells_its_leader_rests_and_says_ok() {
    let sm = world(true).await;
    let mut leader = RawClient::log_in(sm.telnet_addr(), "Raijin", "raijin1")
        .await
        .unwrap();
    let (h, mut rx) = start(&sm, None, Some(rest())).await;
    set_mana(&sm, 2, 40).await;
    // A round brings a prompt with the new MP.
    sm.tick(5).await;
    until(&mut rx, |r| *r == Report::Resting(true), "resting").await;
    leader
        .read_until(|i| telepath(i, "@wait"))
        .await
        .expect("the leader is told to wait");
    let seen = until_ticking(
        &sm,
        &mut rx,
        |r| *r == Report::Resting(false),
        "done resting",
    )
    .await;
    leader
        .read_until(|i| telepath(i, "@ok"))
        .await
        .expect("the leader is told the rest is over");
    let mp = seen
        .iter()
        .rev()
        .find_map(|r| match r {
            Report::Vitals(v) => v.mp,
            _ => None,
        })
        .expect("prompts while resting");
    assert!(mp >= 12, "rested to the stop threshold, at {mp}");
    h.send(Command::Logoff);
    h.finished().await;
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn dragged_off_by_its_leader_mid_rest_the_bot_rests_again() {
    let sm = world(true).await;
    let leader = leader_invites(&sm).await;
    let (h, mut rx) = start(&sm, Some(party()), Some(rest())).await;
    until(
        &mut rx,
        |r| *r == Report::Party(PartyState::Following),
        "following",
    )
    .await;
    let mut leader = timeout(WAIT, leader).await.unwrap().unwrap();
    set_mana(&sm, 2, 40).await;
    until_ticking(
        &sm,
        &mut rx,
        |r| matches!(r, Report::Vitals(v) if v.rest == Rest::Resting),
        "resting",
    )
    .await;
    leader.send("n").await.unwrap();
    until_ticking(
        &sm,
        &mut rx,
        |r| *r == Report::Sent(s("rest")),
        "resting again",
    )
    .await;
    assert_eq!(room_of(&sm, "Ermias").await, 205);
    h.send(Command::Logoff);
    h.finished().await;
    sm.stop().await.unwrap();
}
