//! The walker (#52), as pure logic: it finds where it is with `/room`, walks a planned
//! route a step at a time, confirming each room, opens doors in the way, re-plans when it
//! lands somewhere else, waits out a fight, and gives up on an exit the game refuses.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mp_engine::{RoomBook, RoomInfo, WalkState, WalkStep, Walker};
use mp_parse::{Event, Item};

/// A line of rooms 1/1 – 1/2 – 1/3 (east), with a door between 1/2 and 1/3, and 1/9 off
/// to the side of 1/1 (north), from where the way back is south.
#[derive(Debug)]
struct Map(HashMap<(u32, u32), Vec<WalkStep>>);

fn step(dir: u8, room: u32, door: bool) -> WalkStep {
    WalkStep {
        dir,
        to: (1, room),
        door,
    }
}

impl RoomBook for Map {
    fn room(&self, _: u32, _: u32) -> Option<RoomInfo> {
        None
    }
    fn route(&self, from: (u32, u32), to: (u32, u32)) -> Option<Vec<WalkStep>> {
        if to != (1, 3) {
            return None;
        }
        self.0.get(&from).cloned()
    }
}

fn map() -> Arc<Map> {
    Arc::new(Map(HashMap::from([
        ((1, 1), vec![step(2, 2, false), step(2, 3, true)]),
        ((1, 2), vec![step(2, 3, true)]),
        (
            (1, 9),
            vec![step(1, 1, false), step(2, 2, false), step(2, 3, true)],
        ),
    ])))
}

const STEP_WAIT: Duration = Duration::from_secs(10);

struct Walk {
    w: Walker,
    now: Instant,
    sent: Vec<String>,
}

impl Walk {
    fn to(room: u32) -> Self {
        let now = Instant::now();
        let mut w = Walker::new((1, room), map(), STEP_WAIT);
        let sent = w.start(now);
        Self { w, now, sent }
    }
    fn hear(&mut self, e: Event) -> &mut Self {
        let out = self.w.on_item(&Item::Event(e), self.now, false);
        self.sent.extend(out);
        self
    }
    fn at(&mut self, room: u32) -> &mut Self {
        self.hear(Event::RoomKey { map: 1, room })
    }
    fn arrive(&mut self) -> &mut Self {
        self.hear(Event::ObviousExits)
    }
    fn take(&mut self) -> Vec<String> {
        std::mem::take(&mut self.sent)
    }
}

#[test]
fn it_finds_where_it_is_then_walks_and_confirms_each_room() {
    let mut w = Walk::to(3);
    assert_eq!(w.take(), ["/room"]);
    assert_eq!(w.w.state(), WalkState::Locating);
    w.at(1);
    assert_eq!(w.take(), ["east"]);
    assert_eq!(w.w.state(), WalkState::Walking { step: 1, of: 2 });
    w.arrive();
    assert_eq!(w.take(), ["/room"]);
    w.at(2);
    assert_eq!(w.take(), ["east"]);
    w.arrive().at(3);
    assert_eq!(w.w.state(), WalkState::Arrived);
    assert!(w.take().contains(&"/room".to_owned()));
}

#[test]
fn already_there_is_arrived() {
    let mut w = Walk::to(3);
    w.take();
    w.at(3);
    assert_eq!(w.w.state(), WalkState::Arrived);
    assert!(w.take().is_empty());
}

#[test]
fn a_closed_door_is_opened_and_the_step_tried_again() {
    let mut w = Walk::to(3);
    w.take();
    w.at(2);
    assert_eq!(w.take(), ["east"]);
    w.hear(Event::DoorClosed);
    assert_eq!(w.take(), ["open east"]);
    w.hear(Event::DoorOpened);
    assert_eq!(w.take(), ["east"]);
    w.arrive().at(3);
    assert_eq!(w.w.state(), WalkState::Arrived);
}

#[test]
fn a_locked_door_is_bashed_open() {
    let mut w = Walk::to(3);
    w.take();
    w.at(2).hear(Event::DoorClosed).hear(Event::DoorLocked);
    assert_eq!(w.take(), ["east", "open east", "bash east"]);
    // Strength against a roll: it may take a few tries.
    w.hear(Event::BashFailed);
    assert_eq!(w.take(), ["bash east"]);
    w.hear(Event::BashedOpen);
    assert_eq!(w.take(), ["east"]);
    w.arrive().at(3);
    assert_eq!(w.w.state(), WalkState::Arrived);
}

#[test]
fn a_door_that_wont_break_ends_the_walk() {
    let mut w = Walk::to(3);
    w.take();
    w.at(2).hear(Event::DoorClosed).hear(Event::DoorLocked);
    for _ in 0..8 {
        w.hear(Event::BashFailed);
    }
    assert!(matches!(w.w.state(), WalkState::Failed(why) if why.contains("locked")));
}

#[test]
fn landing_somewhere_else_plans_again_from_there() {
    let mut w = Walk::to(3);
    w.take();
    w.at(1);
    w.take();
    // East from 1/1 was meant to reach 1/2; the game put the bot in 1/9.
    w.arrive().at(9);
    assert_eq!(w.take(), ["/room", "south"]);
    assert_eq!(w.w.state(), WalkState::Walking { step: 1, of: 3 });
}

#[test]
fn no_exit_finds_where_it_is_again() {
    let mut w = Walk::to(3);
    w.take();
    w.at(1);
    w.take();
    w.hear(Event::NoExit);
    assert_eq!(w.take(), ["/room"]);
    assert_eq!(w.w.state(), WalkState::Locating);
}

#[test]
fn an_exit_the_game_refuses_ends_the_walk() {
    let mut w = Walk::to(3);
    w.take();
    w.at(1).hear(Event::ExitRefused {
        text: "You have progressed too far to go through this exit!".into(),
    });
    assert_eq!(
        w.w.state(),
        WalkState::Failed("You have progressed too far to go through this exit!".into())
    );
}

#[test]
fn no_route_ends_the_walk() {
    let mut w = Walk::to(4);
    w.take();
    w.at(1);
    assert!(matches!(w.w.state(), WalkState::Failed(why) if why.contains("no way")));
}

#[test]
fn a_fight_pauses_the_walk_and_it_goes_on_after() {
    let mut w = Walk::to(3);
    w.take();
    w.at(1);
    w.take();
    // Attacked on the way: nothing goes while fighting.
    let out = w.w.on_item(&Item::Event(Event::CombatEngaged), w.now, true);
    assert!(out.is_empty());
    assert_eq!(w.w.state(), WalkState::Paused);
    assert!(
        w.w.on_tick(w.now + Duration::from_secs(30), true)
            .is_empty()
    );
    // Over: it finds where it is, and goes on.
    assert_eq!(
        w.w.on_tick(w.now + Duration::from_secs(31), false),
        ["/room"]
    );
    w.at(1);
    assert_eq!(w.take(), ["east"]);
}

#[test]
fn a_step_with_no_answer_finds_where_it_is_again_then_gives_up() {
    let mut w = Walk::to(3);
    w.take();
    w.at(1);
    w.take();
    let mut t = w.now;
    for _ in 0..3 {
        t += STEP_WAIT + Duration::from_secs(1);
        assert_eq!(w.w.on_tick(t, false), ["/room"]);
    }
    t += STEP_WAIT + Duration::from_secs(1);
    assert!(w.w.on_tick(t, false).is_empty());
    assert!(matches!(w.w.state(), WalkState::Failed(why) if why.contains("answer")));
}
