//! Routines as rule lists (#65, design §14.6): `routines/<name>.yaml`, a list of rules read
//! top-down, the first that applies wins, the last being what the bot does when nothing
//! else is due. A character names one (`routine: newhaven-arena`), changes its parameters
//! (`routine: { use: newhaven-arena, with: { rest_below: 60% } }`), or writes one inline
//! (`routine: { do: [...] }`).
//!
//! **Parameters.** A routine declares its parameters and their defaults under `params:`,
//! and uses them as `${name}` in any value. A value that is only `${name}` takes the
//! parameter as YAML would read it (`50%`, `20`, `4m`); one with more around it takes the
//! text. A character may set only parameters the routine declares, and a routine may use
//! only parameters it declares.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_norway::Value;

use crate::character::{Amount, Pools};
use crate::error::ConfigError;
use crate::place::Place;
use crate::schema::one_key;

/// Which routine a character runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutineRef {
    /// A file in `routines/`, with parameters of the character's own.
    Named {
        name: String,
        with: BTreeMap<String, String>,
    },
    /// Written in the character's file.
    Inline(Box<Routine>),
}

/// A routine, its parameters filled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Routine {
    pub name: String,
    /// The area its places are looked for in first.
    pub places: Option<String>,
    /// What its walks never use (#64).
    pub avoid: Avoid,
    /// Rooms walked through only when the way round is longer than this many steps more.
    pub prefer_not: BTreeMap<String, u32>,
    /// Top-down: the first that applies wins.
    pub rules: Vec<RuleSpec>,
}

/// What a walk never uses. Rooms are places (aliases, slugs, names or keys, from the
/// compiled room files); areas are compiled areas; exits are silvermere's exit kinds as the
/// room files write them (`door`, `trap`, `toll`, ...).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Avoid {
    #[serde(default)]
    pub rooms: Vec<Place>,
    #[serde(default)]
    pub areas: Vec<String>,
    #[serde(default)]
    #[schemars(schema_with = "crate::schema::exit_kinds")]
    pub exits: Vec<String>,
}

/// One rule, written as a one-key map: `- rest: { at: road, when: { hp_below: 50% } }`, or
/// a bare word for one with nothing to say: `- gear`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleSpec {
    Rest(RestRule),
    Circuit(CircuitRule),
    Camp(CampRule),
    Gear(GearRule),
    Train(TrainRule),
}

/// Train a level at a trainer whenever the experience for it is there (#56): `- train`,
/// or `- train: { at: guild, up_to: 3 }`. The fee is the game's (5 silver a level in
/// Newhaven); one the bot can't pay is said, and tried again later.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TrainRule {
    /// The trainer; left out, those the compiled room files say train the character's
    /// class (or any class), in the routine's `places` area first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Place>,
    /// The highest level to train to: an arena's top level, say, since a level above it
    /// locks the bot out. Left out, as high as the trainer goes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub up_to: Option<u32>,
    #[serde(default)]
    pub when: When,
}

/// Keep the character's `gear:` (#55): check the inventory when the routine starts and
/// after each rest, put on what's carried but not in use, and buy what's missing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GearRule {
    #[serde(default)]
    pub when: When,
}

/// When a rule starts. Every condition given must hold; none given always holds. A rule
/// that has started goes on until it's done, unless a rule above it starts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct When {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hp_below: Option<Amount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mp_below: Option<Amount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hp_at_least: Option<Amount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mp_at_least: Option<Amount>,
}

/// Walk to a room and rest there.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RestRule {
    pub at: Place,
    /// Left out, it's refused by the check, which says what it needs.
    #[serde(default)]
    pub when: When,
    /// Rest until both reach these; the character's `rest.stop_when_all` when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<Pools>,
}

