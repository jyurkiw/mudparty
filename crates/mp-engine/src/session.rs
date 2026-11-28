//! One character's link to the game (design §3.1, §6.2, §4.4): connect, log in, set the
//! statline, take config changes, log off, and reconnect a lost link.
//!
//! A session runs as a tokio task, driven by [`Command`]s and telling its owner what it is
//! doing with [`Report`]s. The same runner serves a headless bot in the server, a MudPlay
//! seat on the proxy (design §2.1, [`spawn_seated`]) and, later, the interactive client.
//!
//! A seated session's driver is the user: MudPlay logs in and plays, every byte passes
//! through unchanged both ways, and the engines read what the game says (the tap) but
//! decide nothing.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use mp_config::{Amount, Automation};
use mp_log::{CharacterLogs, Header, Source, StreamRolls, SystemClock};
use mp_parse::{Event, Item, Reader, Statline, Vitals};
use mp_telnet::ClientCodec;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{MissedTickBehavior, interval, sleep, timeout};

use crate::capture::{Captured, Capturer};
use crate::combat::{Combat, CombatSettings};
use crate::history::{HistLine, HistoryFilter};
use crate::login::{Login, LoginAction, LoginFailure, LoginSettings};
use crate::pacing::{Action, Pacer, PacingSettings};
use crate::party::{PartyJoin, PartySettings, PartyState};
use crate::remote::{
    AutoSwitch, BotView, Effect, ExpKnown, Query, Remote, RemoteSettings, RoomBook, exp_reply,
    status_reply, where_reply,
};
use crate::rest::{RestSettings, Rester};
use crate::routine::{Goal, Routine, RoutinePart, RoutineSettings, Seen, TrainEnd};
use crate::sheet::{Experience, SheetPart, SheetReader};
use crate::shop::{EquipState, Equipper, InventoryReader};
use crate::walk::{RouteRules, WalkState, Walker};

/// How long a walk's line may go unanswered before the walker asks where it is again.
const WALK_STEP_WAIT: Duration = Duration::from_secs(10);

/// How long `i`, a `buy` or a `wield`/`wear` may go unanswered before it's sent again.
const GEAR_WAIT: Duration = Duration::from_secs(10);
/// How long the page's ask for a command's output may go unanswered (help, look).
const CAPTURE_WAIT: Duration = Duration::from_secs(15);

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
    /// How each log stream rolls (`[logging]` in `mudparty.toml`).
    pub log_rolls: StreamRolls,
    /// How to fight, or `None` for a bot that doesn't (design §6.4).
    pub combat: Option<CombatSettings>,
    /// How fast lines go to the game once in it (design §6.1).
    pub pacing: PacingSettings,
    /// Whose party to join, or `None` (design §6.5).
    pub party: Option<PartySettings>,
    /// When to rest, or `None` for a bot that doesn't (design §6.6).
    pub rest: Option<RestSettings>,
    /// Who may send @ commands, or `None` to read none (design §6.3).
    pub remote: Option<RemoteSettings>,
    /// Which engines start switched on; `@auto-…` changes them.
    pub automation: Automation,
    /// Room names and exits from the game data, for `@where` and `@status`.
    pub rooms: Option<Arc<dyn RoomBook>>,
    /// A seated character whose MudPlay has dropped hangs up when HP falls below this
    /// (`unattended.hang_up_below`).
    pub hang_up_below: Option<Amount>,
    /// Asked before the bot telepaths `@invite` to its leader (`party.invite_by_injection`):
    /// it may invite the bot another way.
    pub inviter: Option<Arc<dyn Inviter>>,
    /// Where the character's own statline is kept while the bot's is set (#34).
    pub statline_store: Option<Arc<dyn StatlineStore>>,
    /// Put the saved statline back on an orderly logoff (`statline_restore`).
    pub statline_restore: bool,
    /// A solo bot's loop (#53), or none. A seated or unattended character runs none.
    pub routine: Option<RoutineSettings>,
}

/// When the routine's lairs were last cleared, as kept, turned into this run's clock. A time
/// still to come (the clock was set back) counts as now.
fn kept_clocks(set: &RoutineSettings, now: Instant) -> BTreeMap<(u32, u32), Instant> {
    let Some(store) = &set.lair_clocks else {
        return BTreeMap::new();
    };
    let wall = SystemTime::now();
    set.lairs()
        .filter_map(|l| {
            let at = store.last_cleared(l.at)?;
            let ago = wall.duration_since(at).unwrap_or_default();
            Some((l.at, now.checked_sub(ago)?))
        })
        .collect()
}

/// Keeps a character's own statline while a bot has its own set (#34, design §6.1).
/// silvermere saves the statline with the character, so without this a character a bot
/// played keeps the bot's statline afterwards.
pub trait StatlineStore: Send + Sync + std::fmt::Debug {
    /// The statline saved for `name`, as PROFILE showed it.
    fn saved(&self, name: &str) -> Option<String>;
    fn save(&self, name: &str, original: &str);
    fn clear(&self, name: &str);
}

/// The line that puts a saved statline back. PROFILE shows `On`, `Full`, `Off`, or a
/// custom template, but not whether that template was `custom` or `full custom`; the two
/// differ only in whether a bare Enter redraws the room. A template comes back as
/// `full custom`, a new character's mode (user, 2026-11-24): a known limit.
fn restore_line(saved: &str) -> String {
    match saved.to_ascii_lowercase().as_str() {
        w @ ("on" | "full" | "off") => format!("set statline {w}"),
        _ => format!("set statline full custom {saved}"),
    }
}

/// Invites a bot into its leader's party without the bot asking in the game: the server
/// injects `invite <bot>` when the leader is played through the proxy (design §6.5).
pub trait Inviter: Send + Sync + std::fmt::Debug {
    /// Invites `bot` as `leader`, if it can; `false` and the bot telepaths `@invite`.
    fn invite(&self, leader: &str, bot: &str) -> bool;
}

/// How long a query waits for the game's answer before it is answered with what is known.
const ASK_WAIT: Duration = Duration::from_secs(5);

/// A query waiting for the game (`/room` or `exp`).
#[derive(Debug, Clone)]
struct Ask {
    query: Query,
    channel: mp_config::Channel,
    to: String,
    until: Instant,
}

/// What the bot is doing, for `@status`.
struct Doing {
    fighting: bool,
    poisoned: bool,
}

/// The switches @ commands set (design §6.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoState {
    pub combat: bool,
    pub nuke: bool,
    pub rest: bool,
    /// `@auto-all off`.
    pub all_off: bool,
    /// `@stop`, until `@rego`.
    pub stopped: bool,
    /// Who has sent `@wait` and not yet `@ok`.
    pub held_by: Vec<String>,
}

impl AutoState {
    fn new(a: Automation) -> Self {
        Self {
            combat: a.combat,
            nuke: a.nuke,
            rest: a.rest,
            all_off: false,
            stopped: false,
            held_by: Vec::new(),
        }
    }

    /// Whether the combat engine may act.
    pub fn fights(&self) -> bool {
        self.combat && !self.all_off && !self.stopped
    }

    /// Whether the rest engine may act.
    pub fn rests(&self) -> bool {
        self.rest && !self.all_off && !self.stopped
    }

    fn automation(&self) -> Automation {
        Automation {
            combat: self.combat,
            nuke: self.nuke,
            rest: self.rest,
        }
    }
}

