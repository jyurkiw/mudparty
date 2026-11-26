//! The mudparty server (design §3.1): it owns the config, hosts the headless bots, and logs.
//!
//! It starts and stops bots by name (one session per character), pushes an edited
//! character file to its running bot, keeps the party view the web page shows (*§6.8*,
//! [`web`]), carries out the page's orders, and on an orderly stop logs every bot off.
//! The hub and the proxy come in later steps.

pub mod history;
pub mod proxy;
pub mod view;
pub mod web;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use anyhow::Context;
use history::{History, Position};
use mp_config::Loaded;
use mp_config::{
    CharacterConfig, Logging, PartyRole, Secrets, ServerConfig, load_character, load_server,
};
use mp_data::{GameData, ShowsAs};
use mp_engine::{
    AutoSwitch, CombatSettings, Command, Effect, HuntSettings, Inviter, LoginSettings, Offline,
    PartySettings, RemoteSettings, Report, RestSettings, RoomBook, RoomInfo, RoutineSettings, Seat,
    SessionHandle, SessionSpec, SessionState, Upstream, spawn, spawn_seated,
};
use mp_log::{RollConfig, ServerLog, StreamRolls, SystemClock};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::json;
use tokio::sync::{broadcast, mpsc, watch};
use view::Card;

/// Waits before each reconnect of a lost link; the last one repeats (design §6.2).
const RECONNECT: [u64; 4] = [5, 10, 30, 60];
/// A combat round on a stock game (design §6.4).
const ROUND: Duration = Duration::from_secs(5);
/// How long a login step waits for the game.
const STEP_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a bot waits on an unanswered `@invite`, or away from its leader, before it
/// checks `par` again (design §6.5).
const PARTY_RETRY: Duration = Duration::from_secs(30);
/// How long after the last sign of poison a bot waits before it tries to rest (design §6.6).
const SICK_WAIT: Duration = Duration::from_secs(30);
/// How long an edited character file must be quiet before it is read.
const SETTLE: Duration = Duration::from_millis(300);
/// How long after a failed walk a routine tries again (#53).
const ROUTINE_RETRY: Duration = Duration::from_secs(30);
/// How long a hunted monster may stand there unfought before the hunt gives up on it.
const KILL_WAIT: Duration = Duration::from_secs(30);

/// Something a bot did, for anyone watching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BotEvent {
    Session {
        name: String,
        report: Report,
    },
    /// The character's file was edited and doesn't read; the bot keeps its old config.
    ConfigError {
        name: String,
        message: String,
    },
}

/// Why a bot didn't start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartError {
    /// The character already has a session.
    Running(String),
    /// Its file is missing or doesn't read.
    Config(String),
    /// Its file has no `login`: the main character, who logs in through MudPlay.
    NoLogin(String),
    /// `secrets.yaml` has no password for this account.
    NoPassword(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Running(n) => write!(f, "{n} already has a session"),
            Self::Config(e) => write!(f, "{e}"),
            Self::NoLogin(n) => write!(f, "{n}'s file has no login, so it can't run as a bot"),
            Self::NoPassword(a) => write!(f, "secrets.yaml has no password for account {a}"),
        }
    }
}

impl std::error::Error for StartError {}

/// An order from the web page (design §4.2) for one character.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(tag = "order", rename_all = "snake_case")]
pub enum Order {
    /// Start it as a headless bot.
    Start,
    /// Log it off.
    Stop,
    /// `combat`, `nuke` or `rest` on or off, or `all` (every engine) on or off.
    Toggle { switch: String, on: bool },
    /// A line for the game.
    Command { text: String },
    /// Attack this monster first, until it dies.
    Kill { target: String },
    /// Hold it, as a `@wait` from the page.
    Wait,
    /// Free it, as an `@ok` from the page.
    Ok,
    /// Join this leader's party.
    Join { leader: String },
    /// Walk to a room, written `map/room` (`1/2141`).
    Goto { room: String },
    /// Pause movement and automation, the routine with them, as a `@stop`.
    Pause,
    /// Go on after a pause, as a `@rego`.
    Resume,
}

/// Who a `wait` or `ok` from the web page is from, in `held_by`.
pub const WEB: &str = "web";

struct Bot {
    handle: SessionHandle,
    rev: String,
    /// Tells this run of the bot from a later one under the same name.
    run: u64,
    /// Driven by a MudPlay seat rather than its engine.
    seated: bool,
}

