//! The text history (the card's History modal): what a character saw, less what the page
//! already shows elsewhere. Chat, fights, monsters coming and going, and text nothing
//! else reads are kept; sheets, room displays, `par`, and informational commands' output
//! are not.

use mp_engine::{HistLine, HistoryFilter};
use mp_parse::{Event, Item, Rest, Vitals};

fn prompt() -> (String, Item) {
    (
        s("[HP=48/56,MA=9/9]:"),
        Item::Prompt(Vitals {
            hp: 48,
            max_hp: Some(56),
            mp: Some(9),
            max_mp: Some(9),
            rest: Rest::Standing,
        }),
    )
}

fn s(v: &str) -> String {
    v.to_owned()
}

fn text(t: &str) -> (String, Item) {
    (s(t), Item::Text(s(t)))
}

fn ev(plain: &str, e: Event) -> (String, Item) {
    (s(plain), Item::Event(e))
}

/// The lines kept, as `kind: text`.
fn feed(f: &mut HistoryFilter, lines: &[(String, Item)]) -> Vec<String> {
    lines
        .iter()
        .flat_map(|(plain, item)| f.on_item(plain, item))
        .map(|HistLine { kind, text }| format!("{kind}: {text}"))
        .collect()
}

#[test]
fn chat_fights_and_comings_and_goings_are_kept() {
    let mut f = HistoryFilter::default();
    let kept = feed(
        &mut f,
        &[
            ev(
                "Raijin telepaths: @health",
                Event::Telepath {
                    who: s("Raijin"),
                    text: s("@health"),
                },
            ),
            text("Bob says \"hello there\""),
            text("A angry giant rat creeps into the room from nowhere."),
            ev(
                "kobold just left to the north.",
                Event::Left { who: s("kobold") },
            ),
            ev("*Combat Engaged*", Event::CombatEngaged),
            text("You whap kobold for 6 damage!"),
            ev("You gain 4 experience.", Event::ExpGain { exp: 4 }),
            prompt(),
        ],
    );
    assert_eq!(
        kept,
        [
            "chat: Raijin telepaths: @health",
            "chat: Bob says \"hello there\"",
            "move: A angry giant rat creeps into the room from nowhere.",
            "move: kobold just left to the north.",
            "combat: *Combat Engaged*",
            "combat: You gain 4 experience.",
            "text: You whap kobold for 6 damage!",
        ],
        "events at once; plain text when its prompt comes"
    );
}

#[test]
fn a_room_display_is_left_out() {
    let mut f = HistoryFilter::default();
    f.sent("north");
    let kept = feed(
        &mut f,
        &[
            text("north"),
            text("Newhaven, Arena"),
            text("    This huge room is has been carved into the earth."),
            text("You notice 3 silver nobles here."),
            ev(
                "Also here: kobold.",
                Event::AlsoHere {
                    names: vec![s("kobold")],
                },
            ),
            ev("Obvious exits: up", Event::ObviousExits),
            text("The kobold lunges at you!"),
            prompt(),
        ],
    );
    assert_eq!(kept, ["text: The kobold lunges at you!"]);
}

#[test]
fn an_informational_command_s_output_is_left_out() {
    let mut f = HistoryFilter::default();
    f.sent("help strength");
    let kept = feed(
        &mut f,
        &[
            text("help strength"),
            text("Strength is a measure of your physical power."),
            text("Bob gossips: anyone selling a club?"),
            prompt(),
            text("Something else happens."),
            prompt(),
        ],
    );
    assert_eq!(
        kept,
        [
            "chat: Bob gossips: anyone selling a club?",
            "text: Something else happens.",
        ]
    );
    // A display's label isn't talk, though its word is a talk verb's.
    let kept = feed(&mut f, &[text("Allow Telepaths:    Yes"), prompt()]);
    assert_eq!(kept, ["text: Allow Telepaths:    Yes"]);
    // A command typed by a player in MudPlay, never sent by the bot: known by its word.
    let kept = feed(
        &mut f,
        &[text("st"), text("an unread status line"), prompt()],
    );
    assert!(kept.is_empty(), "{kept:?}");
}

#[test]
fn what_the_sheets_and_party_already_show_is_left_out() {
    let mut f = HistoryFilter::default();
    let kept = feed(
        &mut f,
        &[
            ev(
                "Name: Ermias    Lives/CP: 9/0",
                Event::StName {
                    name: s("Ermias"),
                    lives: 9,
                    cp: 0,
                },
            ),
            ev(
                "You are carrying club",
                Event::Carrying { items: s("club") },
            ),
            ev("Wealth: 10 copper farthings", Event::Wealth { copper: 10 }),
            ev("1/2150", Event::RoomKey { map: 1, room: 2150 }),
            ev(
                "--- Telepath Sent to Raijin ---",
                Event::TelepathSent { who: s("Raijin") },
            ),
            prompt(),
        ],
    );
    assert!(kept.is_empty(), "{kept:?}");
}

#[test]
fn the_bot_s_own_echo_is_left_out() {
    let mut f = HistoryFilter::default();
    f.sent("a kobold");
    let kept = feed(
        &mut f,
        &[
            text("a kobold"),
            ev("*Combat Engaged*", Event::CombatEngaged),
            prompt(),
        ],
    );
    assert_eq!(kept, ["combat: *Combat Engaged*"]);
}