/// What the owner can ask of a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Leave the realm cleanly and stop.
    Logoff,
    /// The character's config changed: a new statline and revision.
    Config { statline: String, rev: String },
    /// Attack this monster first, until it dies (`@kill`, the web page's kill box).
    Kill { target: String },
    /// An order from the web page, carried out as the @ command that does the same
    /// (design §6.8): a toggle, `wait`/`ok`, a line typed for the game.
    Do(Effect),
    /// A line to send as a seated character (design §2, injection). It waits until the
    /// user has no half-typed line and the last line's prompt has come back.
    Inject(String),
    /// Walk to this room, `(map, room)`, by the game data's exits (#52), keeping to
    /// `rules` (#64).
    Goto { to: (u32, u32), rules: RouteRules },
    /// Read the sheets again (#10): `st` and `i`, once the bot isn't fighting or resting.
    RefreshSheet,
    /// Send `line` and hand back its output as `Report::Captured`, under the page's `id`:
    /// `help strength`, `look club`. A seated character's line is injected.
    Capture { id: String, line: String },
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
    /// A seated session's MudPlay left before it was in the game, and the link was dropped
    /// with it.
    SeatLeft,
    /// An unattended character's HP fell below `hang_up_below`, so the link was dropped.
    HungUp {
        hp: i32,
        max_hp: Option<i32>,
    },
    /// The game closed a seated session's link. MudPlay reconnects by itself, so the
    /// session doesn't.
    SeatLinkLost,
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
    /// A line the session sent to the game (never a password).
    Sent(String),
    /// Where joining the leader's party stands.
    Party(PartyState),
    /// The bot started (`true`) or finished (`false`) a rest.
    Resting(bool),
    /// An @ command changed a switch.
    Automation(AutoState),
    /// What the bot is fighting, and with what: a cast code or `melee`.
    Fight {
        target: Option<String>,
        tier: Option<String>,
    },
    /// The room the game last said the bot is in (`/room`).
    Room {
        map: u32,
        room: u32,
    },
    /// Injected lines still waiting to go, oldest first.
    Queued(Vec<String>),
    /// MudPlay dropped: the character stays in the game, idle, under its engine.
    Unattended,
    /// Where a walk stands (#52).
    Walk(WalkState),
    /// Where the routine is (#53).
    Routine(RoutinePart),
    /// Something the routine says for the card: an item of gear skipped, and why (#55).
    RoutineNote(String),
    /// A sheet part read whole (#10): the status display or the inventory, whoever asked.
    Sheet(SheetPart),
    /// A command's output, asked for by the page (`Command::Capture`): help text, a look.
    Captured(Captured),
    /// Lines for the text history: chat, fights, comings and goings, unread text.
    History(Vec<HistLine>),
    /// Experience, for the card's XP bar: from `exp`, kept up by the kills since.
    Exp(Experience),
}

/// A routine and the goal the session is carrying out for it (#53).
struct Running {
    routine: Routine,
    /// The goal under way; `None` until the first is started, or after a pause.
    goal: Option<Goal>,
    /// The walker under way was started for the routine.
    walking: bool,
    /// The rest under way was started for the routine.
    resting: bool,
    last_look: Instant,
    shown: Option<RoutinePart>,
    /// When `i` went for the routine's gear check, while its answer is awaited.
    asked_inventory: Option<Instant>,
    inventory: InventoryReader,
    /// Buying and putting on an item, for the routine.
    equipper: Option<Equipper>,
    /// When `train` went for the routine, and whether it's gone twice (#56).
    training: Option<(Instant, bool)>,
    /// When `exp` went for the routine, while its answer is awaited.
    asked_exp: Option<Instant>,
}

/// What [`Runner::watch_states`] last reported, so only changes are.
#[derive(Default)]
struct Watched {
    party: Option<PartyState>,
    resting: bool,
    fight: (Option<String>, Option<String>),
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
    let task = tokio::spawn(run_from(spec, rx, reports, None, None));
    SessionHandle { cmds, task }
}

/// A MudPlay connection attached to a session (design §2.1).
#[derive(Debug)]
pub struct Seat {
    /// What MudPlay sent, telnet already taken out.
    pub input: mpsc::UnboundedReceiver<SeatInput>,
    /// Data bytes from the game, for the seat to write to MudPlay.
    pub output: mpsc::UnboundedSender<Vec<u8>>,
}

/// Something MudPlay sent through its seat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeatInput {
    /// Typed bytes, as MudPlay sent them (a key at a time, or a line).
    Data(Vec<u8>),
    /// MudPlay's window size (NAWS).
    Window { cols: u16, rows: u16 },
}

/// A link to the game opened before its session started: the proxy opens one while MudPlay
/// types its account name, and hands it over once the name says whose session it is.
#[derive(Debug)]
pub struct Upstream {
    pub stream: TcpStream,
    /// The link's telnet state, as negotiated so far.
    pub telnet: ClientCodec,
}