/// A running server.
pub struct Server {
    root: PathBuf,
    config: ServerConfig,
    secrets: Secrets,
    bots: Mutex<BTreeMap<String, Bot>>,
    events: broadcast::Sender<BotEvent>,
    log: Mutex<ServerLog>,
    watcher: Mutex<Option<RecommendedWatcher>>,
    /// `gamedata/gamedata.sqlite`, when there is one.
    gamedata: Arc<Mutex<Option<GameData>>>,
    runs: AtomicU64,
    /// The party as the web page shows it, by name.
    cards: Mutex<BTreeMap<String, Card>>,
    /// Counts changes to `cards`, for the page's feed.
    changed: watch::Sender<u64>,
    /// `mudparty.sqlite`, when it opens.
    history: Option<Arc<History>>,
    /// The server itself, for what a session asks of it (an invitation by injection).
    me: Mutex<Weak<Server>>,
}

impl Server {
    /// Starts a server on the data root `root`: reads `mudparty.toml` and `secrets.yaml`,
    /// opens `logs/server.jsonl`, watches `characters/`, and starts the bots whose files
    /// say `autostart: true`.
    pub async fn start(root: &Path) -> anyhow::Result<Arc<Self>> {
        let config = load_server(&root.join("mudparty.toml"))?.value;
        let secrets_path = root.join("secrets.yaml");
        let secrets = if secrets_path.exists() {
            Secrets::load(&secrets_path)?
        } else {
            Secrets::default()
        };
        let roll = config.logging.roll_for("server");
        let log = ServerLog::open(
            &root.join("logs"),
            RollConfig {
                file_size: roll.file_size,
                plain_files: roll.plain_files,
                zip_archives: roll.zip_archives,
            },
            Box::new(SystemClock),
        )
        .context("opening logs/server.jsonl")?;
        let characters = root.join("characters");
        std::fs::create_dir_all(&characters)?;
        let (history, history_error) = match History::open(&root.join("mudparty.sqlite")) {
            Ok(h) => (Some(Arc::new(h)), None),
            Err(e) => (None, Some(e.to_string())),
        };
        let (events, _) = broadcast::channel(1024);
        let server = Arc::new(Self {
            root: root.to_owned(),
            config,
            secrets,
            bots: Mutex::new(BTreeMap::new()),
            events,
            log: Mutex::new(log),
            watcher: Mutex::new(None),
            gamedata: Arc::new(Mutex::new(None)),
            runs: AtomicU64::new(0),
            cards: Mutex::new(BTreeMap::new()),
            changed: watch::Sender::new(0),
            history,
            me: Mutex::new(Weak::new()),
        });
        *server.me.lock().expect("me lock") = Arc::downgrade(&server);
        if let Some(e) = history_error {
            server.record("history_open_failed", json!({"error": e}));
        }
        server.record("start", json!({"root": root.display().to_string()}));
        server.open_gamedata();
        server.watch(&characters)?;
        let mut names: Vec<String> = std::fs::read_dir(&characters)?
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let p = e.path();
                (p.extension()? == "yaml").then(|| p.file_stem()?.to_str().map(str::to_owned))?
            })
            .collect();
        names.sort();
        for name in names {
            let Ok(c) = load_character(&characters.join(format!("{name}.yaml"))) else {
                continue;
            };
            server.card_for(&c.value);
            if c.value.autostart
                && let Err(e) = server.start_bot(&name)
            {
                server.record(
                    "autostart_failed",
                    json!({"character": name, "why": e.to_string()}),
                );
            }
        }
        Ok(server)
    }

    /// Opens `gamedata/gamedata.sqlite`, importing it first from `silvermere.db` when it is
    /// missing (design §7.5). Without either, bots still log in, but don't fight: the engine
    /// needs the game's monster names to tell monsters from players.
    fn open_gamedata(&self) {
        let path = self.root.join("gamedata").join("gamedata.sqlite");
        if !path.exists() {
            let source = self.root.join(&self.config.gamedata.silvermere_db);
            if !source.exists() {
                self.record(
                    "gamedata_missing",
                    json!({"silvermere_db": source.display().to_string()}),
                );
                return;
            }
            let imported = std::fs::create_dir_all(path.parent().expect("a folder"))
                .map_err(|e| e.to_string())
                .and_then(|()| mp_data::import(&source, &path).map_err(|e| e.to_string()));
            match imported {
                Ok(r) => self.record(
                    "gamedata_imported",
                    json!({"rev": r.rev, "rooms": r.rooms, "spells": r.spells, "monsters": r.monsters}),
                ),
                Err(e) => {
                    self.record("gamedata_import_failed", json!({"error": e}));
                    return;
                }
            }
        }
        match GameData::open(&path) {
            Ok(gd) => *self.gamedata.lock().expect("gamedata lock") = Some(gd),
            Err(e) => self.record("gamedata_open_failed", json!({"error": e.to_string()})),
        }
    }

    /// How a bot fights, from its file and the game data; `None` when its file turns combat
    /// off or there is no game data.
    /// How the bot fights. Built whether or not `automation.combat` starts it on, so
    /// `@auto-combat on` has something to switch on.
    fn combat_for(&self, c: &CharacterConfig) -> Option<CombatSettings> {
        let gd = self.gamedata.lock().expect("gamedata lock");
        let Some(gd) = gd.as_ref() else {
            self.record(
                "combat_off",
                json!({"character": c.name, "why": "no game data"}),
            );
            return None;
        };
        let monsters = gd.monster_names().unwrap_or_default();
        let spells = c
            .combat
            .tiers
            .iter()
            .map(|t| t.spell.as_str())
            .chain(c.combat.room_tiers.iter().map(|t| t.spell.as_str()));
        let spell_costs = spells
            .filter_map(|short| {
                let s = gd.spell_by_short(short).ok()??;
                Some((short.to_owned(), s.mana as i32))
            })
            .collect();
        Some(CombatSettings {
            name: c.name.clone(),
            leader: c.party.leader.clone(),
            targeting: c.combat.targeting.clone(),
            tiers: c.combat.tiers.clone(),
            room_tiers: c.combat.room_tiers.clone(),
            melee: c.combat.melee.clone(),
            spell_costs,
            monsters,
            leader_target_stale: self.config.stale.target,
            round: ROUND,
        })
    }

    /// Where the web page is served (`listen.client_hub`).
    pub fn listen(&self) -> SocketAddr {
        self.config.listen.client_hub
    }

    /// Where MudPlay connects (`listen.proxy`).
    pub fn proxy_listen(&self) -> SocketAddr {
        self.config.listen.proxy
    }

    /// Where `name` was last known to be, from `mudparty.sqlite`.
    pub fn last_position(&self, name: &str) -> Option<Position> {
        self.history.as_ref()?.last(name)
    }

    /// Records where `name` is now, from its card, and why.
    fn note_position(&self, name: &str, why: String) {
        let card = self.cards.lock().expect("cards lock").get(name).cloned();
        let Some(card) = card else { return };
        let Some(room) = card.room else {
            self.record("position_unknown", json!({"character": name, "why": why}));
            return;
        };
        let p = Position {
            map: room.map,
            room: room.room,
            room_name: room.name,
            hp: card.vitals.map(|v| v.hp),
            max_hp: card.vitals.and_then(|v| v.max_hp),
            why,
            at: jiff::Timestamp::now().to_string(),
        };
        self.record("position", json!({"character": name, "position": p}));
        if let Some(h) = &self.history
            && let Err(e) = h.record(name, &p)
        {
            self.record("history_write_failed", json!({"error": e.to_string()}));
        }
        self.card(name, |c| c.last_position = Some(p));
    }

    /// Every character's card, the main character first.
    pub fn party(&self) -> Vec<Card> {
        view::ordered(&self.cards.lock().expect("cards lock"))
    }

    /// Changes whenever a card does.
    pub fn watch_party(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    /// Changes `name`'s card, if it has one.
    fn card(&self, name: &str, f: impl FnOnce(&mut Card)) {
        let mut cards = self.cards.lock().expect("cards lock");
        if let Some(c) = cards.get_mut(name) {
            f(c);
            drop(cards);
            self.changed.send_modify(|n| *n += 1);
        }
    }

    /// Makes or refreshes the card for a character file.
    fn card_for(&self, c: &CharacterConfig) {
        let main = self.config.main_character.as_deref() == Some(c.name.as_str());
        let last = self.last_position(&c.name);
        self.cards
            .lock()
            .expect("cards lock")
            .entry(c.name.clone())
            .and_modify(|card| card.configure(c))
            .or_insert_with(|| {
                let mut card = Card::new(c, main);
                card.last_position = last;
                card
            });
        self.changed.send_modify(|n| *n += 1);
    }

    /// Carries out an order from the web page. `Ok` says what was done; `Err` why not.
    pub fn order(self: &Arc<Self>, name: &str, order: Order) -> Result<String, String> {
        if !self.cards.lock().expect("cards lock").contains_key(name) {
            return Err(format!("no character {name}"));
        }
        self.record(
            "order",
            json!({"character": name, "order": format!("{order:?}")}),
        );
        let cmd = match order {
            Order::Start => {
                return self
                    .start_bot(name)
                    .map(|()| format!("{name} starting"))
                    .map_err(|e| e.to_string());
            }
            Order::Stop => {
                if !self.running().iter().any(|n| n == name) {
                    return Err(format!("{name} isn't running"));
                }
                if self.seated(name) {
                    return Err(format!("{name} is played in MudPlay"));
                }
                let me = Arc::clone(self);
                let who = name.to_owned();
                tokio::spawn(async move { me.stop_bot(&who).await });
                return Ok(format!("{name} logging off"));
            }
            Order::Toggle { switch, on } => Command::Do(match switch.as_str() {
                "combat" => Effect::SetAuto(AutoSwitch::Combat, on),
                "nuke" => Effect::SetAuto(AutoSwitch::Nuke, on),
                "rest" => Effect::SetAuto(AutoSwitch::Rest, on),
                "all" => Effect::AllOff(!on),
                other => return Err(format!("no switch {other:?}")),
            }),
            Order::Command { text } if text.trim().is_empty() => {
                return Err("an empty command".into());
            }
            Order::Command { text } if self.seated(name) => return self.inject(name, &text),
            Order::Command { text } => Command::Do(Effect::Send(text)),
            Order::Kill { target } => Command::Kill { target },
            Order::Wait => Command::Do(Effect::Wait(WEB.into())),
            Order::Ok => Command::Do(Effect::Ok(WEB.into())),
            Order::Join { leader } => Command::Do(Effect::Join(leader)),
            Order::Pause => Command::Do(Effect::Stop(true)),
            Order::Resume => Command::Do(Effect::Stop(false)),
            Order::Goto { room } => Command::Goto {
                to: room_key(&room).ok_or_else(|| format!("{room:?} isn't a room like 1/2141"))?,
            },
        };
        let bots = self.bots.lock().expect("bots lock");
        let Some(bot) = bots.get(name) else {
            return Err(format!("{name} isn't running"));
        };
        if bot.seated {
            return Err(format!("{name} is played in MudPlay"));
        }
        bot.handle.send(cmd);
        Ok("sent".into())
    }

    /// Everything bots do, from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<BotEvent> {
        self.events.subscribe()
    }

    /// The characters with a session, by name.
    pub fn running(&self) -> Vec<String> {
        self.bots
            .lock()
            .expect("bots lock")
            .keys()
            .cloned()
            .collect()
    }

    // The server log is never allowed to stop the server, so a failed write is dropped.
    fn record(&self, kind: &str, fields: serde_json::Value) {
        let _ = self.log.lock().expect("log lock").record(kind, fields);
    }

    fn file(&self, name: &str) -> PathBuf {
        self.root.join("characters").join(format!("{name}.yaml"))
    }

    /// Starts `name` as a headless bot.
    pub fn start_bot(self: &Arc<Self>, name: &str) -> Result<(), StartError> {
        let mut bots = self.bots.lock().expect("bots lock");
        if bots.contains_key(name) {
            return Err(StartError::Running(name.to_owned()));
        }
        let loaded =
            load_character(&self.file(name)).map_err(|e| StartError::Config(e.to_string()))?;
        let c = loaded.value;
        let login = c
            .login
            .clone()
            .ok_or_else(|| StartError::NoLogin(name.to_owned()))?;
        let password = self
            .secrets
            .password(&login.account)
            .ok_or_else(|| StartError::NoPassword(login.account.clone()))?;
        let login = LoginSettings {
            account: login.account.clone(),
            password: password.to_owned(),
            takeover: login.takeover,
            step_timeout: STEP_TIMEOUT,
        };
        let spec = self.spec_for(&c, &loaded.rev, login);
        let (tx, rx) = mpsc::unbounded_channel();
        let handle = spawn(spec, tx);
        self.attach(&mut bots, &c, loaded.rev, handle, rx, false);
        drop(bots);
        self.record("bot_start", json!({"character": name}));
        Ok(())
    }

    /// Sends `text` as a seated character (design §2): it waits until the user has no
    /// half-typed line in MudPlay and the last line's prompt is back.
    pub fn inject(&self, name: &str, text: &str) -> Result<String, String> {
        if text.trim().is_empty() {
            return Err("an empty command".into());
        }
        let bots = self.bots.lock().expect("bots lock");
        match bots.get(name) {
            Some(b) if b.seated => {
                b.handle.send(Command::Inject(text.to_owned()));
                drop(bots);
                self.record("inject", json!({"character": name, "line": text}));
                Ok(format!("queued for {name}"))
            }
            _ => Err(format!("{name} isn't being played in MudPlay")),
        }
    }

    /// Whether `name`'s session is driven by a MudPlay seat and in the game.
    pub(crate) fn seat_in_game(&self, name: &str) -> bool {
        self.seated(name)
            && self
                .cards
                .lock()
                .expect("cards lock")
                .get(name)
                .is_some_and(|c| matches!(c.mode, "idle" | "fighting" | "resting" | "waiting"))
    }

    /// `#who`'s answer: each character mudparty holds, and who drives it.
    pub(crate) fn who(&self) -> String {
        let bots = self.bots.lock().expect("bots lock");
        let list: Vec<String> = bots
            .iter()
            .map(|(n, b)| format!("{n} ({})", if b.seated { "MudPlay" } else { "bot" }))
            .collect();
        if list.is_empty() {
            "no characters".into()
        } else {
            list.join(", ")
        }
    }

    /// Whether `name`'s session is driven by a MudPlay seat.
    fn seated(&self, name: &str) -> bool {
        self.bots
            .lock()
            .expect("bots lock")
            .get(name)
            .is_some_and(|b| b.seated)
    }

    /// The game's `host:port`.
    pub fn game(&self) -> String {
        format!("{}:{}", self.config.game.host, self.config.game.port)
    }

    /// Whose session a MudPlay seat is, from the account it typed (design §2.1): the
    /// character whose `login.account` it is, else `main_character`, else a character named
    /// as typed with no file.
    fn seat_character(&self, account: &str) -> (String, Option<Loaded<CharacterConfig>>) {
        let dir = self.root.join("characters");
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).collect())
            .unwrap_or_default();
        files.sort();
        for p in files
            .iter()
            .filter(|p| p.extension().is_some_and(|e| e == "yaml"))
        {
            if let Ok(l) = load_character(p)
                && l.value
                    .login
                    .as_ref()
                    .is_some_and(|g| g.account.eq_ignore_ascii_case(account))
            {
                return (l.value.name.clone(), Some(l));
            }
        }
        match &self.config.main_character {
            Some(main) => (main.clone(), load_character(&self.file(main)).ok()),
            None => (account.to_owned(), None),
        }
    }

    /// Attaches a MudPlay seat to a new session on `upstream`, named from the account
    /// MudPlay typed (design §2.1). A bot already playing that character is left to the
    /// game's takeover question, which MudPlay answers. Returns the session's name.
    pub fn seat_session(self: &Arc<Self>, account: &str, upstream: Upstream, seat: Seat) -> String {
        let (name, loaded) = self.seat_character(account);
        let c = match &loaded {
            Some(l) => l.value.clone(),
            None => match CharacterConfig::from_yaml(&format!("schema: 1\nname: {name:?}\n")) {
                Ok(c) => c,
                Err(_) => CharacterConfig::from_yaml("schema: 1\nname: Unknown\n")
                    .expect("a literal file"),
            },
        };
        let rev = loaded.map(|l| l.rev).unwrap_or_default();
        let login = LoginSettings {
            account: account.to_owned(),
            password: self
                .secrets
                .password(account)
                .unwrap_or_default()
                .to_owned(),
            takeover: true,
            step_timeout: STEP_TIMEOUT,
        };
        let spec = self.spec_for(&c, &rev, login);
        let (tx, rx) = mpsc::unbounded_channel();
        let handle = spawn_seated(spec, upstream, seat, tx);
        let mut bots = self.bots.lock().expect("bots lock");
        if let Some(old) = bots.remove(&name) {
            // The game takes the character over from the old session; let it finish.
            self.record("seat_takes_over", json!({"character": name}));
            tokio::spawn(old.handle.finished());
        }
        self.attach(&mut bots, &c, rev, handle, rx, true);
        drop(bots);
        self.record(
            "seat_attached",
            json!({"character": name, "account": account}),
        );
        name
    }

    /// What a session for `c` runs on.
    fn spec_for(&self, c: &CharacterConfig, rev: &str, login: LoginSettings) -> SessionSpec {
        SessionSpec {
            name: c.name.clone(),
            game: self.game(),
            login,
            statline: c.statline.clone(),
            config_rev: rev.to_owned(),
            logoff_timeout: self.config.logoff_timeout,
            reconnect_delays: RECONNECT.iter().map(|s| Duration::from_secs(*s)).collect(),
            logs: Some(self.root.join("logs")),
            log_rolls: log_rolls(&self.config.logging),
            combat: self.combat_for(c),
            pacing: Default::default(),
            party: party_for(c),
            rest: rest_for(c),
            remote: Some(RemoteSettings {
                name: c.name.clone(),
                allow: c.remote.allow.clone(),
                channels: c.remote.channels.clone(),
                warn_denied: c.remote.warn_denied,
                version: env!("CARGO_PKG_VERSION").to_owned(),
            }),
            automation: c.automation,
            rooms: Some(Arc::new(GameRooms(Arc::clone(&self.gamedata)))),
            hang_up_below: c.unattended.hang_up_below,
            statline_store: self
                .history
                .clone()
                .map(|h| h as Arc<dyn mp_engine::StatlineStore>),
            statline_restore: c.statline_restore,
            routine: routine_for(c),
            inviter: c.party.invite_by_injection.then(|| {
                Arc::new(SeatInviter(self.me.lock().expect("me lock").clone())) as Arc<dyn Inviter>
            }),
        }
    }

    /// Puts a new session in the running list and its card, and follows its reports.
    fn attach(
        self: &Arc<Self>,
        bots: &mut BTreeMap<String, Bot>,
        c: &CharacterConfig,
        rev: String,
        handle: SessionHandle,
        mut rx: mpsc::UnboundedReceiver<Report>,
        seated: bool,
    ) {
        let name = c.name.clone();
        let automation = c.automation;
        self.card_for(c);
        self.card(&name, |card| card.started(automation, seated));
        let run = self.runs.fetch_add(1, Ordering::Relaxed);
        bots.insert(
            name.clone(),
            Bot {
                handle,
                rev,
                run,
                seated,
            },
        );
        let me = Arc::clone(self);
        let who = name;
        tokio::spawn(async move {
            while let Some(report) = rx.recv().await {
                if let Report::State(SessionState::Offline(off)) = &report {
                    let mut bots = me.bots.lock().expect("bots lock");
                    if bots.get(&who).is_some_and(|b| b.run == run) {
                        bots.remove(&who);
                    }
                    drop(bots);
                    me.record(
                        "bot_offline",
                        json!({"character": who, "why": format!("{off:?}")}),
                    );
                }
                let was_unattended = me
                    .cards
                    .lock()
                    .expect("cards lock")
                    .get(&who)
                    .is_some_and(|c| c.driver == Some("unattended"));
                let rooms = GameRooms(Arc::clone(&me.gamedata));
                me.card(&who, |card| {
                    card.report(&report, |m, r| rooms.room(m, r).map(|i| i.name));
                });
                match &report {
                    // MudPlay dropped: a bot now, taking the page's orders.
                    Report::Unattended => {
                        if let Some(b) = me.bots.lock().expect("bots lock").get_mut(&who)
                            && b.run == run
                        {
                            b.seated = false;
                        }
                        me.record("unattended", json!({"character": who}));
                    }
                    // Where an unattended character is, for a rescue.
                    Report::Room { .. } if was_unattended => {
                        me.note_position(&who, "MudPlay left".into());
                    }
                    Report::State(SessionState::Offline(off)) if was_unattended => {
                        me.note_position(&who, view::offline_text(off));
                    }
                    _ => {}
                }
                let _ = me.events.send(BotEvent::Session {
                    name: who.clone(),
                    report,
                });
            }
        });
    }

    /// Logs `name` off and waits for it. `None` if it had no session.
    pub async fn stop_bot(&self, name: &str) -> Option<Offline> {
        let bot = self.bots.lock().expect("bots lock").remove(name)?;
        self.record("bot_stop", json!({"character": name}));
        Some(self.log_off(bot).await)
    }

    async fn log_off(&self, bot: Bot) -> Offline {
        bot.handle.send(Command::Logoff);
        let limit = self.config.logoff_timeout + Duration::from_secs(5);
        match tokio::time::timeout(limit, bot.handle.finished()).await {
            Ok(off) => off,
            Err(_) => Offline::LogoffTimedOut,
        }
    }

    /// The orderly stop (design §4.4): every bot logs off at once, and the server waits for
    /// them. Returns how each one ended, by name.
    pub async fn shutdown(&self) -> Vec<(String, Offline)> {
        self.watcher.lock().expect("watcher lock").take();
        let bots = std::mem::take(&mut *self.bots.lock().expect("bots lock"));
        for bot in bots.values() {
            bot.handle.send(Command::Logoff);
        }
        let mut ended = Vec::new();
        for (name, bot) in bots {
            ended.push((name, self.log_off(bot).await));
        }
        let summary: Vec<_> = ended
            .iter()
            .map(|(n, o)| json!({"character": n, "ended": format!("{o:?}")}))
            .collect();
        self.record("shutdown", json!({ "bots": summary }));
        ended
    }

    /// Watches `characters/` and hands each edited file to [`Server::reload`].
    fn watch(self: &Arc<Self>, dir: &Path) -> anyhow::Result<()> {
        let (tx, mut rx) = mpsc::unbounded_channel::<PathBuf>();
        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if let Ok(event) = res
                    && (event.kind.is_modify() || event.kind.is_create())
                {
                    for p in event.paths {
                        let _ = tx.send(p);
                    }
                }
            })?;
        watcher.watch(dir, RecursiveMode::NonRecursive)?;
        *self.watcher.lock().expect("watcher lock") = Some(watcher);
        let me = Arc::downgrade(self);
        tokio::spawn(async move {
            // An editor, or `fs::write`, empties a file before writing it, and Linux's
            // watcher reports that half-written moment too. So changes are gathered until a
            // file has been quiet for `SETTLE`, and each changed file is read once, whole.
            while let Some(first) = rx.recv().await {
                let mut changed = std::collections::BTreeSet::from([first]);
                loop {
                    match tokio::time::timeout(SETTLE, rx.recv()).await {
                        Ok(Some(p)) => {
                            changed.insert(p);
                        }
                        Ok(None) => return,
                        Err(_) => break,
                    }
                }
                let Some(me) = me.upgrade() else { break };
                for path in changed {
                    me.reload(&path);
                }
            }
        });
        Ok(())
    }

    /// An edited character file (design §5.1): a running bot gets the new config if it reads
    /// and changed; if it doesn't read, the bot keeps the old one and the error is reported.
    fn reload(&self, path: &Path) {
        if path.extension().is_none_or(|e| e != "yaml") {
            return;
        }
        let Some(name) = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned) else {
            return;
        };
        let loaded = load_character(path);
        match &loaded {
            Ok(l) => self.card_for(&l.value),
            Err(e) => {
                let message = e.to_string();
                self.card(&name, |c| c.config_error = Some(message));
            }
        }
        let mut bots = self.bots.lock().expect("bots lock");
        let Some(bot) = bots.get_mut(&name) else {
            return;
        };
        match loaded {
            Ok(l) if l.rev != bot.rev => {
                bot.rev = l.rev.clone();
                bot.handle.send(Command::Config {
                    statline: l.value.statline,
                    rev: l.rev.clone(),
                });
                drop(bots);
                self.record("config_changed", json!({"character": name, "rev": l.rev}));
            }
            Ok(_) => {}
            Err(e) => {
                drop(bots);
                let message = e.to_string();
                self.record("config_error", json!({"character": name, "error": message}));
                let _ = self.events.send(BotEvent::ConfigError { name, message });
            }
        }
    }
}

