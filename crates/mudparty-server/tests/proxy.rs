//! The MudPlay proxy (design §2, §2.1, §10.4 scenario 8; MVP 9a): a seat on the proxy port
//! looks to MudPlay exactly like silvermere, the bytes pass through unchanged both ways,
//! the session is named from the account MudPlay types, and the tap puts the main
//! character on the party page.

use std::fs;
use std::net::SocketAddr;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use mp_engine::{Report, SessionState};
use mp_parse::Item;
use mp_silvermere::{RawClient, StockChar, StockSilvermere, WAIT};
use mp_telnet::{ClientCodec, ServerCodec};
use mudparty_server::{BotEvent, Server, proxy};
use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tokio::sync::broadcast;
use tokio::time::{Instant, timeout};

const RAIJIN: &str = "schema: 1\nname: Raijin\nclass: Warrior\n";
const ERMIAS: &str = "schema: 1\nname: Ermias\nclass: Mage\nlogin:\n  account: Ermias\n";

async fn until(rx: &mut broadcast::Receiver<BotEvent>, who: &str, want: impl Fn(&Report) -> bool) {
    let deadline = Instant::now() + WAIT;
    loop {
        let wait = deadline.saturating_duration_since(Instant::now());
        match timeout(wait, rx.recv()).await {
            Ok(Ok(BotEvent::Session { name, report })) if name == who && want(&report) => return,
            Ok(Ok(_)) => {}
            _ => panic!("never saw the report from {who}"),
        }
    }
}

/// The first `n` bytes a fresh connection to `addr` is sent.
async fn opening(addr: SocketAddr, n: usize) -> Vec<u8> {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let mut got = Vec::new();
    let mut buf = [0u8; 256];
    while got.len() < n {
        let k = timeout(WAIT, s.read(&mut buf)).await.unwrap().unwrap();
        assert!(k > 0, "closed early");
        got.extend_from_slice(&buf[..k]);
    }
    got.truncate(n);
    got
}

/// The session's wire log: the data bytes it read from the game, and what the user sent.
fn wire(root: &Path, name: &str) -> (Vec<u8>, Vec<u8>, Vec<Value>) {
    let text = fs::read_to_string(root.join("logs").join(name).join("wire.jsonl")).unwrap();
    let (mut game, mut user, mut redacted) = (Vec::new(), Vec::new(), Vec::new());
    let mut telnet = ClientCodec::new(80, 24);
    for line in text.lines() {
        let rec: Value = serde_json::from_str(line).unwrap();
        if rec["kind"] != "wire" {
            continue;
        }
        if rec.get("redacted").is_some() {
            redacted.push(rec);
            continue;
        }
        let bytes = STANDARD.decode(rec["b64"].as_str().unwrap()).unwrap();
        match (rec["dir"].as_str(), rec["src"].as_str()) {
            (Some("in"), _) => telnet.decode(&bytes, &mut game, &mut Vec::new()),
            (Some("out"), Some("user")) => user.extend_from_slice(&bytes),
            _ => {}
        }
    }
    (game, user, redacted)
}

