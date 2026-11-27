//! A character's YAML (design §7.3, §6.4, §6.6).

use std::borrow::Cow;
use std::collections::BTreeMap;

use mp_parse::Statline;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::error::ConfigError;
use crate::place::Place;
use crate::routine::RoutineRef;
use crate::schema::{amount_schema, one_key};

/// The statline a bot runs unless its file says otherwise (design §6.1).
pub const BOT_STATLINE: &str = "[HP=%h/%H,MA=%m/%M]:%r";

/// One character's file: a bot's, or the main character's shorter one.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "mudparty character file")]
pub struct CharacterConfig {
    pub schema: u32,
    pub name: String,
    /// The class, checked against game data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    /// Start as a headless bot when the server starts.
    #[serde(default)]
    pub autostart: bool,
    /// How a bot logs in; the main character logs in through MudPlay and has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login: Option<Login>,
    /// The statline template: set by a bot, read for the main character. Any template the
    /// prompt matcher can read will do. The tokens, as silvermere's `SET STATLINE CUSTOM`
    /// expands them (silvermere-text `statline.rs`):
    ///
    /// - `%h` HP, `%H` max HP, `%m` mana or kai, `%M` the maximum;
    /// - `%x` experience, `%X` experience still needed;
    /// - `%c` all coins in copper farthings, `%i` first name, `%w` the evil warning setting;
    /// - `%r` ` (Resting) ` or ` (Meditating) `, `%%` a `%`;
    /// - colours `%f0`–`%f7` (foreground), `%b0`–`%b7` (background), and `%B %N %U %L %R %d`.
    ///
    /// `%n` (a line break) isn't allowed, since the prompt must stay one line. silvermere
    /// keeps the first 60 characters.
    #[serde(default = "d_statline")]
    pub statline: String,
    /// Put the character's own statline back when the bot logs off cleanly (#34).
    #[serde(default = "yes")]
    pub statline_restore: bool,
    #[serde(default)]
    pub party: Party,
    #[serde(default)]
    pub remote: Remote,
    #[serde(default)]
    pub automation: Automation,
    #[serde(default)]
    pub combat: Combat,
    #[serde(default)]
    pub rest: Rest,
    /// What a character played in MudPlay does when MudPlay drops (design §2.1).
    #[serde(default)]
    pub unattended: Unattended,
    /// A solo bot's routine (#65): a file in `routines/`, with parameters of the
    /// character's own, or one written here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routine: Option<RoutineRef>,
    /// What the character carries (#55). A routine's `gear` rule keeps it so.
    #[serde(default)]
    pub gear: Vec<Gear>,
}

/// One item of a character's gear.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Gear {
    /// The item's name, as the game lists it (`padded vest`).
    pub item: String,
    /// `wield` or `wear`.
    #[serde(rename = "use")]
    pub how: GearUse,
    /// The shop to buy it at, a place; left out, a shop the compiled room files say sells
    /// it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Place>,
}

/// How an item is put to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GearUse {
    Wield,
    Wear,
}

/// A character whose MudPlay has dropped stays in the game, idle, under its engine. It
/// sets no statline of its own, so the user's comes back unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Unattended {
    /// Hang up when HP falls below this (`30%`, or points), after recording where the
    /// character is, so it can be found and rescued. `none` never hangs up.
    #[serde(
        default = "d_hang_up",
        deserialize_with = "opt_amount",
        serialize_with = "ser_opt_amount"
    )]
    #[schemars(schema_with = "opt_amount_schema")]
    pub hang_up_below: Option<Amount>,
}

impl Default for Unattended {
    fn default() -> Self {
        Self {
            hang_up_below: d_hang_up(),
        }
    }
}

fn d_hang_up() -> Option<Amount> {
    Some(Amount::Percent(30))
}

/// An [`Amount`], or `none`.
fn opt_amount<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Amount>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        None(NoneWord),
        Amount(Amount),
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "lowercase")]
    enum NoneWord {
        None,
    }
    Ok(match Raw::deserialize(d)? {
        Raw::None(_) => None,
        Raw::Amount(a) => Some(a),
    })
}

/// An [`Amount`], or `none`, as [`opt_amount`] reads it.
fn ser_opt_amount<S: Serializer>(a: &Option<Amount>, s: S) -> Result<S::Ok, S::Error> {
    match a {
        None => s.serialize_str("none"),
        Some(a) => a.serialize(s),
    }
}

fn opt_amount_schema(g: &mut SchemaGenerator) -> Schema {
    let amount = g.subschema_for::<Amount>();
    json_schema!({ "anyOf": [{ "const": "none" }, amount] })
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Login {
    /// The account name; its password is `secrets.yaml` → `accounts.<account>`.
    pub account: String,
    #[serde(default)]
    pub steps: LoginSteps,
    /// Answer Y to silvermere's "already logged on" question (design §6.2).
    #[serde(default = "yes")]
    pub takeover: bool,
}

/// `default-silvermere`, or an explicit list of steps.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum LoginSteps {
    #[default]
    DefaultSilvermere,
    Steps(Vec<LoginStep>),
}

