//! The walker (#52), as pure logic: it finds where it is with `/room`, walks a planned
//! route a step at a time, confirming each room, opens doors in the way, re-plans when it
//! lands somewhere else, waits out a fight, and gives up on an exit the game refuses.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mp_engine::{RoomBook, RoomInfo, RouteRules, WalkState, WalkStep, Walker};
use mp_parse::{Event, Item};

/// A line of rooms 1/1 – 1/2 – 1/3 (east), with a door between 1/2 and 1/3, and 1/9 off
/// to the side of 1/1 (north), from where the way back is south. With 1/1's east exit
/// refused, the way from 1/1 goes north by 1/9 and on east to 1/3. Every rule set a plan
/// was made with is kept, to see what the walker asked for.
#[derive(Debug)]
struct Map(HashMap<(u32, u32), Vec<WalkStep>>, Mutex<Vec<RouteRules>>);

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
    fn route(
        &self,
        from: (u32, u32),
        to: (u32, u32),
        rules: &RouteRules,
    ) -> Result<Vec<WalkStep>, String> {
        self.1.lock().unwrap().push(rules.clone());
        let none = || {
            format!(
                "no way from {}/{} to {}/{} by walking",
                from.0, from.1, to.0, to.1
            )
        };
        if to != (1, 3) || rules.refused.contains(&((from.0, from.1), 2)) && from != (1, 1) {
            return Err(none());
        }
        if from == (1, 1) && rules.refused.contains(&((1, 1), 2)) {
            return Ok(vec![step(0, 9, false), step(2, 3, false)]);
        }
        self.0.get(&from).cloned().ok_or_else(none)
    }
}

fn map() -> Arc<Map> {
    Arc::new(Map(
        HashMap::from([
            ((1, 1), vec![step(2, 2, false), step(2, 3, true)]),
            ((1, 2), vec![step(2, 3, true)]),
            (
                (1, 9),
                vec![step(1, 1, false), step(2, 2, false), step(2, 3, true)],
            ),
        ]),
        Mutex::new(Vec::new()),
    ))
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
fn an_exit_the_game_refuses_with_no_way_round_ends_the_walk() {
    let mut w = Walk::to(3);
    w.take();
    w.at(2).hear(Event::ExitRefused {
        text: "You have progressed too far to go through this exit!".into(),
    });
    assert_eq!(
        w.w.state(),
        WalkState::Failed("You have progressed too far to go through this exit!".into())
    );
}

#[test]
fn an_exit_the_game_refuses_is_planned_around() {
    let mut w = Walk::to(3);
    w.take();
    w.at(1);
    assert_eq!(w.take(), ["east"]);
    w.hear(Event::ExitRefused {
        text: "You have not progressed far enough to go through this exit!".into(),
    });
    // East from 1/1 is barred: north by 1/9 instead.
    assert_eq!(w.take(), ["north"]);
    assert_eq!(w.w.state(), WalkState::Walking { step: 1, of: 2 });
    w.arrive().at(9);
    assert_eq!(w.take(), ["/room", "east"]);
    w.arrive().at(3);
    assert_eq!(w.w.state(), WalkState::Arrived);
}

#[test]
fn every_plan_of_a_walk_keeps_its_rules() {
    let map = map();
    let book: Arc<dyn RoomBook> = map.clone();
    let rules = RouteRules {
        avoid_rooms: [(1, 7)].into(),
        ..Default::default()
    };
    let now = Instant::now();
    let mut w = Walker::new((1, 3), book, STEP_WAIT).with_rules(rules);
    w.start(now);
    w.on_item(&Item::Event(Event::RoomKey { map: 1, room: 1 }), now, false);
    w.on_item(&Item::Event(Event::ObviousExits), now, false);
    // A wrong room: the walk plans again from 1/9.
    w.on_item(&Item::Event(Event::RoomKey { map: 1, room: 9 }), now, false);
    let asked = map.1.lock().unwrap();
    assert_eq!(asked.len(), 2);
    assert!(asked.iter().all(|r| r.avoid_rooms.contains(&(1, 7))));
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

// A step the game takes no notice of, again and again, though `/room` answers each time
// (#78): the bot can't move, and the walk gives up rather than try for ever.
#[test]
fn a_step_that_never_goes_through_gives_up_though_the_room_answers() {
    let mut w = Walk::to(3);
    w.take();
    w.at(1);
    assert_eq!(w.take(), ["east"]);
    let mut t = w.now;
    for _ in 0..3 {
        t += STEP_WAIT + Duration::from_secs(1);
        assert_eq!(w.w.on_tick(t, false), ["/room"]);
        // Still in 1/1: it tries the step again.
        assert_eq!(
            w.w.on_item(&Item::Event(Event::RoomKey { map: 1, room: 1 }), t, false),
            ["east"]
        );
    }
    t += STEP_WAIT + Duration::from_secs(1);
    assert!(w.w.on_tick(t, false).is_empty());
    assert!(matches!(w.w.state(), WalkState::Failed(why) if why.contains("answer")));
}

// HP at 0 or below: silvermere refuses every move (#78). The walk ends saying so.
#[test]
fn a_mortally_wounded_bot_cant_walk() {
    let mut w = Walk::to(3);
    w.take();
    w.at(1);
    w.take();
    w.hear(Event::MortallyWounded);
    assert!(
        matches!(w.w.state(), WalkState::Failed(why) if why.contains("mortally wounded")),
        "{:?}",
        w.w.state()
    );
    assert!(w.take().is_empty());
}
