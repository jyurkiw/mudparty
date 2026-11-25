//! Command pacing and telepath acks (design §6.1), as pure logic: lines go out on a time
//! floor with the prompt as a gate, a line the game dropped as typed too quickly goes again,
//! and each telepath is paired with its acknowledgement and sent again, up to 3 tries.

use std::time::{Duration, Instant};

use mp_engine::{Pacer, PacingSettings};
use mp_parse::{Event, Item, Rest, Vitals};

fn set() -> PacingSettings {
    PacingSettings {
        floor: Duration::from_millis(100),
        telepath_floor: Duration::from_millis(100),
        max_unanswered: 2,
        prompt_timeout: Duration::from_secs(3),
        telepath_tries: 3,
    }
}

fn prompt() -> Item {
    Item::Prompt(Vitals {
        hp: 30,
        max_hp: Some(30),
        mp: None,
        max_mp: None,
        rest: Rest::Standing,
    })
}

fn ms(t: Instant, n: u64) -> Instant {
    t + Duration::from_millis(n)
}

#[test]
fn lines_go_out_in_order_on_the_time_floor() {
    let t = Instant::now();
    let mut p = Pacer::new(set());
    p.push("look");
    p.push("par");
    assert_eq!(p.ready(t), vec!["look"]);
    assert_eq!(p.ready(ms(t, 50)), Vec::<String>::new(), "inside the floor");
    assert_eq!(p.next_at(), Some(ms(t, 100)));
    assert_eq!(p.ready(ms(t, 100)), vec!["par"]);
    assert_eq!(p.next_at(), None, "nothing left");
}

#[test]
fn the_prompt_gates_unanswered_lines() {
    let t = Instant::now();
    let mut p = Pacer::new(set());
    for l in ["a", "b", "c"] {
        p.push(l);
    }
    assert_eq!(p.ready(t), vec!["a"]);
    assert_eq!(p.ready(ms(t, 100)), vec!["b"]);
    assert_eq!(p.ready(ms(t, 200)), Vec::<String>::new(), "two unanswered");
    p.on_item(&prompt(), ms(t, 250));
    assert_eq!(p.ready(ms(t, 250)), vec!["c"], "one more per prompt");
}

#[test]
fn a_lost_prompt_does_not_stall_the_queue() {
    let t = Instant::now();
    let mut p = Pacer::new(set());
    for l in ["a", "b", "c"] {
        p.push(l);
    }
    p.ready(t);
    p.ready(ms(t, 100));
    assert!(p.ready(ms(t, 2000)).is_empty());
    assert_eq!(p.next_at(), Some(ms(t, 3100)), "shut until the timeout");
    assert_eq!(
        p.ready(ms(t, 3200)),
        vec!["c"],
        "the gate opens after the timeout"
    );
}

#[test]
fn a_line_typed_too_quickly_goes_again_first() {
    let t = Instant::now();
    let mut p = Pacer::new(set());
    p.push("a");
    p.push("b");
    assert_eq!(p.ready(t), vec!["a"]);
    p.on_item(&Item::Event(Event::TooFast), ms(t, 20));
    assert_eq!(p.ready(ms(t, 100)), vec!["a"]);
    assert_eq!(p.ready(ms(t, 200)), vec!["b"]);
}

#[test]
fn a_telepath_is_sent_as_a_slash_line_and_acked() {
    let t = Instant::now();
    let mut p = Pacer::new(set());
    p.telepath("Raijin", "@invite");
    assert_eq!(p.ready(t), vec!["/Raijin @invite"]);
    assert_eq!(p.pending_telepaths(), 1);
    let gave_up = p.on_item(
        &Item::Event(Event::TelepathSent {
            who: "Raijin".into(),
        }),
        ms(t, 30),
    );
    assert!(gave_up.is_empty());
    assert_eq!(p.pending_telepaths(), 0);
}

#[test]
fn a_telepath_not_sent_is_tried_three_times_then_given_up() {
    let t = Instant::now();
    let mut p = Pacer::new(set());
    p.telepath("Raijin", "@wait");
    let mut at = t;
    let mut sent = Vec::new();
    for _ in 0..3 {
        sent.extend(p.ready(at));
        let gave_up = p.on_item(&Item::Event(Event::TelepathNotSent), ms(at, 10));
        at = ms(at, 100);
        if !gave_up.is_empty() {
            assert_eq!(gave_up, vec![("Raijin".to_owned(), "@wait".to_owned())]);
            assert_eq!(sent, vec!["/Raijin @wait"; 3]);
            assert!(p.ready(at).is_empty(), "no fourth try");
            return;
        }
        p.on_item(&prompt(), ms(at, 20));
    }
    panic!("never gave up; sent {sent:?}");
}

#[test]
fn acks_pair_with_telepaths_in_the_order_they_went() {
    let t = Instant::now();
    let mut p = Pacer::new(set());
    p.telepath("Raijin", "@wait");
    p.telepath("Bob", "@ok");
    assert_eq!(p.ready(t), vec!["/Raijin @wait"]);
    assert_eq!(p.ready(ms(t, 100)), vec!["/Bob @ok"]);
    // The first went; the second didn't, so the second is the one sent again.
    p.on_item(
        &Item::Event(Event::TelepathSent {
            who: "Raijin".into(),
        }),
        ms(t, 120),
    );
    p.on_item(&Item::Event(Event::TelepathNotSent), ms(t, 130));
    p.on_item(&prompt(), ms(t, 140));
    p.on_item(&prompt(), ms(t, 150));
    assert_eq!(p.ready(ms(t, 200)), vec!["/Bob @ok"]);
}

#[test]
fn a_telepath_to_someone_not_on_is_given_up_at_once() {
    let t = Instant::now();
    let mut p = Pacer::new(set());
    p.telepath("Raijin", "@invite");
    p.telepath("Bob", "@ok");
    p.ready(t);
    let gave_up = p.on_item(&Item::Event(Event::UserUnknown), ms(t, 10));
    assert_eq!(gave_up, vec![("Raijin".to_owned(), "@invite".to_owned())]);
    assert_eq!(p.pending_telepaths(), 0, "no ack is waited for");
    p.on_item(&prompt(), ms(t, 20));
    assert_eq!(p.ready(ms(t, 100)), vec!["/Bob @ok"]);
}