/// Wait for `wait`, then send `send` (`{account}` and `{password}` filled in).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoginStep {
    pub wait: String,
    pub send: String,
}

impl<'de> Deserialize<'de> for LoginSteps {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Named(String),
            Steps(Vec<LoginStep>),
        }
        match Raw::deserialize(d)? {
            Raw::Named(n) if n == "default-silvermere" => Ok(Self::DefaultSilvermere),
            Raw::Named(n) => Err(de::Error::custom(format!(
                "unknown login steps {n:?}: `default-silvermere` or a list of {{wait, send}}"
            ))),
            Raw::Steps(s) => Ok(Self::Steps(s)),
        }
    }
}

impl Serialize for LoginSteps {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::DefaultSilvermere => s.serialize_str("default-silvermere"),
            Self::Steps(steps) => steps.serialize(s),
        }
    }
}

impl JsonSchema for LoginSteps {
    fn schema_name() -> Cow<'static, str> {
        "LoginSteps".into()
    }

    fn json_schema(g: &mut SchemaGenerator) -> Schema {
        let steps = g.subschema_for::<Vec<LoginStep>>();
        json_schema!({
            "description": "`default-silvermere`, or a list of { wait, send }.",
            "anyOf": [{ "const": "default-silvermere" }, steps]
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PartyRole {
    #[default]
    Member,
    Leader,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Party {
    /// The leader to join after login.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leader: Option<String>,
    /// Have the server inject `invite <bot>` when the leader is the proxied main character.
    #[serde(default)]
    pub invite_by_injection: bool,
    #[serde(default)]
    pub role: PartyRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Telepath,
    Gangpath,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Remote {
    /// Who may send @ commands.
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default = "d_channels")]
    pub channels: Vec<Channel>,
    /// Answer a refused command with the MegaMud failure reply.
    #[serde(default)]
    pub warn_denied: bool,
}

impl Default for Remote {
    fn default() -> Self {
        Self {
            allow: Vec::new(),
            channels: d_channels(),
            warn_denied: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Automation {
    #[serde(default = "yes")]
    pub combat: bool,
    #[serde(default = "yes")]
    pub nuke: bool,
    #[serde(default = "yes")]
    pub rest: bool,
}

impl Default for Automation {
    fn default() -> Self {
        Self {
            combat: true,
            nuke: true,
            rest: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Combat {
    /// Evaluated top-down; the first rule naming a monster in the room wins.
    #[serde(default)]
    pub targeting: Vec<Rule>,
    /// The first tier whose `min_mp` current MP meets is cast, re-picked every round.
    #[serde(default)]
    pub tiers: Vec<Tier>,
    #[serde(default)]
    pub room_tiers: Vec<RoomTier>,
    #[serde(default)]
    pub melee: Melee,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stealth_opener: Option<StealthOpener>,
}

/// A targeting rule (design §6.4), written as a one-key map: `- assist_leader: true`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rule {
    /// An `@kill` or web "kill" target, until it dies.
    KillOrder(bool),
    /// Monster names, case-blind globs, in order.
    Priority(Vec<String>),
    /// What the leader is attacking.
    AssistLeader(bool),
    /// Whatever is attacking this bot.
    FightBack(bool),
}

impl<'de> Deserialize<'de> for Rule {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = serde_norway::Value::deserialize(d)?;
        // A rule that takes no value may be its bare name: `kill_order`.
        let (name, value) = match raw {
            serde_norway::Value::String(name) => (name, serde_norway::Value::Bool(true)),
            other => {
                let map = BTreeMap::<String, serde_norway::Value>::deserialize(other)
                    .map_err(de::Error::custom)?;
                let mut it = map.into_iter();
                let (Some((name, value)), None) = (it.next(), it.next()) else {
                    return Err(de::Error::custom(
                        "a targeting rule is its name, or a map with one key",
                    ));
                };
                (name, value)
            }
        };
        let flag = |v: serde_norway::Value| bool::deserialize(v).map_err(de::Error::custom);
        match name.as_str() {
            "kill_order" => Ok(Self::KillOrder(flag(value)?)),
            "priority" => Ok(Self::Priority(
                Vec::<String>::deserialize(value).map_err(de::Error::custom)?,
            )),
            "assist_leader" => Ok(Self::AssistLeader(flag(value)?)),
            "fight_back" => Ok(Self::FightBack(flag(value)?)),
            other => Err(de::Error::custom(format!(
                "unknown targeting rule `{other}`: kill_order, priority, assist_leader or fight_back"
            ))),
        }
    }
}

impl Serialize for Rule {
    /// A flag that's on as its bare name, as it may be written; the rest as a one-key map.
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let (name, on) = match self {
            Self::KillOrder(on) => ("kill_order", *on),
            Self::AssistLeader(on) => ("assist_leader", *on),
            Self::FightBack(on) => ("fight_back", *on),
            Self::Priority(names) => {
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("priority", names)?;
                return m.end();
            }
        };
        if on {
            return s.serialize_str(name);
        }
        let mut m = s.serialize_map(Some(1))?;
        m.serialize_entry(name, &on)?;
        m.end()
    }
}

impl JsonSchema for Rule {
    fn schema_name() -> Cow<'static, str> {
        "TargetingRule".into()
    }

    fn json_schema(g: &mut SchemaGenerator) -> Schema {
        let flag = g.subschema_for::<bool>();
        let names = g.subschema_for::<Vec<String>>();
        json_schema!({
            "description": "A targeting rule: its name, or a map with one key.",
            "anyOf": [
                { "enum": ["kill_order", "assist_leader", "fight_back"] },
                one_key("kill_order", flag.clone()),
                one_key("assist_leader", flag.clone()),
                one_key("fight_back", flag),
                one_key("priority", names)
            ]
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Tier {
    /// The short name the game accepts.
    pub spell: String,
    /// Cast only while MP is at least this.
    pub min_mp: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoomTier {
    pub spell: String,
    pub min_mp: i32,
    pub min_enemies: u32,
}

/// The attack command when no tier fits, or `none`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Melee {
    #[default]
    None,
    Command(String),
}

impl<'de> Deserialize<'de> for Melee {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(if s.trim() == "none" || s.trim().is_empty() {
            Self::None
        } else {
            Self::Command(s)
        })
    }
}

impl Serialize for Melee {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::None => s.serialize_str("none"),
            Self::Command(c) => s.serialize_str(c),
        }
    }
}

impl JsonSchema for Melee {
    fn schema_name() -> Cow<'static, str> {
        "Melee".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "The attack command when no tier fits (`a`, `bs`), or `none`.",
            "type": "string"
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Require {
    Sneaking,
    Hidden,
    Either,
}

/// The fork's spell backstab, for Avenger and Assassin bots.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StealthOpener {
    pub spell: String,
    pub require: Require,
}

/// `50%` of the maximum, or a number of points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Amount {
    Percent(i32),
    Points(i32),
}

impl<'de> Deserialize<'de> for Amount {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Points(i32),
            Text(String),
        }
        let bad =
            |s: &str| de::Error::custom(format!("{s:?} isn't a number or a percentage like 50%"));
        match Raw::deserialize(d)? {
            Raw::Points(n) => Ok(Self::Points(n)),
            Raw::Text(t) => match t.trim().strip_suffix('%') {
                Some(n) => n.trim().parse().map(Self::Percent).map_err(|_| bad(&t)),
                None => t.trim().parse().map(Self::Points).map_err(|_| bad(&t)),
            },
        }
    }
}

impl Serialize for Amount {
    /// `50%`, or a bare number of points.
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Percent(p) => s.serialize_str(&format!("{p}%")),
            Self::Points(n) => s.serialize_i32(*n),
        }
    }
}

impl JsonSchema for Amount {
    fn schema_name() -> Cow<'static, str> {
        "Amount".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        amount_schema()
    }
}

