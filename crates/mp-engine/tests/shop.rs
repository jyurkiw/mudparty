//! Gear (#55), as pure logic: reading what the bot carries from the inventory display, and
//! buying an item and putting it to use.

use std::time::{Duration, Instant};

use mp_config::GearUse;
use mp_engine::{Carried, EquipState, Equipper, InventoryReader, parse_carried};
use mp_parse::{Event, Item, Rest, Vitals};

fn carried(name: &str, count: u32, equipped: bool) -> Carried {
    Carried {
        name: name.into(),
        count,
        equipped,
    }
}

#[test]
fn the_carried_list_reads_counts_and_what_is_worn() {
    assert_eq!(
        parse_carried(
            "18 silver nobles, padded vest (Torso), club (Weapon Hand), 2 torch, quarterstaff"
        ),
        vec![
            carried("silver nobles", 18, false),
            carried("padded vest", 1, true),
            carried("club", 1, true),
            carried("torch", 2, false),
            carried("quarterstaff", 1, false),
        ]
    );
    assert_eq!(
        parse_carried("quarterstaff (Two handed), lantern (Readied/3)"),
        vec![
            carried("quarterstaff", 1, true),
            carried("lantern", 1, true)
        ]
    );
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

#[test]
fn a_wrapped_list_is_read_whole_up_to_the_keys_line() {
    let mut r = InventoryReader::default();
    assert_eq!(
        r.on_item(&Item::Event(Event::Carrying {
            items: "club (Weapon Hand), padded vest (Torso), padded pants (Legs), padded".into()
        })),
        None
    );
    assert_eq!(r.on_item(&Item::Text("helm (Head), torch".into())), None);
    assert_eq!(
        r.on_item(&Item::Event(Event::KeysLine)),
        Some(vec![
            carried("club", 1, true),
            carried("padded vest", 1, true),
            carried("padded pants", 1, true),
            carried("padded helm", 1, true),
            carried("torch", 1, false),
        ])
    );
    // Lines after it are no part of it.
    assert_eq!(r.on_item(&Item::Text("whatever".into())), None);
    assert_eq!(r.on_item(&Item::Event(Event::Wealth { copper: 0 })), None);
}

#[test]
fn nothing_is_an_empty_list_and_a_prompt_ends_a_list() {
    let mut r = InventoryReader::default();
    assert_eq!(
        r.on_item(&Item::Event(Event::CarryingNothing)),
        Some(Vec::new())
    );
    r.on_item(&Item::Event(Event::Carrying {
        items: "club".into(),
    }));
    assert_eq!(r.on_item(&prompt()), Some(vec![carried("club", 1, false)]));
}

const WAIT: Duration = Duration::from_secs(10);

fn hear(e: &mut Equipper, ev: Event, now: Instant) -> Vec<String> {
    e.on_item(&Item::Event(ev), now)
}

#[test]
fn it_buys_then_wields() {
    let now = Instant::now();
    let mut e = Equipper::new("club", GearUse::Wield, true, WAIT);
    assert_eq!(e.start(now), ["buy club"]);
    assert_eq!(e.state(), EquipState::Buying);
    assert_eq!(
        hear(
            &mut e,
            Event::Bought {
                item: "club".into()
            },
            now
        ),
        ["wield club"]
    );
    assert_eq!(e.state(), EquipState::Equipping);
    assert!(
        hear(
            &mut e,
            Event::Equipped {
                item: "club".into()
            },
            now
        )
        .is_empty()
    );
    assert_eq!(e.state(), EquipState::Done);
}

#[test]
fn an_item_already_carried_is_only_worn() {
    let now = Instant::now();
    let mut e = Equipper::new("padded vest", GearUse::Wear, false, WAIT);
    assert_eq!(e.start(now), ["wear padded vest"]);
    hear(
        &mut e,
        Event::Equipped {
            item: "padded vest".into(),
        },
        now,
    );
    assert_eq!(e.state(), EquipState::Done);
}

#[test]
fn what_it_cant_afford_or_use_fails_with_the_reason() {
    let now = Instant::now();
    let mut e = Equipper::new("club", GearUse::Wield, true, WAIT);
    e.start(now);
    hear(
        &mut e,
        Event::CannotAfford {
            item: "club".into(),
        },
        now,
    );
    assert_eq!(e.state(), EquipState::Failed("can't afford club".into()));

    let mut e = Equipper::new("club", GearUse::Wield, true, WAIT);
    e.start(now);
    hear(
        &mut e,
        Event::Bought {
            item: "club".into(),
        },
        now,
    );
    hear(
        &mut e,
        Event::EquipRefused {
            text: "You may not use that weapon.".into(),
        },
        now,
    );
    assert_eq!(
        e.state(),
        EquipState::Failed("You may not use that weapon.".into())
    );

    let mut e = Equipper::new("club", GearUse::Wield, true, WAIT);
    e.start(now);
    hear(
        &mut e,
        Event::BuyRefused {
            text: "You cannot buy club here!".into(),
        },
        now,
    );
    assert_eq!(
        e.state(),
        EquipState::Failed("You cannot buy club here!".into())
    );
}

#[test]
fn a_line_with_no_answer_is_sent_again_once() {
    let now = Instant::now();
    let mut e = Equipper::new("club", GearUse::Wield, true, WAIT);
    e.start(now);
    assert!(e.on_tick(now + WAIT - Duration::from_secs(1)).is_empty());
    assert_eq!(e.on_tick(now + WAIT), ["buy club"]);
    assert!(e.on_tick(now + WAIT * 2).is_empty());
    assert!(matches!(e.state(), EquipState::Failed(why) if why.contains("answer")));
}
