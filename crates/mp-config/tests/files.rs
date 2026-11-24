//! Reading the files from a data root, their revisions, and `secrets.yaml` (design §5.1,
//! §7.1, §7.4).

use std::fs;

use mp_config::{Secrets, load_character, load_server};

const SERVER: &str =
    "schema = 1\n[game]\nhost = \"127.0.0.1\"\nport = 2323\n[gamedata]\nsilvermere_db = \"s.db\"\n";

#[test]
fn a_loaded_file_carries_a_revision_of_its_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mudparty.toml");
    fs::write(&path, SERVER).unwrap();
    let a = load_server(&path).unwrap();
    let b = load_server(&path).unwrap();
    assert_eq!(a.rev, b.rev);
    assert_eq!(a.rev.len(), 12);
    fs::write(&path, format!("{SERVER}\n# a comment\n")).unwrap();
    assert_ne!(load_server(&path).unwrap().rev, a.rev);
}

#[test]
fn an_error_names_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Ermias.yaml");
    fs::write(
        &path,
        "schema: 1\nname: Ermias\nrest: { stop_when_all: { hp: 300% } }\n",
    )
    .unwrap();
    let e = load_character(&path).unwrap_err().to_string();
    assert!(
        e.contains("Ermias.yaml") && e.contains("rest.stop_when_all.hp"),
        "{e}"
    );
}

#[test]
fn a_characters_name_must_match_its_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Ermias.yaml");
    fs::write(&path, "schema: 1\nname: Raijin\n").unwrap();
    let e = load_character(&path).unwrap_err().to_string();
    assert!(e.contains("name"), "{e}");
}

#[test]
fn secrets_give_each_account_its_password() {
    let s = Secrets::from_yaml("accounts:\n  ermias: \"hunter22\"\n").unwrap();
    assert_eq!(s.password("ermias"), Some("hunter22"));
    assert_eq!(s.password("raijin"), None);
}

#[test]
fn secrets_never_show_a_password_when_printed() {
    let s = Secrets::from_yaml("accounts:\n  ermias: \"hunter22\"\n").unwrap();
    let shown = format!("{s:?}");
    assert!(
        shown.contains("ermias") && !shown.contains("hunter22"),
        "{shown}"
    );
}

#[test]
fn a_bad_secrets_file_never_shows_its_text_in_the_error() {
    let e = Secrets::from_yaml("accounts:\n  ermias: [hunter22\n")
        .unwrap_err()
        .to_string();
    assert!(!e.contains("hunter22"), "{e}");
}