/// How each of a bot's log streams rolls: its own `[logging.<stream>]` table over the
/// shared `[logging]` settings (design §9.3).
pub fn log_rolls(logging: &Logging) -> StreamRolls {
    let roll = |stream: &str| {
        let r = logging.roll_for(stream);
        RollConfig {
            file_size: r.file_size,
            plain_files: r.plain_files,
            zip_archives: r.zip_archives,
        }
    };
    StreamRolls {
        wire: roll("wire"),
        transcript: roll("transcript"),
        decisions: roll("decisions"),
        events: roll("events"),
    }
}

/// `1/2141` as `(1, 2141)`.
fn room_key(text: &str) -> Option<(u32, u32)> {
    let (map, room) = text.trim().split_once('/')?;
    Some((map.trim().parse().ok()?, room.trim().parse().ok()?))
}

/// Whose party the bot joins: its `party.leader`, unless it leads.
fn party_for(c: &CharacterConfig) -> Option<PartySettings> {
    if c.party.role == PartyRole::Leader {
        return None;
    }
    Some(PartySettings {
        name: c.name.clone(),
        leader: c.party.leader.clone()?,
        retry: PARTY_RETRY,
    })
}

/// When the bot rests. Built whether or not `automation.rest` starts it on, so
/// `@auto-rest on` has something to switch on.
fn rest_for(c: &CharacterConfig) -> Option<RestSettings> {
    let tell = c.rest.tell_leader.then(|| c.party.leader.clone()).flatten();
    Some(RestSettings {
        start_when_any: c.rest.start_when_any,
        stop_when_all: c.rest.stop_when_all,
        command: c.rest.command.clone(),
        tell,
        // One rest command a round at most.
        retry: ROUND,
        sick_wait: SICK_WAIT,
    })
}

