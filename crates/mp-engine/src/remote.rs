//! @ commands (design §6.3, §4.3), as pure logic: a telepath or gangpath in, what to do
//! and what to say out.
//!
//! Everything a player sees comes from MudPlay (`Game/Remote/*`, user 2026-11-22), because
//! MudPlay reads some of it back:
//! - The command is the first word, case-blind, less one trailing `!` (`@stop!`).
//! - A reply is wrapped in `{ }`, in plain ASCII (`WireText.Ascii`), and goes back on the
//!   channel the command came in on: `/<sender> {…}` or `bg {…}`. A gangpath reaches the
//!   whole gang, the speaker too, so the bot's own `gangpaths:` echo is ignored.
//! - An unknown `@word` is ordinary chat and gets nothing. A known command from a sender
//!   not on `remote.allow` gets the failure reply, `command invalid or not allowed`, only
//!   with `remote.warn_denied` on. Every other refusal follows the same setting.
//! - Anything with `reroll` in it, and an `@do` with `suicide`, is dropped with no answer
//!   whatever the setting, so a caller can't probe for the block. (MudPlay forwards a
//!   guarded `@do suicide` to its own `@suicide` handling; mudparty has none.)
//! - Replies are cut to 200 characters a part, as MudPlay's `@help` does, to keep inside
//!   one telepath.

use mp_config::{Automation, Channel};
use mp_parse::{Event, Item, Rest, Vitals};

use crate::pacing::Action;

/// Who may command the bot, and where it listens.
#[derive(Debug, Clone)]
pub struct RemoteSettings {
    /// The bot's own first name.
    pub name: String,
    /// First names allowed to send @ commands.
    pub allow: Vec<String>,
    /// The channels read for commands.
    pub channels: Vec<Channel>,
    /// Answer refusals (otherwise they are silent).
    pub warn_denied: bool,
    /// mudparty's version, for `@version`.
    pub version: String,
}

/// What the handler can see of the bot.
#[derive(Debug, Clone)]
pub struct BotView {
    /// The last prompt.
    pub vitals: Option<Vitals>,
    pub automation: Automation,
    /// `@auto-all off`: every engine held.
    pub all_off: bool,
    /// `@stop`: movement and automation paused until `@rego`.
    pub stopped: bool,
    /// The leader the bot follows, if any.
    pub following: Option<String>,
}

/// One automation engine `@auto-…` switches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoSwitch {
    Combat,
    Nuke,
    Rest,
}

/// What a command asks of the bot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// A reply (or refusal) to `to`, unbraced.
    Reply {
        channel: Channel,
        to: String,
        text: String,
    },
    /// A line for the game.
    Send(String),
    /// `@kill`: this monster first, until it dies.
    Kill(String),
    SetAuto(AutoSwitch, bool),
    /// `@auto-all`: `true` holds every engine, `false` lets them run again.
    AllOff(bool),
    /// `@stop` (`true`) or `@rego` (`false`).
    Stop(bool),
    /// `@wait` from this player: hold fight-picking until their `@ok`.
    Wait(String),
    Ok(String),
    /// `@join`: join this leader's party.
    Join(String),
}

impl Effect {
    /// The line or telepath the effect puts on the wire, if any.
    pub fn action(&self) -> Option<Action> {
        match self {
            Self::Reply { channel, to, text } => {
                let body = format!("{{{}}}", ascii(text));
                Some(match channel {
                    Channel::Telepath => Action::Telepath {
                        to: to.clone(),
                        text: body,
                    },
                    Channel::Gangpath => Action::Send(format!("bg {body}")),
                })
            }
            Self::Send(l) => Some(Action::Send(l.clone())),
            _ => None,
        }
    }
}

/// MudPlay's `WireText.Ascii`: printable ASCII kept, common typography spelled out,
/// anything else `?`.
fn ascii(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            ' '..='~' => out.push(c),
            '—' | '–' | '‒' | '‐' | '‑' | '−' => out.push('-'),
            '‘' | '’' | '‚' | '′' => out.push('\''),
            '“' | '”' | '„' | '″' => out.push('"'),
            '…' => out.push_str("..."),
            '·' | '•' => out.push('.'),
            '≥' => out.push_str(">="),
            '≤' => out.push_str("<="),
            '×' => out.push('x'),
            '→' => out.push_str("->"),
            '←' => out.push_str("<-"),
            '\u{a0}' | '\t' => out.push(' '),
            _ => out.push('?'),
        }
    }
    out
}

