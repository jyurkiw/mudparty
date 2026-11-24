//! One character's link to the game (design §3.1, §6.2, §4.4): connect, log in, set the
//! statline, take config changes, log off, and reconnect a lost link.
//!
//! A session runs as a tokio task, driven by [`Command`]s and telling its owner what it is
//! doing with [`Report`]s. The same runner serves a headless bot in the server and, later,
//! the interactive client.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use mp_log::{CharacterLogs, Header, RollConfig, Source, SystemClock};
use mp_parse::{Event, Item, Reader, Statline, Vitals};
use mp_telnet::ClientCodec;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{MissedTickBehavior, interval, sleep, timeout};

use crate::login::{Login, LoginAction, LoginFailure, LoginSettings};

/// What a session needs to run.
#[derive(Debug, Clone)]
pub struct SessionSpec {
    /// The character's first name.
    pub name: String,
    /// The game's `host:port`.
    pub game: String,
    pub login: LoginSettings,
    /// The statline template to set once in the game.
    pub statline: String,
    pub config_rev: String,
    /// How long the realm exit may take before the link is dropped.
    pub logoff_timeout: Duration,
    /// Waits before each reconnect; the last one repeats.
    pub reconnect_delays: Vec<Duration>,
    /// Where the logs go (`<logs>/<name>/`), or none.
    pub logs: Option<PathBuf>,
}

/// What the owner can ask of a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Leave the realm cleanly and stop.
    Logoff,
    /// The character's config changed: a new statline and revision.
    Config { statline: String, rev: String },
}

/// What a session is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    Connecting,
    LoggingIn,
    InGame,
    LoggingOff,
    /// The link was lost; trying again after this wait.
    Reconnecting {
        after: Duration,
    },
    Offline(Offline),
}

/// Why a session stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offline {
    LoggedOff,
    LoginFailed(LoginFailure),
    /// Someone else logged the character in; not reconnected, so as not to fight them.
    TakenOver,
    /// The realm exit didn't finish in time; the link was dropped.
    LogoffTimedOut,
    /// The owner went away (its command channel closed).
    Dropped,
}

/// What a session tells its owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Report {
    State(SessionState),
    /// A prompt whose values changed.
    Vitals(Vitals),
    /// A config change is in use.
    ConfigApplied {
        rev: String,
    },
}

/// A running session.
#[derive(Debug)]
pub struct SessionHandle {
    cmds: mpsc::UnboundedSender<Command>,
    task: JoinHandle<Offline>,
}

impl SessionHandle {
    /// Sends a command; a session that has stopped ignores it.
    pub fn send(&self, c: Command) {
        let _ = self.cmds.send(c);
    }

    /// Waits for the session to stop.
    pub async fn finished(self) -> Offline {
        self.task.await.unwrap_or(Offline::Dropped)
    }

    /// Whether the session has stopped.
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
}