/// A solo bot's loop (#53), its places looked up. The config loader has checked that each
/// one is a room.
fn routine_for(c: &CharacterConfig) -> Option<RoutineSettings> {
    let r = c.routine.as_ref()?;
    Some(RoutineSettings {
        camp: c.place(&r.camp)?,
        rest_at: c.place(&r.rest_at)?,
        retreat: c.rest.start_when_any,
        look_every: r.look_every,
        hunts: r
            .hunts
            .iter()
            .filter_map(|h| {
                Some(HuntSettings {
                    monster: h.monster.clone(),
                    at: c.place(&h.at)?,
                    every: h.every,
                    min_hp: h.min_hp,
                })
            })
            .collect(),
        retry: ROUTINE_RETRY,
        kill_wait: KILL_WAIT,
    })
}

/// Invites a bot by injecting `invite <bot>` as its leader, when the leader is played
/// through the proxy and in the game (design §6.5, `party.invite_by_injection`).
#[derive(Debug)]
struct SeatInviter(Weak<Server>);

impl Inviter for SeatInviter {
    fn invite(&self, leader: &str, bot: &str) -> bool {
        let Some(server) = self.0.upgrade() else {
            return false;
        };
        let Some(leader) = server
            .running()
            .into_iter()
            .find(|n| n.eq_ignore_ascii_case(leader))
        else {
            return false;
        };
        if !server.seat_in_game(&leader) {
            return false;
        }
        let done = server.inject(&leader, &format!("invite {bot}")).is_ok();
        server.record(
            "invite_by_injection",
            json!({"leader": leader, "bot": bot, "injected": done}),
        );
        done
    }
}

