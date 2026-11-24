//! The login state machine (design §6.2), over silvermere's dialogue
//! (`silvermere-auth/messages/auth.toml`, `docs/auth.md`):
//!
//! | Waits for | Sends |
//! |---|---|
//! | `Username:` | the account name |
//! | `Password:` | the password |
//! | `Disconnect the other connection and log in here? (Y/N):` | `Y`, or `N` and stop |
//! | `Make your selection (X to exit):` | `M` |
//! | `[MAJORMUD]:` | `E` |
//! | a game prompt | done |
//!
//! Pure logic: parsed items and a clock in, actions out, so it is tested without sockets.

use std::fmt;
use std::time::{Duration, Instant};

use mp_parse::{Event, Item};

/// What the login needs. Printing it never shows the password.
#[derive(Clone)]
pub struct LoginSettings {
    pub account: String,
    pub password: String,
    /// Answer Y to "already logged on".
    pub takeover: bool,
    /// How long a step may wait for the game before the login fails.
    pub step_timeout: Duration,
}

impl fmt::Debug for LoginSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoginSettings")
            .field("account", &self.account)
            .field("password", &"********")
            .field("takeover", &self.takeover)
            .field("step_timeout", &self.step_timeout)
            .finish()
    }
}

/// Why a login stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginFailure {
    InvalidCredentials,
    AccountLocked {
        minutes: u32,
    },
    NoPassword,
    /// Already logged on, and `takeover: false`.
    AlreadyLoggedOn,
    /// silvermere's other connection didn't close in time; try again in a minute.
    TakeoverFailed,
    /// The game asked again for something already given: the credential guard.
    AskedAgain(&'static str),
    Timeout {
        waiting_for: &'static str,
    },
}

impl fmt::Display for LoginFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCredentials => f.write_str("the account name or password was refused"),
            Self::AccountLocked { minutes } => {
                write!(f, "the account is locked for {minutes} more minute(s)")
            }
            Self::NoPassword => f.write_str("the account has no password yet"),
            Self::AlreadyLoggedOn => {
                f.write_str("the account is already logged on, and takeover is off")
            }
            Self::TakeoverFailed => f.write_str("the other connection didn't close in time"),
            Self::AskedAgain(what) => write!(f, "the game asked for the {what} a second time"),
            Self::Timeout { waiting_for } => write!(f, "timed out waiting for {waiting_for}"),
        }
    }
}

/// What the session must do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginAction {
    /// Send a line.
    Send(String),
    /// Send a line that must never be logged (the password).
    SendSecret(String),
    /// The game prompt has come: logged in.
    InGame,
    Failed(LoginFailure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Username,
    Password,
    /// After the password: the menus, the takeover question, a refusal.
    Menus,
    /// `E` sent: the game prompt comes next.
    Prompt,
    Done,
}

impl Step {
    fn waiting_for(self) -> &'static str {
        match self {
            Self::Username => "the username prompt",
            Self::Password => "the password prompt",
            Self::Menus => "the game menu",
            Self::Prompt | Self::Done => "the game prompt",
        }
    }
}

/// One login.
#[derive(Debug)]
pub struct Login {
    settings: LoginSettings,
    step: Step,
    since: Instant,
}

impl Login {
    pub fn new(settings: LoginSettings, now: Instant) -> Self {
        Self {
            settings,
            step: Step::Username,
            since: now,
        }
    }

    /// Logged in, or stopped.
    pub fn is_done(&self) -> bool {
        self.step == Step::Done
    }

    fn go(&mut self, step: Step, now: Instant) {
        self.step = step;
        self.since = now;
    }

    fn fail(&mut self, f: LoginFailure) -> Vec<LoginAction> {
        self.step = Step::Done;
        vec![LoginAction::Failed(f)]
    }

    /// The next item the game sent.
    pub fn on_item(&mut self, item: &Item, now: Instant) -> Vec<LoginAction> {
        use LoginAction as A;
        let step = self.step;
        if step == Step::Done {
            return Vec::new();
        }
        match item {
            Item::Event(Event::UsernamePrompt) if step == Step::Username => {
                self.go(Step::Password, now);
                vec![A::Send(self.settings.account.clone())]
            }
            Item::Event(Event::UsernamePrompt) => self.fail(LoginFailure::AskedAgain("username")),
            Item::Event(Event::PasswordPrompt) if step == Step::Password => {
                self.go(Step::Menus, now);
                vec![A::SendSecret(self.settings.password.clone())]
            }
            Item::Event(Event::PasswordPrompt) => self.fail(LoginFailure::AskedAgain("password")),
            Item::Event(Event::TakeoverQuestion) if self.settings.takeover => {
                self.since = now;
                vec![A::Send("Y".into())]
            }
            Item::Event(Event::TakeoverQuestion) => {
                let mut out = vec![A::Send("N".into())];
                out.extend(self.fail(LoginFailure::AlreadyLoggedOn));
                out
            }
            Item::Event(Event::InvalidCredentials) => self.fail(LoginFailure::InvalidCredentials),
            Item::Event(Event::AccountLocked { minutes }) => {
                self.fail(LoginFailure::AccountLocked { minutes: *minutes })
            }
            Item::Event(Event::NoPassword) => self.fail(LoginFailure::NoPassword),
            Item::Event(Event::TakeoverFailed) => self.fail(LoginFailure::TakeoverFailed),
            Item::Event(Event::MainMenu) if step == Step::Menus => {
                self.since = now;
                vec![A::Send("M".into())]
            }
            Item::Event(Event::MudMenu) if step == Step::Menus => {
                self.go(Step::Prompt, now);
                vec![A::Send("E".into())]
            }
            Item::Prompt(_) if step == Step::Prompt => {
                self.step = Step::Done;
                vec![A::InGame]
            }
            _ => Vec::new(),
        }
    }

    /// The clock moved on: a step that waited past `step_timeout` fails.
    pub fn on_tick(&mut self, now: Instant) -> Vec<LoginAction> {
        if self.step != Step::Done && now.duration_since(self.since) > self.settings.step_timeout {
            let waiting_for = self.step.waiting_for();
            return self.fail(LoginFailure::Timeout { waiting_for });
        }
        Vec::new()
    }
}