/// The commands, with their `@help` syntax and description (MudPlay's
/// `RemoteCommandCatalog`, worded for mudparty where it does something else).
const COMMANDS: &[(&str, &str, &str)] = &[
    (
        "@health",
        "@health",
        "HP / MA and resting-or-meditating state",
    ),
    ("@version", "@version", "the app name + version"),
    (
        "@help",
        "@help [command]",
        "the commands you're allowed to use, or one command's syntax",
    ),
    (
        "@auto-all",
        "@auto-all [on|off]",
        "kill switch: off stops every engine, on restores what was running",
    ),
    (
        "@auto-combat",
        "@auto-combat [on|off]",
        "toggle the auto-combat engine",
    ),
    (
        "@auto-nuke",
        "@auto-nuke [on|off]",
        "toggle the auto-nuke (offensive spell) engine",
    ),
    (
        "@auto-rest",
        "@auto-rest [on|off]",
        "toggle auto-rest (resting and meditating)",
    ),
    (
        "@stop",
        "@stop",
        "pauses movement and automation until @rego",
    ),
    ("@rego", "@rego", "resumes after @stop"),
    (
        "@do",
        "@do <command>",
        "sends the command verbatim to the game (highest-trust)",
    ),
    (
        "@kill",
        "@kill <target>",
        "makes the named monster the target until it dies",
    ),
    (
        "@wait",
        "@wait",
        "hold: picking new fights pauses until @ok releases it",
    ),
    ("@ok", "@ok", "releases a @wait hold"),
    (
        "@invite",
        "@invite",
        "asks you to invite the sender into your party",
    ),
    (
        "@join",
        "@join [leader]",
        "asks you to join the sender's (or the leader's) party",
    ),
];

const FAILURE: &str = "command invalid or not allowed";
const PART: usize = 200;

