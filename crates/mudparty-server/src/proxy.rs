//! The MudPlay proxy (design §2, §2.1): MudPlay connects here instead of to silvermere, and
//! each connection is a **seat**.
//!
//! The server ends telnet on both sides. Downstream, the seat offers MudPlay silvermere's
//! own options ([`ServerCodec`]); upstream, it negotiates with silvermere as a bot does.
//! Data bytes pass through unchanged both ways.
//!
//! A seat opens its own link to the game and passes bytes through while MudPlay logs in.
//! The first line MudPlay types answers silvermere's `Username:`, and says whose session
//! this is ([`Server::seat_session`]). From then on the session owns the game link: it taps
//! both directions for the party page and the logs, and the seat only carries bytes between
//! MudPlay and it. MudPlay's password goes through the session, which keeps it out of the
//! logs.
//!
//! Once in the game, a line whose first key is `#` is held at the seat ([`HashLines`]):
//! MudPlay sends keys as they are typed, so the seat echoes them itself, as silvermere would
//! have. On Enter a mudparty command (`#help`, `#who`) is answered here and never reaches
//! the game; any other `#` line goes to the game whole.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::bail;
use mp_engine::{Seat, SeatInput, Upstream};
use mp_telnet::{ClientCodec, ServerCodec, escape};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use crate::Server;

/// Listens for MudPlay on `addr`, which must be a loopback address. Returns the address
/// bound (for port 0, the port picked).
pub async fn serve(server: Arc<Server>, addr: SocketAddr) -> anyhow::Result<SocketAddr> {
    if !addr.ip().is_loopback() {
        bail!("the proxy has no login of its own, so it listens on loopback only, not {addr}");
    }
    let listener = TcpListener::bind(addr).await?;
    let bound = listener.local_addr()?;
    tokio::spawn(async move {
        while let Ok((socket, from)) = listener.accept().await {
            server.record("seat_open", json!({"from": from.to_string()}));
            tokio::spawn(seat(Arc::clone(&server), socket));
        }
    });
    Ok(bound)
}

/// The `#` commands the seat answers itself, and what each does.
const COMMANDS: [(&str, &str); 2] = [
    ("#help", "this list"),
    (
        "#who",
        "the characters mudparty is playing, and who drives each",
    ),
];

/// What the seat does with typed bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Typed {
    /// To the game.
    Pass(Vec<u8>),
    /// Back to MudPlay only.
    Echo(Vec<u8>),
    /// A whole `#` line mudparty knows, Enter typed.
    Command(String),
}

/// Holds `#` lines at the seat (design §2.1).
#[derive(Debug)]
struct HashLines {
    /// The next key starts a line.
    at_line_start: bool,
    /// The `#` line being held.
    held: Option<Vec<u8>>,
    /// A held line ended with CR, so an LF straight after is the same Enter.
    skip_lf: bool,
}

impl HashLines {
    fn new() -> Self {
        Self {
            at_line_start: true,
            held: None,
            skip_lf: false,
        }
    }

    /// Sorts typed bytes. `hold` says whether `#` lines are held now (in the game only, so
    /// a password starting with `#` is never echoed).
    fn feed(&mut self, data: &[u8], hold: bool) -> Vec<Typed> {
        let mut out = Vec::new();
        let push = |out: &mut Vec<Typed>, t: Typed| match (out.last_mut(), t) {
            (Some(Typed::Pass(a)), Typed::Pass(b)) | (Some(Typed::Echo(a)), Typed::Echo(b)) => {
                a.extend(b)
            }
            (_, t) => out.push(t),
        };
        for &b in data {
            if std::mem::take(&mut self.skip_lf) && b == b'\n' {
                continue;
            }
            if let Some(line) = &mut self.held {
                match b {
                    b'\r' | b'\n' => {
                        let line = self.held.take().expect("held");
                        let text = String::from_utf8_lossy(&line).trim().to_owned();
                        self.skip_lf = b == b'\r';
                        self.at_line_start = true;
                        let word = text.split_whitespace().next().unwrap_or_default();
                        if COMMANDS.iter().any(|(c, _)| c.eq_ignore_ascii_case(word)) {
                            push(&mut out, Typed::Command(text));
                        } else {
                            // Take back the seat's echo; the game echoes the line itself.
                            push(&mut out, Typed::Echo(b"\x08 \x08".repeat(line.len())));
                            let mut whole = line;
                            whole.extend_from_slice(b"\r\n");
                            push(&mut out, Typed::Pass(whole));
                        }
                    }
                    8 | 127 => {
                        line.pop();
                        push(&mut out, Typed::Echo(b"\x08 \x08".to_vec()));
                        if line.is_empty() {
                            self.held = None;
                        }
                    }
                    _ => {
                        line.push(b);
                        push(&mut out, Typed::Echo(vec![b]));
                    }
                }
                continue;
            }
            if hold && self.at_line_start && b == b'#' {
                self.held = Some(vec![b]);
                push(&mut out, Typed::Echo(vec![b]));
                continue;
            }
            self.at_line_start = matches!(b, b'\r' | b'\n');
            push(&mut out, Typed::Pass(vec![b]));
        }
        out
    }
}

