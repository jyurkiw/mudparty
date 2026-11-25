//! @ commands in the session, against a real silvermere on its stock world (design §6.3,
//! §10.4 scenario 7): the test plays Raijin, who may command the bot Ermias, and Kaelen,
//! who may not, telepathing from the arena.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use mp_config::{Automation, Channel, Melee, Rule};
use mp_engine::{
    AutoState, CombatSettings, Command, LoginSettings, RemoteSettings, Report, RoomBook, RoomInfo,
    SessionHandle, SessionSpec, SessionState, spawn,
};
use mp_parse::{Event, Item};
use mp_silvermere::{RawClient, StockChar, StockSilvermere, WAIT};
use regex::Regex;
use tokio::sync::mpsc;
use tokio::time::{Instant, timeout};

fn s(v: &str) -> String {
    v.to_owned()
}

/// The game data's arena, standing in for the server's lookup.
#[derive(Debug)]
struct Arena;

impl RoomBook for Arena {
    fn room(&self, map: u32, room: u32) -> Option<RoomInfo> {
        ((map, room) == (1, 206)).then(|| RoomInfo {
            name: s("Arena Practice Room"),
            exits: vec![0, 8],
        })
    }
}

fn spec(game: SocketAddr, warn_denied: bool) -> SessionSpec {
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
        combat: Some(CombatSettings {
            name: s("Ermias"),
            leader: None,
            targeting: vec![Rule::KillOrder(true)],
            tiers: vec![],
            room_tiers: vec![],
            melee: Melee::Command(s("a")),
            spell_costs: Default::default(),
            monsters: vec![s("practice dummy")],
            leader_target_stale: Duration::from_secs(6),
            round: Duration::from_millis(200),
        }),
        pacing: Default::default(),
        party: None,
        rest: None,
        remote: Some(RemoteSettings {
            name: s("Ermias"),
            allow: vec![s("Raijin")],
            channels: vec![Channel::Telepath, Channel::Gangpath],
            warn_denied,
            version: s("0.1.0"),
        }),
        automation: Automation::default(),
        rooms: Some(Arc::new(Arena)),
        hang_up_below: None,
        inviter: None,
        statline_store: None,
        statline_restore: true,
    }
}

async fn world() -> StockSilvermere {
    StockSilvermere::start(
        &[
            StockChar::new("Ermias", "Mage"),
            StockChar::new("Raijin", "Warrior"),
            StockChar::new("Kaelen", "Warrior"),
        ],
        false,
    )
    .await
    .expect("the stock world starts")
}

async fn start(
    sm: &StockSilvermere,
    warn_denied: bool,
) -> (SessionHandle, mpsc::UnboundedReceiver<Report>) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let h = spawn(spec(sm.telnet_addr(), warn_denied), tx);
    until(&mut rx, |r| *r == Report::State(SessionState::InGame)).await;
    // A prompt in the bot's own statline, which shows the maxima, so @health has them. The
    // bot sets it only once PROFILE has said what the character had (#34).
    until(
        &mut rx,
        |r| matches!(r, Report::Vitals(v) if v.max_hp.is_some()),
    )
    .await;
    (h, rx)
}

async fn until(rx: &mut mpsc::UnboundedReceiver<Report>, mut want: impl FnMut(&Report) -> bool) {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match timeout(left, rx.recv()).await {
            Ok(Some(r)) if want(&r) => return,
            Ok(Some(_)) => {}
            _ => panic!("never saw the report"),
        }
    }
}

/// Telepaths `cmd` to the bot and returns the bot's telepathed reply.
async fn ask(player: &mut RawClient, cmd: &str) -> String {
    player.send(&format!("/Ermias {cmd}")).await.unwrap();
    let Item::Event(Event::Telepath { text, .. }) = player
        .read_until(|i| matches!(i, Item::Event(Event::Telepath { who, .. }) if who == "Ermias"))
        .await
        .expect("a reply")
    else {
        unreachable!()
    };
    text
}

