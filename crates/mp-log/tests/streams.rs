//! A character's log streams and the server log (design §9.2, §9.4).

use std::fs;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use mp_log::{CharacterLogs, FixedClock, Header, RollConfig, ServerLog, Source};
use serde_json::{Value, json};

const T0: &str = "2026-11-23T10:00:00.250Z";

fn header() -> Header {
    Header {
        character: "Ermias".into(),
        session: 7,
        config_rev: "c1".into(),
        gamedata_rev: "g1".into(),
        realm: "silvermere".into(),
    }
}

fn open(root: &Path) -> CharacterLogs {
    CharacterLogs::open(
        root,
        header(),
        RollConfig::default(),
        Box::new(FixedClock::at(T0)),
    )
    .unwrap()
}

fn jsonl(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn a_character_gets_four_streams_each_opening_with_a_header() {
    let root = tempfile::tempdir().unwrap();
    open(root.path()).close().unwrap();
    let dir = root.path().join("Ermias");
    let mut names: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "decisions.jsonl",
            "events.jsonl",
            "transcript.log",
            "wire.jsonl"
        ]
    );
    for f in ["decisions.jsonl", "events.jsonl", "wire.jsonl"] {
        let h = &jsonl(&dir.join(f))[0];
        assert_eq!(h["kind"], "header");
        assert_eq!(h["character"], "Ermias");
        assert_eq!(h["session"], 7);
        assert_eq!(h["config_rev"], "c1");
        assert_eq!(h["gamedata_rev"], "g1");
        assert_eq!(h["realm"], "silvermere");
        assert_eq!(h["mudparty"], env!("CARGO_PKG_VERSION"));
    }
    let t = fs::read_to_string(dir.join("transcript.log")).unwrap();
    assert!(
        t.starts_with(&format!(
            "{T0}  # mudparty {} session 7 Ermias",
            env!("CARGO_PKG_VERSION")
        )),
        "{t}"
    );
}

#[test]
fn wire_records_carry_time_sequence_direction_source_and_bytes() {
    let root = tempfile::tempdir().unwrap();
    let mut logs = open(root.path());
    logs.wire_in(b"\xff\xfb\x01Username: ").unwrap();
    logs.wire_out(b"north\r\n", &Source::Engine).unwrap();
    logs.wire_out(b"bg hi\r\n", &Source::Remote("Raijin".into()))
        .unwrap();
    logs.close().unwrap();
    let recs = jsonl(&root.path().join("Ermias/wire.jsonl"));
    let (a, b, c) = (&recs[1], &recs[2], &recs[3]);
    assert_eq!(a["t"], T0);
    assert_eq!(a["dir"], "in");
    assert_eq!(
        STANDARD.decode(a["b64"].as_str().unwrap()).unwrap(),
        b"\xff\xfb\x01Username: "
    );
    assert_eq!(b["dir"], "out");
    assert_eq!(b["src"], "engine");
    assert_eq!(c["src"], "remote:Raijin");
    let seqs: Vec<u64> = recs.iter().map(|r| r["seq"].as_u64().unwrap()).collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]), "{seqs:?}");
}

#[test]
fn the_sequence_is_shared_by_all_of_a_characters_streams() {
    let root = tempfile::tempdir().unwrap();
    let mut logs = open(root.path());
    logs.wire_in(b"x").unwrap();
    logs.decision("tier_pick", json!({"mp": 40})).unwrap();
    logs.event("kill", json!({"monster": "kobold"})).unwrap();
    logs.close().unwrap();
    let dir = root.path().join("Ermias");
    let w = jsonl(&dir.join("wire.jsonl"))[1]["seq"].as_u64().unwrap();
    let d = jsonl(&dir.join("decisions.jsonl"))[1]["seq"]
        .as_u64()
        .unwrap();
    let e = jsonl(&dir.join("events.jsonl"))[1]["seq"].as_u64().unwrap();
    assert!(w < d && d < e);
}

#[test]
fn decisions_and_events_merge_their_fields_with_the_kind() {
    let root = tempfile::tempdir().unwrap();
    let mut logs = open(root.path());
    logs.decision("tier_pick", json!({"mp": 40, "spell": "mmis"}))
        .unwrap();
    logs.close().unwrap();
    let d = &jsonl(&root.path().join("Ermias/decisions.jsonl"))[1];
    assert_eq!(d["kind"], "tier_pick");
    assert_eq!(d["mp"], 40);
    assert_eq!(d["spell"], "mmis");
    assert_eq!(d["t"], T0);
}

#[test]
fn the_transcript_shows_lines_and_sent_commands() {
    let root = tempfile::tempdir().unwrap();
    let mut logs = open(root.path());
    logs.transcript_line("Obvious exits: north").unwrap();
    logs.transcript_sent("north", &Source::Engine).unwrap();
    logs.close().unwrap();
    let t = fs::read_to_string(root.path().join("Ermias/transcript.log")).unwrap();
    let lines: Vec<&str> = t.lines().collect();
    assert_eq!(lines[1], format!("{T0}  Obvious exits: north"));
    assert_eq!(lines[2], format!("{T0}  > north  [engine]"));
}

#[test]
fn a_password_is_never_written_anywhere() {
    let root = tempfile::tempdir().unwrap();
    let mut logs = open(root.path());
    logs.wire_out_redacted(b"hunter22\r\n", "password", &Source::Engine)
        .unwrap();
    logs.transcript_sent_redacted(&Source::Engine).unwrap();
    logs.close().unwrap();
    let dir = root.path().join("Ermias");
    let wire = jsonl(&dir.join("wire.jsonl"));
    assert_eq!(wire[1]["dir"], "out");
    assert_eq!(wire[1]["redacted"], "password");
    assert_eq!(wire[1]["len"], 10);
    assert!(wire[1].get("b64").is_none());
    let t = fs::read_to_string(dir.join("transcript.log")).unwrap();
    assert!(t.contains("> ********  [engine]"));
    let b64 = STANDARD.encode(b"hunter22\r\n");
    for e in fs::read_dir(&dir).unwrap() {
        let bytes = fs::read(e.unwrap().path()).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("hunter22") && !text.contains(&b64));
    }
}

#[test]
fn sources_name_who_sent_a_line() {
    let s = |src: Source| src.to_string();
    assert_eq!(s(Source::Engine), "engine");
    assert_eq!(s(Source::User), "user");
    assert_eq!(s(Source::Remote("Bob".into())), "remote:Bob");
    assert_eq!(s(Source::Web), "web");
    assert_eq!(s(Source::Inject), "inject");
}

#[test]
fn the_server_log_is_one_jsonl_stream() {
    let root = tempfile::tempdir().unwrap();
    let mut log = ServerLog::open(
        root.path(),
        RollConfig::default(),
        Box::new(FixedClock::at(T0)),
    )
    .unwrap();
    log.record("bot_start", json!({"character": "Ermias"}))
        .unwrap();
    log.close().unwrap();
    let recs = jsonl(&root.path().join("server.jsonl"));
    assert_eq!(recs[0]["kind"], "header");
    assert_eq!(recs[1]["kind"], "bot_start");
    assert_eq!(recs[1]["character"], "Ermias");
    assert_eq!(recs[1]["t"], T0);
}