/// Visit lairs as each fills again (#67, design §14.7), as MudPlay's Auto-Lair: a lair's
/// clock starts at its last kill, and it's ready its respawn time later. The bot waits
/// next door, never stepping in early, and kills all it finds. A hunt of one monster is a
/// circuit of one lair. When no lair is ready the rules below it run.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CircuitRule {
    /// The lairs, as places.
    pub lairs: Vec<Place>,
    /// How long after its last kill each lair fills again, by the lair as written (its room
    /// number for the long form): `{ small-cavern: 4m }`. A lair left out takes the compiled
    /// room file's time.
    #[serde(
        default,
        deserialize_with = "crate::units::durations",
        serialize_with = "crate::units::ser_durations",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    #[schemars(schema_with = "crate::schema::durations")]
    pub respawn: BTreeMap<String, Duration>,
    /// How long the bot will wait outside a lair for it to fill, rather than give way to
    /// the rules below; none by default.
    #[serde(
        default,
        deserialize_with = "crate::units::duration",
        serialize_with = "crate::units::ser_duration"
    )]
    #[schemars(schema_with = "crate::schema::duration")]
    pub wait: Duration,
    /// Which lair next: `soonest`, the nearest that will be ready when the bot gets there,
    /// else the one whose fight can start first; or `throughput`, the nearest, counting
    /// time spent waiting as free.
    #[serde(default)]
    pub pick: Pick,
    #[serde(default)]
    pub when: When,
}

/// How a circuit picks its next lair.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Pick {
    #[default]
    Soonest,
    Throughput,
}

/// Stay in a room and fight what comes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CampRule {
    pub at: Place,
    /// How often the bot looks around (a bare Enter redraws the room), so it sees monsters
    /// that arrive.
    #[serde(
        default = "d_look",
        deserialize_with = "crate::units::duration",
        serialize_with = "crate::units::ser_duration"
    )]
    #[schemars(schema_with = "crate::schema::duration")]
    pub look_every: Duration,
}

fn d_look() -> Duration {
    Duration::from_secs(5)
}

impl<'de> Deserialize<'de> for RuleSpec {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = Value::deserialize(d)?;
        let (name, value) = match raw {
            Value::String(name) => (name, Value::Mapping(Default::default())),
            other => {
                let map =
                    BTreeMap::<String, Value>::deserialize(other).map_err(de::Error::custom)?;
                let mut it = map.into_iter();
                let (Some((name, value)), None) = (it.next(), it.next()) else {
                    return Err(de::Error::custom(
                        "a rule is its name, or a map with one key",
                    ));
                };
                (name, value)
            }
        };
        let e = de::Error::custom;
        match name.as_str() {
            "rest" => RestRule::deserialize(value).map(Self::Rest).map_err(e),
            "circuit" => CircuitRule::deserialize(value)
                .map(Self::Circuit)
                .map_err(e),
            "camp" => CampRule::deserialize(value).map(Self::Camp).map_err(e),
            "gear" => GearRule::deserialize(value).map(Self::Gear).map_err(e),
            "train" => TrainRule::deserialize(value).map(Self::Train).map_err(e),
            "hunt" => Err(de::Error::custom(
                "`hunt` is now a circuit of one lair (#67): \
                 `circuit: { lairs: [small-cavern], respawn: { small-cavern: 4m } }`",
            )),
            other => Err(de::Error::custom(format!(
                "unknown rule `{other}`: rest, circuit, camp, gear or train"
            ))),
        }
    }
}

impl Serialize for RuleSpec {
    /// A one-key map, or the bare `gear` when it says nothing more.
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if matches!(self, Self::Gear(g) if *g == GearRule::default()) {
            return s.serialize_str("gear");
        }
        if matches!(self, Self::Train(t) if *t == TrainRule::default()) {
            return s.serialize_str("train");
        }
        let mut m = s.serialize_map(Some(1))?;
        match self {
            Self::Rest(r) => m.serialize_entry("rest", r)?,
            Self::Circuit(c) => m.serialize_entry("circuit", c)?,
            Self::Camp(c) => m.serialize_entry("camp", c)?,
            Self::Gear(g) => m.serialize_entry("gear", g)?,
            Self::Train(t) => m.serialize_entry("train", t)?,
        }
        m.end()
    }
}

impl JsonSchema for RuleSpec {
    fn schema_name() -> Cow<'static, str> {
        "Rule".into()
    }

    fn json_schema(g: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A rule: a map with one key (rest, circuit, camp, gear or train), or the bare word `gear` or `train`.",
            "anyOf": [
                { "const": "gear" },
                { "const": "train" },
                one_key("rest", g.subschema_for::<RestRule>()),
                one_key("circuit", g.subschema_for::<CircuitRule>()),
                one_key("camp", g.subschema_for::<CampRule>()),
                one_key("gear", g.subschema_for::<GearRule>()),
                one_key("train", g.subschema_for::<TrainRule>())
            ]
        })
    }
}

