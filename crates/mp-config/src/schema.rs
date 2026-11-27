//! JSON Schemas for the files written by hand (#66, design §14.9), generated from the
//! config types, so an editor with a YAML extension completes keys, shows each field's doc
//! and marks errors as you type. `mudparty-server schema` writes them into `schemas/`; a
//! file points to one with a first line like
//! `# yaml-language-server: $schema=../schemas/character.schema.json`.
//!
//! The schemas say what a file may look like; the loaders still check what it means (a
//! rest rule's `when`, a percentage over 100%, a rule that would never run), and `check`
//! what the world makes of it.

use schemars::{Schema, json_schema, schema_for};
use serde_json::Value;

use crate::character::CharacterConfig;
use crate::routine::Body;

/// `${name}`: a routine's parameter, which may stand where any value goes.
const PARAM: &str = r"\$\{[^}]+\}";

/// A one-key map, `{ name: <schema> }`: how a rule is written.
pub(crate) fn one_key(name: &str, schema: Schema) -> Schema {
    let mut properties = serde_json::Map::new();
    properties.insert(name.to_owned(), schema.to_value());
    json_schema!({
        "type": "object",
        "properties": properties,
        "required": [name],
        "additionalProperties": false
    })
}

/// `50%` of the maximum, or a number of points.
pub(crate) fn amount_schema() -> Schema {
    json_schema!({
        "description": "A percentage of the maximum (`50%`) or a number of points.",
        "anyOf": [
            { "type": "integer", "minimum": 0 },
            { "type": "string", "pattern": format!(r"^\s*(\d+\s*%?|{PARAM})\s*$") }
        ]
    })
}

/// `250ms`, `30s`, `5m` or `1h`.
pub(crate) fn duration(_: &mut schemars::SchemaGenerator) -> Schema {
    json_schema!({
        "description": "A duration: `250ms`, `30s`, `5m` or `1h`.",
        "type": "string",
        "pattern": format!(r"^\s*(\d+(ms|s|m|h)|{PARAM})\s*$")
    })
}

/// Names to durations: `{ small-cavern: 4m }`.
pub(crate) fn durations(g: &mut schemars::SchemaGenerator) -> Schema {
    json_schema!({
        "type": "object",
        "additionalProperties": duration(g).to_value()
    })
}

/// A parameter's value: `50%`, `20`, `4m`, `true`.
pub(crate) fn scalars(_: &mut schemars::SchemaGenerator) -> Schema {
    json_schema!({
        "type": "object",
        "additionalProperties": { "type": ["string", "number", "boolean"] }
    })
}

/// Extra steps a room costs, or a parameter.
pub(crate) fn costs(_: &mut schemars::SchemaGenerator) -> Schema {
    json_schema!({
        "type": "object",
        "additionalProperties": {
            "anyOf": [
                { "type": "integer", "minimum": 0 },
                { "type": "string", "pattern": format!("^{PARAM}$") }
            ]
        }
    })
}

/// Exit kinds, by the names the room files give them.
pub(crate) fn exit_kinds(_: &mut schemars::SchemaGenerator) -> Schema {
    json_schema!({
        "type": "array",
        "items": { "enum": mp_data::places::exit_kinds() }
    })
}

/// A character's file, `characters/<name>.yaml`.
pub fn character() -> Value {
    schema_for!(CharacterConfig).to_value()
}

/// A routine's file, `routines/<name>.yaml`.
pub fn routine() -> Value {
    schema_for!(Body).to_value()
}

/// An area's hand-written companion, `places/<area>.local.yaml`.
pub fn places_local() -> Value {
    schema_for!(mp_data::places::Local).to_value()
}

/// Every schema with the file `schemas/` keeps it in.
pub fn all() -> Vec<(&'static str, Value)> {
    vec![
        ("character.schema.json", character()),
        ("routine.schema.json", routine()),
        ("places-local.schema.json", places_local()),
    ]
}

/// A schema as `schemas/` keeps it: pretty, with a newline at the end.
pub fn to_text(schema: &Value) -> String {
    let mut t = serde_json::to_string_pretty(schema).expect("a schema always writes");
    t.push('\n');
    t
}
