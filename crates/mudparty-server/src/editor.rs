//! The character editor's server side (#110, design §16): open a character's file for the
//! page, check a change from its forms or its YAML tab, save it (atomically, against the
//! revision it was opened at, with a password into `secrets.yaml`), and the choices the
//! forms offer, from the game data and the files.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use mp_config::{CharacterConfig, Melee, character_from_text, revision};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;

use crate::{Server, check_text};

/// What the page sends: the forms' file, as JSON, or the YAML tab's text.
#[derive(Debug, Clone)]
pub enum EditInput {
    Config(serde_json::Value),
    Yaml(String),
}

/// A file opened for the editor.
#[derive(Debug, Clone, Serialize)]
pub struct Opened {
    /// The revision it was opened at, for the save to keep to.
    pub rev: String,
    /// The file as the server reads it, for the forms; `null` when it doesn't read.
    pub config: serde_json::Value,
    /// The file's text, for the YAML tab.
    pub yaml: String,
    /// It has comments, which a form save would lose.
    pub comments: bool,
    /// `secrets.yaml` has a password for its account (the password itself never leaves).
    pub password_set: bool,
}

/// What a change would make of the file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct EditChecked {
    /// What blocks a save: the file doesn't read, or `check` finds it can't run.
    pub problems: Vec<String>,
    /// What doesn't: traps such as targeting with nothing to attack with (#93).
    pub warnings: Vec<String>,
    /// The YAML a save would write.
    pub yaml: String,
}

/// How a save went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Saved {
    /// Written; its new revision.
    Ok { rev: String },
    /// The file changed on disk since it was opened: what's there now.
    Conflict { rev: String, yaml: String },
    /// Not written: what's in the way.
    Refused(EditChecked),
}

