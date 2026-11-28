# mudparty — architecture

*Status: draft for review, 2026-11-22. No code yet: nothing is built until the user says so.*

mudparty is a Rust MajorMUD party client and companion server for the user's silvermere
servers. The user plays a main character in MudPlay. mudparty drives the other characters
in the party, and a web page shows the whole party.

References:

- **MudPlay** (`E:/Repos/MudPlay`, C#): its telnet layer, login automator, @-command
  catalogue, combat cascade, the MegaMud `.mp` decoder and `GAME_MECHANICS.md`. That file
  is the record of how the engine behaves and what text it prints, and this design leans
  on it.
- **silvermere** (`E:/Repos/silvermere`, Rust): the server. Its telnet option table, the
  login dialogue, the SQLite world schema, `/room` (PR #13) and the fork's message
  catalogue.

Conventions carried over from both projects:

- **Never invent game mechanics.** Every line the client matches is exact wire text taken
  from silvermere's catalogue or from `GAME_MECHANICS.md`. A line nobody has confirmed is
  listed in *Open questions*, never guessed.
- **MegaMud parity on the wire.** Commands, prompts and @-replies keep stock MajorMUD and
  MegaMud shapes, so MudPlay (and MegaMud) on the other end understand them.

---

## 1. Decisions settled with the user (2026-11-22)

| Topic | Decision |
|---|---|
| Project | New repo `E:/Repos/mudparty` (working name). Design docs in `design/`. |
| Realms | **silvermere only** in step 1: stock 1.11p text plus the fork's extensions (`/room`, fork classes and their combat lines). Other realms can come later, each as a realm profile. |
| Game data | Imported from **silvermere's SQLite world tables** into mudparty's own SQLite file. |
| Pass-through proxy | **Yes.** It runs **inside the server** and can **watch and inject** on the main character's link. |
| Switch character | **`#switch <character>` in MudPlay, or a web button: TODO**, but the proxy is built for it from step 1. Every game link is a server-owned session; a MudPlay connection is a seat attached to one. The character left behind becomes a bot, an offline target is logged in, and a dropped MudPlay leaves its character playing as a bot (*§2.1*). |
| Server dependence | **The server is required.** A client gets its config from the server and does not start without it. |
| Headless bots | **Run inside the server** as tasks, not as separate processes. If the server dies they all go with it. |
| Launching | **By hand.** Interactive clients are started by the user in their own terminals. A headless bot starts when the user presses Start on the web page (or `mudparty-server bot start <name>`), or at server start if its YAML says `autostart: true` (default off). |
| Client UI | **Raw pass-through** in the terminal, with `#` client commands and a one-line status bar. |
| Party comms | **Server first.** Bots share state and orders through the server, and they also answer in-game @ commands. |
| Following | After login a bot **joins the leader's party automatically**, then moves by the game's own party follow. |
| @ commands, step 1 | Status queries, automation toggles, actions (`@do`, `@kill`, `@wait`/`@ok`), party joining (`@invite`/`@join`). |
| Spell tiers | Chosen by MP, **re-picked every round**: a tier can come back mid-fight as MP regenerates. |
| No tier affordable | The **melee command from the YAML**, or `none`. |
| Targeting | **Priority rules**, starting with assist-the-leader. They have to make room for name-based rules such as "casters first" ahead of the leader's target. |
| Party rest | A resting bot sends **`@wait`** to the leader and **`@ok`** when done. |
| Stealth casters | **In step 1.** Avenger and Assassin bots open with a spell backstab and read the fork's two lead-in lines. |
| Web UI | **View plus controls**: toggles and a command box per bot, and for the main character through injection. |
| Characters menu | **In the first web UI**: a sheet per character from `stat`, `spells`, `inventory`, `look <name>` and `look <item>` for each item carried or worn. Refreshed after login, after relevant changes and from a button. Shown as the game's text with a few parsed fields. For the main character it fills from what the user types, and its Refresh button injects (*§6.9*). |
| Passwords | A **separate `secrets.yaml`**, git-ignored, in plain text. |
| Staleness ("dirty") | All three kinds: **config changed**, **game data changed**, **party state too old**. |
| Web login | **None in step 1.** The page is bound to loopback only. |
| Injection while typing | **Queued** until the user's typed line has gone out and its prompt has come back. The page shows the command as queued. |
| Low-HP escape | **Not in step 1.** |
| Logging | **Everything, always**, with no levels: wire capture, transcript, decisions, events and server, rolled at 1 MB, the last 2 files plain and 3 zipped per stream (all settings in `mudparty.toml`). Snapshots from the web page and `#report`. Logs are read from disk; a web log viewer is on the TODO list (*§9*, *§13*). |
| Testing | Unit tests; every parser pattern keyed to silvermere's `catalogue.toml`; replayed captures checked against decision snapshots; and **whole-system tests of the mudparty server against an in-process silvermere** (the `silvermere` package's `serve_in_process`, a git dependency at a pinned commit). Path translation is tested on Winterhawk's `.mp` pack (*§10*). |
| Server config | **`mudparty.toml`** (TOML). Character, behaviour and path files are YAML. |
| Server loss | Headless bots die with the server. An interactive client logs off cleanly once its fight ends (*§4.4*). |

---

## 2. Can the server be a pass-through proxy for MudPlay?

**Yes.** Telnet is a byte stream with option negotiation in-band. A proxy that copies every
byte both ways, IAC sequences included, is invisible to both ends:

- silvermere offers `WILL ECHO`, `WILL SGA`, `WILL BINARY` and `DO NAWS`, and refuses every
  other option (`crates/silvermere-net/src/telnet/options.rs`).
- MudPlay offers TERM-TYPE and GMCP besides. silvermere refuses them, and MudPlay falls
  back cleanly.
- Window-size reports (NAWS) reach silvermere, which keeps its 80×24 default honest.
- That would let the proxy pass negotiation through untouched. The design ends telnet at
  the server on each side instead, with the same option set, so that MudPlay's link can
  later move between characters (*§2.1*).

The proxy gives us three things:

1. **A tap.** A second telnet decoder runs on a copy of the server→client bytes and feeds
   the same parser the bots use. The server then knows the main character's HP, MP, room
   and fight the way a bot knows its own. Client→server bytes are tapped too, so the server
   sees the commands the user types (`a kobold`, `/room`).
2. **Injection.** The server can send commands as the main character, from the web UI or
   for party coordination (inviting a bot that asked to join). See *§6.4*.
3. **The whole party on the web page**, main character included.

Costs, and how the design handles them:

- **A server restart drops MudPlay's link.** The user chose that over a separate proxy
  process. MudPlay's auto-reconnect brings the link back. Restarts should be rare, because
  config changes reload live (*§5*).
- **silvermere sees every proxied connection as coming from 127.0.0.1.** That matters only
  if a per-address limit is ever added. rlogin trusts loopback, but the proxy speaks telnet
  only, so the proxy never reaches rlogin.
- **Injecting into a half-typed line would corrupt it.** MudPlay sends keystrokes as they
  are typed. The proxy injects only when the last client→server byte ended a line (CR or LF)
  and that line's prompt has come back. Otherwise it waits.

### 2.1 Seats and sessions: built for `#switch`

`#switch <character>` in MudPlay (or a button on the web page) is on the TODO list (*§13*).
It moves MudPlay from one character to another without logging out. Step 1 doesn't ship
it, but **step 1's proxy is built in the shape it needs**, so adding it later means a new
command, not a rework. The user settled the behaviour on 2026-11-22.

**Sessions.** Every game link belongs to a server-side **session**, one per character
and the same object a headless bot runs in. A session holds the upstream telnet link,
its parser and state, its engine, its logs, and a screen buffer (the last few hundred raw
lines, for redrawing). It has one **driver** at a time:

- `engine`: a headless bot, with automation per its YAML;
- `seat`: MudPlay is attached and the user drives. The engine keeps parsing and stays
  ready, but its automation is off, as for an interactive client's `#hold`.

**Seats.** A **seat** is one MudPlay connection on the proxy port. It is attached to at
most one session. In step 1 a seat attaches once, when MudPlay logs in, and detaches when
MudPlay leaves. `#switch` will add a re-attach.

So from step 1:

- **The server ends telnet on both sides,** in place of passing negotiation bytes through.
  Upstream, each session negotiates with silvermere as a bot does (agreeing to `ECHO`,
  `SGA`, `BINARY`, sending NAWS). Downstream, the seat offers MudPlay the same set
  silvermere does (`WILL ECHO`, `WILL SGA`, `WILL BINARY`, `DO NAWS`) and refuses the rest,
  as silvermere would. MudPlay sees no difference, but one MudPlay link can then be moved
  between upstream links that were each negotiated on their own. The seat's NAWS goes to
  whichever session it is attached to. Game data bytes still pass through unchanged
  (*§10.4*).
- **MudPlay still logs in by itself.** A new seat gets a new session with no character
  yet. Bytes pass through while MudPlay runs its own logon steps. The account name MudPlay
  types (the line after `Username:`, which is not secret) names the character, through
  each YAML's `login.account`. If none matches, the TOML's `main_character` does. The
  password line is redacted in the logs (*§9.4*), as before.
- **`#` lines are held at the seat.** MudPlay sends each key as it is typed, so a command
  line would otherwise reach the game a letter at a time. When a line's first byte is `#`,
  the seat holds it:
  - it echoes each key back to MudPlay itself, since silvermere would have, and handles
    backspace;
  - on Enter, a known mudparty command is handled and never reaches the game, and any
    other `#` line is sent on whole, as typed.

  Step 1 knows `#help` and `#who` (the characters mudparty holds, and their drivers). The
  same handlers as the interactive client's `#` commands serve them. Replies are written to
  MudPlay as `*** mudparty: … ***` lines, followed by the game's prompt.
- **Statline per driver.** A seated session runs the statline MudPlay expects (the main
  character's `statline` template), and an engine-driven one runs the bot statline.
  Switching driver sets the other one (*§6.1*).

**As built (MVP 9a, `mudparty-server/src/proxy.rs`, `mp-engine` `spawn_seated`):**
- The seat offers `mp_telnet::ServerCodec::OFFERS`, silvermere's own bytes in its order,
  and answers as silvermere's option table does, so MudPlay's option state matches a
  direct connection. A NAWS report goes upstream once the session has agreed NAWS.
- Until MudPlay's first typed line (its answer to `Username:`), the seat holds the game
  link itself and carries the bytes. That line names the session: the character whose
  `login.account` matches it, case-blind, else `main_character`, else a session named as
  typed with no file. The seat then hands the link (stream and telnet state) to a new
  session and only carries bytes between MudPlay and it.
- A seated session watches the login instead of driving it, and is in the game at its
  first prompt. Its engines read everything (vitals, room, roster) but decide nothing, it
  doesn't set the statline (a config edit doesn't either) or ask `/room`, and it answers
  no @ commands. The page shows its card with `driver: seat` and refuses its orders.
- What MudPlay types is logged as it arrives (`src: user` on the wire stream, each line on
  the transcript). After `Password:` the next line is logged by length only.
- When MudPlay leaves, the session drops the link (`SeatLeft`); detaching to a bot is
  MVP 9c. When the game drops the link, the session ends (`SeatLinkLost`) and MudPlay
  reconnects by itself. A bot already playing the character is left for the game's
  takeover question, which MudPlay answers.

**As built (MVP 9b):**
- `#` lines are held by the seat (`proxy.rs` `HashLines`), only once the session is in
  the game, so a password starting with `#` is never echoed. The seat echoes each key and
  handles backspace. On Enter, `#help` and `#who` are answered with
  `*** mudparty: … ***` lines, then the game's last unfinished line (its prompt) is drawn
  again from what the seat last carried. Nothing goes to the game. Any other `#` line has
  the seat's echo taken back with backspaces and goes to the game whole, so the game
  echoes it once.
- Injection (`Command::Inject`, `Server::inject`, the page's `inject` message, or the
  `command` order for a seated character) queues in the session. A line goes only when
  the user has no half-typed line and the last line sent, typed or injected, has had its
  prompt back. Each goes alone and waits for its own prompt. The queue is reported
  (`Report::Queued`) and shown on the card. It is logged as `src: inject`.

**As built (MVP 9c), what the user chose on 2026-11-25:** when MudPlay drops mid-game the
character **stays online, idle**, rather than running as a full bot, because a bot would
set its statline and silvermere would keep it (#34).
- The session reports `Unattended`; its driver is its engine, so the page's orders and @
  commands work, and the file's engines run (an empty file runs none). Its statline is
  left alone, and a config edit doesn't set it either. The injection queue is dropped.
- It asks `/room`, and the server records the room in `mudparty.sqlite` (`positions`:
  character, time, why, map, room, room name, HP) as "MudPlay left". The card shows the
  last position, kept across restarts. Before MudPlay drops, the room is known only if
  the user typed `/room`.
- **Hang-up:** on a prompt with HP below `unattended.hang_up_below` (default 30%; points
  or `none` too), it drops the link (`Offline::HungUp`), and the server records the
  position again with "hung up: HP n/m". Whatever else ends an unattended session is
  recorded the same way. A statline without the maximum makes the percentage relative to
  the most HP seen on the link.
- A session MudPlay started never reconnects, unattended or not: MudPlay logs back in, and
  the game's takeover question ends the old session.

**How `#switch Ermias` will work** (TODO, as the user settled it):

1. Check the target: it must have a character YAML, and must not be an interactive
   client's character or already in another seat.
2. **If Ermias isn't logged in, the server logs it in** with its YAML and secrets. MudPlay
   is told `*** mudparty: logging in Ermias ***`.
3. **The character left behind becomes a bot**: its driver goes to `engine`, running its
   own YAML (combat, rest, follow). Bots that were following it keep following.
4. Ermias's driver goes to `seat`. Its statline switches to MudPlay's template.
5. MudPlay gets `*** mudparty: now playing Ermias ***` and the tail of Ermias's screen
   buffer. The seat then sends a bare Enter upstream, and Ermias's prompt and room arrive
   fresh. Full statline modes redraw the room on an empty line (*GAME_MECHANICS: Wire →
   Statline & prompt shape*).
6. The web page's **Play in MudPlay** button on a character's card does the same, for the
   seat that is open.

**When MudPlay drops** (user's choice): the seated character becomes a bot until MudPlay
is back, rather than hanging up. To hand it back when MudPlay reconnects and logs in as
that character, without a second login hitting silvermere's takeover question, the
proposal is a **seat lobby**: the seat plays the login dialogue to MudPlay itself, with
silvermere's own rendered text (`silvermere-text`'s `render_menu` and login greeting). It
checks the password against `secrets.yaml`, then attaches the seat to the held session.
That needs a `secrets.yaml` entry for the main character. The details are a TODO design
item (*§13*). Until it exists, a drop makes the character a bot, and MudPlay's reconnect
takes it over through the normal login and takeover question (*§6.2*).

**A limit that stays:** MudPlay doesn't know it changed character. Its profile, its own
automation, its remote-command permissions and its room tracking all still belong to the
character it logged in as. While switched, MudPlay's automation should be off, or its
profile switched by hand. Stock MudPlay can't be told, so the switch notice says so. A
MudPlay fork that can be told is on the TODO list (*§13*). The seat already ends telnet
itself, which is what lets it talk GMCP to MudPlay even though silvermere refuses GMCP.

---

## 3. Architecture

```
                         ┌──────────────────────── mudparty-server (one) ─────────────────────────┐
  MudPlay (user) ─telnet─► Proxy listener ──────────telnet───────────────────────────────────────┐ │
     ▲                   │   ├ tap decoder ─► parser ─► party state                              │ │
     └──────telnet───────┤   └ injection queue                                                   │ │
                         │                                                                       │ │
  Browser ───http/ws─────► Web UI (axum: static page + /ws/ui feed)                              │ │
                         │                                                                       │ │
  mudparty-client ─ws────► Client hub (/ws/client) ◄─► Party hub ◄─► Config store (YAML, watch)  │ │
  (interactive,          │                              ▲  ▲         Game data (SQLite)          │ │
   one per character     │   Headless bots (tasks) ◄────┘  └──────── Path translator (.mp ↔ YAML)│ │
   you play by hand)     │     └ one engine + telnet link each ──────telnet──────────────────────┤ │
     │                   └───────────────────────────────────────────────────────────────────────┼─┘
     └──────────────────────────────────telnet──────────────────────► silvermere game server ◄──┘
```

### 3.1 Processes and bots

| What | How many | Job |
|---|---|---|
| `mudparty-server` | one process | Owns config (YAML) and game data (SQLite). Runs the party hub, the MudPlay proxy and the web UI, translates path files, and **hosts the headless bots**. |
| Session | a tokio task inside the server, one per character in the game | Its own telnet link to silvermere: login, parsing, combat, resting, @ commands. Driven by its engine (a **headless bot**, with no terminal, started from the web page, the CLI or `autostart`) or by a MudPlay **seat** (*§2.1*). |
| `mudparty-client` | one process per character you want a terminal for | The same engine as a headless bot, with the raw pass-through terminal on top (*§6.7*). It keeps its own link, so it can't be switched into. |
| MudPlay | one (the user) | A seat on the proxy port, attached to the main character's session. |
| Browser | any | The party page. |

A headless bot and an interactive client run the same `mp-engine` code and speak the same
messages (*§4.1*): the bot over an in-memory channel, the client over a WebSocket. A
character runs in one place at a time. The server refuses a second session for the same
character, so an interactive client can't log in a character that a headless bot is
already playing, or the other way round.

An interactive client needs the server. On start it connects to
`ws://127.0.0.1:<port>/ws/client`, names its character and gets that character's config.
With no server it exits with a message. If the server goes away mid-session, see *§4.4*.

### 3.2 Crates (one cargo workspace)

| Crate | Contents | Used by |
|---|---|---|
| `mp-telnet` | IAC codec, option state, NAWS, 0xFF escaping. A client side (sessions, the interactive client) and a server side (the seat facing MudPlay), offering silvermere's option set. | client, server |
| `mp-text` | Line extraction from the byte stream: ANSI stripping that keeps the raw bytes, backspace folding (stock direction words carry a filler byte and a backspace), the `ESC[79D ESC[K` clear-line prefix, prompt recognition for lines that end without CR/LF. | client, server |
| `mp-parse` | The realm profile: prompt/statline matcher, message catalogue (combat, rest, telepath, party, room display), classified events. silvermere is the only profile in step 1. | client, server |
| `mp-data` | The game-data SQLite schema, read API, the importer from silvermere's DB. | client (read-only), server |
| `mp-config` | YAML schemas (serde), validation, revision hashes. | client, server |
| `mp-paths` | MegaMud `.mp` / `Rooms.md` decoding, encoding and the room hash, plus the YAML path model. | server |
| `mp-protocol` | Client↔server and UI↔server messages (serde JSON), versioned. | client, server |
| `mp-engine` | Automation: login, combat, rest, remote commands, party join. Pure logic over events in and commands out, with an injected clock, so it can be tested on replayed byte streams. Also the **bot runner**: one telnet link plus one engine, talking to a hub through a channel. | client, server (headless bots) |
| `mudparty-client` (bin) | Terminal I/O and the WebSocket around a bot runner. | |
| `mudparty-server` (bin) | axum, proxy, hubs, the headless bots, file watching, the CLI subcommands (`import-gamedata`, `paths import/export`, `bot start/stop`). | |

Stack: tokio, axum (HTTP and WebSocket), rusqlite (bundled SQLite; 0.43 links the same
`libsqlite3-sys` as silvermere's sqlx, so the two build together), notify (file watching),
clap, tracing, regex. YAML through `serde_norway`, a maintained fork of the archived
`serde_yaml` (chosen in MVP 3a).

---

## 4. How the processes communicate

### 4.1 Bot ↔ hub messages

Interactive clients use a WebSocket on loopback (`127.0.0.1`), one per client, carrying
JSON in the form `{ "v": 1, "type": "...", ... }`. Headless bots exchange the same
messages with the hub as Rust values over a tokio channel. In this section "client" means
either one.

Client → server:

| Message | When | Carries |
|---|---|---|
| `hello` | connect | character name, client version, `pid`, interactive or headless |
| `state` | on change, at most ~4 a second | vitals (HP/MP and their maxima), level, room (`map/room` and name), mode (idle / fighting / resting / waiting / logging in / offline), current target and tier, automation toggles, each with an `observed_at` time |
| `event` | as it happens | kill, death, level, login result, telepath in/out, @ command handled, errors |
| `sheet` | a sheet part was captured (*§6.9*) | the part, `captured_at`, the raw output (ANSI kept), parsed fields |
| `ack_config` | after applying a config | the `config_rev` now in use |
| `ack_gamedata` | after reopening game data | the `gamedata_rev` now in use |

Server → client:

| Message | When | Carries |
|---|---|---|
| `welcome` | reply to `hello` | the character's config (parsed YAML), `config_rev`, the game-data file path, `gamedata_rev`, the party roster |
| `config_changed` | the YAML changed and still validates | new config and rev |
| `gamedata_changed` | after a re-import | new rev; the client reopens the file |
| `party` | on change, at most ~4 a second | every member's state, **including the main character from the tap**, each with its stale flags (*§5.3*) |
| `order` | web UI, another bot, or the server | `toggle {name, on}`, `command {text}`, `kill {target}`, `wait`, `ok`, `stop`, `join {leader}` |
| `refuse` | a second client for the same character | reason; the client exits |

### 4.2 Browser ↔ server

- `GET /` serves one static page (HTML and a small script, no build step).
- `/ws/ui` sends the party snapshot, then deltas (the same `party` payload plus each
  client's config and game-data status). The page sends `order` messages for any bot, and
  `inject {text}` for the main character.
- Bound to `127.0.0.1` with no login in step 1 (an open question).
- **As built (MVP 7, `mudparty-server/src/web.rs`):** every frame is JSON text with
  `"v": 1`. The server sends `party` (`members`, every card with the main character first,
  and `server.running`) on connecting and on each change, at most four a second; it is the
  whole party each time, not a delta. The page sends `order` with `name` and `order`:
  `start`, `stop`, `toggle` (`switch`: `combat`, `nuke`, `rest` or `all`; `on`), `command`
  (`text`), `kill` (`target`), `wait`, `ok` or `join` (`leader`). Each order gets a
  `result` (`ok`, `message`), so a refused one is said, not dropped. `inject` is refused
  until the proxy exists. `serve` refuses any address that isn't loopback.

### 4.3 In-game @ commands

Bots answer @ commands sent by telepath or gangpath from players on the character's
`remote.allow` list, mainly the main character, typed in MudPlay. A bot replies on the
channel the command arrived on, as MegaMud does. Bots don't send @ commands to each other
for coordination: that goes through the server, which has no telepath throttle and no
3-second page cap. The exception is the leader: a bot sends `@wait`, `@ok` and `@invite`
to the leader in game, so MudPlay sees them as it would from MegaMud.

### 4.4 When the server stops

- **An orderly stop** (Ctrl-C, `mudparty-server stop`, a service stop): every headless bot
  logs off cleanly in parallel. Each finishes or breaks off its fight, sends `quit` and
  waits for the realm exit sequence (*GAME_MECHANICS: Wire → Realm exit / logoff
  sequence*), up to a time limit. Only then does the server close the proxy and exit.
  Interactive clients are told `shutdown` and do the same.
- **A crash:** the headless bots' sockets close with the process, so silvermere sees a
  lost carrier on each, not a logoff. On the user's servers that costs nothing:
  silvermere's hang-up punishment `game.pun_hup` is at its default, `None`. Turned on, it
  would take HP and drop items unless the bot stood in a safe room. MudPlay's link through
  the proxy drops too, and MudPlay reconnects. When the bots log in again, silvermere's
  takeover question clears any link still hanging (*§6.2*).
- **An interactive client that loses the server:** it starts no new fights, takes no new
  orders and keeps defending (the game's own auto-combat carries on). Once out of combat it
  logs off cleanly and exits. A character played by hand shouldn't be left running
  unwatched.

---

## 5. Staleness ("dirty" information)

Each kind has a revision or a timestamp. The web page shows a badge wherever a client is
behind.

### 5.1 Config changed

- The server is the **only writer** of the YAML files. It watches `characters/` and
  `paths/` for edits made in an editor.
- On a change it parses and validates the file. If the file is valid, the server stores it
  under a new `config_rev` (a content hash) and sends `config_changed`. If it isn't, the
  server keeps the last good version, logs the error and shows it on the page. The client
  carries on with its current config.
- A client applies new config **at a safe point**: out of combat, or between rounds for
  combat-only sections. Then it sends `ack_config`. Until the ack arrives the page marks
  that bot **config pending**.

### 5.2 Game data changed

- The `gamedata.sqlite` file has a `gamedata_rev` (the import's content hash) in its `meta`
  table.
- `mudparty-server import-gamedata` writes a new file next to the old one and renames it
  into place, so a client never reads half an import. The server then sends
  `gamedata_changed`.
- The client reopens the file at a safe point and acks. A client on an old rev is flagged
  **game data stale**. Old revs keep working; nothing is forced mid-fight.

### 5.3 Party state too old

- Every value in `state` carries its `observed_at`. The server adds a `stale` flag per
  value group (vitals, room, target) once its age passes a threshold:
  - vitals: no prompt for `stale.vitals` (default 15 s);
  - room: `stale.room` (default 30 s) since the room was last seen;
  - leader target: `stale.target` (default 6 s, a little over one 5-second combat round).
- The main character's values come from the tap and age the same way.
- **Decisions never use stale values.** A bot never assists a stale leader target, and it
  never reads a stale leader room as "same room".
- A client whose socket is down shows **offline** with its last values greyed out.

---

## 6. The step-1 features

### 6.1 Telnet, text and prompts

- `mp-telnet` speaks as a client: it agrees to silvermere's `ECHO`, `SGA` and `BINARY`,
  sends NAWS (the client's terminal size, 80×24 when headless) and refuses the rest.
- `mp-text` builds lines for the parser and keeps the raw bytes for the screen:
  - it folds backspaces, so `n?\borth` reads as `north` (*GAME_MECHANICS: Wire → Direction
    words on the wire*);
  - it strips ANSI from the parser copy only;
  - it treats the statline as a line even with no CR/LF after it, and splits text glued
    after a prompt (`[HP=32/MA=27]:--- Telepath Sent to Raijin ---`).
- **Bots own their statline.** After login a bot sends
  `set statline full custom [HP=%h/%H,MA=%m/%M]:%r` (under the 60-character cap), so every
  prompt gives current and maximum HP and MP. The prompt matcher is generated from the same
  template, as MudPlay does it. Three unmatched prompts in a row mean the statline is sent
  again.
  - **The statline sticks to the character** (found in MVP 3b): silvermere saves it with
    the character, so a character a bot has played comes back with the bot statline on its
    next login. A session therefore reads its own statline as well as silvermere's default
    prompts (`[HP=n/MA=n]:`, `[HP=n/KAI=n]:`, `[HP=n]:`) while it logs in. For a character
    the user later plays in MudPlay, MudPlay sets its own statline on logon, so this costs
    nothing there.
- **The main character's statline belongs to MudPlay.** The tap reads it with a template
  set in the main character's YAML (default: MudPlay's default shape). The proxy never
  changes the user's statline.
- **Pacing.** Commands go out on a time floor, with the prompt as a gate on top of it:
  at most a few unanswered commands at once, and one more per prompt. A
  `You are typing too quickly - command ignored` line means the last command was lost
  (*GAME_MECHANICS: Wire → Command rate limit*). Telepaths keep a 100 ms floor, and each
  `--- Telepath Sent to <Name> ---` or `--- Telepath Not Sent ---` acknowledgement is
  paired with its send in order. A telepath that wasn't sent is sent again, up to 3 tries.
  - **As built (MVP 5):** a 100 ms floor for every line, at most 3 unanswered, and a shut
    gate opens after 3 s with no prompt, so a lost prompt can't stall the bot. silvermere
    queues typed lines and drops only from the fourteenth waiting, so 3 never loses one.
    Login lines and `quit` skip the pacer. A telepath is `/<name> <text>`. A telepath to
    someone not in the game gets `Cannot find user!` and no acknowledgement, so it is given
    up at once and the acknowledgements stay paired.
  - **Pages (found in MVP 6a, silvermere `comms/allowance.rs`):** a character receives at
    most `maxpages` + 1 telepaths (default 3, so 4) in each 3 s medium cycle. A telepath
    sent again inside the same cycle fails again. So one that wasn't sent waits 3 s
    before its next try, and the lines behind it go on meanwhile. This also caps how fast
    a player can command one bot by telepath.

**As built (#34):** after login a bot sends `pro` and reads the `Statusline:` row
(catalogue `profile_statline`: `On`, `Full`, `Off`, or the custom template). If that is
already the bot's template (first 60 characters), it sends nothing. Otherwise it saves the
row in `mudparty.sqlite` (`statlines`) and sets its own. Its own template is never saved,
so a login after a crash keeps the original from before. On an orderly logoff, before
`quit`, it sends the saved statline back (`set statline on|full|off`, or
`set statline full custom <template>`: PROFILE doesn't tell `custom` from `full custom`,
so a template comes back as `full custom`, a known limit) and forgets it. If PROFILE never
answers, the bot sets its own and saves nothing. A seated or unattended character's
statline is never touched. A character whose statline is `Off` gets no prompt at all from
silvermere, so a bot can't finish logging it in yet.

### 6.2 Auto login

A small state machine. Its steps are data in the character YAML, defaulting to silvermere's
dialogue (`bin/silvermere/tests/login_e2e.rs`, `docs/auth.md`):

| Wait for | Send |
|---|---|
| `Username:` | the account name |
| `Password:` | the password from `secrets.yaml` |
| `Make your selection (X to exit):` | `M` |
| `[MAJORMUD]:` | `E` |
| the statline prompt | done: set the statline, then the party join (*§6.5*) |

- **The account is already logged on** (`docs/auth.md`): after the right password,
  silvermere prints `This account is already logged on.` and
  `Disconnect the other connection and log in here? (Y/N):`. The bot answers `Y`, which
  closes the old connection as a hang-up. That is usually a link left over from a crash,
  since mudparty never runs two sessions for one character. With `login.takeover: false`
  it answers `N`, stops, and reports why. `The other connection did not close in time.
  Please try again in a minute.` means retry after a minute. `Invalid Credentials` and
  `Too many failed logins: …` stop the login without a retry.
- **Credential guard** (from MudPlay): once the account name, and later the password, has
  been sent and the next expected prompt has arrived, a second request for the same thing
  stops the login rather than sending it again.
- Each step has a timeout. A failure is reported to the server with the last screen of
  text.
- **Written steps** (`login.steps`, #33; `mp-engine` `login.rs`): a list of
  `{wait, send}` in place of `default-silvermere`. Each step waits, in turn, for its text
  in a finished line or in the line waiting for an answer, and sends its line with
  `{account}` and `{password}` filled in. A line with the password goes as a secret, kept
  out of the logs. The reader reports a waiting line once it ends with one of the login's
  texts (`Reader::set_waits`), as it does a known prompt, so no text is read twice; the
  waits are cleared once in the game. The game's refusals, the credential guard (a step
  that sent the name or the password matched again) and the step timeout (naming the text
  waited for) hold as for silvermere's dialogue. The bot is in the game at the first
  prompt after the last step. A takeover question isn't answered unless a step answers it.
- On a dropped link the client reconnects with backoff (5 s, 10 s, 30 s, then every 60 s),
  unless the drop followed an orderly logoff.
- Interactive mode can turn auto-login off for a session (`--no-login`).

### 6.3 @ commands

- **Arrival:** `<sender> telepaths: <msg>` and `<sender> gangpaths: <msg>`, each after
  the clear-line prefix and a colour change between name and text (silvermere catalogue).
  A message starting with `@` from a sender on `remote.allow` is a command. Others are
  ignored, and with `remote.warn_denied: true` they get the MegaMud failure reply.
- **Replies** go back on the same channel: a telepath as `/<sender> <reply>`, a gangpath
  as `bg <reply>` (`BG`, `GB` and `BROADGANG` all work). A gangpath goes to every gang
  member, the speaker included, so a bot ignores its own `gangpaths:` echo.
- **Reply shapes are MudPlay's** (user, 2026-11-22), copied from its handlers
  (`Game/Remote/*Handler.cs`), because MudPlay parses some of them back. `@where` must read
  `{<room name> (map <m>, room <r>); exits: <list>}` (`WhereReplyParser`), and `@health`
  reads `HP=<hp>/<max>…`. Each reply is tested against the regex MudPlay reads it with,
  copied into the tests as a fixture.

| Group | Commands | What a bot does |
|---|---|---|
| Status queries | `@health`, `@status`, `@where`, `@exp`, `@version`, `@help` | Replies from its state. `@where` uses `/room` for `map/room` and the game-data name. |
| Automation toggles | `@auto-all`, `@auto-combat`, `@auto-nuke`, `@auto-rest` `[on\|off]`, `@stop` | Flips the toggle, tells the server, replies with the new state. `@stop` halts movement and automation actions without logging off. |
| Actions | `@do <cmd>`, `@kill <target>`, `@wait`, `@ok` | `@do` sends the command (with MudPlay's hard blocks: anything containing `suicide` or `reroll` is refused, silently). `@kill` makes the target the top-priority rule until it dies. `@wait`/`@ok` pause and resume the bot's own fight-picking and movement. |
| Party joining | `@invite`, `@join` | `@join <leader>` (or bare, meaning the sender): the bot joins that party (*§6.5*). `@invite` sent *to* a bot: the bot invites the sender. |

The same handlers serve `order` messages from the server and `#` commands typed at an
interactive client, so there is one code path for every source.

**As built (MVP 6a, `mp-engine/src/remote.rs`):**
- The command is the first word, case-blind, less one trailing `!`. An `@word` that isn't
  a command is chat and gets nothing, whoever sent it.
- A refusal (the generic `command invalid or not allowed`, or a handler's own reason) is
  sent only with `warn_denied`. Hard blocks stay silent whatever the setting: anything
  with `reroll` in it (part of a word too), and `@do` with `suicide`. `@help` is a query
  and is not blocked.
- Replies are `{…}` in plain ASCII, spelled as MudPlay's `WireText.Ascii` does. `@help`'s
  list comes in parts of at most 200 characters.
- The replies built, each MudPlay's text:
  - `@health`: `HP=43/43,MA=15/34` and `, Resting` or `, Meditating`, with no MA part for
    a class without mana. Before the first prompt it refuses with
    `HP unknown - no prompt observed yet`.
  - `@version`: `mudparty <version>`.
  - `@auto-…`: `@auto-rest: on`. A bare toggle flips it, and anything but `on`/`off` gets
    `?`.
  - `@do`: `ok`.
  - `@stop`: `movement paused`, or `already @stopped`.
  - `@rego`: `resuming`, or `nothing to resume`. **`@rego` is added** to lift `@stop`,
    as in MudPlay.
- `@kill`, `@wait`, `@ok`, `@invite` and `@join` reply nothing when they work.
  - `@invite` and `@join` are refused with `I'm following <leader>; denied.`. A `@join`
    from the leader already followed is taken all the same.
  - `@join` starts the §6.5 join with that leader.
- The switches stay set across reconnects:
  - `@auto-combat off`, `@auto-all off` and `@stop` keep the combat engine reading the
    room, but it decides nothing.
  - `@auto-nuke off` makes it melee.
  - A `@wait` hold picks no new fight. The fight going on goes on, and so does a kill
    order.
  - Rest follows `@auto-rest`, `@auto-all` and `@stop` the same way.
  - Combat and rest are built from the YAML even when `automation` starts them off, so a
    toggle can turn them on.
  - Every change reaches the server as `Report::Automation`.
- **As built (MVP 6b):** `@where`, `@status` and `@exp` ask the game before answering.
  One question serves every query waiting on it. After 5 s with no answer the query
  gets what is known.
  - `@where` and `@status` send silvermere's `/room`, a fork command answering `1/206`.
    The room name and exits come from the game data, through the server's `RoomBook`.
    Only exits that show as obvious are listed (plain and doors), in direction order, so
    no hidden exit leaks.
    - `@where`: `Arena Practice Room (map 1, room 206); exits: north, up`, read back by
      MudPlay's `WhereReplyParser`. A room not in the game data reads
      `map 1, room 9999 (not in map data)`. With no answer: the refusal
      `Location unknown`.
    - `@status`: `<idle|fighting|resting|meditating>; <room>; no ailments` (or
      `ailments: poisoned`). Poison is read from the rest engine's signs of it. With no
      answer the room is `location unknown`. There is no walking to report yet.
  - `@exp` sends `exp`. `Made:` is the session's own total from the `You gain N
    experience.` lines, and the rate is that over the time since the bot came in. The
    reply is MudPlay's progress line, with its number, rate and time formats:
    `Made: 1,500  Needed: 1,304 (L2)  Rate: 652/hr  Will level in: 2h 0m`.
  - There is no room tracking between queries yet: `/room` is asked each time.

### 6.4 Combat

The engine is evaluated on every prompt and on every combat event. It sends a command
only when it would change what is happening.

**Targeting.** A rule list, evaluated top-down. The first rule that names a monster present
in the room wins:

```yaml
combat:
  targeting:
    - kill_order: true                         # an @kill / web "kill" target, until it dies
    - priority: ["*shaman*", "*mage*", "acolyte"]  # names, case-blind globs, in order
    - assist_leader: true                      # what the leader is attacking
    - fight_back: true                         # whatever is attacking this bot
```

- **The room's monsters** come from the room display (`Also here:`) and from departure
  lines (`<monster> just left to the north.`, or upwards, downwards): a target that leaves
  is let go, and the next is picked.
- **Arrivals** (#105) print each monster's own words from the game data (`A angry giant rat
  creeps into the room from nowhere.`, silvermere's `monster_arrival_text`), or `<monster>
  moves into the room from the east.` The engine reads any line with `into the room` in it:
  after an `A`, `An` or `The`, the longest run of words that is, or ends with, a monster's
  name is the monster (`angry giant rat` is a `giant rat`). What follows `into the room`
  (nowhere, a direction, above, below) doesn't matter. A player walking in names no monster.
- **The leader's attack** (`<Leader> moves to attack <target>.`, or `moves to cast … upon`)
  puts its target on the roster too, if it's a monster's name: the leader is in the room
  and attacking it, though the bot may not have seen it come in. Before #105 a party bot,
  which never looks round, assisted only against the monsters it saw as it arrived.
- **Kills:** a kill is the experience line
  (`You gain <n> experience.`), or, past the experience a character may hold untrained,
  `You have progressed too far without training to acquire anymore experience!` in its
  place.
- **`fight_back`** answers `<monster> moves to attack you!`. Most attacks print the
  monster's own attack words, which aren't read yet (#85), so it rarely fires; a bot that
  must fight what comes names the monsters in `priority`. `priority: ["*"]` takes in
  shopkeepers too (#89).
- **Seen, not yet explained** (#87): the attack sent a second time just after
  `*Combat Engaged*`, which silvermere answers `*Combat Off*`, a few times before a fight
  settles.
- **The leader's target** comes from the bot's own stream, which is the authoritative
  source when they share a room: `<Leader> moves to attack <target>.` or
  `<Leader> moves to cast <spell> upon <target>.`. A room spell reads
  `<Leader> moves to attack everyone in the room.` (*GAME_MECHANICS: Combat → Attack
  announce lines and round commits*). The tap is the fallback: the command the user typed,
  then `*Combat Engaged*`. Leader targets go stale after `stale.target` (*§5.3*).
- **Shared monster names:** the game attacks the first monster of that name. The bot sends
  the name as the game abbreviates it and doesn't try to choose between two of the same
  monster.
- Later rule types fit the same list without changing the step-1 ones. Examples: `hp_below`
  (finish the weakest), `count_at_least` (room spells), `avoid` (never attack).

**Spell tiers, re-picked every round.**

```yaml
combat:
  tiers:                       # top to bottom: the first one that fits is used
    - spell: lbol              # the short name the game accepts
      min_mp: 60               # cast only while MP >= this
    - spell: mmis
      min_mp: 5
  room_tiers:                  # optional; used when enough monsters are present
    - spell: fbal
      min_mp: 40
      min_enemies: 3
  melee: a                     # the attack command when no tier fits; "none" for casters
```

- Every round the engine takes the first tier whose `min_mp` the current MP meets, the
  spell's cost is covered, and the spell is known to the character (game data: the spell's
  class and level against the bot's). As MP falls the choice drops down the list, and it
  climbs back when MP regenerates (the user's choice, unlike MudPlay's per-target latch).
- A change of tier means announcing the new spell at the same target. Re-announcing a
  single-target spell is harmless (*GAME_MECHANICS: Combat → Spell attacks auto-repeat*).
  The engine never announces two different attack spells in one round.
- **A kill always wins over a tier switch**: when the target dies, the bot picks a new
  target and never casts at the corpse.
- `Your spell has no effect on <monster>.` marks that spell as useless against that
  monster's name for the session. The bot skips to the next tier, then to melee.
  `Your weapon has no effect against this monster!` marks melee likewise.
- Room tiers follow their own settings and are never cast at a lone monster just because
  nothing else can hurt it (MudPlay's client policy, recorded in *GAME_MECHANICS*).
- **Engaged** is `*Combat Engaged*`; **off** is `*Combat Off*`. A monster death line ends
  the target.

**Stealth casters (Avenger, Assassin).** From silvermere's catalogue
(`crates/silvermere-text/messages/catalogue.toml`) and `design/stealth-casters.md`:

```yaml
combat:
  stealth_opener:
    spell: mmis        # a single-target attack spell
    require: sneaking  # sneaking | hidden | either
```

- **A bot can only believe it's stealthed.** `sneak` prints `Attempting to sneak...`, and
  `hide` prints `Attempting to hide...`. A success and an unnoticed failure end that line
  the same way, with nothing more. Only a noticed failure says so:
  ` You don't think you are hidden.` or `You don't think you're sneaking.`. Other lines
  that bear on it: `You may not sneak right now!`, `You must wait before you may do that!`,
  `Sneaking...` when moving while sneaking, and `You are no longer sneaking.` when it
  ends. Inviting someone unhides the inviter.
- When the bot believes it is sneaking or hidden, has picked a target that isn't already
  fighting it, and the opener's `min_mp` and cost are met, it opens with the spell instead
  of a tier. This is the spell backstab: one attempt. The lead-in line that follows says
  whether the stealth was real.
- What it reads, after the cyan clear-line prefix `ESC[79D ESC[K`:
  - `You strike from the shadows with <spell>!`: stealth kept; the stock cast lines follow;
  - `Your foe spots you as you begin to cast!`: stealth lost; a normal cast follows.
- Either way the bot is revealed and the fight is on, so normal tier logic takes over the
  next round. This extra lead-in line ahead of the stock cast lines is what MegaMud's
  scripts don't expect, and it is a first-class event here.
- `VANISH` (Avenger) is not in step 1.

### 6.5 Party join and following

The wire text comes from silvermere's catalogue
(`crates/silvermere-text/messages/catalogue.toml`; every line below starts with the
clear-line prefix and a colour):

1. After login the bot sends `set follow normal` (`You will see your normal room
   descriptions when following.`), so it gets full room displays while it is dragged
   along. Then it checks `par`: `You are following <leader>.` under
   `The following people are in your travel party:` means it is done.
2. Otherwise it telepaths `@invite` to `party.leader` (MudPlay invites a member it permits).
   If `party.invite_by_injection: true` and the leader is the proxied main character, the
   server injects `invite <bot>` instead. The leader sees
   `You have invited <bot> to follow you.`, though a full party prints that line too.
3. On `<leader> has invited you to follow <whom>.` (`whom` is the leader's object
   pronoun), the bot sends `follow <leader>`. On silvermere `join <name>` falls through to
   `follow`, and `join <number>` joins a broadcast channel instead. On
   `You are now following <leader>` (no full stop) it is in, and the leader sees
   `<bot> started to follow you.` The refusals:
   - `You must be invited first!` asks again;
   - `You don't see that anywhere!` (follow) or `You don't see <name> here!` (invite) means
     **they aren't in the same room**. Inviting and following both need it, so a bot
     reports `not with leader` and waits.
   - `You cannot invite <name>.`: the invitee has the inviter on their forget list.
4. `par` shows the party. `You are not in a party at the present time.` is followed by the
   player's own row. Each row is
   `  <first last, 30 wide> (<class>, 12 wide)[ M:<mp>%] [H:<hp>%]<P|space><R|M|space> - <Front|Back|Mid>rank`,
   or `  <name> <class> [Invited]`. The mana part is nine spaces when the MP percentage is
   0. Because every member's HP% and MP% are in it, a bot's periodic `par` is a fallback
   source for party vitals.
   - **The order (found in MVP 5, silvermere `display_party`):** `You are following
     <leader>.` comes first, then the player's own followers (or the "not in a party"
     line), and the player's own row is always last. So a member that follows prints both
     `You are following Raijin.` and `You are not in a party at the present time.`, and the
     own row is what ends the answer.
   - An `@invite` that goes unanswered, or a bot left away from its leader, checks `par`
     again after 30 s.
   **As built (#48):** the session asks its `Inviter` before it telepaths `@invite`. The
   server's says yes when the leader has a seated session in the game, and injects
   `invite <bot>` there (queued behind the user's half-typed line like any injection).
   Otherwise the bot telepaths. The rest of the join is unchanged: the bot still sees the
   invitation and sends `follow <leader>`.
5. From then on the game moves the bot with the leader
   (` -- Following your Party leader <dir> --`). The bot tracks its room from room
   displays and `/room`.
6. Losing the party puts the bot back at step 1 once it's able: the leader leaves, the bot
   drops or trains, or the bot sees `You are no longer following <leader>.` or
   `<bot> has been removed from your followers.`.

### 6.6 Resting

```yaml
rest:
  start_when_any: { hp: 50%, mp: 30% }     # start when either falls below
  stop_when_all:  { hp: 95%, mp: 100% }    # stop when both reach these
  command: rest                            # or meditate where the class has it
  tell_leader: true                        # @wait at the start, @ok at the end
```

- Rest starts only out of combat, not while poisoned (poison prevents resting), and not in
  a room whose spell does damage. Each rule comes from *GAME_MECHANICS: Health, resting &
  recovery*.
- Being attacked, moving or casting ends a rest. Combat takes over, and the bot rests again
  afterwards if it still needs to.
- With `tell_leader`, the bot telepaths `@wait` to the leader when it starts and `@ok` when
  it stops. MudPlay pauses on `@wait` as it would for a MegaMud member. The server also
  shows the bot as resting, and other bots below their stop thresholds top up while they
  wait.
- Values can be percentages or absolute numbers.
- **As built (MVP 5):**
  - A rest runs from falling below a start threshold to meeting every stop threshold. It
    starts only where one can be taken, so the leader is never told to wait through a
    fight. Once it has started, a fight or a move that stands the bot up doesn't end it:
    the leader goes on waiting, and the bot sends `rest` again once it is out of combat,
    at most once a round.
  - Whether the bot is resting is read from the prompt's `%r`.
  - "In combat" means the combat engine has a target, a `*Combat Engaged*` is running, or
    the bot was attacked within the last round.
  - Poison is known from `You are too sick to rest!` and from `You feel ill.` each slow tick.
    The bot waits 30 s after the last sign before trying again.
  - **Not built yet:** the rule for rooms whose spell does damage. It needs the bot to know
    its room, which comes with room tracking.

### 6.7 The terminal client and headless bots

- **Interactive (`mudparty-client`):** raw game output goes to the terminal with its ANSI colours.
  Whole lines typed by the user go to the game. Lines starting with `#` are client
  commands: `#auto on|off`, `#combat`, `#rest`, `#status`, `#hold` (pause automation while
  you play by hand), `#quit` (log off and exit). They go through the same handlers as @
  commands. The bottom row is a status bar (name, HP/MP, mode, target, tier, server link),
  kept out of the game's scroll area by a scroll region. Windows Terminal supports it.
- `mudparty-client <character> [--server ws://127.0.0.1:7171] [--no-login]`. Ctrl-C or
  `#quit` logs off cleanly.
- **Headless bots** have no terminal. Their streams go to `logs/<character>/` (*§9*). The
  web page's card shows the last few events, and the transcript on disk shows the rest.

### 6.8 The web page

- One card per party member, main character first: name, class and level; HP and MP bars;
  room (`map/room` and name); mode and target with the current tier; the automation
  toggles; stale, config and game-data badges; the last few events.
- Controls: Start and Stop (log off) for headless bots, Report (*§9.5*), toggle buttons, a kill box,
  wait/ok, and a command line per bot. On the main character's card the command line
  injects (*§2*), and the card says so. A command typed while the user is mid-line in
  MudPlay shows as **queued** until it goes out.
- A server panel: connected clients and running bots, the last config errors, the
  game-data rev.
- A **Characters** menu, one entry per character (*§6.9*).
- **As built (MVP 7):** a card per character file, from `mudparty-server/src/view.rs`:
  class, HP and MP bars, the room (map, room and its name from the game data), the mode
  (`offline`, `connecting`, `logging_in`, `reconnecting`, `logging_off`, or in the game
  `fighting`, `resting`, `waiting` or `idle`), the target and the attack on it (a cast code
  or `melee`), the switches and who holds the bot, the party state, why it last went
  offline, a config error, and its last 8 events, each line sent shown as `> line`.
  Start/Stop, Wait/Ok, the toggles, the kill box and a command line. A `wait` or `ok` from
  the page holds the bot as `web`. The engine reports the target and attack
  (`Report::Fight`) and the room (`Report::Room`); a bot asks `/room` as it comes in and
  learns its room again from each `@where` or `@status`. Rooms are not tracked between
  those yet, and the level, Report (*§9.5*), the stale and game-data badges, the server
  panel's errors and the Characters menu wait for later steps.

### 6.9 The Characters menu: character sheets

A sheet per character, main character included, opened from the **Characters** menu.

**The parts**, each one a game command whose output is captured whole, from the command
to the next prompt. The wire text is from silvermere's catalogue:

| Part | Command | Starts with | Parsed fields |
|---|---|---|---|
| Status | `st` (STATUS) | `Name: <name>    Lives/CP:  <lives>/<cp>` (`st_name`), then `st_race`, `st_class`, `st_hits`, `st_mana` / `st_kai` / `st_traps`, `st_picklocks`, `st_str`, `st_int`, `st_wil` | name, lives, CP, race, exp, perception, class, level, stealth, HP/max, AC/DR, thievery, mana or kai/max, spellcasting, traps, picklocks, the six stats (with their `mark` for a modified stat) and tracking, martial arts and magic resistance, poisoned, rank, and the `spell cast upon you` list |
| Spells | `spells` | `You have the following <what>:` (`spells_header`), the `Level <cost> Short Spell Name` columns, then one `spell_row` each; `You have no <what>.` when there are none | level, mana, short name and name per spell (`what` is spells, or the kai word for a Mystic) |
| Inventory | `i` | `You are carrying <items>` (`inv_carrying`, wrapped across lines), `You have the following keys: <keys>.`, `Wealth: <copper> <name>`, `Encumbrance: <cur>/<max> - <level> [<pct>%]` | carried items, keys, wealth, encumbrance |
| Appearance | `look <own name>` | `[ <first> <last>…` (`look_player_header`), the description (`look_player`), then `<pronoun> is equipped with:` and one row per item: `<item> (Readied)`, `<item> (Weapon Hand)` or `<item> (<slot>)` | description, and each equipped item with its slot |
| Items | `look <item>` for each item worn or carried | `look_item` lines, the item's description | the description per item, kept under the item's name |

- **The item list** for the Items part comes from the Inventory and Appearance parts, so it
  runs after those two. Duplicates are looked at once. The name sent is the one the game
  printed, and whether `look` takes it as printed, or needs a shorter word for some items,
  is checked in the tests (*§10.4*), not assumed.
- **Captured whenever it appears.** The parser knows each part by its first line, so a
  sheet part is filled whenever its output arrives, whoever sent the command: a bot's
  refresh, an `@do st`, or the user typing `st` in MudPlay (seen through the tap).
- **Refresh:**
  - **all parts after login** and from the sheet's **Refresh** button;
  - **one part after a change**: Status after a level, a train or a stat change, Spells
    after a spell is learned, Inventory and Items after a get, drop, buy, sell or give,
    and Appearance and Items after an equip, wield or remove;
  - **never in combat.** A refresh that falls due during a fight waits until the bot is
    idle. Refresh commands go out through the normal pacing (*§6.1*). `look` adds no
    action delay, but the Items part can be a long burst, so it is paced like MudPlay's
    bulk sends: a few unanswered commands at a time.
- **The main character:** the sheet fills only from commands the user types, until they
  press **Refresh**. Refresh injects the commands through the proxy (*§2*), so their
  output also scrolls past on the MudPlay screen. The button says so.
- **Shown** as the game printed it: ANSI colours turned into HTML on the server, with HTML
  escaped. The parsed fields go above it (level, exp, HP/MP max, stats, spell and item
  lists, for sorting and for the party cards). Each part shows its age. A part whose
  output didn't parse shows the text with a "not parsed" badge, and a `parse_miss`
  record goes in the decision log (*§9.2*).
- **Kept** in `mudparty.sqlite` (*§7.6*), so the sheets survive restarts and show as
  "as of <time>" while a character is offline.
- **As built, part 1 (#10):** the Status and Inventory parts.
  - `mp-parse` reads each `st` line (`StName` … `StWillpower`, `StPoisoned`, each stat's
    red star as `marked`) and the inventory's `Encumbrance:` line.
  - `mp-engine` `sheet.rs`'s `SheetReader` builds the parts from whatever output comes,
    whoever asked: a `Status`, and an `Inventory` with the coins by type (taken out of the
    carried list, which names them first), the items with where they're worn, `Wealth:`,
    and the weight line. From those it works out the coins' value, their weight (3 coins
    of a type weigh 1) and how many more fit (*§15.3*).
  - The session sends `st` and `i` once its statline is set after login, and on the
    page's `refresh` order (`Command::RefreshSheet`). Never in a fight or a rest, since
    `i` stands a resting character up; a seated character's sheet fills from what its
    player types.
  - The card keeps the parts (`Card.sheet`) and the page shows them in a modal from each
    card's Sheet button, drawn as the game draws `st` and `i`.
  - **Not yet:** Spells, Appearance and Items; the keys; keeping sheets in
    `mudparty.sqlite` with their age; refreshing after a change (a level, a buy, an equip);
    the main character's Refresh by injection; the parsed-fields-over-text view.

---

### 6.10 Walking (#52)

A bot walks to a room by itself: the first piece of 1.0 (*§12.2*).

- **Routes** come from the game data (`mp-data` `GameData::route`): the shortest walk
  over the exits a walker passes by walking. Those are plain exits, doors and gates,
  map changes, level exits (the game refuses a character outside the range), and the
  types silvermere passes like plain ones (ticket, user count, block guard, delay).
  Key, spell, item, toll, action, hidden, trap, text, remote, class, race, timed and
  alignment exits aren't walked.
- **The walker** (`mp-engine` `walk.rs`) is pure logic with a clock, like the engines.
  1. It asks `/room` to find where it is, plans, and sends one direction at a time.
  2. After each move it waits for the room display's `Obvious exits:` line, then
     confirms the room with `/room`. A wrong room re-plans from there; after three, it
     gives up.
  3. `There is no exit in that direction!` sends it back to finding where it is.
  4. A closed door (`The door is closed!`) gets `open <dir>`. A locked one
     (`The door is locked.`) gets `bash <dir>`, up to 8 tries. In silvermere a bash wins
     if a 0-100 roll is under strength plus the door's bonus, costs two ticks, and one
     try in four does 1-4 damage. The door locks again later. Newhaven's arena door to
     the cave bear is locked.
  5. Any other refusal (`ExitRefused`: level, alignment, timed, a spell or item needed)
     marks that exit refused for the walk, which plans round it (#64, *§14.5*); with no
     way round, it ends the walk with the game's words.
  6. A fight pauses it; once over, it finds where it is and goes on.
  7. A line with no answer for 10 s is asked again, three times. `/room` answering doesn't
     count as progress, only a step confirmed does, so a step the game takes no notice of
     gives up after three tries rather than looping (#78).
  8. With HP at 0 or below silvermere refuses every move (`You may not do that while you
     are mortally wounded!`), and the walk ends saying the bot can't move until it heals
     (#78). That was the old long walk test's stall: with combat off, the cave bear in its
     cavern wounded the bot, and every step out was refused while `/room` kept answering.
- **In the session**, `Command::Goto { to, rules }` starts a walk (in the game, not seated). Its
  progress is `Report::Walk(WalkState)`. The page has a "Go" box (`goto` order, room as
  `1/2141`), and the card shows the walk.
- **Not yet:** tracking the room between walks (a party drag or a flee leaves the last
  known room stale until the next `/room`), and picking locks for thieves. Named places
  came with the routine (*§6.11*).

### 6.11 The routine (#53)

*The YAML described here (`places:`, `routine:` with fixed slots) is replaced by the
shape in §14; the engine pieces below stay.*

A solo bot's loop, the heart of 1.0 (*§12.2*): camp, retreat to rest, return, and timed
hunts, from the character's `routine:` and `places:` (*§7.3*).

- **Goals.** The routine (`mp-engine` `routine.rs`) is pure logic with a clock. It hands
  the session one `Goal` at a time: `Walk(room)`, `Rest`, `Camp`, `Kill(monster)` or
  `Wait`. The bot's engines carry each one out: the walker walks (*§6.10*), the rest engine
  rests (`Rester::begin`, to its stop thresholds), the combat engine fights (a kill order
  for `Kill`, its targeting rules in the camp). The session tells the routine how a walk
  or a rest ended, and shows what the bot is doing (`Seen`: vitals, fighting, the room's
  monsters, the room, walking). In 1.2 the party director (#59) hands out the same goals
  for several characters, so they stay few and plain.
- **The loop.**
  1. Walk to the camp and camp there. A bot found elsewhere, not walking, walks back.
  2. In the camp, a bare Enter every `look_every` (5 s) redraws the room, so the combat
     engine sees monsters that arrive (silvermere's arrival lines are per-monster text).
  3. `rest.start_when_any` met, with no fight going on: walk to `rest_at`, rest, walk back.
     A routine turns the rest engine's own start off, so it never rests in the camp.
     Fleeing mid-fight is #16.
  4. A hunt due, HP at least its `min_hp`, no fight: walk to its room. The monster there
     (its name, or a name ending with it: `big cave bear`) is killed; then back to camp.
     It's due again `every` after the kill, or after a visit that found it gone. A monster
     left unfought for 30 s is given up on. *Hunts became circuits in #67 (§14.7).*
  5. A walk that fails waits 30 s, then starts again from the camp (or the rest).
- **Pausing.** `@stop` (the page's Pause) pauses the routine with the rest of the bot and
  calls its walk off; `@rego` (Go on) picks it up where it was. While it runs, a `goto`
  order is refused.
- **The roster.** A room display lists who else is here before its exits and leaves the
  line out when the room is empty, so the combat engine now empties its roster at
  `Obvious exits:` with no `Also here:` since the last prompt.
- **Only bots.** A seated character, or one whose MudPlay has dropped, runs no routine.
- **Not yet:** an @ command to send the bot to a part. Training came as the `train` rule
  (#56). Shopping came as the `gear` rule (#55), and lair times kept
  across a reconnect with circuits (#67).

## 7. Data model

### 7.1 Files

```
<data root>/                        default: %AppData%\mudparty
  mudparty.toml                     server config (TOML; everything else is YAML)
  secrets.yaml                      passwords, git-ignored
  characters/<name>.yaml            one per bot character, plus the main character's
  paths/<group>/<name>.yaml         paths and loops (stored now, walked in a later step)
  places/<area>.yaml                compiled room file, never edited (§14.4)
  places/<area>.local.yaml          aliases, tags, avoid marks, map pins (§14.2)
  routines/<name>.yaml              routines as rule lists (§14.6)
  gamedata/gamedata.sqlite          imported game data (read-only to clients)
  mudparty.sqlite                   the server's own history
  logs/                             §9
  reports/                          snapshots (§9.5)
```

### 7.2 `mudparty.toml`

The server's main config is TOML, like silvermere's. Character, behaviour and path files
are YAML.

```toml
schema = 1
main_character = "Raijin"              # whose stream the proxy carries
logoff_timeout = "30s"                 # per bot, on an orderly server stop

[game]                                 # the silvermere server
host = "127.0.0.1"
port = 2323                            # where the user's silvermere listens (also 8080, 8088)

[listen]                               # must not clash with silvermere's 2323, 8080, 8088
client_hub = "127.0.0.1:7171"          # /ws/client, /ws/ui and the web page
proxy      = "127.0.0.1:2424"          # MudPlay connects here

[gamedata]
silvermere_db = "E:/silvermere-data/silvermere.db"

[stale]
vitals = "15s"
room   = "30s"
target = "6s"

[logging]                              # §9.3
file_size      = "1MB"                 # roll a plain-text file at this size
plain_files    = 2                     # the live file and the one before it
zip_archives   = 3                     # older files, zipped; the oldest is deleted
```

### 7.3 `characters/<name>.yaml`

```yaml
schema: 1
name: Ermias
class: Assassin                               # checked against game data
autostart: false                              # start as a headless bot when the server starts
login:
  account: ermias                             # password is secrets.yaml → accounts.ermias
  steps: default-silvermere                   # or an explicit list of {wait, send}
  takeover: true                              # answer Y to "already logged on" (§6.2)
statline: "[HP=%h/%H,MA=%m/%M]:%r"            # bots only; the main char's is read, not set
statline_restore: true                        # put the character's own back on logoff (#34)
party:
  leader: Raijin
  invite_by_injection: true
remote:
  allow: [Raijin]
  channels: [telepath, gangpath]
  warn_denied: false
automation: { combat: true, nuke: true, rest: true }
unattended:                                   # a character played in MudPlay, once it drops
  hang_up_below: 30%                          # hang up below this (points, %, or none)
combat: { ... }                               # §6.4
rest: { ... }                                 # §6.6
routine:                                      # routines/newhaven-arena.yaml (§14.6)
  use: newhaven-arena
  with: { rest_below: 60% }                   # the routine's params
```

The main character's file is smaller: `name`, `statline` (the template MudPlay uses, for
the tap to read) and `party.role: leader`.

### 7.4 `secrets.yaml`

```yaml
accounts:
  ermias: "…"
```

The server reads it and passes each client only its own password, in `welcome`. It is never
logged and never shown on the page.

### 7.5 `gamedata.sqlite`

Copied from silvermere's world tables (`migrations/sqlite/0001_schema.sql`), keeping what
the client reads and its numbering, so a `map/room` from `/room` is a direct key:

```sql
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
  -- gamedata_rev, source ('silvermere'), source_path, imported_at, schema

CREATE TABLE rooms (
  map INTEGER, room INTEGER, name TEXT NOT NULL,
  light INTEGER, room_type INTEGER, shopnum INTEGER, flags INTEGER,
  roomspell INTEGER,
  mm_hash TEXT,                 -- MegaMud 8-char hashExits, precomputed (§8)
  PRIMARY KEY (map, room));
CREATE INDEX rooms_mm_hash ON rooms (mm_hash);

CREATE TABLE exits (
  map INTEGER, room INTEGER, dir INTEGER,      -- 0..9 = N S E W NE NW SE SW U D
  to_map INTEGER, to_room INTEGER,
  exit_type INTEGER, par1 INTEGER, par2 INTEGER, par3 INTEGER, par4 INTEGER,
  shows_as TEXT,                -- 'plain' | 'door' | 'hidden' | 'none', from exit_type
  PRIMARY KEY (map, room, dir));
-- shows_as follows what `Obvious exits:` prints (silvermere display/exits.rs):
--   door   Door 7, Key 2, Gate 11: shown as "closed/open door|gate|trap door <dir>",
--          weighted x2 in the MegaMud hash
--   hidden Hidden 6, plus Trap 9 / Timed: shown only once searched, trapped or open;
--          left out of the hash
--   none   Remote Action 12, Text 10: never shown; left out
--   plain  everything else with a target (Normal, Spell, Item, Toll, Action, Map Change,
--          Class, Race, Level, ...): the plain direction word, weighted x1
-- MudPlay counts Toll as door-like; on silvermere a Toll shows a plain word, so it is
-- plain here. The Winterhawk corpus test (§10.5) checks this.
-- Map Change 8 keeps the destination map in par1: to_map = par1, else to_map = map.

CREATE TABLE spells (
  number INTEGER PRIMARY KEY, name TEXT, short TEXT, mana INTEGER, level INTEGER,
  magery INTEGER, targets INTEGER, attack_type INTEGER, ...);
CREATE TABLE classes  (number INTEGER PRIMARY KEY, name TEXT, magery INTEGER, ...);
CREATE TABLE races    (number INTEGER PRIMARY KEY, name TEXT, ...);
CREATE TABLE monsters (number INTEGER PRIMARY KEY, name TEXT, ...);
```

`...` marks columns copied as needed when the engines that read them are built. Step 1
needs rooms, exits, spells and classes. `shows_as` and `mm_hash` are mudparty's own,
derived at import.

### 7.6 `mudparty.sqlite` (server-owned history)

```sql
CREATE TABLE sessions (id INTEGER PRIMARY KEY, character TEXT, started_at TEXT, ended_at TEXT, end_reason TEXT);
CREATE TABLE events   (id INTEGER PRIMARY KEY, session_id INTEGER, at TEXT, kind TEXT, data TEXT); -- JSON
CREATE TABLE vitals   (session_id INTEGER, at TEXT, hp INTEGER, hp_max INTEGER, mp INTEGER, mp_max INTEGER, map INTEGER, room INTEGER);
CREATE TABLE statlines (character TEXT PRIMARY KEY, original TEXT, saved_at TEXT); -- #34: the character's own while a bot's is set
CREATE TABLE positions (id INTEGER PRIMARY KEY, character TEXT, at TEXT, why TEXT, map INTEGER, room INTEGER, room_name TEXT, hp INTEGER, max_hp INTEGER); -- MVP 9c: for rescues
CREATE TABLE sheet_parts (                      -- §6.9, the latest of each part
  character TEXT, part TEXT,                    -- status | spells | inventory | appearance | item:<name>
  captured_at TEXT, raw BLOB,                   -- the output as received, ANSI kept
  parsed TEXT,                                  -- JSON of the parsed fields, NULL if it didn't parse
  PRIMARY KEY (character, part));
```

It holds kills, deaths, levels and logins for the page's history and for later exp/hour
figures. Nothing in step 1 depends on reading it back.

---

## 8. MegaMud path translation (`.mp` ↔ YAML)

### 8.1 The MegaMud formats (as MudPlay decodes them)

`.mp` path or loop file (`MudPlay/Game/Map/MpFile/MpFileParser.cs`):

```
[label][author]
[startCode:group:name]               optional header room lines (Rooms.md codes)
[endCode:group:name]
SSSSSSSS:EEEEEEEE:steps:use:gold:item:failPath:donePath
HHHHHHHH:flags:action                one row per step: the hashExits of the room being left,
...                                  hex step flags, then the action (n, s[search s], go path…)
```

- A **hashExits** is 8 hex characters: a 3-character name hash (the sum of position ×
  char code, low 12 bits, `FFF` for an empty name) and a 5-character exit code (ten exits
  weighted into five hex digits; doors, gates, key and toll exits count double; hidden
  exits are left out). `MegaMudHash.cs` reproduces it exactly.
- Step flags: Dark `0x1`, RestHere `0x2`, DontRest `0x4`, PitchBlack `0x8`, Stash `0x10`,
  NoAttack `0x40`, Relearn `0x80`, Disarm `0x200`, CanPick `0x400`. MegaMud's own
  bookkeeping bits: Suspect `0x100`, Cycled `0x8000`.
- Start hash = end hash means a loop; otherwise it's a goto path. Files may end with
  Ctrl-Z (0x1A).
- **`Rooms.md`**: `hashExits:options:minLevel:maxLevel:class:code:group:name`, MegaMud's
  named rooms. Options: Shop `0x2`, Bank `0x4`, Trainer `0x8`, StopBefore `0x10`, Avoid
  `0x20`, HideInGoto `0x40`, CommonRooms `0x80`.

**An `.mp` file holds no room numbers.** Importing means finding a walk through the room
graph whose rooms hash to the file's tokens, step by step.

### 8.2 The YAML path format

*Loops as routine rules are designed in §14.7: waypoints routed at run time, with this
step form kept for imports and for when the exact way matters.*

```yaml
schema: 1
name: Animated Defenders (NW Loop)
author: someone
kind: loop                      # loop | goto
start: 17/7039
end: 17/7039
requires: { gold: 0, item: null }
on_fail: null                   # path names, as MegaMud's fail/done paths
on_done: null
steps:
  - from: 17/7039               # the room being left
    go: n                       # compass move...
    pre: [search n]             # ...with MegaMud's bracketed extra commands
    flags: [rest_here]
  - from: 17/7042
    cmd: go path                # or a plain command
    to: 17/7300                 # where it lands, when the graph can't tell
source:                         # kept so export reproduces the original exactly
  mp_hashes: true
  use: "-1"
```

### 8.3 `.mp` → YAML (`mudparty-server paths import <file.mp> [--rooms Rooms.md]`)

1. Decode the file, keeping problems (bad rows, a wrong step count) as warnings, as MudPlay
   does.
2. **Find start candidates:** rooms whose `mm_hash` equals the start hash. A `Rooms.md`
   beside the file narrows them down by code and name.
3. **Walk** each candidate through the steps. A compass step follows the exit, and the room
   reached must hash to the next step's token. A command step (`go path`, `pull lever`)
   takes any room whose hash matches the next token, preferring rooms the game data links
   by a text or special exit.
4. **Exactly one surviving walk:** write the YAML. **More than one:** write the YAML for the
   first and list the others for the user to choose. **None:** report the step where every
   walk died.
5. `/room` (live, in a later step) can settle a remaining ambiguity by standing in the room.

### 8.4 YAML → `.mp` (`mudparty-server paths export <file.yaml>`)

1. For each step, compute the from-room's hashExits from game data, using `name` and the
   exits' `shows_as` (door-like doubled, hidden left out).
2. Write the header, the metadata line (start hash, end hash, step count, `use`, gold,
   item, fail and done paths) and one row per step, with flags mapped back to hex.
3. With `source.mp_hashes`, compare each computed hash to the one recorded at import and
   warn about any that differ. That flags a game-data change since import, or a
   `shows_as` mapping that is wrong.
4. Optionally (`--rooms`) write `Rooms.md` entries for the start and end rooms.

### 8.5 How faithful the round trip is

| Field | `.mp` → YAML → `.mp` |
|---|---|
| Label, author, steps, actions, pre-commands, flags | exact |
| Hashes | exact when game data and the `shows_as` mapping agree with MegaMud's view of the room |
| Header room codes | kept in `source`; regenerated only with `Rooms.md` |
| Unknown flag bits, `use` field | kept verbatim in `source` |
| Notes (`-- (Hidden/Needs 1 Actions`) | kept as `note:` on the step |

---

## 9. Logging

*Settled with the user 2026-11-22.*

### 9.1 Rules

- **Log everything, all the time.** No log levels and no switches to turn detail down.
  Text is cheap and a repro is expensive.
- **Every record carries a UTC time to the millisecond, a sequence number, the character and
  the session id.** The streams of one character, and of the whole party, can then be lined
  up by clock (MudPlay's lesson: cross-reference sections by their shared clock and read
  newest first).
- **The main character gets the same streams as a bot**, taken by the proxy.

### 9.2 Streams

Each character has four streams, and the server has one:

| Stream | File | Format | What's in it |
|---|---|---|---|
| Wire capture | `logs/<char>/wire.jsonl` | JSON lines: `{t, seq, dir: "in"\|"out", src, b64}` | Every byte read and written, one record per socket read or write, raw (IAC and ANSI included). `src` says who an outgoing write came from: `engine`, `user` (terminal or MudPlay), `remote:<sender>`, `web`, `inject`. **Always on.** This is also the test-fixture format (*§9.6*). |
| Transcript | `logs/<char>/transcript.log` | Plain text | Game lines with ANSI stripped and backspaces folded, each with its time. Commands sent appear as `> <cmd>  [src]`. Easy to read and grep. |
| Decisions | `logs/<char>/decisions.jsonl` | JSON lines: `{t, seq, kind, …}` | Why the engine did what it did. Each record holds its inputs and its result: `target_pick` (the rule that matched, the candidates in the room, the leader target and its age), `tier_pick` (MP, cost, the tiers skipped and why), `rest_start`/`rest_stop` (which threshold), `remote_cmd` (sender, channel, allowed or refused, reply), `login_step`, `party_join`, `telepath_ack`/`resend`, `rate_limited`, `stale_ignored`, `config_applied`, `gamedata_reopened`, `parse_miss` (a prompt the matcher didn't read). |
| Session events | `logs/<char>/events.jsonl` | JSON lines | Connect, login, logoff, link loss, reconnect, kill, death, level. Also copied to `mudparty.sqlite` (*§7.6*). |
| Server | `logs/server.jsonl` | JSON lines | Process start and stop, client connect and refuse, bot start and stop, config reloads with validation errors, game-data imports, proxy link up and down, panics. |

- Each file starts with a **header record**: character, session, mudparty version, the
  `config_rev` and `gamedata_rev`, and the realm. A new session inside a file writes
  another header. Sessions don't roll files, so reconnects don't eat archives.
- Headless bots are written by the server. An interactive client writes its own files in
  the same `logs/` folder, with the settings it gets in `welcome`. Before `welcome` (no
  server) it writes only to stderr.

### 9.3 Rotation and archives

Configured in `mudparty.toml` `[logging]`, applied to every stream of every character:

```
logs/Ermias/
  transcript.log          the live file, rolled when it reaches file_size (1 MB)
  transcript.1.log        the one before it
  transcript.2.zip        older, zip-compressed
  transcript.3.zip
  transcript.4.zip        the oldest; deleted when the next one is archived
```

- `file_size` (default 1 MB), `plain_files` (default 2: the live file and the previous one)
  and `zip_archives` (default 3) are all settings, so 5 files per stream per character by
  default. Any stream can override them (`[logging.wire] file_size = "5MB"`), since the
  wire capture fills fastest. At the defaults a busy bot's wire capture covers well under an
  hour.
- A file rolls only on a record boundary, so a JSON record or text line is never split.
- Zipping and deleting run on a background task. The writer never waits on compression.

### 9.4 Secrets stay out

- The password a bot sends is never written. The transcript shows `> ********`, and the
  wire record of that write is `{dir: "out", redacted: "password", len: n}` in place of
  its bytes.
- **The proxied main character:** the user types the password in MudPlay, and it passes
  through the tap. After a `Password:` prompt the proxy redacts outgoing bytes in the logs
  until that line ends. The bytes still reach silvermere unchanged.
- Snapshots scrub `secrets.yaml` values from everything they copy, as a second guard.

### 9.5 Snapshots (bug reports)

- From a **Report** button on each web card, or `#report [note]` in an interactive client.
  The server panel has a **Party report** that takes every character at once.
- Writes `reports/<char>-<UTC time>.md`:
  - the note;
  - versions, revs and the character's YAML with secrets scrubbed;
  - engine state: toggles, mode, target and tier, rest, party, stale flags, and the
    pacing queue;
  - the last 10 minutes (configurable) of decisions, events and transcript, newest at the
    end, with a reminder to read from the end.

  The wire tail for the same span goes in `reports/<char>-<UTC time>.wire.jsonl` beside
  it, so the incident can be replayed.

### 9.6 Captures as tests

A `wire.jsonl` file (or the wire tail of a report) replays through `mp-text`, `mp-parse`
and `mp-engine`, with the engine's clock driven by the record times. It should produce the
same decisions. A bug report then becomes a regression test without re-recording (*§10.3*).

---

## 10. Testing

*Settled with the user 2026-11-22.*

All of it runs under `cargo test`. Four layers, from fast to whole-system.

### 10.1 Unit tests

- `mp-telnet`: negotiation, IAC escaping, NAWS. The server side gives a client the same
  answers silvermere's option table does.
- `mp-text`: line splitting, backspace folding, prompts with no line end, text glued after
  a prompt, the clear-line prefix.
- `mp-parse`: the statline matcher generated from each template, and event classification.
- `mp-engine`, with an injected clock: tier choice at each MP, targeting rule order,
  rest thresholds, @-command permissions and replies, telepath pacing and resends, login
  steps and the credential guard.
- `mp-config`: every example in this doc parses and validates. Bad files give the error
  the web page will show.
- Log writers: rolling at the size, zipping, deleting the oldest archive, redaction.

### 10.2 The text guard: every pattern names its catalogue entry

- Each line `mp-parse` matches is declared with the silvermere catalogue key it comes from
  (`messages.invited_you_follow`, `messages.now_following`, the stealth lead-ins, the
  telepath and gangpath forms). This applies to any line silvermere prints from its
  catalogue.
- `silvermere-text`, a dev-dependency at the testkit's pinned revision, exposes the typed
  `Msg` enum, `render(msg, palette)` and the embedded `CATALOGUE_TOML`. A test builds each
  pattern's `Msg` variant (`Msg::InvitedYouFollow { who, whom }`), renders it in every
  palette and checks that the pattern matches the bytes and pulls out the right fields.
  The test also checks that the key a pattern names is in `CATALOGUE_TOML`. A renamed
  message then fails to compile, and changed text fails the test, as soon as the pin
  moves.
- A pattern with no catalogue key isn't accepted. Text that isn't in the catalogue (DLL
  text, the login menus) must name the capture or source it came from instead.

### 10.3 Replay tests with snapshots

- `tests/captures/<name>.wire.jsonl` files, recorded by the wire stream (*§9.2*) or taken
  from a report, replay through the engine on a clock driven by the record times.
- The decision stream they produce is compared with a stored snapshot (`insta`). A change
  in behaviour shows as a diff to review and accept, or as a bug to fix.
- Turning a bug report into a test means copying its `.wire.jsonl` into `tests/captures/`
  and accepting the snapshot once the fix is in.

### 10.4 Whole-system tests on an in-process silvermere

The real game comes from the `silvermere` package itself, a dev-dependency from
`jyurkiw/silvermere` at a pinned commit. (`silvermere-testkit`'s `TestServer` is only an
echo server; found in MVP 3.) mudparty's test-support crate `mp-silvermere` wraps it:

- `silvermere::serve_in_process` runs the whole server inside the test, on a temp SQLite
  from `silvermere-db`'s `TestDb`, with the telnet port bound to `127.0.0.1:0`
  (`Running::telnet_addr` says where);
- it seeds a fixture world and characters with silvermere's public helpers
  (`silvermere_engine::world::blank`, `clear_player`, `silvermere_auth::cli::create_with_cost`),
  as silvermere's own `bin/silvermere/tests/common` does;
- the manual clock is `cfg.game.sec_fast` set to a day, with `EngineInput::Tick` sent on
  `Running::to_engine` to step rounds by hand, so a 5-second round takes no time;
- the stock world (for the arena practice dummy in room 1/206: 9999 HP, no exp, fights
  that don't end) is imported the way silvermere's `tests/common/stock.rs` does, from the
  cached stock package. It comes in with combat (MVP 4).

The **mudparty server itself is under test**: a test starts an in-process silvermere,
then a mudparty server in the same process, pointed at it, with a temporary data root
(`mudparty.toml`, `secrets.yaml`, character YAML, logs). It then plays parts against it:

| Part | Played by |
|---|---|
| Headless bots | the server's own bot tasks, started through the CLI or a UI `order` |
| An interactive client | `mudparty-client`'s runner, driven from the test over the WebSocket |
| MudPlay | a telnet client (mudparty's own codec) connected to the **proxy** port, typing as the main character |
| The browser | a WebSocket client on `/ws/ui` |

Scenarios, each checked through silvermere's read-back records, mudparty's decision log
and the UI feed:

1. **Login:** a bot logs in, sets its statline and shows `fighting`/`idle` on `/ws/ui`.
   Wrong password, a timeout, and a second session for the same character are refused.
2. **Party join:** the main character (through the proxy) and two bots. The bots ask with
   `@invite`, or are invited by injection, then join and follow. When the main character
   moves, the bots' rooms follow.
3. **Combat tiers:** a caster bot with two tiers against the dummy. Tick rounds while
   draining MP and check the drop to the lower tier, the melee fallback, and the climb back
   as MP regenerates.
4. **Targeting:** a room with a priority-listed monster and the leader's target. The
   priority monster is attacked first, then the bot assists the leader. `@kill` overrides
   both.
5. **Stealth opener:** Avenger and Assassin bots sneak, then open with a spell backstab.
   The seeded RNG covers both the `strike from the shadows` and the `spots you` outcomes.
6. **Resting:** below the thresholds the bot sends `@wait` to the leader, rests, sends
   `@ok`, and stops at the targets. An attack during the rest ends it.
7. **@ commands:** each step-1 command from an allowed sender gets the right reply on the
   channel it came in on. A sender who isn't allowed gets nothing (or the failure reply).
8. **Proxy transparency and seats:**
   - the game data bytes the MudPlay stand-in receives equal what silvermere sent, and the
     other way round;
   - the seat's negotiation leaves the stand-in in the same option state a direct
     connection to silvermere would;
   - injected commands wait for the end of a half-typed line;
   - a typed `#who` is answered by mudparty and never reaches silvermere, while a typed
     `#something-else` arrives at silvermere whole;
   - the seated session is named from the typed account name;
   - detaching the seat turns the session into a bot rather than hanging up. This is the
     piece `#switch` will build on.
9. **Staleness:** edit a character's YAML mid-fight. It applies at the safe point, the ack
   clears the page's pending badge, and a bad edit keeps the old config with an error on
   the page. Re-import game data and check the rev change. Stop a bot's feed and watch its
   values go stale.
10. **Shutdown:** an orderly stop logs every bot off (silvermere's records show them out of
    the realm). Aborting the server task shows the hang-up path.
11. **Logging:** each scenario's logs are present, rolled and redacted, with no password
    bytes anywhere under the data root.
12. **Character sheets:** a bot with a known kit (testkit helpers set its items, spells
    and stats) is refreshed after login. Every part parses to the values the test set. Each
    carried and worn item gets its own `look` description, which settles which item names
    `look` accepts as printed. A `get`, then an `equip`, refreshes only the parts they
    should. Commands the main character types through the proxy fill its sheet without
    injection, and Refresh injects.

These run on every `cargo test`. The testkit's stock world is downloaded and cached once,
as in silvermere's own tests.

### 10.5 Path translation

- **Real files:** Winterhawk's path pack in `E:/Repos/Majormud_MDB_Repo/Winterhawks_Paths_Files_Only_v2.0/`
  (1,957 `.mp` files in `Default/`, with its `Rooms.md`). The test finds it through
  `MUDPARTY_MP_CORPUS` and is skipped when that isn't set. The files are not copied into
  this repo. For each file:
  - import against game data from the testkit's stock world;
  - export the YAML again and compare with the original;
  - report counts: resolved to one walk, ambiguous, failed (with the step), and
    round-trips that came back byte-identical.

  The counts are a snapshot, so a change in the hash, `shows_as` or the walker shows up as
  a diff. They also answer open question 5: a `shows_as` mapping that is wrong shows up as
  failed walks through those rooms.
- **Generated files:** loops and goto paths built from the game data with the hash code,
  including door, hidden and command steps, must import to the walk they were built from.
  These run without the corpus.
- **MudPlay as a cross-check:** for a sample of corpus files, mudparty's hashes match those
  computed by MudPlay's `MegaMudHash` on the same rooms (fixed values copied into the test,
  with no run-time dependency on MudPlay).

### 10.6 What isn't automated

- The web page's look and behaviour in a browser (its feed and orders are tested through
  `/ws/ui`).
- Real MudPlay through the proxy: it's played by a stand-in in tests, and checked by hand
  against the user's server at the end of each build step.

---

## 11. Questions settled (2026-11-22)

Each one was settled from silvermere's code, or by the user where noted. The answers are
written into the sections named.

1. **Gangpath replies** (*§6.3*): the reply verb is `bg <text>` (`BG`, `GB` and
   `BROADGANG` all work, `commands/table.rs`). The speaker gets their own gangpath back.
2. **Party join and refusals** (*§6.5*): joining a party is `follow <leader>`, and
   `join <name>` falls through to it. It needs an invite and the same room. The refusal
   lines and the `par` row format are taken from the catalogue.
3. **"Already logged on"** (*§6.2*): silvermere asks whether to take over the old
   connection. Bots answer `Y` by default, with `login.takeover: false` to answer `N`
   (user).
4. **Sneaking and hiding** (*§6.4*): success prints nothing extra, so a bot can only
   believe it's stealthed. The spell's lead-in line settles it.
5. **Exit types for the MegaMud hash** (*§7.5*): mapped from `display/exits.rs`. Toll is
   plain on silvermere, unlike MudPlay. The Winterhawk corpus test checks the mapping.
   Map Change exits carry their map in `par1`.
6. **Reply shapes** (*§6.3*): MudPlay's, tested against MudPlay's own parsing regexes
   (user).
7. **Cost of a hang-up** (*§4.4*): `game.pun_hup` defaults to `None` and is off on the
   user's servers (user), so a crash costs nothing in game.
8. **Reaching silvermere from mudparty** (*§10.2*, *§10.4*): `silvermere-text` exposes
   `Msg`, `render` and `CATALOGUE_TOML`, so no new silvermere API is needed. The testkit's
   stock package is cached in `~/.cache/silvermere/stock` (already there on this PC) and
   downloaded once elsewhere. silvermere's `[patch]` of `etcetera` doesn't carry over to a
   dependent. It only avoids a `user32.dll` load on Windows, so tests don't need it.

Settled earlier in review: no web login in step 1, queued injection, no low-HP escape in
step 1, and headless bots inside the server (*§1*). `mudparty` stays the working name.

Nothing is open. New questions go here as implementation finds them.

---

## 12. Build order (when the user says go)

**Work is tracked as GitHub issues** on [jyurkiw/mudparty](https://github.com/jyurkiw/mudparty/issues) (user, 2026-11-23): milestone **MVP** is #1–#7 (*§12.1*), **Step 1** is #8–#14, and **Later** is #15–#24 (*§13*). Each issue links its design section and says what "done" means. Anything new goes in as an issue, and the lists here are kept in step with them.

1. **Workspace and `mp-telnet`/`mp-text`/`mp-parse`, plus the log writers** (streams,
   rotation, zipping, redaction), tested on byte captures from a local silvermere (its
   testkit can drive one in-process). Logging comes first, so every later step is logged
   from its first run.
2. **The bot runner and the interactive client**: connect, auto login, statline, status
   bar.
3. **The server**: config store and watching, `secrets.yaml`, the client hub, `welcome` and
   config pushes, game-data import, headless bots started and stopped from the CLI, and
   the orderly logoff on stop.
4. **Combat and resting**, tiers and targeting, stealth opener.
5. **@ commands and party join**, `@wait`/`@ok`, telepath pacing.
6. **The proxy as seats and sessions** (*§2.1*): the tap, `#` line holding with `#help`
   and `#who`, then injection. `#switch` itself waits on the TODO list.
7. **The web page.**
8. **Path translation** and its CLI.

**Test-first** (user, 2026-11-22): within each step, the failing test is written before
the code wherever possible. Each step ends with its tests from *§10*: unit, catalogue and replay tests, and from step 3
on the whole-system scenarios on an in-process silvermere. Then a manual run against the
user's server.

### 12.1 The MVP cut

*Estimated 2026-11-22, from this design rather than measured. Agreed with the user to write in.*

The MVP is the smallest server worth using. **Headless bots that log in, join the user's
party, fight with spell tiers, rest with `@wait`/`@ok`, answer a few @ commands, log
everything and show on a simple web page.** The user plays the main character in MudPlay
connected straight to silvermere, with no proxy yet.

| Piece | In the MVP | Rough size (Rust, excluding tests) |
|---|---|---|
| `mp-telnet` | client side only | ~400 lines |
| `mp-text` | lines, ANSI, prompts, backspace folding, glued text | ~600 |
| `mp-parse` | the bot statline, catalogue-keyed patterns for login, combat, party and telepaths | ~1,200 |
| `mp-config` | `mudparty.toml`, character YAML, `secrets.yaml`, validation | ~500 |
| `mp-data` | import of rooms, exits, spells and classes from `silvermere.db` | ~400 |
| `mp-engine` | login (with takeover), combat tiers and targeting rules, melee fallback, rest, party join, pacing and telepath acks; `@health`, `@status`, `@where`, `@version`, `@auto-*`, `@stop`, `@do`, `@kill`, `@wait`/`@ok` | ~2,000 |
| Server | sessions as headless bots, the hub, YAML watching and `config_changed`, the log streams with rotation and zipping, `bot start/stop`, orderly logoff | ~1,200 |
| Web page | party cards: vitals, room, mode, target and tier, toggles, Start/Stop, a command box | ~300 Rust, ~400 HTML/JS |

That totals **about 5,000–6,000 lines**, with about as much again in tests, in **6–8
steps** of 600–1,000 lines each. The steps follow the build order above with the
non-MVP parts left out:

1. Workspace, `mp-telnet`, `mp-text`, the log writers.
2. `mp-parse` and the catalogue test, the replay harness.
3. `mp-config`, `mp-data` import, the server shell with sessions, a bot that logs in (the
   first whole-system test).
4. Combat: tiers, targeting, melee fallback.
5. Resting, party join, telepath pacing.
6. The step-1 @ commands.
7. The web page.

**Left for after the MVP**, which roughly doubles it to the full step 1 (about
11,000–13,000 lines):

- the interactive `mudparty-client`;
- the proxy with seats, the tap, `#` lines and injection (~700);
- character sheets;
- the stealth opener;
- game-data staleness;
- path translation (~900);
- snapshots.

**Where the effort and the risk are:**

1. **The parser.** It has to handle MajorMUD's text quirks: prompts with no line end, text
   glued to a prompt, a backspace inside direction words, the clear-line prefix, the space
   where `%r` sits. The catalogue-keyed patterns (*§10.2*) and replay snapshots
   (*§10.3*) are the guard.
2. **Combat details.** One spell per round, no casting at a corpse, "no effect" lines,
   room spells. MudPlay's `GAME_MECHANICS.md` has the rules, so this is careful work, not
   guesswork.
3. **The first whole-system test.** `silvermere-testkit` pulls silvermere's dependency tree
   into dev builds, so the first compile is slow. It needs the cached stock world, and
   driving `TestServer` from outside silvermere's workspace may need a small adapter. It is
   step 3, so it fails early if it's going to.

---

### 12.2 The 1.0 target (user, 2026-11-25)

**1.0 is one character per bot, running Newhaven alone**, with no player leading
(milestone 1.0, goal #57). The server logs the character in. The bot:
- gears up from the Newhaven shops;
- fights in the arena (1/2150, levels 1-3);
- retreats one room up to rest (Narrow Road, 1/2146) and goes back;
- kills the cave bear in Small Cavern (1/2156, through the arena's locked north door)
  when it respawns, about every 4 minutes, HP permitting;
- trains at the Adventurer's Guild (1/2147) up to level 3, since level 4 locks it out
  of the arena.

All of it is configured in hand-written YAML. Several such bots may run at once, each on
its own. **The goal test** (`mudparty-server` `tests/newhaven.rs`) runs the files in
`examples/newhaven` on the stock world and sees the whole loop. Building it found what
the pieces alone hadn't:
- The Small Cavern is dark (light -200), so a bot without a light can't see the bear and
  is beaten without fighting back: gear gained `use: light`.
- A kill past the experience cap prints no experience line, so the combat engine never
  saw the kill and attacked the corpse for ever. A bot held at level 3 gets there.
- A target that ran off was kept, and the walk paused for its fight never went on.
- `priority: ["*"]` attacks the shopkeepers (#89), and `fight_back` rarely fires (#85,
  *§6.4*), so the example names Newhaven's monsters.
- A character with no creation points spent loses to the bear; the test's warrior has
  them in strength, health and agility. The kobolds and rats of the Dungeon Entrance, on
  the way, take a share of its HP first (#88). The pieces: walking (#52, *§6.10*), the routine (#53), shopping (#55), training
(#56) and login steps (#33). The walk that could stall in a long session (#78) was a
mortally wounded bot whose every step was refused; such a walk now ends saying why.

**1.1 is the web UI for basic interactions** (user, 2026-11-27; goal #95). The first
party test on a real server showed how easy hand-written files are to get wrong: a
`.yml` extension was ignored without a word, and a bot with targeting but no `melee` or
`tiers` picked targets and never attacked. Every fix meant Stop and Start. So 1.1 makes
the page the place to set characters up:
- creating and editing character files there, passwords included (#58);
- a validator that catches traps as well as bad YAML, its findings on each card (#93);
- reloading an edited file into a running bot without a logoff (#94).

**1.2** is what the user is really after: **one bot controlling several characters in
concert**, the party director (#59). A director in the server sets goals for each
character, and the characters' engines keep their reflexes. It brings a
`parties/<name>.yaml`, keeps the game party underneath (#54), and also runs bots around
a main character played through MudPlay. The user chose this order knowing 1.0's
routine moves into the director, so #53 keeps it behind a narrow interface of goals.
Building routines and places on the page, with the map (#71), comes with it. (Until
2026-11-27 the director was 1.1 and the page editor 1.2.)

**The YAML's shape** was settled on 2026-11-26 (*§14*): a compiled room file, one router
with filters, routines as rule lists in their own files, and a map. 1.0 needs its first
six build steps (*§14.12*).

## 13. TODO (after step 1)

Each item is an issue in the **Later** milestone (#15–#24).

- **A log-file web interface**: browse, tail and filter each character's streams, and open
  snapshots, from the web page.
- A low-HP escape (`flee_below`, hang-up).
- The Avenger's `VANISH`.
- Walking paths and loops (they are stored and translated in step 1, not walked).
- Live `/room` checks for path import ambiguity.
- Other realms (stock boards, Paradigm) as realm profiles, and the `.mdb` game-data source.
- A web login token, for opening the page beyond loopback.
- **`#switch <character>` and the web page's Play in MudPlay button** (*§2.1*): re-attach a
  MudPlay seat to another session. The character left behind becomes a bot, and an offline
  target is logged in first. Also the **seat lobby**: after a MudPlay drop, hand the held
  session back on reconnect without a second login. The lobby plays the login dialogue
  with silvermere's rendered text and checks the password against `secrets.yaml`. Design
  its details before building it.
- **A MudPlay fork that knows about mudparty**, if it helps enough once `#switch` exists.
  The user can fork `E:/Repos/MudPlay`. MudPlay already speaks GMCP, and each character's
  services already reload on `ProfileService.ProfileLoaded` (the per-character services
  are disposed before a swap), so an in-place profile swap fits its design. The pieces:
  - **A GMCP channel between seat and MudPlay**, under its own `MudParty.*` package. The
    seat agrees to GMCP only when `mudparty.toml` says the MudPlay on that port is the
    fork, so stock MudPlay keeps today's behaviour.
  - **`MudParty.Seat.Attached {character}`** from the seat after a switch. The fork loads
    that character's profile in place, with no reconnect, and its automation, permissions
    and room tracking follow. This removes the limit in *§2.1*.
  - **`MudParty.Seat.Switch {character}`** from the fork, sent from a MudPlay menu or
    hotkey, in place of typing `#switch`.
  - **`MudParty.Seat.Resume {character}`** from the fork on reconnect, to take back a held
    session. It could replace the seat lobby: the fork proves itself with a token from
    `mudparty.toml` in place of replayed login screens.

  Not needed for step 1. Decide whether to fork once `#switch` works with stock MudPlay
  and the limits are felt in play.
- **Pull more from `silvermere.db`.** Beyond the world tables imported now, read what the
  database knows that the game's commands don't show: each character's full record
  (stats, abilities, inventory and keys, spells and their durations, quest flags, bank),
  item, monster and spell details for the sheets and the targeting rules, and live room
  state. Open read-only. The database is only as fresh as silvermere's last save, so
  anything read this way carries its own staleness, kept apart from the live sheets.

## 14. Scripts: places, routes, routines and the map (designed 2026-11-26)

The shape the YAML moves to, settled with the user while #55 was being built. It replaces
the `places:` and `routine:` sections of a character's file (*§6.11*, PR #61) before
shopping, training and the 1.2 director add more to them. Nothing has shipped, so the old
form is replaced, not kept beside the new one.

### 14.1 Why

- **Scripts are written by hand as well as by the page.** The web editor (#58 for
  character files in 1.1, routines and places with the map in 1.2) will write most of
  them, but writing one by hand stays a first-class way to do it. The files must read
  well, check well and explain their own mistakes.
- **A character's file mixed four kinds of knowledge.** Who the character is (name,
  login), how it reacts (combat, rest), facts about the world (room numbers, which shop
  sells what, where a monster spawns) and what it does all day (the routine). The world
  facts and the routine are the same for every Newhaven bot, yet each file repeated them.
- **Fixed slots don't grow.** `camp`, `rest_at` and `hunts` meant a new slot for each new
  behaviour: training, shopping, lair runs, the director's goals.
- **Perfect information, for now** (user). With `silvermere.db` the room record is
  complete, so mudparty is built on knowing the world exactly. Discovering rooms,
  exploring and coping with a wrong or partial map wait until a source without a complete
  record needs them (#20).

### 14.2 The files

```
places/<area>.yaml            compiled from silvermere.db: never edited (§14.4)
places/<area>.local.yaml      hand-written beside it: aliases, tags, avoid marks, map pins
routines/<name>.yaml          a behaviour, reusable by any character (§14.6)
characters/<name>.yaml        who, how it reacts, what it carries, which routine (§14.8)
parties/<name>.yaml           1.2: the director's groups (#59)
paths/<group>/<name>.yaml     loops imported from MegaMud .mp files (§8)
```

A character's file may still inline `places:` and `do:` for a one-off; the shared files
are the usual way.

### 14.3 Naming a room

Room names go a long way, but some rooms can only be named by number (user). A place is
resolved in this order:

1. an alias from `places/<area>.local.yaml`;
2. the compiled slug: `weapons-shop`, or `dungeon:entrance` for another area's. A name an
   area repeats gets its room number in the slug: `arena-2150`, and `arena` then names
   both, an error that lists them;
3. the game's own name, `"Newhaven, Arena"`, while only one room has it. A repeated name
   is an error that lists the rooms it could be;
4. the key, `1/2150`, which always works.

The long form `{ room: 1/2150, name: "Newhaven, Arena" }` (`mp-config` `Place`, #66),
written wherever a file names a place, names the room by number and checks the name: if the world is renumbered, `check` (*§14.9*) says the name no longer
matches. The web editor writes this form, so a file it saved stays readable. MudPlay saves
its loops' rooms as `1/5` strings for the same reason (`LoopWaypoint.cs`).

### 14.4 The compiled room file

`mudparty-server places compile [--area <name>] [--root <data root>]` (#63) reads the
`silvermere.db` that `mudparty.toml` names, read-only (the same tables the world-editor
prototype reads, silvermere `tools/world-editor-proto/src/data.rs`), and writes one file
per area into `places/`: 316 areas for the stock world's 26,720 rooms. From the stock
world (`mp-data` `places.rs`, trimmed):

```yaml
# Compiled from silvermere.db by `mudparty-server places compile`. Don't edit:
# names and notes go in the area's .local.yaml beside it.
schema: 1
area: newhaven
source:
  silvermere_db: E:/silvermere-data/silvermere.db
rooms:
  adventurers-guild:
    key: 1/2147
    name: Newhaven, Adventurer's Guild
    type: shop
    exits:
      south: narrow-road-2146
    boss: Corwyn
    shop: { name: Training Room, type: training, trains: any class }
  arena-2150:
    key: 1/2150
    name: Newhaven, Arena
    type: arena
    levels: 1-3
    exits:
      north: { to: dungeon:entrance, kind: door, lock: locked, bash: 30 }
      up: narrow-road-2146
    spawns:
      monsters: [giant rat, lashworm, filthbug, carrion beast, acid slime, kobold thief]
      max: 2
  narrow-road-2146:
    key: 1/2146
    name: Newhaven, Narrow Road
    exits:
      down: { to: arena-2150, kind: block_guard }
      east: narrow-path
      north: { to: adventurers-guild, kind: block_guard }
      west: { to: healer, kind: level, levels: 1-3 }
  weapons-shop:
    key: 1/2141
    name: Newhaven, Weapons Shop
    type: shop
    exits:
      south: village-entrance
    boss: Nathaniel
    shop: { name: Newhaven Weapon Shop, type: specific, sells: [quarterstaff, club] }
```

and in `places/dungeon.yaml`:

```yaml
  small-cavern-2156:
    key: 1/2156
    name: Small Cavern
    type: lair
    exits:
      south: entrance
    lair: { monsters: [cave bear], max: 1, respawn: 2m }
```

- **What it holds:** each room's key, name and type; an arena's player levels; its exits
  by direction (a plain exit as a place; any other with its kind, and what the compiler
  reads of its parameters: a door's or gate's lock, bash bonus and key, a level or
  alignment range, a toll, the item, class or race an exit wants, the raw slots for the
  rest); what spawns there; a lair's monsters, cap and respawn; the room's own monster
  (`boss`); and its shop (type, what it sells, whom it trains).
- **Areas** follow MajorMUD's `Area, Room` names, but most rooms have none (20,253 of the
  stock world's). A room without one joins the area of the nearest room that has one, by
  walking distance: Small Cavern, one step past `Dungeon, Entrance`, is in `dungeon`; the
  Docks and Small Pier are in `newhaven`. A room no named room reaches goes in `map-<n>`.
  The walk runs in key order, so the same world always gives the same areas.
- **Slugs** come from the name after the area prefix (or the whole name); a name the area
  repeats takes its room number. Newhaven has two `Newhaven, Arena` rooms (2150, and 2155,
  a 1-1 arena below it) and two Narrow Roads, so its arena is `arena-2150`, and `arena`
  wants an alias.
- **Exits name places:** a slug in the same area, `area:slug` in another, or the key for a
  room no area has.
- **Spawns and respawns come from silvermere's rules** (`generate.rs`, `Request::for_room`;
  `silvermere-rules` `spawn.rs`). A room's `mongroup` is the `montype` its monsters are
  drawn from (-1 any), and its `minlevel`..`maxlevel` their `genindex` range, unless
  `genmonster` names one monster. These are the same two fields an arena reads as its
  players' levels: legacy fields read two ways, kept as they are. A lair fills when a
  player walks in and nobody else is there, once `regendelay` minutes have passed since
  its last kill (`respawn: 2m`), or the server's `regen_mins` when the room sets none
  (`server default`). A unique monster (`maxalive` 1) also waits its `regenhrs`
  (`unique_hours`). So the cave bear is tied to Small Cavern after all: group 6, index
  66 to 66, which is the cave bear's.
- **One format, any source.** A later source (a MegaMud `.mdb`, exploration) writes the
  same file, so scripts and the router never know where the facts came from. The world
  editor could read and write it too: the two share the format, not code.
- **The local file** (`<area>.local.yaml`) holds `aliases:` for now
  (`arena: 1/2150`, `road: narrow-road-2146`); tags, avoid marks and map pins come with
  the router (#64) and the map (#69). Recompiling never touches it. An alias that names
  no room is an error naming the file and the alias.
- **Naming a room** (*§14.3*) is `mp-data` `Places::resolve`.

`gamedata.sqlite` (*§7.5*) stays the router's fast index; the compiled file is the
readable record scripts are written and checked against.

### 14.5 The router

Every move goes through one router: a weighted shortest path over the compiled exits.
What a walk may use is decided by **filters that wrap each other**, as MudPlay does
(`IRoomFilter`, `WalkAroundRoomsFilter`): a filter holds for every plan a walk makes,
re-plans on the way included, rather than being a flag on one search.

```yaml
avoid:
  rooms: [healer]                     # never walked through
  areas: [sewers]
  exits: [trap, toll]                 # exit kinds
prefer_not: { dungeon-entrance: 20 }  # allowed, but costs 20 steps more
```

- **Built in #64** (`mp-data` `GameData::route_with`, `RouteRules`): Dijkstra over the
  walkable exits. Each step costs one, plus a room's `prefer_not`. Ties go to the exit
  found first, in direction order, so a world and its rules always give the same walk.
  The rules are a plain set held by the walk rather than wrapped filters: rooms and the
  rooms of areas avoided, exit kinds avoided, exits the game refused, and the costs. A
  walk and a routine's rules come together by joining the sets.
- **With no walk, the error says what's in the way.** The router plans once more without
  the rules and names the first room, exit kind or refused exit on that walk:
  `every way from Newhaven, Arena (1/2150) to Newhaven, Weapons Shop (1/2141) is barred:
  the shortest goes through Newhaven, Narrow Path (1/2143), which is avoided`.
- **Exits the game refuses** (a level exit, an alignment or timed one) are added to the
  walk's rules, and it plans round them from where it stands, up to three times, before
  ending with the game's words (*§6.10*). That gates exits by level without knowing the
  character's level.
- **Given a level** (`RouteRules::level`, #66), a level exit that won't let the character
  through isn't taken, as silvermere decides it: at least `par1`, and at most `par2`
  unless `par2` is 0. The error names it: `by an exit for levels 1 to 3, and the character
  is level 5`. `check --level` uses it. A running bot does once it knows its level, from
  `exp` or a training (#56).
- **Where it's set:** a routine's `avoid:` and `prefer_not:` (the current routine, until
  #65 moves them onto routines and rules), and the page's Go order's `avoid`, for one
  trip. Rooms are places (a key, the character's `places:`, or the compiled room files);
  areas are compiled areas; exit kinds are the room files' names. They are looked up
  when the bot starts, so one that doesn't read stops the bot with its name.
- With perfect information, `check` (#66, *§14.9*) proves that every place a script names
  can be reached without crossing anything avoided, before the bot runs it.
- The walker (*§6.10*) keeps carrying out the steps: doors, bashing, confirming each
  room, pausing for a fight.

### 14.6 Routines: a list of rules

A routine is a list of rules, read top-down; the first that applies wins. The last rule
is what the bot does when nothing else is due. It reads like the combat targeting list
(*§6.4*), so a file has one way of reading.

```yaml
schema: 1
name: newhaven-arena
places: newhaven
avoid: { rooms: [healer] }
do:
  - rest:    { at: narrow-road, when: { hp_below: 50% }, until: { hp: 100% } }
  - gear:    { when: missing }
  - train:   { at: guild, up_to: 3 }
  - circuit: { lairs: [small-cavern], respawn: { small-cavern: 4m }, when: { hp_at_least: 80% } }
  - camp:    { at: arena }
```

- **Conditions** (`when:`): `hp_below`, `mp_below`, `hp_at_least`, `mp_at_least`; every
  one given must hold. `level_below` and time windows later (a train rule says how far with `up_to`, #56).
  The `gear` rule needs no `missing` condition: it applies by itself when the gear wants
  checking (*§14.7*).
- **Taking over:** a rule's `when` says when it starts; once started it runs until it's
  done. A rule higher up that starts to apply takes over at a safe point, never mid-fight
  (fleeing is #16). The lower rule keeps its state (a circuit its lairs' clocks) and applies again
  once its `when` holds.
- **Parameters:** a routine declares them with their defaults under `params:` and uses
  them as `${name}`; a character changes them:
  `routine: { use: newhaven-arena, with: { rest_below: 60% } }`. A value that is only
  `${name}` takes the parameter as YAML would read it; one with more around it takes the
  text. A character may set only parameters the routine declares, and a routine may use
  only those it declares.
- **Built in #65** (`mp-config` `routine.rs`, `mp-engine` `routine.rs`): the rules `rest`
  (`at`, `when`, `until`, the character's `rest.stop_when_all` when left out), `hunt`
  (`monster`, `at`, `every`, `when`; #67 made it a circuit) and `camp` (`at`,
  `look_every`). A routine file has `schema`, `name`, `places` (the area its places are
  looked for in first), `params`, `avoid`, `prefer_not` and `do`. It is checked when read:
  a rule below one that always applies (a camp) would never run, and a rest says when.
  Its places are looked up when the bot starts. A routine may be written inline in the
  character's file (`routine: { do: [...] }`); a character's own `places:` are gone, in
  favour of the compiled room files and their local aliases.
- **The goals stay the interface** (*§6.11*): each rule hands the engines goals (walk,
  rest, camp, kill, buy and equip, train), and the 1.2 director (#59) hands out the same
  goals, with rules of its own (follow the leader, hold the group).
- `@stop` and `@rego` pause and resume the whole routine, as now.

### 14.7 Script shapes

The activities a rule can run:

| Shape | What it does |
|---|---|
| `camp` | Stay in a room and fight what comes, looking around now and then (built, *§6.11*). |
| `loop` | A cycle of waypoints, or exact steps. |
| `circuit` | Visit lairs as they respawn, ordered by travel cost. |
| `sweep` | Visit every room of an area, least recently seen first, for wandering monsters. |
| `errand` | Go, do something, come back: buy and equip (`gear`), `train`, heal, sell. |
| `follow` | Stay with a leader: the game party, then the 1.2 director. |

**Loops.** As MudPlay's v3 loops (`LoopWaypoint.cs`, which replaced v2's explicit steps):
an ordered cycle of waypoints, each with an optional command to send on arrival, a delay
before going on, and `no_rest` for a room too dangerous to sit in. The moves between
waypoints are routed when the loop runs, so they follow the avoid filters and survive a
map change.

```yaml
loop:
  - arena
  - { at: dungeon-entrance, no_rest: true }
  - { at: small-cavern, command: search, delay: 2s }
```

Where the exact way matters (traps, one-way exits, MegaMud's own paths), `steps:` lists
the moves, and `check` confirms each one reaches the room it should. MegaMud `.mp` imports
(*§8*) come in as steps.

**Circuits.** A list of lairs and their respawn times; the bot goes from lair to lair as
each is ready, as MudPlay's Auto-Lair does (`AutoLairScheduler.cs`, `LairTimerStore.cs`):

- **A lair's clock** starts at its last kill on Stock (at the entry, on Paradigm). It is
  ready at clock start + respawn. A lair never seen this session is ready now.
- **It repopulates when entered after it is ready**, so the bot must not step in early: it
  waits in the lair's **wait-room**, the nearest neighbour that isn't itself a lair and
  isn't avoided.
- **Picking the next lair** scores each by *slack*, arrival time minus ready time
  (counted to the wait-room). Positive slack is respawn time wasted; negative is time spent
  waiting. The default picks the closest lair with slack of at least zero, else the one
  whose fight can start soonest. A throughput option counts waiting as free.
  Re-planning at every lair matters more than solving the whole tour, since which lairs
  are ready changes while the bot travels.
- **Bosses** can spawn early: on Stock at 87.5% of the timer, then surely at 100%
  (`BossTimerMath.cs`). A circuit can head for the early window.
- **Timers live in `mudparty.sqlite`**, so they survive a restart and a reconnect.
- `idle:` says what to do when nothing is ready (`{ camp: arena }`); the default waits in
  the next lair's wait-room.

A `hunt` of one monster, as #61 built it, is a circuit of one lair.

**Built in #67** (`mp-config` `CircuitRule`, `mp-engine` `routine.rs`, `mudparty-server`
`circuit_lairs` and `history.rs`):

```yaml
- circuit:
    lairs: [bear]            # places
    respawn: { bear: 4m }    # by the lair as written; the room file's time when left out
    wait: 1m                 # how long it will wait outside for a lair; none by default
    pick: soonest            # or throughput
    when: { hp_at_least: 80% }
```

- **In a rule list, `idle:` is the rules below.** The circuit applies when its `when` holds
  and some lair will be ready by the time the bot gets to its wait-room, waiting up to
  `wait`. When none is, the rules below run (the camp), and the circuit applies again once
  one will be. So a one-lair circuit above a camp is #61's hunt, set off early enough to
  arrive as the lair fills.
- **Picking:** `soonest` takes the nearest lair ready on arrival, else the one whose fight
  can start first; `throughput` the nearest within `wait`, waiting counted as free.
  It plans again after every lair, from where the bot stands.
- **Travel** is reckoned from the router's route to the wait-room, 2 s a step (moving and
  confirming the room with `/room`), and kept per pair of rooms. A lair it can't walk to is
  left out.
- **The wait-room** is the nearest room by exits that isn't a lair itself, never through an
  avoided room (`Places::wait_room`). In the stock world the cave bear's Small Cavern opens
  only onto the Dungeon Entrance, a lair of rats (`1m`), so the bear is waited for in the
  arena: the bot walks through the rats' lair each time, filling it on the way, as a
  player would. The circuit's `when` was checked as it set off, so a bot worn below it
  on the way still goes in (#88). A lair with nowhere to wait is an error when the bot starts, and in `check`.
- **In the lair** every monster it shows is killed in turn (a kill order each), and the
  lair is clear once none is left. That starts its clock, as a visit that finds it empty
  does. A monster left unfought for 30 s is given up on, which starts the clock too. A
  lair whose fight was cut short (a rest above took over) isn't cleared, so it's still
  ready afterwards.
- **Respawn times:** the room file's `lair.respawn` when the rule gives none. silvermere
  counts it in whole minutes from the kill's minute (`regen_blocked`), so a lair can be a
  minute late; an empty visit then starts its clock again. A room that isn't a lair needs
  a time given, as does a lair at the game's default (`server default`), which the room
  file doesn't know.
- **Clocks** are the world's, not a character's: `mudparty.sqlite`'s `lairs` table keeps
  when each was last cleared, by any bot (`LairClocks`). The session reads them when the
  routine starts and writes each lair as it's cleared.
- **The card** shows `going to Small Cavern`, `waiting for Small Cavern to fill` and
  `killing cave bear`, and is told `cleared Small Cavern` (or `gave up on cave bear in
  Small Cavern`) when the bot is done with it. A monster in the lair that fights the bot
  as it walks in is fought by the combat rules on the walk, so the lair may already be
  clear when the walk ends.
- **Not built:** bosses' early window (a leftover issue), and a circuit's own `idle:`
  rule.

**Errands.** `gear` (built in #55, `mp-engine` `shop.rs` and the routine's gear rule)
keeps the character's `gear:` (`{ item, use: wield | wear | light, at }`):

- It applies when the gear wants checking: when the routine starts, and after each rest.
  It sends `i` and reads the carried list (`You are carrying ...`, wrapped at 79 columns,
  up to the keys line): a worn item is named with its place (`padded vest (Torso)`), the
  weapon with `(Weapon Hand)` or `(Two handed)`, duplicates with a count in front.
- The first item carried but not in use is put on where the bot stands (`wield`, `wear`,
  or `light` for a light source, which silvermere answers `You lit the torch.` and then
  shows readied, `torch (Readied/n)`; one that burns out is replaced only at the next
  check, #86); the first one missing is bought at a shop that sells it, and put on. Then it
  looks again, rather than trusting what it did; with nothing left to do, it is done until
  the next rest.
- **Shops:** `at:` names one; otherwise the compiled room files' shops that sell the item,
  those in the routine's `places` area first, then by key. A shop it can't walk to gives
  way to the next. Choosing the nearest by route waits for the router to be asked for a
  walk's length.
- **An item it can't get** (`You cannot afford`, a refusal to sell or to wear or wield, no
  shop selling it, no way to any) is skipped and said on the card (`Report::RoutineNote`),
  and not tried again until the bot restarts.
- A buy, wield, wear or light with no answer for 10 s is sent once more, then given up.

`train` goes to a trainer while the next level is affordable and below `up_to` (#56).

**Built in #56** (`mp-config` `TrainRule`, `mp-engine` `routine.rs`, `mp-data`
`Places::trainers`): `- train`, or `- train: { at: guild, up_to: 3, when: {...} }`.

- **The experience.** A routine with a train rule wants the bot's level and experience.
  The session sends `exp` while they aren't known (at the start, and after each training),
  reads `Exp: .. Level: .. Exp needed for next level: <left>`, and takes each `You gain N
  experience` off `left`. silvermere shows `left` as 0 once the experience is there, and
  `train` checks the same figure.
- **When.** It applies when its `when` holds, the level and experience are known, `left`
  is 0, and the next level is no higher than `up_to` (left out, as high as a trainer
  goes).
- **Where.** The trainer `at:` names, or the compiled room files' training shops for the
  character's `class:` or for any class, those in the routine's `places` area first.
  Newhaven's Adventurer's Guild (1/2147) trains any class, levels 1 to 10, for 5 silver a
  level (silvermere's `training_fee_copper`: the level times 5 silver, plus the shop's
  markup).
- **Training.** At the trainer it sends `train`, and reads `You hand over .. and you
  receive training to attain level N.` Then it waits where it stands until `exp` answers
  for the new level, so a bot with experience for several levels trains them one after
  another without walking off. A `train` with no answer in 10 s is sent once more.
- **Refusals.** `You do not have the money required for your training.` and `You do not
  have the required experience to train yet!` are noted on the card and tried again in 5
  minutes. The others (not a training room, another class's trainer, a level outside the
  trainer's, tournament mode) refuse that trainer: the next is tried, and with none left
  training stops until the bot restarts.
- **The level on walks.** The session keeps the level from `exp` and each training, and
  each walk's rules carry it (`RouteRules::level`), so a level exit that won't let the bot
  through isn't planned through (*§14.5*).
- **Not built:** training stats (silvermere's `train stats` is the creation editor, not a
  stat at a time), and stopping the routine at the cap. In a rule list the rules below
  run once `up_to` is reached, so the bot fights on, as the 1.0 loop wants.

### 14.8 A character under the new shape

```yaml
schema: 1
name: Ermias
class: Warrior
login: { account: Ermias }
combat:
  targeting: [kill_order, { priority: ["*cave bear", "*kobold thief", "*giant rat"] }]
  melee: a
rest:
  until: { hp: 100% }
gear:
  - { item: club, use: wield }
  - { item: padded vest, use: wear }
  - { item: torch, use: light }
routine: newhaven-arena
```

`gear:` says what the character carries; the routine's `gear` rule keeps it so.
Targeting rules may be written as bare words when they take no value.

### 14.9 Help for writing by hand

Built in #66.

- **JSON Schemas** are generated from the config types with `schemars` (`mp-config`
  `schema.rs`). There is one each for a character's file, a routine's and a places local
  file, kept in the repository's `schemas/`. With them an editor that has a YAML extension
  (VS Code's, for one) completes keys, shows each field's doc and marks errors as you
  type. `# yaml-language-server: $schema=...` at the top of a file, or the editor's
  settings, points to one.
  - The types written by hand (rules, targeting rules, amounts, durations, places, a
    routine reference) describe themselves.
  - A routine's `${name}` parameters are allowed wherever an amount, a duration or a cost
    goes.
  - `mudparty-server schema [--out <folder>]` writes them. A test fails when `schemas/`
    differs from what the types generate, and another checks the README's examples
    against them.
  - A schema checks the shape of a file. What it means is still the loaders' to check (a
    rest rule's `when`, a percentage over 100%, a rule that would never run).
- **`mudparty-server check <file> [--level <n>]`** (`mudparty-server` `check.rs`) prints the
  file as the server reads it:
  - defaults filled in, and a routine's parameters;
  - every place as the long form, with the world's name;
  - a rest's `until` from the character's `rest.stop_when_all`;
  - a character's routine written inline, so the output is itself a file that reads the
    same (`CharacterConfig::to_yaml`, `Routine::to_yaml`);
  - where each item of gear would be bought, in comments.

  It takes a local file too, printing each alias as its key with the room's name beside
  it. Then it lists what would stop the bot, each `file: field: why` with the file named
  from the data root, and exits 1 if anything would.
- **What it proves:**
  - **Reachability.** Every place the rules name can be walked to and back from the place
    of the last rule (where the bot is when nothing else is due), keeping to the
    routine's `avoid` and `prefer_not`. Between them, every place reaches every other.
    With a gear rule, a shop for each item can be walked to and back from too, and the
    first that can is the one named.
  - **Level exits,** given `--level` (*§14.5*). Without it they're walked, and the game
    decides.
  - **Long-form names** (*§14.3*): a room by number whose `name` isn't the world's any more
    is said to have been renumbered, perhaps.
  - **Unknown places:** a name no room answers to, an area no file has.
- **The game data** is opened as the server opens it, imported first when `gamedata/`
  hasn't it.
- Errors name the file and the field, as now (*§7*). A file that doesn't read is one
  error.

### 14.10 The map

MudPlay has a graphical map (`Controls/MapControl.cs`, `Game/Map/BfsMapper.cs`), and the
web page needs one too (user).

- **Layout** as MudPlay's `RoomLayout`: each level a 2D grid, laid out by a breadth-first
  walk from a room. Up and down exits are glyphs on a room, not a third axis. A room
  MajorMUD's geometry won't fit (north then south doesn't always come back) is left out and
  its exit drawn as a stub; a layout full of stubs is tried again from a more central
  room. The world editor's `layout.rs` meets the same problem in 3D, moving a room to the
  nearest free cell.
- **Computed when drawn**, not stored in the room file, since it depends on which room is
  the centre. The local file can pin a room's position where the automatic layout reads
  badly.
- **Built in three steps:**
  1. *Read-only:* the area around a bot, every bot's position, and the route each is
     walking.
  2. *Overlays:* lairs with their timers, avoided rooms, shops and trainers, the loop or
     circuit a bot is running.
  3. *Editing (1.2, #71):* click a room to make it a lair, a waypoint or avoided,
     which is how MudPlay sets lairs and loops. The map becomes the script editor.

### 14.11 Moving to it

- PR #61's `places:` and `routine:` are replaced (#65); the README follows the code.
- #55's `gear:` lands as the `gear` errand, its shops found from the compiled file.
- The engine pieces stay: the walker (*§6.10*), the goals and the session's wiring
  (*§6.11*), the rest and combat engines.

### 14.12 Build order

1. The compiled room file and `places compile` (#63).
2. The router and its filters (#64).
3. Routines as rule lists in their own files, with shared places (#65).
4. `check` and the JSON Schema (#66).
5. Circuits (#67), replacing #61's hunts.
6. Errands: gear (#55) and training (#56).
7. Loops (#18) and sweeps (#68).
8. The map: read-only (#69), overlays (#70), editing (#71).

1.0 (*§12.2*) needs steps 1-6.

## 15. The party's money: sheets, a banker, bank runs and shopping (designed 2026-11-27)

The user's next web work, after the first party test on a real server: each character's
stats and inventory on the page, and the party's money handled as a player would. One
member holds the money, banks it and does the shopping, because shop prices depend on the
buyer's charm.

### 15.1 Why one member holds the money

silvermere's prices (`silvermere-rules` `economy.rs`, Stock's formulas):

- **Buying** costs `cost × (100 + markup)% × (110 − charm/5)%`: 110% of the list price at
  charm 0, 100% at 50, 90% at 100.
- **Selling** pays `cost × (charm/2 + 25)%`: 25% at charm 0, 50% at 50, 75% at 100.

So the party's highest-charm member should do all its buying and selling. A shop's `list`
shows the prices, and its `(You can't use)` and `(Too powerful)` flags, for whoever looks.
A character may buy an item it can't use, so the banker can buy for every class.

Moving money and items, the rules this design relies on (silvermere `commands/inventory.rs`,
`commands/bank.rs`):

- **`give <n> <currency> to <player>`** needs both in one room. `n` is at most 32,767 (a
  16-bit amount), and the game moves only what the receiver can carry: 3 coins of a type
  weigh 1, against carrying capacity (48 per point of strength). It says how many went:
  `You gave Bob 10 copper` to the giver, and `Alice gave you 10 copper` to the receiver.
  A player who has set NORECEIVE refuses: `Bob refuses your offer.`
- **`give <item> to <player>`**: `You just gave <item> to <who>.` and
  `<who> just gave you <item>.` It's refused if the receiver can't carry it.
- Giving, depositing and withdrawing all **end a rest**.
- **Coins on the floor** show in the room as `You notice 3 silver nobles, 12 copper
  farthings here.`, and are picked up with `get <currency>` or `get <n> <currency>`
  (`You picked up 9 copper farthings`).
- **Banks** are shops of type `bank` (the compiled room files show them as
  `shop: { type: bank }`).
  - `deposit <n>` and `withdraw <n>` count in copper value, not coins, at most nine digits.
  - The book is the account's, kept separately at each bank. `bankbook` shows it.
  - After every deposit or withdrawal the purse is changed up into the fewest coins
    (`economy::normalise`): 3,000 copper becomes 3 gold. So one visit can cut a purse's
    weight a hundredfold.

### 15.2 The party file

A party file now, holding only what the money needs. The 1.2 director (#59) grows it
into its groups (*§14.2*: `parties/<name>.yaml`).

```yaml
# parties/main.yaml
schema: 1
name: main
members: [Artemis, Braven, Grond, Tzarst]
money:
  banker: highest_charm     # or a member's name
  keep: 0                   # what each other member keeps, in money (below)
  banker_keeps: 5 gold      # what the banker carries after a bank run, for shopping
  bank_at: newhaven-bank    # optional: the bank to use; else any bank the party is in
```

- **`members`** are character files. The main character can be one, and so can bots.
  A character may be in one party.
- **`banker`** is a member's name, or `highest_charm`: the member whose status sheet
  (*§15.3*) shows the most charm, ties going to the first listed. A member with no sheet
  yet isn't counted until it has one. The page shows who it is and why.
- **`keep`** is what each other member holds back (0: give everything). **`banker_keeps`**
  is what the banker carries out of the bank, so it can shop.
- **Money** is written as coins: `5 gold`, `120 silver`, `2 platinum 50 gold`, or a bare
  number of copper. The coins are silvermere's: runic (100 platinum), platinum (100 gold),
  gold (10 silver), silver (10 copper), copper.
- **A banker you play in MudPlay** (the main character): bots give to it by themselves,
  since giving needs nothing from the receiver. Its own steps (depositing, withdrawing,
  buying, giving items) go into your session as injected lines only when you press the
  page's button for them, never by themselves, since you're at the keyboard.
- `check` reads a party file like the others: members that have no character file, a
  banker who isn't a member, money that doesn't read.

### 15.3 Sheets: stats, inventory and wealth

The character sheets of *§6.9* (#10), built now:

- **Status** (`st`): the six stats (charm among them), level, experience, HP, mana, AC and
  DR, and the rest *§6.9* lists.
- **Spells.**
- **Inventory** (`i`): carried items, keys, **wealth** and **encumbrance**. The carried
  list and `Wealth:` are already read for gear (#55).
- **Appearance** (`look <self>`): what's worn and where.

They refresh as *§6.9* says: all at login and on the Refresh button; one part after a
change; never in combat. The parts added for money:

- **Wealth** is kept as coins by type, as `Wealth:` gives it, with its weight
  (`coins/3` per type) and the carrying capacity from strength. So the server knows how
  many more coins each member can take.
- A money move (a give, a deposit, a purchase) refreshes Inventory once it's done.

**On the page** a sheet opens from each card. Its tabs:
- Stats;
- Spells;
- Inventory and equipment, with weight and wealth.

Item actions on the sheet:
- **give to** another member;
- **equip**, **remove**, and **drop**;
- **sell**: the item goes to the banker, who sells it at its own price.

Each action goes through the bot's pacing as an order, as the page's other buttons do, and
the sheet refreshes after it.

**A party panel** shows the money:
- the banker, and why it was chosen;
- each member's coins, weight, and how much more it can carry;
- the banked total at each bank seen (`bankbook`);
- what the members' gear lists want, priced at the banker's charm where a shop's list has
  been seen;
- the Bank run and Shop buttons (*§15.5*, *§15.6*), with whether each can happen here.

### 15.4 Giving to the banker

Every other member gives its coins above `keep` to the banker, by itself, when:

- they're in the same room, and the banker is in the game;
- no one in the room is fighting (no target, no `*Combat Engaged*` running);
- the member isn't resting, since a give ends a rest. It gives after the rest.

How:
- **Highest coin first**, which moves the most value for the least weight: one `give <n>
  <currency> to <banker>` a currency, `n` at most 32,767.
- The game caps each give at what the banker can carry. The bot reads `You gave …` for
  what went, and keeps the rest.
- **When the banker is full** (the server knows from its sheet), members stop giving until
  it has room. The panel shows the coins still out with members, and that a bank run is
  due.
- A refusal (`Bob refuses your offer.`, or `Why would you want to give to that?` when
  the banker isn't there after all) is said on the card and not tried again until something changes: the banker comes back, or the file
  changes.
- Giving waits behind a routine's own steps and a walk: it happens between them, never
  mid-walk.

The banker gives nothing back by itself in 1.1. A member that needs money (to train, or
for the healer) gets it from a page order, *give money to*. A float for training fees is
`keep`.

### 15.5 Bank runs

A bank run happens when the banker is in a bank room. The leader brings the party there:
the party follows its leader, and in 1.1 that's usually you. A run can start three ways:

- by itself, whenever the banker enters a bank, if it has money above `banker_keeps` or
  members have coins out;
- from the panel's **Bank run** button;
- `@bank` from an allowed player.

The run:

1. The banker deposits everything above `banker_keeps` (`deposit <copper value>`). Its
   purse is changed up into the fewest coins as it does.
2. Members in the room give their coins above `keep` to the banker (*§15.4*). Its purse
   now has room again.
3. Again from 1, until no member in the room holds more than `keep`, or a round moves
   nothing. A bank run can take several deposits when the party holds more coins than one
   character can carry, as it may after a long script.
4. If the members' gear lists want things (*§15.6*), the banker withdraws what they'll
   cost at its prices, plus `banker_keeps`.
5. `bankbook`, for the panel's banked total.

A member not in the bank room keeps its coins until the next run. Its card says so.

### 15.6 Shopping

The wants are each member's `gear:` (#55). An item missing from its inventory and
equipment sheets is wanted. When the banker is in a shop room:

1. The banker sends `list`. The rows give each item's price at the banker's charm, and
   the panel keeps them per shop.
2. For each wanted item this shop sells, cheapest want first, the banker buys it, if it
   can pay.
3. It gives the item to its owner if the owner is in the room. Otherwise the banker keeps
   it until they next share a room, and the panel shows it as held for that member.
4. The owner puts it on, as the gear errand does with an item carried but not in use
   (`wield`, `wear`, `light`).
5. An item it can't afford is said on the panel, with its price and what's in the bank.

Like a bank run, shopping starts by itself when the banker enters a shop that sells
something wanted, or from the panel's **Shop** button, or `@shop`. A routine bot's own
`gear` rule (*§14.7*) keeps working for a bot in no party. A bot in a party with a banker
asks the banker instead, rather than buy at its own charm.

**Selling**, from an item's *sell* action: the member gives it to the banker, who sells
it at the next shop that buys it (`sell <item>`), and the coins join the party's.
Selling loot by itself waits for a rule saying what to keep.

### 15.7 Not decided

- **Picking up coins after a kill** (#102). Nothing does yet, and this design leaves it out.
  Options: the banker if it's in the room, else whoever is; or the leader only. Until then
  coins on the floor are picked up by hand.
- **Walking the party to town.** In 1.1 the leader does it. The 1.2 director (#59) could
  send the party to a bank or a shop by itself, when the banker is full or wants are
  waiting.
- **Which bank:** books are kept per bank. A party that banks in two towns has two
  balances. `bank_at` names one; without it the panel shows each.

### 15.8 Build order

1. The sheets (#10): status, inventory, wealth and encumbrance, and the sheet page,
   view only.
2. The party file and the banker (`highest_charm` from the sheets), with the party panel
   (#97).
3. Giving to the banker (#98).
4. Bank runs (#99).
5. Shopping by the banker, and selling (#100).
6. The sheet's item actions (#101).

They're web interface work, in 1.1 with #95's. Picking up coins is #102 (Later).
