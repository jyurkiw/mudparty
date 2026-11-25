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
    loop {
        tokio::select! {
            data = to_seat.recv() => match data {
                Some(data) => {
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
                if !data.is_empty() {
                    let _ = input.send(SeatInput::Data(data));
                }
            }
        }
    }
    server.record("seat_closed", json!({"character": name}));
}
