//! The compiled room file (#63, design §14.2-14.4): `places/<area>.yaml` written from
//! silvermere's database, read back, and rooms named in it by alias, slug, the game's
//! name, or key. Run against the stock world, whose Newhaven the 1.0 bot plays.

use std::collections::BTreeSet;
use std::fs;
use std::sync::OnceLock;

use mp_data::places::{Area, Exit, Lock, PlaceError, Places, compile};
use mp_silvermere::stock_template;

/// Every area of the stock world, compiled once for all the tests here.
fn areas() -> &'static Vec<Area> {
    static A: OnceLock<Vec<Area>> = OnceLock::new();
    A.get_or_init(|| compile(&stock_template()).expect("the stock world compiles"))
}

fn area(name: &str) -> &'static Area {
    areas()
        .iter()
        .find(|a| a.area == name)
        .unwrap_or_else(|| panic!("no area {name}"))
}

#[test]
fn newhaven_is_an_area_of_its_own() {
    let nh = area("newhaven");
    for slug in [
        "village-entrance",
        "weapons-shop",
        "armour-shop",
        "spell-shop",
        "general-store",
        "narrow-path",
        "adventurers-guild",
        "healer",
    ] {
        assert!(
            nh.rooms.contains_key(slug),
            "no {slug} in {:?}",
            nh.rooms.keys()
        );
    }
    assert_eq!(nh.rooms["weapons-shop"].key, "1/2141");
    assert_eq!(nh.rooms["weapons-shop"].name, "Newhaven, Weapons Shop");
}

#[test]
fn a_name_the_area_repeats_takes_its_room_number() {
    // Newhaven has two rooms called "Newhaven, Arena", and two Narrow Roads.
    let nh = area("newhaven");
    assert!(!nh.rooms.contains_key("arena"));
    assert_eq!(nh.rooms["arena-2150"].key, "1/2150");
    assert!(nh.rooms.contains_key("arena-2155"));
    assert_eq!(nh.rooms["narrow-road-2146"].key, "1/2146");
}

#[test]
fn the_arena_reads_with_its_levels_spawns_and_locked_door() {
    let arena = &area("newhaven").rooms["arena-2150"];
    assert_eq!(arena.kind.as_deref(), Some("arena"));
    assert_eq!(arena.levels.as_deref(), Some("1-3"));
    let spawns = arena.spawns.as_ref().expect("the arena spawns monsters");
    assert_eq!(spawns.max, 2);
    assert!(!spawns.monsters.is_empty());
    assert_eq!(arena.exits["up"], Exit::To("narrow-road-2146".into()));
    // North, through a locked door, into another area.
    match &arena.exits["north"] {
        Exit::Special(x) => {
            assert_eq!(x.to, "dungeon:entrance");
            assert_eq!(x.kind, "door");
            assert_eq!(x.lock, Some(Lock::Locked));
            assert_eq!(x.bash, Some(30));
        }
        other => panic!("north is {other:?}"),
    }
}

#[test]
fn shops_say_what_they_sell_and_the_guild_trains() {
    let nh = area("newhaven");
    let weapons = nh.rooms["weapons-shop"].shop.as_ref().unwrap();
    // The stock world makes it a "specific" shop (its own list), not a "weapons" one.
    assert_eq!(weapons.kind, "specific");
    assert_eq!(weapons.sells, ["quarterstaff", "club"]);
    let armour = nh.rooms["armour-shop"].shop.as_ref().unwrap();
    assert!(armour.sells.contains(&"padded vest".to_owned()));
    let guild = nh.rooms["adventurers-guild"].shop.as_ref().unwrap();
    assert_eq!(guild.kind, "training");
    assert_eq!(guild.trains.as_deref(), Some("any class"));
}

#[test]
fn the_cave_bears_cavern_is_a_lair() {
    // Small Cavern has no area name of its own; it joins the nearest named room's, and the
    // Dungeon has more than one.
    let cavern = &area("dungeon").rooms["small-cavern-2156"];
    assert_eq!(cavern.key, "1/2156");
    let lair = cavern.lair.as_ref().expect("a lair");
    assert_eq!(lair.monsters, ["cave bear"]);
    assert_eq!(lair.max, 1);
    // Two minutes after its last kill, on the next entry.
    assert_eq!(lair.respawn, "2m");
}

#[test]
fn the_file_reads_back_as_it_was_written() {
    let dir = tempfile::tempdir().unwrap();
    let nh = area("newhaven");
    let path = dir.path().join("newhaven.yaml");
    nh.write(&path).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("# Compiled from silvermere.db"), "{text}");
    assert_eq!(&Area::read(&path).unwrap(), nh);
    // The same world compiles to the same text.
    let again = dir.path().join("again.yaml");
    compile(&stock_template())
        .unwrap()
        .into_iter()
        .find(|a| a.area == "newhaven")
        .unwrap()
        .write(&again)
        .unwrap();
    assert_eq!(fs::read_to_string(&again).unwrap(), text);
}

