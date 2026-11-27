//! `mudparty-server check <file>` (#66, design §14.9): a character, routine or places local
//! file printed as the server reads it, and what would stop it, by file and field.
//!
//! - **Printed:** defaults filled in, a routine's parameters filled in, every place resolved
//!   to its room as the long form `{ room: 1/2150, name: "Newhaven, Arena" }`. A character's
//!   routine is printed inline, its rests' `until` filled from the character's own
//!   `rest.stop_when_all`, so what's printed is itself a file that reads the same. Where each
//!   item of gear would be bought is said in comments after it.
//! - **Reachable:** every place a routine's rules name can be walked to and back from the
//!   place of its last rule (where the bot is when nothing else is due), keeping to the
//!   routine's `avoid` and `prefer_not` and, given a level, to the level exits that let the
//!   character through (*§14.5*). Between them, every place reaches every other. With a gear
//!   rule, so can a shop for each item; with a circuit, each lair and the room the bot waits
//!   in for it (#67), whose respawn time is printed as the server takes it.
//! - **Renumbered:** a room by number whose `name` isn't the world's any more.
//!
//! The game data is opened as the server opens it, imported from `silvermere.db` first when
//! `gamedata/` hasn't it yet.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Context;
use mp_config::{
    CharacterConfig, ConfigError, Place, Routine, RoutineRef, RuleSpec, load_character,
    load_routine, load_server,
};
use mp_data::GameData;
use mp_data::places::{Local, Places};
use mp_engine::RouteRules;

use crate::{circuit_lairs, data_rules, open_gamedata, place};

/// What `check` makes of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    /// The file as the server reads it, as YAML; empty when it doesn't read.
    pub text: String,
    /// What would stop it, each `file: field: why`, the file named from the data root.
    pub problems: Vec<String>,
}

/// Checks `file`, a character's, a routine's or a places local file, on the data root
/// `root`; `level`, when given, is the character's, for the level exits on its walks.
pub fn check(root: &Path, file: &Path, level: Option<u32>) -> anyhow::Result<Checked> {
    let config = load_server(&root.join("mudparty.toml"))?.value;
    let text = std::fs::read_to_string(file).with_context(|| file.display().to_string())?;
    let mut out = Problems {
        root,
        list: Vec::new(),
    };
    let places_dir = root.join("places");
    let places = if places_dir.is_dir() {
        match Places::load(&places_dir) {
            Ok(p) => p,
            Err(e) => {
                out.add(&e.path, "", &e.message);
                return Ok(out.done(String::new()));
            }
        }
    } else {
        Places::default()
    };
    let (gd, _) = open_gamedata(root, &config).map_err(|e| anyhow::anyhow!("{e}"))?;
    let world = World {
        root,
        places: &places,
        gd: &gd,
        level,
    };
    let printed = match kind(file, &text) {
        Kind::Local => world.local(&mut out, file, &text),
        Kind::Routine => world.routine_file(&mut out, file),
        Kind::Character => world.character(&mut out, file),
    };
    Ok(out.done(printed))
}

enum Kind {
    Character,
    Routine,
    Local,
}

/// What a file is: a places local file by its name or its `area:`, a routine by its `do:`,
/// else a character's.
fn kind(file: &Path, text: &str) -> Kind {
    let named_local = file
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(".local.yaml"));
    let v: serde_norway::Value = serde_norway::from_str(text).unwrap_or_default();
    if named_local || v.get("area").is_some() {
        Kind::Local
    } else if v.get("do").is_some() {
        Kind::Routine
    } else {
        Kind::Character
    }
}

/// The problems found, by file and field.
struct Problems<'a> {
    root: &'a Path,
    list: Vec<String>,
}

impl Problems<'_> {
    fn add(&mut self, file: &Path, field: &str, why: &str) {
        let name = file
            .strip_prefix(self.root)
            .unwrap_or(file)
            .display()
            .to_string()
            .replace('\\', "/");
        self.list.push(if field.is_empty() {
            format!("{name}: {why}")
        } else {
            format!("{name}: {field}: {why}")
        });
    }

    /// A file that didn't read, in `file` unless the error names another.
    fn config(&mut self, file: &Path, e: &ConfigError) {
        let file = e.file.as_deref().unwrap_or(file);
        self.add(file, e.field.as_deref().unwrap_or(""), &e.message);
    }

    fn done(self, text: String) -> Checked {
        Checked {
            text,
            problems: self.list,
        }
    }
}

