//! The web page (design §4.2, §6.8; MVP 7): `GET /` serves the page, and `/ws/ui` feeds it
//! the party and takes its orders. Driven here as the page drives it, over a WebSocket.

use std::fs;
use std::net::SocketAddr;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use mp_silvermere::{StockChar, StockSilvermere, WAIT};
use mudparty_server::{Server, web};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{Instant, timeout};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

const ERMIAS: &str = r#"schema: 1
name: Ermias
class: Mage
login:
  account: Ermias
combat:
  targeting:
    - kill_order: true
  tiers:
    - spell: mmis
      min_mp: 1
  melee: a
automation:
  rest: false
"#;

/// The main character: played in MudPlay, so it has no login and can't be started.
const RAIJIN: &str = "schema: 1\nname: Raijin\nclass: Warrior\n";

struct Ui {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    /// The last party the server sent.
    party: Value,
}

impl Ui {
    async fn connect(addr: SocketAddr) -> Self {
        let (ws, _) = connect_async(format!("ws://{addr}/ws/ui")).await.unwrap();
        let mut ui = Self {
            ws,
            party: Value::Null,
        };
        // The snapshot comes first.
        let first = ui.next().await;
        assert_eq!(first["type"], "party", "{first}");
        ui
    }

    async fn next(&mut self) -> Value {
        loop {
            let msg = timeout(WAIT, self.ws.next())
                .await
                .expect("the server said nothing")
                .expect("the feed closed")
                .unwrap();
            if let Message::Text(t) = msg {
                let v: Value = serde_json::from_str(&t).unwrap();
                assert_eq!(v["v"], 1, "{v}");
                if v["type"] == "party" {
                    self.party = v.clone();
                }
                return v;
            }
        }
    }

