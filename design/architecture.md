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
| Passwords | A **separate `secrets.yaml`**, git-ignored, in plain text. |
| Staleness ("dirty") | All three kinds: **config changed**, **game data changed**, **party state too old**. |
| Web login | **None in step 1.** The page is bound to loopback only. |
| Injection while typing | **Queued** until the user's typed line has gone out and its prompt has come back. The page shows the command as queued. |
| Low-HP escape | **Not in step 1.** |
| Logging | **Everything, always**, with no levels: wire capture, transcript, decisions, events and server, rolled at 1 MB, the last 2 files plain and 3 zipped per stream (all settings in `mudparty.toml`). Snapshots from the web page and `#report`. Logs are read from disk; a web log viewer is on the TODO list (*§9*, *§13*). |
| Testing | Unit tests; every parser pattern keyed to silvermere's `catalogue.toml`; replayed captures checked against decision snapshots; and **whole-system tests of the mudparty server against an in-process silvermere** (`silvermere-testkit`, a git dependency at a pinned commit). Path translation is tested on Winterhawk's `.mp` pack (*§10*). |
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
- The proxy never answers a negotiation itself, so it never has to agree with either side.
  Window-size reports (NAWS) pass through, which keeps silvermere's 80×24 default honest.

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
| Headless bot | a tokio task inside the server, one per character | Its own telnet link to silvermere: login, parsing, combat, resting, @ commands. No terminal. Started from the web page, the CLI or `autostart`. |
| `mudparty-client` | one process per character you want a terminal for | The same engine as a headless bot, with the raw pass-through terminal on top (*§6.7*). |
| MudPlay | one (the user) | The main character, through the proxy. |
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
| `mp-telnet` | IAC codec, option state, NAWS, 0xFF escaping. A **pass-through mode** that decodes without ever answering, for the tap. | client, proxy |
| `mp-text` | Line extraction from the byte stream: ANSI stripping that keeps the raw bytes, backspace folding (stock direction words carry a filler byte and a backspace), the `ESC[79D ESC[K` clear-line prefix, prompt recognition for lines that end without CR/LF. | client, server |
| `mp-parse` | The realm profile: prompt/statline matcher, message catalogue (combat, rest, telepath, party, room display), classified events. silvermere is the only profile in step 1. | client, server |
| `mp-data` | The game-data SQLite schema, read API, the importer from silvermere's DB. | client (read-only), server |
| `mp-config` | YAML schemas (serde), validation, revision hashes. | client, server |
| `mp-paths` | MegaMud `.mp` / `Rooms.md` decoding, encoding and the room hash, plus the YAML path model. | server |
| `mp-protocol` | Client↔server and UI↔server messages (serde JSON), versioned. | client, server |
| `mp-engine` | Automation: login, combat, rest, remote commands, party join. Pure logic over events in and commands out, with an injected clock, so it can be tested on replayed byte streams. Also the **bot runner**: one telnet link plus one engine, talking to a hub through a channel. | client, server (headless bots) |
| `mudparty-client` (bin) | Terminal I/O and the WebSocket around a bot runner. | |
| `mudparty-server` (bin) | axum, proxy, hubs, the headless bots, file watching, the CLI subcommands (`import-gamedata`, `paths import/export`, `bot start/stop`). | |

Stack: tokio, axum (HTTP and WebSocket), rusqlite (bundled SQLite), notify (file watching),
clap, tracing, regex. YAML through a maintained serde YAML crate, picked at implementation
time (`serde_yaml` is archived).

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
- **The main character's statline belongs to MudPlay.** The tap reads it with a template
  set in the main character's YAML (default: MudPlay's default shape). The proxy never
  changes the user's statline.
- **Pacing.** Commands go out on a time floor, with the prompt as a gate on top of it:
  at most a few unanswered commands at once, and one more per prompt. A
  `You are typing too quickly - command ignored` line means the last command was lost
  (*GAME_MECHANICS: Wire → Command rate limit*). Telepaths keep a 100 ms floor, and each
  `--- Telepath Sent to <Name> ---` or `--- Telepath Not Sent ---` acknowledgement is
  paired with its send in order. A telepath that wasn't sent is sent again, up to 3 tries.