/// Where a routine's walks start, and the rules they keep to.
struct Reach {
    hub: (u32, u32),
    rules: mp_data::RouteRules,
}

struct World<'a> {
    root: &'a Path,
    places: &'a Places,
    gd: &'a GameData,
    level: Option<u32>,
}

impl World<'_> {
    fn character(&self, out: &mut Problems, file: &Path) -> String {
        let mut c: CharacterConfig = match load_character(file) {
            Ok(l) => l.value,
            Err(e) => {
                out.config(file, &e);
                return String::new();
            }
        };
        let mut notes = Vec::new();
        let mut area = None;
        let mut reach = None;
        let mut gear_rule = false;
        if let Some(rr) = &c.routine {
            let (rfile, prefix) = match rr {
                RoutineRef::Named { name, .. } => {
                    (self.root.join("routines").join(format!("{name}.yaml")), "")
                }
                RoutineRef::Inline(_) => (file.to_owned(), "routine."),
            };
            match load_routine(&self.root.join("routines"), &c.name, rr) {
                Err(e) => out.config(&rfile, &e),
                Ok(mut r) => {
                    // As the server runs it: a rest without `until` stops at the
                    // character's own thresholds.
                    for rule in &mut r.rules {
                        if let RuleSpec::Rest(rest) = rule
                            && rest.until.is_none()
                        {
                            rest.until = Some(c.rest.stop_when_all);
                        }
                    }
                    reach = self.walks(out, &rfile, prefix, &mut r, c.class.as_deref());
                    area = r.places.clone();
                    gear_rule = r.rules.iter().any(|x| matches!(x, RuleSpec::Gear(_)));
                    c.routine = Some(RoutineRef::Inline(Box::new(r)));
                }
            }
        }
        for (j, g) in c.gear.iter_mut().enumerate() {
            let shops = match &mut g.at {
                Some(p) => {
                    match self.resolve(out, file, &format!("gear[{j}].at"), p, area.as_deref()) {
                        Some(k) => vec![k],
                        None => continue,
                    }
                }
                None => self.places.shops_selling(&g.item, area.as_deref()),
            };
            if !gear_rule {
                continue;
            }
            if shops.is_empty() {
                out.add(
                    file,
                    &format!("gear[{j}]"),
                    &format!("no shop in places/ sells {:?}", g.item),
                );
                continue;
            }
            let Some(reach) = &reach else {
                continue;
            };
            // The routine tries the shops in turn; the first it can walk to is where it buys.
            let mut first_why = None;
            let shop = shops.iter().find(|&&s| match self.both_ways(reach, s) {
                Ok(()) => true,
                Err(why) => {
                    first_why.get_or_insert(why);
                    false
                }
            });
            match shop {
                Some(&s) => notes.push(format!(
                    "# gear[{j}] {}: bought at {}",
                    g.item,
                    self.named(s)
                )),
                None => out.add(
                    file,
                    &format!("gear[{j}]"),
                    &format!(
                        "no shop selling {:?} can be walked to: {}",
                        g.item,
                        first_why.unwrap_or_default()
                    ),
                ),
            }
        }
        let mut text = c.to_yaml();
        for n in notes {
            text.push_str(&n);
            text.push('\n');
        }
        text
    }

    fn routine_file(&self, out: &mut Problems, file: &Path) -> String {
        let name = file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_owned();
        let dir = file.parent().unwrap_or(Path::new("."));
        let named = RoutineRef::Named {
            name: name.clone(),
            with: BTreeMap::new(),
        };
        match load_routine(dir, &name, &named) {
            Err(e) => {
                out.config(file, &e);
                String::new()
            }
            Ok(mut r) => {
                self.walks(out, file, "", &mut r, None);
                r.to_yaml()
            }
        }
    }

    /// Each alias as the room it names, the room's name beside it.
    fn local(&self, out: &mut Problems, file: &Path, text: &str) -> String {
        let local: Local = match serde_norway::from_str(text) {
            Ok(l) => l,
            Err(e) => {
                out.add(file, "", &e.to_string());
                return String::new();
            }
        };
        let mut lines = vec![
            format!("schema: {}", local.schema),
            format!("area: {}", local.area),
            "aliases:".to_owned(),
        ];
        for (alias, target) in &local.aliases {
            let mut p = Place::Named(target.clone());
            let field = format!("aliases.{alias}");
            let line = BTreeMap::from([(alias.as_str(), target.as_str())]);
            let line = serde_norway::to_string(&line).unwrap_or_default();
            match self.resolve(out, file, &field, &mut p, Some(&local.area)) {
                Some(k) => {
                    let one = BTreeMap::from([(alias.as_str(), format!("{}/{}", k.0, k.1))]);
                    let written = serde_norway::to_string(&one).unwrap_or_default();
                    lines.push(format!("  {}  # {}", written.trim_end(), self.named(k)));
                }
                None => lines.push(format!("  {}", line.trim_end())),
            }
        }
        let mut t = lines.join("\n");
        t.push('\n');
        t
    }

    /// Resolves a routine's places in place and proves each reachable; the walks' start
    /// and rules, when there's a place to start from.
    fn walks(
        &self,
        out: &mut Problems,
        file: &Path,
        prefix: &str,
        r: &mut Routine,
        class: Option<&str>,
    ) -> Option<Reach> {
        let area = r.places.clone();
        let area = area.as_deref();
        let mut spots: Vec<(String, (u32, u32))> = Vec::new();
        // Circuits whose lairs all name rooms, for their wait-rooms once the avoided rooms
        // are known.
        let mut circuits = Vec::new();
        // Train rules that name no trainer, for the trainers the room files list.
        let mut trains = Vec::new();
        for (i, rule) in r.rules.iter_mut().enumerate() {
            let at = match rule {
                RuleSpec::Rest(x) => &mut x.at,
                RuleSpec::Camp(c) => &mut c.at,
                RuleSpec::Gear(_) => continue,
                RuleSpec::Train(t) => match &mut t.at {
                    Some(p) => p,
                    None => {
                        trains.push(i);
                        continue;
                    }
                },
                RuleSpec::Circuit(c) => {
                    // Respawn times are keyed by the lair as written, which becomes its
                    // room number in the long form.
                    let given = std::mem::take(&mut c.respawn);
                    let mut all = true;
                    for (j, l) in c.lairs.iter_mut().enumerate() {
                        let key = l.as_key();
                        let field = format!("{prefix}do[{i}].lairs[{j}]");
                        match self.resolve(out, file, &field, l, area) {
                            Some(k) => {
                                if let Some(d) = given.get(&key) {
                                    c.respawn.insert(format!("{}/{}", k.0, k.1), *d);
                                }
                                spots.push((field, k));
                            }
                            None => all = false,
                        }
                    }
                    if all {
                        circuits.push(i);
                    }
                    continue;
                }
            };
            let field = format!("{prefix}do[{i}].at");
            if let Some(k) = self.resolve(out, file, &field, at, area) {
                spots.push((field, k));
            }
        }
        let mut rules = RouteRules::default();
        for (i, p) in r.avoid.rooms.iter_mut().enumerate() {
            let field = format!("{prefix}avoid.rooms[{i}]");
            if let Some(k) = self.resolve(out, file, &field, p, area) {
                rules.avoid_rooms.insert(k);
            }
        }
        for (i, a) in r.avoid.areas.iter().enumerate() {
            match self.places.rooms_in(a) {
                Some(keys) => rules.avoid_rooms.extend(keys),
                None => out.add(
                    file,
                    &format!("{prefix}avoid.areas[{i}]"),
                    &format!("no area {a:?} in places/"),
                ),
            }
        }
        for e in &r.avoid.exits {
            // The routine's own check has refused a kind it doesn't know.
            if let Some(code) = mp_data::places::exit_kind_code(e) {
                rules.avoid_kinds.insert(code);
            }
        }
        for (name, cost) in &r.prefer_not {
            match place(self.places, &Place::Named(name.clone()), area) {
                Ok(k) => {
                    rules.prefer_not.insert(k, *cost);
                }
                Err(e) => out.add(file, &format!("{prefix}prefer_not.{name}"), &e),
            }
        }
        // Each lair's wait-room, and its respawn time as the server would take it.
        for i in circuits {
            let RuleSpec::Circuit(c) = &mut r.rules[i] else {
                continue;
            };
            match circuit_lairs(self.places, area, c, &rules.avoid_rooms) {
                Ok(lairs) => {
                    for (j, l) in lairs.iter().enumerate() {
                        c.respawn
                            .entry(format!("{}/{}", l.at.0, l.at.1))
                            .or_insert(l.respawn);
                        spots.push((format!("{prefix}do[{i}].lairs[{j}]"), l.wait_at));
                    }
                }
                Err((f, why)) => out.add(file, &format!("{prefix}do[{i}].{f}"), &why),
            }
        }
        // Where the bot is when nothing else is due: the last rule's place.
        let hub = r
            .rules
            .iter()
            .rev()
            .filter_map(|rule| match rule {
                RuleSpec::Rest(x) => Some(&x.at),
                RuleSpec::Camp(c) => Some(&c.at),
                RuleSpec::Circuit(c) => c.lairs.last(),
                RuleSpec::Gear(_) | RuleSpec::Train(_) => None,
            })
            .find_map(|p| place(self.places, p, area).ok())?;
        let reach = Reach {
            hub,
            rules: data_rules(&rules, self.level),
        };
        for (field, k) in &spots {
            if let Err(why) = self.both_ways(&reach, *k) {
                out.add(file, field, &why);
            }
        }
        // A train rule naming no trainer trains at the first it can walk to (#56).
        for i in trains {
            let field = format!("{prefix}do[{i}]");
            let trainers = self.places.trainers(class, area);
            let Some(first) = trainers.first() else {
                let who = class.map_or("any class".to_owned(), |c| format!("a {c}"));
                out.add(file, &field, &format!("no trainer in places/ trains {who}"));
                continue;
            };
            if !trainers.iter().any(|&t| self.both_ways(&reach, t).is_ok()) {
                let why = self.both_ways(&reach, *first).err().unwrap_or_default();
                out.add(file, &field, &format!("no trainer can be walked to: {why}"));
            }
        }
        Some(reach)
    }

    /// Whether the bot can walk from the routine's start to `to` and back.
    fn both_ways(&self, reach: &Reach, to: (u32, u32)) -> Result<(), String> {
        let key = |(m, r): (u32, u32)| (i64::from(m), i64::from(r));
        for (a, b) in [(reach.hub, to), (to, reach.hub)] {
            self.gd
                .route_with(key(a), key(b), &reach.rules)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// The room `p` names, written back as the long form with the world's name; a
    /// problem at `field` when it names none, or its name isn't the world's any more.
    fn resolve(
        &self,
        out: &mut Problems,
        file: &Path,
        field: &str,
        p: &mut Place,
        area: Option<&str>,
    ) -> Option<(u32, u32)> {
        let k = match place(self.places, p, area) {
            Ok(k) => k,
            Err(e) => {
                out.add(file, field, &e);
                return None;
            }
        };
        let Some(world) = self
            .gd
            .room(k.0.into(), k.1.into())
            .ok()
            .flatten()
            .map(|r| r.name)
        else {
            out.add(
                file,
                field,
                &format!("no room {}/{} in the game data", k.0, k.1),
            );
            return None;
        };
        if let Place::Room { name: Some(n), .. } = p
            && !n.trim().eq_ignore_ascii_case(world.trim())
        {
            out.add(
                file,
                field,
                &format!(
                    "room {}/{} is {world:?} in the game data, not {:?}: has the world been renumbered?",
                    k.0,
                    k.1,
                    n.trim()
                ),
            );
        }
        *p = Place::Room {
            map: k.0,
            room: k.1,
            name: Some(world),
        };
        Some(k)
    }

    /// `Newhaven, Arena (1/2150)`.
    fn named(&self, k: (u32, u32)) -> String {
        match self.gd.room(k.0.into(), k.1.into()).ok().flatten() {
            Some(r) => format!("{} ({}/{})", r.name, k.0, k.1),
            None => format!("{}/{}", k.0, k.1),
        }
    }
}