/// The stock world's places, written to a folder with a local file beside Newhaven's.
fn places() -> (tempfile::TempDir, Places) {
    let dir = tempfile::tempdir().unwrap();
    for a in areas() {
        if a.area == "newhaven" || a.area == "dungeon" {
            a.write(&dir.path().join(format!("{}.yaml", a.area)))
                .unwrap();
        }
    }
    fs::write(
        dir.path().join("newhaven.local.yaml"),
        "schema: 1\narea: newhaven\naliases:\n  arena: 1/2150\n  road: narrow-road-2146\n  bear: dungeon:small-cavern-2156\n",
    )
    .unwrap();
    let places = Places::load(dir.path()).unwrap();
    (dir, places)
}

#[test]
fn a_room_is_named_by_alias_slug_name_or_key() {
    let (_dir, p) = places();
    let nh = Some("newhaven");
    assert_eq!(p.resolve("arena", nh), Ok((1, 2150)));
    assert_eq!(p.resolve("road", nh), Ok((1, 2146)));
    assert_eq!(p.resolve("bear", nh), Ok((1, 2156)));
    assert_eq!(p.resolve("arena-2155", nh), Ok((1, 2155)));
    assert_eq!(p.resolve("weapons-shop", None), Ok((1, 2141)));
    assert_eq!(p.resolve("dungeon:small-cavern-2156", nh), Ok((1, 2156)));
    assert_eq!(p.resolve("small-cavern-2156", nh), Ok((1, 2156)));
    assert_eq!(p.resolve("Newhaven, Weapons Shop", None), Ok((1, 2141)));
    assert_eq!(p.resolve("1/2150", None), Ok((1, 2150)));
}

#[test]
fn a_name_that_could_be_several_rooms_says_which() {
    let (_dir, p) = places();
    // Without the alias, `arena` is two rooms; so is the game's own name.
    match p.resolve("narrow-road", Some("newhaven")) {
        Err(PlaceError::Ambiguous { rooms, .. }) => {
            let keys: Vec<&str> = rooms.iter().map(|(k, _)| k.as_str()).collect();
            assert!(keys.contains(&"1/2146"), "{keys:?}");
            assert!(keys.contains(&"1/2151"), "{keys:?}");
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        p.resolve("Newhaven, Arena", None),
        Err(PlaceError::Ambiguous { .. })
    ));
}

#[test]
fn an_unknown_room_is_an_error() {
    let (_dir, p) = places();
    assert!(matches!(
        p.resolve("tavern", None),
        Err(PlaceError::Unknown(_))
    ));
    assert!(matches!(
        p.resolve("1/999999", None),
        Err(PlaceError::Unknown(_))
    ));
}

#[test]
fn a_local_alias_to_a_room_that_isnt_there_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    area("newhaven")
        .write(&dir.path().join("newhaven.yaml"))
        .unwrap();
    fs::write(
        dir.path().join("newhaven.local.yaml"),
        "schema: 1\narea: newhaven\naliases:\n  tavern: the-prancing-pony\n",
    )
    .unwrap();
    let e = Places::load(dir.path()).unwrap_err().to_string();
    assert!(e.contains("tavern"), "{e}");
    assert!(e.contains("newhaven.local.yaml"), "{e}");
}

#[test]
fn a_lair_knows_its_respawn_and_where_to_wait_outside_it() {
    let (_dir, p) = places();
    assert_eq!(p.lair_respawn((1, 2156)), Some("2m"));
    assert_eq!(p.lair_respawn((1, 2152)), Some("1m"));
    assert_eq!(p.lair_respawn((1, 2150)), None);
    // A lair fills when it's entered, so a bot waits outside, in the nearest room that
    // isn't a lair. The cavern's one way out is the Dungeon Entrance, a lair of rats
    // itself, so that's the arena past it.
    assert_eq!(p.wait_room((1, 2156), &BTreeSet::new()), Some((1, 2150)));
    // With the arena avoided, there's nowhere to wait.
    assert_eq!(p.wait_room((1, 2156), &BTreeSet::from([(1, 2150)])), None);
    // The entrance's own wait-room is the arena next door.
    assert_eq!(p.wait_room((1, 2152), &BTreeSet::new()), Some((1, 2150)));
}

#[test]
fn a_trainer_is_found_for_a_class() {
    let (_dir, p) = places();
    // The Adventurer's Guild trains any class, so it's a Warrior's trainer and one for a
    // character whose class isn't known.
    assert_eq!(
        p.trainers(Some("Warrior"), Some("newhaven")).first(),
        Some(&(1, 2147))
    );
    assert!(p.trainers(None, Some("newhaven")).contains(&(1, 2147)));
}

// What kind of shop a room is, for the page's Sell (#101).
#[test]
fn a_room_s_shop_kind_is_known() {
    let (_dir, p) = places();
    assert_eq!(p.shop_kind((1, 2141)), Some("specific"));
    assert_eq!(p.shop_kind((1, 2147)), Some("training"));
    assert_eq!(p.shop_kind((1, 2150)), None);
}
