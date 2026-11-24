//! The reader: bytes in, prompts, events and text out, with a prompt split from what follows
//! it on the same line.

use mp_parse::{Event, Item, Reader, Rest, Statline, Vitals};
use silvermere_auth::login::AuthMsg;
use silvermere_text::{Msg, Palette, render};

const BOT: &str = "[HP=%h/%H,MA=%m/%M]:%r";

fn reader() -> Reader {
    Reader::new(Some(Statline::compile(BOT).unwrap()))
}

fn bytes(m: &Msg) -> Vec<u8> {
    render(m, Palette::new(0).unwrap()).bytes
}

fn prompt(hp: i32, mp: i32) -> Vec<u8> {
    // As silvermere sends it: the clear-line prefix, a colour, the statline, no line end.
    format!("\x1b[79D\x1b[K\x1b[0;37m[HP={hp}/120,MA={mp}/40]:").into_bytes()
}

fn vitals(hp: i32, mp: i32) -> Item {
    Item::Prompt(Vitals {
        hp,
        max_hp: Some(120),
        mp: Some(mp),
        max_mp: Some(40),
        rest: Rest::Standing,
    })
}

#[test]
fn a_prompt_is_reported_once_even_when_the_next_message_writes_over_it() {
    let mut r = reader();
    assert_eq!(r.feed(&prompt(87, 12)), [vitals(87, 12)]);
    let items = r.feed(&bytes(&Msg::CombatEngaged));
    assert_eq!(items, [Item::Event(Event::CombatEngaged)]);
}

#[test]
fn a_prompt_split_across_reads_is_reported_when_whole() {
    let mut r = reader();
    let p = prompt(87, 12);
    let (a, b) = p.split_at(p.len() - 6);
    assert!(r.feed(a).is_empty());
    assert_eq!(r.feed(b), [vitals(87, 12)]);
}

#[test]
fn an_acknowledgement_glued_after_a_prompt_is_split_from_it() {
    let mut r = reader();
    let mut b = prompt(32, 27);
    b.extend_from_slice(b"\x1b[0;37m--- Telepath Sent to Raijin ---\r\n");
    assert_eq!(
        r.feed(&b),
        [
            vitals(32, 27),
            Item::Event(Event::TelepathSent {
                who: "Raijin".into()
            })
        ]
    );
}

#[test]
fn an_echoed_command_after_a_prompt_is_text() {
    let mut r = reader();
    assert_eq!(r.feed(&prompt(87, 12)), [vitals(87, 12)]);
    assert_eq!(r.feed(b"north\r\n"), [Item::Text("north".into())]);
}

#[test]
fn login_prompts_are_reported_once_while_they_wait() {
    let mut r = Reader::new(None);
    let items = r.feed(AuthMsg::UsernamePrompt.text().as_bytes());
    assert_eq!(items, [Item::Event(Event::UsernamePrompt)]);
    // The user's typing, echoed, then the line end.
    assert!(r.feed(b"Ermias").is_empty());
    assert_eq!(r.feed(b"\r\n"), [Item::Text("Ermias".into())]);
}

#[test]
fn without_a_statline_a_prompt_is_only_text() {
    let mut r = Reader::new(None);
    let mut b = prompt(87, 12);
    b.extend_from_slice(b"\r\n");
    assert_eq!(r.feed(&b), [Item::Text("[HP=87/120,MA=12/40]:".into())]);
}

#[test]
fn the_statline_can_be_set_after_login() {
    let mut r = Reader::new(None);
    r.set_statline(Statline::compile(BOT).unwrap());
    assert_eq!(r.feed(&prompt(5, 6)), [vitals(5, 6)]);
}

#[test]
fn blank_lines_are_not_reported() {
    let mut r = reader();
    assert_eq!(r.feed(b"\r\n\r\nHello\r\n"), [Item::Text("Hello".into())]);
}

#[test]
fn a_fight_reads_in_order() {
    let mut r = reader();
    let mut b = Vec::new();
    b.extend(prompt(120, 40));
    b.extend(b"mm kob\r\n");
    b.extend(bytes(&Msg::CombatEngaged));
    b.extend(prompt(120, 38));
    b.extend(bytes(&Msg::CastMovesToCast {
        who: "Raijin".into(),
        spell: "magic missile".into(),
        target: "small kobold".into(),
    }));
    b.extend(bytes(&Msg::ExpGain { exp: 40 }));
    b.extend(bytes(&Msg::CombatOff));
    b.extend(prompt(118, 38));
    assert_eq!(
        r.feed(&b),
        [
            vitals(120, 40),
            Item::Text("mm kob".into()),
            Item::Event(Event::CombatEngaged),
            vitals(120, 38),
            Item::Event(Event::MovesToCast {
                who: "Raijin".into(),
                spell: "magic missile".into(),
                target: "small kobold".into(),
            }),
            Item::Event(Event::ExpGain { exp: 40 }),
            Item::Event(Event::CombatOff),
            vitals(118, 38),
        ]
    );
}

#[test]
fn a_reader_with_several_statlines_reads_any_of_them() {
    let mut r = Reader::with_statlines(Statline::silvermere_defaults());
    let items = r.feed(b"\x1b[79D\x1b[K\x1b[0;37m[HP=35]:");
    assert!(
        matches!(items.as_slice(), [Item::Prompt(v)] if v.hp == 35),
        "{items:?}"
    );
    let items = r.feed(b"\x1b[79D\x1b[K\x1b[0;37m[HP=40/MA=9]:");
    assert!(
        matches!(items.as_slice(), [Item::Prompt(v)] if v.mp == Some(9)),
        "{items:?}"
    );
}

#[test]
fn setting_the_statline_replaces_the_defaults() {
    let mut r = Reader::with_statlines(Statline::silvermere_defaults());
    r.set_statline(Statline::compile(BOT).unwrap());
    let mut b = b"\x1b[79D\x1b[K[HP=35]:".to_vec();
    b.extend_from_slice(b"\r\n");
    assert_eq!(r.feed(&b), [Item::Text("[HP=35]:".into())]);
}

#[test]
fn a_traced_feed_gives_each_item_with_its_text() {
    let mut r = reader();
    let mut b = prompt(32, 27);
    b.extend_from_slice(b"\x1b[0;37m--- Telepath Sent to Raijin ---\r\nnorth\r\n");
    let got: Vec<(Item, String)> = r
        .feed_traced(&b)
        .into_iter()
        .map(|t| (t.item, t.plain))
        .collect();
    assert_eq!(
        got,
        [
            (vitals(32, 27), "[HP=32/120,MA=27/40]:".to_owned()),
            (
                Item::Event(Event::TelepathSent {
                    who: "Raijin".into()
                }),
                "--- Telepath Sent to Raijin ---".to_owned()
            ),
            (Item::Text("north".into()), "north".to_owned()),
        ]
    );
}
