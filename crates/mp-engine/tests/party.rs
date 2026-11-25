//! Party join (design §6.5), as pure logic fed with silvermere's own rendered text: check
//! `par`, ask the leader with `@invite`, follow on the invitation, wait when not in the same
//! room, and start again when the party is lost.

use std::time::{Duration, Instant};

use mp_engine::{Action, PartyJoin, PartySettings, PartyState};
use mp_parse::{Item, Reader, Statline};
use silvermere_text::{Msg, Palette, render};

const BOT: &str = "[HP=%h/%H,MA=%m/%M]:%r";

fn s(v: &str) -> String {
    v.to_owned()
}

fn join() -> PartyJoin {
    PartyJoin::new(PartySettings {
        name: s("Ermias"),
        leader: s("Raijin"),
        retry: Duration::from_secs(30),
    })
}

/// What silvermere's rendering of `msgs` reads as.
fn items(msgs: Vec<Msg>) -> Vec<Item> {
    let mut r = Reader::new(Some(Statline::compile(BOT).unwrap()));
    let mut out = Vec::new();
    for m in msgs {
        out.extend(r.feed(&render(&m, Palette::new(0).unwrap()).bytes));
    }
    out
}

fn feed(p: &mut PartyJoin, msgs: Vec<Msg>, now: Instant) -> Vec<Action> {
    items(msgs).iter().flat_map(|i| p.on_item(i, now)).collect()
}

fn own_row() -> Msg {
    Msg::PartyRow {
        name: format!("{:<30}", "Ermias"),
        class: format!("{:<12}", "(Mage)"),
        mana: " [M:100%]".into(),
        hp: "100".into(),
        poison: " ".into(),
        rest: " ".into(),
        rank: " - Frontrank".into(),
    }
}

fn send(l: &str) -> Action {
    Action::Send(s(l))
}

fn invite() -> Action {
    Action::Telepath {
        to: s("Raijin"),
        text: s("@invite"),
    }
}

#[test]
fn it_starts_by_asking_for_full_rooms_and_the_party() {
    let mut p = join();
    assert_eq!(
        p.start(Instant::now()),
        vec![send("set follow normal"), send("par")]
    );
    assert_eq!(p.state(), PartyState::Checking);
}

#[test]
fn already_following_the_leader_means_done() {
    let t = Instant::now();
    let mut p = join();
    p.start(t);
    let acts = feed(
        &mut p,
        vec![
            Msg::PartyFollowing { who: s("Raijin") },
            Msg::PartyNone,
            own_row(),
        ],
        t,
    );
    assert!(acts.is_empty(), "{acts:?}");
    assert_eq!(p.state(), PartyState::Following);
}

#[test]
fn not_in_a_party_asks_the_leader_then_follows_the_invitation() {
    let t = Instant::now();
    let mut p = join();
    p.start(t);
    assert_eq!(
        feed(&mut p, vec![Msg::PartyNone, own_row()], t),
        vec![invite()]
    );
    assert_eq!(p.state(), PartyState::Asking);
    let acts = feed(
        &mut p,
        vec![Msg::InvitedYouFollow {
            who: s("Raijin"),
            whom: s("him"),
        }],
        t,
    );
    assert_eq!(acts, vec![send("follow Raijin")]);
    feed(&mut p, vec![Msg::NowFollowing { who: s("Raijin") }], t);
    assert_eq!(p.state(), PartyState::Following);
}

#[test]
fn an_invitation_from_someone_else_is_ignored() {
    let t = Instant::now();
    let mut p = join();
    p.start(t);
    feed(&mut p, vec![Msg::PartyNone, own_row()], t);
    let acts = feed(
        &mut p,
        vec![Msg::InvitedYouFollow {
            who: s("Mallory"),
            whom: s("her"),
        }],
        t,
    );
    assert!(acts.is_empty());
    assert_eq!(p.state(), PartyState::Asking);
}

#[test]
fn following_someone_else_still_asks_the_leader() {
    let t = Instant::now();
    let mut p = join();
    p.start(t);
    let acts = feed(
        &mut p,
        vec![
            Msg::PartyFollowing { who: s("Mallory") },
            Msg::PartyNone,
            own_row(),
        ],
        t,
    );
    assert_eq!(acts, vec![invite()]);
}

#[test]
fn must_be_invited_first_asks_again() {
    let t = Instant::now();
    let mut p = join();
    p.start(t);
    feed(&mut p, vec![Msg::PartyNone, own_row()], t);
    feed(
        &mut p,
        vec![Msg::InvitedYouFollow {
            who: s("Raijin"),
            whom: s("him"),
        }],
        t,
    );
    assert_eq!(feed(&mut p, vec![Msg::InviteFirst], t), vec![invite()]);
}

#[test]
fn not_in_the_same_room_waits_and_checks_again_later() {
    let t = Instant::now();
    let mut p = join();
    p.start(t);
    feed(&mut p, vec![Msg::PartyNone, own_row()], t);
    feed(
        &mut p,
        vec![Msg::InvitedYouFollow {
            who: s("Raijin"),
            whom: s("him"),
        }],
        t,
    );
    assert!(feed(&mut p, vec![Msg::FollowNothing], t).is_empty());
    assert_eq!(p.state(), PartyState::NotWithLeader);
    assert!(p.on_tick(t + Duration::from_secs(10)).is_empty());
    assert_eq!(p.on_tick(t + Duration::from_secs(31)), vec![send("par")]);
    assert_eq!(p.state(), PartyState::Checking);
}

#[test]
fn an_unanswered_ask_is_checked_again_later() {
    let t = Instant::now();
    let mut p = join();
    p.start(t);
    feed(&mut p, vec![Msg::PartyNone, own_row()], t);
    assert_eq!(p.on_tick(t + Duration::from_secs(31)), vec![send("par")]);
}

#[test]
fn losing_the_leader_starts_again() {
    let t = Instant::now();
    let mut p = join();
    p.start(t);
    feed(
        &mut p,
        vec![
            Msg::PartyFollowing { who: s("Raijin") },
            Msg::PartyNone,
            own_row(),
        ],
        t,
    );
    let acts = feed(&mut p, vec![Msg::NoLongerFollowing { who: s("Raijin") }], t);
    assert_eq!(acts, vec![send("par")]);
    assert_eq!(p.state(), PartyState::Checking);
}

#[test]
fn being_removed_from_the_followers_starts_again() {
    let t = Instant::now();
    let mut p = join();
    p.start(t);
    feed(&mut p, vec![Msg::NowFollowing { who: s("Raijin") }], t);
    assert_eq!(p.state(), PartyState::Following);
    let acts = feed(&mut p, vec![Msg::FollowerRemoved { who: s("Ermias") }], t);
    assert_eq!(acts, vec![send("par")]);
}
