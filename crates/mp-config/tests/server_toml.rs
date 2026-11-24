//! `mudparty.toml`, the server's config (design §7.2).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use mp_config::{ServerConfig, StreamRoll};

/// The design's example, verbatim apart from the comments.
const EXAMPLE: &str = r#"
schema = 1
main_character = "Raijin"
logoff_timeout = "30s"

[game]
host = "127.0.0.1"
port = 2323

[listen]
client_hub = "127.0.0.1:7171"
proxy      = "127.0.0.1:2424"

[gamedata]
silvermere_db = "E:/silvermere-data/silvermere.db"

[stale]
vitals = "15s"
room   = "30s"
target = "6s"

[logging]
file_size      = "1MB"
plain_files    = 2
zip_archives   = 3
"#;

#[test]
fn the_design_example_reads() {
    let c = ServerConfig::from_toml(EXAMPLE).unwrap();
    assert_eq!(c.main_character.as_deref(), Some("Raijin"));
    assert_eq!(c.logoff_timeout, Duration::from_secs(30));
    assert_eq!((c.game.host.as_str(), c.game.port), ("127.0.0.1", 2323));
    assert_eq!(
        c.listen.client_hub,
        "127.0.0.1:7171".parse::<SocketAddr>().unwrap()
    );
    assert_eq!(
        c.listen.proxy,
        "127.0.0.1:2424".parse::<SocketAddr>().unwrap()
    );
    assert_eq!(
        c.gamedata.silvermere_db,
        PathBuf::from("E:/silvermere-data/silvermere.db")
    );
    assert_eq!(c.stale.vitals, Duration::from_secs(15));
    assert_eq!(c.stale.target, Duration::from_secs(6));
    assert_eq!(c.logging.file_size, 1024 * 1024);
    assert_eq!((c.logging.plain_files, c.logging.zip_archives), (2, 3));
}

#[test]
fn only_the_game_and_its_data_are_required() {
    let c = ServerConfig::from_toml(
        "schema = 1\n[game]\nhost = \"127.0.0.1\"\nport = 2323\n[gamedata]\nsilvermere_db = \"s.db\"\n",
    )
    .unwrap();
    assert_eq!(c.main_character, None);
    assert_eq!(c.logoff_timeout, Duration::from_secs(30));
    assert_eq!(c.listen.client_hub.port(), 7171);
    assert_eq!(c.listen.proxy.port(), 2424);
    assert_eq!(c.stale.room, Duration::from_secs(30));
    assert_eq!(c.logging.file_size, 1024 * 1024);
}

#[test]
fn a_stream_can_override_the_rolling() {
    let toml = format!("{EXAMPLE}\n[logging.wire]\nfile_size = \"5MB\"\n");
    let c = ServerConfig::from_toml(&toml).unwrap();
    assert_eq!(
        c.logging.roll_for("wire"),
        StreamRoll {
            file_size: 5 * 1024 * 1024,
            plain_files: 2,
            zip_archives: 3
        }
    );
    assert_eq!(
        c.logging.roll_for("transcript"),
        StreamRoll {
            file_size: 1024 * 1024,
            plain_files: 2,
            zip_archives: 3
        }
    );
}

#[test]
fn durations_and_sizes_take_their_usual_units() {
    let toml = EXAMPLE
        .replace("\"30s\"", "\"2m\"")
        .replace("vitals = \"15s\"", "vitals = \"1500ms\"")
        .replace("\"1MB\"", "\"512KB\"");
    let c = ServerConfig::from_toml(&toml).unwrap();
    assert_eq!(c.logoff_timeout, Duration::from_secs(120));
    assert_eq!(c.stale.vitals, Duration::from_millis(1500));
    assert_eq!(c.logging.file_size, 512 * 1024);
}

#[test]
fn a_bad_value_names_its_field() {
    let err = ServerConfig::from_toml(&EXAMPLE.replace("\"6s\"", "\"soon\"")).unwrap_err();
    assert!(err.to_string().contains("target"), "{err}");
}

#[test]
fn an_unknown_field_is_refused() {
    let err = ServerConfig::from_toml(&format!("{EXAMPLE}\n[game2]\nhost = \"x\"\n")).unwrap_err();
    assert!(err.to_string().contains("game2"), "{err}");
}

#[test]
fn another_schema_is_refused() {
    let err = ServerConfig::from_toml(&EXAMPLE.replace("schema = 1", "schema = 2")).unwrap_err();
    assert!(err.to_string().contains("schema"), "{err}");
}

#[test]
fn the_proxy_may_not_share_a_port_with_the_hub() {
    let err = ServerConfig::from_toml(&EXAMPLE.replace("2424", "7171")).unwrap_err();
    assert!(err.to_string().contains("proxy"), "{err}");
}
