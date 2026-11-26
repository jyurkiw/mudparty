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

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Deserializer, de};
use serde_norway::Value;

use crate::character::{Amount, Pools};
use crate::error::ConfigError;

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
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Avoid {
    #[serde(default)]
    pub rooms: Vec<String>,
    #[serde(default)]
    pub areas: Vec<String>,
    #[serde(default)]
    pub exits: Vec<String>,
}

/// One rule, written as a one-key map: `- rest: { at: road, when: { hp_below: 50% } }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleSpec {
    Rest(RestRule),
    Hunt(HuntRule),
    Camp(CampRule),
}

/// When a rule starts. Every condition given must hold; none given always holds. A rule
/// that has started goes on until it's done, unless a rule above it starts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct When {
    #[serde(default)]
    pub hp_below: Option<Amount>,
    #[serde(default)]
    pub mp_below: Option<Amount>,
    #[serde(default)]
    pub hp_at_least: Option<Amount>,
    #[serde(default)]
    pub mp_at_least: Option<Amount>,
}

/// Walk to a room and rest there.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestRule {
    pub at: String,
    /// Left out, it's refused by the check, which says what it needs.
    #[serde(default)]
    pub when: When,
    /// Rest until both reach these; the character's `rest.stop_when_all` when left out.
    #[serde(default)]
    pub until: Option<Pools>,
}

/// A monster with a known spawn room and respawn time, such as Newhaven's cave bear.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HuntRule {
    /// The monster's name. A name the room shows that ends with it counts (`big cave bear`
    /// is a `cave bear`).
    pub monster: String,
    /// Its spawn room.
    pub at: String,
    /// How long after a kill, or a visit that found it gone, it's due again.
    #[serde(deserialize_with = "crate::units::duration")]
    pub every: Duration,
    #[serde(default)]
    pub when: When,
}

/// Stay in a room and fight what comes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CampRule {
    pub at: String,
    /// How often the bot looks around (a bare Enter redraws the room), so it sees monsters
    /// that arrive.
    #[serde(default = "d_look", deserialize_with = "crate::units::duration")]
    pub look_every: Duration,
}

fn d_look() -> Duration {
    Duration::from_secs(5)
}

impl<'de> Deserialize<'de> for RuleSpec {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let map = BTreeMap::<String, Value>::deserialize(d)?;
        let mut it = map.into_iter();
        let (Some((name, value)), None) = (it.next(), it.next()) else {
            return Err(de::Error::custom("a rule is a map with one key"));
        };
        let e = de::Error::custom;
        match name.as_str() {
            "rest" => RestRule::deserialize(value).map(Self::Rest).map_err(e),
            "hunt" => HuntRule::deserialize(value).map(Self::Hunt).map_err(e),
            "camp" => CampRule::deserialize(value).map(Self::Camp).map_err(e),
            other => Err(de::Error::custom(format!(
                "unknown rule `{other}`: rest, hunt or camp"
            ))),
        }
    }
}

/// A routine file as written, its parameters already filled in.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    #[serde(default)]
    schema: Option<u32>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    places: Option<String>,
    #[serde(default)]
    params: BTreeMap<String, Value>,
    #[serde(default)]
    avoid: Avoid,
    #[serde(default)]
    prefer_not: BTreeMap<String, u32>,
    #[serde(rename = "do")]
    rules: Vec<RuleSpec>,
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
        let place = |p: &str| {
            if p.trim().is_empty() {
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
            RuleSpec::Hunt(h) => {
                place(&h.at)?;
                if h.monster.trim().is_empty() {
                    return Err(ConfigError::at(
                        format!("{at}.monster"),
                        "a monster is needed",
                    ));
                }
                if h.every.is_zero() {
                    return Err(ConfigError::at(
                        format!("{at}.every"),
                        "must be more than 0s",
                    ));
                }
                check_when(&at, &h.when)?;
            }
            RuleSpec::Camp(c) => place(&c.at)?,
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
    for (field, names) in [("rooms", &r.avoid.rooms), ("areas", &r.avoid.areas)] {
        if let Some(i) = names.iter().position(|n| n.trim().is_empty()) {
            return Err(ConfigError::at(
                format!("avoid.{field}[{i}]"),
                "a name is needed",
            ));
        }
    }
    Ok(())
}

/// Whether a rule applies whatever the bot looks like.
fn always(rule: &RuleSpec) -> bool {
    match rule {
        RuleSpec::Camp(_) => true,
        RuleSpec::Rest(_) | RuleSpec::Hunt(_) => false,
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
