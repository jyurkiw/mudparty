//! Routes between rooms over the game data's exits (#52): the shortest walk, through doors
//! and map changes, and never through an exit a walker can't pass by walking. With rules
//! (#64): rooms and exit kinds avoided, exits the game refused, and rooms that cost more.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use mp_data::{GameData, RouteError, RouteRules, Step, import};
use silvermere_db::TestDb;
use silvermere_engine::world::blank;
use silvermere_types::enums::ExitType;
use silvermere_types::ids::{RoomId, RoomKey};
use silvermere_types::records::Exit;

// Exit slots, silvermere's DIR_ order.
const N: usize = 0;
const S: usize = 1;
const E: usize = 2;
const W: usize = 3;
const U: usize = 8;
const D: usize = 9;

fn exit(kind: ExitType, target: u32, par1: u32) -> Exit {
    Exit {
        target: RoomId(target),
        kind,
        par1,
        par2: 0,
        par3: 0,
        par4: 0,
    }
}

/// A small world:
///
/// ```text
///   1/2 Street ── east ── 1/6 Lane
///      │ south              │ south
///   1/1 Square ── east, a door ── 1/3 Shop
///      │  up: hidden 1/4 Loft
///      │  down: a key exit to 1/5 Vault
///      └ south: a map change to 2/1 Far Room
/// ```
async fn world() -> (TestDb, GameData, tempfile::TempDir) {
    let t = TestDb::sqlite_temp().await.unwrap();
    let db = t.db();
    let mut sq = blank::clear_room(RoomKey::new(1, 1));
    sq.name = "Square".into();
    sq.exits[N] = exit(ExitType::Normal, 2, 0);
    sq.exits[E] = exit(ExitType::Door, 3, 0);
    sq.exits[U] = exit(ExitType::Hidden, 4, 0);
    sq.exits[D] = exit(ExitType::Key, 5, 0);
    sq.exits[S] = exit(ExitType::MapChange, 1, 2);
    db.rooms().insert(&sq).await.unwrap();
    let mut st = blank::clear_room(RoomKey::new(1, 2));
    st.name = "Street".into();
    st.exits[S] = exit(ExitType::Normal, 1, 0);
    st.exits[E] = exit(ExitType::Normal, 6, 0);
    db.rooms().insert(&st).await.unwrap();
    let mut shop = blank::clear_room(RoomKey::new(1, 3));
    shop.name = "Shop".into();
    shop.exits[W] = exit(ExitType::Door, 1, 0);
    shop.exits[N] = exit(ExitType::Normal, 6, 0);
    db.rooms().insert(&shop).await.unwrap();
    let mut lane = blank::clear_room(RoomKey::new(1, 6));
    lane.name = "Lane".into();
    lane.exits[W] = exit(ExitType::Normal, 2, 0);
    lane.exits[S] = exit(ExitType::Normal, 3, 0);
    db.rooms().insert(&lane).await.unwrap();
    for (key, name) in [((1, 4), "Loft"), ((1, 5), "Vault"), ((2, 1), "Far Room")] {
        let mut r = blank::clear_room(RoomKey::new(key.0, key.1));
        r.name = name.into();
        db.rooms().insert(&r).await.unwrap();
    }
    let url = db.url();
    let path = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))
        .unwrap();
    let path = PathBuf::from(path.split('?').next().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("gamedata.sqlite");
    import(&path, &out).unwrap();
    (t, GameData::open(&out).unwrap(), dir)
}

