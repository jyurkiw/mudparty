//! The web page (design §4.2, §6.8): `GET /` serves one static page, and `/ws/ui` feeds it.
//!
//! The feed, as JSON text frames `{ "v": 1, "type": ..., ... }`:
//! - server → page `party`: every card (`members`, the main character first) and the
//!   running bots, on connecting and then on each change, at most four a second;
//! - page → server `order`: `{ "name": <character>, "order": <what>, ... }`, where `order`
//!   is `start`, `stop`, `toggle` (`switch`, `on`), `command` (`text`), `kill` (`target`),
//!   `wait`, `ok`, `join` (`leader`), `goto` (`room`: a key or a place, and `avoid`: places), `pause` or `resume`. See [`Order`]. For a character played in MudPlay,
//!   `command` injects, as `inject` (`text`) does;
//! - server → page `result`: the answer to each order, `ok` and a `message`;
//! - server → page `captured`: a command's output for a `capture` order (`id`, `line`,
//!   `text`, `answered`), to every page, which match it by `id`;
//! - page → server `history` (`name`, `since`): the character's text history after line
//!   `since`, answered with a `history` message (`lines`: `seq`, `at`, `kind`, `text`).
//!
//! There is no login, so the page is served on loopback only.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::bail;
use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::{Html, Response};
use axum::routing::get;
use serde_json::{Value, json};
use tokio::time::Instant;

use crate::{BotEvent, Order, Server};
use mp_engine::Report;
use tokio::sync::broadcast::error::RecvError;

/// The page: HTML and a small script, no build step.
const PAGE: &str = include_str!("../static/index.html");

/// The least time between two `party` messages on one feed.
const THROTTLE: Duration = Duration::from_millis(250);

/// Serves the page and its feed on `addr`, which must be a loopback address. Returns the
/// address bound (for port 0, the port picked).
pub async fn serve(server: Arc<Server>, addr: SocketAddr) -> anyhow::Result<SocketAddr> {
    if !addr.ip().is_loopback() {
        bail!("the web page has no login, so it listens on loopback only, not {addr}");
    }
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let bound = listener.local_addr()?;
    let app = Router::new()
        .route("/", get(|| async { Html(PAGE) }))
        .route("/ws/ui", get(upgrade))
        .with_state(server);
    tokio::spawn(async move { axum::serve(listener, app).await });
    Ok(bound)
}

async fn upgrade(ws: WebSocketUpgrade, State(server): State<Arc<Server>>) -> Response {
    ws.on_upgrade(move |socket| feed(socket, server))
}

fn party(server: &Server) -> String {
    json!({
        "v": 1,
        "type": "party",
        "members": server.party(),
        "server": { "running": server.running() },
    })
    .to_string()
}

async fn feed(mut socket: WebSocket, server: Arc<Server>) {
    let mut events = server.subscribe();
    let mut changes = server.watch_party();
    changes.mark_unchanged();
    if socket
        .send(Message::Text(party(&server).into()))
        .await
        .is_err()
    {
        return;
    }
    let mut last = Instant::now();
    loop {
        tokio::select! {
            changed = changes.changed() => {
                if changed.is_err() {
                    return;
                }
                // Changes that come close together go out as one message.
                tokio::time::sleep_until(last + THROTTLE).await;
                changes.mark_unchanged();
                last = Instant::now();
                if socket.send(Message::Text(party(&server).into())).await.is_err() {
                    return;
                }
            }
            // A command's output the page asked for.
            ev = events.recv() => match ev {
                Ok(BotEvent::Session { name, report: Report::Captured(c) }) => {
                    let out = json!({
                        "v": 1, "type": "captured", "name": name, "id": c.id,
                        "line": c.line, "text": c.text, "answered": c.answered,
                    });
                    if socket.send(Message::Text(out.to_string().into())).await.is_err() {
                        return;
                    }
                }
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return,
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Text(t))) => {
                    let reply = handle(&server, &t);
                    if socket.send(Message::Text(reply.to_string().into())).await.is_err() {
                        return;
                    }
                }
                Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                Some(Ok(_)) => {}
            }
        }
    }
}

/// One message from the page; returns the `result` to send back.
fn handle(server: &Arc<Server>, text: &str) -> Value {
    let result = |name: &str, r: Result<String, String>| {
        let (ok, message) = match r {
            Ok(m) => (true, m),
            Err(m) => (false, m),
        };
        json!({"v": 1, "type": "result", "name": name, "ok": ok, "message": message})
    };
    let Ok(Value::Object(mut msg)) = serde_json::from_str::<Value>(text) else {
        return result("", Err("not a JSON object".into()));
    };
    let name = msg
        .remove("name")
        .and_then(|n| n.as_str().map(str::to_owned))
        .unwrap_or_default();
    if msg.remove("v") != Some(json!(1)) {
        return result(&name, Err("unknown version; this server speaks v 1".into()));
    }
    match msg.remove("type").as_ref().and_then(Value::as_str) {
        Some("order") => {}
        // The text history after a line, for the card's History modal.
        Some("history") => {
            let since = msg.get("since").and_then(Value::as_u64).unwrap_or(0);
            return json!({
                "v": 1, "type": "history", "name": name, "lines": server.texts(&name, since),
            });
        }
        // Typing for a character played in MudPlay.
        Some("inject") => {
            let text = msg.get("text").and_then(Value::as_str).unwrap_or_default();
            return result(&name, server.inject(&name, text));
        }
        other => return result(&name, Err(format!("unknown message type {other:?}"))),
    }
    match serde_json::from_value::<Order>(Value::Object(msg)) {
        Ok(order) => result(&name, server.order(&name, order)),
        Err(e) => result(&name, Err(format!("not an order: {e}"))),
    }
}
