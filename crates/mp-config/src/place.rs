//! A place as the files write it (design §14.3): a name, or a room by number.

use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_norway::Value;

/// A room, named one of two ways.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Place {
    /// An alias, a compiled slug (`weapons-shop`, `dungeon:entrance`), the game's own name
    /// while one room has it, or a key (`1/2150`): looked up in the compiled room files.
    Named(String),
    /// The long form, `{ room: 1/2150, name: "Newhaven, Arena" }`: the room by number,
    /// and the name it had, which `check` compares with the world's (#66). The web editor
    /// writes this form, so a file it saved stays readable when names repeat.
    Room {
        map: u32,
        room: u32,
        name: Option<String>,
    },
}

impl Place {
    /// A name with nothing in it.
    pub fn is_blank(&self) -> bool {
        matches!(self, Self::Named(n) if n.trim().is_empty())
    }

    /// The place as a map key names it: the name as written, or the long form's room
    /// number, `1/2156`.
    pub fn as_key(&self) -> String {
        match self {
            Self::Named(n) => n.trim().to_owned(),
            Self::Room { map, room, .. } => format!("{map}/{room}"),
        }
    }
}

/// A place written as this name.
impl PartialEq<&str> for Place {
    fn eq(&self, other: &&str) -> bool {
        matches!(self, Self::Named(n) if n == other)
    }
}

impl fmt::Display for Place {
    /// `arena`, `1/2150`, or `1/2150 (Newhaven, Arena)`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Named(n) => f.write_str(n),
            Self::Room {
                map,
                room,
                name: None,
            } => write!(f, "{map}/{room}"),
            Self::Room {
                map,
                room,
                name: Some(n),
            } => write!(f, "{map}/{room} ({n})"),
        }
    }
}

/// The long form as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Long {
    room: Value,
    #[serde(default)]
    name: Option<String>,
}

/// `1/2150` as `(1, 2150)`.
fn key(text: &str) -> Option<(u32, u32)> {
    let (map, room) = text.trim().split_once('/')?;
    Some((map.trim().parse().ok()?, room.trim().parse().ok()?))
}

impl<'de> Deserialize<'de> for Place {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match Value::deserialize(d)? {
            Value::String(s) => Ok(Self::Named(s)),
            v @ Value::Mapping(_) => {
                let long = Long::deserialize(v).map_err(de::Error::custom)?;
                let text = match &long.room {
                    Value::String(s) => s.clone(),
                    other => serde_norway::to_string(other).unwrap_or_default(),
                };
                let (map, room) = key(&text).ok_or_else(|| {
                    de::Error::custom(format!(
                        "room: is the room's number, like 1/2150, not {:?}",
                        text.trim()
                    ))
                })?;
                Ok(Self::Room {
                    map,
                    room,
                    name: long.name,
                })
            }
            _ => Err(de::Error::custom(
                "a place is a name, or { room: 1/2150, name: \"Newhaven, Arena\" }",
            )),
        }
    }
}

impl Serialize for Place {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Named(n) => s.serialize_str(n),
            Self::Room { map, room, name } => {
                let mut m = s.serialize_map(None)?;
                m.serialize_entry("room", &format!("{map}/{room}"))?;
                if let Some(n) = name {
                    m.serialize_entry("name", n)?;
                }
                m.end()
            }
        }
    }
}

impl JsonSchema for Place {
    fn schema_name() -> Cow<'static, str> {
        "Place".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A room: an alias, a slug (`weapons-shop`, `dungeon:entrance`), the game's name, a key (`1/2150`), or { room: 1/2150, name: \"Newhaven, Arena\" }.",
            "anyOf": [
                { "type": "string", "minLength": 1 },
                {
                    "type": "object",
                    "properties": {
                        "room": {
                            "description": "The room's number, map/room.",
                            "type": "string",
                            "pattern": "^\\s*\\d+\\s*/\\s*\\d+\\s*$"
                        },
                        "name": {
                            "description": "Its name, checked against the world's by `check`.",
                            "type": "string"
                        }
                    },
                    "required": ["room"],
                    "additionalProperties": false
                }
            ]
        })
    }
}
