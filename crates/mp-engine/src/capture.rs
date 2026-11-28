//! Capturing a command's output for the page (#10, #101), as pure logic: `help strength`,
//! `look club`, `look Ermias`. The page asks; the session sends the line and hands back
//! what the game answered.
//!
//! Game behaviour it relies on: silvermere echoes a typed line (the client agrees to its
//! ECHO), so the answer starts after a line that is the command itself, and the next prompt
//! ends it. Lines before the echo (a monster arriving, a fight) aren't the answer. Lines
//! between the echo and the prompt are kept whatever they say, so a fight's lines in the
//! middle of an answer come along with it.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// A command's output, for the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Captured {
    /// The page's own name for the ask, to match the answer to it.
    pub id: String,
    /// The command sent.
    pub line: String,
    /// What came back, a line to each `\n`, as plain text.
    pub text: String,
    /// The game echoed the command; `false` when it never did within the wait.
    pub answered: bool,
}

#[derive(Debug)]
struct Pending {
    id: String,
    line: String,
    /// The echo has come.
    started: bool,
    lines: Vec<String>,
    until: Instant,
}

impl Pending {
    fn finish(self) -> Captured {
        Captured {
            id: self.id,
            line: self.line,
            text: self.lines.join("\n"),
            answered: self.started,
        }
    }
}

/// Captures, in the order they were asked.
#[derive(Debug)]
pub struct Capturer {
    queue: VecDeque<Pending>,
    wait: Duration,
}

impl Capturer {
    /// `wait`: how long an ask may go unanswered, from when it's asked.
    pub fn new(wait: Duration) -> Self {
        Self {
            queue: VecDeque::new(),
            wait,
        }
    }

    /// The page asks for `line`'s output, under its own `id`. The caller sends the line.
    pub fn ask(&mut self, id: &str, line: &str, now: Instant) {
        self.queue.push_back(Pending {
            id: id.to_owned(),
            line: line.trim().to_owned(),
            started: false,
            lines: Vec::new(),
            until: now + self.wait,
        });
    }

    /// The next line the game sent, as plain text; `prompt` for a prompt. An answer, once
    /// it's whole.
    pub fn on_line(&mut self, plain: &str, prompt: bool) -> Option<Captured> {
        let front = self.queue.front_mut()?;
        if !front.started {
            if !prompt && plain.trim().eq_ignore_ascii_case(&front.line) {
                front.started = true;
            }
            return None;
        }
        if prompt {
            return self.queue.pop_front().map(Pending::finish);
        }
        front.lines.push(plain.trim_end().to_owned());
        None
    }

    /// Time passing: asks past their wait end, with what came.
    pub fn on_tick(&mut self, now: Instant) -> Vec<Captured> {
        let mut out = Vec::new();
        while self.queue.front().is_some_and(|p| now >= p.until) {
            if let Some(p) = self.queue.pop_front() {
                out.push(p.finish());
            }
        }
        out
    }
}