/// Starts a session on `upstream`, driven by a MudPlay seat. MudPlay goes on logging in by
/// itself; the session is in the game at the first prompt it reads.
pub fn spawn_seated(
    spec: SessionSpec,
    upstream: Upstream,
    seat: Seat,
    reports: mpsc::UnboundedSender<Report>,
) -> SessionHandle {
    let (cmds, rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(run_from(spec, rx, reports, Some(upstream), Some(seat)));
    SessionHandle { cmds, task }
}

/// Lines injected into a seated session, waiting for their moment.
#[derive(Debug, Default)]
struct Injector {
    queue: VecDeque<String>,
    /// A line went to the game (typed or injected) and its prompt hasn't come back yet.
    awaiting_prompt: bool,
}

/// What the seat is in the middle of typing, for the logs.
#[derive(Debug, Default)]
struct Typing {
    line: Vec<u8>,
    /// The game asked for the password, so the line being typed is one.
    secret: bool,
    /// The last byte ended a line with CR, so an LF after it is the same line end.
    after_cr: bool,
}

/// Waits for the seat's next input; forever when there is no seat.
async fn from_seat(seat: &mut Option<Seat>) -> Option<Option<SeatInput>> {
    match seat {
        Some(s) => Some(s.input.recv().await),
        None => std::future::pending().await,
    }
}

/// How one link ended.
enum LinkEnd {
    Done(Offline),
    Lost { was_in_game: bool },
}

struct Runner {
    spec: SessionSpec,
    /// Kept across reconnects: a switch a player set stays set.
    auto: AutoState,
    reports: mpsc::UnboundedSender<Report>,
    logs: Option<CharacterLogs>,
    /// Queries waiting for the game.
    asks: Vec<Ask>,
    /// This session's experience, and since when, for `@exp`.
    exp_made: i64,
    exp_since: Option<Instant>,
    exp_known: Option<ExpKnown>,
    /// The character's level, as `exp` or a training last said (#56).
    level: Option<u32>,
    last_vitals: Option<Vitals>,
    /// What goes in the text history: told what the bot sends, so its echoes stay out.
    history: HistoryFilter,
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
        if !secret {
            self.history.sent(line);
            self.report(Report::Sent(line.to_owned()));
        }
        let mut out = Vec::with_capacity(bytes.len());
        mp_telnet::escape(&bytes, &mut out);
        stream.write_all(&out).await
    }

    /// Logs what the seat typed: the raw bytes on the wire stream, each finished line on the
    /// transcript. A line typed at the password prompt is kept by its length only. Returns
    /// whether a line was finished.
    fn typed(&mut self, typing: &mut Typing, bytes: &[u8]) -> bool {
        let secret = typing.secret;
        let mut lines = Vec::new();
        for &b in bytes {
            match b {
                b'\n' if typing.after_cr => typing.after_cr = false,
                b'\r' | b'\n' => {
                    typing.after_cr = b == b'\r';
                    lines.push((std::mem::take(&mut typing.line), typing.secret));
                    typing.secret = false;
                }
                // Backspace and delete take back the last key.
                8 | 127 => {
                    typing.after_cr = false;
                    typing.line.pop();
                }
                _ => {
                    typing.after_cr = false;
                    typing.line.push(b);
                }
            }
        }
        let finished = !lines.is_empty();
        let Some(l) = &mut self.logs else {
            return finished;
        };
        if secret {
            let _ = l.wire_out_redacted(bytes, "password", &Source::User);
        } else {
            let _ = l.wire_out(bytes, &Source::User);
        }
        for (line, secret) in lines {
            if secret {
                let _ = l.transcript_sent_redacted(&Source::User);
            } else {
                let _ = l.transcript_sent(&String::from_utf8_lossy(&line), &Source::User);
            }
        }
        finished
    }

    /// Sends the next injected line, if this is its moment: the user has no half-typed
    /// line, and the last line's prompt has come back.
    async fn inject_next(
        &mut self,
        stream: &mut TcpStream,
        inject: &mut Injector,
        typing: &Typing,
    ) -> std::io::Result<()> {
        if inject.awaiting_prompt || !typing.line.is_empty() {
            return Ok(());
        }
        let Some(line) = inject.queue.pop_front() else {
            return Ok(());
        };
        inject.awaiting_prompt = true;
        let bytes = format!("{line}\r\n").into_bytes();
        if let Some(l) = &mut self.logs {
            let _ = l.wire_out(&bytes, &Source::Inject);
            let _ = l.transcript_sent(&line, &Source::Inject);
        }
        self.decision("injected", json!({"line": line}));
        self.report(Report::Sent(line));
        self.report(Report::Queued(inject.queue.iter().cloned().collect()));
        let mut out = Vec::with_capacity(bytes.len());
        mp_telnet::escape(&bytes, &mut out);
        stream.write_all(&out).await
    }

    /// Carries out what an @ command asked for.
    fn apply(
        &mut self,
        e: Effect,
        pacer: &mut Pacer,
        combat: &mut Option<Combat>,
        party: &mut Option<PartyJoin>,
        now: Instant,
    ) {
        if let Some(a) = e.action() {
            self.queue(pacer, "remote", vec![a]);
        }
        let before = self.auto.clone();
        match e {
            Effect::Reply { .. } | Effect::Send(_) => {}
            Effect::Ask { query, channel, to } => {
                let line = if query == Query::Exp { "exp" } else { "/room" };
                // One question to the game answers every query waiting on it.
                let asked = self
                    .asks
                    .iter()
                    .any(|a| (a.query == Query::Exp) == (query == Query::Exp));
                if !asked {
                    pacer.push(line);
                }
                self.asks.push(Ask {
                    query,
                    channel,
                    to,
                    until: now + ASK_WAIT,
                });
            }
            Effect::Kill(target) => {
                self.decision("kill_order", json!({"target": target}));
                if let Some(c) = combat {
                    c.order_kill(&target);
                    if self.auto.fights() {
                        for cmd in c.act(now) {
                            self.decision("attack", json!({"command": cmd, "target": target}));
                            pacer.push(cmd);
                        }
                    }
                }
            }
            Effect::SetAuto(which, on) => match which {
                AutoSwitch::Combat => self.auto.combat = on,
                AutoSwitch::Nuke => self.auto.nuke = on,
                AutoSwitch::Rest => self.auto.rest = on,
            },
            Effect::AllOff(off) => self.auto.all_off = off,
            Effect::Stop(stop) => self.auto.stopped = stop,
            Effect::Wait(who) => {
                let mut held: BTreeSet<String> = self.auto.held_by.drain(..).collect();
                held.insert(who);
                self.auto.held_by = held.into_iter().collect();
            }
            Effect::Ok(who) => self.auto.held_by.retain(|h| !h.eq_ignore_ascii_case(&who)),
            Effect::Join(leader) => {
                let retry = self
                    .spec
                    .party
                    .as_ref()
                    .map_or(Duration::from_secs(30), |p| p.retry);
                let mut p = PartyJoin::new(PartySettings {
                    name: self.spec.name.clone(),
                    leader,
                    retry,
                });
                let acts = p.start(now);
                self.queue(pacer, "party", acts);
                *party = Some(p);
            }
        }
        if self.auto != before {
            if let Some(c) = combat {
                c.set_nuke(self.auto.nuke);
                c.set_hold(!self.auto.held_by.is_empty());
            }
            self.event("automation", json!({"state": format!("{:?}", self.auto)}));
            self.report(Report::Automation(self.auto.clone()));
        }
    }

    /// Answers the waiting queries `which` picks.
    fn answer(
        &mut self,
        pacer: &mut Pacer,
        which: impl Fn(Query) -> bool,
        room: Option<(u32, u32)>,
        doing: &Doing,
    ) {
        let (now, rest): (Vec<Ask>, Vec<Ask>) = self.asks.drain(..).partition(|a| which(a.query));
        self.asks = rest;
        for a in now {
            self.reply_to(pacer, a, room, doing);
        }
    }

    /// The reply to one query, from what is known now.
    fn reply_to(&mut self, pacer: &mut Pacer, a: Ask, room: Option<(u32, u32)>, doing: &Doing) {
        let info = |key: (u32, u32)| self.spec.rooms.as_ref().and_then(|b| b.room(key.0, key.1));
        let text = match a.query {
            Query::Where => match room {
                Some(key) => Some(where_reply(key, info(key).as_ref())),
                // A refusal, said only with warn_denied.
                None => self
                    .spec
                    .remote
                    .as_ref()
                    .is_some_and(|r| r.warn_denied)
                    .then(|| "Location unknown".to_owned()),
            },
            Query::Status => {
                let activity = if doing.fighting {
                    "fighting"
                } else {
                    match self.last_vitals.map(|v| v.rest) {
                        Some(mp_parse::Rest::Resting) => "resting",
                        Some(mp_parse::Rest::Meditating) => "meditating",
                        _ => "idle",
                    }
                };
                let ailments: &[&str] = if doing.poisoned { &["poisoned"] } else { &[] };
                let found = room.map(|k| (k, info(k)));
                Some(status_reply(
                    activity,
                    found.as_ref().map(|(k, i)| (*k, i.as_ref())),
                    ailments,
                ))
            }
            Query::Exp => {
                let hours = self
                    .exp_since
                    .map_or(0.0, |at| at.elapsed().as_secs_f64() / 3600.0);
                let rate = if hours > 0.0 && self.exp_made > 0 {
                    self.exp_made as f64 / hours
                } else {
                    0.0
                };
                Some(exp_reply(self.exp_made, rate, self.exp_known))
            }
        };
        if let Some(text) = text {
            let reply = Effect::Reply {
                channel: a.channel,
                to: a.to,
                text,
            };
            if let Some(act) = reply.action() {
                self.queue(pacer, "remote", vec![act]);
            }
        }
    }

    /// Queues what an engine wants sent.
    fn queue(&mut self, pacer: &mut Pacer, engine: &str, acts: Vec<Action>) {
        for a in acts {
            // A bot's ask to join, answered by the server instead when it can.
            if let Action::Telepath { to, text } = &a
                && engine == "party"
                && text == "@invite"
                && let Some(i) = &self.spec.inviter
                && i.invite(to, &self.spec.name)
            {
                self.decision("invite_by_injection", json!({"leader": to}));
                continue;
            }
            self.decision(engine, json!({"action": format!("{a:?}")}));
            match a {
                Action::Send(l) => pacer.push(l),
                Action::Telepath { to, text } => pacer.telepath(&to, &text),
            }
        }
    }

    /// Sends what the pacer lets go now.
    async fn drain(&mut self, stream: &mut TcpStream, pacer: &mut Pacer) -> std::io::Result<()> {
        loop {
            let lines = pacer.ready(Instant::now());
            if lines.is_empty() {
                return Ok(());
            }
            for l in lines {
                self.send(stream, &l, false).await?;
            }
        }
    }

    /// Reports a walk's state when it changes, and lets a finished walk go, returning how
    /// it ended.
    fn watch_walk(
        &mut self,
        walker: &mut Option<Walker>,
        seen: &mut Option<WalkState>,
    ) -> Option<WalkState> {
        let w = walker.as_ref()?;
        let now = w.state();
        if seen.as_ref() != Some(&now) {
            self.event("walk", json!({"to": format!("{}/{}", w.destination().0, w.destination().1), "state": format!("{now:?}")}));
            self.report(Report::Walk(now.clone()));
            *seen = Some(now.clone());
        }
        if w.done() {
            *walker = None;
            return Some(now);
        }
        None
    }

    /// Feeds the routine's gear steps (#55) the next item the game sent, or time passing
    /// (`item` none): the inventory it asked for, and how a buy and equip went.
    fn feed_gear(
        &mut self,
        run: &mut Running,
        item: Option<&Item>,
        pacer: &mut Pacer,
        now: Instant,
    ) {
        match (item, run.asked_inventory) {
            (Some(i), Some(_)) => {
                if let Some(carried) = run.inventory.on_item(i) {
                    run.asked_inventory = None;
                    self.decision("inventory", json!({"carried": format!("{carried:?}")}));
                    run.routine.inventory(&carried, now);
                }
            }
            (None, Some(at)) if now.duration_since(at) >= GEAR_WAIT => {
                pacer.push("i");
                run.asked_inventory = Some(now);
            }
            _ => {}
        }
        // A `train` the routine asked for (#56): its answer, or none in time.
        if let Some((at, resent)) = run.training {
            let end = match item {
                Some(Item::Event(Event::Trained { level })) => Some(TrainEnd::Trained(*level)),
                Some(Item::Event(Event::TrainNoExp)) => Some(TrainEnd::NoExp),
                Some(Item::Event(Event::TrainNoMoney)) => Some(TrainEnd::NoMoney),
                Some(Item::Event(Event::TrainRefused { text })) => {
                    Some(TrainEnd::Refused(text.clone()))
                }
                None if now.duration_since(at) >= GEAR_WAIT && resent => Some(TrainEnd::NoAnswer),
                None if now.duration_since(at) >= GEAR_WAIT => {
                    pacer.push("train");
                    run.training = Some((now, true));
                    None
                }
                _ => None,
            };
            if let Some(end) = end {
                run.training = None;
                self.decision("trained", json!({"end": format!("{end:?}")}));
                run.routine.train_ended(end, now);
            }
        }
        if item.is_some_and(|i| matches!(i, Item::Event(Event::ExpLine { .. }))) {
            run.asked_exp = None;
        }
        if let Some(e) = &mut run.equipper {
            let lines = match item {
                Some(i) => e.on_item(i, now),
                None => e.on_tick(now),
            };
            for line in lines {
                pacer.push(line);
            }
            if e.done() {
                let result = match e.state() {
                    EquipState::Failed(why) => Err(why),
                    _ => Ok(()),
                };
                self.decision("equipped", json!({"result": format!("{result:?}")}));
                run.equipper = None;
                run.routine.equip_ended(result, now);
            }
        }
    }

    /// Moves the routine on and starts the goal it wants, if that changed (#53). `ended`:
    /// how a walk that just finished ended.
    #[allow(clippy::too_many_arguments)]
    fn drive_routine(
        &mut self,
        run: &mut Running,
        ended: Option<WalkState>,
        pacer: &mut Pacer,
        walker: &mut Option<Walker>,
        walk_seen: &mut Option<WalkState>,
        combat: &mut Option<Combat>,
        rester: &mut Option<Rester>,
        vitals: Option<Vitals>,
        room: Option<(u32, u32)>,
        now: Instant,
    ) {
        let fighting = combat.as_ref().is_some_and(|c| c.target().is_some());
        let seen = Seen {
            vitals,
            fighting,
            monsters: combat
                .as_ref()
                .map_or_else(Vec::new, |c| c.monsters().to_vec()),
            room,
            walking: walker.is_some(),
            exp: self.exp_known,
        };
        // A routine that trains wants the level and experience: `exp` while unknown.
        if run.routine.wants_exp()
            && self.exp_known.is_none()
            && walker.is_none()
            && run
                .asked_exp
                .is_none_or(|at| now.duration_since(at) >= GEAR_WAIT)
        {
            pacer.push("exp");
            run.asked_exp = Some(now);
        }
        run.routine
            .set_paused(self.auto.stopped || self.auto.all_off);
        if let Some(end) = ended
            && run.walking
        {
            run.walking = false;
            run.routine.walk_ended(&end, &seen, now);
        }
        if run.resting && !rester.as_ref().is_some_and(Rester::resting) {
            run.resting = false;
            run.routine.rest_ended(now);
        }
        run.routine.update(&seen, now);
        let goal = run.routine.goal();
        if run.goal.as_ref() != Some(&goal) {
            self.decision(
                "routine_goal",
                json!({"goal": format!("{goal:?}"), "was": format!("{:?}", run.goal)}),
            );
            // A gear step the routine no longer wants is let go.
            run.equipper = None;
            run.asked_inventory = None;
            run.training = None;
            // A walk the routine started and no longer wants is called off.
            if run.walking {
                run.walking = false;
                *walker = None;
                *walk_seen = None;
                self.report(Report::Walk(WalkState::Failed("called off".into())));
            }
            match &goal {
                Goal::Walk(to) => match self.spec.rooms.clone() {
                    Some(book) => {
                        // The level exits it may take, as far as its level is known.
                        let mut rules = run.routine.route_rules().clone();
                        rules.level = self.level;
                        let mut w = Walker::new(*to, book, WALK_STEP_WAIT).with_rules(rules);
                        for line in w.start(now) {
                            pacer.push(line);
                        }
                        *walker = Some(w);
                        *walk_seen = None;
                        run.walking = true;
                    }
                    None => run.routine.walk_ended(
                        &WalkState::Failed("no game data to plan a walk on".into()),
                        &seen,
                        now,
                    ),
                },
                Goal::Rest(until) => match rester {
                    Some(r) => {
                        let acts = r.begin_until(*until);
                        self.queue(pacer, "rest", acts);
                        run.resting = true;
                    }
                    None => run.routine.rest_ended(now),
                },
                Goal::Kill(target) => {
                    if let Some(c) = combat {
                        c.order_kill(target);
                        if self.auto.fights() {
                            for cmd in c.act(now) {
                                self.decision("attack", json!({"command": cmd, "target": target}));
                                pacer.push(cmd);
                            }
                        }
                    }
                }
                Goal::Camp(_) => run.last_look = now,
                Goal::Inventory => {
                    pacer.push("i");
                    run.asked_inventory = Some(now);
                    run.inventory = InventoryReader::default();
                }
                Goal::Equip { item, how, buy } => {
                    let mut e = Equipper::new(item, *how, *buy, GEAR_WAIT);
                    for line in e.start(now) {
                        pacer.push(line);
                    }
                    run.equipper = Some(e);
                }
                Goal::Train => {
                    pacer.push("train");
                    run.training = Some((now, false));
                }
                Goal::Wait => {}
            }
            self.watch_walk(walker, walk_seen);
            run.goal = Some(goal);
        }
        // The camp's eyes: a bare Enter redraws the room, so the bot sees what arrived.
        if let Some(Goal::Camp(every)) = run.goal
            && !fighting
            && walker.is_none()
            && now.duration_since(run.last_look) >= every
        {
            run.last_look = now;
            pacer.push("");
        }
        // Each lair cleared, kept for the next run (#67).
        for (lair, at) in run.routine.take_cleared() {
            self.event(
                "lair_cleared",
                json!({"lair": format!("{}/{}", lair.0, lair.1)}),
            );
            if let Some(store) = &run.routine.settings().lair_clocks {
                let ago = Instant::now().saturating_duration_since(at);
                store.cleared(lair, SystemTime::now() - ago);
            }
        }
        for note in run.routine.take_notes() {
            self.event("routine_note", json!({"note": note}));
            self.report(Report::RoutineNote(note));
        }
        let part = run.routine.part();
        if run.shown.as_ref() != Some(&part) {
            self.event("routine", json!({"part": part.to_string()}));
            self.report(Report::Routine(part.clone()));
            run.shown = Some(part);
        }
    }

    /// Reports the party and rest states when they change.
    fn watch_states(
        &mut self,
        party: &Option<PartyJoin>,
        rester: &Option<Rester>,
        combat: &Option<Combat>,
        seen: &mut Watched,
    ) {
        let fight = combat
            .as_ref()
            .map_or((None, None), |c| (c.target().map(str::to_owned), c.tier()));
        if fight != seen.fight {
            seen.fight = fight.clone();
            self.report(Report::Fight {
                target: fight.0,
                tier: fight.1,
            });
        }
        let now_party = party.as_ref().map(PartyJoin::state);
        if now_party != seen.party {
            seen.party = now_party;
            if let Some(s) = now_party {
                self.event("party", json!({"state": format!("{s:?}")}));
                self.report(Report::Party(s));
            }
        }
        let now_resting = rester.as_ref().is_some_and(Rester::resting);
        if now_resting != seen.resting {
            seen.resting = now_resting;
            self.event("resting", json!({"resting": now_resting}));
            self.report(Report::Resting(now_resting));
        }
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

    /// Sets the bot's statline unless the character already has it, saving the character's
    /// own first (`found`, from PROFILE; `None` when PROFILE didn't answer). The bot's own
    /// template is never saved as the original: a login after a crash finds it still set,
    /// and keeps the original saved before.
    async fn settle_statline(
        &mut self,
        stream: &mut TcpStream,
        reader: &mut Reader,
        found: Option<&str>,
    ) -> std::io::Result<()> {
        let mine = self.spec.statline.clone();
        // silvermere keeps the first 60 characters of a template.
        let mine_cut: String = mine.chars().take(60).collect();
        if found == Some(mine_cut.as_str()) {
            if let Ok(s) = Statline::compile(&mine) {
                reader.set_statline(s);
            }
            self.decision("statline_kept", json!({"template": mine}));
            return Ok(());
        }
        if let (Some(found), Some(store)) = (found, &self.spec.statline_store) {
            store.save(&self.spec.name, found);
            self.decision("statline_saved", json!({"original": found}));
        }
        self.set_statline(stream, reader).await
    }

    /// The line that puts the character's saved statline back, if there is one to restore
    /// (and forgets it, since the character will have it again).
    fn restore_statline(&mut self, users_statline: bool) -> Option<String> {
        if users_statline || !self.spec.statline_restore {
            return None;
        }
        let store = self.spec.statline_store.clone()?;
        let saved = store.saved(&self.spec.name)?;
        store.clear(&self.spec.name);
        self.decision("statline_restored", json!({"original": saved}));
        Some(restore_line(&saved))
    }

    /// One connection, from connecting to its end.
    async fn link(
        &mut self,
        cmds: &mut mpsc::UnboundedReceiver<Command>,
        upstream: Option<Upstream>,
        seat: &mut Option<Seat>,
    ) -> LinkEnd {
        let lost = LinkEnd::Lost { was_in_game: false };
        let (mut stream, mut telnet) = match upstream {
            Some(u) => (u.stream, u.telnet),
            None => {
                let Ok(Ok(stream)) = timeout(
                    self.spec.login.step_timeout,
                    TcpStream::connect(&self.spec.game),
                )
                .await
                else {
                    return lost;
                };
                (stream, ClientCodec::new(80, 24))
            }
        };
        self.state(SessionState::LoggingIn);
        self.event(
            "connected",
            json!({"game": self.spec.game, "seat": seat.is_some()}),
        );
        let mut typing = Typing::default();
        let mut inject = Injector::default();
        // The user's statline stays: set on neither a seated nor an unattended link.
        let users_statline = seat.is_some();
        let mut unattended = false;
        // The most HP seen on this link: the maximum, for a statline that doesn't show it.
        let mut most_hp = 0;
        // Waiting for PROFILE's statline row, until then.
        let mut probe: Option<Instant> = None;
        // A walk under way, and the state last reported.
        let mut walker: Option<Walker> = None;
        let mut walk_seen: Option<WalkState> = None;
        // The room the game last said the bot is in.
        let mut room_now: Option<(u32, u32)> = None;
        // A solo bot's routine: a bot's own, never a seated character's.
        let mut running = self
            .spec
            .routine
            .clone()
            .filter(|_| seat.is_none())
            .map(|set| Running {
                routine: {
                    let now = Instant::now();
                    let clocks = kept_clocks(&set, now);
                    Routine::with_clocks(set, clocks, now)
                },
                goal: None,
                walking: false,
                resting: false,
                last_look: Instant::now(),
                shown: None,
                asked_inventory: None,
                inventory: InventoryReader::default(),
                equipper: None,
                training: None,
                asked_exp: None,
            });
        // silvermere saves the statline with the character, so one this bot played before
        // comes back with the bot's own statline; read that as well as the defaults.
        let mut shapes = Statline::silvermere_defaults();
        if let Ok(own) = Statline::compile(&self.spec.statline) {
            shapes.insert(0, own);
        }
        let mut reader = Reader::with_statlines(shapes);
        let mut login = Login::new(self.spec.login.clone(), Instant::now());
        // A written login (#33) waits for text of its own, in a waiting line too.
        reader.set_waits(login.waits());
        // A new link is a new view of the room: the roster starts again.
        let mut combat = self.spec.combat.clone().map(Combat::new);
        if let Some(c) = &mut combat {
            c.set_nuke(self.auto.nuke);
            c.set_hold(!self.auto.held_by.is_empty());
        }
        let remote = self.spec.remote.clone().map(Remote::new);
        let mut party = self.spec.party.clone().map(PartyJoin::new);
        let mut rester = self.spec.rest.clone().map(Rester::new);
        // With a routine, the routine decides where to rest.
        if let (Some(r), Some(_)) = (&mut rester, &running) {
            r.set_auto_start(false);
        }
        let mut pacer = Pacer::new(self.spec.pacing.clone());
        let mut seen = Watched::default();
        // The sheets (#10): read from whatever output comes; asked for after login and on
        // a refresh, once the bot is standing out of a fight.
        let mut sheet = SheetReader::default();
        let mut sheet_due = false;
        // When `exp` went for a bot that fights by level blocks and doesn't know its level.
        let mut asked_level: Option<Instant> = None;
        // Experience, from `exp` (asked with the sheets) and the kills since.
        let mut experience: Option<Experience> = None;
        // Commands whose output the page asked for.
        let mut capturer = Capturer::new(CAPTURE_WAIT);
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
                    if let Some(s) = seat
                        && !data.is_empty()
                    {
                        // A seat that has gone is noticed on its input.
                        let _ = s.output.send(data.clone());
                    }
                    for t in reader.feed_traced(&data) {
                        if let Some(l) = &mut self.logs {
                            let _ = l.transcript_line(&t.plain);
                        }
                        if in_game
                            && let Some(c) = capturer.on_line(&t.plain, matches!(t.item, Item::Prompt(_)))
                        {
                            self.report(Report::Captured(c));
                        }
                        if in_game {
                            let kept = self.history.on_item(&t.plain, &t.item);
                            if !kept.is_empty() {
                                self.report(Report::History(kept));
                            }
                        }
                        if in_game && let Some(part) = sheet.on_item(&t.item) {
                            let kind = match &part {
                                SheetPart::Status(_) => "status",
                                SheetPart::Inventory(_) => "inventory",
                            };
                            self.decision("sheet", json!({"part": kind}));
                            self.report(Report::Sheet(part));
                        }
                        if !in_game {
                            // The room shown on the way in is the first roster; the
                            // engine decides nothing until the bot is in the game.
                            if let Some(c) = &mut combat
                                && !matches!(t.item, Item::Prompt(_))
                            {
                                c.on_item(&t.item, Instant::now());
                            }
                            // MudPlay logs in by itself: the session only watches, and is in
                            // the game at the first prompt.
                            if seat.is_some() {
                                match &t.item {
                                    Item::Event(Event::PasswordPrompt) => typing.secret = true,
                                    Item::Prompt(v) => {
                                        in_game = true;
                                        self.exp_since.get_or_insert_with(Instant::now);
                                        self.state(SessionState::InGame);
                                        self.event("in_game", json!({"seat": true}));
                                        last_vitals = Some(*v);
                                        self.report(Report::Vitals(*v));
                                    }
                                    _ => {}
                                }
                                continue;
                            }
                            for action in login.on_traced(&t, Instant::now()) {
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
                                        reader.set_waits(Vec::new());
                                        self.exp_since.get_or_insert_with(Instant::now);
                                        self.state(SessionState::InGame);
                                        self.event("in_game", json!({}));
                                        // Read the character's statline before changing it.
                                        if self.send(&mut stream, "pro", false).await.is_err() {
                                            return LinkEnd::Lost { was_in_game: true };
                                        }
                                        probe = Some(Instant::now() + self.spec.login.step_timeout);
                                        // Where the bot came in, for the web page's card.
                                        pacer.push("/room");
                                        // Its sheets, once its statline is set.
                                        sheet_due = true;
                                        if let Some(p) = &mut party {
                                            let acts = p.start(Instant::now());
                                            self.queue(&mut pacer, "party", acts);
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
                        if logging_off.is_some() {
                            continue;
                        }
                        if let Item::Prompt(v) = &t.item {
                            inject.awaiting_prompt = false;
                            most_hp = most_hp.max(v.hp);
                        }
                        if unattended
                            && let Item::Prompt(v) = &t.item
                            && crate::rest::below(
                                self.spec.hang_up_below,
                                Some(v.hp),
                                v.max_hp.or((most_hp > 0).then_some(most_hp)),
                            )
                        {
                            self.decision("hang_up", json!({"hp": v.hp, "max_hp": v.max_hp, "below": format!("{:?}", self.spec.hang_up_below)}));
                            return LinkEnd::Done(Offline::HungUp { hp: v.hp, max_hp: v.max_hp });
                        }
                        let now = Instant::now();
                        for (to, text) in pacer.on_item(&t.item, now) {
                            self.decision("telepath_given_up", json!({"to": to, "text": text}));
                        }
                        self.last_vitals = last_vitals;
                        match &t.item {
                            Item::Event(Event::ExpGain { exp }) => {
                                self.exp_made += exp;
                                // Still needed for the next level, less what's gained.
                                if let Some(k) = &mut self.exp_known {
                                    k.left = (k.left - exp).max(0);
                                }
                                if let Some(x) = &mut experience {
                                    x.gain(*exp);
                                    self.report(Report::Exp(*x));
                                }
                            }
                            // A new level: what it needs isn't known until `exp` again, and
                            // the sheets have changed.
                            Item::Event(Event::Trained { level }) => {
                                self.exp_known = None;
                                self.level = Some(*level);
                                sheet_due = true;
                            }
                            Item::Event(Event::TrainNoExp) => self.exp_known = None,
                            Item::Event(Event::ExpLine { exp, level, left, need, .. }) => {
                                self.exp_known = Some(ExpKnown {
                                    level: *level,
                                    left: *left,
                                });
                                let x = Experience { exp: *exp, level: *level, left: *left, need: *need };
                                experience = Some(x);
                                self.report(Report::Exp(x));
                                self.level = Some(*level);
                                self.answer(&mut pacer, |q| q == Query::Exp, None, &Doing::of(&combat, &rester, now));
                            }
                            Item::Event(Event::ProfileStatline { mode }) if probe.is_some() => {
                                probe = None;
                                let mode = mode.clone();
                                if self.settle_statline(&mut stream, &mut reader, Some(&mode)).await.is_err() {
                                    return LinkEnd::Lost { was_in_game: true };
                                }
                            }
                            Item::Event(Event::RoomKey { map, room }) => {
                                room_now = Some((*map, *room));
                                self.report(Report::Room { map: *map, room: *room });
                                self.answer(&mut pacer, |q| q != Query::Exp, Some((*map, *room)), &Doing::of(&combat, &rester, now));
                            }
                            _ => {}
                        }
                        // A seat drives: the engines read, but decide nothing.
                        let driving = seat.is_none();
                        if let Some(rm) = remote.as_ref().filter(|_| driving) {
                            let view = BotView {
                                vitals: last_vitals,
                                automation: self.auto.automation(),
                                all_off: self.auto.all_off,
                                stopped: self.auto.stopped,
                                following: party
                                    .as_ref()
                                    .filter(|p| p.state() == PartyState::Following)
                                    .map(|p| p.leader().to_owned()),
                            };
                            let effects = rm.on_item(&t.item, &view);
                            if !effects.is_empty() {
                                self.decision("remote", json!({"read": t.plain, "effects": format!("{effects:?}")}));
                            }
                            for e in effects {
                                self.apply(e, &mut pacer, &mut combat, &mut party, now);
                            }
                        }
                        // An engine that is switched off still reads the room, so it is up to
                        // date when it's switched on; it just decides nothing, and it decides
                        // only on prompts.
                        let prompt = matches!(t.item, Item::Prompt(_));
                        if let Some(c) = &mut combat
                            && ((driving && self.auto.fights()) || !prompt)
                        {
                            c.set_level(self.level);
                            let commands = c.on_item(&t.item, now);
                            let target = c.target().map(str::to_owned);
                            for cmd in commands {
                                self.decision("attack", json!({"command": cmd, "target": target}));
                                pacer.push(cmd);
                            }
                        }
                        if let Some(p) = party.as_mut().filter(|_| driving) {
                            let acts = p.on_item(&t.item, now);
                            self.queue(&mut pacer, "party", acts);
                        }
                        if let Some(r) = &mut rester
                            && ((driving && self.auto.rests()) || !prompt)
                        {
                            let fighting = combat.as_ref().is_some_and(|c| c.target().is_some());
                            let acts = r.on_item(&t.item, now, fighting);
                            self.queue(&mut pacer, "rest", acts);
                        }
                        if let Some(w) = &mut walker {
                            let fighting = combat.as_ref().is_some_and(|c| c.target().is_some());
                            for line in w.on_item(&t.item, now, fighting) {
                                pacer.push(line);
                            }
                        }
                        let ended = self.watch_walk(&mut walker, &mut walk_seen);
                        // A bot that fights by level asks its level at once (#125).
                        if in_game
                            && driving
                            && self.level.is_none()
                            && matches!(t.item, Item::Prompt(_))
                            && combat.as_ref().is_some_and(Combat::wants_level)
                            && asked_level.is_none_or(|at| now.duration_since(at) >= GEAR_WAIT)
                        {
                            pacer.push("exp");
                            asked_level = Some(now);
                        }
                        if let Some(run) = running.as_mut().filter(|_| driving && !unattended) {
                            self.feed_gear(run, Some(&t.item), &mut pacer, now);
                            self.drive_routine(run, ended, &mut pacer, &mut walker, &mut walk_seen, &mut combat, &mut rester, last_vitals, room_now, now);
                        }
                        self.watch_states(&party, &rester, &combat, &mut seen);
                    }
                    if in_game
                        && logging_off.is_none()
                        && self.inject_next(&mut stream, &mut inject, &typing).await.is_err()
                    {
                        return LinkEnd::Lost { was_in_game: true };
                    }
                    if self.drain(&mut stream, &mut pacer).await.is_err() {
                        return LinkEnd::Lost { was_in_game: in_game };
                    }
                }
                input = from_seat(seat) => match input.flatten() {
                    Some(SeatInput::Data(bytes)) => {
                        if self.typed(&mut typing, &bytes) {
                            inject.awaiting_prompt = true;
                        }
                        let mut out = Vec::with_capacity(bytes.len());
                        mp_telnet::escape(&bytes, &mut out);
                        if stream.write_all(&out).await.is_err() {
                            return LinkEnd::Lost { was_in_game: in_game };
                        }
                    }
                    Some(SeatInput::Window { cols, rows }) => {
                        let mut reply = Vec::new();
                        telnet.set_window_size(cols, rows, &mut reply);
                        if stream.write_all(&reply).await.is_err() {
                            return LinkEnd::Lost { was_in_game: in_game };
                        }
                    }
                    None if !in_game => {
                        self.decision("seat_left", json!({}));
                        return LinkEnd::Done(Offline::SeatLeft);
                    }
                    None => {
                        // MudPlay dropped mid-game: the character stays, idle, under its
                        // engine, and the session finds out where it is.
                        *seat = None;
                        unattended = true;
                        running = None;
                        inject.queue.clear();
                        self.decision("unattended", json!({"hang_up_below": format!("{:?}", self.spec.hang_up_below)}));
                        self.event("unattended", json!({}));
                        self.report(Report::Unattended);
                        self.report(Report::Queued(Vec::new()));
                        if let Some(p) = &mut party {
                            let acts = p.start(Instant::now());
                            self.queue(&mut pacer, "party", acts);
                        }
                        pacer.push("/room");
                        if self.drain(&mut stream, &mut pacer).await.is_err() {
                            return LinkEnd::Lost { was_in_game: true };
                        }
                    }
                },
                cmd = cmds.recv() => match cmd {
                    Some(Command::Kill { target }) => {
                        self.decision("kill_order", json!({"target": target}));
                        if let Some(c) = &mut combat {
                            c.order_kill(&target);
                            if in_game && logging_off.is_none() && self.auto.fights() {
                                for cmd in c.act(Instant::now()) {
                                    self.decision("attack", json!({"command": cmd, "target": target}));
                                    pacer.push(cmd);
                                }
                                if self.drain(&mut stream, &mut pacer).await.is_err() {
                                    return LinkEnd::Lost { was_in_game: true };
                                }
                            }
                        }
                    }
                    Some(Command::Do(e)) if in_game && logging_off.is_none() => {
                        self.decision("order", json!({"effect": format!("{e:?}")}));
                        let now = Instant::now();
                        self.apply(e, &mut pacer, &mut combat, &mut party, now);
                        self.watch_states(&party, &rester, &combat, &mut seen);
                        if self.drain(&mut stream, &mut pacer).await.is_err() {
                            return LinkEnd::Lost { was_in_game: true };
                        }
                    }
                    Some(Command::Do(e)) => {
                        self.decision("order_dropped", json!({"effect": format!("{e:?}"), "why": "not in the game"}));
                    }
                    Some(Command::Inject(line)) if seat.is_some() => {
                        self.decision("inject_queued", json!({"line": line}));
                        inject.queue.push_back(line);
                        self.report(Report::Queued(inject.queue.iter().cloned().collect()));
                        if in_game
                            && logging_off.is_none()
                            && self.inject_next(&mut stream, &mut inject, &typing).await.is_err()
                        {
                            return LinkEnd::Lost { was_in_game: true };
                        }
                    }
                    Some(Command::Inject(line)) => {
                        self.decision("inject_dropped", json!({"line": line, "why": "no seat"}));
                    }
                    Some(Command::Goto { to, .. }) if running.is_some() && !self.auto.stopped && !self.auto.all_off => {
                        self.decision("goto_dropped", json!({"to": format!("{}/{}", to.0, to.1), "why": "the routine is running"}));
                        self.report(Report::Walk(WalkState::Failed("the routine is walking the bot: @stop it first".into())));
                    }
                    Some(Command::Goto { to, rules }) if in_game && logging_off.is_none() && seat.is_none() => {
                        self.decision("goto", json!({"to": format!("{}/{}", to.0, to.1), "rules": format!("{rules:?}")}));
                        match self.spec.rooms.clone() {
                            Some(book) => {
                                let mut w = Walker::new(to, book, WALK_STEP_WAIT).with_rules(rules);
                                for line in w.start(Instant::now()) {
                                    pacer.push(line);
                                }
                                walker = Some(w);
                                walk_seen = None;
                                if let Some(run) = &mut running {
                                    // A walk by hand: not the routine's to follow.
                                    run.walking = false;
                                }
                                self.watch_walk(&mut walker, &mut walk_seen);
                            }
                            None => self.report(Report::Walk(WalkState::Failed("no game data to plan a walk on".into()))),
                        }
                        if self.drain(&mut stream, &mut pacer).await.is_err() {
                            return LinkEnd::Lost { was_in_game: true };
                        }
                    }
                    Some(Command::Goto { to, .. }) => {
                        self.decision("goto_dropped", json!({"to": format!("{}/{}", to.0, to.1), "why": "not in the game, or played in MudPlay"}));
                        self.report(Report::Walk(WalkState::Failed("not able to walk now".into())));
                    }
                    Some(Command::RefreshSheet) => sheet_due = true,
                    Some(Command::Capture { id, line }) if in_game && logging_off.is_none() => {
                        self.decision("capture", json!({"id": id, "line": line}));
                        capturer.ask(&id, &line, Instant::now());
                        if seat.is_some() {
                            inject.queue.push_back(line);
                            self.report(Report::Queued(inject.queue.iter().cloned().collect()));
                            if self.inject_next(&mut stream, &mut inject, &typing).await.is_err() {
                                return LinkEnd::Lost { was_in_game: true };
                            }
                        } else {
                            pacer.push(line);
                        }
                    }
                    Some(Command::Capture { id, line }) => {
                        self.report(Report::Captured(Captured { id, line, text: String::new(), answered: false }));
                    }
                    None => return LinkEnd::Done(Offline::Dropped),
                    Some(Command::Logoff) if !in_game => return LinkEnd::Done(Offline::LoggedOff),
                    Some(Command::Logoff) => {
                        if logging_off.is_none() {
                            // Nothing queued goes after the exit.
                            pacer = Pacer::new(self.spec.pacing.clone());
                            logging_off = Some((Instant::now() + self.spec.logoff_timeout, 1));
                            self.state(SessionState::LoggingOff);
                            self.decision("logoff", json!({}));
                            // The character's own statline goes back before the exit.
                            if let Some(line) = self.restore_statline(users_statline)
                                && self.send(&mut stream, &line, false).await.is_err()
                            {
                                return LinkEnd::Done(Offline::LoggedOff);
                            }
                            if self.send(&mut stream, "quit", false).await.is_err() {
                                return LinkEnd::Done(Offline::LoggedOff);
                            }
                        }
                    }
                    Some(Command::Config { statline, rev }) => {
                        self.spec.statline = statline;
                        self.spec.config_rev = rev.clone();
                        // A seated session runs MudPlay's statline, not the file's.
                        if in_game
                            && !users_statline
                            && self.set_statline(&mut stream, &mut reader).await.is_err()
                        {
                            return LinkEnd::Lost { was_in_game: true };
                        }
                        self.decision("config_applied", json!({"rev": rev}));
                        self.report(Report::ConfigApplied { rev });
                    }
                },
                _ = tick.tick() => {
                    let now = Instant::now();
                    // PROFILE never answered: set the bot's statline, saving nothing.
                    if probe.is_some_and(|at| now >= at) {
                        probe = None;
                        if self.settle_statline(&mut stream, &mut reader, None).await.is_err() {
                            return LinkEnd::Lost { was_in_game: true };
                        }
                    }
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
                    if in_game && logging_off.is_none() {
                        // A query the game didn't answer in time gets what is known.
                        if self.asks.iter().any(|a| now >= a.until) {
                            let doing = Doing::of(&combat, &rester, now);
                            let late: Vec<Ask> = self.asks.iter().filter(|a| now >= a.until).cloned().collect();
                            self.asks.retain(|a| now < a.until);
                            for a in late {
                                self.decision("ask_timed_out", json!({"query": format!("{:?}", a.query)}));
                                self.reply_to(&mut pacer, a, None, &doing);
                            }
                        }
                        if let Some(p) = &mut party {
                            let acts = p.on_tick(now);
                            self.queue(&mut pacer, "party", acts);
                        }
                        for c in capturer.on_tick(now) {
                            self.report(Report::Captured(c));
                        }
                        // The sheets, when they're due: never in a fight, and never while
                        // resting, since `i` would stand the bot up. A seated character's
                        // fill from what its player types.
                        let fighting = combat.as_ref().is_some_and(|c| c.target().is_some());
                        let standing = last_vitals.is_none_or(|v| v.rest == mp_parse::Rest::Standing)
                            && !rester.as_ref().is_some_and(Rester::resting);
                        if sheet_due && probe.is_none() && seat.is_none() && !unattended && !fighting && standing {
                            sheet_due = false;
                            pacer.push("st");
                            pacer.push("i");
                            pacer.push("exp");
                        }
                        if let Some(w) = &mut walker {
                            let fighting = combat.as_ref().is_some_and(|c| c.target().is_some());
                            for line in w.on_tick(now, fighting) {
                                pacer.push(line);
                            }
                        }
                        let ended = self.watch_walk(&mut walker, &mut walk_seen);
                        if let Some(run) = running.as_mut().filter(|_| seat.is_none() && !unattended) {
                            self.feed_gear(run, None, &mut pacer, now);
                            self.drive_routine(run, ended, &mut pacer, &mut walker, &mut walk_seen, &mut combat, &mut rester, last_vitals, room_now, now);
                        }
                        self.watch_states(&party, &rester, &combat, &mut seen);
                        if self.drain(&mut stream, &mut pacer).await.is_err() {
                            return LinkEnd::Lost { was_in_game: true };
                        }
                    }
                }
                _ = pace(pacer.next_at()) => {
                    if self.drain(&mut stream, &mut pacer).await.is_err() {
                        return LinkEnd::Lost { was_in_game: in_game };
                    }
                }
            }
        }
    }
}

impl Doing {
    fn of(combat: &Option<Combat>, rester: &Option<Rester>, now: Instant) -> Self {
        Self {
            fighting: combat.as_ref().is_some_and(|c| c.target().is_some()),
            poisoned: rester.as_ref().is_some_and(|r| r.poisoned(now)),
        }
    }
}

/// Waits until the pacer's next line may go; forever when none is waiting.
async fn pace(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at.into()).await,
        None => std::future::pending().await,
    }
}