    fn member(&self, name: &str) -> Value {
        self.party["members"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["name"] == name)
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// Reads until `name`'s card satisfies `want`, ticking the game clock meanwhile.
    async fn until(&mut self, sm: &StockSilvermere, name: &str, want: impl Fn(&Value) -> bool) {
        let deadline = Instant::now() + WAIT;
        while !want(&self.member(name)) {
            assert!(
                Instant::now() < deadline,
                "{name}'s card never got there: {}",
                self.member(name)
            );
            sm.tick(1).await;
            if let Ok(Some(Ok(Message::Text(t)))) =
                timeout(Duration::from_millis(200), self.ws.next()).await
            {
                let v: Value = serde_json::from_str(&t).unwrap();
                if v["type"] == "party" {
                    self.party = v;
                }
            }
        }
    }

    /// Reads until the output of the capture `id` comes, ticking the game clock meanwhile.
    async fn captured(&mut self, sm: &StockSilvermere, id: &str) -> Value {
        let deadline = Instant::now() + WAIT;
        loop {
            assert!(Instant::now() < deadline, "capture {id} never came");
            sm.tick(1).await;
            if let Ok(Some(Ok(Message::Text(t)))) =
                timeout(Duration::from_millis(200), self.ws.next()).await
            {
                let v: Value = serde_json::from_str(&t).unwrap();
                if v["type"] == "party" {
                    self.party = v;
                } else if v["type"] == "captured" && v["id"] == id {
                    return v;
                }
            }
        }
    }

    /// Asks for a character's text history and returns its lines.
    async fn history(&mut self, name: &str) -> Vec<Value> {
        let msg = json!({"v": 1, "type": "history", "name": name, "since": 0});
        self.ws
            .send(Message::Text(msg.to_string().into()))
            .await
            .unwrap();
        loop {
            let v = self.next().await;
            if v["type"] == "history" {
                assert_eq!(v["name"], name, "{v}");
                return v["lines"].as_array().unwrap().clone();
            }
        }
    }

    /// Sends an order and returns the server's answer to it.
    async fn order(&mut self, name: &str, order: Value) -> Value {
        let mut msg = json!({"v": 1, "type": "order", "name": name});
        msg.as_object_mut()
            .unwrap()
            .extend(order.as_object().unwrap().clone());
        self.ws
            .send(Message::Text(msg.to_string().into()))
            .await
            .unwrap();
        loop {
            let v = self.next().await;
            if v["type"] == "result" {
                return v;
            }
        }
    }
}

async fn get(addr: SocketAddr, path: &str) -> String {
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn the_page_shows_the_party_and_orders_its_bots() {
    let sm = StockSilvermere::start(
        &[
            StockChar::new("Raijin", "Warrior"),
            StockChar::new("Ermias", "Mage").knows(&["mmis"]),
        ],
        true,
    )
    .await
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let port = sm.telnet_addr().port();
    let db = sm.db_path().display().to_string().replace('\\', "/");
    fs::write(
        dir.path().join("mudparty.toml"),
        format!(
            "schema = 1\nmain_character = \"Raijin\"\nlogoff_timeout = \"2s\"\n[game]\nhost = \"127.0.0.1\"\nport = {port}\n[gamedata]\nsilvermere_db = \"{db}\"\n"
        ),
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("characters")).unwrap();
    fs::write(dir.path().join("characters/Ermias.yaml"), ERMIAS).unwrap();
    fs::write(dir.path().join("characters/Raijin.yaml"), RAIJIN).unwrap();
    fs::write(
        dir.path().join("secrets.yaml"),
        "accounts:\n  Ermias: \"ermias1\"\n",
    )
    .unwrap();

    let server = Server::start(dir.path()).await.unwrap();
    let addr = web::serve(server.clone(), "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();

    // The page itself, with the script that opens the feed.
    let page = get(addr, "/").await;
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page.contains("/ws/ui"), "the page opens the feed");

    let mut ui = Ui::connect(addr).await;
    // One card per character, the main character first; nothing is running yet.
    let names: Vec<&str> = ui.party["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Raijin", "Ermias"]);
    let raijin = ui.member("Raijin");
    assert_eq!(raijin["main"], true);
    assert_eq!(raijin["bot"], false, "the main character plays in MudPlay");
    let ermias = ui.member("Ermias");
    assert_eq!(
        (
            ermias["bot"].clone(),
            ermias["running"].clone(),
            ermias["mode"].clone()
        ),
        (json!(true), json!(false), json!("offline"))
    );
    assert_eq!(ermias["class"], "Mage");

    // Orders the server can't carry out are answered, not dropped.
    let r = ui.order("Nobody", json!({"order": "start"})).await;
    assert_eq!(r["ok"], false, "{r}");
    let r = ui.order("Raijin", json!({"order": "start"})).await;
    assert_eq!(r["ok"], false, "{r}");

    // Start: the card follows the bot into the game, with its vitals and its room.
    let r = ui.order("Ermias", json!({"order": "start"})).await;
    assert_eq!(r["ok"], true, "{r}");
    ui.until(&sm, "Ermias", |m| {
        m["mode"] == "idle" && m["vitals"]["hp"].is_number() && m["room"]["room"] == 206
    })
    .await;
    let ermias = ui.member("Ermias");
    assert_eq!(ermias["room"]["map"], 1);
    assert_eq!(ermias["room"]["name"], "Arena Practice Room");
    assert_eq!(ermias["automation"]["combat"], true);
    assert_eq!(ermias["automation"]["rest"], false);
    let r = ui.order("Ermias", json!({"order": "start"})).await;
    assert_eq!(r["ok"], false, "a second start is refused: {r}");

    // A command's output, for the sheet's help and look modals (#10, #101).
    let r = ui
        .order(
            "Ermias",
            json!({"order": "capture", "id": "races", "line": "help races"}),
        )
        .await;
    assert_eq!(r["ok"], true, "{r}");
    let c = ui.captured(&sm, "races").await;
    assert_eq!(c["name"], "Ermias");
    assert_eq!(c["line"], "help races");
    assert_eq!(c["answered"], true, "{c}");
    assert!(c["text"].as_str().unwrap().contains("Human"), "{c}");
    let r = ui
        .order(
            "Ermias",
            json!({"order": "capture", "id": "me", "line": "look Ermias"}),
        )
        .await;
    assert_eq!(r["ok"], true, "{r}");
    let c = ui.captured(&sm, "me").await;
    assert!(c["text"].as_str().unwrap().contains("Ermias"), "{c}");

    // The text history: what it sent and its notes, not the help and look it was asked
    // for, which have windows of their own.
    let lines = ui.history("Ermias").await;
    let has = |kind: &str, text: &str| {
        lines
            .iter()
            .any(|l| l["kind"] == kind && l["text"].as_str().unwrap().contains(text))
    };
    assert!(has("event", "in the game"), "{lines:?}");
    assert!(has("sent", "> help races"), "{lines:?}");
    assert!(
        !lines
            .iter()
            .any(|l| l["kind"] == "text" && l["text"].as_str().unwrap().contains("Human")),
        "the help's output stays out: {lines:?}"
    );
    let seqs: Vec<u64> = lines.iter().map(|l| l["seq"].as_u64().unwrap()).collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]), "{seqs:?}");
    // Raijin is played in MudPlay but not connected: no session to ask.
    let r = ui
        .order(
            "Raijin",
            json!({"order": "capture", "id": "no", "line": "help races"}),
        )
        .await;
    assert_eq!(r["ok"], false, "{r}");

    // The toggles.
    let r = ui
        .order(
            "Ermias",
            json!({"order": "toggle", "switch": "rest", "on": true}),
        )
        .await;
    assert_eq!(r["ok"], true, "{r}");
    ui.until(&sm, "Ermias", |m| m["automation"]["rest"] == true)
        .await;
    let r = ui
        .order(
            "Ermias",
            json!({"order": "toggle", "switch": "warp", "on": true}),
        )
        .await;
    assert_eq!(r["ok"], false, "an unknown switch: {r}");

    // Wait and ok hold and free the bot, as @wait and @ok from the page.
    ui.order("Ermias", json!({"order": "wait"})).await;
    ui.until(&sm, "Ermias", |m| {
        m["mode"] == "waiting" && m["automation"]["held_by"] == json!(["web"])
    })
    .await;
    ui.order("Ermias", json!({"order": "ok"})).await;
    ui.until(&sm, "Ermias", |m| m["automation"]["held_by"] == json!([]))
        .await;

    // The command line goes to the game, and the card's events show it went.
    ui.order(
        "Ermias",
        json!({"order": "command", "text": "say hello there"}),
    )
    .await;
    ui.until(&sm, "Ermias", |m| {
        m["events"]
            .as_array()
            .is_some_and(|e| e.iter().any(|e| e == "> say hello there"))
    })
    .await;

    // Stop logs the bot off, and its card says so.
    let r = ui.order("Ermias", json!({"order": "stop"})).await;
    assert_eq!(r["ok"], true, "{r}");
    ui.until(&sm, "Ermias", |m| {
        m["running"] == false && m["mode"] == "offline"
    })
    .await;
    assert_eq!(ui.member("Ermias")["offline"], "logged off");
    let r = ui.order("Ermias", json!({"order": "wait"})).await;
    assert_eq!(r["ok"], false, "nothing to hold: {r}");

    // Started again, it is back in the game.
    ui.order("Ermias", json!({"order": "start"})).await;
    ui.until(&sm, "Ermias", |m| {
        m["mode"] == "idle" && m["offline"].is_null()
    })
    .await;

    // The kill box: the card shows the target and the tier fighting it. (The practice
    // dummy fights back, and the game won't let a fighter quit, so this comes last.)
    ui.order(
        "Ermias",
        json!({"order": "kill", "target": "practice dummy"}),
    )
    .await;
    ui.until(&sm, "Ermias", |m| {
        m["mode"] == "fighting" && m["target"] == "practice dummy" && m["tier"] == "mmis"
    })
    .await;

    server.shutdown().await;
    sm.stop().await.unwrap();
}

#[tokio::test]
async fn the_page_listens_on_loopback_only() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("mudparty.toml"),
        "schema = 1\n[game]\nhost = \"127.0.0.1\"\nport = 1\n[gamedata]\nsilvermere_db = \"none.db\"\n",
    )
    .unwrap();
    let server = Server::start(dir.path()).await.unwrap();
    let err = web::serve(server.clone(), "0.0.0.0:0".parse().unwrap())
        .await
        .expect_err("a page with no login is for this machine only");
    assert!(err.to_string().contains("loopback"), "{err}");
    server.shutdown().await;
}
