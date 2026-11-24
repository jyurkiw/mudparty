//! The import from silvermere's database (design §7.5). The source is written by silvermere
//! itself (its migrations, its record types and writer), so the import is tested against
//! silvermere's real schema.

use std::path::{Path, PathBuf};

use mp_data::{GameData, ShowsAs, import};
use silvermere_db::TestDb;
use silvermere_engine::world::blank;
use silvermere_types::enums::{ExitType, Magery};
use silvermere_types::ids::{ClassNo, MonsterNo, RoomId, RoomKey};
use silvermere_types::records::Exit;

// Exit slots, silvermere's DIR_ order.
const N: usize = 0;
const S: usize = 1;
const E: usize = 2;
const U: usize = 8;

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

/// A small silvermere world: Town Square (1/1) with a plain exit north, a door east, a hidden
/// exit up and a map change south; Stone Street (1/2) back south; a room on map 2; an
/// Assassin class; and the spell `mmis`.
async fn silvermere_db() -> (TestDb, PathBuf) {
    let t = TestDb::sqlite_temp().await.unwrap();
    let db = t.db();
    let mut sq = blank::clear_room(RoomKey::new(1, 1));
    sq.name = "Town Square".into();
    sq.light = 3;
    sq.exits[N] = exit(ExitType::Normal, 2, 0);
    sq.exits[E] = exit(ExitType::Door, 3, 0);
    sq.exits[U] = exit(ExitType::Hidden, 4, 0);
    sq.exits[S] = exit(ExitType::MapChange, 1, 2);
    db.rooms().insert(&sq).await.unwrap();
    let mut st = blank::clear_room(RoomKey::new(1, 2));
    st.name = "Stone Street".into();
    st.exits[S] = exit(ExitType::Normal, 1, 0);
    db.rooms().insert(&st).await.unwrap();
    for (key, name) in [((1, 3), "Shop"), ((1, 4), "Loft"), ((2, 1), "Far Room")] {
        let mut r = blank::clear_room(RoomKey::new(key.0, key.1));
        r.name = name.into();
        db.rooms().insert(&r).await.unwrap();
    }
    for (n, name) in [(1, "practice dummy"), (2, "kobold"), (3, "orc shaman")] {
        let mut m = blank::clear_known_monster(MonsterNo(n));
        m.name = name.into();
        db.monsters().insert(&m).await.unwrap();
    }
    let mut c = blank::clear_class(ClassNo(7));
    c.name = "Assassin".into();
    c.magery = Magery::Mage;
    c.magelvl = 2;
    db.classes().insert(&c).await.unwrap();
    // `sqlite://<path>`, or on Windows `sqlite:C:\...`, perhaps with `?mode=rwc`.
    let url = db.url();
    let path = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))
        .unwrap();
    let path = PathBuf::from(path.split('?').next().unwrap());
    // Spells have no blank record in silvermere; the row goes into its migrated table.
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute(
        "INSERT INTO spells (number, name, desc0, desc1, resist, levelcap, flags, energy, level,
             minval, maxval, spell_type, resmode, diff, durlvl, target, duration, element,
             resstat, magery, msgnum, mana, maxinc, maxlvls, magelvl, mininc, minlvls, durinc,
             durlvls, shortnm)
         VALUES (1, 'magic missile', '', '', 0, 0, 0, 500, 1, 1, 5, 1, 0, 0, 0, 2, 0, 0, 0,
             1, 0, 3, 0, 0, 1, 0, 0, 0, 0, 'mmis')",
        [],
    )
    .unwrap();
    (t, path)
}

fn out(dir: &Path) -> PathBuf {
    dir.join("gamedata.sqlite")
}

#[tokio::test(flavor = "multi_thread")]
async fn rooms_and_their_exits_are_imported() {
    let (_t, src) = silvermere_db().await;
    let dir = tempfile::tempdir().unwrap();
    import(&src, &out(dir.path())).unwrap();
    let gd = GameData::open(&out(dir.path())).unwrap();
    let r = gd.room(1, 1).unwrap().unwrap();
    assert_eq!(r.name, "Town Square");
    assert_eq!(r.light, 3);
    let exits = gd.exits(1, 1).unwrap();
    let to = |dir: usize| exits.iter().find(|e| e.dir == dir as u8).unwrap();
    assert_eq!(
        (to(N).to_map, to(N).to_room, to(N).shows_as),
        (1, 2, ShowsAs::Plain)
    );
    assert_eq!((to(E).to_room, to(E).shows_as), (3, ShowsAs::Door));
    assert_eq!((to(U).to_room, to(U).shows_as), (4, ShowsAs::Hidden));
    // A map change leads to the same room number on the map in par1.
    assert_eq!(
        (to(S).to_map, to(S).to_room, to(S).shows_as),
        (2, 1, ShowsAs::Plain)
    );
    assert_eq!(exits.len(), 4);
    assert!(gd.room(9, 9).unwrap().is_none());
}

