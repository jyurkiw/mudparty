//! The mudparty server (design §3.1): it owns the config, hosts the headless bots, and logs.
//!
//! It starts and stops bots by name (one session per character), pushes an edited
//! character file to its running bot, keeps the party view the web page shows (*§6.8*,
//! [`web`]), carries out the page's orders, and on an orderly stop logs every bot off.
//! The hub and the proxy come in later steps.

pub mod view;
pub mod web;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use mp_config::{CharacterConfig, PartyRole, Secrets, ServerConfig, load_character, load_server};
use mp_data::{GameData, ShowsAs};
use mp_engine::{
    AutoSwitch, CombatSettings, Command, Effect, LoginSettings, Offline, PartySettings,
    RemoteSettings, Report, RestSettings, RoomBook, RoomInfo, SessionHandle, SessionSpec,
    SessionState, spawn,
};
use mp_log::{RollConfig, ServerLog, SystemClock};
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
}

/// Who a `wait` or `ok` from the web page is from, in `held_by`.
pub const WEB: &str = "web";

struct Bot {
    handle: SessionHandle,
    rev: String,
    /// Tells this run of the bot from a later one under the same name.
    run: u64,
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
        });
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
        self.cards
            .lock()
            .expect("cards lock")
            .entry(c.name.clone())
            .and_modify(|card| card.configure(c))
            .or_insert_with(|| Card::new(c, main));
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
            Order::Command { text } => Command::Do(Effect::Send(text)),
            Order::Kill { target } => Command::Kill { target },
            Order::Wait => Command::Do(Effect::Wait(WEB.into())),
            Order::Ok => Command::Do(Effect::Ok(WEB.into())),
            Order::Join { leader } => Command::Do(Effect::Join(leader)),
        };
        let bots = self.bots.lock().expect("bots lock");
        let Some(bot) = bots.get(name) else {
            return Err(format!("{name} isn't running"));
        };
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
        let spec = SessionSpec {
            name: c.name.clone(),
            game: format!("{}:{}", self.config.game.host, self.config.game.port),
            login: LoginSettings {
                account: login.account.clone(),
                password: password.to_owned(),
                takeover: login.takeover,
                step_timeout: STEP_TIMEOUT,
            },
            statline: c.statline.clone(),
            config_rev: loaded.rev.clone(),
            logoff_timeout: self.config.logoff_timeout,
            reconnect_delays: RECONNECT.iter().map(|s| Duration::from_secs(*s)).collect(),
            logs: Some(self.root.join("logs")),
            combat: self.combat_for(&c),
            pacing: Default::default(),
            party: party_for(&c),
            rest: rest_for(&c),
            remote: Some(RemoteSettings {
                name: c.name.clone(),
                allow: c.remote.allow.clone(),
                channels: c.remote.channels.clone(),
                warn_denied: c.remote.warn_denied,
                version: env!("CARGO_PKG_VERSION").to_owned(),
            }),
            automation: c.automation,
            rooms: Some(Arc::new(GameRooms(Arc::clone(&self.gamedata)))),
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = spawn(spec, tx);
        let automation = c.automation;
        self.card_for(&c);
        self.card(name, |card| card.started(automation));
        let run = self.runs.fetch_add(1, Ordering::Relaxed);
        bots.insert(
            name.to_owned(),
            Bot {
                handle,
                rev: loaded.rev,
                run,
            },
        );
        drop(bots);
        self.record("bot_start", json!({"character": name}));
        let me = Arc::clone(self);
        let who = name.to_owned();
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
                let rooms = GameRooms(Arc::clone(&me.gamedata));
                me.card(&who, |card| {
                    card.report(&report, |m, r| rooms.room(m, r).map(|i| i.name));
                });
                let _ = me.events.send(BotEvent::Session {
                    name: who.clone(),
                    report,
                });
            }
        });
        Ok(())
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
}
