//! Routes between rooms over the game data's exits (#52): the shortest walk, through doors
//! and map changes, and never through an exit a walker can't pass by walking.

use std::path::PathBuf;

use mp_data::{GameData, Step, import};
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
///   1/2 Street ── north of ── 1/1 Square ── east, a door ── 1/3 Shop
///                              │  up: hidden 1/4 Loft
///                              │  down: a key exit to 1/5 Vault
///                              └ south: a map change to 2/1 Far Room
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
    db.rooms().insert(&st).await.unwrap();
    let mut shop = blank::clear_room(RoomKey::new(1, 3));
    shop.name = "Shop".into();
    shop.exits[W] = exit(ExitType::Door, 1, 0);
    db.rooms().insert(&shop).await.unwrap();
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