/// Every file under `dir`, read whole.
fn all_bytes(dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            all_bytes(&p, out);
        } else {
            out.push((p.display().to_string(), fs::read(&p).unwrap()));
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn mudplay_plays_the_main_character_through_a_seat() {
    let sm = StockSilvermere::start(
        &[
            StockChar::new("Raijin", "Warrior"),
            StockChar::new("Ermias", "Mage"),
        ],
        false,
    )
    .await
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let port = sm.telnet_addr().port();
    let db = sm.db_path().display().to_string().replace('\\', "/");
    fs::write(
        dir.path().join("mudparty.toml"),
        format!(
            "schema = 1\nmain_character = \"Raijin\"\nlogoff_timeout = \"5s\"\n[game]\nhost = \"127.0.0.1\"\nport = {port}\n[gamedata]\nsilvermere_db = \"{db}\"\n"
        ),
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("characters")).unwrap();
    fs::write(dir.path().join("characters/Raijin.yaml"), RAIJIN).unwrap();
    fs::write(dir.path().join("characters/Ermias.yaml"), ERMIAS).unwrap();
    fs::write(
        dir.path().join("secrets.yaml"),
        "accounts:\n  Ermias: \"ermias1\"\n",
    )
    .unwrap();

    let server = Server::start(dir.path()).await.unwrap();
    let addr = proxy::serve(server.clone(), "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let mut rx = server.subscribe();

    // The seat opens as silvermere does.
    assert_eq!(opening(addr, 12).await, ServerCodec::OFFERS);
    assert_eq!(opening(sm.telnet_addr(), 12).await, ServerCodec::OFFERS);

    // MudPlay logs in by itself. Raijin's file has no login, so the account it types names
    // the session through main_character.
    let mut mudplay = RawClient::log_in(addr, "Raijin", "raijin1").await.unwrap();
    until(&mut rx, "Raijin", |r| {
        *r == Report::State(SessionState::InGame)
    })
    .await;
    let card = server
        .party()
        .into_iter()
        .find(|c| c.name == "Raijin")
        .unwrap();
    assert!(card.running);
    assert_eq!(card.driver, Some("seat"));

    // What the user types goes to the game, and the tap reads what comes back: the card
    // gets the main character's vitals and room as MudPlay shows them.
    mudplay.send("/room").await.unwrap();
    mudplay
        .read_until(|i| matches!(i, Item::Event(mp_parse::Event::RoomKey { .. })))
        .await
        .unwrap();
    until(&mut rx, "Raijin", |r| {
        matches!(r, Report::Room { map: 1, room: 206 })
    })
    .await;
    let card = server
        .party()
        .into_iter()
        .find(|c| c.name == "Raijin")
        .unwrap();
    assert!(card.vitals.is_some(), "the tap read Raijin's prompt");
    assert_eq!(
        card.room.unwrap().name.as_deref(),
        Some("Arena Practice Room")
    );
    assert_eq!(
        card.mode, "idle",
        "a seated character's engines decide nothing"
    );

    // The seat leaves; MudPlay's link closes with it.
    server.shutdown().await;
    mudplay.read_to_close().await.unwrap();

    // Every data byte the session read from the game reached MudPlay unchanged, and every
    // byte MudPlay typed reached the game, the password aside.
    let (game, user, redacted) = wire(dir.path(), "Raijin");
    assert!(!game.is_empty());
    assert!(
        mudplay.data.ends_with(&game),
        "MudPlay saw something other than what the game sent"
    );
    let user = String::from_utf8_lossy(&user).into_owned();
    assert!(user.contains("/room\r\n"), "{user:?}");
    assert_eq!(redacted.len(), 1, "the password line: {redacted:?}");
    assert_eq!(redacted[0]["redacted"], "password");

    // And no password anywhere under the data root.
    let mut files = Vec::new();
    all_bytes(&dir.path().join("logs"), &mut files);
    for (path, bytes) in files {
        assert!(
            !bytes.windows(7).any(|w| w == b"raijin1"),
            "the password is in {path}"
        );
    }
    sm.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_account_typed_picks_the_character_by_its_login() {
    let sm = StockSilvermere::start(&[StockChar::new("Ermias", "Mage")], false)
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let port = sm.telnet_addr().port();
    fs::write(
        dir.path().join("mudparty.toml"),
        format!(
            "schema = 1\nmain_character = \"Raijin\"\n[game]\nhost = \"127.0.0.1\"\nport = {port}\n[gamedata]\nsilvermere_db = \"none.db\"\n"
        ),
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("characters")).unwrap();
    fs::write(dir.path().join("characters/Raijin.yaml"), RAIJIN).unwrap();
    fs::write(dir.path().join("characters/Ermias.yaml"), ERMIAS).unwrap();
    let server = Server::start(dir.path()).await.unwrap();
    let addr = proxy::serve(server.clone(), "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let mut rx = server.subscribe();
    // Typed in lower case: account names match case-blind.
    let _mudplay = RawClient::log_in(addr, "ermias", "ermias1").await.unwrap();
    until(&mut rx, "Ermias", |r| {
        *r == Report::State(SessionState::InGame)
    })
    .await;
    assert_eq!(server.running(), ["Ermias"]);
    server.shutdown().await;
    sm.stop().await.unwrap();
}

#[tokio::test]
async fn the_proxy_listens_on_loopback_only() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("mudparty.toml"),
        "schema = 1\n[game]\nhost = \"127.0.0.1\"\nport = 1\n[gamedata]\nsilvermere_db = \"none.db\"\n",
    )
    .unwrap();
    let server = Server::start(dir.path()).await.unwrap();
    let err = proxy::serve(server.clone(), "0.0.0.0:0".parse().unwrap())
        .await
        .expect_err("the proxy has no login of its own");
    assert!(err.to_string().contains("loopback"), "{err}");
    server.shutdown().await;
}