### 6.2 Auto login

A small state machine. Its steps are data in the character YAML, defaulting to silvermere's
dialogue (`bin/silvermere/tests/login_e2e.rs`, `docs/auth.md`):

| Wait for | Send |
|---|---|
| `Username:` | the account name |
| `Password:` | the password from `secrets.yaml` |
| `(N)onstop, (Q)uit, or (C)ontinue?` (any time, as a pager) | Enter |
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

- **The room's monsters** come from the room display (`Also here:`), from arrival and
  departure lines, and from death lines.
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

---

## 7. Data model

### 7.1 Files

```
<data root>/                        default: %AppData%\mudparty
  mudparty.toml                     server config (TOML; everything else is YAML)
  secrets.yaml                      passwords, git-ignored
  characters/<name>.yaml            one per bot character, plus the main character's
  paths/<group>/<name>.yaml         paths and loops (stored now, walked in a later step)
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
port = 23

[listen]
client_hub = "127.0.0.1:7171"          # /ws/client, /ws/ui and the web page
proxy      = "127.0.0.1:2323"          # MudPlay connects here

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
party:
  leader: Raijin
  invite_by_injection: true
remote:
  allow: [Raijin]
  channels: [telepath, gangpath]
  warn_denied: false
automation: { combat: true, nuke: true, rest: true }
combat: { ... }                               # §6.4
rest: { ... }                                 # §6.6
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

- `mp-telnet`: negotiation, IAC escaping, NAWS. **Pass-through mode never writes a byte.**
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

`silvermere-testkit` is a dev-dependency from `jyurkiw/silvermere` at a pinned commit. Its
`TestServer` runs a real silvermere inside the test on the imported stock world, with
helpers to create characters, place them in rooms and read back their records.
`start_manual` stops the real-time clock, and `tick` drives combat rounds by hand, so a
5-second round takes no time. The arena practice dummy in room 1/206 (9999 HP, no exp)
gives fights that don't end.

The **mudparty server itself is under test**: a test starts a silvermere `TestServer`,
then a mudparty server in the same process, pointed at it, with a temporary data root
(`mudparty.toml`, `secrets.yaml`, character YAML, logs). It then plays parts against it:

| Part | Played by |
|---|---|
| Headless bots | the server's own bot tasks, started through the CLI or a UI `order` |
| An interactive client | `mudparty-client`'s runner, driven from the test over the WebSocket |
| MudPlay | a testkit `TelnetClient` connected to the **proxy** port, typing as the main character |
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
8. **Proxy transparency:** the bytes the MudPlay stand-in receives equal what silvermere
   sent, negotiation included, and the other way round. Injected commands wait for the end
   of a half-typed line.
9. **Staleness:** edit a character's YAML mid-fight. It applies at the safe point, the ack
   clears the page's pending badge, and a bad edit keeps the old config with an error on
   the page. Re-import game data and check the rev change. Stop a bot's feed and watch its
   values go stale.
10. **Shutdown:** an orderly stop logs every bot off (silvermere's records show them out of
    the realm). Aborting the server task shows the hang-up path.
11. **Logging:** each scenario's logs are present, rolled and redacted, with no password
    bytes anywhere under the data root.

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
6. **The proxy and the tap**, then injection.
7. **The web page.**
8. **Path translation** and its CLI.

Each step ends with its tests from *§10*: unit, catalogue and replay tests, and from step 3
on the whole-system scenarios on an in-process silvermere. Then a manual run against the
user's server.

---

## 13. TODO (after step 1)

- **A log-file web interface**: browse, tail and filter each character's streams, and open
  snapshots, from the web page.
- A low-HP escape (`flee_below`, hang-up).
- The Avenger's `VANISH`.
- Walking paths and loops (they are stored and translated in step 1, not walked).
- Live `/room` checks for path import ambiguity.
- Other realms (stock boards, Paradigm) as realm profiles, and the `.mdb` game-data source.
- A web login token, for opening the page beyond loopback.
