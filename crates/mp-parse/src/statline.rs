//! The prompt matcher, generated from the statline template (design §6.1).
//!
//! The tokens are silvermere's (`silvermere-text/src/statline.rs`, after Stock 1.11p's
//! `_substitute_string`). Matching is on plain text, so the colour tokens, which print only
//! ANSI, match nothing.

use regex::Regex;

/// Resting state from `%r`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rest {
    Standing,
    Resting,
    Meditating,
}

/// What a prompt shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vitals {
    pub hp: i32,
    pub max_hp: Option<i32>,
    pub mp: Option<i32>,
    pub max_mp: Option<i32>,
    pub rest: Rest,
}

/// A template the matcher can't follow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatlineError(pub String);

impl std::fmt::Display for StatlineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StatlineError {}

/// A compiled statline template.
#[derive(Debug, Clone)]
pub struct Statline {
    re: Regex,
}

impl Statline {
    /// Compiles `template`. It must show `%h` and stay on one line (no `%n`).
    pub fn compile(template: &str) -> Result<Self, StatlineError> {
        let mut pat = String::from("^");
        let mut seen_hp = false;
        let mut chars = template.chars().peekable();
        let mut literal = String::new();
        let flush = |lit: &mut String, pat: &mut String| {
            pat.push_str(&regex::escape(lit));
            lit.clear();
        };
        while let Some(c) = chars.next() {
            if c != '%' {
                literal.push(c);
                continue;
            }
            let Some(t) = chars.next() else { break };
            let group = match t {
                'h' => {
                    seen_hp = true;
                    Some(r"(?P<hp>-?\d+)")
                }
                'H' => Some(r"(?P<maxhp>-?\d+)"),
                'm' => Some(r"(?P<mp>-?\d+)"),
                'M' => Some(r"(?P<maxmp>-?\d+)"),
                // `%X` prints a negative number on some servers, or a bare `X` when the
                // race or class record is missing.
                'x' | 'c' => Some(r"\d+"),
                'X' => Some(r"(?:-?\d+|X)"),
                'i' => Some(r"\S+"),
                'w' => Some(r"(?:On|Off)"),
                'r' => Some(r"(?P<rest> \(Resting\) | \(Meditating\) )?"),
                '%' => {
                    literal.push('%');
                    None
                }
                'n' => {
                    return Err(StatlineError(
                        "a statline with %n spans two lines; the prompt matcher reads one".into(),
                    ));
                }
                // `%f0`..`%f7` and `%b0`..`%b7` take a digit.
                'f' | 'b' => {
                    chars.next_if(|d| d.is_ascii_digit());
                    None
                }
                // The other colour tokens, and anything unknown, print no text.
                _ => None,
            };
            if let Some(g) = group {
                flush(&mut literal, &mut pat);
                pat.push_str(g);
            }
        }
        flush(&mut literal, &mut pat);
        if !seen_hp {
            return Err(StatlineError(
                "the statline must show %h for mudparty to read hit points".into(),
            ));
        }
        let re = Regex::new(&pat).map_err(|e| StatlineError(e.to_string()))?;
        Ok(Self { re })
    }

    /// Reads a prompt at the start of `plain`: its vitals and the byte offset where it ends.
    pub fn read(&self, plain: &str) -> Option<(Vitals, usize)> {
        let caps = self.re.captures(plain)?;
        let num = |name: &str| caps.name(name).and_then(|m| m.as_str().parse().ok());
        let rest = match caps.name("rest").map(|m| m.as_str()) {
            Some(" (Resting) ") => Rest::Resting,
            Some(" (Meditating) ") => Rest::Meditating,
            _ => Rest::Standing,
        };
        let v = Vitals {
            hp: num("hp")?,
            max_hp: num("maxhp"),
            mp: num("mp"),
            max_mp: num("maxmp"),
            rest,
        };
        Some((v, caps.get(0).expect("the whole match").end()))
    }
}