/// The forms' choices.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Choices {
    pub classes: Vec<ClassChoice>,
    pub spells: Vec<SpellChoice>,
    pub monsters: Vec<String>,
    pub items: Vec<ItemChoice>,
    pub routines: Vec<RoutineChoice>,
    /// The character files, by name.
    pub characters: Vec<String>,
    /// Characters in silvermere's database that have no file yet.
    pub unfiled: Vec<Unfiled>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClassChoice {
    pub name: String,
    /// Its kind of magic (0 none), and how far into it it goes.
    pub magery: i64,
    pub magelvl: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpellChoice {
    /// The cast code.
    pub short: String,
    pub name: String,
    pub mana: i64,
    pub level: i64,
    /// The kind of magic, and the level of it, a class needs to cast it.
    pub magery: i64,
    pub magelvl: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ItemChoice {
    pub item: String,
    /// The shops that sell it, by key (`1/2141`).
    pub shops: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RoutineChoice {
    pub name: String,
    /// Its parameters and their defaults, as written.
    pub params: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Unfiled {
    pub name: String,
    pub class: String,
}

/// A name a file may be made for: letters only, as the game's first names are.
fn file_name_ok(name: &str) -> bool {
    !name.is_empty() && name.len() <= 30 && name.chars().all(|c| c.is_ascii_alphabetic())
}

/// Whether a file has comments: a `#` starting a line, or after a space, outside quotes.
fn has_comments(text: &str) -> bool {
    text.lines().any(|line| {
        let mut quote: Option<char> = None;
        let mut prev = ' ';
        for c in line.chars() {
            match quote {
                Some(q) if c == q => quote = None,
                Some(_) => {}
                None if c == '"' || c == '\'' => quote = Some(c),
                None if c == '#' && prev.is_whitespace() => return true,
                None => {}
            }
            prev = c;
        }
        false
    })
}

/// Traps a file can hold though it reads and runs (#93's start): warnings, not problems.
fn warnings(c: &CharacterConfig, file: &str) -> Vec<String> {
    let mut out = Vec::new();
    let combat = &c.combat;
    if !combat.targeting.is_empty()
        && combat.melee == Melee::None
        && combat.tiers.is_empty()
        && combat.room_tiers.is_empty()
    {
        out.push(format!(
            "{file}: combat: targeting picks targets, but with no melee and no tiers the bot never attacks; set `melee: a`, or add a tier"
        ));
    }
    out
}

/// Writes `text` to `path` beside it first, then renamed into place, so a reader (the
/// server's watcher) never sees half of it.
fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("no file name in {}", path.display()))?;
    let temp = path.with_file_name(format!(".{name}.tmp"));
    std::fs::write(&temp, text).map_err(|e| format!("{}: {e}", temp.display()))?;
    std::fs::rename(&temp, path).map_err(|e| format!("{}: {e}", path.display()))
}

impl Server {
    fn character_path(&self, name: &str) -> Result<PathBuf, String> {
        if !file_name_ok(name) {
            return Err(format!("{name:?} isn't a character's name"));
        }
        Ok(self.root.join("characters").join(format!("{name}.yaml")))
    }

    /// Opens `name`'s file for the editor.
    pub fn edit_open(&self, name: &str) -> Result<Opened, String> {
        let path = self.character_path(name)?;
        let yaml = std::fs::read_to_string(&path).map_err(|_| format!("{name} has no file"))?;
        let parsed = character_from_text(&path, &yaml).ok();
        let password_set = parsed
            .as_ref()
            .and_then(|c| c.login.as_ref())
            .is_some_and(|l| {
                self.secrets
                    .lock()
                    .expect("secrets lock")
                    .password(&l.account)
                    .is_some()
            });
        Ok(Opened {
            rev: revision(yaml.as_bytes()),
            config: parsed
                .and_then(|c| serde_json::to_value(c).ok())
                .unwrap_or(serde_json::Value::Null),
            comments: has_comments(&yaml),
            password_set,
            yaml,
        })
    }

    /// The text a change would save: the forms' file written out, or the YAML as typed.
    fn edit_text(input: &EditInput) -> Result<String, String> {
        match input {
            EditInput::Yaml(t) => Ok(t.clone()),
            EditInput::Config(v) => serde_json::from_value::<CharacterConfig>(v.clone())
                .map(|c| c.to_yaml())
                .map_err(|e| format!("the forms' file doesn't read: {e}")),
        }
    }

    /// Checks a change to `name`'s file without saving it.
    pub fn edit_check(&self, name: &str, input: EditInput) -> Result<EditChecked, String> {
        let path = self.character_path(name)?;
        let yaml = match Self::edit_text(&input) {
            Ok(t) => t,
            Err(why) => {
                return Ok(EditChecked {
                    problems: vec![why],
                    ..EditChecked::default()
                });
            }
        };
        let checked = check_text(&self.root, &path, &yaml, None).map_err(|e| e.to_string())?;
        let file = format!("characters/{name}.yaml");
        let warnings = character_from_text(&path, &yaml)
            .map(|c| warnings(&c, &file))
            .unwrap_or_default();
        Ok(EditChecked {
            problems: checked.problems,
            warnings,
            yaml,
        })
    }

    /// Saves a change to `name`'s file: against `rev`, the revision it was opened at, or as
    /// a new file when `rev` is none; with `password` for its account.
    pub fn edit_save(
        &self,
        name: &str,
        rev: Option<&str>,
        input: EditInput,
        password: Option<&str>,
    ) -> Result<Saved, String> {
        let path = self.character_path(name)?;
        let on_disk = std::fs::read_to_string(&path).ok();
        match (rev, &on_disk) {
            (Some(rev), Some(now)) if revision(now.as_bytes()) != rev => {
                return Ok(Saved::Conflict {
                    rev: revision(now.as_bytes()),
                    yaml: now.clone(),
                });
            }
            (Some(_), None) => return Err(format!("{name}'s file has gone")),
            (None, Some(_)) => return Err(format!("{name} already has a file")),
            _ => {}
        }
        let checked = self.edit_check(name, input)?;
        if !checked.problems.is_empty() {
            return Ok(Saved::Refused(checked));
        }
        let c = character_from_text(&path, &checked.yaml).map_err(|e| e.to_string())?;
        if let Some(pw) = password {
            let account = c
                .login
                .as_ref()
                .map(|l| l.account.clone())
                .ok_or_else(|| "a password needs login.account".to_owned())?;
            let mut secrets = self.secrets.lock().expect("secrets lock");
            let mut next = secrets.clone();
            next.set_password(&account, pw);
            write_atomic(&self.root.join("secrets.yaml"), &next.to_yaml())?;
            *secrets = next;
        }
        write_atomic(&path, &checked.yaml)?;
        // The card, at once rather than when the watcher gets to it.
        self.reload(&path);
        Ok(Saved::Ok {
            rev: revision(checked.yaml.as_bytes()),
        })
    }

    /// The forms' choices, from the game data and the files.
    pub fn edit_choices(&self) -> Result<Choices, String> {
        let mut out = Choices::default();
        let mut class_names: BTreeMap<i64, String> = BTreeMap::new();
        if let Some(gd) = self.gamedata.lock().expect("gamedata lock").as_ref() {
            for k in gd.classes().map_err(|e| e.to_string())? {
                class_names.insert(k.number, k.name.clone());
                out.classes.push(ClassChoice {
                    name: k.name,
                    magery: k.magery,
                    magelvl: k.magelvl,
                });
            }
            out.spells = gd
                .spells()
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|s| SpellChoice {
                    short: s.short,
                    name: s.name,
                    mana: s.mana,
                    level: s.level,
                    magery: s.magery,
                    magelvl: s.magelvl,
                })
                .collect();
            out.monsters = gd.monster_names().map_err(|e| e.to_string())?;
        }
        out.items = self
            .places
            .items_sold()
            .into_iter()
            .map(|(item, shops)| ItemChoice {
                item,
                shops: shops.into_iter().map(|(m, r)| format!("{m}/{r}")).collect(),
            })
            .collect();
        out.routines = routine_choices(&self.root.join("routines"));
        out.characters = yaml_stems(&self.root.join("characters"));
        let filed: BTreeSet<String> = out.characters.iter().map(|n| n.to_lowercase()).collect();
        out.unfiled = unfiled(
            &self.config.gamedata.silvermere_db,
            &self.root,
            &class_names,
        )
        .into_iter()
        .filter(|u| !filed.contains(&u.name.to_lowercase()))
        .collect();
        Ok(out)
    }
}

/// The names of a folder's `.yaml` files, sorted.
fn yaml_stems(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|e| {
            let p = e.path();
            (p.extension()? == "yaml").then(|| p.file_stem()?.to_str().map(str::to_owned))?
        })
        .collect();
    out.sort();
    out
}

