//! Turning game bytes (telnet already removed) into lines for the parser.

use mp_text::{Extractor, LineEnd, cp437_char};

fn feed_all(chunks: &[&[u8]]) -> (Vec<(String, LineEnd)>, Extractor) {
    let mut x = Extractor::new();
    let mut out = Vec::new();
    for c in chunks {
        for line in x.feed(c) {
            out.push((line.plain, line.end));
        }
    }
    (out, x)
}

fn plain(lines: &[(String, LineEnd)]) -> Vec<&str> {
    lines.iter().map(|(p, _)| p.as_str()).collect()
}

#[test]
fn crlf_ends_a_line() {
    let (lines, _) = feed_all(&[b"hello\r\nworld\r\n"]);
    assert_eq!(plain(&lines), ["hello", "world"]);
    assert!(lines.iter().all(|(_, e)| *e == LineEnd::Newline));
}

#[test]
fn cr_and_lf_split_across_reads_end_one_line() {
    let (lines, _) = feed_all(&[b"hello\r", b"\nworld\r\n"]);
    assert_eq!(plain(&lines), ["hello", "world"]);
}

#[test]
fn a_lone_lf_or_cr_ends_a_line() {
    let (lines, _) = feed_all(&[b"one\ntwo\rthree\r\n"]);
    assert_eq!(plain(&lines), ["one", "two", "three"]);
}

#[test]
fn empty_lines_are_kept() {
    let (lines, _) = feed_all(&[b"\r\n\r\nx\r\n"]);
    assert_eq!(plain(&lines), ["", "", "x"]);
}

#[test]
fn ansi_is_stripped_from_plain_and_kept_in_raw() {
    let mut x = Extractor::new();
    let lines = x.feed(b"\x1b[0;32mObvious exits: closed door north, up\r\n");
    assert_eq!(lines[0].plain, "Obvious exits: closed door north, up");
    assert_eq!(
        lines[0].raw,
        b"\x1b[0;32mObvious exits: closed door north, up\r\n"
    );
}

#[test]
fn an_escape_split_across_reads_is_still_stripped() {
    let (lines, _) = feed_all(&[b"\x1b[1", b";36mYou gain 5 experience.\r\n"]);
    assert_eq!(plain(&lines), ["You gain 5 experience."]);
}

// silvermere's direction words carry a filler byte and a backspace (display/exits.rs).
#[test]
fn backspace_folds_out_the_filler_letter() {
    let (lines, _) = feed_all(&[b"Obvious exits: nQ\x08orth, sQ\x08outh\r\n"]);
    assert_eq!(plain(&lines), ["Obvious exits: north, south"]);
}

#[test]
fn backspace_at_the_start_of_a_line_does_nothing() {
    let (lines, _) = feed_all(&[b"\x08ok\r\n"]);
    assert_eq!(plain(&lines), ["ok"]);
}

#[test]
fn cp437_high_bytes_become_their_characters() {
    let (lines, _) = feed_all(&[b"\xc4\xc4 \xb3 \xdb \xe1\r\n"]);
    assert_eq!(plain(&lines), ["── │ █ ß"]);
    assert_eq!(cp437_char(0xFA), '·');
    assert_eq!(cp437_char(b'A'), 'A');
}

#[test]
fn bell_and_nul_are_dropped_from_plain() {
    let (lines, _) = feed_all(&[b"a\x07b\x00c\r\n"]);
    assert_eq!(plain(&lines), ["abc"]);
}

#[test]
fn a_prompt_with_no_line_end_is_pending() {
    let (lines, x) = feed_all(&[b"\x1b[0;37m[HP=35/MA=12]:"]);
    assert!(lines.is_empty());
    let p = x.pending().expect("pending");
    assert_eq!(p.plain, "[HP=35/MA=12]:");
    assert_eq!(p.end, LineEnd::Pending);
}

// silvermere's lines open with `ESC[79D ESC[K` (ANSI_CLRLINE): back to column 0, erase.
// Arriving after an unanswered prompt, that ends the prompt as a line of its own.
#[test]
fn a_clear_line_prefix_after_a_prompt_ends_the_prompt() {
    let mut x = Extractor::new();
    assert!(x.feed(b"[HP=35/MA=12]:").is_empty());
    let lines = x.feed(b"\x1b[79D\x1b[K\x1b[0;36mYou gain 5 experience.\r\n");
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].plain, "[HP=35/MA=12]:");
    assert_eq!(lines[0].end, LineEnd::Overwritten);
    assert_eq!(lines[1].plain, "You gain 5 experience.");
    assert!(lines[1].raw.starts_with(b"\x1b[79D\x1b[K"));
    assert!(x.pending().is_none());
}

#[test]
fn a_clear_line_prefix_on_an_empty_line_ends_nothing() {
    let (lines, _) = feed_all(&[b"\x1b[79D\x1b[KHello\r\n"]);
    assert_eq!(plain(&lines), ["Hello"]);
}

#[test]
fn a_short_cursor_left_does_not_split_the_line() {
    let (lines, _) = feed_all(&[b"abcdef\x1b[2Dxy\r\n"]);
    assert_eq!(lines.len(), 1);
}

#[test]
fn text_after_a_prompt_with_no_clear_line_stays_one_line() {
    // The prompt-then-text split for this shape belongs to the prompt matcher (mp-parse).
    let (lines, _) = feed_all(&[b"[HP=32/MA=27]:--- Telepath Sent to Raijin ---\r\n"]);
    assert_eq!(
        plain(&lines),
        ["[HP=32/MA=27]:--- Telepath Sent to Raijin ---"]
    );
}

// silvermere's help boxes (help human, help warrior) line their columns up with
// cursor-forward, `ESC[nC`; on its cleared screen that shows as n spaces.
#[test]
fn a_cursor_forward_shows_as_spaces() {
    let (lines, _) =
        feed_all(&[b"\xba \x1b[0;1mStatistics\x1b[9CAbilities\x1b[7C\x1b[0;34m\xba\r\n"]);
    assert_eq!(plain(&lines), ["║ Statistics         Abilities       ║"]);
}

#[test]
fn a_cursor_forward_with_no_count_is_one_space() {
    let (lines, _) = feed_all(&[b"a\x1b[Cb\x1b[0Cc\x1b[1Cd\r\n"]);
    assert_eq!(plain(&lines), ["a b c d"]);
}

#[test]
fn a_cursor_forward_split_across_reads_is_spaces() {
    let (lines, _) = feed_all(&[b"Charm\x1b[", b"5C40\r\n"]);
    assert_eq!(plain(&lines), ["Charm     40"]);
}
