//! Bytes in, prompts, events and text out.

use mp_text::{Extractor, Line};

use crate::event::Event;
use crate::patterns::classify;
use crate::statline::{Statline, Vitals};

/// One thing read from the game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// The statline prompt.
    Prompt(Vitals),
    /// A line mudparty knows.
    Event(Event),
    /// Any other line with text: room text, an echoed command, chat.
    Text(String),
}

/// An item with the plain text it was read from, for the transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Traced {
    pub item: Item,
    pub plain: String,
}

/// Reads a session's game bytes (telnet already removed).
///
/// A prompt has no line end, so it is reported as soon as it is whole, and once only: the
/// rest of its line (an echoed command, or a reply glued straight after it) is read
/// separately when that line ends. Login prompts such as `Username:` are reported the same
/// way while they wait.
#[derive(Debug, Clone)]
pub struct Reader {
    lines: Extractor,
    /// The prompt shapes to read, tried in order.
    statlines: Vec<Statline>,
    /// How much of the unfinished line's plain text has been reported already.
    reported: usize,
    /// Texts a written login waits for (#33): a waiting line that ends with one is
    /// reported as text, as a known prompt is.
    waits: Vec<String>,
}

impl Reader {
    /// A reader for a session whose prompt is `statline`, or `None` before it is known.
    pub fn new(statline: Option<Statline>) -> Self {
        Self::with_statlines(statline.into_iter().collect())
    }

    /// A reader that knows several prompt shapes, as a session does before it has set its
    /// own statline (`Statline::silvermere_defaults`).
    pub fn with_statlines(statlines: Vec<Statline>) -> Self {
        Self {
            lines: Extractor::new(),
            statlines,
            reported: 0,
            waits: Vec::new(),
        }
    }

    /// Reports a waiting line once it ends with any of `waits`, as a login written in the
    /// character's file needs (#33); none, as before.
    pub fn set_waits(&mut self, waits: Vec<String>) {
        self.waits = waits
            .into_iter()
            .map(|w| w.trim().to_owned())
            .filter(|w| !w.is_empty())
            .collect();
    }

    /// Reads prompts with `statline` alone from now on, as after a bot sets its own.
    pub fn set_statline(&mut self, statline: Statline) {
        self.statlines = vec![statline];
    }

    /// A prompt at the start of `text`, by the first shape that reads it.
    fn read_prompt(&self, text: &str) -> Option<(Vitals, usize)> {
        self.statlines.iter().find_map(|s| s.read(text))
    }

    /// Feeds `bytes`, returning what they complete, in order.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Item> {
        self.feed_traced(bytes)
            .into_iter()
            .map(|t| t.item)
            .collect()
    }

    /// [`Reader::feed`], with the plain text each item was read from.
    pub fn feed_traced(&mut self, bytes: &[u8]) -> Vec<Traced> {
        let mut out = Vec::new();
        for line in self.lines.feed(bytes) {
            let rest = unreported(&line, self.reported).to_owned();
            self.reported = 0;
            self.read_text(&rest, &mut out);
        }
        if let Some(p) = self.lines.pending() {
            let candidate = unreported(&p, self.reported);
            if let Some(item) = self.whole_prompt(candidate) {
                out.push(Traced {
                    item,
                    plain: candidate.to_owned(),
                });
                self.reported = p.plain.len();
            }
        }
        out
    }

    /// A waiting line that is a whole prompt: the statline, or a prompt-shaped event.
    fn whole_prompt(&self, text: &str) -> Option<Item> {
        if text.trim().is_empty() {
            return None;
        }
        if let Some((v, end)) = self.read_prompt(text)
            && end == text.len()
        {
            return Some(Item::Prompt(v));
        }
        classify(text).map(Item::Event).or_else(|| {
            let end = text.trim_end();
            self.waits
                .iter()
                .any(|w| end.ends_with(w.as_str()))
                .then(|| Item::Text(text.to_owned()))
        })
    }

    /// A finished line's text: a prompt and what follows it, or one line.
    fn read_text(&self, text: &str, out: &mut Vec<Traced>) {
        let mut rest = text;
        if let Some((v, end)) = self.read_prompt(text) {
            out.push(Traced {
                item: Item::Prompt(v),
                plain: text[..end].to_owned(),
            });
            rest = &text[end..];
        }
        if rest.trim().is_empty() {
            return;
        }
        let item = match classify(rest) {
            Some(e) => Item::Event(e),
            None => Item::Text(rest.to_owned()),
        };
        out.push(Traced {
            item,
            plain: rest.to_owned(),
        });
    }
}

/// The part of `line` not yet reported. A backspace can shorten a line under what was
/// reported, in which case the whole line is read again.
fn unreported(line: &Line, reported: usize) -> &str {
    line.plain.get(reported..).unwrap_or(&line.plain)
}
