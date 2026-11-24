//! The mudparty server (design §3.1): it owns the config, hosts the headless bots, and logs.
//!
//! In this step it starts and stops bots by name (one session per character), pushes an
//! edited character file to its running bot, and on an orderly stop logs every bot off.
//! The hub, the proxy and the web page come in later steps.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use mp_config::{Secrets, ServerConfig, load_character, load_server};
use mp_engine::{
    Command, LoginSettings, Offline, Report, SessionHandle, SessionSpec, SessionState, spawn,
};
use mp_log::{RollConfig, ServerLog, SystemClock};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::json;
use tokio::sync::{broadcast, mpsc};

/// Waits before each reconnect of a lost link; the last one repeats (design §6.2).
const RECONNECT: [u64; 4] = [5, 10, 30, 60];
/// How long a login step waits for the game.
const STEP_TIMEOUT: Duration = Duration::from_secs(30);

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
    runs: AtomicU64,
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
            runs: AtomicU64::new(0),
        });
        server.record("start", json!({"root": root.display().to_string()}));
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
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = spawn(spec, tx);
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
            while let Some(path) = rx.recv().await {
                let Some(me) = me.upgrade() else { break };
                me.reload(&path);
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
        let mut bots = self.bots.lock().expect("bots lock");
        let Some(bot) = bots.get_mut(&name) else {
            return;
        };
        match load_character(path) {
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
