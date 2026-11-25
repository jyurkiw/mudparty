//! The README's example files read as they say: every `toml` block is a whole
//! `mudparty.toml`, and every `yaml` block a character file (`schema:`) or a
//! `secrets.yaml` (`accounts:`). An example that drifts from the loaders fails here.

use mp_config::{CharacterConfig, Secrets, ServerConfig};

/// The README's fenced blocks in `lang`, in order.
fn blocks(lang: &str) -> Vec<String> {
    let readme = include_str!("../../../README.md");
    let mut out = Vec::new();
    let mut lines = readme.lines();
    while let Some(line) = lines.next() {
        if line.trim() == format!("```{lang}") {
            let body: Vec<&str> = lines.by_ref().take_while(|l| l.trim() != "```").collect();
            out.push(body.join("\n"));
        }
    }
    out
}

#[test]
fn every_mudparty_toml_in_the_readme_reads() {
    let found = blocks("toml");
    assert!(
        found.len() >= 2,
        "the README shows a short and a full mudparty.toml"
    );
    for b in found {
        if let Err(e) = ServerConfig::from_toml(&b) {
            panic!("{e}\nin:\n{b}");
        }
    }
}

#[test]
fn every_yaml_file_in_the_readme_reads() {
    let found = blocks("yaml");
    assert!(!found.is_empty());
    for b in found {
        let read = if b.starts_with("accounts:") {
            Secrets::from_yaml(&b).map(|_| ())
        } else {
            CharacterConfig::from_yaml(&b).map(|_| ())
        };
        if let Err(e) = read {
            panic!("{e}\nin:\n{b}");
        }
    }
}

#[test]
fn the_full_example_shows_every_default() {
    // The full example writes out every setting at its default, so it must read the same
    // as a file that leaves them all out.
    let full = blocks("toml")
        .into_iter()
        .find(|b| b.contains("[logging]"))
        .expect("a full example with [logging]");
    let mut c = ServerConfig::from_toml(&full).unwrap();
    let bare = ServerConfig::from_toml(
        "schema = 1\n[game]\nhost = \"127.0.0.1\"\nport = 2323\n[gamedata]\nsilvermere_db = \"x\"\n",
    )
    .unwrap();
    c.main_character = None;
    c.gamedata = bare.gamedata.clone();
    c.game = bare.game.clone();
    assert_eq!(c, bare);
}