/// A `#` command's answer, as lines.
fn answer(server: &Server, command: &str) -> Vec<String> {
    let word = command.split_whitespace().next().unwrap_or_default();
    if word.eq_ignore_ascii_case("#who") {
        return vec![server.who()];
    }
    COMMANDS
        .iter()
        .map(|(c, what)| format!("{c}  {what}"))
        .collect()
}

/// Writes `data` to MudPlay, with 0xFF doubled.
async fn to_mudplay(down: &mut TcpStream, data: &[u8]) -> std::io::Result<()> {
    let mut out = Vec::with_capacity(data.len());
    escape(data, &mut out);
    down.write_all(&out).await
}

/// One MudPlay connection, from accept to close.
async fn seat(server: Arc<Server>, mut down: TcpStream) {
    let mut codec = ServerCodec::new();
    if down.write_all(&ServerCodec::OFFERS).await.is_err() {
        return;
    }
    let Ok(mut up) = TcpStream::connect(server.game()).await else {
        let _ = to_mudplay(&mut down, b"\r\n*** mudparty: can't reach the game ***\r\n").await;
        return;
    };
    let mut telnet = ClientCodec::new(80, 24);
    let (mut ubuf, mut dbuf) = ([0u8; 8192], [0u8; 8192]);
    let mut window = None;
    // Until MudPlay has typed its account name, the seat carries the bytes itself.
    let mut line: Vec<u8> = Vec::new();
    let account = 'login: loop {
        tokio::select! {
            read = up.read(&mut ubuf) => {
                let Ok(n @ 1..) = read else { return };
                let (mut data, mut reply) = (Vec::new(), Vec::new());
                telnet.decode(&ubuf[..n], &mut data, &mut reply);
                if up.write_all(&reply).await.is_err() || to_mudplay(&mut down, &data).await.is_err() {
                    return;
                }
            }
            read = down.read(&mut dbuf) => {
                let Ok(n @ 1..) = read else { return };
                let (mut data, mut reply) = (Vec::new(), Vec::new());
                codec.decode(&dbuf[..n], &mut data, &mut reply);
                if down.write_all(&reply).await.is_err() {
                    return;
                }
                let mut naws = Vec::new();
                if codec.window() != window {
                    window = codec.window();
                    if let Some((cols, rows)) = window {
                        telnet.set_window_size(cols, rows, &mut naws);
                    }
                }
                let mut out = naws;
                escape(&data, &mut out);
                if up.write_all(&out).await.is_err() {
                    return;
                }
                for b in data {
                    match b {
                        b'\r' | b'\n' if !line.is_empty() => {
                            break 'login String::from_utf8_lossy(&line).trim().to_owned();
                        }
                        b'\r' | b'\n' => {}
                        8 | 127 => {
                            line.pop();
                        }
                        _ => line.push(b),
                    }
                }
            }
        }
    };

    // The session takes the game link from here.
    let (input, from_seat) = mpsc::unbounded_channel();
    let (output, mut to_seat) = mpsc::unbounded_channel();
    let name = server.seat_session(
        &account,
        Upstream { stream: up, telnet },
        Seat {
            input: from_seat,
            output,
        },
    );
    let mut hash = HashLines::new();
    // The game's last unfinished line, its prompt as a rule: drawn again after a `#` answer.
    let mut tail: Vec<u8> = Vec::new();
    loop {
        tokio::select! {
            data = to_seat.recv() => match data {
                Some(data) => {
                    match data.iter().rposition(|&b| b == b'\n') {
                        Some(i) => tail = data[i + 1..].to_vec(),
                        None => tail.extend_from_slice(&data),
                    }
                    if tail.len() > 4096 {
                        tail.drain(..tail.len() - 4096);
                    }
                    if to_mudplay(&mut down, &data).await.is_err() {
                        break;
                    }
                }
                // The session ended: MudPlay's link closes with it.
                None => break,
            },
            read = down.read(&mut dbuf) => {
                let Ok(n @ 1..) = read else { break };
                let (mut data, mut reply) = (Vec::new(), Vec::new());
                codec.decode(&dbuf[..n], &mut data, &mut reply);
                if down.write_all(&reply).await.is_err() {
                    break;
                }
                if codec.window() != window {
                    window = codec.window();
                    if let Some((cols, rows)) = window {
                        let _ = input.send(SeatInput::Window { cols, rows });
                    }
                }
                let hold = server.seat_in_game(&name);
                let mut gone = false;
                for typed in hash.feed(&data, hold) {
                    let sent = match typed {
                        Typed::Pass(bytes) => {
                            let _ = input.send(SeatInput::Data(bytes));
                            Ok(())
                        }
                        Typed::Echo(bytes) => to_mudplay(&mut down, &bytes).await,
                        Typed::Command(command) => {
                            server.record("seat_command", json!({"character": name, "command": command}));
                            let mut reply = b"\r\n".to_vec();
                            for line in answer(&server, &command) {
                                reply.extend_from_slice(format!("*** mudparty: {line} ***\r\n").as_bytes());
                            }
                            reply.extend_from_slice(&tail);
                            to_mudplay(&mut down, &reply).await
                        }
                    };
                    if sent.is_err() {
                        gone = true;
                        break;
                    }
                }
                if gone {
                    break;
                }
            }
        }
    }
    server.record("seat_closed", json!({"character": name}));
}

