//! The routine (#53, #65), as pure logic: a list of rules read top-down, the first that
//! applies wins. It hands the session one goal at a time (walk, rest, camp, kill), which the
//! bot's engines carry out, so the 1.1 director can hand out the same goals.

use std::time::{Duration, Instant};

use mp_config::{Amount, Pools, When};
use mp_engine::{
    Goal, HuntSettings, Routine, RoutinePart, RoutineSettings, RuleSettings, Seen, WalkState,
};
use mp_parse::{Rest, Vitals};

const CAMP: (u32, u32) = (1, 2150);
const ROAD: (u32, u32) = (1, 2146);
const CAVERN: (u32, u32) = (1, 2156);
const EVERY: Duration = Duration::from_secs(240);
const RETRY: Duration = Duration::from_secs(30);
const KILL_WAIT: Duration = Duration::from_secs(30);
const LOOK: Duration = Duration::from_secs(5);
const UNTIL: Pools = Pools {
    hp: Some(Amount::Percent(100)),
    mp: None,
};

fn rest_rule(when: When) -> RuleSettings {
    RuleSettings::Rest {
        at: ROAD,
        when,
        until: UNTIL,
    }
}

fn hp_below(p: i32) -> When {
    When {
        hp_below: Some(Amount::Percent(p)),
        ..Default::default()
    }
}

fn bear() -> RuleSettings {
    RuleSettings::Hunt(HuntSettings {
        monster: "cave bear".into(),
        at: CAVERN,
        every: EVERY,
        when: When {
            hp_at_least: Some(Amount::Percent(80)),
            ..Default::default()
        },
    })
}

fn camp() -> RuleSettings {
    RuleSettings::Camp {
        at: CAMP,
        look_every: LOOK,
    }
}

/// The Newhaven arena: rest below half HP, hunt the bear, camp.
fn settings(hunts: bool) -> RoutineSettings {
    let mut rules = vec![rest_rule(hp_below(50))];
    if hunts {
        rules.push(bear());
    }
    rules.push(camp());
    RoutineSettings {
        rules,
        retry: RETRY,
        kill_wait: KILL_WAIT,
        route: Default::default(),
    }
}

fn hp(hp: i32) -> Vitals {
    Vitals {
        hp,
        max_hp: Some(100),
        mp: None,
        max_mp: None,
        rest: Rest::Standing,
    }
}

/// What the bot looks like: `hp` of 100, in `room`, not fighting, nothing here.
fn seen(hp_now: i32, room: (u32, u32)) -> Seen {
    Seen {
        vitals: Some(hp(hp_now)),
        fighting: false,
        monsters: Vec::new(),
        room: Some(room),
        walking: false,
    }
}

struct Bot {
    r: Routine,
    t0: Instant,
}

impl Bot {
    fn with(set: RoutineSettings) -> Self {
        let t0 = Instant::now();
        Self {
            r: Routine::new(set, t0),
            t0,
        }
    }
    fn new(hunts: bool) -> Self {
        Self::with(settings(hunts))
    }
    fn at(&self, secs: u64) -> Instant {
        self.t0 + Duration::from_secs(secs)
    }
    fn see(&mut self, s: &Seen, secs: u64) -> Goal {
        let now = self.at(secs);
        self.r.update(s, now);
        self.r.goal()
    }
    fn arrive(&mut self, s: &Seen, secs: u64) -> Goal {
        let now = self.at(secs);
        self.r.walk_ended(&WalkState::Arrived, s, now);
        self.r.goal()
    }
}

#[test]
fn it_starts_by_walking_to_the_camp_and_camps_there() {
    let mut b = Bot::new(false);
    assert_eq!(b.r.goal(), Goal::Walk(CAMP));
    assert_eq!(b.r.part(), RoutinePart::ToCamp);
    assert_eq!(b.arrive(&seen(100, CAMP), 1), Goal::Camp(LOOK));
    assert_eq!(b.r.part(), RoutinePart::Camping);
    // Nothing to do at full HP: it stays.
    assert_eq!(b.see(&seen(100, CAMP), 2), Goal::Camp(LOOK));
}

