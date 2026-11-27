//! The routine (#53, #65, #67), as pure logic: a list of rules read top-down, the first
//! that applies wins. It hands the session one goal at a time (walk, rest, camp, kill),
//! which the bot's engines carry out, so the 1.1 director can hand out the same goals.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mp_config::{Amount, GearUse, Pick, Pools, When};
use mp_engine::{
    Carried, CircuitSettings, ExpKnown, GearSettings, Goal, LairSettings, RoomBook, RoomInfo,
    RouteRules, Routine, RoutinePart, RoutineSettings, RuleSettings, Seen, TrainEnd, WalkState,
    WalkStep,
};
use mp_parse::{Rest, Vitals};

const CAMP: (u32, u32) = (1, 2150);
const ROAD: (u32, u32) = (1, 2146);
/// A room near the cavern that isn't a lair: where the bot waits for the cavern to fill.
const OUTSIDE: (u32, u32) = (1, 9156);
const CAVERN: (u32, u32) = (1, 2156);
/// Two more lairs for the order a circuit takes them in, each with its wait-room: one near
/// the camp, one far.
const NEAR: (u32, u32) = (1, 3000);
const NEAR_WAIT: (u32, u32) = (1, 3001);
const FAR: (u32, u32) = (1, 4000);
const FAR_WAIT: (u32, u32) = (1, 4001);
const RESPAWN: Duration = Duration::from_secs(240);
/// How long the routine reckons a step takes.
const STEP: Duration = Duration::from_secs(2);
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

/// Steps between rooms, either way; a pair it doesn't list has no way between.
#[derive(Debug)]
struct Steps(BTreeMap<(Room, Room), usize>);

type Room = (u32, u32);

impl RoomBook for Steps {
    fn room(&self, _map: u32, _room: u32) -> Option<RoomInfo> {
        None
    }

    fn route(
        &self,
        from: (u32, u32),
        to: (u32, u32),
        _rules: &RouteRules,
    ) -> Result<Vec<WalkStep>, String> {
        let n = if from == to {
            0
        } else {
            *self
                .0
                .get(&(from, to))
                .or_else(|| self.0.get(&(to, from)))
                .ok_or("no way")?
        };
        Ok(vec![
            WalkStep {
                dir: 0,
                to,
                door: false,
            };
            n
        ])
    }
}

/// The camp is 2 steps from the room outside the cavern and 3 from the cavern; the road is
/// 1 from the camp. The near lair's wait-room is 2 steps from the camp, the far one's 6.
fn book() -> Arc<dyn RoomBook> {
    let mut m = BTreeMap::new();
    for (a, b, n) in [
        (CAMP, OUTSIDE, 2),
        (OUTSIDE, CAVERN, 1),
        (CAMP, CAVERN, 3),
        (ROAD, CAMP, 1),
        (ROAD, OUTSIDE, 3),
        (CAMP, NEAR_WAIT, 2),
        (CAMP, FAR_WAIT, 6),
        (NEAR_WAIT, NEAR, 1),
        (FAR_WAIT, FAR, 1),
        (NEAR, FAR_WAIT, 5),
        (NEAR_WAIT, FAR_WAIT, 5),
        (FAR, NEAR_WAIT, 5),
    ] {
        m.insert((a, b), n);
    }
    Arc::new(Steps(m))
}

fn lair(at: (u32, u32), name: &str, wait_at: (u32, u32)) -> LairSettings {
    LairSettings {
        at,
        name: name.into(),
        wait_at,
        respawn: RESPAWN,
    }
}

fn cavern() -> LairSettings {
    lair(CAVERN, "Small Cavern", OUTSIDE)
}

fn circuit(lairs: Vec<LairSettings>, wait: Duration, pick: Pick) -> RuleSettings {
    RuleSettings::Circuit(CircuitSettings {
        lairs,
        when: When {
            hp_at_least: Some(Amount::Percent(80)),
            ..Default::default()
        },
        wait,
        pick,
    })
}

/// The cave bear's cavern, a circuit of one lair.
fn bear() -> RuleSettings {
    circuit(vec![cavern()], Duration::ZERO, Pick::Soonest)
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
    with_rules(rules)
}

