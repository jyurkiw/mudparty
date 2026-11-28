//! Capturing a command's output for the page (#10, #101): `help strength`, `look club`.
//! The output starts after the game echoes the command and ends at the next prompt.

use std::time::{Duration, Instant};

use mp_engine::{Captured, Capturer};

const WAIT: Duration = Duration::from_secs(15);

fn line(c: &mut Capturer, text: &str) -> Option<Captured> {
    c.on_line(text, false)
}

fn prompt(c: &mut Capturer) -> Option<Captured> {
    c.on_line("[HP=48/56,MA=9/9]:", true)
}

#[test]
fn the_output_runs_from_the_echo_to_the_prompt() {
    let now = Instant::now();
    let mut c = Capturer::new(WAIT);
    c.ask("h1", "help strength", now);
    // Whatever came before the echo isn't the answer.
    assert_eq!(
        line(&mut c, "A giant rat creeps into the room from nowhere."),
        None
    );
    assert_eq!(prompt(&mut c), None);
    assert_eq!(line(&mut c, "help strength"), None);
    assert_eq!(
        line(&mut c, "Strength is a measure of your physical power."),
        None
    );
    assert_eq!(line(&mut c, "It decides how much you can carry."), None);
    assert_eq!(
        prompt(&mut c),
        Some(Captured {
            id: "h1".into(),
            line: "help strength".into(),
            text:
                "Strength is a measure of your physical power.\nIt decides how much you can carry."
                    .into(),
            answered: true,
        })
    );
    assert_eq!(prompt(&mut c), None, "each answer once");
}

#[test]
fn the_echo_is_matched_loosely_and_queued_asks_answer_in_turn() {
    let now = Instant::now();
    let mut c = Capturer::new(WAIT);
    c.ask("a", "look club", now);
    c.ask("b", "help human", now);
    assert_eq!(line(&mut c, "  LOOK Club "), None);
    assert_eq!(line(&mut c, "A heavy wooden club."), None);
    let first = prompt(&mut c).unwrap();
    assert_eq!(
        (first.id.as_str(), first.text.as_str()),
        ("a", "A heavy wooden club.")
    );
    assert_eq!(line(&mut c, "help human"), None);
    assert_eq!(line(&mut c, "Humans are the most common race."), None);
    assert_eq!(prompt(&mut c).unwrap().id, "b");
}

#[test]
fn an_ask_never_echoed_gives_up_saying_so() {
    let now = Instant::now();
    let mut c = Capturer::new(WAIT);
    c.ask("x", "help nothing", now);
    assert!(c.on_tick(now + WAIT - Duration::from_secs(1)).is_empty());
    assert_eq!(
        c.on_tick(now + WAIT),
        vec![Captured {
            id: "x".into(),
            line: "help nothing".into(),
            text: String::new(),
            answered: false,
        }]
    );
    assert!(c.on_tick(now + WAIT * 2).is_empty());
}

#[test]
fn an_answer_cut_off_by_the_wait_keeps_what_came() {
    let now = Instant::now();
    let mut c = Capturer::new(WAIT);
    c.ask("y", "help races", now);
    line(&mut c, "help races");
    line(&mut c, "Human, Elf, Dwarf");
    let late = c.on_tick(now + WAIT);
    assert_eq!(late.len(), 1);
    assert_eq!(late[0].text, "Human, Elf, Dwarf");
    assert!(late[0].answered);
}