/// Starts a session.
pub fn spawn(spec: SessionSpec, reports: mpsc::UnboundedSender<Report>) -> SessionHandle {
    let (cmds, rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(run(spec, rx, reports));
    SessionHandle { cmds, task }
}

/// How one link ended.
enum LinkEnd {
    Done(Offline),
    Lost { was_in_game: bool },
}

struct Runner {
    spec: SessionSpec,
    reports: mpsc::UnboundedSender<Report>,
    logs: Option<CharacterLogs>,
}

impl Runner {
    fn report(&self, r: Report) {
        let _ = self.reports.send(r);
    }

    fn state(&self, s: SessionState) {
        self.report(Report::State(s));
    }

    // A log that can't be written must never stop a bot, so log errors are dropped here; the
    // server log reports a logs folder it can't write when the session starts.
    fn event(&mut self, kind: &str, fields: serde_json::Value) {
        if let Some(l) = &mut self.logs {
            let _ = l.event(kind, fields);
        }
    }

    fn decision(&mut self, kind: &str, fields: serde_json::Value) {
        if let Some(l) = &mut self.logs {
            let _ = l.decision(kind, fields);
        }
    }

    async fn send(
        &mut self,
        stream: &mut TcpStream,
        line: &str,
        secret: bool,
    ) -> std::io::Result<()> {
        let bytes = format!("{line}\r\n").into_bytes();
        if let Some(l) = &mut self.logs {
            if secret {
                let _ = l.wire_out_redacted(&bytes, "password", &Source::Engine);
                let _ = l.transcript_sent_redacted(&Source::Engine);
            } else {
                let _ = l.wire_out(&bytes, &Source::Engine);
                let _ = l.transcript_sent(line, &Source::Engine);
            }
        }
        let mut out = Vec::with_capacity(bytes.len());
        mp_telnet::escape(&bytes, &mut out);
        stream.write_all(&out).await
    }

    async fn set_statline(
        &mut self,
        stream: &mut TcpStream,
        reader: &mut Reader,
    ) -> std::io::Result<()> {
        let template = self.spec.statline.clone();
        self.send(
            stream,
            &format!("set statline full custom {template}"),
            false,
        )
        .await?;
        match Statline::compile(&template) {
            Ok(s) => reader.set_statline(s),
            // The config loader checks the template, so this is a template that skipped it.
            Err(e) => self.decision(
                "statline_unreadable",
                json!({"template": template, "error": e.to_string()}),
            ),
        }
        self.decision("statline_set", json!({"template": template}));
        Ok(())
    }

    /// One connection, from connecting to its end.
    async fn link(&mut self, cmds: &mut mpsc::UnboundedReceiver<Command>) -> LinkEnd {
        let lost = LinkEnd::Lost { was_in_game: false };
        let Ok(Ok(mut stream)) = timeout(
            self.spec.login.step_timeout,
            TcpStream::connect(&self.spec.game),
        )
        .await
        else {
            return lost;
        };
        self.state(SessionState::LoggingIn);
        self.event("connected", json!({"game": self.spec.game}));
        let mut telnet = ClientCodec::new(80, 24);
        // silvermere saves the statline with the character, so one this bot played before
        // comes back with the bot's own statline; read that as well as the defaults.
        let mut shapes = Statline::silvermere_defaults();
        if let Ok(own) = Statline::compile(&self.spec.statline) {
            shapes.insert(0, own);
        }
        let mut reader = Reader::with_statlines(shapes);
        let mut login = Login::new(self.spec.login.clone(), Instant::now());
        let mut in_game = false;
        let mut last_vitals: Option<Vitals> = None;
        // While logging off: when to give up, and how many `quit`s were sent.
        let mut logging_off: Option<(Instant, u32)> = None;
        let mut tick = interval(Duration::from_millis(200));
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut buf = [0u8; 8192];
        loop {
            tokio::select! {
                read = stream.read(&mut buf) => {
                    let n = match read {
                        Ok(0) | Err(_) if logging_off.is_some() => return LinkEnd::Done(Offline::LoggedOff),
                        Ok(0) | Err(_) => return LinkEnd::Lost { was_in_game: in_game },
                        Ok(n) => n,
                    };
                    if let Some(l) = &mut self.logs {
                        let _ = l.wire_in(&buf[..n]);
                    }
                    let (mut data, mut reply) = (Vec::new(), Vec::new());
                    telnet.decode(&buf[..n], &mut data, &mut reply);
                    if stream.write_all(&reply).await.is_err() {
                        return LinkEnd::Lost { was_in_game: in_game };
                    }
                    for t in reader.feed_traced(&data) {
                        if let Some(l) = &mut self.logs {
                            let _ = l.transcript_line(&t.plain);
                        }
                        if !in_game {
                            for action in login.on_item(&t.item, Instant::now()) {
                                self.decision("login_step", json!({"read": t.plain, "action": format!("{action:?}").replace(&self.spec.login.password, "********")}));
                                match action {
                                    LoginAction::Send(line) => {
                                        if self.send(&mut stream, &line, false).await.is_err() {
                                            return lost;
                                        }
                                    }
                                    LoginAction::SendSecret(line) => {
                                        if self.send(&mut stream, &line, true).await.is_err() {
                                            return lost;
                                        }
                                    }
                                    LoginAction::InGame => {
                                        in_game = true;
                                        self.state(SessionState::InGame);
                                        self.event("in_game", json!({}));
                                        if self.set_statline(&mut stream, &mut reader).await.is_err() {
                                            return LinkEnd::Lost { was_in_game: true };
                                        }
                                    }
                                    LoginAction::Failed(f) => {
                                        self.event("login_failed", json!({"why": f.to_string()}));
                                        return LinkEnd::Done(Offline::LoginFailed(f));
                                    }
                                }
                            }
                            continue;
                        }
                        match &t.item {
                            Item::Prompt(v) if last_vitals != Some(*v) => {
                                last_vitals = Some(*v);
                                self.report(Report::Vitals(*v));
                            }
                            Item::Event(Event::TakenOver) => return LinkEnd::Done(Offline::TakenOver),
                            // Out of the realm: the board's `[MAJORMUD]:` prompt.
                            Item::Event(Event::MudMenu) if logging_off.is_some() => {
                                return LinkEnd::Done(Offline::LoggedOff);
                            }
                            // An attack called the exit off: ask again, a few times.
                            Item::Event(Event::ExitInterrupted) => {
                                if let Some((deadline, sent)) = logging_off
                                    && sent < 3
                                {
                                    logging_off = Some((deadline, sent + 1));
                                    self.decision("logoff_retry", json!({"try": sent + 1}));
                                    if self.send(&mut stream, "quit", false).await.is_err() {
                                        return LinkEnd::Done(Offline::LoggedOff);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                cmd = cmds.recv() => match cmd {
                    None => return LinkEnd::Done(Offline::Dropped),
                    Some(Command::Logoff) if !in_game => return LinkEnd::Done(Offline::LoggedOff),
                    Some(Command::Logoff) => {
                        if logging_off.is_none() {
                            logging_off = Some((Instant::now() + self.spec.logoff_timeout, 1));
                            self.state(SessionState::LoggingOff);
                            self.decision("logoff", json!({}));
                            if self.send(&mut stream, "quit", false).await.is_err() {
                                return LinkEnd::Done(Offline::LoggedOff);
                            }
                        }
                    }
                    Some(Command::Config { statline, rev }) => {
                        self.spec.statline = statline;
                        self.spec.config_rev = rev.clone();
                        if in_game && self.set_statline(&mut stream, &mut reader).await.is_err() {
                            return LinkEnd::Lost { was_in_game: true };
                        }
                        self.decision("config_applied", json!({"rev": rev}));
                        self.report(Report::ConfigApplied { rev });
                    }
                },
                _ = tick.tick() => {
                    let now = Instant::now();
                    if !in_game {
                        for action in login.on_tick(now) {
                            if let LoginAction::Failed(f) = action {
                                self.event("login_failed", json!({"why": f.to_string()}));
                                return LinkEnd::Done(Offline::LoginFailed(f));
                            }
                        }
                    }
                    if let Some((deadline, _)) = logging_off
                        && now > deadline
                    {
                        return LinkEnd::Done(Offline::LogoffTimedOut);
                    }
                }
            }
        }
    }
}

async fn run(
    spec: SessionSpec,
    mut cmds: mpsc::UnboundedReceiver<Command>,
    reports: mpsc::UnboundedSender<Report>,
) -> Offline {
    let logs = spec.logs.as_ref().and_then(|root| {
        let header = Header {
            character: spec.name.clone(),
            session: 1,
            config_rev: spec.config_rev.clone(),
            gamedata_rev: String::new(),
            realm: "silvermere".into(),
        };
        CharacterLogs::open(root, header, RollConfig::default(), Box::new(SystemClock)).ok()
    });
    let mut r = Runner {
        spec,
        reports,
        logs,
    };
    let mut attempt = 0usize;
    loop {
        r.state(SessionState::Connecting);
        match r.link(&mut cmds).await {
            LinkEnd::Done(off) => {
                let kind = match &off {
                    Offline::LoggedOff => "logged_off",
                    Offline::LoginFailed(_) => "stopped_login_failed",
                    Offline::TakenOver => "taken_over",
                    Offline::LogoffTimedOut => "logoff_timed_out",
                    Offline::Dropped => "dropped",
                };
                r.event(kind, json!({}));
                r.state(SessionState::Offline(off.clone()));
                if let Some(l) = r.logs.take() {
                    let _ = l.close();
                }
                return off;
            }
            LinkEnd::Lost { was_in_game } => {
                if was_in_game {
                    attempt = 0;
                }
                let delays = &r.spec.reconnect_delays;
                let after = delays
                    .get(attempt)
                    .or(delays.last())
                    .copied()
                    .unwrap_or(Duration::from_secs(5));
                attempt += 1;
                r.event(
                    "link_lost",
                    json!({"reconnect_after_ms": after.as_millis() as u64}),
                );
                r.state(SessionState::Reconnecting { after });
                tokio::select! {
                    _ = sleep(after) => {}
                    cmd = cmds.recv() => match cmd {
                        None | Some(Command::Logoff) => {
                            let off = if cmd.is_none() { Offline::Dropped } else { Offline::LoggedOff };
                            r.state(SessionState::Offline(off.clone()));
                            return off;
                        }
                        Some(Command::Config { statline, rev }) => {
                            r.spec.statline = statline;
                            r.spec.config_rev = rev.clone();
                            r.report(Report::ConfigApplied { rev });
                        }
                    }
                }
            }
        }
    }
}
