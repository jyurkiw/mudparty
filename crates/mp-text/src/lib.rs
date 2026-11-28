//! Game bytes into lines: the raw bytes for the screen and the logs, and plain text for the
//! parser.
//!
//! The input is data bytes with telnet already removed. Plain text is what a terminal would
//! show: ANSI removed, backspaces applied (silvermere's direction words carry a filler byte
//! and a backspace), cursor-forward shown as spaces (its help boxes line columns up with
//! it), CP437 decoded. Telling a prompt from text glued after it is the prompt
//! matcher's job (mp-parse), except where silvermere writes over a prompt with its
//! clear-line prefix `ESC[79D ESC[K`, which ends the prompt here.

/// How a line ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnd {
    /// CR, LF or CR LF.
    Newline,
    /// Not ended yet, as a prompt waits for input.
    Pending,
    /// A clear-line sequence started a new line over it (a prompt the game wrote over).
    Overwritten,
}

/// One line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// The bytes as received, ANSI and line end included.
    pub raw: Vec<u8>,
    /// The text as a terminal would show it: no ANSI, backspaces applied, cursor-forward as
    /// spaces, CP437 decoded.
    pub plain: String,
    pub end: LineEnd,
}

/// CP437 0x80..=0xFF.
const CP437_HIGH: &str = "ÇüéâäàåçêëèïîìÄÅ\
                          ÉæÆôöòûùÿÖÜ¢£¥₧ƒ\
                          áíóúñÑªº¿⌐¬½¼¡«»\
                          ░▒▓│┤╡╢╖╕╣║╗╝╜╛┐\
                          └┴┬├─┼╞╟╚╔╩╦╠═╬╧\
                          ╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀\
                          αßΓπΣσµτΦΘΩδ∞φε∩\
                          ≡±≥≤⌠⌡÷≈°∙·√ⁿ²■\u{a0}";

/// The character CP437 byte `b` stands for. Bytes below 0x80 are ASCII.
pub fn cp437_char(b: u8) -> char {
    if b < 0x80 {
        char::from(b)
    } else {
        CP437_HIGH
            .chars()
            .nth(usize::from(b - 0x80))
            .expect("the table has 128 characters")
    }
}

const ESC: u8 = 0x1b;
const BS: u8 = 0x08;
const CR: u8 = b'\r';
const LF: u8 = b'\n';

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Esc {
    #[default]
    None,
    /// After ESC.
    Start,
    /// Inside `ESC [`.
    Csi,
}

/// Splits a byte stream into [`Line`]s. Feed it every data byte in order.
#[derive(Debug, Clone, Default)]
pub struct Extractor {
    raw: Vec<u8>,
    plain: String,
    esc: Esc,
    /// The escape sequence so far; it joins `raw` once complete, so a sequence that starts a
    /// new line lands in that line.
    esc_buf: Vec<u8>,
    /// The last byte ended a line with CR, so an LF straight after belongs to it.
    after_cr: bool,
}

impl Extractor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds `bytes`, returning every line they complete.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Line> {
        let mut out = Vec::new();
        for &b in bytes {
            if std::mem::take(&mut self.after_cr) && b == LF {
                // The CR LF of a line already handed out. Across two reads the LF is
                // simply dropped.
                if let Some(last) = out.last_mut() {
                    let last: &mut Line = last;
                    last.raw.push(LF);
                }
                continue;
            }
            match self.esc {
                Esc::Start => {
                    self.esc_buf.push(b);
                    if b == b'[' {
                        self.esc = Esc::Csi;
                    } else {
                        self.finish_escape(&mut out, None);
                    }
                    continue;
                }
                Esc::Csi => {
                    self.esc_buf.push(b);
                    if (0x40..=0x7e).contains(&b) {
                        self.finish_escape(&mut out, Some(b));
                    }
                    continue;
                }
                Esc::None => {}
            }
            match b {
                ESC => {
                    self.esc = Esc::Start;
                    self.esc_buf.push(b);
                }
                CR | LF => {
                    self.raw.push(b);
                    out.push(self.take(LineEnd::Newline));
                    self.after_cr = b == CR;
                }
                BS => {
                    self.raw.push(b);
                    self.plain.pop();
                }
                b'\t' => {
                    self.raw.push(b);
                    self.plain.push('\t');
                }
                0x00..=0x1f | 0x7f => self.raw.push(b),
                _ => {
                    self.raw.push(b);
                    self.plain.push(cp437_char(b));
                }
            }
        }
        out
    }

    /// The line received so far that hasn't ended, if it has any text.
    pub fn pending(&self) -> Option<Line> {
        (!self.plain.is_empty()).then(|| Line {
            raw: self.raw.clone(),
            plain: self.plain.clone(),
            end: LineEnd::Pending,
        })
    }

    /// An escape sequence is complete. A cursor-left back to column 0 or beyond, with text
    /// on the line, ends that text as an overwritten line. A cursor-forward shows as that
    /// many spaces: silvermere clears the screen before the help boxes that use it, so the
    /// cells it moves over are blank.
    fn finish_escape(&mut self, out: &mut Vec<Line>, csi_final: Option<u8>) {
        self.esc = Esc::None;
        match csi_final {
            Some(b'D') if !self.plain.is_empty() => {
                if self.csi_count() >= self.plain.chars().count() {
                    out.push(self.take(LineEnd::Overwritten));
                }
            }
            Some(b'C') => {
                let n = self.csi_count();
                self.plain.extend(std::iter::repeat_n(' ', n));
            }
            _ => {}
        }
        self.raw.append(&mut self.esc_buf);
    }

    /// The count of the CSI sequence in `esc_buf`: missing or 0 means 1, as a terminal reads
    /// it.
    fn csi_count(&self) -> usize {
        let params = &self.esc_buf[2..self.esc_buf.len() - 1];
        std::str::from_utf8(params)
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(1)
            .max(1)
    }

    fn take(&mut self, end: LineEnd) -> Line {
        Line {
            raw: std::mem::take(&mut self.raw),
            plain: std::mem::take(&mut self.plain),
            end,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_cp437_table_covers_the_high_half() {
        assert_eq!(super::CP437_HIGH.chars().count(), 128);
    }
}
