//! A character's four streams and the server log (design §9.2).

use std::fmt;
use std::io;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Map, Value, json};

use crate::clock::{Clock, stamp};
use crate::rolling::{RollConfig, RollingFile};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// What a stream's files open with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub character: String,
    pub session: u64,
    pub config_rev: String,
    pub gamedata_rev: String,
    pub realm: String,
}

/// Who an outgoing write came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The bot's own engine.
    Engine,
    /// The user, at a terminal or in MudPlay.
    User,
    /// Another player's @ command.
    Remote(String),
    /// The web page.
    Web,
    /// Injected by the server into a MudPlay seat.
    Inject,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Engine => f.write_str("engine"),
            Self::User => f.write_str("user"),
            Self::Remote(who) => write!(f, "remote:{who}"),
            Self::Web => f.write_str("web"),
            Self::Inject => f.write_str("inject"),
        }
    }
}

/// `{t, seq, kind, …fields}` as one JSON line.
fn json_line(t: &str, seq: u64, kind: &str, fields: Value) -> Vec<u8> {
    let mut m = Map::new();
    m.insert("t".into(), t.into());
    m.insert("seq".into(), seq.into());
    m.insert("kind".into(), kind.into());
    if let Value::Object(f) = fields {
        for (k, v) in f {
            m.entry(k).or_insert(v);
        }
    }
    let mut line = serde_json::to_vec(&Value::Object(m)).expect("JSON values serialise");
    line.push(b'\n');
    line
}

/// A character's wire capture, transcript, decisions and events, in `<root>/<character>/`.
pub struct CharacterLogs {
    wire: RollingFile,
    transcript: RollingFile,
    decisions: RollingFile,
    events: RollingFile,
    clock: Box<dyn Clock>,
    seq: u64,
}

impl CharacterLogs {
    /// Opens the four streams, each file starting with a header record for `header`.
    pub fn open(
        root: &Path,
        header: Header,
        cfg: RollConfig,
        clock: Box<dyn Clock>,
    ) -> io::Result<Self> {
        let dir = root.join(&header.character);
        let t = stamp(clock.now());
        let mut seq = 0;
        let mut next = || {
            seq += 1;
            seq
        };
        let fields = json!({
            "character": header.character,
            "session": header.session,
            "mudparty": VERSION,
            "config_rev": header.config_rev,
            "gamedata_rev": header.gamedata_rev,
            "realm": header.realm,
        });
        let jsonl = |seq: u64| json_line(&t, seq, "header", fields.clone());
        let wire = RollingFile::open(&dir, "wire", "jsonl", cfg, jsonl(next()))?;
        let decisions = RollingFile::open(&dir, "decisions", "jsonl", cfg, jsonl(next()))?;
        let events = RollingFile::open(&dir, "events", "jsonl", cfg, jsonl(next()))?;
        let text_header = format!(
            "{t}  # mudparty {VERSION} session {} {} config {} gamedata {} realm {}\n",
            header.session, header.character, header.config_rev, header.gamedata_rev, header.realm
        );
        let transcript =
            RollingFile::open(&dir, "transcript", "log", cfg, text_header.into_bytes())?;
        Ok(Self {
            wire,
            transcript,
            decisions,
            events,
            clock,
            seq,
        })
    }

    fn next(&mut self) -> (String, u64) {
        self.seq += 1;
        (stamp(self.clock.now()), self.seq)
    }

    /// Bytes read from the game, raw.
    pub fn wire_in(&mut self, bytes: &[u8]) -> io::Result<()> {
        let (t, seq) = self.next();
        let rec = json_line(
            &t,
            seq,
            "wire",
            json!({"dir": "in", "b64": STANDARD.encode(bytes)}),
        );
        self.wire.write_record(&rec)
    }

    /// Bytes written to the game, raw.
    pub fn wire_out(&mut self, bytes: &[u8], src: &Source) -> io::Result<()> {
        let (t, seq) = self.next();
        let fields = json!({"dir": "out", "src": src.to_string(), "b64": STANDARD.encode(bytes)});
        self.wire.write_record(&json_line(&t, seq, "wire", fields))
    }

    /// A write to the game that must not be kept (`what`: `password`): its length only.
    pub fn wire_out_redacted(&mut self, bytes: &[u8], what: &str, src: &Source) -> io::Result<()> {
        let (t, seq) = self.next();
        let fields =
            json!({"dir": "out", "src": src.to_string(), "redacted": what, "len": bytes.len()});
        self.wire.write_record(&json_line(&t, seq, "wire", fields))
    }

    /// A game line, as plain text.
    pub fn transcript_line(&mut self, plain: &str) -> io::Result<()> {
        let (t, _) = self.next();
        self.transcript
            .write_record(format!("{t}  {plain}\n").as_bytes())
    }

    /// A command sent to the game.
    pub fn transcript_sent(&mut self, command: &str, src: &Source) -> io::Result<()> {
        let (t, _) = self.next();
        self.transcript
            .write_record(format!("{t}  > {command}  [{src}]\n").as_bytes())
    }

    /// A command that must not be kept, shown as `********`.
    pub fn transcript_sent_redacted(&mut self, src: &Source) -> io::Result<()> {
        self.transcript_sent("********", src)
    }

    /// Why the engine did something: `kind` and its inputs and result.
    pub fn decision(&mut self, kind: &str, fields: Value) -> io::Result<()> {
        let (t, seq) = self.next();
        self.decisions
            .write_record(&json_line(&t, seq, kind, fields))
    }

    /// A session event: connect, login, kill, death, level, …
    pub fn event(&mut self, kind: &str, fields: Value) -> io::Result<()> {
        let (t, seq) = self.next();
        self.events.write_record(&json_line(&t, seq, kind, fields))
    }

    /// Flushes every stream and waits for any zipping.
    pub fn close(self) -> io::Result<()> {
        self.wire.close()?;
        self.transcript.close()?;
        self.decisions.close()?;
        self.events.close()
    }
}

/// The server's own stream, `<root>/server.jsonl`.
pub struct ServerLog {
    file: RollingFile,
    clock: Box<dyn Clock>,
    seq: u64,
}

impl ServerLog {
    pub fn open(root: &Path, cfg: RollConfig, clock: Box<dyn Clock>) -> io::Result<Self> {
        let header = json_line(
            &stamp(clock.now()),
            1,
            "header",
            json!({"mudparty": VERSION}),
        );
        let file = RollingFile::open(root, "server", "jsonl", cfg, header)?;
        Ok(Self {
            file,
            clock,
            seq: 1,
        })
    }

    /// One record of `kind` with `fields`.
    pub fn record(&mut self, kind: &str, fields: Value) -> io::Result<()> {
        self.seq += 1;
        let rec = json_line(&stamp(self.clock.now()), self.seq, kind, fields);
        self.file.write_record(&rec)
    }

    pub fn close(self) -> io::Result<()> {
        self.file.close()
    }
}