// MegaMud's room hash: the name hash (MudPlay's tested values: "Town Square" is 9D4), then
// the exits code: N and S share the last digit (1 + 4), E a door doubled (2), U hidden left out.
#[tokio::test(flavor = "multi_thread")]
async fn each_room_carries_its_megamud_hash() {
    let (_t, src) = silvermere_db().await;
    let dir = tempfile::tempdir().unwrap();
    import(&src, &out(dir.path())).unwrap();
    let gd = GameData::open(&out(dir.path())).unwrap();
    assert_eq!(gd.room(1, 1).unwrap().unwrap().mm_hash, "9D400025");
    assert_eq!(gd.room(1, 2).unwrap().unwrap().mm_hash, "E9900004");
    assert_eq!(gd.rooms_by_mm_hash("9D400025").unwrap(), [(1, 1)]);
}

#[tokio::test(flavor = "multi_thread")]
async fn spells_and_classes_are_imported() {
    let (_t, src) = silvermere_db().await;
    let dir = tempfile::tempdir().unwrap();
    import(&src, &out(dir.path())).unwrap();
    let gd = GameData::open(&out(dir.path())).unwrap();
    let s = gd.spell_by_short("MMIS").unwrap().unwrap();
    assert_eq!(
        (
            s.number,
            s.name.as_str(),
            s.mana,
            s.level,
            s.magery,
            s.magelvl
        ),
        (1, "magic missile", 3, 1, 1, 1)
    );
    let c = gd.class_by_name("assassin").unwrap().unwrap();
    assert_eq!(
        (c.number, c.name.as_str(), c.magery, c.magelvl),
        (7, "Assassin", 1, 2)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_revision_follows_the_content() {
    let (_t, src) = silvermere_db().await;
    let dir = tempfile::tempdir().unwrap();
    let a = import(&src, &out(dir.path())).unwrap().rev;
    assert_eq!(GameData::open(&out(dir.path())).unwrap().rev().unwrap(), a);
    let b = import(&src, &out(dir.path())).unwrap().rev;
    assert_eq!(a, b, "the same world gives the same revision");
    let conn = rusqlite::Connection::open(&src).unwrap();
    conn.execute(
        "UPDATE rooms SET name = 'Old Square' WHERE map = 1 AND room = 1",
        [],
    )
    .unwrap();
    let c = import(&src, &out(dir.path())).unwrap().rev;
    assert_ne!(a, c);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_import_replaces_the_old_file_whole() {
    let (_t, src) = silvermere_db().await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(out(dir.path()), b"an old file").unwrap();
    let report = import(&src, &out(dir.path())).unwrap();
    assert_eq!(
        (
            report.rooms,
            report.exits,
            report.spells,
            report.classes,
            report.monsters
        ),
        (5, 5, 1, 1, 3)
    );
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["gamedata.sqlite"], "no temp file is left");
    GameData::open(&out(dir.path())).unwrap();
}

#[test]
fn opening_a_missing_file_fails_rather_than_creating_one() {
    let dir = tempfile::tempdir().unwrap();
    assert!(GameData::open(&out(dir.path())).is_err());
    assert!(!out(dir.path()).exists());
}

#[test]
fn exit_types_show_as_obvious_exits_print_them() {
    use mp_data::shows_as;
    assert_eq!(shows_as(i64::from(ExitType::Normal.code())), ShowsAs::Plain);
    for t in [ExitType::Door, ExitType::Key, ExitType::Gate] {
        assert_eq!(shows_as(i64::from(t.code())), ShowsAs::Door, "{t:?}");
    }
    for t in [ExitType::Hidden, ExitType::Trap, ExitType::Timed] {
        assert_eq!(shows_as(i64::from(t.code())), ShowsAs::Hidden, "{t:?}");
    }
    for t in [ExitType::RemoteAction, ExitType::Text] {
        assert_eq!(shows_as(i64::from(t.code())), ShowsAs::None, "{t:?}");
    }
    for t in [
        ExitType::Toll,
        ExitType::Spell,
        ExitType::MapChange,
        ExitType::Level,
    ] {
        assert_eq!(shows_as(i64::from(t.code())), ShowsAs::Plain, "{t:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn monster_names_are_imported_for_telling_monsters_from_players() {
    let (_t, src) = silvermere_db().await;
    let dir = tempfile::tempdir().unwrap();
    import(&src, &out(dir.path())).unwrap();
    let gd = GameData::open(&out(dir.path())).unwrap();
    assert_eq!(
        gd.monster_names().unwrap(),
        ["kobold", "orc shaman", "practice dummy"]
    );
}
