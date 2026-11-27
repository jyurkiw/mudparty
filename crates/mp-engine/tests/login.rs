//! The login state machine (design §6.2): silvermere's dialogue, the takeover question, the
//! credential guard and the step timeout. Pure logic over parsed items and a clock.

use std::time::{Duration, Instant};

use mp_engine::{Login, LoginAction as A, LoginFailure as F, LoginSettings, ScriptStep};
use mp_parse::{Event, Item, Rest, Traced, Vitals};

fn settings(takeover: bool) -> LoginSettings {
    LoginSettings {
        account: "ermias".into(),
        password: "hunter22".into(),
        takeover,
        step_timeout: Duration::from_secs(30),
        script: None,
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
            waiting_for: "the username prompt".into()
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
            waiting_for: "the password prompt".into()
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

// ----- a dialogue written in the character's file (#33)

fn step(wait: &str, send: &str) -> ScriptStep {
    ScriptStep {
        wait: wait.into(),
        send: send.into(),
    }
}

/// A login of the user's own: a name, a secret word, a key to go on.
fn scripted() -> LoginSettings {
    LoginSettings {
        script: Some(vec![
            step("Enter your name:", "{account}"),
            step("Secret word:", "{password}"),
            step("Press any key", ""),
        ]),
        ..settings(true)
    }
}

/// A line read, as the reader gives it: `item`, from the text `plain`.
fn line(item: Item, plain: &str) -> Traced {
    Traced {
        item,
        plain: plain.into(),
    }
}

fn text(plain: &str) -> Traced {
    line(Item::Text(plain.into()), plain)
}

fn run_script(lines: &[Traced]) -> Vec<A> {
    let t0 = Instant::now();
    let mut l = Login::new(scripted(), t0);
    lines.iter().flat_map(|t| l.on_traced(t, t0)).collect()
}

#[test]
fn a_written_dialogue_is_followed_step_by_step() {
    let got = run_script(&[
        text("Welcome, traveller."),
        text("Enter your name: "),
        text("Secret word: "),
        text("Press any key to go on."),
        line(prompt(35), "[HP=35]:"),
    ]);
    assert_eq!(
        got,
        [
            A::Send("ermias".into()),
            // The password is never logged.
            A::SendSecret("hunter22".into()),
            A::Send("".into()),
            A::InGame,
        ]
    );
}

#[test]
fn each_step_waits_for_its_own_text_in_turn() {
    // The second step's text before the first's: nothing yet.
    assert_eq!(run_script(&[text("Secret word: ")]), []);
    // A game prompt before the last step isn't the game yet.
    assert_eq!(
        run_script(&[text("Enter your name: "), line(prompt(35), "[HP=35]:")]),
        [A::Send("ermias".into())]
    );
}

#[test]
fn the_credential_guard_holds_in_a_written_dialogue() {
    let got = run_script(&[
        text("Enter your name: "),
        text("Secret word: "),
        text("Secret word: "),
    ]);
    assert_eq!(got.last(), Some(&A::Failed(F::AskedAgain("password"))));
    assert_eq!(
        got.iter().filter(|a| matches!(a, A::SendSecret(_))).count(),
        1
    );
    let got = run_script(&[text("Enter your name: "), text("Enter your name: ")]);
    assert_eq!(got.last(), Some(&A::Failed(F::AskedAgain("username"))));
}

#[test]
fn the_games_refusals_stop_a_written_dialogue_too() {
    let got = run_script(&[
        text("Enter your name: "),
        text("Secret word: "),
        line(ev(Event::InvalidCredentials), "Invalid Credentials"),
    ]);
    assert_eq!(got.last(), Some(&A::Failed(F::InvalidCredentials)));
}

#[test]
fn a_written_step_that_waits_too_long_names_its_text() {
    let t0 = Instant::now();
    let mut l = Login::new(scripted(), t0);
    l.on_traced(&text("Enter your name: "), t0);
    assert_eq!(
        l.on_tick(t0 + Duration::from_secs(31)),
        [A::Failed(F::Timeout {
            waiting_for: "`Secret word:`".into()
        })]
    );
}

#[test]
fn a_login_tells_the_reader_what_it_waits_for() {
    let t0 = Instant::now();
    assert_eq!(
        Login::new(scripted(), t0).waits(),
        ["Enter your name:", "Secret word:", "Press any key"]
    );
    assert!(Login::new(settings(true), t0).waits().is_empty());
}