/// MudPlay's `HelpHandler.Chunk`: comma-joined parts of at most `max` characters.
fn chunks(items: &[&str], max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for item in items {
        if !cur.is_empty() && cur.len() + 2 + item.len() > max {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push_str(", ");
        }
        cur.push_str(item);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn on_off(arg: &str) -> Option<bool> {
    if arg.eq_ignore_ascii_case("on") {
        Some(true)
    } else if arg.eq_ignore_ascii_case("off") {
        Some(false)
    } else {
        None
    }
}

#[derive(Debug)]
pub struct Remote {
    set: RemoteSettings,
}

/// One command, as it came.
struct Request<'a> {
    sender: &'a str,
    channel: Channel,
    command: String,
    args: Vec<&'a str>,
}

impl Remote {
    pub fn new(set: RemoteSettings) -> Self {
        Self { set }
    }

    fn allowed(&self, who: &str) -> bool {
        self.set.allow.iter().any(|a| a.eq_ignore_ascii_case(who))
    }

    /// The next item the game sent; what it asks for, if it is a command.
    pub fn on_item(&self, item: &Item, view: &BotView) -> Vec<Effect> {
        let (channel, who, text) = match item {
            Item::Event(Event::Telepath { who, text }) => (Channel::Telepath, who, text),
            Item::Event(Event::Gangpath { who, text }) => (Channel::Gangpath, who, text),
            _ => return Vec::new(),
        };
        if !self.set.channels.contains(&channel) || who.eq_ignore_ascii_case(&self.set.name) {
            return Vec::new();
        }
        let mut words = text.split_whitespace();
        let Some(first) = words.next().filter(|w| w.starts_with('@')) else {
            return Vec::new();
        };
        let mut command = first.to_ascii_lowercase();
        if command.len() > 2 && command.ends_with('!') {
            command.pop();
        }
        let req = Request {
            sender: who,
            channel,
            command,
            args: words.collect(),
        };
        if !COMMANDS.iter().any(|(c, _, _)| *c == req.command) {
            return Vec::new();
        }
        let blocked = req.command != "@help"
            && (std::iter::once(req.command.as_str())
                .chain(req.args.iter().copied())
                .any(|w| w.to_ascii_lowercase().contains("reroll"))
                || (req.command == "@do"
                    && req.args.iter().any(|w| w.eq_ignore_ascii_case("suicide"))));
        if blocked {
            return Vec::new();
        }
        if !self.allowed(req.sender) {
            return self.refuse(&req, None);
        }
        self.handle(&req, view)
    }

    fn reply(&self, req: &Request, text: impl Into<String>) -> Effect {
        Effect::Reply {
            channel: req.channel,
            to: req.sender.to_owned(),
            text: text.into(),
        }
    }

    /// A refusal, said only with `warn_denied`.
    fn refuse(&self, req: &Request, reason: Option<&str>) -> Vec<Effect> {
        if !self.set.warn_denied {
            return Vec::new();
        }
        vec![self.reply(req, reason.unwrap_or(FAILURE))]
    }

    fn handle(&self, req: &Request, view: &BotView) -> Vec<Effect> {
        let arg = req.args.first().copied();
        match req.command.as_str() {
            "@health" => match view.vitals {
                None => self.refuse(req, Some("HP unknown - no prompt observed yet")),
                Some(v) => {
                    let mana = match (v.mp, v.max_mp) {
                        (Some(m), Some(max)) => format!(",MA={m}/{max}"),
                        _ => String::new(),
                    };
                    let pos = match v.rest {
                        Rest::Resting => ", Resting",
                        Rest::Meditating => ", Meditating",
                        Rest::Standing => "",
                    };
                    let max = v.max_hp.map_or_else(|| "?".to_owned(), |m| m.to_string());
                    vec![self.reply(req, format!("HP={}/{max}{mana}{pos}", v.hp))]
                }
            },
            "@version" => vec![self.reply(req, format!("mudparty {}", self.set.version))],
            "@help" => match arg {
                Some(want) => {
                    let key = want.trim_end_matches('!').to_ascii_lowercase();
                    let key = if key.starts_with('@') {
                        key
                    } else {
                        format!("@{key}")
                    };
                    match COMMANDS.iter().find(|(c, _, _)| *c == key) {
                        Some((_, syntax, desc)) => {
                            vec![self.reply(req, format!("{syntax} - {desc}"))]
                        }
                        None => self.refuse(
                            req,
                            Some(&format!(
                                "no such command '{want}' - try @help for the list"
                            )),
                        ),
                    }
                }
                None => {
                    let names: Vec<&str> = COMMANDS.iter().map(|(c, _, _)| *c).collect();
                    let mut out: Vec<Effect> = chunks(&names, PART)
                        .into_iter()
                        .map(|c| self.reply(req, c))
                        .collect();
                    out.push(self.reply(req, "(type @help <command> for its syntax)"));
                    out
                }
            },
            "@auto-all" => {
                let wanted_on = match arg {
                    None => view.all_off,
                    Some(a) => match on_off(a) {
                        Some(b) => b,
                        None => return self.refuse(req, Some("?")),
                    },
                };
                let state = if wanted_on { "on" } else { "off" };
                vec![
                    Effect::AllOff(!wanted_on),
                    self.reply(req, format!("@auto-all: {state}")),
                ]
            }
            "@auto-combat" | "@auto-nuke" | "@auto-rest" => {
                let (which, current) = match req.command.as_str() {
                    "@auto-combat" => (AutoSwitch::Combat, view.automation.combat),
                    "@auto-nuke" => (AutoSwitch::Nuke, view.automation.nuke),
                    _ => (AutoSwitch::Rest, view.automation.rest),
                };
                let wanted = match arg {
                    None => !current,
                    Some(a) => match on_off(a) {
                        Some(b) => b,
                        None => return self.refuse(req, Some("?")),
                    },
                };
                let state = if wanted { "on" } else { "off" };
                vec![
                    Effect::SetAuto(which, wanted),
                    self.reply(req, format!("{}: {state}", req.command)),
                ]
            }
            "@stop" if view.stopped => vec![self.reply(req, "already @stopped")],
            "@stop" => vec![Effect::Stop(true), self.reply(req, "movement paused")],
            "@rego" if !view.stopped => self.refuse(req, Some("nothing to resume")),
            "@rego" => vec![Effect::Stop(false), self.reply(req, "resuming")],
            "@do" if req.args.is_empty() => Vec::new(),
            "@do" => vec![Effect::Send(req.args.join(" ")), self.reply(req, "ok")],
            "@kill" if req.args.is_empty() => self.refuse(req, None),
            "@kill" => vec![Effect::Kill(req.args.join(" "))],
            "@wait" => vec![Effect::Wait(req.sender.to_owned())],
            "@ok" => vec![Effect::Ok(req.sender.to_owned())],
            "@invite" => match &view.following {
                Some(l) => self.refuse(req, Some(&format!("I'm following {l}; denied."))),
                None => vec![Effect::Send(format!("invite {}", req.sender))],
            },
            "@join" => {
                let leader = arg.unwrap_or(req.sender).to_owned();
                match &view.following {
                    // Following the one who asks: our idea of the party is stale, so the
                    // join is taken (MudPlay's OnJoin).
                    Some(l) if !l.eq_ignore_ascii_case(req.sender) => {
                        self.refuse(req, Some(&format!("I'm following {l}; denied.")))
                    }
                    _ => vec![Effect::Join(leader)],
                }
            }
            _ => Vec::new(),
        }
    }
}
