//! Combat in the session, against a real silvermere on its stock world (design §6.4,
//! §10.4 scenarios 3 and 4; MVP 4b): a mage bot in the arena with the practice dummy, the
//! rounds stepped by the test.

use std::net::SocketAddr;
use std::time::Duration;

use mp_config::{Melee, Rule, Tier};
use mp_engine::{
    CombatSettings, Command, LoginSettings, Report, SessionHandle, SessionSpec, spawn,
};
use mp_silvermere::{RawClient, StockChar, StockSilvermere, WAIT};
use tokio::sync::mpsc;
use tokio::time::{Instant, timeout};

fn s(v: &str) -> String {
    v.to_owned()
}

fn combat(targeting: Vec<Rule>) -> CombatSettings {
    CombatSettings {
        name: s("Ermias"),
        leader: Some(s("Raijin")),
        targeting,
        tiers: vec![Tier {
            spell: s("mmis"),
            min_mp: 20,
        }],
        room_tiers: vec![],
        melee: Melee::Command(s("a")),
        spell_costs: Default::default(),
        monsters: vec![s("practice dummy")],
        leader_target_stale: Duration::from_secs(6),
        round: Duration::from_millis(200),
    }
}

fn spec(game: SocketAddr, combat: CombatSettings) -> SessionSpec {
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
        combat: Some(combat),
        pacing: Default::default(),
        party: None,
        rest: None,
        remote: None,
        automation: Default::default(),
        rooms: None,
        hang_up_below: None,
        inviter: None,
        statline_store: None,
        statline_restore: true,
        routine: None,
    }
}

/// The world with the mage Ermias (knowing magic missile) and the warrior Raijin, both in
/// the arena, on a hand-driven clock.
async fn arena() -> StockSilvermere {
    StockSilvermere::start(
        &[
            StockChar::new("Ermias", "Mage").knows(&["mmis"]),
            StockChar::new("Raijin", "Warrior"),
        ],
        true,
    )
    .await
    .expect("the stock world starts")
}

async fn set_mana(sm: &StockSilvermere, mana: i16) {
    sm.on_world(move |w| {
        for id in w.session_ids() {
            if let Some(p) = w.session_mut(id).and_then(|s| s.character.as_mut())
                && p.firstname == "Ermias"
            {
                p.maxmana = p.maxmana.max(mana as u16);
                p.mana = mana;
            }
        }
    })
    .await;
}

/// Steps rounds until the bot sends `line`, or [`WAIT`] passes.
async fn until_sent(
    sm: &StockSilvermere,
    rx: &mut mpsc::UnboundedReceiver<Report>,
    line: &str,
) -> Vec<Report> {
    let deadline = Instant::now() + WAIT;
    let mut seen = Vec::new();
    loop {
        assert!(
            Instant::now() < deadline,
            "the bot never sent {line:?}; saw {seen:#?}"
        );
        sm.tick(5).await;
        while let Ok(Some(r)) = timeout(Duration::from_millis(150), rx.recv()).await {
            let done = r == Report::Sent(s(line));
            seen.push(r);
            if done {
                return seen;
            }
        }
    }
}

async fn in_game(rx: &mut mpsc::UnboundedReceiver<Report>) {
    loop {
        let r = timeout(WAIT, rx.recv())
            .await
            .expect("in the game in time")
            .expect("reports");
        // In the game, with the bot's own statline set (#34 sets it once PROFILE answers),
        // so the engine reads MP.
        if matches!(r, Report::Vitals(v) if v.max_hp.is_some()) {
            return;
        }
    }
}

async fn start(
    sm: &StockSilvermere,
    c: CombatSettings,
) -> (SessionHandle, mpsc::UnboundedReceiver<Report>) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let h = spawn(spec(sm.telnet_addr(), c), tx);
    in_game(&mut rx).await;
    (h, rx)
}

#[tokio::test(flavor = "multi_thread")]
async fn tiers_follow_mp_down_to_melee_and_back() {
    let sm = arena().await;
    let (h, mut rx) = start(&sm, combat(vec![Rule::Priority(vec![s("practice dummy")])])).await;
    set_mana(&sm, 40).await;
    until_sent(&sm, &mut rx, "mmis practice dummy").await;
    set_mana(&sm, 5).await;
    until_sent(&sm, &mut rx, "a practice dummy").await;
    set_mana(&sm, 60).await;
    until_sent(&sm, &mut rx, "mmis practice dummy").await;
    h.send(Command::Logoff);
    drop(h);
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bot_assists_its_leader() {
    let sm = arena().await;
    let (h, mut rx) = start(&sm, combat(vec![Rule::AssistLeader(true)])).await;
    set_mana(&sm, 40).await;
    let mut leader = RawClient::log_in(sm.telnet_addr(), "Raijin", "raijin1")
        .await
        .unwrap();
    leader.send("a practice dummy").await.unwrap();
    until_sent(&sm, &mut rx, "mmis practice dummy").await;
    h.send(Command::Logoff);
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_kill_order_starts_a_fight_at_once() {
    let sm = arena().await;
    let (h, mut rx) = start(&sm, combat(vec![Rule::KillOrder(true)])).await;
    set_mana(&sm, 40).await;
    sm.tick(5).await;
    h.send(Command::Kill {
        target: s("practice dummy"),
    });
    until_sent(&sm, &mut rx, "mmis practice dummy").await;
    h.send(Command::Logoff);
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_rule_that_names_a_monster_the_bot_stays_out_of_it() {
    let sm = arena().await;
    let (h, mut rx) = start(&sm, combat(vec![Rule::AssistLeader(true)])).await;
    set_mana(&sm, 40).await;
    sm.tick(20).await;
    while let Ok(Some(r)) = timeout(Duration::from_millis(300), rx.recv()).await {
        assert!(
            !matches!(&r, Report::Sent(l) if l.contains("practice dummy")),
            "{r:?}"
        );
    }
    h.send(Command::Logoff);
    sm.stop().await.unwrap();
}