fn step(dir: u8, map: i64, room: i64, door: bool) -> Step {
    Step {
        dir,
        to_map: map,
        to_room: room,
        door,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_shortest_walk_goes_through_doors() {
    let (_t, gd, _dir) = world().await;
    assert_eq!(
        gd.route((1, 2), (1, 3)).unwrap(),
        Some(vec![step(1, 1, 1, false), step(2, 1, 3, true)])
    );
    assert_eq!(gd.route((1, 3), (1, 2)).unwrap().map(|r| r.len()), Some(2));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_map_change_is_walked_like_any_exit() {
    let (_t, gd, _dir) = world().await;
    assert_eq!(
        gd.route((1, 1), (2, 1)).unwrap(),
        Some(vec![step(1, 2, 1, false)])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn hidden_and_key_exits_are_not_walked() {
    let (_t, gd, _dir) = world().await;
    assert_eq!(gd.route((1, 1), (1, 4)).unwrap(), None);
    assert_eq!(gd.route((1, 1), (1, 5)).unwrap(), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn already_there_is_no_steps() {
    let (_t, gd, _dir) = world().await;
    assert_eq!(gd.route((1, 2), (1, 2)).unwrap(), Some(vec![]));
}

// ----- rules (#64)

fn rooms(keys: &[(i64, i64)]) -> HashSet<(i64, i64)> {
    keys.iter().copied().collect()
}

/// The rooms a route passes, in order.
fn through(steps: &[Step]) -> Vec<(i64, i64)> {
    steps.iter().map(|s| (s.to_map, s.to_room)).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn an_avoided_exit_kind_is_walked_around() {
    let (_t, gd, _dir) = world().await;
    // The door from the Square is the short way; without doors it's round by the Lane.
    let rules = RouteRules {
        avoid_kinds: HashSet::from([7]),
        ..Default::default()
    };
    let r = gd.route_with((1, 1), (1, 3), &rules).unwrap();
    assert_eq!(through(&r), [(1, 2), (1, 6), (1, 3)]);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_avoided_room_is_walked_around_and_said_when_it_cant_be() {
    let (_t, gd, _dir) = world().await;
    let mut rules = RouteRules {
        avoid_rooms: rooms(&[(1, 1)]),
        ..Default::default()
    };
    // Street to Shop: round by the Lane, not through the Square.
    let r = gd.route_with((1, 2), (1, 3), &rules).unwrap();
    assert_eq!(through(&r), [(1, 6), (1, 3)]);
    // With the Lane avoided too, there's no way; the error names what's in the way.
    rules.avoid_rooms.insert((1, 6));
    match gd.route_with((1, 2), (1, 3), &rules) {
        Err(RouteError::NoWay(why)) => {
            assert!(why.contains("avoided"), "{why}");
            assert!(why.contains("Square") || why.contains("Lane"), "{why}");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_avoided_destination_is_refused() {
    let (_t, gd, _dir) = world().await;
    let rules = RouteRules {
        avoid_rooms: rooms(&[(1, 3)]),
        ..Default::default()
    };
    match gd.route_with((1, 1), (1, 3), &rules) {
        Err(RouteError::NoWay(why)) => assert!(why.contains("Shop"), "{why}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_exit_is_planned_around() {
    let (_t, gd, _dir) = world().await;
    // The game refused the Square's east door: round by the Street and the Lane.
    let rules = RouteRules {
        refused: HashSet::from([((1, 1), 2)]),
        ..Default::default()
    };
    let r = gd.route_with((1, 1), (1, 3), &rules).unwrap();
    assert_eq!(through(&r), [(1, 2), (1, 6), (1, 3)]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_room_that_costs_more_is_gone_round_when_it_can_be() {
    let (_t, gd, _dir) = world().await;
    // Street to Shop is two steps either way: by the Square or by the Lane.
    for (dear, way) in [((1, 1), (1, 6)), ((1, 6), (1, 1))] {
        let rules = RouteRules {
            prefer_not: HashMap::from([(dear, 20)]),
            ..Default::default()
        };
        let r = gd.route_with((1, 2), (1, 3), &rules).unwrap();
        assert_eq!(through(&r)[0], way, "costly {dear:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn no_way_at_all_says_so() {
    let (_t, gd, _dir) = world().await;
    match gd.route_with((1, 1), (1, 4), &RouteRules::default()) {
        Err(RouteError::NoWay(why)) => assert!(why.contains("no way"), "{why}"),
        other => panic!("{other:?}"),
    }
}