/// A routine file as written, its parameters already filled in.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "RoutineFile", title = "mudparty routine file")]
pub(crate) struct Body {
    /// The file's format: 1.
    #[serde(default)]
    schema: Option<u32>,
    /// The routine's name; the file's name when left out.
    #[serde(default)]
    name: Option<String>,
    /// The area its places are looked for in first.
    #[serde(default)]
    places: Option<String>,
    /// Its parameters and their defaults, used as `${name}` in any value; a character may
    /// set them with `with:`.
    #[serde(default)]
    #[schemars(schema_with = "crate::schema::scalars")]
    params: BTreeMap<String, Value>,
    /// What its walks never use.
    #[serde(default)]
    avoid: Avoid,
    /// Rooms walked through only when the way round is longer than this many steps more.
    #[serde(default)]
    #[schemars(schema_with = "crate::schema::costs")]
    prefer_not: BTreeMap<String, u32>,
    /// Its rules, read top-down: the first that applies wins.
    #[serde(rename = "do")]
    rules: Vec<RuleSpec>,
}

/// A routine as its file would write it.
#[derive(Serialize)]
struct Written<'a> {
    schema: u32,
    #[serde(skip_serializing_if = "str::is_empty")]
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    places: &'a Option<String>,
    avoid: &'a Avoid,
    prefer_not: &'a BTreeMap<String, u32>,
    #[serde(rename = "do")]
    rules: &'a [RuleSpec],
}

impl Serialize for Routine {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        Written {
            schema: 1,
            name: &self.name,
            places: &self.places,
            avoid: &self.avoid,
            prefer_not: &self.prefer_not,
            rules: &self.rules,
        }
        .serialize(s)
    }
}

impl Routine {
    /// The routine as its file would write it, parameters filled in and defaults written
    /// out: what `check` prints (#66). It reads back the same.
    pub fn to_yaml(&self) -> String {
        serde_norway::to_string(self).expect("a routine always writes")
    }
}

/// A YAML scalar as text: `50%`, `20`, `true`.
fn scalar_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Fills in `${name}` throughout `v`.
fn fill(v: &mut Value, params: &BTreeMap<String, String>) -> Result<(), ConfigError> {
    match v {
        Value::String(s) => {
            if let Some(name) = s
                .strip_prefix("${")
                .and_then(|r| r.strip_suffix('}'))
                .filter(|n| !n.contains('}'))
            {
                let text = params.get(name).ok_or_else(|| unknown_param(name))?;
                *v = serde_norway::from_str(text).unwrap_or_else(|_| Value::String(text.clone()));
                return Ok(());
            }
            let mut out = String::new();
            let mut rest = s.as_str();
            while let Some(i) = rest.find("${") {
                out.push_str(&rest[..i]);
                let after = &rest[i + 2..];
                let Some(j) = after.find('}') else {
                    out.push_str(&rest[i..]);
                    rest = "";
                    break;
                };
                let name = &after[..j];
                out.push_str(params.get(name).ok_or_else(|| unknown_param(name))?);
                rest = &after[j + 1..];
            }
            out.push_str(rest);
            *s = out;
            Ok(())
        }
        Value::Sequence(items) => items.iter_mut().try_for_each(|i| fill(i, params)),
        Value::Mapping(m) => m.values_mut().try_for_each(|i| fill(i, params)),
        Value::Tagged(t) => fill(&mut t.value, params),
        _ => Ok(()),
    }
}

fn unknown_param(name: &str) -> ConfigError {
    ConfigError::new(format!(
        "${{{name}}} is used, but `{name}` isn't one of the routine's params:"
    ))
}

