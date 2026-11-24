//! The login state machine (design §6.2): silvermere's dialogue, the takeover question, the
//! credential guard and the step timeout. Pure logic over parsed items and a clock.

use std::time::{Duration, Instant};

use mp_engine::{Login, LoginAction as A, LoginFailure as F, LoginSettings};
use mp_parse::{Event, Item, Rest, Vitals};

fn settings(takeover: bool) -> LoginSettings {
    LoginSettings {
        account: "ermias".into(),
        password: "hunter22".into(),
        takeover,
        step_timeout: Duration::from_secs(30),
    }
}

fn ev(e: Event) -> Item {
    Item::Event(e)
}

fn prompt(hp: i32) -> Item {
    Item::Prompt(Vitals {
        hp,
        max_hp: None,
        mp: None,
        max_mp: None,
        rest: Rest::Standing,
    })
}

/// Runs `items` through a fresh login, collecting every action.
fn run(takeover: bool, items: &[Item]) -> Vec<A> {
    let t0 = Instant::now();
    let mut l = Login::new(settings(takeover), t0);
    items.iter().flat_map(|i| l.on_item(i, t0)).collect()
}

#[test]
fn silvermeres_dialogue_reaches_the_game() {
    let got = run(
        true,
        &[
            ev(Event::UsernamePrompt),
            ev(Event::PasswordPrompt),
            ev(Event::MainMenu),
            ev(Event::MudMenu),
            prompt(35),
        ],
    );
    assert_eq!(
        got,
        [
            A::Send("ermias".into()),
            A::SendSecret("hunter22".into()),
            A::Send("M".into()),
            A::Send("E".into()),
            A::InGame,
        ]
    );
}

#[test]
fn other_lines_on_the_way_are_ignored() {
    let got = run(
        true,
        &[
            Item::Text("Welcome to silvermere".into()),
            ev(Event::UsernamePrompt),
        ],
    );
    assert_eq!(got, [A::Send("ermias".into())]);
}

#[test]
fn the_takeover_question_is_answered_yes_by_default() {
    let got = run(
        true,
        &[
            ev(Event::UsernamePrompt),
            ev(Event::PasswordPrompt),
            ev(Event::DuplicateLogin),
            ev(Event::TakeoverQuestion),
        ],
    );
    assert_eq!(got.last(), Some(&A::Send("Y".into())));
}

#[test]
fn with_takeover_off_the_answer_is_no_and_the_login_stops() {
    let got = run(
        false,
        &[
            ev(Event::UsernamePrompt),
            ev(Event::PasswordPrompt),
            ev(Event::TakeoverQuestion),
        ],
    );
    assert_eq!(
        &got[2..],
        [A::Send("N".into()), A::Failed(F::AlreadyLoggedOn)]
    );
}

#[test]
fn refusals_stop_the_login() {
    for (event, failure) in [
        (Event::InvalidCredentials, F::InvalidCredentials),
        (
            Event::AccountLocked { minutes: 9 },
            F::AccountLocked { minutes: 9 },
        ),
        (Event::NoPassword, F::NoPassword),
        (Event::TakeoverFailed, F::TakeoverFailed),
    ] {
        let got = run(
            true,
            &[
                ev(Event::UsernamePrompt),
                ev(Event::PasswordPrompt),
                ev(event),
            ],
        );
        assert_eq!(got.last(), Some(&A::Failed(failure.clone())), "{failure:?}");
    }
}

// The credential guard (from MudPlay): a second request for the name or the password is
// never answered.
#[test]
fn a_second_request_for_the_password_is_refused() {
    let got = run(
        true,
        &[
            ev(Event::UsernamePrompt),
            ev(Event::PasswordPrompt),
            ev(Event::PasswordPrompt),
        ],
    );
    assert_eq!(got.last(), Some(&A::Failed(F::AskedAgain("password"))));
    assert_eq!(
        got.iter().filter(|a| matches!(a, A::SendSecret(_))).count(),
        1
    );
}

#[test]
fn a_second_request_for_the_username_is_refused() {
    let got = run(
        true,
        &[
            ev(Event::UsernamePrompt),
            ev(Event::PasswordPrompt),
            ev(Event::UsernamePrompt),
        ],
    );
    assert_eq!(got.last(), Some(&A::Failed(F::AskedAgain("username"))));
}

#[test]
fn a_step_that_waits_too_long_times_out_naming_what_it_waited_for() {
    let t0 = Instant::now();
    let mut l = Login::new(settings(true), t0);
    assert!(l.on_tick(t0 + Duration::from_secs(29)).is_empty());
    assert_eq!(
        l.on_tick(t0 + Duration::from_secs(31)),
        [A::Failed(F::Timeout {
            waiting_for: "the username prompt"
        })]
    );
}

#[test]
fn progress_resets_the_timeout() {
    let t0 = Instant::now();
    let mut l = Login::new(settings(true), t0);
    l.on_item(&ev(Event::UsernamePrompt), t0 + Duration::from_secs(20));
    assert!(l.on_tick(t0 + Duration::from_secs(45)).is_empty());
    assert_eq!(
        l.on_tick(t0 + Duration::from_secs(51)),
        [A::Failed(F::Timeout {
            waiting_for: "the password prompt"
        })]
    );
}

#[test]
fn a_finished_login_does_nothing_more() {
    let t0 = Instant::now();
    let mut l = Login::new(settings(true), t0);
    for i in [
        ev(Event::UsernamePrompt),
        ev(Event::PasswordPrompt),
        ev(Event::MainMenu),
        ev(Event::MudMenu),
        prompt(35),
    ] {
        l.on_item(&i, t0);
    }
    assert!(l.is_done());
    assert!(l.on_item(&ev(Event::UsernamePrompt), t0).is_empty());
    assert!(l.on_tick(t0 + Duration::from_secs(600)).is_empty());
}

#[test]
fn the_password_never_shows_when_the_settings_are_printed() {
    let shown = format!("{:?}", settings(true));
    assert!(!shown.contains("hunter22"), "{shown}");
}
