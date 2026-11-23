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

/// Reads a session's game bytes (telnet already removed).
///
/// A prompt has no line end, so it is reported as soon as it is whole, and once only: the
/// rest of its line (an echoed command, or a reply glued straight after it) is read
/// separately when that line ends. Login prompts such as `Username:` are reported the same
/// way while they wait.
#[derive(Debug, Clone)]
pub struct Reader {
    lines: Extractor,
    statline: Option<Statline>,
    /// How much of the unfinished line's plain text has been reported already.
    reported: usize,
}

impl Reader {
    /// A reader for a session whose prompt is `statline`, or `None` before it is known.
    pub fn new(statline: Option<Statline>) -> Self {
        Self {
            lines: Extractor::new(),
            statline,
            reported: 0,
        }
    }

    /// Starts reading prompts with `statline`, as after a bot sets its own.
    pub fn set_statline(&mut self, statline: Statline) {
        self.statline = Some(statline);
    }

    /// Feeds `bytes`, returning what they complete, in order.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Item> {
        let mut out = Vec::new();
        for line in self.lines.feed(bytes) {
            let rest = unreported(&line, self.reported).to_owned();
            self.reported = 0;
            self.read_text(&rest, &mut out);
        }
        if let Some(p) = self.lines.pending() {
            let candidate = unreported(&p, self.reported);
            if let Some(item) = self.whole_prompt(candidate) {
                out.push(item);
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
        if let Some((v, end)) = self.statline.as_ref().and_then(|s| s.read(text))
            && end == text.len()
        {
            return Some(Item::Prompt(v));
        }
        classify(text).map(Item::Event)
    }

    /// A finished line's text: a prompt and what follows it, or one line.
    fn read_text(&self, text: &str, out: &mut Vec<Item>) {
        let mut rest = text;
        if let Some((v, end)) = self.statline.as_ref().and_then(|s| s.read(text)) {
            out.push(Item::Prompt(v));
            rest = &text[end..];
        }
        if rest.trim().is_empty() {
            return;
        }
        out.push(match classify(rest) {
            Some(e) => Item::Event(e),
            None => Item::Text(rest.to_owned()),
        });
    }
}

/// The part of `line` not yet reported. A backspace can shorten a line under what was
/// reported, in which case the whole line is read again.
fn unreported(line: &Line, reported: usize) -> &str {
    line.plain.get(reported..).unwrap_or(&line.plain)
}