/// Reads a routine's text, its parameters (defaults overridden by `with`) filled in, and
/// checks it if `checked`. `name` is the routine's name when the text gives none.
fn read(
    text: &Value,
    name: &str,
    with: &BTreeMap<String, String>,
    checked: bool,
) -> Result<Routine, ConfigError> {
    let mut v = text.clone();
    let mut params: BTreeMap<String, String> = BTreeMap::new();
    if let Some(Value::Mapping(declared)) = v.get("params") {
        for (k, val) in declared {
            let (Some(k), Some(val)) = (k.as_str(), scalar_text(val)) else {
                return Err(ConfigError::at(
                    "params",
                    "each parameter is a name and a value like 50%, 20 or 4m",
                ));
            };
            params.insert(k.to_owned(), val);
        }
    }
    for (k, val) in with {
        if !params.contains_key(k) {
            return Err(ConfigError::at(
                format!("with.{k}"),
                format!("the routine {name} has no parameter `{k}`"),
            ));
        }
        params.insert(k.clone(), val.clone());
    }
    if let Value::Mapping(m) = &mut v {
        for (k, val) in m.iter_mut() {
            if k.as_str() != Some("params") {
                fill(val, &params)?;
            }
        }
    }
    let body: Body = serde_norway::from_value(v).map_err(|e| ConfigError::new(e.to_string()))?;
    if body.schema.is_some_and(|s| s != 1) {
        return Err(ConfigError::at(
            "schema",
            "this mudparty reads routine schema 1",
        ));
    }
    let _ = body.params;
    let r = Routine {
        name: body.name.unwrap_or_else(|| name.to_owned()),
        places: body.places,
        avoid: body.avoid,
        prefer_not: body.prefer_not,
        rules: body.rules,
    };
    if checked {
        check(&r)?;
    }
    Ok(r)
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

fn check_when(at: &str, w: &When) -> Result<(), ConfigError> {
    for (k, a) in [
        ("hp_below", w.hp_below),
        ("mp_below", w.mp_below),
        ("hp_at_least", w.hp_at_least),
        ("mp_at_least", w.mp_at_least),
    ] {
        check_amount(&format!("{at}.when.{k}"), a)?;
    }
    Ok(())
}

/// What makes a routine unusable, by field.
pub(crate) fn check(r: &Routine) -> Result<(), ConfigError> {
    if r.rules.is_empty() {
        return Err(ConfigError::at("do", "a routine needs at least one rule"));
    }
    for (i, rule) in r.rules.iter().enumerate() {
        let at = format!("do[{i}]");
        let place = |p: &Place| {
            if p.is_blank() {
                Err(ConfigError::at(format!("{at}.at"), "a place is needed"))
            } else {
                Ok(())
            }
        };
        match rule {
            RuleSpec::Rest(rest) => {
                place(&rest.at)?;
                if rest.when.hp_below.is_none() && rest.when.mp_below.is_none() {
                    return Err(ConfigError::at(
                        format!("{at}.when"),
                        "a rest rule says when: hp_below or mp_below",
                    ));
                }
                check_when(&at, &rest.when)?;
                if let Some(u) = rest.until {
                    check_amount(&format!("{at}.until.hp"), u.hp)?;
                    check_amount(&format!("{at}.until.mp"), u.mp)?;
                }
            }
            RuleSpec::Circuit(c) => {
                if c.lairs.is_empty() {
                    return Err(ConfigError::at(
                        format!("{at}.lairs"),
                        "a circuit needs at least one lair",
                    ));
                }
                if let Some(j) = c.lairs.iter().position(Place::is_blank) {
                    return Err(ConfigError::at(
                        format!("{at}.lairs[{j}]"),
                        "a place is needed",
                    ));
                }
                for (name, d) in &c.respawn {
                    if !c.lairs.iter().any(|l| l.as_key() == name.trim()) {
                        return Err(ConfigError::at(
                            format!("{at}.respawn.{name}"),
                            "isn't one of the circuit's lairs, as its `lairs:` writes it",
                        ));
                    }
                    if d.is_zero() {
                        return Err(ConfigError::at(
                            format!("{at}.respawn.{name}"),
                            "must be more than 0s",
                        ));
                    }
                }
                check_when(&at, &c.when)?;
            }
            RuleSpec::Camp(c) => place(&c.at)?,
            RuleSpec::Gear(g) => check_when(&at, &g.when)?,
            RuleSpec::Train(t) => {
                if let Some(p) = &t.at {
                    place(p)?;
                }
                if t.up_to.is_some_and(|l| l < 2) {
                    return Err(ConfigError::at(
                        format!("{at}.up_to"),
                        "the level to train up to: 2 or more",
                    ));
                }
                check_when(&at, &t.when)?;
            }
        }
        // A rule below one that always applies would never run.
        if let Some(j) = r.rules[..i].iter().position(always) {
            return Err(ConfigError::at(
                at,
                format!("this rule would never run: do[{j}] above it always applies"),
            ));
        }
    }
    for (i, kind) in r.avoid.exits.iter().enumerate() {
        if mp_data::places::exit_kind_code(kind).is_none() {
            return Err(ConfigError::at(
                format!("avoid.exits[{i}]"),
                format!("{kind:?} isn't an exit kind (door, gate, trap, toll, key, level, ...)"),
            ));
        }
    }
    if let Some(i) = r.avoid.rooms.iter().position(Place::is_blank) {
        return Err(ConfigError::at(
            format!("avoid.rooms[{i}]"),
            "a name is needed",
        ));
    }
    if let Some(i) = r.avoid.areas.iter().position(|n| n.trim().is_empty()) {
        return Err(ConfigError::at(
            format!("avoid.areas[{i}]"),
            "a name is needed",
        ));
    }
    Ok(())
}

/// Whether a rule applies whatever the bot looks like.
fn always(rule: &RuleSpec) -> bool {
    match rule {
        RuleSpec::Camp(_) => true,
        RuleSpec::Rest(_) | RuleSpec::Circuit(_) | RuleSpec::Gear(_) | RuleSpec::Train(_) => false,
    }
}

impl<'de> Deserialize<'de> for RoutineRef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        if let Some(name) = v.as_str() {
            return Ok(Self::Named {
                name: name.trim().to_owned(),
                with: BTreeMap::new(),
            });
        }
        let Some(m) = v.as_mapping() else {
            return Err(de::Error::custom(
                "a routine's name, { use: <name>, with: {...} }, or an inline { do: [...] }",
            ));
        };
        if let Some(name) = m.get("use").and_then(Value::as_str) {
            let mut with = BTreeMap::new();
            if let Some(w) = m.get("with") {
                let Some(w) = w.as_mapping() else {
                    return Err(de::Error::custom("with: is a map of parameters"));
                };
                for (k, val) in w {
                    let (Some(k), Some(val)) = (k.as_str(), scalar_text(val)) else {
                        return Err(de::Error::custom(
                            "each with: parameter is a name and a value like 50%, 20 or 4m",
                        ));
                    };
                    with.insert(k.to_owned(), val);
                }
            }
            if let Some(other) = m
                .keys()
                .filter_map(Value::as_str)
                .find(|k| *k != "use" && *k != "with")
            {
                return Err(de::Error::custom(format!(
                    "unknown field `{other}` beside use: (use and with)"
                )));
            }
            return Ok(Self::Named {
                name: name.trim().to_owned(),
                with,
            });
        }
        // Inline: read with its own defaults, and checked with the character (so its errors
        // name `routine.` fields); the character's name is filled in later.
        let r =
            read(&v, "", &BTreeMap::new(), false).map_err(|e| de::Error::custom(e.to_string()))?;
        Ok(Self::Inline(Box::new(r)))
    }
}