/// HP and MP amounts; either may be left out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Pools {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hp: Option<Amount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mp: Option<Amount>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Rest {
    /// Start when either falls below.
    #[serde(default)]
    pub start_when_any: Pools,
    /// Stop when both reach these.
    #[serde(default = "d_stop")]
    pub stop_when_all: Pools,
    /// `rest`, or `meditate` where the class has it.
    #[serde(default = "d_rest")]
    pub command: String,
    /// `@wait` to the leader at the start, `@ok` at the end.
    #[serde(default = "yes")]
    pub tell_leader: bool,
}

impl Default for Rest {
    fn default() -> Self {
        Self {
            start_when_any: Pools::default(),
            stop_when_all: d_stop(),
            command: d_rest(),
            tell_leader: true,
        }
    }
}

fn yes() -> bool {
    true
}
fn d_statline() -> String {
    BOT_STATLINE.to_owned()
}
fn d_channels() -> Vec<Channel> {
    vec![Channel::Telepath, Channel::Gangpath]
}
fn d_rest() -> String {
    "rest".to_owned()
}
fn d_stop() -> Pools {
    Pools {
        hp: Some(Amount::Percent(100)),
        mp: Some(Amount::Percent(100)),
    }
}

impl CharacterConfig {
    /// Reads and checks a character's YAML.
    pub fn from_yaml(text: &str) -> Result<Self, ConfigError> {
        let c: Self = serde_norway::from_str(text).map_err(|e| ConfigError::new(e.to_string()))?;
        c.validate()?;
        Ok(c)
    }

