//! Replays wire captures for snapshot tests (design §10.3).
//!
//! [`read_wire`] reads a `wire.jsonl` stream as mp-log writes it. [`replay`] runs its
//! received bytes through telnet, text and the parser, and lists what was read and sent, one
//! line each, for an insta snapshot. When the engine exists (MVP 4 on) the replay also drives
//! it, and its decisions join the listing.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use mp_parse::{Item, Reader, Rest, Statline};
use mp_telnet::ClientCodec;
use serde_json::Value;

/// Which way a record's bytes went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    /// From the game.
    In,
    /// To the game.
    Out,
}

/// One record of a wire stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireRecord {
    pub t: String,
    pub seq: u64,
    pub dir: Dir,
    /// Who an outgoing write came from (`engine`, `user`, `remote:<name>`, …).
    pub src: Option<String>,
    /// The bytes; `None` for a redacted write.
    pub bytes: Option<Vec<u8>>,
    /// For a redacted write: what it was (`password`) and its length.
    pub redacted: Option<(String, usize)>,
}

/// Reads a `wire.jsonl` stream, skipping header records.
pub fn read_wire(text: &str) -> Result<Vec<WireRecord>, String> {
    let mut out = Vec::new();
    for (i, line) in text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
    {
        let bad = |what: &str| format!("line {}: {what}", i + 1);
        let v: Value = serde_json::from_str(line).map_err(|e| bad(&e.to_string()))?;
        if v["kind"] != "wire" {
            continue;
        }
        let dir = match v["dir"].as_str() {
            Some("in") => Dir::In,
            Some("out") => Dir::Out,
            _ => return Err(bad("no dir")),
        };
        let bytes = match v["b64"].as_str() {
            Some(b) => Some(STANDARD.decode(b).map_err(|e| bad(&e.to_string()))?),
            None => None,
        };
        let redacted = v["redacted"]
            .as_str()
            .map(|what| (what.to_owned(), v["len"].as_u64().unwrap_or(0) as usize));
        if bytes.is_none() && redacted.is_none() {
            return Err(bad("neither bytes nor a redaction"));
        }
        out.push(WireRecord {
            t: v["t"].as_str().unwrap_or_default().to_owned(),
            seq: v["seq"].as_u64().unwrap_or_default(),
            dir,
            src: v["src"].as_str().map(str::to_owned),
            bytes,
            redacted,
        });
    }
    Ok(out)
}

/// The statline template a sent `set statline [full] custom <template>` installs.
fn statline_set(command: &str) -> Option<&str> {
    let rest = command.strip_prefix("set statline ")?;
    let rest = rest.strip_prefix("full ").unwrap_or(rest);
    rest.strip_prefix("custom ")
}

/// Replays `records`, listing what the parser read and what was sent.
pub fn replay(records: &[WireRecord]) -> Vec<String> {
    let mut telnet = ClientCodec::new(80, 24);
    let mut reader = Reader::new(None);
    let mut out = Vec::new();
    for rec in records {
        let src = rec.src.as_deref().unwrap_or("?");
        match (rec.dir, &rec.bytes) {
            (Dir::In, Some(bytes)) => {
                let (mut data, mut reply) = (Vec::new(), Vec::new());
                telnet.decode(bytes, &mut data, &mut reply);
                out.extend(reader.feed(&data).iter().map(describe));
            }
            (Dir::Out, Some(bytes)) => {
                let text = String::from_utf8_lossy(bytes);
                let text = text.trim_end_matches(['\r', '\n']);
                out.push(format!("> {text} [{src}]"));
                if let Some(template) = statline_set(text) {
                    match Statline::compile(template) {
                        Ok(s) => reader.set_statline(s),
                        Err(e) => out.push(format!("! statline not readable: {e}")),
                    }
                }
            }
            (_, None) => {
                let (what, len) = rec.redacted.clone().unwrap_or_default();
                out.push(format!("> [redacted {what}, {len} bytes] [{src}]"));
            }
        }
    }
    out
}

fn describe(item: &Item) -> String {
    match item {
        Item::Prompt(v) => {
            let pair = |a: Option<i32>, b: Option<i32>| match (a, b) {
                (Some(a), Some(b)) => format!("{a}/{b}"),
                (Some(a), None) => a.to_string(),
                _ => "-".into(),
            };
            let rest = match v.rest {
                Rest::Standing => "",
                Rest::Resting => " resting",
                Rest::Meditating => " meditating",
            };
            format!(
                "PROMPT hp={} mp={}{rest}",
                pair(Some(v.hp), v.max_hp),
                pair(v.mp, v.max_mp)
            )
        }
        Item::Event(e) => format!("EVENT {e:?}"),
        Item::Text(t) => format!("TEXT {t}"),
    }
}