/// The game data's rooms, for a bot's `@where` and `@status`.
struct GameRooms(Arc<Mutex<Option<GameData>>>);

impl std::fmt::Debug for GameRooms {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GameRooms")
    }
}

impl RoomBook for GameRooms {
    fn room(&self, map: u32, room: u32) -> Option<RoomInfo> {
        let gd = self.0.lock().expect("gamedata lock");
        let gd = gd.as_ref()?;
        let r = gd.room(map.into(), room.into()).ok()??;
        // Only what `Obvious exits:` shows: a hidden or text exit isn't told.
        let exits = gd
            .exits(map.into(), room.into())
            .unwrap_or_default()
            .into_iter()
            .filter(|e| matches!(e.shows_as, ShowsAs::Plain | ShowsAs::Door))
            .map(|e| e.dir)
            .collect();
        Some(RoomInfo {
            name: r.name,
            exits,
        })
    }

    fn route(&self, from: (u32, u32), to: (u32, u32)) -> Option<Vec<mp_engine::WalkStep>> {
        let gd = self.0.lock().expect("gamedata lock");
        let steps = gd
            .as_ref()?
            .route((from.0.into(), from.1.into()), (to.0.into(), to.1.into()))
            .ok()??;
        steps
            .into_iter()
            .map(|s| {
                Some(mp_engine::WalkStep {
                    dir: s.dir,
                    to: (
                        u32::try_from(s.to_map).ok()?,
                        u32::try_from(s.to_room).ok()?,
                    ),
                    door: s.door,
                })
            })
            .collect()
    }
}