#[test]
fn low_hp_in_the_camp_sends_it_to_rest_and_back() {
    let mut b = Bot::new(false);
    b.arrive(&seen(100, CAMP), 1);
    assert_eq!(b.see(&seen(40, CAMP), 2), Goal::Walk(ROAD));
    assert_eq!(b.r.part(), RoutinePart::ToRest);
    // It rests until the rule's `until`.
    assert_eq!(b.arrive(&seen(40, ROAD), 3), Goal::Rest(UNTIL));
    assert_eq!(b.r.part(), RoutinePart::Resting);
    // The rest engine rests to its stop thresholds; the routine waits for it.
    assert_eq!(b.see(&seen(70, ROAD), 60), Goal::Rest(UNTIL));
    b.r.rest_ended(b.at(90));
    // Rested: the camp applies again, and it isn't there.
    assert_eq!(b.r.goal(), Goal::Walk(CAMP));
    assert_eq!(b.arrive(&seen(100, CAMP), 91), Goal::Camp(LOOK));
}

#[test]
fn a_fight_is_finished_before_a_rule_above_takes_over() {
    let mut b = Bot::new(false);
    b.arrive(&seen(100, CAMP), 1);
    let fighting = Seen {
        fighting: true,
        ..seen(40, CAMP)
    };
    assert_eq!(b.see(&fighting, 2), Goal::Camp(LOOK));
    assert_eq!(b.see(&seen(35, CAMP), 3), Goal::Walk(ROAD));
}

#[test]
fn mp_counts_as_well_as_hp() {
    let mut set = settings(false);
    set.rules[0] = rest_rule(When {
        mp_below: Some(Amount::Percent(30)),
        ..Default::default()
    });
    let mut b = Bot::with(set);
    b.arrive(&seen(100, CAMP), 0);
    let low_mp = Seen {
        vitals: Some(Vitals {
            mp: Some(10),
            max_mp: Some(50),
            ..hp(100)
        }),
        ..seen(100, CAMP)
    };
    assert_eq!(b.see(&low_mp, 1), Goal::Walk(ROAD));
}

#[test]
fn the_first_rule_that_applies_wins() {
    // Two rests: below 30% at the road, below 60% at the camp's own room.
    let mut set = settings(false);
    set.rules = vec![
        rest_rule(hp_below(30)),
        RuleSettings::Rest {
            at: CAMP,
            when: hp_below(60),
            until: UNTIL,
        },
        camp(),
    ];
    let mut b = Bot::with(set);
    b.arrive(&seen(100, CAMP), 0);
    // 50%: only the second applies; it rests where it stands.
    assert_eq!(b.see(&seen(50, CAMP), 1), Goal::Walk(CAMP));
    assert_eq!(b.arrive(&seen(50, CAMP), 2), Goal::Rest(UNTIL));
    // Down to 20% while resting (attacked): the first, above it, takes over.
    assert_eq!(b.see(&seen(20, CAMP), 3), Goal::Walk(ROAD));
}

#[test]
fn a_hunt_waits_for_enough_hp_then_goes() {
    let mut b = Bot::new(true);
    b.arrive(&seen(70, CAMP), 1);
    // Due from the start, but 70% is under the hunt's 80%.
    assert_eq!(b.see(&seen(70, CAMP), 2), Goal::Camp(LOOK));
    assert_eq!(b.see(&seen(90, CAMP), 3), Goal::Walk(CAVERN));
    assert_eq!(b.r.part(), RoutinePart::Hunting("cave bear".into()));
}

#[test]
fn a_hunt_kills_the_monster_and_goes_back_until_it_is_due_again() {
    let mut b = Bot::new(true);
    b.arrive(&seen(100, CAMP), 1);
    assert_eq!(b.see(&seen(100, CAMP), 2), Goal::Walk(CAVERN));
    // The cavern's display named a big cave bear: that one is the target.
    let bear = Seen {
        monsters: vec!["big cave bear".into()],
        ..seen(100, CAVERN)
    };
    assert_eq!(b.arrive(&bear, 10), Goal::Kill("big cave bear".into()));
    assert_eq!(b.r.part(), RoutinePart::Killing("big cave bear".into()));
    let fight = Seen {
        fighting: true,
        ..bear.clone()
    };
    assert_eq!(b.see(&fight, 15), Goal::Kill("big cave bear".into()));
    // Dead: gone from the room, and the fight over. The camp applies again.
    assert_eq!(b.see(&seen(60, CAVERN), 40), Goal::Walk(CAMP));
    assert_eq!(b.arrive(&seen(60, CAMP), 45), Goal::Camp(LOOK));
    // Due again four minutes after the kill, not before.
    assert_eq!(b.see(&seen(100, CAMP), 40 + 239), Goal::Camp(LOOK));
    assert_eq!(b.see(&seen(100, CAMP), 40 + 240), Goal::Walk(CAVERN));
}