    /// The character as YAML, every setting written out, defaults too: what `check` prints
    /// (#66). It reads back the same.
    pub fn to_yaml(&self) -> String {
        serde_norway::to_string(self).expect("a character always writes")
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.schema != 1 {
            return Err(ConfigError::at(
                "schema",
                format!(
                    "schema {} isn't supported (this mudparty reads schema 1)",
                    self.schema
                ),
            ));
        }
        if self.name.trim().is_empty() {
            return Err(ConfigError::at("name", "a character needs a name"));
        }
        // A written login (#33): each step waits for some text.
        if let Some(Login {
            steps: LoginSteps::Steps(steps),
            ..
        }) = &self.login
        {
            if steps.is_empty() {
                return Err(ConfigError::at(
                    "login.steps",
                    "a list of {wait, send}, or default-silvermere",
                ));
            }
            if let Some(i) = steps.iter().position(|s| s.wait.trim().is_empty()) {
                return Err(ConfigError::at(
                    format!("login.steps[{i}].wait"),
                    "a step waits for some text from the game",
                ));
            }
        }
        Statline::compile(&self.statline)
            .map_err(|e| ConfigError::at("statline", e.to_string()))?;
        for (i, r) in self.combat.targeting.iter().enumerate() {
            if matches!(r, Rule::Priority(names) if names.iter().all(|n| n.trim().is_empty())) {
                return Err(ConfigError::at(
                    format!("combat.targeting[{i}]"),
                    "a priority rule needs at least one name",
                ));
            }
        }
        for (i, t) in self.combat.tiers.iter().enumerate() {
            check_tier(&format!("combat.tiers[{i}]"), &t.spell, t.min_mp)?;
        }
        for (i, t) in self.combat.room_tiers.iter().enumerate() {
            let at = format!("combat.room_tiers[{i}]");
            check_tier(&at, &t.spell, t.min_mp)?;
            if t.min_enemies == 0 {
                return Err(ConfigError::at(format!("{at}.min_enemies"), "at least 1"));
            }
        }
        let start = self.rest.start_when_any;
        let stop = self.rest.stop_when_all;
        for (name, a) in [
            ("rest.start_when_any.hp", start.hp),
            ("rest.start_when_any.mp", start.mp),
            ("rest.stop_when_all.hp", stop.hp),
            ("rest.stop_when_all.mp", stop.mp),
        ] {
            check_amount(name, a)?;
        }
        for (name, from, to) in [("hp", start.hp, stop.hp), ("mp", start.mp, stop.mp)] {
            if let (Some(Amount::Percent(a)), Some(Amount::Percent(b))) = (from, to)
                && b < a
            {
                return Err(ConfigError::at(
                    format!("rest.stop_when_all.{name}"),
                    format!("stops at {b}%, below the {a}% it starts at"),
                ));
            }
            if let (Some(Amount::Points(a)), Some(Amount::Points(b))) = (from, to)
                && b < a
            {
                return Err(ConfigError::at(
                    format!("rest.stop_when_all.{name}"),
                    format!("stops at {b}, below the {a} it starts at"),
                ));
            }
        }
        if self.rest.command.trim().is_empty() {
            return Err(ConfigError::at("rest.command", "a rest command is needed"));
        }
        for (i, g) in self.gear.iter().enumerate() {
            if g.item.trim().is_empty() {
                return Err(ConfigError::at(
                    format!("gear[{i}].item"),
                    "an item is needed",
                ));
            }
            if g.at.as_ref().is_some_and(Place::is_blank) {
                return Err(ConfigError::at(
                    format!("gear[{i}].at"),
                    "a place is needed",
                ));
            }
        }
        if let Some(RoutineRef::Inline(r)) = &self.routine {
            crate::routine::check(r).map_err(|e| e.under("routine"))?;
        }
        Ok(())
    }
}

fn check_tier(at: &str, spell: &str, min_mp: i32) -> Result<(), ConfigError> {
    if spell.trim().is_empty() {
        return Err(ConfigError::at(format!("{at}.spell"), "a spell is needed"));
    }
    if min_mp < 0 {
        return Err(ConfigError::at(format!("{at}.min_mp"), "must be 0 or more"));
    }
    Ok(())
}

fn check_amount(at: &str, a: Option<Amount>) -> Result<(), ConfigError> {
    match a {
        Some(Amount::Percent(p)) if !(0..=100).contains(&p) => Err(ConfigError::at(
            at,
            format!("{p}% isn't between 0% and 100%"),
        )),
        Some(Amount::Points(n)) if n < 0 => Err(ConfigError::at(at, "must be 0 or more")),
        _ => Ok(()),
    }
}