fn with_rules(rules: Vec<RuleSettings>) -> RoutineSettings {
    RoutineSettings {
        rules,
        retry: RETRY,
        kill_wait: KILL_WAIT,
        route: Default::default(),
        rooms: Some(book()),
        step_time: STEP,
        lair_clocks: None,
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
        exp: None,
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
    /// A routine whose lairs were last cleared `ago` seconds before it starts.
    fn cleared(set: RoutineSettings, ago: &[((u32, u32), u64)]) -> Self {
        let t0 = Instant::now() + Duration::from_secs(1000);
        let clocks = ago
            .iter()
            .map(|&(lair, secs)| (lair, t0 - Duration::from_secs(secs)))
            .collect();
        Self {
            r: Routine::with_clocks(set, clocks, t0),
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
fn a_circuit_waits_for_enough_hp_then_goes() {
    let mut b = Bot::new(true);
    b.arrive(&seen(70, CAMP), 1);
    // The cavern has never been seen, so it's ready; but 70% is under the rule's 80%.
    assert_eq!(b.see(&seen(70, CAMP), 2), Goal::Camp(LOOK));
    // Ready now: straight in.
    assert_eq!(b.see(&seen(90, CAMP), 3), Goal::Walk(CAVERN));
    assert_eq!(b.r.part(), RoutinePart::ToLair("Small Cavern".into()));
}

fn with(monsters: &[&str], s: Seen) -> Seen {
    Seen {
        monsters: monsters.iter().map(|m| (*m).to_owned()).collect(),
        ..s
    }
}

#[test]
fn a_lair_is_cleared_and_visited_again_once_it_has_filled() {
    let mut b = Bot::new(true);
    b.arrive(&seen(100, CAMP), 1);
    assert_eq!(b.see(&seen(100, CAMP), 2), Goal::Walk(CAVERN));
    // It filled as the bot walked in.
    let bear = with(&["big cave bear"], seen(100, CAVERN));
    assert_eq!(b.arrive(&bear, 10), Goal::Kill("big cave bear".into()));
    assert_eq!(b.r.part(), RoutinePart::Killing("big cave bear".into()));
    let fight = Seen {
        fighting: true,
        ..bear.clone()
    };
    assert_eq!(b.see(&fight, 15), Goal::Kill("big cave bear".into()));
    // Dead: the lair is clear, its clock starts, and the camp applies again.
    assert_eq!(b.see(&seen(100, CAVERN), 40), Goal::Walk(CAMP));
    assert_eq!(b.r.take_cleared(), [(CAVERN, b.at(40))]);
    assert_eq!(b.r.take_cleared(), []);
    assert_eq!(b.arrive(&seen(100, CAMP), 45), Goal::Camp(LOOK));
    // Ready at 40 + 240. The room outside is 2 steps, 4 seconds, away, so it sets off
    // at 276 to be there as the lair fills, and waits there rather than step in early.
    assert_eq!(b.see(&seen(100, CAMP), 275), Goal::Camp(LOOK));
    assert_eq!(b.see(&seen(100, CAMP), 276), Goal::Walk(OUTSIDE));
    assert_eq!(b.r.part(), RoutinePart::ToLair("Small Cavern".into()));
    assert_eq!(b.arrive(&seen(100, OUTSIDE), 278), Goal::Wait);
    assert_eq!(b.r.part(), RoutinePart::WaitingFor("Small Cavern".into()));
    assert_eq!(b.see(&seen(100, OUTSIDE), 279), Goal::Wait);
    assert_eq!(b.see(&seen(100, OUTSIDE), 280), Goal::Walk(CAVERN));
}

#[test]
fn every_monster_in_the_lair_is_killed_before_it_counts_as_cleared() {
    let mut b = Bot::new(true);
    b.arrive(&seen(100, CAMP), 1);
    b.see(&seen(100, CAMP), 2);
    let both = with(&["cave bear", "kobold"], seen(100, CAVERN));
    assert_eq!(b.arrive(&both, 10), Goal::Kill("cave bear".into()));
    let one_left = with(&["kobold"], seen(100, CAVERN));
    assert_eq!(b.see(&one_left, 20), Goal::Kill("kobold".into()));
    assert_eq!(b.r.take_cleared(), []);
    assert_eq!(b.see(&seen(100, CAVERN), 30), Goal::Walk(CAMP));
    assert_eq!(b.r.take_cleared(), [(CAVERN, b.at(30))]);
}

#[test]
fn an_empty_lair_starts_its_clock_at_the_visit() {
    let mut b = Bot::new(true);
    b.arrive(&seen(100, CAMP), 1);
    b.see(&seen(100, CAMP), 2);
    assert_eq!(b.arrive(&seen(100, CAVERN), 10), Goal::Walk(CAMP));
    assert_eq!(b.r.take_cleared(), [(CAVERN, b.at(10))]);
    b.arrive(&seen(100, CAMP), 15);
    assert_eq!(b.see(&seen(100, CAMP), 10 + 235), Goal::Camp(LOOK));
    assert_eq!(b.see(&seen(100, CAMP), 10 + 236), Goal::Walk(OUTSIDE));
}

#[test]
fn a_kill_that_never_starts_is_given_up() {
    let mut b = Bot::new(true);
    b.arrive(&seen(100, CAMP), 1);
    b.see(&seen(100, CAMP), 2);
    let bear = with(&["cave bear"], seen(100, CAVERN));
    assert_eq!(b.arrive(&bear, 10), Goal::Kill("cave bear".into()));
    assert_eq!(b.see(&bear, 10 + 29), Goal::Kill("cave bear".into()));
    assert_eq!(b.see(&bear, 10 + 30), Goal::Walk(CAMP));
    assert_eq!(b.r.take_cleared(), [(CAVERN, b.at(40))]);
}

#[test]
fn low_hp_in_a_lair_goes_to_rest_and_the_lair_waits() {
    let mut b = Bot::new(true);
    b.arrive(&seen(100, CAMP), 1);
    b.see(&seen(100, CAMP), 2);
    let bear = with(&["cave bear"], seen(100, CAVERN));
    b.arrive(&bear, 10);
    let losing = Seen {
        fighting: true,
        ..with(&["cave bear"], seen(30, CAVERN))
    };
    // Mid-fight it stays (fleeing is #16); once the fight is off, it goes to rest.
    assert_eq!(b.see(&losing, 20), Goal::Kill("cave bear".into()));
    let off = with(&["cave bear"], seen(30, CAVERN));
    assert_eq!(b.see(&off, 21), Goal::Walk(ROAD));
    b.arrive(&seen(30, ROAD), 25);
    b.r.rest_ended(b.at(100));
    // The bear wasn't killed, so the lair's clock didn't start: rested, it goes again.
    assert_eq!(b.r.take_cleared(), []);
    assert_eq!(b.see(&seen(100, ROAD), 101), Goal::Walk(CAVERN));
}

#[test]
fn clocks_from_before_the_start_carry_on() {
    // Cleared 100 s before this run began (kept in mudparty.sqlite): ready in 140 s.
    let mut b = Bot::cleared(settings(true), &[(CAVERN, 100)]);
    assert_eq!(b.arrive(&seen(100, CAMP), 1), Goal::Camp(LOOK));
    assert_eq!(b.see(&seen(100, CAMP), 135), Goal::Camp(LOOK));
    assert_eq!(b.see(&seen(100, CAMP), 136), Goal::Walk(OUTSIDE));
}

/// A circuit of the far lair, then the near one, above the camp.
fn two_lairs(wait: Duration, pick: Pick) -> RoutineSettings {
    with_rules(vec![
        circuit(
            vec![
                lair(FAR, "Far Lair", FAR_WAIT),
                lair(NEAR, "Near Lair", NEAR_WAIT),
            ],
            wait,
            pick,
        ),
        camp(),
    ])
}

#[test]
fn the_closest_ready_lair_goes_first_and_it_plans_again_at_each() {
    let mut b = Bot::with(two_lairs(Duration::ZERO, Pick::Soonest));
    // Both ready: the near one first, though the far one is listed first.
    assert_eq!(b.see(&seen(100, CAMP), 1), Goal::Walk(NEAR));
    // Cleared; from there the far one is the one ready.
    assert_eq!(b.arrive(&seen(100, NEAR), 5), Goal::Walk(FAR));
    assert_eq!(b.arrive(&seen(100, FAR), 17), Goal::Walk(CAMP));
}

#[test]
fn a_lair_that_wont_be_ready_gives_way_to_one_that_will() {
    // The near one was cleared a minute ago; the far one is ready.
    let mut b = Bot::cleared(two_lairs(Duration::ZERO, Pick::Soonest), &[(NEAR, 60)]);
    assert_eq!(b.see(&seen(100, CAMP), 1), Goal::Walk(FAR));
}

#[test]
fn with_none_ready_it_waits_for_the_one_whose_fight_starts_soonest() {
    // From the camp the near lair's wait-room is 4 s away and the lair ready in 50 s; the
    // far one's 12 s away and ready in 30 s. The far fight can start first.
    let ago = [(NEAR, 190), (FAR, 210)];
    let mut b = Bot::cleared(two_lairs(Duration::from_secs(60), Pick::Soonest), &ago);
    assert_eq!(b.see(&seen(100, CAMP), 0), Goal::Walk(FAR_WAIT));
    assert_eq!(b.r.part(), RoutinePart::ToLair("Far Lair".into()));
    // Not willing to wait at all, it camps instead.
    let mut b = Bot::cleared(two_lairs(Duration::ZERO, Pick::Soonest), &ago);
    assert_eq!(b.see(&seen(100, CAMP), 0), Goal::Walk(CAMP));
}

#[test]
fn throughput_counts_waiting_as_free() {
    let ago = [(NEAR, 190), (FAR, 210)];
    let mut b = Bot::cleared(two_lairs(Duration::from_secs(60), Pick::Throughput), &ago);
    assert_eq!(b.see(&seen(100, CAMP), 0), Goal::Walk(NEAR_WAIT));
}

#[test]
fn a_lair_it_cant_walk_to_is_left_out() {
    let mut b = Bot::with(with_rules(vec![
        circuit(
            vec![lair((9, 1), "Nowhere", (9, 2))],
            Duration::ZERO,
            Pick::Soonest,
        ),
        camp(),
    ]));
    assert_eq!(b.see(&seen(100, CAMP), 1), Goal::Walk(CAMP));
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
        RoutinePart::ToLair("Small Cavern".into()).to_string(),
        "going to Small Cavern"
    );
    assert_eq!(
        RoutinePart::WaitingFor("Small Cavern".into()).to_string(),
        "waiting for Small Cavern to fill"
    );
    assert_eq!(
        RoutinePart::Stuck("no way".into()).to_string(),
        "stuck (no way); trying again soon"
    );
    assert_eq!(RoutinePart::ToTrainer.to_string(), "going to train");
    assert_eq!(RoutinePart::Training.to_string(), "training");
}

// ----- gear (#55)

const WEAPONS: (u32, u32) = (1, 2141);
const ARMOUR: (u32, u32) = (1, 2142);
const OTHER_ARMOUR: (u32, u32) = (1, 9999);

fn gear_rule() -> RuleSettings {
    RuleSettings::Gear {
        items: vec![
            GearSettings {
                item: "club".into(),
                how: GearUse::Wield,
                shops: vec![WEAPONS],
            },
            GearSettings {
                item: "padded vest".into(),
                how: GearUse::Wear,
                shops: vec![OTHER_ARMOUR, ARMOUR],
            },
        ],
        when: When::default(),
    }
}

/// Rest, keep the gear, camp.
fn geared() -> Bot {
    let mut set = settings(false);
    set.rules.insert(1, gear_rule());
    Bot::with(set)
}

fn item(name: &str, equipped: bool) -> Carried {
    Carried {
        name: name.into(),
        count: 1,
        equipped,
    }
}

fn club(buy: bool) -> Goal {
    Goal::Equip {
        item: "club".into(),
        how: GearUse::Wield,
        buy,
    }
}

fn vest() -> Goal {
    Goal::Equip {
        item: "padded vest".into(),
        how: GearUse::Wear,
        buy: true,
    }
}

#[test]
fn with_gear_it_checks_the_inventory_first() {
    let b = geared();
    assert_eq!(b.r.goal(), Goal::Inventory);
    assert_eq!(b.r.part(), RoutinePart::CheckingGear);
}

#[test]
fn it_buys_each_missing_item_then_goes_on_to_camp() {
    let mut b = geared();
    b.r.inventory(&[], b.at(1));
    assert_eq!(b.r.goal(), Goal::Walk(WEAPONS));
    assert_eq!(b.r.part(), RoutinePart::ToShop("club".into()));
    assert_eq!(b.arrive(&seen(100, WEAPONS), 5), club(true));
    assert_eq!(b.r.part(), RoutinePart::Equipping("club".into()));
    b.r.equip_ended(Ok(()), b.at(6));
    // It looks again rather than trusting what it did.
    assert_eq!(b.r.goal(), Goal::Inventory);
    b.r.inventory(&[item("club", true)], b.at(7));
    assert_eq!(b.r.goal(), Goal::Walk(OTHER_ARMOUR));
    b.arrive(&seen(100, OTHER_ARMOUR), 10);
    b.r.equip_ended(Ok(()), b.at(11));
    b.r.inventory(&[item("club", true), item("Padded Vest", true)], b.at(12));
    // Nothing missing: on to the camp.
    assert_eq!(b.r.goal(), Goal::Walk(CAMP));
    assert_eq!(b.arrive(&seen(100, CAMP), 20), Goal::Camp(LOOK));
}

#[test]
fn an_item_carried_but_not_in_use_is_put_on_where_it_stands() {
    let mut b = geared();
    b.r.inventory(&[item("club", false), item("padded vest", true)], b.at(1));
    assert_eq!(b.r.goal(), club(false));
}

#[test]
fn a_shop_it_cant_reach_gives_way_to_the_next() {
    let mut b = geared();
    b.r.inventory(&[item("club", true)], b.at(1));
    assert_eq!(b.r.goal(), Goal::Walk(OTHER_ARMOUR));
    b.r.walk_ended(
        &WalkState::Failed("no way from 1/2150 to 1/9999 by walking".into()),
        &seen(100, CAMP),
        b.at(2),
    );
    assert_eq!(b.r.goal(), Goal::Walk(ARMOUR));
    assert_eq!(b.arrive(&seen(100, ARMOUR), 5), vest());
}

#[test]
fn an_item_it_cant_get_is_skipped_and_said() {
    let mut b = geared();
    b.r.inventory(&[], b.at(1));
    b.arrive(&seen(100, WEAPONS), 5);
    b.r.equip_ended(Err("can't afford club".into()), b.at(6));
    assert_eq!(b.r.take_notes(), ["skipped club: can't afford club"]);
    assert!(b.r.take_notes().is_empty());
    b.r.inventory(&[], b.at(7));
    // On to the vest; the club isn't tried again.
    assert_eq!(b.r.goal(), Goal::Walk(OTHER_ARMOUR));
    b.arrive(&seen(100, OTHER_ARMOUR), 10);
    b.r.equip_ended(Ok(()), b.at(11));
    b.r.inventory(&[item("padded vest", true)], b.at(12));
    assert_eq!(b.r.goal(), Goal::Walk(CAMP));
}

#[test]
fn an_item_no_shop_sells_is_skipped() {
    let mut set = settings(false);
    set.rules.insert(
        1,
        RuleSettings::Gear {
            items: vec![GearSettings {
                item: "quest sword".into(),
                how: GearUse::Wield,
                shops: Vec::new(),
            }],
            when: When::default(),
        },
    );
    let mut b = Bot::with(set);
    b.r.inventory(&[], b.at(1));
    assert_eq!(
        b.r.take_notes(),
        ["skipped quest sword: no shop in places/ sells it"]
    );
    assert_eq!(b.r.goal(), Goal::Walk(CAMP));
}

#[test]
fn after_a_rest_it_checks_its_gear_again() {
    let mut b = geared();
    b.r.inventory(&[item("club", true), item("padded vest", true)], b.at(1));
    b.arrive(&seen(100, CAMP), 2);
    assert_eq!(b.see(&seen(40, CAMP), 3), Goal::Walk(ROAD));
    b.arrive(&seen(40, ROAD), 4);
    b.r.rest_ended(b.at(60));
    assert_eq!(b.r.goal(), Goal::Inventory);
}

#[test]
fn the_gear_parts_read_for_the_card() {
    assert_eq!(RoutinePart::CheckingGear.to_string(), "checking gear");
    assert_eq!(
        RoutinePart::ToShop("club".into()).to_string(),
        "going to get club"
    );
    assert_eq!(
        RoutinePart::Equipping("club".into()).to_string(),
        "getting club"
    );
}

// ----- training (#56)

const GUILD: (u32, u32) = (1, 2147);
const OTHER_GUILD: (u32, u32) = (1, 497);

fn train_rule(trainers: Vec<(u32, u32)>, up_to: Option<u32>) -> RuleSettings {
    RuleSettings::Train {
        trainers,
        up_to,
        when: When::default(),
    }
}

/// Train at the guild up to level 3, else camp.
fn trainee() -> Bot {
    Bot::with(with_rules(vec![train_rule(vec![GUILD], Some(3)), camp()]))
}

/// The bot at `level` with `left` experience still needed for the next, in `room`.
fn at_level(level: u32, left: i64, room: (u32, u32)) -> Seen {
    Seen {
        exp: Some(ExpKnown { level, left }),
        ..seen(100, room)
    }
}

#[test]
fn it_trains_once_it_has_the_experience_and_stops_at_its_level() {
    let mut b = trainee();
    // Its experience isn't known yet: it camps, and the session asks for `exp`.
    assert!(b.r.wants_exp());
    assert_eq!(b.see(&seen(100, CAMP), 1), Goal::Walk(CAMP));
    b.arrive(&seen(100, CAMP), 2);
    // 500 short of level 2: not yet.
    assert_eq!(b.see(&at_level(1, 500, CAMP), 3), Goal::Camp(LOOK));
    // Enough: to the guild.
    assert_eq!(b.see(&at_level(1, 0, CAMP), 4), Goal::Walk(GUILD));
    assert_eq!(b.r.part(), RoutinePart::ToTrainer);
    assert_eq!(b.arrive(&at_level(1, 0, GUILD), 10), Goal::Train);
    assert_eq!(b.r.part(), RoutinePart::Training);
    b.r.train_ended(TrainEnd::Trained(2), b.at(11));
    assert_eq!(b.r.take_notes(), ["trained to level 2"]);
    // It waits where it is for the new level's experience, rather than walk off.
    assert_eq!(b.r.goal(), Goal::Wait);
    assert_eq!(b.see(&seen(100, GUILD), 12), Goal::Wait);
    // Enough for level 3 too: it trains again where it stands.
    assert_eq!(b.see(&at_level(2, 0, GUILD), 13), Goal::Train);
    b.r.train_ended(TrainEnd::Trained(3), b.at(14));
    b.r.take_notes();
    // Level 3 is as far as it goes, enough experience or not: back to camp.
    assert_eq!(b.see(&at_level(3, 0, GUILD), 15), Goal::Walk(CAMP));
    assert_eq!(b.arrive(&at_level(3, 0, CAMP), 20), Goal::Camp(LOOK));
    assert_eq!(b.see(&at_level(3, 0, CAMP), 600), Goal::Camp(LOOK));
}

#[test]
fn without_a_cap_it_trains_whenever_it_can() {
    let mut b = Bot::with(with_rules(vec![train_rule(vec![GUILD], None), camp()]));
    assert_eq!(b.see(&at_level(40, 0, CAMP), 1), Goal::Walk(GUILD));
}

#[test]
fn no_money_for_training_is_said_and_tried_again_later() {
    let mut b = trainee();
    b.see(&at_level(1, 0, CAMP), 1);
    b.arrive(&at_level(1, 0, GUILD), 5);
    b.r.train_ended(TrainEnd::NoMoney, b.at(6));
    let notes = b.r.take_notes();
    assert!(notes[0].contains("money"), "{notes:?}");
    assert_eq!(b.r.goal(), Goal::Walk(CAMP));
    b.arrive(&at_level(1, 0, CAMP), 10);
    // Five minutes on, it tries again.
    assert_eq!(b.see(&at_level(1, 0, CAMP), 6 + 299), Goal::Camp(LOOK));
    assert_eq!(b.see(&at_level(1, 0, CAMP), 6 + 300), Goal::Walk(GUILD));
}

#[test]
fn a_trainer_that_wont_train_it_gives_way_to_the_next() {
    let mut b = Bot::with(with_rules(vec![
        train_rule(vec![OTHER_GUILD, GUILD], Some(3)),
        camp(),
    ]));
    assert_eq!(b.see(&at_level(1, 0, CAMP), 1), Goal::Walk(OTHER_GUILD));
    b.arrive(&at_level(1, 0, OTHER_GUILD), 5);
    b.r.train_ended(
        TrainEnd::Refused("This shop is not suitable for your training.".into()),
        b.at(6),
    );
    assert_eq!(b.r.goal(), Goal::Walk(GUILD));
    b.arrive(&at_level(1, 0, GUILD), 10);
    b.r.train_ended(
        TrainEnd::Refused("You have progressed too far to use the training provided here.".into()),
        b.at(11),
    );
    // None will: said, and not tried again until the bot restarts.
    let notes = b.r.take_notes();
    assert!(notes.iter().any(|n| n.contains("no trainer")), "{notes:?}");
    assert_eq!(b.r.goal(), Goal::Walk(CAMP));
    b.arrive(&at_level(1, 0, CAMP), 15);
    assert_eq!(b.see(&at_level(1, 0, CAMP), 5000), Goal::Camp(LOOK));
}

#[test]
fn no_trainer_at_all_never_applies() {
    let mut b = Bot::with(with_rules(vec![train_rule(Vec::new(), Some(3)), camp()]));
    assert_eq!(b.see(&at_level(1, 0, CAMP), 1), Goal::Walk(CAMP));
}