#[cfg(test)]
mod tests {
    use super::{HashLines, Typed};

    #[test]
    fn plain_typing_passes_through() {
        let mut h = HashLines::new();
        assert_eq!(
            h.feed(b"look\r\n", true),
            [Typed::Pass(b"look\r\n".to_vec())]
        );
    }

    #[test]
    fn a_known_hash_line_is_echoed_and_kept_from_the_game() {
        let mut h = HashLines::new();
        let mut out = h.feed(b"#w", true);
        out.extend(h.feed(b"ho\r\n", true));
        assert_eq!(
            out,
            [
                Typed::Echo(b"#w".to_vec()),
                Typed::Echo(b"ho".to_vec()),
                Typed::Command("#who".into())
            ]
        );
        // The LF after the CR was the same Enter, and the next line starts fresh.
        assert_eq!(
            h.feed(b"say hi\r", true),
            [Typed::Pass(b"say hi\r".to_vec())]
        );
    }

    #[test]
    fn an_unknown_hash_line_goes_to_the_game_whole() {
        let mut h = HashLines::new();
        assert_eq!(
            h.feed(b"#zz\r\n", true),
            [
                Typed::Echo(b"#zz\x08 \x08\x08 \x08\x08 \x08".to_vec()),
                Typed::Pass(b"#zz\r\n".to_vec())
            ]
        );
    }

    #[test]
    fn backspace_edits_the_held_line_and_can_undo_the_hold() {
        let mut h = HashLines::new();
        let out = h.feed(b"#wx\x08ho\r", true);
        assert_eq!(out.last(), Some(&Typed::Command("#who".into())));
        let out = h.feed(b"#\x08look\r", true);
        assert_eq!(out.last(), Some(&Typed::Pass(b"look\r".to_vec())));
    }

    #[test]
    fn a_hash_mid_line_or_out_of_the_game_is_just_typed() {
        let mut h = HashLines::new();
        assert_eq!(
            h.feed(b"say #1\r", true),
            [Typed::Pass(b"say #1\r".to_vec())]
        );
        assert_eq!(
            h.feed(b"#secret\r", false),
            [Typed::Pass(b"#secret\r".to_vec())]
        );
    }
}
