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
