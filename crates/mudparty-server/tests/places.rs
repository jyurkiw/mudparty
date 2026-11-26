//! `mudparty-server places compile` (#63): the data root's `places/` folder written from the
//! silvermere database its `mudparty.toml` names.

use std::fs;

use mp_data::places::{Area, Places};
use mp_silvermere::stock_template;
use mudparty_server::compile_places;

fn root() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let db = stock_template().display().to_string().replace('\\', "/");
    fs::write(
        dir.path().join("mudparty.toml"),
        format!("schema = 1\n[game]\nhost = \"127.0.0.1\"\nport = 2323\n[gamedata]\nsilvermere_db = \"{db}\"\n"),
    )
    .unwrap();
    dir
}

#[test]
fn it_writes_every_area_into_places() {
    let dir = root();
    let written = compile_places(dir.path(), None).unwrap();
    assert!(written.len() > 100, "{} areas", written.len());
    let nh = Area::read(&dir.path().join("places/newhaven.yaml")).unwrap();
    assert_eq!(nh.rooms["weapons-shop"].key, "1/2141");
    let places = Places::load(&dir.path().join("places")).unwrap();
    assert_eq!(places.resolve("weapons-shop", None), Ok((1, 2141)));
}

#[test]
fn one_area_alone_and_the_local_file_kept() {
    let dir = root();
    fs::create_dir_all(dir.path().join("places")).unwrap();
    let local = "schema: 1\narea: newhaven\naliases:\n  arena: 1/2150\n";
    fs::write(dir.path().join("places/newhaven.local.yaml"), local).unwrap();
    let written = compile_places(dir.path(), Some("newhaven")).unwrap();
    assert_eq!(written.len(), 1);
    assert_eq!(written[0].0, "newhaven");
    assert!(dir.path().join("places/newhaven.yaml").exists());
    assert!(!dir.path().join("places/dungeon.yaml").exists());
    // Recompiling never touches the hand-written file.
    assert_eq!(
        fs::read_to_string(dir.path().join("places/newhaven.local.yaml")).unwrap(),
        local
    );
}

#[test]
fn an_area_that_isnt_there_is_an_error() {
    let dir = root();
    let e = compile_places(dir.path(), Some("atlantis")).unwrap_err();
    assert!(e.to_string().contains("atlantis"), "{e}");
}