impl Serialize for RoutineRef {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Named { name, with } if with.is_empty() => s.serialize_str(name),
            Self::Named { name, with } => {
                let mut m = s.serialize_map(Some(2))?;
                m.serialize_entry("use", name)?;
                m.serialize_entry("with", with)?;
                m.end()
            }
            Self::Inline(r) => r.serialize(s),
        }
    }
}

impl JsonSchema for RoutineRef {
    fn schema_name() -> Cow<'static, str> {
        "RoutineRef".into()
    }

    fn json_schema(g: &mut SchemaGenerator) -> Schema {
        let with = crate::schema::scalars(g);
        json_schema!({
            "description": "A routine in routines/ by name, { use: <name>, with: {...} } to set its parameters, or one written here.",
            "anyOf": [
                { "type": "string", "minLength": 1 },
                {
                    "type": "object",
                    "properties": {
                        "use": { "description": "The routine's file in routines/, without .yaml.", "type": "string" },
                        "with": with
                    },
                    "required": ["use"],
                    "additionalProperties": false
                },
                g.subschema_for::<Body>()
            ]
        })
    }
}

/// The routine `r` names, read from `dir` (a data root's `routines/`) and checked, its
/// parameters filled in. An inline routine takes the character's name.
pub fn load_routine(dir: &Path, character: &str, r: &RoutineRef) -> Result<Routine, ConfigError> {
    match r {
        RoutineRef::Inline(routine) => {
            let mut routine = (**routine).clone();
            if routine.name.is_empty() {
                routine.name = character.to_owned();
            }
            Ok(routine)
        }
        RoutineRef::Named { name, with } => {
            let path = dir.join(format!("{name}.yaml"));
            let text = std::fs::read_to_string(&path)
                .map_err(|e| ConfigError::new(format!("the routine {name}: {e}")).in_file(&path))?;
            let v: Value = serde_norway::from_str(&text)
                .map_err(|e| ConfigError::new(e.to_string()).in_file(&path))?;
            read(&v, name, with, true).map_err(|e| e.in_file(&path))
        }
    }
}
