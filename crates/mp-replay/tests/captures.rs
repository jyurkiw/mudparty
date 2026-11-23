//! Replay tests (design §10.3): a wire capture replays through telnet, text and the parser,
//! and the result is compared with a stored snapshot.

use std::fs;
use std::path::Path;

use mp_log::{CharacterLogs, FixedClock, Header, RollConfig, Source};
use mp_replay::{read_wire, replay};
use silvermere_auth::login::AuthMsg;
use silvermere_text::{Msg, Palette, render};

fn r(m: Msg) -> Vec<u8> {
    render(&m, Palette::new(0).unwrap()).bytes
}

fn prompt(hp: i32, mp: i32) -> Vec<u8> {
    format!("\x1b[79D\x1b[K\x1b[0;37m[HP={hp}/120,MA={mp}/40]:").into_bytes()
}

fn s(v: &str) -> String {
    v.to_owned()
}

/// A bot's login, its statline, a fight and an @ command, written by mp-log exactly as a
/// session will write it, from silvermere's own rendered text.
fn write_login_and_fight(root: &Path) -> std::path::PathBuf {
    let header = Header {
        character: s("Ermias"),
        session: 1,
        config_rev: s("c1"),
        gamedata_rev: s("g1"),
        realm: s("silvermere"),
    };
    let clock = Box::new(FixedClock::at("2026-11-23T12:00:00.000Z"));
    let mut logs = CharacterLogs::open(root, header, RollConfig::default(), clock).unwrap();
    let gin = |logs: &mut CharacterLogs, b: &[u8]| logs.wire_in(b).unwrap();
    // silvermere's telnet offer, then the login dialogue.
    gin(
        &mut logs,
        &[255, 251, 1, 255, 251, 3, 255, 251, 0, 255, 253, 31],
    );
    gin(&mut logs, AuthMsg::UsernamePrompt.text().as_bytes());
    logs.wire_out(b"ermias\r\n", &Source::Engine).unwrap();
    gin(&mut logs, b"ermias\r\n");
    gin(&mut logs, AuthMsg::PasswordPrompt.text().as_bytes());
    logs.wire_out_redacted(b"hunter22\r\n", "password", &Source::Engine)
        .unwrap();
    gin(&mut logs, b"\r\n");
    gin(&mut logs, &r(Msg::BbsMenu));
    logs.wire_out(b"M\r\n", &Source::Engine).unwrap();
    gin(&mut logs, &r(Msg::MudMenuPrompt));
    logs.wire_out(b"E\r\n", &Source::Engine).unwrap();
    gin(&mut logs, b"E\r\n");
    gin(&mut logs, b"\x1b[79D\x1b[K\x1b[0;37m[HP=120/MA=40]:");
    let set = b"set statline full custom [HP=%h/%H,MA=%m/%M]:%r\r\n";
    logs.wire_out(set, &Source::Engine).unwrap();
    gin(&mut logs, set);
    gin(&mut logs, &prompt(120, 40));
    // The leader starts a fight, the bot joins it with a spell.
    gin(
        &mut logs,
        &r(Msg::CombatMovesToAttackMon {
            who: s("Raijin"),
            mon: s("small kobold"),
        }),
    );
    gin(&mut logs, &prompt(120, 40));
    logs.wire_out(b"mm small\r\n", &Source::Engine).unwrap();
    gin(&mut logs, b"mm small\r\n");
    gin(&mut logs, &r(Msg::CombatEngaged));
    gin(&mut logs, &prompt(120, 37));
    gin(&mut logs, &r(Msg::ExpGain { exp: 40 }));
    gin(&mut logs, &r(Msg::CombatOff));
    gin(&mut logs, &prompt(118, 37));
    // The leader asks for health over telepath; the bot answers and is acknowledged.
    gin(
        &mut logs,
        &r(Msg::Telepath {
            who: s("Raijin"),
            text: s("@health"),
        }),
    );
    gin(&mut logs, &prompt(118, 37));
    logs.wire_out(
        b"/raijin HP=118/120,MA=37/40\r\n",
        &Source::Remote(s("Raijin")),
    )
    .unwrap();
    gin(&mut logs, b"/raijin HP=118/120,MA=37/40\r\n");
    let mut ack = prompt(118, 37);
    ack.extend(r(Msg::TelepathSent { who: s("Raijin") }));
    gin(&mut logs, &ack);
    logs.close().unwrap();
    root.join("Ermias").join("wire.jsonl")
}

#[test]
fn a_bots_login_and_first_fight_replay_to_their_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let path = write_login_and_fight(root.path());
    let records = read_wire(&fs::read_to_string(path).unwrap()).unwrap();
    insta::assert_snapshot!(replay(&records).join("\n"));
}

#[test]
fn a_redacted_write_replays_as_redacted() {
    let root = tempfile::tempdir().unwrap();
    let path = write_login_and_fight(root.path());
    let out = replay(&read_wire(&fs::read_to_string(path).unwrap()).unwrap());
    assert!(
        out.iter()
            .any(|l| l == "> [redacted password, 10 bytes] [engine]"),
        "{out:#?}"
    );
    assert!(!out.iter().any(|l| l.contains("hunter22")));
}

/// Every capture under tests/captures replays to its own snapshot.
#[test]
fn the_capture_folder_replays_to_its_snapshots() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/captures");
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".wire.jsonl"))
        else {
            continue;
        };
        let records = read_wire(&fs::read_to_string(&path).unwrap()).unwrap();
        insta::assert_snapshot!(name.to_owned(), replay(&records).join("\n"));
    }
}