async fn run_from(
    spec: SessionSpec,
    mut cmds: mpsc::UnboundedReceiver<Command>,
    reports: mpsc::UnboundedSender<Report>,
    mut upstream: Option<Upstream>,
    mut seat: Option<Seat>,
) -> Offline {
    let logs = spec.logs.as_ref().and_then(|root| {
        let header = Header {
            character: spec.name.clone(),
            session: 1,
            config_rev: spec.config_rev.clone(),
            gamedata_rev: String::new(),
            realm: "silvermere".into(),
        };
        CharacterLogs::open(root, header, spec.log_rolls, Box::new(SystemClock)).ok()
    });
    let mut r = Runner {
        auto: AutoState::new(spec.automation),
        asks: Vec::new(),
        exp_made: 0,
        exp_since: None,
        exp_known: None,
        level: None,
        last_vitals: None,
        history: HistoryFilter::default(),
        spec,
        reports,
        logs,
    };
    let mut attempt = 0usize;
    // A session MudPlay started is MudPlay's to reconnect, attended or not.
    let seated = seat.is_some();
    loop {
        r.state(SessionState::Connecting);
        let link = r.link(&mut cmds, upstream.take(), &mut seat).await;
        // A seated session's link is MudPlay's: when the game drops it, MudPlay reconnects
        // and logs in by itself.
        let link = match link {
            LinkEnd::Lost { .. } if seated => LinkEnd::Done(Offline::SeatLinkLost),
            other => other,
        };
        match link {
            LinkEnd::Done(off) => {
                let kind = match &off {
                    Offline::LoggedOff => "logged_off",
                    Offline::LoginFailed(_) => "stopped_login_failed",
                    Offline::TakenOver => "taken_over",
                    Offline::LogoffTimedOut => "logoff_timed_out",
                    Offline::Dropped => "dropped",
                    Offline::SeatLeft => "seat_left",
                    Offline::HungUp { .. } => "hung_up",
                    Offline::SeatLinkLost => "seat_link_lost",
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
                        // No link, so no room to fight in: an order from now is dropped.
                        Some(Command::Kill { target }) => {
                            r.decision("kill_order_dropped", json!({"target": target, "why": "reconnecting"}));
                        }
                        Some(Command::Do(e)) => {
                            r.decision("order_dropped", json!({"effect": format!("{e:?}"), "why": "reconnecting"}));
                        }
                        Some(Command::Inject(line)) => {
                            r.decision("inject_dropped", json!({"line": line, "why": "reconnecting"}));
                        }
                        Some(Command::Goto { to, .. }) => {
                            r.decision("goto_dropped", json!({"to": format!("{}/{}", to.0, to.1), "why": "reconnecting"}));
                            r.report(Report::Walk(WalkState::Failed("reconnecting".into())));
                        }
                        // The sheets are read again after the login anyway.
                        Some(Command::RefreshSheet) => {}
                        Some(Command::Capture { id, line }) => {
                            r.report(Report::Captured(Captured { id, line, text: String::new(), answered: false }));
                        }
                    }
                }
            }
        }
    }
}
