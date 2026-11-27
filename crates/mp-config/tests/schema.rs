//! JSON Schemas for writing the files by hand (#66, design §14.9): generated from the config
//! types, kept in the repository's `schemas/` for editors to point to, and checked against
//! the README's examples.

use std::path::PathBuf;

use mp_config::schema;
use serde_json::Value;

/// The README's fenced `yaml` blocks, in order.
fn yaml_blocks() -> Vec<String> {
    let readme = include_str!("../../../README.md");
    let mut out = Vec::new();
    let mut lines = readme.lines();
    while let Some(line) = lines.next() {
        if line.trim() == "```yaml" {
            let body: Vec<&str> = lines.by_ref().take_while(|l| l.trim() != "```").collect();
            out.push(body.join("\n"));
        }
    }
    out
}

fn json(yaml: &str) -> Value {
    serde_norway::from_str(yaml).unwrap()
}

fn errors(schema: &Value, yaml: &str) -> Vec<String> {
    let v = jsonschema::validator_for(schema).unwrap();
    v.iter_errors(&json(yaml))
        .map(|e| format!("{}: {e}", e.instance_path()))
        .collect()
}

#[test]
fn every_readme_example_meets_its_schema() {
    let mut seen = 0;
    for b in yaml_blocks() {
        let schema = if b.starts_with("accounts:") {
            continue;
        } else if b.lines().any(|l| l.starts_with("area:")) {
            schema::places_local()
        } else if b.lines().any(|l| l.starts_with("do:")) {
            schema::routine()
        } else {
            schema::character()
        };
        let errs = errors(&schema, &b);
        assert!(errs.is_empty(), "{errs:#?}\nin:\n{b}");
        seen += 1;
    }
    assert!(seen >= 4, "{seen} examples");
}

#[test]
fn the_schema_marks_what_the_loader_refuses() {
    let c = schema::character();
    for (bad, field) in [
        ("schema: 1\nname: E\nrset: {}\n", "rset"),
        (
            "schema: 1\nname: E\nrest: { start_when_any: { hp: lots } }\n",
            "lots",
        ),
        (
            "schema: 1\nname: E\ngear: [{ item: club, use: swing }]\n",
            "swing",
        ),
        (
            "schema: 1\nname: E\ncombat: { targeting: [shout] }\n",
            "shout",
        ),
    ] {
        let errs = errors(&c, bad);
        assert!(!errs.is_empty(), "{bad}");
        assert!(errs.iter().any(|e| e.contains(field)), "{field}: {errs:?}");
    }
    let r = schema::routine();
    for bad in [
        "do:\n  - jump: { at: arena }\n",
        "do:\n  - camp: {}\n",
        "do:\n  - camp: { at: arena }\n    rest: { at: road }\n",
        "do:\n  - camp: { at: { room: arena } }\n",
        "do:\n  - circuit: { lairs: [cave], respawn: { cave: soon } }\n",
        "do:\n  - circuit: { lairs: [cave], pick: fastest }\n",
        "do:\n  - hunt: { monster: bear, at: cave, every: 4m }\n",
    ] {
        assert!(!errors(&r, bad).is_empty(), "{bad}");
    }
    // A parameter may stand where a value goes.
    let ok = "params: { below: 50% }\ndo:\n  - rest: { at: road, when: { hp_below: \"${below}\" } }\n  - camp: { at: arena }\n";
    assert_eq!(errors(&r, ok), Vec::<String>::new());
}

#[test]
fn the_schemas_folder_is_up_to_date() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
    for (file, schema) in schema::all() {
        let want = schema::to_text(&schema);
        let have = std::fs::read_to_string(dir.join(file)).unwrap_or_default();
        assert!(
            have.replace("\r\n", "\n") == want,
            "schemas/{file} is out of date: run `mudparty-server schema`"
        );
    }
}
