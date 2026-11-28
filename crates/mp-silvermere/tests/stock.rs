//! A test fight in silvermere's stock world, by hand (design §10.4; MVP 4a): a mage created
//! through the game's own dialogue, knowing magic missile, in the arena with the practice
//! dummy, the combat rounds stepped by the test.

use mp_parse::{Event, Item};
use mp_silvermere::{RawClient, StockChar, StockSilvermere};

#[tokio::test(flavor = "multi_thread")]
async fn a_mage_casts_at_the_arena_dummy_by_hand() {
    let sm = StockSilvermere::start(&[StockChar::new("Ermias", "Mage").knows(&["mmis"])], true)
        .await
        .expect("the stock world starts");
    let mut c = RawClient::log_in(sm.telnet_addr(), "Ermias", "ermias1")
        .await
        .unwrap();
    let here = c.seen.iter().find_map(|i| match i {
        Item::Event(Event::AlsoHere { names }) => Some(names.clone()),
        _ => None,
    });
    assert!(
        here.as_ref()
            .is_some_and(|n| n.iter().any(|m| m == "practice dummy")),
        "the arena's roster: {here:?}; saw {:#?}",
        c.seen
    );
    let Some(Item::Prompt(before)) = c
        .seen
        .iter()
        .rev()
        .find(|i| matches!(i, Item::Prompt(_)))
        .cloned()
    else {
        unreachable!()
    };
    c.send("mmis practice dummy").await.unwrap();
    c.read_until(|i| *i == Item::Event(Event::CombatEngaged))
        .await
        .unwrap();
    // Step rounds until the spell has cost mana.
    let mut spent = false;
    for _ in 0..10 {
        sm.tick(10).await;
        let got = c
            .read_until(|i| matches!(i, Item::Prompt(v) if v.mp < before.mp))
            .await;
        if got.is_ok() {
            spent = true;
            break;
        }
    }
    assert!(spent, "no cast cost mana; saw {:#?}", c.seen);
    sm.stop().await.unwrap();
}

// A monster's hit spell on a player recalculates the player's stats, putting a max HP a
// test raised back to the class's own (#129): the arena's acid slime has one. A test that
// boosts HP takes them off first.
#[tokio::test(flavor = "multi_thread")]
async fn the_monsters_hit_spells_can_be_taken_off() {
    let sm = StockSilvermere::start(&[StockChar::new("Ermias", "Mage")], true)
        .await
        .unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let count = move |tx: std::sync::mpsc::Sender<usize>| {
        move |w: &mut silvermere_engine::World| {
            let n = w
                .monsters
                .values()
                .flat_map(|m| m.attacks.iter())
                .filter(|a| a.hitspell.0 != 0)
                .count();
            tx.send(n).unwrap();
        }
    };
    sm.on_world(count(tx.clone())).await;
    assert!(rx.recv().unwrap() > 0, "the stock world has hit spells");
    sm.without_hit_spells().await;
    sm.on_world(count(tx)).await;
    assert_eq!(rx.recv().unwrap(), 0);
    sm.stop().await.unwrap();
}
