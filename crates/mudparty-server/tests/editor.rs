//! The character editor's server side (#110, design §16): open a character's file, check a
//! change from the forms or the YAML tab, save it (with its revision, and a password), and
//! the choices the forms offer. On the stock world, with no game running.

use std::fs;
use std::path::Path;

use mp_config::revision;
use mp_silvermere::{StockChar, StockSilvermere};
use mudparty_server::{EditInput, Saved, Server, compile_places};
use serde_json::json;

const ERMIAS: &str = "# Ermias, by hand.
schema: 1
name: Ermias
class: Warrior
login:
  account: Ermias
combat:
  targeting: [kill_order, assist_leader]
  melee: a
";

const LOCAL: &str = "schema: 1
area: newhaven
aliases:
  arena: arena-2150
  road: narrow-road-2146
  bear: dungeon:small-cavern-2156
";

/// A data root on `db`, with Ermias's file, the example routine and the room files.
fn data_root(dir: &Path, db: &Path) {
    let db = db.display().to_string().replace('\\', "/");
    fs::write(
        dir.join("mudparty.toml"),
        format!("schema = 1\n[game]\nhost = \"127.0.0.1\"\nport = 2323\n[gamedata]\nsilvermere_db = \"{db}\"\n"),
    )
    .unwrap();
    fs::create_dir_all(dir.join("characters")).unwrap();
    fs::write(dir.join("characters/Ermias.yaml"), ERMIAS).unwrap();
    fs::create_dir_all(dir.join("routines")).unwrap();
    fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/newhaven/routines/newhaven-arena.yaml"
        ),
        dir.join("routines/newhaven-arena.yaml"),
    )
    .unwrap();
    compile_places(dir, None).unwrap();
    fs::write(dir.join("places/newhaven.local.yaml"), LOCAL).unwrap();
    fs::write(
        dir.join("secrets.yaml"),
        "accounts:\n  Ermias: \"ermias1\"\n  Kaelen: \"kaelen1\"\n",
    )
    .unwrap();
}

