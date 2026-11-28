//! `mudparty-server check <file>` (#66, design §14.9) on the stock world's Newhaven: a file
//! printed as the server reads it, defaults filled in and places resolved to rooms, and
//! what would stop it, by file and field: a place it can't walk to keeping to the
//! routine's rules and the character's level, and a room by number whose name has changed.

use std::fs;
use std::path::Path;

use mp_config::CharacterConfig;
use mp_silvermere::stock_template;
use mudparty_server::{Checked, check, compile_places};

const ARENA: &str = "schema: 1
name: newhaven-arena
places: newhaven
params:
  rest_below: 50%
do:
  - gear
  - rest: { at: road, when: { hp_below: \"${rest_below}\" } }
  - circuit: { lairs: [dungeon:small-cavern-2156] }
  - camp: { at: arena }
";

const LOCAL: &str = "schema: 1
area: newhaven
aliases:
  arena: arena-2150
  road: narrow-road-2146
";

const ERMIAS: &str = "schema: 1
name: Ermias
login:
  account: Ermias
gear:
  - { item: club, use: wield }
  - { item: padded vest, use: wear }
routine:
  use: newhaven-arena
  with: { rest_below: 60% }
";

/// A data root on the stock world, its Newhaven files written, its rooms compiled.
fn root(routine: &str, character: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    let db = stock_template().display().to_string().replace('\\', "/");
    fs::write(
        r.join("mudparty.toml"),
        format!("schema = 1\n[game]\nhost = \"127.0.0.1\"\nport = 2323\n[gamedata]\nsilvermere_db = \"{db}\"\n"),
    )
    .unwrap();
    compile_places(r, None).unwrap();
    fs::write(r.join("places/newhaven.local.yaml"), LOCAL).unwrap();
    fs::create_dir_all(r.join("routines")).unwrap();
    fs::write(r.join("routines/newhaven-arena.yaml"), routine).unwrap();
    fs::create_dir_all(r.join("characters")).unwrap();
    fs::write(r.join("characters/Ermias.yaml"), character).unwrap();
    dir
}

fn run(r: &Path, file: &str, level: Option<u32>) -> Checked {
    check(r, &r.join(file), level).unwrap()
}

#[test]
fn the_newhaven_files_print_as_the_server_reads_them() {
    let dir = root(ARENA, ERMIAS);
    let c = run(dir.path(), "characters/Ermias.yaml", Some(1));
    assert_eq!(c.problems, Vec::<String>::new(), "{}", c.text);
    let t = &c.text;
    // Places resolved to rooms, by number and name.
    for want in [
        "room: 1/2146",
        "name: Newhaven, Narrow Road",
        "room: 1/2156",
        "room: 1/2150",
        "name: Newhaven, Arena",
    ] {
        assert!(t.contains(want), "{want:?} in:\n{t}");
    }
    // Defaults and parameters filled in: the camp's look_every, the rest's until (the
    // character's own), the routine's rest_below as the character sets it.
    for want in ["look_every: 5s", "hp_below: 60%", "until:", "statline:"] {
        assert!(t.contains(want), "{want:?} in:\n{t}");
    }
    // The cavern's respawn time, from the room file, keyed by its room number.
    assert!(t.contains("1/2156: 2m"), "{t}");
    // Where each item would be bought.
    assert!(t.contains("club") && t.contains("1/2141"), "{t}");
    assert!(t.contains("padded vest") && t.contains("1/2142"), "{t}");
    // What it prints is itself a character file, its routine written inline.
    let back = CharacterConfig::from_yaml(t).unwrap();
    assert!(matches!(
        back.routine,
        Some(mp_config::RoutineRef::Inline(_))
    ));
}

#[test]
fn a_routine_checks_by_itself() {
    let dir = root(ARENA, ERMIAS);
    let c = run(dir.path(), "routines/newhaven-arena.yaml", None);
    assert_eq!(c.problems, Vec::<String>::new(), "{}", c.text);
    assert!(c.text.contains("hp_below: 50%"), "{}", c.text);
    assert!(c.text.contains("room: 1/2150"), "{}", c.text);
}

#[test]
fn a_place_it_cant_walk_to_is_said_by_file_and_field() {
    let routine = ARENA
        .replace(
            "do:\n",
            "avoid:\n  rooms: [narrow-path]\ndo:\n  - rest: { at: village-entrance, when: { mp_below: 10% } }\n",
        )
        .replace("at: road", "at: nowhere-at-all");
    let dir = root(&routine, ERMIAS);
    let c = run(dir.path(), "characters/Ermias.yaml", None);
    let p = c.problems.join("\n");
    // The rest the avoided room cuts off, and the place no file names.
    assert!(
        c.problems.iter().any(
            |l| l.starts_with("routines/newhaven-arena.yaml: do[0].at: ")
                && l.contains("Narrow Path")
        ),
        "{p}"
    );
    assert!(
        c.problems.iter().any(
            |l| l.starts_with("routines/newhaven-arena.yaml: do[2].at: ")
                && l.contains("nowhere-at-all")
        ),
        "{p}"
    );
    // The shops are past the avoided room too.
    assert!(
        c.problems
            .iter()
            .any(|l| l.starts_with("characters/Ermias.yaml: gear[0]: ") && l.contains("club")),
        "{p}"
    );
}

#[test]
fn a_place_past_a_level_exit_is_checked_at_the_level_given() {
    let routine = ARENA.replace("at: road", "at: healer");
    let dir = root(&routine, ERMIAS);
    // The Healer is west of the Narrow Road through an exit for levels 1 to 3.
    assert_eq!(
        run(dir.path(), "characters/Ermias.yaml", Some(3)).problems,
        Vec::<String>::new()
    );
    let c = run(dir.path(), "characters/Ermias.yaml", Some(5));
    assert!(
        c.problems.iter().any(
            |l| l.starts_with("routines/newhaven-arena.yaml: do[1].at: ") && l.contains("level 5")
        ),
        "{:?}",
        c.problems
    );
}

#[test]
fn a_room_whose_name_has_changed_is_said() {
    let routine = ARENA.replace(
        "at: arena }",
        "at: { room: 1/2150, name: \"Newhaven, Pit\" } }",
    );
    let dir = root(&routine, ERMIAS);
    let c = run(dir.path(), "characters/Ermias.yaml", None);
    assert_eq!(c.problems.len(), 1, "{:?}", c.problems);
    let p = &c.problems[0];
    assert!(
        p.starts_with("routines/newhaven-arena.yaml: do[3].at: "),
        "{p}"
    );
    assert!(
        p.contains("Newhaven, Pit") && p.contains("Newhaven, Arena"),
        "{p}"
    );
}

#[test]
fn a_lair_with_no_time_or_nowhere_to_wait_is_said() {
    // The arena isn't a lair, so how long it takes to fill isn't known.
    let routine = ARENA.replace("lairs: [dungeon:small-cavern-2156]", "lairs: [arena]");
    let dir = root(&routine, ERMIAS);
    let c = run(dir.path(), "characters/Ermias.yaml", None);
    assert!(
        c.problems.iter().any(
            |l| l.starts_with("routines/newhaven-arena.yaml: do[2].lairs[0]: ")
                && l.contains("isn't a lair")
        ),
        "{:?}",
        c.problems
    );
    // Given one, it checks.
    let routine = ARENA.replace(
        "lairs: [dungeon:small-cavern-2156]",
        "lairs: [arena], respawn: { arena: 3m }",
    );
    let dir = root(&routine, ERMIAS);
    let c = run(dir.path(), "characters/Ermias.yaml", None);
    assert_eq!(c.problems, Vec::<String>::new());
    // The bot waits for the cavern in the arena, past the Dungeon Entrance (a lair of rats
    // itself); with the arena avoided there's nowhere to wait.
    let routine = ARENA
        .replace("do:\n", "avoid:\n  rooms: [arena]\ndo:\n")
        .replace("  - camp: { at: arena }\n", "  - camp: { at: road }\n");
    let dir = root(&routine, ERMIAS);
    let c = run(dir.path(), "characters/Ermias.yaml", None);
    assert!(
        c.problems.iter().any(
            |l| l.starts_with("routines/newhaven-arena.yaml: do[2].lairs[0]: ")
                && l.contains("nowhere to wait")
        ),
        "{:?}",
        c.problems
    );
}

#[test]
fn a_file_that_doesnt_read_is_one_problem_naming_it() {
    let dir = root(ARENA, "schema: 1\nname: Ermias\nrset: {}\n");
    let c = run(dir.path(), "characters/Ermias.yaml", None);
    assert_eq!(c.problems.len(), 1, "{:?}", c.problems);
    assert!(
        c.problems[0].starts_with("characters/Ermias.yaml: ") && c.problems[0].contains("rset"),
        "{:?}",
        c.problems
    );
}

#[test]
fn the_local_file_prints_each_alias_as_its_room() {
    let dir = root(ARENA, ERMIAS);
    let c = run(dir.path(), "places/newhaven.local.yaml", None);
    assert_eq!(c.problems, Vec::<String>::new());
    assert!(
        c.text.contains("arena: 1/2150  # Newhaven, Arena"),
        "{}",
        c.text
    );
    assert!(c.text.contains("road: 1/2146"), "{}", c.text);
    // It's still a local file.
    serde_norway::from_str::<mp_data::places::Local>(&c.text).unwrap();
}

#[test]
fn a_train_rule_needs_a_trainer_it_can_walk_to() {
    // No trainer named: the Adventurer's Guild trains any class.
    let routine = ARENA.replace("  - gear\n", "  - gear\n  - train: { up_to: 3 }\n");
    let dir = root(&routine, ERMIAS);
    let c = run(dir.path(), "characters/Ermias.yaml", None);
    assert_eq!(c.problems, Vec::<String>::new());
    assert!(c.text.contains("up_to: 3"), "{}", c.text);
    // The guild is north of the Narrow Road; avoided, there's no way to it.
    let routine = routine.replace("do:\n", "avoid:\n  rooms: [road]\ndo:\n");
    let dir = root(&routine, ERMIAS);
    let c = run(dir.path(), "characters/Ermias.yaml", None);
    assert!(
        c.problems
            .iter()
            .any(|l| l.starts_with("routines/newhaven-arena.yaml: do[1]: ")
                && l.contains("no trainer can be walked to")),
        "{:?}",
        c.problems
    );
}

// A cast code the game data hasn't would never go out (#125): each spell a file names, in its
// level blocks and its tiers, is looked up.
#[test]
fn a_spell_the_game_data_hasn_t_is_said() {
    let mage = format!(
        "{ERMIAS}combat:\n  by_level:\n    - levels: 1-\n      spells:\n        - cast: mmis\n        - cast: zzap\n  tiers:\n    - {{ spell: blarg, min_mp: 1 }}\n"
    );
    let dir = root(ARENA, &mage);
    let c = run(dir.path(), "characters/Ermias.yaml", Some(1));
    assert_eq!(
        c.problems,
        [
            "characters/Ermias.yaml: combat.by_level[0].spells[1].cast: no spell `zzap` in the game data",
            "characters/Ermias.yaml: combat.tiers[0].spell: no spell `blarg` in the game data",
        ],
        "{}",
        c.text
    );
}
