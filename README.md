# mudparty

A party client and server for MajorMUD on [silvermere](https://github.com/jyurkiw/silvermere).
You play your main character in MudPlay. mudparty runs the rest of the party as headless
bots, each with its own link to the game. The bots log in, join your party, fight with spell
tiers, rest and tell you to wait, and answer MegaMud-style @ commands. A web page shows
the whole party and lets you order the bots around.

The design is in [design/architecture.md](design/architecture.md), and progress is tracked
in this repository's issues.

## Status

Headless bots, combat, resting, party join, @ commands, logs and the web page work, and
MudPlay can play through the proxy. Still to come: `#` commands and typing from the web
page through the proxy, the interactive terminal client, character sheets and path
translation. A few settings for those already read in `mudparty.toml` (below), but nothing
uses them yet.

## Building

You need Rust. [rust-toolchain.toml](rust-toolchain.toml) pins the version silvermere
builds with (1.100.0), and `rustup` fetches it for you.

```powershell
cargo build --release
```

The server is `target/release/mudparty-server`.

## The data root

The server keeps everything in one folder, the *data root*:

```
<data root>/
  mudparty.toml          the server's config (below)
  secrets.yaml           account passwords
  characters/
    <Name>.yaml          one file per character, named after it
  routines/
    <name>.yaml          what a solo bot does, shared between characters
  places/
    <area>.yaml          the compiled room files (places compile)
    <area>.local.yaml    your own names for rooms
  gamedata/
    gamedata.sqlite      imported from silvermere's database on first start
  logs/
    server.jsonl         what the server did
    <Name>/              each bot's wire, transcript, decisions and events
```

You write `mudparty.toml`, `secrets.yaml` and the character files. The server makes the
rest.

## `mudparty.toml`

The server's config is TOML. The server reads it once at start, so restart the server
after changing it. A setting the server doesn't know is an error, not ignored, so a typo
can't pass unnoticed.

### The smallest file that works

Three things are required: the schema, where the game is, and where silvermere's
database is.

```toml
schema = 1

[game]
host = "127.0.0.1"
port = 2323

[gamedata]
silvermere_db = "E:/silvermere-data/silvermere.db"
```

Everything else has a default.

### Every setting

This file sets everything. All but the first three values are the defaults, so a line you
leave out means what it says here.

```toml
schema = 1                        # required; this mudparty reads schema 1
main_character = "Raijin"         # the character you play in MudPlay
logoff_timeout = "30s"            # how long each bot gets to log off when the server stops

[game]                            # required: the silvermere server
host = "127.0.0.1"
port = 2323

[listen]
client_hub = "127.0.0.1:7171"     # the web page and its feed
proxy = "127.0.0.1:2424"          # where MudPlay connects

[gamedata]                        # required
silvermere_db = "E:/silvermere-data/silvermere.db"

[stale]
vitals = "15s"                    # not used yet
room = "30s"                      # not used yet
target = "6s"                     # how old the leader's target may be and still be assisted

[logging]
file_size = "1MB"                 # roll a log file at this size
plain_files = 2                   # the live file and the one before it
zip_archives = 3                  # older files, zipped; the oldest is deleted
```

### The settings, one by one

**Top level**

| Setting | Default | Meaning |
|---|---|---|
| `schema` | required | The file format. Must be `1`. |
| `main_character` | none | The character you play yourself in MudPlay. Its card comes first on the web page and says it is played in MudPlay. Give it a character file too (with no `login`, so it never runs as a bot). |
| `logoff_timeout` | `"30s"` | When the server stops, each bot breaks off, sends `quit` and waits for the realm exit. After this long it drops the link instead. The game won't let a character quit mid-fight, so a bot that is fighting may need all of it. |

**`[game]`**: the silvermere server the bots connect to. Required.

| Setting | Meaning |
|---|---|
| `host` | Its address, such as `"127.0.0.1"` or a host name. |
| `port` | Its telnet port: silvermere's default is `2323`. |

**`[listen]`**: where mudparty listens.

| Setting | Default | Meaning |
|---|---|---|
| `client_hub` | `"127.0.0.1:7171"` | The web page, at `http://<client_hub>/`. The page has no login, so this must be a loopback address (`127.0.0.1` or `[::1]`); the server won't start otherwise. |
| `proxy` | `"127.0.0.1:2424"` | Where MudPlay connects: point MudPlay's connection here instead of at silvermere. Loopback only, like the page. |

The two must use different ports. Neither may use a port silvermere already listens on:
`2323`, `8080` and `8088` on a default silvermere.

**`[gamedata]`**: required.

| Setting | Meaning |
|---|---|
| `silvermere_db` | silvermere's SQLite database. A relative path is read from the data root. |

On first start the server imports rooms, exits, spells, classes and monsters from it into
`gamedata/gamedata.sqlite`, and reads only that copy afterwards. To import again after the
world changes, stop the server, delete `gamedata/gamedata.sqlite` and start it again.
Without game data, bots still log in but don't fight: a bot needs the game's monster names
to tell a monster from a player.

**`[stale]`**: when party information counts as too old to act on. Each is a duration.

| Setting | Default | Meaning |
|---|---|---|
| `target` | `"6s"` | A bot with `assist_leader` attacks what the leader last attacked, but only if the leader did so this recently. |
| `vitals` | `"15s"` | Not used yet. |
| `room` | `"30s"` | Not used yet. |

**`[logging]`**: how log files roll.

| Setting | Default | Meaning |
|---|---|---|
| `file_size` | `"1MB"` | A file rolls before a record would take it past this size. |
| `plain_files` | `2` | Plain files kept, the live one included. At least `1`. |
| `zip_archives` | `3` | Rolled files kept zipped after those. The oldest is deleted. |

Each log stream can override any of the three in its own table. `[logging.server]` is
the server's own log, `logs/server.jsonl`. Every bot has four streams in `logs/<Name>/`:
`[logging.wire]` (the raw bytes, both ways), `[logging.transcript]` (the game's text as a
player would read it), `[logging.decisions]` (what the bot decided and why) and
`[logging.events]` (connecting, logging in, the party, rests, switches, going
offline). For example, to keep more of the server log and less of the bulky wire capture:

```toml
schema = 1

[game]
host = "127.0.0.1"
port = 2323

[gamedata]
silvermere_db = "silvermere.db"

[logging.server]
file_size = "4MB"
zip_archives = 10

[logging.wire]
zip_archives = 1
```

**Values**

- A **duration** is a number and a unit, in quotes: `"250ms"`, `"30s"`, `"5m"` or `"1h"`.
- A **size** is `"512KB"` or `"1MB"` (powers of 1024), or a plain number of bytes.
- A **path** may use forward slashes on Windows too: `"E:/silvermere-data/silvermere.db"`.

## `secrets.yaml`

Each account's password, by account name. It is kept apart so you can share the rest of
the data root without it, and the server never writes a password to a log.

```yaml
accounts:
  Ermias: "Ermias's password"
  Kaelen: "Kaelen's password"
```

## Character files

One YAML file per character in `characters/`, named after it (`characters/Ermias.yaml`).
A file with a `login` can run as a bot. A short one:

```yaml
schema: 1
name: Ermias
class: Mage
autostart: true
login:
  account: Ermias
party:
  leader: Raijin
remote:
  allow: [Raijin]
combat:
  targeting:
    - assist_leader: true
    - fight_back: true
  tiers:
    - spell: mmis
      min_mp: 3
  melee: a
rest:
  start_when_any: { hp: 50%, mp: 20% }
```

The main character's file needs only a name:

```yaml
schema: 1
name: Raijin
class: Warrior
```

A bot sets its own statline (the file's `statline`) so it can read its HP and MP, but
it leaves the character as it found it. Before changing anything it reads the character's
statline with `pro` and keeps it in `mudparty.sqlite`, and it puts it back when it logs off
cleanly. If the character already has the bot's statline, it sends nothing. After a crash
the saved statline stays, and goes back at the next clean logoff. `statline_restore: false`
leaves the bot's statline on. One limit: a custom statline comes back as `full custom`,
since `pro` doesn't say whether it was `custom` or `full custom`.

A bot logs in by silvermere's own dialogue. A game with a different one is written in
`login.steps`, each step waiting for some text and then sending a line, with `{account}`
and `{password}` filled in (the password from `secrets.yaml`, never logged):

```yaml
schema: 1
name: Ermias
login:
  account: Ermias
  steps:
    - { wait: "Username:", send: "{account}" }
    - { wait: "Password:", send: "{password}" }
    - { wait: "Make your selection (X to exit):", send: "M" }
    - { wait: "[MAJORMUD]:", send: "E" }
```

A step's text may come in a finished line or in a line still waiting for an answer. The
bot is in the game at the first prompt after the last step. The game's refusals (a wrong
password, a locked account) stop the login as usual, and so does a second request for the
name or the password once it's been sent.

### A routine

A bot with a `routine:` runs a loop of its own, with no one leading it. A routine is a
file in `routines/`, shared by any character that names it: a list of rules read from the
top, where the first that applies wins and the last is what the bot does when nothing else
is due. Newhaven's arena, with its cave bear, as `routines/newhaven-arena.yaml`:

```yaml
schema: 1
name: newhaven-arena
places: newhaven             # places are looked for in this area first
params:                      # a character may change these; used as ${name}
  rest_below: 50%
  bear_hp: 80%
avoid:                       # never walked through; rooms are places, as everywhere
  rooms: [healer]
  exits: [trap]              # exit kinds, as the room files name them
do:
  - rest:                    # below the line: walk to the road and rest
      at: road
      when: { hp_below: "${rest_below}" }
      until: { hp: 100% }    # the character's rest.stop_when_all when left out
  - circuit:                 # when a lair is ready and HP allows: the bear's cavern
      lairs: [bear]          # an alias of dungeon:small-cavern-2156
      respawn: { bear: 4m }  # how long after its last kill it fills; the room file's if left out
      when: { hp_at_least: "${bear_hp}" }
  - camp:                    # otherwise: stay in the arena and fight what comes
      at: arena
      look_every: 5s         # the default
```

The rules are `rest`, `circuit`, `camp`, `gear` and `train` (below). A rule's `when` (`hp_below`,
`mp_below`, `hp_at_least`, `mp_at_least`) says when it starts; once started it runs until
it's done, unless a rule above it starts, which takes over once no fight is going on.
`road`, `arena` and `bear` are aliases in `places/newhaven.local.yaml` (see
[Places](#places)), since Newhaven has two of each.

A `circuit` visits lairs as each fills again. A lair's clock starts at its last kill, and
it's ready its `respawn` later; one not seen yet is ready now. The circuit applies when a
lair will be ready by the time the bot gets there. It takes the nearest such lair, walks to
the room next door (a lair fills when it's entered, so the bot never steps in early), goes
in when it's ready, and kills everything there. Then it plans again from where it stands,
and when no lair is ready the rules below run. A lair's clock is kept in `mudparty.sqlite`,
so it survives a restart, and a lair cut short by a rest is still ready afterwards.

- `wait: 1m` lets the bot wait next door for a lair that will be ready within a minute,
  instead of giving way to the rules below; with none ready on arrival, it waits for the
  one whose fight can start soonest.
- `pick: throughput` takes the nearest lair, counting time spent waiting as free.
- `respawn` is keyed by the lair as `lairs:` writes it (by room number, `1/2156`, for the
  long form). A lair left out takes the compiled room file's time; silvermere counts it in
  whole minutes, so a minute more may be needed.

A character names the routine, and may change its parameters:

```yaml
schema: 1
name: Ermias
login:
  account: Ermias
combat:
  targeting: [kill_order, { priority: ["*"] }]   # a rule with no value may be a bare word
  melee: a
routine:
  use: newhaven-arena        # or just `routine: newhaven-arena`
  with: { rest_below: 60% }
gear:                        # what it carries, kept by the routine's gear rule
  - { item: club, use: wield }
  - { item: padded vest, use: wear }
  - { item: padded pants, use: wear, at: armour-shop }   # at: a shop of your choosing
```

A one-off routine may be written in the character's file instead, as `routine: { do: [...] }`.

A `gear` rule (`- gear`, or `- gear: { when: {...} }`) keeps the character's `gear:`. When
the routine starts and after each rest, it looks at the inventory. An item carried but not
in use is put on where the bot stands. A missing one is bought at a shop that sells it, as
the compiled room files say (those in the routine's `places` area first), and put on; then
it looks again. An item it can't get (no money, can't use it, no shop sells it, no way
there) is skipped and noted on the card, and not tried again until the bot restarts. Put
`gear` at the top of a new character's routine, so it gears up before it fights:

```yaml
schema: 1
name: newhaven-start
places: newhaven
do:
  - gear
  - train: { up_to: 3 }      # the arena takes levels 1 to 3
  - camp: { at: arena }
```

A `train` rule (`- train`, or `- train: { at: guild, up_to: 3 }`) trains a level whenever
the bot has the experience for it. The routine asks the game with `exp` and counts what's
gained after. With enough for the next level, and the next no higher than `up_to`, the bot
walks to a trainer and sends `train`. It then waits there for the new level's experience,
and trains again or goes on. Without `at:` it trains at the first of the compiled room
files' trainers for the character's `class:` (or any class), those in the routine's
`places` area first. Newhaven's Adventurer's Guild trains any class, for 5 silver a level.
If it can't pay (or the game says the experience isn't there), that's noted on the card
and tried again in 5 minutes. A trainer that won't train the bot (another class's, or a
level outside its own) gives way to the next; with none left, training stops until the bot
restarts. Put `up_to` at an arena's top level, since a level above it locks the bot out.
Once the level is known, walks keep out of level exits that won't let the bot through.

The card shows where the routine is: camping, going to rest, resting, going to a lair,
waiting for one to fill, killing, going to train, training. `@stop` (or
the page's Pause) pauses it with the rest of the bot, and `@rego` (Go on) picks it up where
it was. While it runs, the page's Go box is refused, since the routine is walking the bot.

Section 7.3 of [the design](design/architecture.md#73-charactersnameyaml) lists every
setting. The server watches `characters/`, and a running bot picks up an edited file
without restarting.

## Places

`mudparty-server places compile` writes `places/<area>.yaml` from the silvermere database
`mudparty.toml` names, one file per area (`--area newhaven` for one). The files list each
room's key, name, exits, spawns, lairs and shops, and scripts name rooms by them. Don't edit
them: recompiling replaces them. Your own names for rooms go in `places/<area>.local.yaml`
beside each, which recompiling never touches:

```yaml
schema: 1
area: newhaven
aliases:
  arena: 1/2150            # a key, a slug, or the game's name while it's unique
  road: narrow-road-2146
  bear: dungeon:small-cavern-2156
```

A room is named by an alias, its slug (`weapons-shop`, or `dungeon:entrance` in another
area), the game's own name while only one room has it (`Newhaven, Weapons Shop`), or its key
(`1/2141`). A name that could be several rooms is an error that lists them.

Wherever a file names a place, it may name the room by number and keep its name beside it:
`at: { room: 1/2150, name: "Newhaven, Arena" }`. The number is what's used. The name is for
you and for `check` (below), which says when the world's name for that room has changed, as
it would if the world were renumbered. Section 14 of
[the design](design/architecture.md#14-scripts-places-routes-routines-and-the-map-designed-2026-11-26)
has the rest.

## Writing the files by hand

**Schemas.** `schemas/` holds a JSON Schema for each file you write in YAML:
`character.schema.json`, `routine.schema.json` and `places-local.schema.json`. With them an
editor that has a YAML extension (VS Code's Red Hat YAML, for one) completes keys, shows
each setting's description, and marks mistakes as you type. Point a file to its schema with
a first line such as `# yaml-language-server: $schema=../schemas/routine.schema.json`, or
point whole folders to them in the editor's settings:

```json
"yaml.schemas": {
  "E:/Repos/mudparty/schemas/character.schema.json": "characters/*.yaml",
  "E:/Repos/mudparty/schemas/routine.schema.json": "routines/*.yaml",
  "E:/Repos/mudparty/schemas/places-local.schema.json": "places/*.local.yaml"
}
```

`mudparty-server schema --out <folder>` writes them again, from the version of mudparty you
run. A schema checks how a file is written. What it means is checked when it's read, and
against the world by `check`.

**`check`.** `mudparty-server check <file> [--level <n>] [--root <data root>]` reads a
character's, a routine's or a places local file as the server would, and prints it with
everything filled in:

- every setting, defaults too;
- a routine's parameters;
- each place as the room it names, in the long form;
- where each item of gear would be bought, in comments at the end.

A character's routine is printed inline, so what's printed is itself a file that reads the
same. Then `check` says, by file and field, what would stop the bot, and exits 1 if
anything would:

```text
routines/newhaven-arena.yaml: do[1].at: every way from Newhaven, Arena (1/2150) to Newhaven, Healer (1/2190) is barred: the shortest goes west from Newhaven, Narrow Road (1/2146) by an exit for levels 1 to 3, and the character is level 5
characters/Ermias.yaml: gear[0]: no shop selling "club" can be walked to: ...
```

It checks three things:

- **Every place can be walked to and back** from the place of the routine's last rule,
  keeping to its `avoid` and `prefer_not`. With a gear rule, so can a shop for each item.
- **Level exits.** With `--level`, a walk through a level exit that won't let the character
  through counts as no walk. Without it, the game decides on the way.
- **Rooms by number** whose names have changed.

`check` opens the game data as the server does, importing it from `silvermere.db` first if
`gamedata/` hasn't got it yet.

## Running

```powershell
mudparty-server --root E:/mudparty-data
```

`--root` defaults to the current folder. The server starts every bot whose file says
`autostart: true` and prints the web page's and the proxy's addresses. Ctrl-C logs every
character off cleanly before it exits, MudPlay's included.

### Playing through the proxy

Point MudPlay at the proxy (`listen.proxy`, `127.0.0.1:2424` by default) instead of at
silvermere, and log in as usual. MudPlay can't tell the difference: the proxy offers the
same telnet options silvermere does and passes every byte through unchanged. The account
name you type says which character this is: the one whose file has that `login.account`,
else `main_character`. Its card on the page then follows your HP, MP and room as you play,
and its logs record the session, your password left out.

In MudPlay, a line starting with `#` is for mudparty: `#help` lists the commands, and
`#who` says which characters mudparty is playing and whether MudPlay or a bot drives each.
mudparty answers them itself, and they never reach the game. Any other `#` line goes to
the game as typed.

The character's card on the page has a command line too. What you send there goes to the
game as that character, but never into the middle of a line you're typing in MudPlay: it
waits, shown as queued, until your line has gone and its prompt has come back.

A bot whose file says `party.invite_by_injection: true` doesn't telepath `@invite` to a
leader you're playing through the proxy: mudparty types `invite <bot>` for you, the same
careful way, and the bot follows. With the leader played any other way, the bot asks by
telepath as usual.

```yaml
schema: 1
name: Ermias
login:
  account: Ermias
party:
  leader: Arandmoor
  invite_by_injection: true
```

If a bot is already playing that character, the game asks MudPlay whether to take the
character over, as it would without the proxy.

**When MudPlay disconnects** mid-game, the character stays in the game, idle: its card says
so, and it takes the page's orders like a bot. It doesn't change your statline. The server
asks the game where the character is and records that in `mudparty.sqlite`, so a downed
or nearly dead character can be found and rescued; the card shows it as "last seen". If HP
then falls below the file's `unattended.hang_up_below`, the server records where again and
hangs up. When you reconnect MudPlay and log in, the game asks to take the character over;
answer yes.

```yaml
schema: 1
name: Arandmoor
unattended:
  hang_up_below: 30%    # the default; a number of points works too, or none
```

A percentage needs the maximum HP. If your statline doesn't show it, the most HP the
session has seen stands in for it, so points are the surer choice there.

On the page, each character has a card showing HP and MP, room, what it's doing and
against what, and its last few events. A bot's card has Start and Stop, Wait and Ok, the
combat, nuke, rest and all switches, a kill box, a command line to the game, and a "Go"
box that walks the bot to a room: a key like `1/2141`, or a place from the compiled room
files (`weapons-shop`; see [Places](#places)). The bot plans its route from the game data,
confirms each room as it goes, opens doors and bashes locked ones, plans round an exit the
game refuses, and waits out a fight on the way.

In the game, a player on a bot's `remote.allow` list can command it by telepath or
gangpath: `/Ermias @help` lists the commands.

## Testing

```powershell
cargo test --workspace
```

The whole-system tests run a real silvermere in-process from its stock world. The first
build takes a while, because it builds silvermere too.

## License

MIT