async fn server(
    chars: &[StockChar],
) -> (StockSilvermere, tempfile::TempDir, std::sync::Arc<Server>) {
    let sm = StockSilvermere::start(chars, false).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    data_root(dir.path(), &sm.db_path());
    let server = Server::start(dir.path()).await.unwrap();
    (sm, dir, server)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_opens_with_its_revision_and_what_the_forms_need() {
    let (sm, dir, server) = server(&[]).await;
    let o = server.edit_open("Ermias").unwrap();
    assert_eq!(o.yaml, ERMIAS);
    assert_eq!(o.rev, revision(ERMIAS.as_bytes()));
    assert!(o.comments, "it has a comment, which a form save would lose");
    assert!(o.password_set);
    assert_eq!(o.config["name"], "Ermias");
    assert_eq!(o.config["combat"]["melee"], "a");
    assert!(server.edit_open("Nobody").is_err());
    let _ = dir;
    server.shutdown().await;
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_change_is_checked_before_it_is_saved() {
    let (sm, _dir, server) = server(&[]).await;
    let mut config = server.edit_open("Ermias").unwrap().config;

    // A form's change: the YAML it would write, and nothing in the way.
    config["rest"]["start_when_any"] = json!({ "hp": "40%" });
    let c = server
        .edit_check("Ermias", EditInput::Config(config.clone()))
        .unwrap();
    assert_eq!(c.problems, Vec::<String>::new());
    assert!(c.yaml.contains("hp: 40%"), "{}", c.yaml);

    // YAML that doesn't read is a problem at its field.
    let bad = ERMIAS.replace(
        "melee: a",
        "melee: a\nrest:\n  start_when_any: { hp: 150% }",
    );
    let c = server.edit_check("Ermias", EditInput::Yaml(bad)).unwrap();
    assert!(
        c.problems
            .iter()
            .any(|p| p.contains("rest.start_when_any.hp")),
        "{:?}",
        c.problems
    );

    // `check`'s problems block too: a routine that isn't there.
    config["routine"] = json!("no-such-routine");
    let c = server
        .edit_check("Ermias", EditInput::Config(config))
        .unwrap();
    assert!(
        c.problems.iter().any(|p| p.contains("no-such-routine")),
        "{:?}",
        c.problems
    );

    // A trap is a warning, not a problem: targeting with nothing to attack with.
    let unarmed = ERMIAS.replace("  melee: a\n", "");
    let c = server
        .edit_check("Ermias", EditInput::Yaml(unarmed))
        .unwrap();
    assert_eq!(c.problems, Vec::<String>::new());
    assert!(
        c.warnings.iter().any(|w| w.contains("melee")),
        "{:?}",
        c.warnings
    );
    server.shutdown().await;
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_save_keeps_to_its_revision() {
    let (sm, dir, server) = server(&[]).await;
    let root = dir.path();
    let o = server.edit_open("Ermias").unwrap();

    // The YAML tab saves its text as typed, comments and all.
    let typed = ERMIAS.replace("melee: a", "melee: a   # swing");
    let Saved::Ok { rev } = server
        .edit_save("Ermias", Some(&o.rev), EditInput::Yaml(typed.clone()), None)
        .unwrap()
    else {
        panic!("saved");
    };
    assert_eq!(
        fs::read_to_string(root.join("characters/Ermias.yaml")).unwrap(),
        typed
    );
    assert_eq!(rev, revision(typed.as_bytes()));

    // The old revision is stale now: a conflict, with what's on disk.
    match server
        .edit_save("Ermias", Some(&o.rev), EditInput::Yaml(ERMIAS.into()), None)
        .unwrap()
    {
        Saved::Conflict { rev: now, yaml } => {
            assert_eq!(now, rev);
            assert_eq!(yaml, typed);
        }
        other => panic!("{other:?}"),
    }

    // A save with problems is refused, and the file is left alone.
    let bad = typed.replace(
        "melee: a",
        "melee: a\nrest:\n  start_when_any: { hp: 150% }",
    );
    assert!(matches!(
        server
            .edit_save("Ermias", Some(&rev), EditInput::Yaml(bad), None)
            .unwrap(),
        Saved::Refused(_)
    ));
    assert_eq!(
        fs::read_to_string(root.join("characters/Ermias.yaml")).unwrap(),
        typed
    );

    // A form's save writes the file afresh: every setting, comments gone.
    let mut config = server.edit_open("Ermias").unwrap().config;
    config["autostart"] = json!(true);
    let Saved::Ok { .. } = server
        .edit_save("Ermias", Some(&rev), EditInput::Config(config), None)
        .unwrap()
    else {
        panic!("saved");
    };
    let written = fs::read_to_string(root.join("characters/Ermias.yaml")).unwrap();
    assert!(written.contains("autostart: true"), "{written}");
    assert!(!written.contains('#'), "{written}");
    server.shutdown().await;
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_character_and_its_password() {
    let (sm, dir, server) = server(&[]).await;
    let root = dir.path();
    let braven = "schema: 1\nname: Braven\nclass: Paladin\nlogin:\n  account: Braven\n";
    let Saved::Ok { .. } = server
        .edit_save(
            "Braven",
            None,
            EditInput::Yaml(braven.into()),
            Some("braven1"),
        )
        .unwrap()
    else {
        panic!("saved");
    };
    assert_eq!(
        fs::read_to_string(root.join("characters/Braven.yaml")).unwrap(),
        braven
    );
    assert!(!root.join("characters/Braven.yml").exists());
    // The password went in, and the other accounts stayed.
    let secrets = fs::read_to_string(root.join("secrets.yaml")).unwrap();
    assert!(
        secrets.contains("Braven") && secrets.contains("braven1"),
        "{secrets}"
    );
    assert!(
        secrets.contains("kaelen1") && secrets.contains("ermias1"),
        "{secrets}"
    );
    assert!(server.edit_open("Braven").unwrap().password_set);
    // The card came with the file.
    assert!(server.party().iter().any(|c| c.name == "Braven"));

    // A new file for a name that has one is refused; so is a name its file doesn't match.
    assert!(
        server
            .edit_save("Braven", None, EditInput::Yaml(braven.into()), None)
            .is_err()
    );
    let grong = braven.replace("name: Braven", "name: Grong");
    assert!(matches!(
        server.edit_save("Grond", None, EditInput::Yaml(grong), None),
        Ok(Saved::Refused(_))
    ));
    assert!(!root.join("characters/Grond.yaml").exists());
    assert!(
        server
            .edit_save("../escape", None, EditInput::Yaml(braven.into()), None)
            .is_err()
    );
    server.shutdown().await;
    sm.stop().await.unwrap();
}

// The same through the page's feed.
#[tokio::test(flavor = "multi_thread")]
async fn the_editor_s_messages_go_through_the_feed() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    let (sm, _dir, server) = server(&[]).await;
    let addr = mudparty_server::web::serve(server.clone(), "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let (mut ws, _) = connect_async(format!("ws://{addr}/ws/ui")).await.unwrap();
    // Sends a message and reads until one of `kind` comes back.
    async fn ask(
        ws: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        msg: serde_json::Value,
        kind: &str,
    ) -> serde_json::Value {
        ws.send(Message::Text(msg.to_string().into()))
            .await
            .unwrap();
        loop {
            let Some(Ok(Message::Text(t))) = ws.next().await else {
                panic!("the feed closed");
            };
            let v: serde_json::Value = serde_json::from_str(&t).unwrap();
            if v["type"] == kind {
                return v;
            }
        }
    }
    let o = ask(
        &mut ws,
        json!({"v": 1, "type": "edit_open", "name": "Ermias"}),
        "edit",
    )
    .await;
    assert_eq!(o["yaml"], ERMIAS);
    assert_eq!(o["password_set"], true);
    let rev = o["rev"].as_str().unwrap().to_owned();
    let c = ask(
        &mut ws,
        json!({"v": 1, "type": "edit_check", "name": "Ermias", "yaml": ERMIAS.replace("  melee: a\n", "")}),
        "edit_checked",
    )
    .await;
    assert!(c["warnings"][0].as_str().unwrap().contains("melee"), "{c}");
    let typed = ERMIAS.replace("class: Warrior", "class: Warrior\nautostart: true");
    let s = ask(
        &mut ws,
        json!({"v": 1, "type": "edit_save", "name": "Ermias", "rev": rev, "yaml": typed}),
        "edit_saved",
    )
    .await;
    assert_ne!(s["rev"], rev);
    let conflict = ask(
        &mut ws,
        json!({"v": 1, "type": "edit_save", "name": "Ermias", "rev": rev, "yaml": ERMIAS}),
        "edit_conflict",
    )
    .await;
    assert_eq!(conflict["yaml"], typed);
    let choices = ask(
        &mut ws,
        json!({"v": 1, "type": "edit_choices", "name": ""}),
        "choices",
    )
    .await;
    assert!(
        choices["classes"].as_array().unwrap().len() > 3,
        "{choices}"
    );
    server.shutdown().await;
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_choices_come_from_the_game_and_the_files() {
    let (sm, _dir, server) = server(&[
        StockChar::new("Ermias", "Warrior"),
        StockChar::new("Kaelen", "Mage"),
    ])
    .await;
    let c = server.edit_choices().unwrap();
    assert!(
        c.classes.iter().any(|k| k.name == "Warrior"),
        "{:?}",
        c.classes
    );
    let mage = c.classes.iter().find(|k| k.name == "Mage").unwrap();
    assert!(mage.magery != 0);
    let mmis = c
        .spells
        .iter()
        .find(|s| s.short == "mmis")
        .expect("magic missile");
    assert!(mmis.mana > 0);
    assert_eq!(mmis.magery, mage.magery);
    assert!(c.monsters.iter().any(|m| m == "kobold thief"));
    let club = c
        .items
        .iter()
        .find(|i| i.item == "club")
        .expect("a club for sale");
    assert!(club.shops.iter().any(|s| s == "1/2141"), "{:?}", club.shops);
    let arena = c
        .routines
        .iter()
        .find(|r| r.name == "newhaven-arena")
        .unwrap();
    assert_eq!(
        arena.params.get("rest_below").map(String::as_str),
        Some("50%")
    );
    assert_eq!(c.characters, ["Ermias"]);
    // In silvermere's database with no file yet: Kaelen, not Ermias.
    assert!(
        c.unfiled
            .iter()
            .any(|u| u.name == "Kaelen" && u.class == "Mage"),
        "{:?}",
        c.unfiled
    );
    assert!(!c.unfiled.iter().any(|u| u.name == "Ermias"));
    server.shutdown().await;
    sm.stop().await.unwrap();
}
