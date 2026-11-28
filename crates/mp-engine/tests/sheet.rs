//! Character sheets (#10, design §6.9, §15.3): the status display and the inventory, read
//! whenever their output arrives, whoever asked for it.

use mp_engine::{Coins, Encumbrance, Inventory, Magic, SheetItem, SheetPart, SheetReader, Stat};
use mp_parse::{Event, Item, Rest, Vitals};

fn prompt() -> Item {
    Item::Prompt(Vitals {
        hp: 48,
        max_hp: Some(56),
        mp: Some(9),
        max_mp: Some(9),
        rest: Rest::Standing,
    })
}

fn ev(e: Event) -> Item {
    Item::Event(e)
}

fn s(v: &str) -> String {
    v.to_owned()
}

/// A Mage's status display, as `st` prints it.
fn status_display(poisoned: bool) -> Vec<Item> {
    let mut v = vec![
        ev(Event::StName {
            name: s("Kasimir Drakemoor"),
            lives: 9,
            cp: 12,
        }),
        ev(Event::StRace {
            race: s("Human"),
            exp: 1712,
            perception: 33,
        }),
        ev(Event::StClass {
            class: s("Mage"),
            level: 2,
            stealth: 0,
        }),
        ev(Event::StHits {
            hp: 48,
            max_hp: 56,
            ac: 19,
            dr: 1,
            thievery: 0,
        }),
        ev(Event::StMagic {
            kind: s("Mana"),
            marked: true,
            mp: 9,
            max_mp: 9,
            spellcasting: Some(40),
            traps: 0,
        }),
        ev(Event::StPicklocks { picklocks: 0 }),
        ev(Event::StStrength {
            strength: 79,
            strength_marked: false,
            agility: 42,
            agility_marked: true,
            tracking: 0,
        }),
        ev(Event::StIntellect {
            intellect: 40,
            intellect_marked: false,
            health: 64,
            health_marked: false,
            martial_arts: 0,
        }),
        ev(Event::StWillpower {
            willpower: 40,
            willpower_marked: false,
            charm: 70,
            charm_marked: false,
            magic_res: 30,
        }),
    ];
    if poisoned {
        v.push(ev(Event::StPoisoned));
    }
    v
}

/// What the reader gives for `items`, in order.
fn read(r: &mut SheetReader, items: &[Item]) -> Vec<SheetPart> {
    items.iter().filter_map(|i| r.on_item(i)).collect()
}

#[test]
fn the_status_display_is_read_whole_at_its_prompt() {
    let mut r = SheetReader::default();
    let mut items = status_display(false);
    assert!(read(&mut r, &items).is_empty(), "nothing before the prompt");
    items = vec![prompt()];
    let parts = read(&mut r, &items);
    let [SheetPart::Status(st)] = parts.as_slice() else {
        panic!("{parts:?}");
    };
    assert_eq!(st.name, "Kasimir Drakemoor");
    assert_eq!((st.lives, st.cp), (9, 12));
    assert_eq!(
        (st.race.as_str(), st.exp, st.perception),
        ("Human", 1712, 33)
    );
    assert_eq!((st.class.as_str(), st.level), ("Mage", 2));
    assert_eq!((st.hp, st.max_hp, st.ac, st.dr), (48, 56, 19, 1));
    assert_eq!(
        st.magic,
        Some(Magic {
            kind: s("Mana"),
            marked: true,
            mp: 9,
            max_mp: 9,
            spellcasting: Some(40),
        })
    );
    assert_eq!(
        st.strength,
        Stat {
            value: 79,
            marked: false
        }
    );
    assert_eq!(
        st.agility,
        Stat {
            value: 42,
            marked: true
        }
    );
    assert_eq!(st.charm.value, 70);
    assert_eq!(
        (st.health.value, st.intellect.value, st.willpower.value),
        (64, 40, 40)
    );
    assert_eq!((st.tracking, st.martial_arts, st.magic_res), (0, 0, 30));
    assert!(!st.poisoned);
}

#[test]
fn a_poisoned_character_and_a_class_without_magic() {
    let mut r = SheetReader::default();
    let mut items: Vec<Item> = status_display(true)
        .into_iter()
        .map(|i| match i {
            Item::Event(Event::StMagic { .. }) => ev(Event::StTraps { traps: 5 }),
            other => other,
        })
        .collect();
    items.push(prompt());
    let parts = read(&mut r, &items);
    let [SheetPart::Status(st)] = parts.as_slice() else {
        panic!("{parts:?}");
    };
    assert!(st.poisoned);
    assert_eq!(st.magic, None);
    assert_eq!(st.traps, 5);
}

