//! A bot's log streams roll by `[logging]` and each stream's own table (design §9.3).

use mp_config::ServerConfig;
use mp_log::RollConfig;
use mudparty_server::log_rolls;

const MB: u64 = 1024 * 1024;

#[test]
fn each_bot_stream_takes_its_table_over_the_shared_settings() {
    let c = ServerConfig::from_toml(
        "schema = 1
[game]
host = \"127.0.0.1\"
port = 2323
[gamedata]
silvermere_db = \"x\"
[logging]
file_size = \"2MB\"
zip_archives = 5
[logging.wire]
file_size = \"8MB\"
zip_archives = 0
[logging.events]
plain_files = 4
",
    )
    .unwrap();
    let r = log_rolls(&c.logging);
    let shared = RollConfig {
        file_size: 2 * MB,
        plain_files: 2,
        zip_archives: 5,
    };
    assert_eq!(
        r.wire,
        RollConfig {
            file_size: 8 * MB,
            zip_archives: 0,
            ..shared
        }
    );
    assert_eq!(
        r.events,
        RollConfig {
            plain_files: 4,
            ..shared
        }
    );
    assert_eq!(r.transcript, shared);
    assert_eq!(r.decisions, shared);
}