/// Each routine in `routines/`, with its parameters and their defaults as written.
fn routine_choices(dir: &Path) -> Vec<RoutineChoice> {
    yaml_stems(dir)
        .into_iter()
        .map(|name| {
            let text =
                std::fs::read_to_string(dir.join(format!("{name}.yaml"))).unwrap_or_default();
            let v: serde_norway::Value = serde_norway::from_str(&text).unwrap_or_default();
            let params = v
                .get("params")
                .and_then(serde_norway::Value::as_mapping)
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| {
                            let text = match v {
                                serde_norway::Value::String(s) => s.clone(),
                                serde_norway::Value::Number(n) => n.to_string(),
                                serde_norway::Value::Bool(b) => b.to_string(),
                                _ => return None,
                            };
                            Some((k.as_str()?.to_owned(), text))
                        })
                        .collect()
                })
                .unwrap_or_default();
            RoutineChoice { name, params }
        })
        .collect()
}

/// The characters in silvermere's database, read-only: as fresh as its last save. None
/// when it can't be read.
fn unfiled(db: &Path, root: &Path, classes: &BTreeMap<i64, String>) -> Vec<Unfiled> {
    let path = if db.is_relative() {
        root.join(db)
    } else {
        db.to_owned()
    };
    let Ok(conn) = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
        return Vec::new();
    };
    let Ok(mut q) = conn.prepare("SELECT firstname, class FROM characters ORDER BY firstname")
    else {
        return Vec::new();
    };
    q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .map(|rows| {
            rows.filter_map(Result::ok)
                .filter(|(name, _)| !name.is_empty())
                .map(|(name, class)| Unfiled {
                    name,
                    class: classes.get(&class).cloned().unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mp_config::Secrets;

    #[test]
    fn comments_are_found_outside_quotes() {
        assert!(has_comments("# a file\nname: Ermias\n"));
        assert!(has_comments("melee: a   # swing\n"));
        assert!(!has_comments("statline: \"[HP=%h #x]\"\n"));
        assert!(!has_comments("name: Ermias\n"));
    }

    #[test]
    fn a_file_name_is_a_name() {
        assert!(file_name_ok("Ermias"));
        assert!(!file_name_ok("../escape"));
        assert!(!file_name_ok(""));
        assert!(!file_name_ok("Er mias"));
    }

    #[test]
    fn secrets_write_every_account() {
        let mut s = Secrets::default();
        s.set_password("Ermias", "a \"quoted\" one");
        let back = Secrets::from_yaml(&s.to_yaml()).unwrap();
        assert_eq!(back.password("Ermias"), Some("a \"quoted\" one"));
    }
}