#[test]
fn a_monster_that_isnt_there_is_looked_for_again_later() {
    let mut b = Bot::new(true);
    b.arrive(&seen(100, CAMP), 1);
    b.see(&seen(100, CAMP), 2);
    assert_eq!(b.arrive(&seen(100, CAVERN), 10), Goal::Walk(CAMP));
    b.arrive(&seen(100, CAMP), 15);
    assert_eq!(b.see(&seen(100, CAMP), 10 + 239), Goal::Camp(LOOK));
    assert_eq!(b.see(&seen(100, CAMP), 10 + 240), Goal::Walk(CAVERN));
}

#[test]
fn a_kill_that_never_starts_is_given_up() {
    let mut b = Bot::new(true);
    b.arrive(&seen(100, CAMP), 1);
    b.see(&seen(100, CAMP), 2);
    let bear = Seen {
        monsters: vec!["cave bear".into()],
        ..seen(100, CAVERN)
    };
    assert_eq!(b.arrive(&bear, 10), Goal::Kill("cave bear".into()));
    assert_eq!(b.see(&bear, 10 + 29), Goal::Kill("cave bear".into()));
    assert_eq!(b.see(&bear, 10 + 30), Goal::Walk(CAMP));
}

#[test]
fn low_hp_on_a_hunt_goes_to_rest_and_the_hunt_waits() {
    let mut b = Bot::new(true);
    b.arrive(&seen(100, CAMP), 1);
    b.see(&seen(100, CAMP), 2);
    let bear = Seen {
        monsters: vec!["cave bear".into()],
        ..seen(100, CAVERN)
    };
    b.arrive(&bear, 10);
    let losing = Seen {
        fighting: true,
        monsters: vec!["cave bear".into()],
        ..seen(30, CAVERN)
    };
    // Mid-fight it stays (fleeing is #16); once the fight is off, it goes to rest.
    assert_eq!(b.see(&losing, 20), Goal::Kill("cave bear".into()));
    let off = Seen {
        monsters: vec!["cave bear".into()],
        ..seen(30, CAVERN)
    };
    assert_eq!(b.see(&off, 21), Goal::Walk(ROAD));
    b.arrive(&seen(30, ROAD), 25);
    b.r.rest_ended(b.at(100));
    // The bear wasn't killed, so the hunt is still due: rested, it goes again.
    let rested = seen(100, ROAD);
    assert_eq!(b.see(&rested, 101), Goal::Walk(CAVERN));
}

#[test]
fn a_failed_walk_waits_then_tries_again() {
    let mut b = Bot::new(false);
    let now = b.at(5);
    b.r.walk_ended(
        &WalkState::Failed("the game gave no answer".into()),
        &seen(100, ROAD),
        now,
    );
    assert_eq!(b.r.goal(), Goal::Wait);
    assert_eq!(
        b.r.part(),
        RoutinePart::Stuck("the game gave no answer".into())
    );
    assert_eq!(b.see(&seen(100, ROAD), 5 + 29), Goal::Wait);
    assert_eq!(b.see(&seen(100, ROAD), 5 + 30), Goal::Walk(CAMP));
}

#[test]
fn a_bot_found_away_from_its_camp_walks_back() {
    let mut b = Bot::new(false);
    b.arrive(&seen(100, CAMP), 1);
    // Walking somewhere by hand: left alone.
    let walking = Seen {
        walking: true,
        ..seen(100, ROAD)
    };
    assert_eq!(b.see(&walking, 2), Goal::Camp(LOOK));
    assert_eq!(b.see(&seen(100, ROAD), 3), Goal::Walk(CAMP));
}

#[test]
fn a_paused_routine_waits_and_picks_up_where_it_was() {
    let mut b = Bot::new(false);
    b.arrive(&seen(100, CAMP), 1);
    b.see(&seen(40, CAMP), 2);
    b.r.set_paused(true);
    assert_eq!(b.r.goal(), Goal::Wait);
    assert_eq!(b.r.part(), RoutinePart::Paused);
    assert_eq!(b.see(&seen(40, CAMP), 3), Goal::Wait);
    b.r.set_paused(false);
    assert_eq!(b.r.goal(), Goal::Walk(ROAD));
}

#[test]
fn each_part_reads_for_the_card() {
    assert_eq!(RoutinePart::Camping.to_string(), "camping");
    assert_eq!(RoutinePart::ToRest.to_string(), "going to rest");
    assert_eq!(
        RoutinePart::Hunting("cave bear".into()).to_string(),
        "hunting cave bear"
    );
    assert_eq!(
        RoutinePart::Stuck("no way".into()).to_string(),
        "stuck (no way); trying again soon"
    );
}
