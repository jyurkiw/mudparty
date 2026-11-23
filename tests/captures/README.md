# Wire captures

Each `<name>.wire.jsonl` here is a character's wire stream (`logs/<character>/wire.jsonl`,
design §9.2) or the wire tail of a snapshot (§9.5). `crates/mp-replay/tests/captures.rs`
replays every one through telnet, text and the parser, and compares the result with
`crates/mp-replay/tests/snapshots/captures__<name>.snap` (insta).

To turn a bug report into a test: copy its `.wire.jsonl` here, fix the bug, run
`cargo test -p mp-replay` with `INSTA_UPDATE=always`, review the new snapshot and commit
both. A wire capture holds no passwords: mp-log writes them redacted.