#[test]
fn a_status_display_cut_short_is_dropped() {
    let mut r = SheetReader::default();
    let mut items: Vec<Item> = status_display(false).into_iter().take(4).collect();
    items.push(prompt());
    assert!(read(&mut r, &items).is_empty());
    // The next whole one still reads.
    let mut again = status_display(false);
    again.push(prompt());
    assert_eq!(read(&mut r, &again).len(), 1);
}

#[test]
fn the_inventory_reads_coins_items_wealth_and_weight() {
    let mut r = SheetReader::default();
    let items = vec![
        ev(Event::Carrying {
            items: s("1 gold crown, 18 silver nobles, 12 copper farthings, padded vest (Torso),"),
        }),
        Item::Text(s("club (Weapon Hand), torch (Readied/80), 2 rations")),
        ev(Event::KeysLine),
        ev(Event::Wealth { copper: 292 }),
        ev(Event::Encumbrance {
            cur: 520,
            max: 3792,
            level: s("None"),
            pct: 13,
        }),
    ];
    let parts = read(&mut r, &items);
    let [SheetPart::Inventory(inv)] = parts.as_slice() else {
        panic!("{parts:?}");
    };
    assert_eq!(
        **inv,
        Inventory {
            coins: Coins {
                runic: 0,
                platinum: 0,
                gold: 1,
                silver: 18,
                copper: 12,
            },
            items: vec![
                SheetItem {
                    name: s("padded vest"),
                    count: 1,
                    place: Some(s("Torso")),
                },
                SheetItem {
                    name: s("club"),
                    count: 1,
                    place: Some(s("Weapon Hand")),
                },
                SheetItem {
                    name: s("torch"),
                    count: 1,
                    place: Some(s("Readied/80")),
                },
                SheetItem {
                    name: s("rations"),
                    count: 2,
                    place: None,
                },
            ],
            wealth: Some(292),
            encumbrance: Some(Encumbrance {
                cur: 520,
                max: 3792,
                level: s("None"),
                pct: 13,
            }),
        }
    );
    assert_eq!(inv.coins.copper_value(), 100 + 180 + 12);
    // 3 coins of a type weigh 1: 1 gold and 18 silver and 12 copper weigh 0 + 6 + 4.
    assert_eq!(inv.coins.weight(), 10);
    // Room for (3792 - 520) × 3 more coins.
    assert_eq!(inv.coins_room(), Some((3792 - 520) * 3));
}

#[test]
fn an_empty_inventory_and_the_rarer_coins() {
    let mut r = SheetReader::default();
    let parts = read(
        &mut r,
        &[
            ev(Event::CarryingNothing),
            ev(Event::KeysLine),
            ev(Event::Wealth { copper: 0 }),
            ev(Event::Encumbrance {
                cur: 0,
                max: 3792,
                level: s("None"),
                pct: 0,
            }),
        ],
    );
    let [SheetPart::Inventory(inv)] = parts.as_slice() else {
        panic!("{parts:?}");
    };
    assert!(inv.items.is_empty());
    assert_eq!(inv.coins, Coins::default());

    let parts = read(
        &mut r,
        &[
            ev(Event::Carrying {
                items: s("2 runic coins, 1 platinum piece, 1 copper farthing"),
            }),
            ev(Event::KeysLine),
            ev(Event::Wealth { copper: 2_010_001 }),
            prompt(),
        ],
    );
    let [SheetPart::Inventory(inv)] = parts.as_slice() else {
        panic!("{parts:?}");
    };
    assert_eq!(
        (inv.coins.runic, inv.coins.platinum, inv.coins.copper),
        (2, 1, 1)
    );
    assert_eq!(inv.coins.copper_value(), 2_010_001);
    assert_eq!(
        inv.encumbrance, None,
        "a prompt ends it without the weight line"
    );
}

// The card's XP bar (#10): `exp`'s figures, kept up by the kills since.
#[test]
fn experience_is_kept_up_by_kills() {
    use mp_engine::Experience;
    let mut x = Experience {
        exp: 346,
        level: 1,
        left: 1304,
        need: 1650,
    };
    assert_eq!(x.pct(), 20, "346 of 1650, as exp's [pct%] reckons it");
    x.gain(1000);
    assert_eq!((x.exp, x.left, x.pct()), (1346, 304, 81));
    x.gain(500);
    assert_eq!(
        (x.left, x.pct()),
        (0, 100),
        "ready to train, and no further"
    );
}