#[tokio::test(flavor = "multi_thread")]
async fn each_command_from_an_allowed_sender_gets_its_answer() {
    let sm = world().await;
    let mut raijin = RawClient::log_in(sm.telnet_addr(), "Raijin", "raijin1")
        .await
        .unwrap();
    let (h, mut rx) = start(&sm, false).await;
    // A player receives at most maxpages + 1 telepaths each 3 s; this test sends more than
    // that, so both get the room a player would give with `set maxpages`.
    sm.on_world(|w| {
        for id in w.session_ids() {
            if let Some(p) = w.session_mut(id).and_then(|s| s.character.as_mut()) {
                p.pagemax = 50;
            }
        }
    })
    .await;

    let health = ask(&mut raijin, "@health").await;
    let mudplay = Regex::new(
        r"(?i)^\{?HP=(?<hp>-?\d+)/(?<hpmax>\d+)(?:,(?<mpkind>MA|KAI)=(?<mp>-?\d+)/(?<mpmax>\d+))?(?:,[^}]*)?\}?\s*$",
    )
    .unwrap();
    assert!(mudplay.is_match(&health), "{health}");

    assert_eq!(ask(&mut raijin, "@version").await, "{mudparty 0.1.0}");
    // These ask the game first: `/room`, then `exp`.
    let where_ = ask(&mut raijin, "@where").await;
    assert_eq!(
        where_,
        "{Arena Practice Room (map 1, room 206); exits: north, up}"
    );
    let mudplay_where = Regex::new(r"(?i)\{[^}]*\(map (\d+), room (\d+)\)").unwrap();
    assert!(mudplay_where.is_match(&where_), "MudPlay reads it");
    assert_eq!(
        ask(&mut raijin, "@status").await,
        "{idle; Arena Practice Room (map 1, room 206); no ailments}"
    );
    let exp = ask(&mut raijin, "@exp").await;
    assert!(
        exp.starts_with("{Made: 0  Needed: ") && exp.ends_with(" (L2)  Rate: unknown}"),
        "{exp}"
    );
    assert_eq!(ask(&mut raijin, "@do say hello").await, "{ok}");
    assert_eq!(
        ask(&mut raijin, "@auto-rest off").await,
        "{@auto-rest: off}"
    );
    until(&mut rx, |r| {
        matches!(r, Report::Automation(AutoState { rest: false, .. }))
    })
    .await;
    assert_eq!(ask(&mut raijin, "@stop").await, "{movement paused}");
    assert_eq!(ask(&mut raijin, "@rego").await, "{resuming}");

    raijin.send("/Ermias @kill practice dummy").await.unwrap();
    until(&mut rx, |r| *r == Report::Sent(s("a practice dummy"))).await;

    raijin.send("/Ermias @invite").await.unwrap();
    raijin
        .read_until(
            |i| matches!(i, Item::Event(Event::InvitedYouFollow { who }) if who == "Ermias"),
        )
        .await
        .expect("the bot invites the sender");

    h.send(Command::Logoff);
    h.finished().await;
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sender_not_allowed_gets_nothing_or_the_failure_reply() {
    let sm = world().await;
    let mut kaelen = RawClient::log_in(sm.telnet_addr(), "Kaelen", "kaelen1")
        .await
        .unwrap();
    let (h, _rx) = start(&sm, false).await;
    kaelen.send("/Ermias @health").await.unwrap();
    let heard = timeout(
        Duration::from_secs(2),
        kaelen.read_until(
            |i| matches!(i, Item::Event(Event::Telepath { who, .. }) if who == "Ermias"),
        ),
    )
    .await;
    assert!(heard.is_err(), "no answer: {heard:?}");
    h.send(Command::Logoff);
    h.finished().await;

    let (h, _rx) = start(&sm, true).await;
    assert_eq!(
        ask(&mut kaelen, "@health").await,
        "{command invalid or not allowed}"
    );
    h.send(Command::Logoff);
    h.finished().await;
    sm.stop().await.unwrap();
}
