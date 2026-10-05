//! The write-policy registry behind `dux config get` and `dux config set`.
//!
//! A small, core-owned boundary for addressing ONE setting by a dot path and
//! writing it safely, shared by the CLI and (later) the web's structured
//! writers. Deliberately not a universal config framework: it knows how to
//! find a setting, how to read a typed value for it, and whether that setting
//! may be written as plain text at all.
//!
//! # Paths
//!
//! A path is table names and a key joined by dots: `server.port`,
//! `ui.theme`, `server.auth.require`. Each part is letters, digits, `_` or
//! `-`; a key whose own name contains a dot cannot be addressed (edit the file
//! for that). The settings are the ones `Config::default()` has, plus the
//! entries of two named maps: `providers.<name>.<field>` (a provider by its
//! name, existing or new) and `env.<NAME>`. A list is replaced whole, written
//! as a TOML array (`'["a", "b"]'`). `[[projects]]`, `[keys]` and `[macros]`
//! are read with `get` but not written with `set`: each has its own rules
//! (identity, the keybinding parser, ordering) that a one-key write would
//! bypass.
//!
//! # Write policies
//!
//! [`WritePolicy::Plain`] settings take their value on the command line.
//! [`WritePolicy::Secret`] ones never do: `server.auth.password` is a VIRTUAL
//! key, read from a hidden prompt or a pipe, checked against the configured
//! minimums, and stored as its Argon2id hash at `server.auth.password_hash`.
//! `get` on it prints that hash. `env.<NAME>` values are secrets too
//! ([`SecretKind::Text`]): asked for or piped in, stored as typed, and printed
//! by `get` only on request ([`Key::is_sensitive`]). A future write-only
//! setting is one more row in [`VIRTUAL_KEYS`].
//!
//! Every write goes through [`crate::config_write::mutate_config_file`], so it
//! keeps the file's comments, never loses a concurrent writer's update, and
//! can never leave `[server.auth]` in a state dux would refuse to start with.

use std::fmt;
use std::path::Path;

use anyhow::{Context, Result};
use toml_edit::{DocumentMut, InlineTable, Item, Table, Value};

use crate::auth::{MinimumCheck, Password, Strength};
use crate::config::{Config, ProviderCommandConfig};
pub use crate::config_write::MissingConfig;

/// How a setting may be written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WritePolicy {
    /// An ordinary value, given on the command line and validated by type.
    Plain,
    /// Never given on the command line or stored as typed.
    Secret(SecretKind),
}

/// What a [`WritePolicy::Secret`] setting turns its input into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretKind {
    /// An Argon2id hash of the input, stored at `stores_at` (its path's
    /// segments).
    PasswordHash { stores_at: &'static [&'static str] },
    /// Stored as typed, but never taken from the command line and never
    /// printed unless asked for: `env.<NAME>`, where API tokens live.
    Text,
}

/// Write-only settings that have no field of their own in `Config`, each by
/// its path's segments.
pub const VIRTUAL_KEYS: &[(&[&str], WritePolicy)] = &[(
    &["server", "auth", "password"],
    WritePolicy::Secret(SecretKind::PasswordHash {
        stores_at: &["server", "auth", "password_hash"],
    }),
)];

/// Top-level tables `set` refuses, with the reason it gives.
const NOT_SETTABLE: &[(&str, &str)] = &[
    (
        "projects",
        "projects are added and changed with dux itself, or by editing [[projects]] in config.toml",
    ),
    (
        "keys",
        "keybindings are checked by the terminal UI when it loads them; edit [keys] in config.toml",
    ),
    (
        "macros",
        "macros keep their order and surface together; edit [macros] in config.toml or use the \
         macro editor",
    ),
];

/// The shape a setting's value takes, from its default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Bool,
    Integer,
    Text,
    List,
    /// An optional setting with no default, so its type is unknown here and
    /// the value is tried as a TOML literal, then as text.
    Unknown,
    Table,
}

/// One addressable setting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    /// The path as given, split on dots.
    pub path: Vec<String>,
    pub policy: WritePolicy,
    shape: Shape,
}

impl Key {
    /// The path as printed, through the one formatter.
    pub fn dotted(&self) -> String {
        crate::config::shown_path("", &self.path)
    }

    /// True for a table (`server.auth`) rather than one setting.
    pub fn is_table(&self) -> bool {
        self.shape == Shape::Table
    }

    /// Whether reading it shows a secret: an environment value, or a table
    /// holding them (`env`, and `projects`, whose entries carry their own
    /// `env`). `get` prints these only when asked to. The password's virtual
    /// key is not one: what it reads is the hash.
    pub fn is_sensitive(&self) -> bool {
        matches!(self.path[0].as_str(), "env" | "projects")
    }
}

/// Why a path does not name a setting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyError {
    /// Not a well-formed path. `known` is the longest start of it that is a
    /// setting or table (empty when none is), the only part of what was typed
    /// that is ever repeated; `reason` holds nothing that was typed.
    Malformed { known: String, reason: String },
    /// No such setting. `known` is the longest start of the path that is a
    /// setting or table (empty when none is); the rest is never repeated,
    /// since it may be a value typed where a name goes. `suggestion` is the
    /// closest setting directly below `known`.
    Unknown {
        known: String,
        suggestion: Option<String>,
    },
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed { known, reason } if known.is_empty() => f.write_str(reason),
            Self::Malformed { known, reason } => write!(
                f,
                "{reason} (the path starts with {known}; what follows it is not repeated)"
            ),
            Self::Unknown { known, suggestion } => {
                if known.is_empty() {
                    write!(f, "there is no setting with that name")?;
                } else {
                    write!(f, "{known} has no setting below it with that name")?;
                }
                match suggestion {
                    Some(close) => write!(f, "; did you mean {close}?")?,
                    None => write!(
                        f,
                        "; `dux config path` shows the file, whose comments list every setting."
                    )?,
                }
                write!(f, " Values are never given in the path.")
            }
        }
    }
}

impl std::error::Error for KeyError {}

/// The longest start of `segments` that is a setting or table, as printed,
/// or empty when none is: the only part of a typed path any refusal repeats.
fn known_prefix(segments: &[String]) -> String {
    let known = (0..=segments.len())
        .rev()
        .find(|len| {
            let prefix = &segments[..*len];
            *len > 0
                && (shape_of(prefix).is_some()
                    || VIRTUAL_KEYS.iter().any(|(name, _)| *name == prefix))
        })
        .unwrap_or(0);
    crate::config::shown_path("", &segments[..known])
}

/// THE refusal of a path that is not well formed: it repeats only the longest
/// start of `typed` that names a setting or table (see [`known_prefix`]),
/// read up to the first part that is empty or holds what no name can, and
/// never what follows it, which may be a value typed where a name goes.
/// `reason` must hold nothing that was typed.
fn malformed_at(typed: &str, reason: impl Into<String>) -> KeyError {
    let segments: Vec<String> = typed
        .split('.')
        .take_while(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
        .map(str::to_string)
        .collect();
    KeyError::Malformed {
        known: known_prefix(&segments),
        reason: reason.into(),
    }
}

/// Find the setting `path` names.
pub fn lookup(path: &str) -> Result<Key, KeyError> {
    // `name=value` is how other tools take a setting. What follows `=` may be
    // a password, so it is never repeated: only the name is.
    // A path is repeated in a message only when it is made of what a setting
    // name can hold; anything else may be a value typed into it (a password
    // after `:` or a space), so it is refused without being repeated.
    let name_like = |text: &str| {
        text.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    };
    let not_a_name = |typed: &str| {
        malformed_at(
            typed,
            "that is not a setting name; values are never given in the path",
        )
    };
    if let Some((name, _)) = path.split_once('=') {
        if !name_like(name) {
            return Err(not_a_name(name));
        }
        return Err(malformed_at(
            name,
            "a value goes after the setting's name and a space, not after `=`: write `dux \
             config set <setting> <value>` (a secret such as server.auth.password is asked for, \
             or read with --stdin). What followed `=` is not repeated here.",
        ));
    }
    if !name_like(path) {
        return Err(not_a_name(path));
    }
    let segments = split_path(path)?;
    if let Some((_, policy)) = VIRTUAL_KEYS
        .iter()
        .find(|(name, _)| *name == segments.as_slice())
    {
        return Ok(Key {
            path: segments,
            policy: *policy,
            shape: Shape::Text,
        });
    }
    if let [table, name] = segments.as_slice()
        && table == "env"
        && !crate::config::is_valid_env_name(name)
    {
        return Err(malformed_at(
            "env",
            "an environment variable name must match [A-Za-z_][A-Za-z0-9_]*, the rule dux \
             starts with",
        ));
    }
    match shape_of(&segments) {
        Some(shape) => Ok(Key {
            policy: if segments.len() == 2 && segments[0] == "env" {
                WritePolicy::Secret(SecretKind::Text)
            } else {
                WritePolicy::Plain
            },
            path: segments,
            shape,
        }),
        None => {
            // The longest start of the path that names a setting or table;
            // what follows it is never repeated (it may be a value).
            let known = (0..segments.len())
                .rev()
                .find(|len| {
                    let prefix = &segments[..*len];
                    *len > 0
                        && (shape_of(prefix).is_some()
                            || VIRTUAL_KEYS.iter().any(|(name, _)| *name == prefix))
                })
                .unwrap_or(0);
            Err(KeyError::Unknown {
                known: known_prefix(&segments[..known]),
                suggestion: suggest(&segments[..known], &segments[known]),
            })
        }
    }
}

fn split_path(path: &str) -> Result<Vec<String>, KeyError> {
    if path.is_empty() {
        return Err(malformed_at("", "a setting's path cannot be empty"));
    }
    let segments: Vec<String> = path.split('.').map(str::to_string).collect();
    for segment in &segments {
        if segment.is_empty() {
            return Err(malformed_at(
                path,
                "the path has an empty part; write the path as table.key, like server.port",
            ));
        }
        if !segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(malformed_at(
                path,
                "the path has a part with characters other than letters, digits, _ and -; a \
                 key whose name needs them cannot be set this way, so edit config.toml",
            ));
        }
    }
    Ok(segments)
}

fn default_tree() -> serde_json::Value {
    serde_json::to_value(Config::default()).unwrap_or(serde_json::Value::Null)
}

fn provider_template() -> serde_json::Value {
    serde_json::to_value(ProviderCommandConfig::default()).unwrap_or(serde_json::Value::Null)
}

fn shape_of_json(value: &serde_json::Value) -> Shape {
    match value {
        serde_json::Value::Bool(_) => Shape::Bool,
        serde_json::Value::Number(_) => Shape::Integer,
        serde_json::Value::String(_) => Shape::Text,
        serde_json::Value::Array(_) => Shape::List,
        serde_json::Value::Null => Shape::Unknown,
        serde_json::Value::Object(_) => Shape::Table,
    }
}

fn shape_of(segments: &[String]) -> Option<Shape> {
    let parts: Vec<&str> = segments.iter().map(String::as_str).collect();
    match parts.as_slice() {
        ["providers"] | ["env"] => return Some(Shape::Table),
        ["providers", _] => return Some(Shape::Table),
        ["providers", _, field] => {
            return provider_template().get(*field).map(shape_of_json);
        }
        ["env", _] => return Some(Shape::Text),
        // Maps whose keys are names: an action's bindings, a macro by its
        // name (and the fields of one written as a table). Any name is a
        // setting there, set or not, as for `[env]` and `[providers]`.
        ["keys", _] | ["macros", _] | ["macros", _, _] => return Some(Shape::Unknown),
        _ => {}
    }
    let mut node = default_tree();
    for part in &parts {
        node = node.get(*part)?.clone();
    }
    Some(shape_of_json(&node))
}

/// Every setting a path can name, by its segments, for suggestions.
fn known_paths() -> Vec<Vec<String>> {
    fn walk(node: &serde_json::Value, path: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
        if let serde_json::Value::Object(map) = node {
            for (name, child) in map {
                path.push(name.clone());
                out.push(path.clone());
                walk(child, path, out);
                path.pop();
            }
        }
    }
    let mut out: Vec<Vec<String>> = VIRTUAL_KEYS
        .iter()
        .map(|(name, _)| name.iter().map(|segment| (*segment).to_string()).collect())
        .collect();
    walk(&default_tree(), &mut Vec::new(), &mut out);
    out
}

/// The setting directly below `known` whose name is closest to `typed` (the
/// segment typed after it), printed through the one formatter, when it is
/// close enough to be the one meant.
fn suggest(known: &[String], typed: &str) -> Option<String> {
    let mut best: Option<(usize, Vec<String>)> = None;
    for candidate in known_paths() {
        if candidate.len() != known.len() + 1 || !candidate.starts_with(known) {
            continue;
        }
        let distance = edit_distance(typed, &candidate[known.len()]);
        if best.as_ref().is_none_or(|(d, _)| distance < *d) {
            best = Some((distance, candidate));
        }
    }
    let (distance, candidate) = best?;
    let allowed = (typed.chars().count() / 3).max(2);
    (distance <= allowed).then(|| crate::config::shown_path("", &candidate))
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut current = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != cb);
            current.push(substitution.min(previous[j + 1] + 1).min(current[j] + 1));
        }
        previous = current;
    }
    previous[b.len()]
}

/// Read `raw` (as typed on the command line) as a value for `key`, checked
/// against the setting's type, range and allowed values. Text settings take
/// `raw` verbatim, with no quotes needed; lists take a TOML array.
pub fn parse_value(key: &Key, raw: &str) -> Result<Value, String> {
    if let WritePolicy::Secret(_) = key.policy {
        return Err(format!(
            "{} is never given on the command line; dux asks for it instead",
            key.dotted()
        ));
    }
    if key.is_table() {
        return Err(format!(
            "{} is a table, not a setting; name one of its keys",
            key.dotted()
        ));
    }
    if let Some((_, reason)) = NOT_SETTABLE.iter().find(|(top, _)| *top == key.path[0]) {
        return Err(format!(
            "{} cannot be set from the command line: {reason}",
            key.dotted()
        ));
    }
    let candidates: Vec<Value> = match key.shape {
        Shape::Text => vec![Value::from(raw)],
        Shape::Bool | Shape::Integer | Shape::List => {
            vec![parse_literal(raw).map_err(|_| match key.shape {
                Shape::Bool => format!("{} takes true or false, not {raw}", key.dotted()),
                Shape::Integer => format!("{} takes a whole number, not {raw}", key.dotted()),
                _ => format!(
                    "{} takes a list written as a TOML array, like '[\"a\", \"b\"]', not {raw}",
                    key.dotted()
                ),
            })?]
        }
        Shape::Unknown => parse_literal(raw)
            .into_iter()
            .chain(std::iter::once(Value::from(raw)))
            .collect(),
        Shape::Table => Vec::new(),
    };
    // A setting with a fixed set of values is checked by the parser dux reads
    // it with (see `config_effective`), so `set` accepts exactly what dux
    // then uses as written.
    if let Some(fixed) = crate::config_effective::fixed_values(&key.path)
        && !fixed.accepts(raw)
    {
        let or_empty = if fixed.or_empty {
            ", or an empty value for the default"
        } else {
            ""
        };
        return Err(format!(
            "{} takes one of {}{or_empty}, not {raw}",
            key.dotted(),
            fixed.listed.join(", ")
        ));
    }
    let mut last_error = String::new();
    for value in candidates {
        match check_alone(key, &value) {
            Ok(()) => return Ok(value),
            Err(error) => last_error = error,
        }
    }
    Err(format!("{} cannot be {raw}: {last_error}", key.dotted()))
}

fn parse_literal(raw: &str) -> Result<Value, String> {
    raw.trim().parse::<Value>().map_err(|e| e.to_string())
}

/// Deserialize a config holding only this one value, so the setting's own
/// type and range decide (a port above 65535, a negative count). A
/// `server.auth` setting is checked for its type only here: its rules,
/// cross-field ones included, are checked against the file's own values
/// with this one applied, under the lock (see [`write_value`]), never
/// against the defaults.
fn check_alone(key: &Key, value: &Value) -> Result<(), String> {
    let mut doc = DocumentMut::new();
    set_in_doc(&mut doc, &key.path, value.clone()).map_err(|e| e.to_string())?;
    let text = doc.to_string();
    if key
        .path
        .starts_with(&["server".to_string(), "auth".to_string()])
    {
        let auth = toml::from_str::<toml::Table>(&text)
            .ok()
            .and_then(|file| file.get("server")?.get("auth").cloned())
            .ok_or_else(|| "not a server.auth setting".to_string())?;
        return crate::config_auth::check_types(auth);
    }
    toml::from_str::<Config>(&text)
        .map(|_| ())
        .map_err(|e| e.message().to_string())
}

/// Set `path` to `value` in `doc`, creating the tables on the way and keeping
/// the comment that trails an existing value.
fn set_in_doc(doc: &mut DocumentMut, path: &[String], mut value: Value) -> Result<()> {
    // A path `lookup` validated, printed through the one formatter.
    fn shown<S: AsRef<str>>(path: &[S]) -> String {
        let segments: Vec<String> = path.iter().map(|s| s.as_ref().to_string()).collect();
        crate::config::shown_path("", &segments)
    }
    let (leaf, parents) = path.split_last().context("empty path")?;
    let mut item: &mut Item = doc.as_item_mut();
    let mut walked: Vec<&str> = Vec::new();
    for segment in parents {
        walked.push(segment);
        let inline = item.is_inline_table();
        let table = item.as_table_like_mut().with_context(|| {
            format!(
                "{} is not a table in config.toml",
                shown(&walked[..walked.len() - 1])
            )
        })?;
        if table.get(segment).is_none() {
            let fresh = if inline {
                Item::Value(Value::InlineTable(InlineTable::new()))
            } else {
                Item::Table(Table::new())
            };
            table.insert(segment, fresh);
        }
        item = table.get_mut(segment).expect("just ensured");
        if !item.is_table_like() {
            anyhow::bail!(
                "{} is a value in config.toml, not a table, so {} cannot be set inside it",
                shown(&walked),
                shown(path)
            );
        }
    }
    let table = item
        .as_table_like_mut()
        .with_context(|| format!("{} is not a table in config.toml", shown(parents)))?;
    match table.get_mut(leaf) {
        // Assigned in place, so the key keeps the comment above it.
        Some(existing) => {
            if let Some(old) = existing.as_value() {
                *value.decor_mut() = old.decor().clone();
            }
            *existing = Item::Value(value);
        }
        None => {
            table.insert(leaf, Item::Value(value));
        }
    }
    Ok(())
}

/// What a successful `set` changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetReport {
    /// The dot path written (for a secret, where its hash went).
    pub path: Vec<String>,
    /// The value the file had before, as TOML, or `None` when it had none.
    pub previous: Option<String>,
    /// The value written, as TOML.
    pub now: String,
    /// What still stops a surface starting with the file after the write,
    /// each with the surfaces it stops: a set may repair one of several
    /// broken values, and says what is left.
    pub remaining_problems: Vec<crate::config::StartProblem>,
    /// Why the value written is not in effect yet, when the entry or section
    /// holding it was already dropped or reset by the load before the set
    /// (over another of its values): it stays at its default until that is
    /// fixed.
    pub held_back: Option<String>,
}

/// Write one plain setting into the config file at `config_path`, through
/// the coordinated mutation path. `raw` is parsed by [`parse_value`].
pub fn set_plain(config_path: &Path, key: &Key, raw: &str) -> Result<SetReport> {
    set_plain_with(config_path, MissingConfig::CreateDocumented, key, raw)
}

/// [`set_plain`] with a choice of what a missing file means.
pub fn set_plain_with(
    config_path: &Path,
    missing: MissingConfig<'_>,
    key: &Key,
    raw: &str,
) -> Result<SetReport> {
    let value = parse_value(key, raw).map_err(anyhow::Error::msg)?;
    write_value(config_path, missing, &key.path, value)
}

fn write_value(
    config_path: &Path,
    missing: MissingConfig<'_>,
    path: &[String],
    value: Value,
) -> Result<SetReport> {
    write_value_checked(config_path, missing, path, value, |_| Ok(()))
}

/// [`write_value`], with `check` run on the document under the same file
/// lock as the write, before the value goes in: what it reads cannot change
/// between the check and the write. An error from it writes nothing.
fn write_value_checked(
    config_path: &Path,
    missing: MissingConfig<'_>,
    path: &[String],
    value: Value,
    mut check: impl FnMut(&DocumentMut) -> Result<()>,
) -> Result<SetReport> {
    // Through the one value printer, as `get` prints too.
    let now = toml_value(&Item::Value(value.clone()))
        .and_then(|now| printed_value("", path, &now, ValueForm::Line))
        .unwrap_or_else(|| NOT_SHOWN.to_string());
    // A set may leave problems `[server.auth]` already had, so a section
    // with several broken values can be repaired one value at a time; one
    // that adds a problem is refused.
    let ((previous, held_back), remaining_problems) =
        crate::config_write::mutate_config_file_repairing(config_path, missing, path, |doc| {
            check(doc)?;
            let before = doc.to_string();
            // Any `[server.auth]` write waits while an auth setting sits where
            // dux does not read it: which one the user meant cannot be told.
            if path.starts_with(&["server".to_string(), "auth".to_string()])
                && let Some(refusal) =
                    misplaced_auth_refusal(&before, "which web UI password settings you meant")
            {
                anyhow::bail!("{refusal}");
            }
            let previous = value_in_doc(doc, path);
            let had = provider_command_in(doc, path);
            prepare_provider(doc, path)?;
            set_in_doc(doc, path, value)?;
            check_provider_command(doc, path, had)?;
            let held_back =
                refuse_a_set_dux_would_undo(&before, &doc.to_string(), path, config_path.parent())?;
            Ok((previous, held_back))
        })?;
    Ok(SetReport {
        path: path.to_vec(),
        previous,
        now,
        remaining_problems,
        held_back,
    })
}

/// The load correction (a drop, a reset of a whole section or entry) that
/// already covers an entry or section holding `path` in the file `raw`, if
/// any: the setting there is not in effect whatever it says.
fn held_back_by(raw: &str, path: &[String]) -> Option<crate::config::LoadCorrection> {
    crate::config::load_corrections_with_sources(raw)
        .into_iter()
        .find(|correction| correction.path.len() < path.len() && path.starts_with(&correction.path))
}

/// Refuse a set whose value dux's load would not use as written, by the same
/// corrections `get` reports (a value dropped, reset, clamped or read as
/// another): reporting it as set and then running something else would be a
/// lie the user finds later. A set inside an entry or section the load
/// already dropped or reset before it (over another of its values) is not
/// what keeps it out of effect, so it is taken, and the reason is returned
/// so the report can say the value stays at its default until that is fixed.
fn refuse_a_set_dux_would_undo(
    before: &str,
    after: &str,
    path: &[String],
    root: Option<&Path>,
) -> Result<Option<String>> {
    if let Some(held) = held_back_by(before, path) {
        let still = held_back_by(after, path).unwrap_or(held);
        return Ok(Some(still.reason));
    }
    // A provider set back to dux's own retired block is pruned by the load.
    if let [section, name, ..] = path
        && section == "providers"
        && let Ok(doc) = after.parse::<DocumentMut>()
        && crate::config_migrate::retired_provider_prunes(&doc)
            .iter()
            .any(|(pruned, _)| *pruned == [section.clone(), name.clone()])
    {
        let provider = crate::config::shown_path(after, &path[..2]);
        anyhow::bail!(
            "that change would make {provider} match the retired stock {name} provider, which \
             dux removes on load; nothing was changed. To remove the provider, delete its block."
        );
    }
    let key = Key {
        path: path.to_vec(),
        policy: WritePolicy::Plain,
        shape: Shape::Unknown,
    };
    let Ok(report) = get_report_inner(after, &key, root) else {
        return Ok(None);
    };
    let Some(correction) = report
        .corrections
        .iter()
        .find(|correction| correction.path == path)
    else {
        return Ok(None);
    };
    let shown = crate::config::shown_path(after, path);
    let reason = correction.reason.trim_end_matches('.');
    match (&correction.used, &correction.in_file) {
        (Some(used), Some(written)) => anyhow::bail!(
            "dux would use {used} instead of {written} for {shown} ({reason}); nothing was \
             changed."
        ),
        _ => anyhow::bail!(
            "that change would make dux drop {shown} when it loads the file ({reason}), so it \
             would not be set; nothing was changed."
        ),
    }
}

/// The refusal while the file `raw` has a web UI password setting where dux
/// does not read it (see [`crate::config::misplaced_auth_problem_list`]):
/// dux cannot tell `what`, so no `[server.auth]` write is made until it is
/// fixed. `None` when nothing is misplaced. Each problem is named by its
/// place, never its value.
fn misplaced_auth_refusal(raw: &str, what: &str) -> Option<String> {
    let misplaced: Vec<String> = crate::config::misplaced_auth_problem_list(raw)
        .into_iter()
        .map(|problem| problem.message)
        .collect();
    (!misplaced.is_empty()).then(|| {
        format!(
            "dux cannot tell {what} while config.toml has a web UI password setting where dux \
             does not read it ({}). Fix that first; nothing was changed.",
            misplaced.join("; ")
        )
    })
}

/// Before one field of a provider the file does not list is set: a
/// provider dux ships is written as dux runs it (every default field), so
/// the set changes that one field and nothing else, rather than leaving a
/// table that reads back with an empty command. A provider dux does not
/// know needs its command first. Providers are the one map of tables `set`
/// can add an entry to (`env` holds plain values), so this is the one place
/// a partial entry could read back as defaults.
fn prepare_provider(doc: &mut DocumentMut, path: &[String]) -> Result<()> {
    let [section, name, field] = path else {
        return Ok(());
    };
    if section != "providers" {
        return Ok(());
    }
    let listed = doc
        .get("providers")
        .and_then(Item::as_table_like)
        .and_then(|providers| providers.get(name))
        .is_some();
    if listed {
        return Ok(());
    }
    let shipped = crate::config::default_provider_commands()
        .into_iter()
        .find(|(shipped, _)| shipped == name)
        .map(|(_, config)| config);
    let Some(shipped) = shipped else {
        if field == "command" {
            return Ok(());
        }
        anyhow::bail!(
            "providers.{name} is not a provider dux knows, so it needs a command first: run \
             `dux config set providers.{name}.command <command>`, then set {field}. Nothing was \
             written."
        );
    };
    let text = toml::to_string(&shipped).context("failed to render the provider's defaults")?;
    let defaults: DocumentMut = text
        .parse()
        .context("failed to read the provider's defaults")?;
    for (key, item) in defaults.iter() {
        if let Some(value) = item.as_value() {
            set_in_doc(
                doc,
                &["providers".to_string(), name.clone(), key.to_string()],
                value.clone(),
            )?;
        }
    }
    Ok(())
}

/// After a provider field is set: the provider must still have a command,
/// or dux could not start it.
/// What a provider's entry says of its command, before or after a set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProviderCommand {
    /// The file does not list the provider.
    NoEntry,
    /// It lists it, without a command (or with an empty one).
    Missing,
    /// It lists it with a command.
    Present,
}

/// [`ProviderCommand`] for the provider `path` is a field of, read on its own
/// (a problem in another field is that field's); `None` for any other path.
fn provider_command_in(doc: &DocumentMut, path: &[String]) -> Option<ProviderCommand> {
    let [section, name, _] = path else {
        return None;
    };
    if section != "providers" {
        return None;
    }
    let file: toml::Table = toml::from_str(&doc.to_string()).ok()?;
    let Some(provider) = file
        .get("providers")
        .and_then(|providers| providers.get(name))
    else {
        return Some(ProviderCommand::NoEntry);
    };
    let command = provider.get("command").and_then(toml::Value::as_str);
    Some(
        if command.is_some_and(|command| !command.trim().is_empty()) {
            ProviderCommand::Present
        } else {
            ProviderCommand::Missing
        },
    )
}

/// After a provider field is set: a provider must not lose its command, and
/// a set must not create a provider entry without one. Attributed like every
/// other rule: a provider the file already lists without a command (`had`)
/// never blocks a set of its other fields, so they can still be repaired.
fn check_provider_command(
    doc: &DocumentMut,
    path: &[String],
    had: Option<ProviderCommand>,
) -> Result<()> {
    let Some(had) = had else {
        return Ok(());
    };
    let [_, name, _] = path else {
        return Ok(());
    };
    let now = provider_command_in(doc, path);
    let answerable = matches!(had, ProviderCommand::Present | ProviderCommand::NoEntry);
    if answerable && now != Some(ProviderCommand::Present) {
        anyhow::bail!(
            "that would leave providers.{name} with no command, so dux could not start it. \
             Nothing was written."
        );
    }
    Ok(())
}

/// What `doc` holds at `path`, however it is written (a value, an inline
/// table, a `[section]`, an array of tables), as `set` reports what it
/// replaced, through the one value printer: a value that is or holds a table
/// at any depth is said to be one, never printed, since its keys are names
/// the file chose. `None` only when nothing is there.
fn value_in_doc(doc: &DocumentMut, path: &[String]) -> Option<String> {
    let mut item = doc.as_item();
    for segment in path {
        item = item.as_table_like()?.get(segment)?;
    }
    if item.is_none() {
        return None;
    }
    Some(
        toml_value(item)
            .and_then(|value| printed_value(&doc.to_string(), path, &value, ValueForm::Line))
            .unwrap_or_else(|| NOT_SHOWN.to_string()),
    )
}

/// `item` as a plain TOML value, read back from the text it writes.
fn toml_value(item: &Item) -> Option<toml::Value> {
    let mut holder = DocumentMut::new();
    holder.insert("value", item.clone());
    toml::from_str::<toml::Table>(&holder.to_string())
        .ok()?
        .remove("value")
}

/// Why a password was not set.
#[derive(Debug)]
pub enum SetPasswordError {
    /// It misses a configured minimum; the check says which and how strong
    /// it is.
    BelowMinimums(MinimumCheck),
    /// Reading the policy, hashing or writing failed.
    Failed(anyhow::Error),
    /// The file's password hash is no longer the one the caller checked: the
    /// password was set or changed meanwhile by someone else. Nothing written.
    ChangedMeanwhile,
}

impl fmt::Display for SetPasswordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // A password no browser can type is refused on that alone: its
            // length and strength are beside the point.
            Self::BelowMinimums(check)
                if check
                    .failures
                    .contains(&crate::auth::MinimumFailure::ControlCharacter) =>
            {
                write!(
                    f,
                    "{}. Nothing was changed.",
                    crate::auth::MinimumFailure::ControlCharacter
                )
            }
            Self::BelowMinimums(check) => {
                let reasons: Vec<String> = check.failures.iter().map(ToString::to_string).collect();
                write!(f, "that password was not set: {}", reasons.join("; and "))?;
                if let Some(hint) = &check.strength.hint {
                    write!(f, ". {hint}")?;
                }
                Ok(())
            }
            Self::Failed(error) => write!(f, "{error:#}"),
            Self::ChangedMeanwhile => f.write_str(
                "the password was changed by someone else while this change was being made, \
                 so nothing was written; try again",
            ),
        }
    }
}

impl std::error::Error for SetPasswordError {}

/// The settings that make up the password policy.
const PASSWORD_POLICY_KEYS: [&str; 3] = [
    "minimum_password_length",
    "minimum_password_score",
    "max_password_bytes",
];

/// The password minimums `config_path` asks for right now, read key by key
/// from the file, so a broken key elsewhere in `[server.auth]` never changes
/// them. A file with no `[server.auth]` (or none at all) asks for the
/// defaults. A policy key that does not read, or breaks its rule, is an
/// error naming it: dux cannot tell what a new password has to meet, so it
/// sets none until that key is fixed.
pub fn current_password_policy(config_path: &Path) -> Result<crate::auth::PasswordPolicy> {
    match std::fs::read_to_string(config_path) {
        Ok(raw) => password_policy_of(&raw),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(crate::config::ServerAuthConfig::default().password_policy())
        }
        Err(error) => {
            Err(error).with_context(|| format!("failed to read {}", config_path.display()))
        }
    }
}

/// [`current_password_policy`] of the file's text `raw`.
fn password_policy_of(raw: &str) -> Result<crate::auth::PasswordPolicy> {
    let defaults = crate::config::ServerAuthConfig::default().password_policy();
    let file: toml::Table = toml::from_str(raw).map_err(|e| {
        anyhow::anyhow!(
            "config.toml is not valid TOML ({}), so dux cannot tell what a new password has to \
             meet; fix it first",
            crate::config::describe_toml_error(raw, &e)
        )
    })?;
    // An auth setting written where dux does not read it (a minimum spelled
    // `minimum-password-length`, or written one level too high) may be the
    // minimum the user meant, so no policy can be read with any confidence
    // until it is fixed: checking against the defaults could let through a
    // password the user's own minimum would refuse.
    if let Some(refusal) = misplaced_auth_refusal(raw, "what a new password has to meet") {
        anyhow::bail!("{refusal}");
    }
    let Some(server) = file.get("server") else {
        return Ok(defaults);
    };
    let auth = match server.as_table() {
        Some(server) => server.get("auth"),
        None => anyhow::bail!(
            "[server] in config.toml is not a table, so dux cannot tell what a new password \
             has to meet; fix it first"
        ),
    };
    let Some(auth) = auth else {
        return Ok(defaults);
    };
    let Some(auth) = auth.as_table() else {
        anyhow::bail!(
            "server.auth in config.toml is not a table, so dux cannot tell what a new password \
             has to meet; fix it first"
        );
    };
    let policy: toml::Table = PASSWORD_POLICY_KEYS
        .iter()
        .filter_map(|key| auth.get(*key).map(|value| (key.to_string(), value.clone())))
        .collect();
    let problems: Vec<String> =
        crate::config_auth::rule_problems_of(toml::Value::Table(policy.clone()))
            .into_iter()
            .map(|problem| problem.message)
            .collect();
    if !problems.is_empty() {
        anyhow::bail!(
            "the password policy in [server.auth] is invalid ({}), so dux cannot tell what a \
             new password has to meet. Fix that first with `dux config set \
             server.auth.<setting> <value>`; no password was set.",
            problems.join("; ")
        );
    }
    crate::config::parse_auth_value(toml::Value::Table(policy))
        .map(|auth| auth.password_policy())
        .map_err(anyhow::Error::msg)
}

/// A password that was stored.
#[derive(Debug)]
pub struct PasswordSet {
    /// How strong it is.
    pub strength: Strength,
    /// What still stops dux starting with the file (see
    /// [`SetReport::remaining_problems`]): a surface any of them stops keeps
    /// its old password until they are fixed.
    pub remaining_problems: Vec<crate::config::StartProblem>,
}

/// Check `password` against the minimums in `config_path`, hash it, and store
/// the hash at `server.auth.password_hash` through the coordinated mutation
/// path. The hash is made before the file lock is taken, so a slow hash never
/// holds up another writer; the minimums are read again under the lock, from
/// the very text the hash is written into, and the password checked against
/// them once more, so minimums raised meanwhile can never let a weaker
/// password through.
pub fn set_password(
    config_path: &Path,
    password: &Password,
    user_inputs: &[&str],
) -> Result<PasswordSet, SetPasswordError> {
    set_password_with(
        config_path,
        MissingConfig::CreateDocumented,
        password,
        user_inputs,
    )
}

/// [`set_password`] with a choice of what a missing file means.
pub fn set_password_with(
    config_path: &Path,
    missing: MissingConfig<'_>,
    password: &Password,
    user_inputs: &[&str],
) -> Result<PasswordSet, SetPasswordError> {
    let policy = current_password_policy(config_path).map_err(SetPasswordError::Failed)?;
    let check = crate::auth::check_minimums(password, &policy, user_inputs);
    if !check.passes() {
        return Err(SetPasswordError::BelowMinimums(check));
    }
    let hash = crate::auth::hash_password(password)
        .map_err(|e| SetPasswordError::Failed(anyhow::Error::msg(e.to_string())))?;
    let path: Vec<String> = password_hash_path();
    // The check that decides is the one under the lock.
    let mut decided: Option<crate::auth::MinimumCheck> = None;
    let written = write_value_checked(config_path, missing, &path, Value::from(hash), |doc| {
        let policy = password_policy_of(&doc.to_string())?;
        let check = crate::auth::check_minimums(password, &policy, user_inputs);
        let passes = check.passes();
        decided = Some(check);
        if passes {
            Ok(())
        } else {
            anyhow::bail!("the password does not meet the minimums the file sets now")
        }
    });
    match (written, decided) {
        (Ok(report), Some(check)) => Ok(PasswordSet {
            strength: check.strength,
            remaining_problems: report.remaining_problems,
        }),
        (Err(_), Some(check)) if !check.passes() => Err(SetPasswordError::BelowMinimums(check)),
        (Err(error), _) => Err(SetPasswordError::Failed(error)),
        (Ok(_), None) => Err(SetPasswordError::Failed(anyhow::anyhow!(
            "the password was written without being checked against the file's minimums"
        ))),
    }
}

/// [`set_password`], but only over the hash the caller checked: `expected` is
/// the `password_hash` the caller verified the current password against (empty
/// when it found none, for a first password). The comparison and the write
/// happen under the config write lock, so a change that lands between the
/// caller's check and this write makes this one fail with
/// [`SetPasswordError::ChangedMeanwhile`] rather than silently replacing it.
///
/// The write is [`set_password`]'s own: the locked, repairing, start-checked
/// mutation, so it adds no problem the file did not have and reports the ones
/// left.
pub fn set_password_if_current(
    config_path: &Path,
    expected: &str,
    password: &Password,
    user_inputs: &[&str],
) -> Result<PasswordSet, SetPasswordError> {
    let policy = current_password_policy(config_path).map_err(SetPasswordError::Failed)?;
    let check = crate::auth::check_minimums(password, &policy, user_inputs);
    if !check.passes() {
        return Err(SetPasswordError::BelowMinimums(check));
    }
    let hash = crate::auth::hash_password(password)
        .map_err(|e| SetPasswordError::Failed(anyhow::Error::msg(e.to_string())))?;
    let path = password_hash_path();
    let written = crate::config_write::mutate_config_file_repairing(
        config_path,
        MissingConfig::CreateDocumented,
        &path,
        |doc| {
            let current = value_in_doc(doc, &path)
                .and_then(|text| text.parse::<toml::Value>().ok())
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default();
            if current != expected {
                return Err(anyhow::Error::new(Unchanged));
            }
            set_in_doc(doc, &path, Value::from(hash))
        },
    );
    match written {
        Ok(((), remaining_problems)) => Ok(PasswordSet {
            strength: check.strength,
            remaining_problems,
        }),
        Err(error) if error.is::<Unchanged>() => Err(SetPasswordError::ChangedMeanwhile),
        Err(error) => Err(SetPasswordError::Failed(error)),
    }
}

/// A mutation that decided to write nothing; it never reaches a caller.
#[derive(Debug)]
struct Unchanged;

impl fmt::Display for Unchanged {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("nothing to change")
    }
}

impl std::error::Error for Unchanged {}

/// What [`append_blocked_address`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BanWrite {
    /// The address was appended to `blocked_addresses`.
    Written,
    /// An entry already covers it (the address itself or a range holding it).
    AlreadyBlocked,
    /// The list already holds `max_blocked_addresses` entries, so nothing was
    /// written; the caller keeps the ban in memory and says so.
    AtLimit,
}

/// Append `ip` to `[server.auth] blocked_addresses` through the coordinated
/// mutation path, keeping every comment and every entry another writer added
/// meanwhile. The repairing path: a problem the file already had elsewhere
/// never refuses a ban, while one the append would add does. An IPv4-mapped IPv6 address is written as the IPv4 address it
/// carries. `max_entries` is `max_blocked_addresses`: a list already that long
/// is left alone.
pub fn append_blocked_address(
    config_path: &Path,
    ip: std::net::IpAddr,
    max_entries: u32,
) -> Result<BanWrite> {
    let ip = crate::config_auth::canonical(ip);
    append_blocked_entry(config_path, ip, &ip.to_string(), max_entries)
}

/// [`append_blocked_address`] writing `entry` (the address itself, or a
/// range holding it) for `ip`. Nothing is written when an entry already
/// covers all of `entry`, and a range takes the place of the single-address
/// entries inside it in the same save.
pub fn append_blocked_entry(
    config_path: &Path,
    ip: std::net::IpAddr,
    entry: &str,
    max_entries: u32,
) -> Result<BanWrite> {
    let ip = crate::config_auth::canonical(ip);
    let new_entry = entry.to_string();
    // The span the new entry covers: the address itself, or the whole range.
    let (first, last) = entry_span(entry).unwrap_or((ip, ip));
    let blocked_path: Vec<String> = ["server", "auth", "blocked_addresses"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let written = crate::config_write::mutate_config_file_repairing(
        config_path,
        MissingConfig::CreateDocumented,
        &blocked_path,
        |doc| {
            let mut array = doc
                .get("server")
                .and_then(|server| server.get("auth"))
                .and_then(|auth| auth.get("blocked_addresses"))
                .and_then(|item| item.as_array())
                .cloned()
                .unwrap_or_default();
            let entries: Vec<String> = array
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            // Nothing is written when an entry already covers all of it: the
            // address, or both ends of the range (an entry is one contiguous
            // range, so covering both ends covers everything between).
            let covered = entries.iter().any(|entry| {
                crate::config_auth::AddressBlock::parse(entry)
                    .is_ok_and(|block| block.contains(first) && block.contains(last))
            });
            if covered {
                return Err(anyhow::Error::new(BanNoop(BanWrite::AlreadyBlocked)));
            }
            // Folded on write: a range takes the place of the single-address
            // entries inside it, in the same save, so the list gets shorter
            // and nobody refused before is let in. Every other entry, and its
            // comments, stays as it was.
            let folds = |value: &toml_edit::Value| {
                first != last
                    && value
                        .as_str()
                        .and_then(single_address)
                        .is_some_and(|single| in_span(single, first, last))
            };
            let folded = array.iter().filter(|value| folds(value)).count();
            if entries.len() - folded >= max_entries as usize {
                return Err(anyhow::Error::new(BanNoop(BanWrite::AtLimit)));
            }
            array.retain(|value| !folds(value));
            array.push(new_entry);
            set_in_doc(doc, &blocked_path, Value::Array(array))
        },
    );
    match written {
        Ok(((), _)) => Ok(BanWrite::Written),
        Err(error) => match error.downcast_ref::<BanNoop>() {
            Some(BanNoop(outcome)) => Ok(*outcome),
            None => Err(error),
        },
    }
}

/// The first and last address a `blocked_addresses` entry covers, when it
/// reads as an address or a CIDR range.
fn entry_span(entry: &str) -> Option<(std::net::IpAddr, std::net::IpAddr)> {
    use std::net::IpAddr;
    let (addr, prefix) = match entry.split_once('/') {
        Some((addr, prefix)) => (addr, Some(prefix.parse::<u32>().ok()?)),
        None => (entry, None),
    };
    let ip = crate::config_auth::canonical(addr.parse::<IpAddr>().ok()?);
    let bits = match ip {
        IpAddr::V4(_) => 32,
        IpAddr::V6(_) => 128,
    };
    let prefix = prefix.unwrap_or(bits).min(bits);
    let host = if prefix == bits {
        0
    } else {
        (1u128 << (bits - prefix)) - 1
    };
    Some(match ip {
        IpAddr::V4(v4) => {
            let n = u128::from(u32::from(v4));
            let (lo, hi) = (n & !host, n | host);
            (
                IpAddr::V4(std::net::Ipv4Addr::from(lo as u32)),
                IpAddr::V4(std::net::Ipv4Addr::from(hi as u32)),
            )
        }
        IpAddr::V6(v6) => {
            let n = u128::from(v6);
            (
                IpAddr::V6(std::net::Ipv6Addr::from(n & !host)),
                IpAddr::V6(std::net::Ipv6Addr::from(n | host)),
            )
        }
    })
}

/// An entry that names exactly one address, as that address.
fn single_address(entry: &str) -> Option<std::net::IpAddr> {
    entry_span(entry)
        .filter(|(first, last)| first == last)
        .map(|(first, _)| first)
}

/// Whether `ip` lies between `first` and `last`, of the same family.
fn in_span(ip: std::net::IpAddr, first: std::net::IpAddr, last: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match (ip, first, last) {
        (IpAddr::V4(ip), IpAddr::V4(first), IpAddr::V4(last)) => first <= ip && ip <= last,
        (IpAddr::V6(ip), IpAddr::V6(first), IpAddr::V6(last)) => first <= ip && ip <= last,
        _ => false,
    }
}

/// A ban that needs no write; it never reaches a caller.
#[derive(Debug)]
struct BanNoop(BanWrite);

impl fmt::Display for BanNoop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

impl std::error::Error for BanNoop {}

/// Store a [`SecretKind::Text`] value (an environment value) as given,
/// through the coordinated mutation path. The caller never prints it.
/// Returns what still stops dux starting with the file.
pub fn set_secret_text(
    config_path: &Path,
    key: &Key,
    value: &Password,
) -> Result<Vec<crate::config::StartProblem>> {
    set_secret_text_with(config_path, MissingConfig::CreateDocumented, key, value)
}

/// [`set_secret_text`] with a choice of what a missing file means.
pub fn set_secret_text_with(
    config_path: &Path,
    missing: MissingConfig<'_>,
    key: &Key,
    value: &Password,
) -> Result<Vec<crate::config::StartProblem>> {
    if key.policy != WritePolicy::Secret(SecretKind::Text) {
        anyhow::bail!("{} is not a setting stored as typed text", key.dotted());
    }
    write_value(config_path, missing, &key.path, Value::from(value.expose()))
        .map(|report| report.remaining_problems)
}

fn password_hash_path() -> Vec<String> {
    let WritePolicy::Secret(SecretKind::PasswordHash { stores_at }) = VIRTUAL_KEYS[0].1 else {
        unreachable!("the first virtual key is the password");
    };
    stores_at
        .iter()
        .map(|segment| (*segment).to_string())
        .collect()
}

/// What `get` found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GetValue {
    /// The file sets it; the TOML text of the value (a string unquoted).
    Set(String),
    /// The file leaves it out; dux uses this (as above), read through the
    /// same loader a start uses. Empty when that is what dux reads, such as
    /// the command of a provider the file lists without one.
    Default(String),
    /// The file leaves it out and dux has no value for it (an optional
    /// setting, or a provider or env entry that does not exist).
    Unset,
    /// dux cannot load the file, so the value it would use cannot be worked
    /// out: `in_file` is what the file's own text says, if anything, and
    /// `reason` is the problem that stops the load.
    Unknown {
        in_file: Option<String>,
        reason: String,
    },
}

/// A value dux uses in place of what the file says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Correction {
    /// The setting it is about, as segments: the key asked for, or an entry
    /// inside the table asked for.
    pub path: Vec<String>,
    /// What the file says, as TOML text (a string unquoted), or `None`
    /// when the file leaves the setting out and the load writes it from a
    /// deprecated key.
    pub in_file: Option<String>,
    /// What dux uses, or `None` when the load drops the entry holding it.
    pub used: Option<String>,
    /// The setting sits where the formatter names no key (see
    /// [`printed_value`]), so no value is printed for it: `in_file` is then
    /// `None` although the file has a value there, and `used` is
    /// [`NOT_SHOWN`] when dux uses one.
    pub value_hidden: bool,
    /// Why dux uses something else: the load's own sentence.
    pub reason: String,
    /// The one surface that uses something else, when only one does (the
    /// terminal UI's own `[keys]` migrations); `None` for every surface that
    /// starts with the file. The reason then says it all.
    pub surface: Option<crate::config::Surface>,
    /// The setting of the file the value came from, when it came from
    /// another one (a deprecated key, a retired binding).
    pub from: Option<Vec<String>>,
}

/// What `get` found: the value every surface that starts with the file
/// uses, what the load changes of what the file says (each with why), and
/// each surface that will not start with the file, with why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GetReport {
    /// The value in use. [`GetValue::Unknown`] only when no surface starts
    /// with the file.
    pub value: GetValue,
    /// For a single setting, at most one: what the file says when dux uses
    /// something else. For a table, one per entry inside it the load changes
    /// or drops.
    pub corrections: Vec<Correction>,
    /// The surfaces that will not start with the file, each with why: their
    /// value cannot be worked out. Empty when [`Self::value`] is unknown,
    /// which says why itself.
    pub refused_by: Vec<(crate::config::Surface, String)>,
    /// For a table whose printed form replaced a name that breaks its map's
    /// rule with a marker: the table as the file names its entries, which
    /// only `--show` prints.
    pub with_names: Option<String>,
}

/// The value dux uses for `key` with the file `raw` (see [`get_report`]).
pub fn get(raw: &str, key: &Key) -> Result<GetValue> {
    get_report(raw, key).map(|report| report.value)
}

/// The value a surface that starts with the file `raw` uses for `key`: what
/// the file says, after every correction and prune the load makes (an
/// out-of-range value reset, a retired provider's stock block dropped, a
/// wrong-typed value `dux server` reads as its default), or the default when
/// the file leaves it out. Every surface that starts with a file reads it
/// through the same load, so they use the same value; a surface that will
/// not start with it is named with why. A whole table is shown as dux uses
/// it: every entry, those the file leaves to their defaults (stock providers
/// among them) marked `# default`, with each entry the file writes that the
/// load changes or drops listed beside it. `[keys]` is the terminal UI's, so
/// it is what the terminal UI's own key resolution makes of the file.
/// Reads the text itself rather than a loaded config, so it works on a file
/// a surface would refuse to start with: `get` is how you look at the broken
/// part. For a [`WritePolicy::Secret`] key it reads where the secret is
/// stored (the password's hash).
pub fn get_report(raw: &str, key: &Key) -> Result<GetReport> {
    get_report_inner(raw, key, None)
}

/// [`get_report`] for the config file in dux's config directory `root`, so
/// a value that depends on files beside the config (the theme) is reported
/// as dux uses it too.
pub fn get_report_at(raw: &str, key: &Key, root: &Path) -> Result<GetReport> {
    get_report_inner(raw, key, Some(root))
}

/// Where `key`'s value is stored in the file: the key itself, or for the
/// password, where its hash goes.
fn stored_path(key: &Key) -> Vec<String> {
    match key.policy {
        WritePolicy::Secret(SecretKind::PasswordHash { stores_at }) => stores_at
            .iter()
            .map(|segment| (*segment).to_string())
            .collect(),
        WritePolicy::Secret(SecretKind::Text) | WritePolicy::Plain => key.path.clone(),
    }
}

/// What `get` prints, without `--show`, in place of the whole value of a key
/// it does not name.
pub const NOT_SHOWN: &str = "(not shown)";

/// A `[server.auth]` setting a summary or a preview never prints without
/// `--show`, though `get` prints it when asked for it by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensitiveAuthSetting {
    /// Not the password, but anyone holding it can guess the password
    /// offline as fast as their hardware allows.
    PasswordHash,
    /// Other people's IP addresses.
    BlockedAddresses,
}

/// Which [`SensitiveAuthSetting`] the key at `path` is, if any: the one
/// answer `dux config diff` and both previews read.
pub fn sensitive_auth_setting(path: &[String]) -> Option<SensitiveAuthSetting> {
    let segments: Vec<&str> = path.iter().map(String::as_str).collect();
    match segments.as_slice() {
        ["server", "auth", "password_hash"] => Some(SensitiveAuthSetting::PasswordHash),
        ["server", "auth", "blocked_addresses"] => Some(SensitiveAuthSetting::BlockedAddresses),
        _ => None,
    }
}

/// The user's config file `raw` as a printer may show it line by line (the
/// previews of `dux config regenerate` and `restore-docs` diff it): every
/// line dux can map to a key path goes through the same rules as the one
/// value printer. A plaintext password (each one [`crate::config::plaintext_passwords`]
/// finds, by its path and by its lines) is never shown, `show` or not;
/// without `show`, a value whose path is hidden or sensitive (anything under
/// an unknown key or a name that breaks its map's rule, an `[env]` value, a
/// project's values, a binding `binding_understood` refuses, a
/// [`SensitiveAuthSetting`]) is its key, as
/// the formatter prints it, and `(not shown)`, and a comment inside a hidden
/// or sensitive table, a header's trailing one included, is a placeholder
/// too, since a comment can hold a pasted token as well. Every other line is
/// as the user wrote it.
///
/// An error, naming the parse problem by line and kind only, when `raw` is
/// not TOML: dux cannot tell where anything sits in such a file, a secret
/// included, so none of its text is shown, `show` or not.
pub fn shown_file_text(
    raw: &str,
    show: bool,
    binding_understood: &dyn Fn(&str) -> bool,
) -> Result<String> {
    use crate::config::PathStep as Step;
    /// What a line holds.
    enum Entry {
        Header {
            line: usize,
            path: Vec<Step>,
            /// Whether it opens an entry of an array of tables.
            array: bool,
            /// The header itself, `[` to `]`, as written.
            text: Option<std::ops::Range<usize>>,
        },
        Value {
            start: usize,
            end: usize,
            path: Vec<Step>,
            value: toml_edit::Value,
        },
    }
    /// How a value line is shown.
    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    enum Mask {
        Shown,
        Hidden,
        Plaintext,
    }
    fn parts(path: &[Step]) -> Vec<crate::config::PathPart<'_>> {
        path.iter().map(Step::as_part).collect()
    }
    fn keys(path: &[Step]) -> Vec<String> {
        path.iter()
            .filter_map(|step| match step {
                Step::Key(key) => Some(key.clone()),
                Step::Index(_) => None,
            })
            .collect()
    }
    if let Err(error) = toml::from_str::<toml::Table>(raw) {
        anyhow::bail!(
            "config.toml is not valid TOML ({}), so dux cannot tell where anything sits in it, a \
             secret included, and shows none of its lines, not even with --show; fix it first",
            crate::config::describe_toml_error(raw, &error)
        );
    }
    let doc = toml_edit::Document::parse(raw).map_err(|error| {
        anyhow::anyhow!(
            "config.toml is not valid TOML ({}), so dux cannot tell where anything sits in it, a \
             secret included, and shows none of its lines, not even with --show; fix it first",
            crate::config::describe_toml_edit_error(raw, &error)
        )
    })?;
    let passwords = crate::config::plaintext_passwords(raw);
    let starts: Vec<usize> = std::iter::once(0)
        .chain(raw.match_indices('\n').map(|(at, _)| at + 1))
        .collect();
    let line_of = |offset: usize| match starts.binary_search(&offset) {
        Ok(index) => index,
        Err(index) => index.saturating_sub(1),
    };
    fn walk(
        table: &toml_edit::Table,
        path: &mut Vec<Step>,
        out: &mut Vec<Entry>,
        line_of: &dyn Fn(usize) -> usize,
    ) {
        for (key, item) in table.iter() {
            path.push(Step::Key(key.to_string()));
            match item {
                toml_edit::Item::Value(value) => {
                    let start = table
                        .key(key)
                        .and_then(toml_edit::Key::span)
                        .or_else(|| value.span())
                        .map(|span| line_of(span.start));
                    let end = value.span().map(|span| line_of(span.end.saturating_sub(1)));
                    if let Some(start) = start {
                        out.push(Entry::Value {
                            start,
                            end: end.unwrap_or(start).max(start),
                            path: path.clone(),
                            value: value.clone(),
                        });
                    }
                }
                toml_edit::Item::Table(child) => {
                    if let Some(span) = child.span() {
                        out.push(Entry::Header {
                            line: line_of(span.start),
                            path: path.clone(),
                            array: false,
                            text: Some(span),
                        });
                    }
                    walk(child, path, out, line_of);
                }
                toml_edit::Item::ArrayOfTables(entries) => {
                    for (index, child) in entries.iter().enumerate() {
                        path.push(Step::Index(index));
                        if let Some(span) = child.span() {
                            out.push(Entry::Header {
                                line: line_of(span.start),
                                path: path.clone(),
                                array: true,
                                text: Some(span),
                            });
                        }
                        walk(child, path, out, line_of);
                        path.pop();
                    }
                }
                toml_edit::Item::None => {}
            }
            path.pop();
        }
    }
    let mut entries = Vec::new();
    walk(doc.as_table(), &mut Vec::new(), &mut entries, &line_of);
    // A path whose values a printer holds back without `show`.
    let held_back = |path: &[Step]| -> bool {
        let keys = keys(path);
        crate::config::first_hidden_part_of(&parts(path))
            || matches!(keys.first().map(String::as_str), Some("env" | "projects"))
            || sensitive_auth_setting(&keys).is_some()
    };
    // Whether a plaintext password sits at, below or above `path`.
    let plaintext = |path: &[Step]| {
        passwords
            .iter()
            .any(|password| password.steps.starts_with(path) || path.starts_with(&password.steps))
    };
    // What a value at `path` (and every key inside it) makes of its line.
    fn mask_of(
        path: &[Step],
        value: &toml_edit::Value,
        show: bool,
        held_back: &dyn Fn(&[Step]) -> bool,
        binding_understood: &dyn Fn(&str) -> bool,
    ) -> Mask {
        let mut mask = Mask::Shown;
        if !show && held_back(path) {
            mask = Mask::Hidden;
        }
        let key_path = keys(path);
        if !show
            && key_path.len() == 2
            && key_path[0] == "keys"
            && match value {
                toml_edit::Value::String(text) => !binding_understood(text.value()),
                toml_edit::Value::Array(items) => items
                    .iter()
                    .any(|item| item.as_str().is_some_and(|text| !binding_understood(text))),
                _ => false,
            }
        {
            mask = Mask::Hidden;
        }
        match value {
            toml_edit::Value::InlineTable(table) => {
                for (key, child) in table.iter() {
                    let mut at = path.to_vec();
                    at.push(Step::Key(key.to_string()));
                    mask = mask.max(mask_of(&at, child, show, held_back, binding_understood));
                }
            }
            toml_edit::Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    let mut at = path.to_vec();
                    at.push(Step::Index(index));
                    mask = mask.max(mask_of(&at, item, show, held_back, binding_understood));
                }
            }
            _ => {}
        }
        mask
    }
    let lines: Vec<&str> = raw.split_inclusive('\n').collect();
    let mut shown: Vec<Option<String>> =
        lines.iter().map(|line| Some((*line).to_string())).collect();
    let newline = |line: &str| if line.ends_with('\n') { "\n" } else { "" };
    let mut headers: Vec<(usize, Vec<Step>)> = Vec::new();
    let mut in_value = vec![false; lines.len()];
    let mut is_header = vec![false; lines.len()];
    for entry in &entries {
        match entry {
            Entry::Header {
                line,
                path,
                array,
                text,
            } => {
                headers.push((*line, path.clone()));
                let Some(line_text) = lines.get(*line) else {
                    continue;
                };
                is_header[*line] = true;
                if show || !held_back(path) {
                    continue;
                }
                // Held back: the header as dux names it, and never the
                // comment after it.
                let (open, close) = if *array { ("[[", "]]") } else { ("[", "]") };
                let header = match text {
                    Some(span) if !crate::config::first_hidden_part_of(&parts(path)) => {
                        raw[span.clone()].trim().to_string()
                    }
                    _ => {
                        let named = if *array {
                            &path[..path.len().saturating_sub(1)]
                        } else {
                            &path[..]
                        };
                        format!(
                            "{open}{}{close}",
                            crate::config::shown_parts(raw, &parts(named))
                        )
                    }
                };
                let after = text
                    .as_ref()
                    .map_or(*line_text, |span| raw.get(span.end..).unwrap_or_default());
                let after = after.split('\n').next().unwrap_or_default();
                let comment = if after.contains('#') {
                    " # (comment not shown)"
                } else {
                    ""
                };
                shown[*line] = Some(format!("{header}{comment}{}", newline(line_text)));
            }
            Entry::Value {
                start,
                end,
                path,
                value,
            } => {
                for line in *start..=*end {
                    if let Some(flag) = in_value.get_mut(line) {
                        *flag = true;
                    }
                }
                let mask = if plaintext(path) {
                    Mask::Plaintext
                } else {
                    mask_of(path, value, show, &held_back, binding_understood)
                };
                if mask == Mask::Shown {
                    continue;
                }
                let Some(text) = lines.get(*start) else {
                    continue;
                };
                let indent: String = text
                    .chars()
                    .take_while(|c| c.is_whitespace() && *c != '\n')
                    .collect();
                let key_text = if crate::config::first_hidden_part_of(&parts(path)) {
                    crate::config::shown_parts(raw, &parts(path))
                } else {
                    text.split_once('=').map_or_else(
                        || text.trim().to_string(),
                        |(key, _)| key.trim().to_string(),
                    )
                };
                let placeholder = match mask {
                    Mask::Plaintext => crate::config::PLAINTEXT_PASSWORD_NOT_SHOWN,
                    _ => NOT_SHOWN,
                };
                // The value's own trailing comment goes with it.
                let commented = value
                    .decor()
                    .suffix()
                    .and_then(|suffix| {
                        suffix
                            .span()
                            .and_then(|span| raw.get(span))
                            .or_else(|| suffix.as_str())
                    })
                    .is_some_and(|suffix| suffix.contains('#'));
                let comment = if commented {
                    " # (comment not shown)"
                } else {
                    ""
                };
                shown[*start] = Some(format!(
                    "{indent}{key_text} = {placeholder}{comment}{}",
                    newline(text)
                ));
                // A value written over several lines is one line here.
                for line in start + 1..=*end {
                    if let Some(slot) = shown.get_mut(line) {
                        *slot = None;
                    }
                }
            }
        }
    }
    // Every line a plaintext password spans is masked above, by its path;
    // one still as written (which no shape dux knows leaves) goes too.
    for password in &passwords {
        for line in password.lines.clone() {
            if is_header.get(line) == Some(&false)
                && let (Some(Some(now)), Some(was)) = (shown.get(line), lines.get(line))
                && now == was
                && !now.trim().is_empty()
            {
                shown[line] = Some(format!(
                    "# {}{}",
                    crate::config::PLAINTEXT_PASSWORD_NOT_SHOWN,
                    newline(was)
                ));
            }
        }
    }
    // A comment belongs to the table whose header comes last before it.
    if !show {
        headers.sort_by_key(|(line, _)| *line);
        for (index, text) in lines.iter().enumerate() {
            if in_value[index] || !text.trim_start().starts_with('#') {
                continue;
            }
            let table = headers.iter().rev().find(|(line, _)| *line < index);
            if let Some((_, path)) = table
                && held_back(path)
            {
                shown[index] = Some(format!("# (comment not shown){}", newline(text)));
            }
        }
    }
    Ok(shown.into_iter().flatten().collect())
}

/// What `set` prints in place of a value that is, or holds, a table.
pub const A_TABLE_NOT_SHOWN: &str = "(a table, not shown)";

/// How [`printed_value`] writes a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueForm {
    /// As `get` prints it: a string as its text, a table as the lines of
    /// TOML it is.
    Get,
    /// As one line of TOML, as `set` reports what it replaced and wrote. A
    /// value that is, or holds at any depth, a table is
    /// [`A_TABLE_NOT_SHOWN`]: a table's keys are names the file chose.
    Line,
    /// As `dux config diff` summarizes it: one line, strings unquoted, a list
    /// as `[a, b]`, and a value that is or holds a table
    /// [`A_TABLE_NOT_SHOWN`].
    Summary,
}

/// THE value printer: every value dux prints from a config file goes through
/// here, with the full key path of where it sits in the file `raw`.
///
/// `None` when any segment of `path` is one the formatter does not name (a
/// key the schema has no place for, a name that breaks its map's rule): such
/// a key may be a token pasted where a name goes, and its value then may be
/// too, so nothing at or below it is printed and the caller says the file has
/// a value there instead. Below a path it does name, every key it does not
/// name is a marker naming its line with its whole value [`NOT_SHOWN`],
/// arrays walked element by element at every depth.
pub fn printed_value(
    raw: &str,
    path: &[String],
    value: &toml::Value,
    form: ValueForm,
) -> Option<String> {
    if crate::config::path_is_hidden(path) {
        return None;
    }
    // Judged by the start check itself, never by a rule of the printer's own.
    let passwords = crate::config::plaintext_passwords_at(path, value);
    if passwords.iter().any(|password| is_at(password, path)) {
        return Some(crate::config::PLAINTEXT_PASSWORD_NOT_SHOWN.to_string());
    }
    Some(match form {
        ValueForm::Get => render(&names_hidden(raw, path, value, &passwords)),
        // A password below `path` sits in a table, which is never written
        // out on one line.
        ValueForm::Line | ValueForm::Summary if holds_a_table(value) || !passwords.is_empty() => {
            A_TABLE_NOT_SHOWN.to_string()
        }
        ValueForm::Line => value.to_string(),
        ValueForm::Summary => summarized(value),
    })
}

/// A value with no table in it, as [`ValueForm::Summary`] writes it.
fn summarized(value: &toml::Value) -> String {
    match value {
        toml::Value::String(text) => text.clone(),
        toml::Value::Array(items) => format!(
            "[{}]",
            items.iter().map(summarized).collect::<Vec<_>>().join(", ")
        ),
        other => other.to_string(),
    }
}

/// Whether the plaintext password at `steps` is the key at `path` itself.
fn is_at(steps: &[crate::config::PathStep], path: &[String]) -> bool {
    steps.len() == path.len()
        && steps
            .iter()
            .zip(path)
            .all(|(step, key)| matches!(step, crate::config::PathStep::Key(at) if at == key))
}

/// `value`, at `path`, with every plaintext password the start check finds
/// in it (see [`crate::config::plaintext_passwords_at`]) replaced by the
/// words that say what it is: the one rendering `--show` uses, which shows
/// every name.
fn plaintext_masked(path: &[String], value: &toml::Value) -> toml::Value {
    use crate::config::PathStep;
    fn masked(
        at: &mut Vec<PathStep>,
        value: &toml::Value,
        passwords: &[Vec<PathStep>],
    ) -> toml::Value {
        if passwords.iter().any(|password| password == at) {
            return toml::Value::String(crate::config::PLAINTEXT_PASSWORD_NOT_SHOWN.to_string());
        }
        match value {
            toml::Value::Table(table) => toml::Value::Table(
                table
                    .iter()
                    .map(|(key, child)| {
                        at.push(PathStep::Key(key.clone()));
                        let child = masked(at, child, passwords);
                        at.pop();
                        (key.clone(), child)
                    })
                    .collect(),
            ),
            toml::Value::Array(items) => toml::Value::Array(
                items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| {
                        at.push(PathStep::Index(index));
                        let item = masked(at, item, passwords);
                        at.pop();
                        item
                    })
                    .collect(),
            ),
            other => other.clone(),
        }
    }
    let passwords = crate::config::plaintext_passwords_at(path, value);
    let mut at = path.iter().map(|key| PathStep::Key(key.clone())).collect();
    masked(&mut at, value, &passwords)
}

/// Whether `value` is a table or holds one, in an array at any depth.
fn holds_a_table(value: &toml::Value) -> bool {
    match value {
        toml::Value::Table(_) => true,
        toml::Value::Array(items) => items.iter().any(holds_a_table),
        _ => false,
    }
}

/// `value`, at `path`, with every key below it the formatter does not name
/// replaced by a marker naming its line, whose whole value is
/// [`NOT_SHOWN`].
fn names_hidden(
    raw: &str,
    path: &[String],
    value: &toml::Value,
    passwords: &[Vec<crate::config::PathStep>],
) -> toml::Value {
    use crate::config::PathStep as Step;
    fn parts(path: &[Step]) -> Vec<crate::config::PathPart<'_>> {
        path.iter().map(Step::as_part).collect()
    }
    fn shown(
        raw: &str,
        path: &mut Vec<Step>,
        value: &toml::Value,
        passwords: &[Vec<Step>],
    ) -> toml::Value {
        match value {
            toml::Value::Table(table) => {
                let mut out = toml::Table::new();
                for (key, child) in table {
                    path.push(Step::Key(key.clone()));
                    let at = parts(path);
                    // Once a key is hidden, nothing below it is printed: its
                    // whole value (a table or an array can hold more names
                    // pasted in the wrong place) is the placeholder.
                    if crate::config::part_is_hidden(&at) {
                        out.insert(
                            format!("<{}>", crate::config::shown_parts(raw, &at)),
                            toml::Value::String(NOT_SHOWN.to_string()),
                        );
                    } else if passwords.iter().any(|password| password == path) {
                        // A plaintext password is never echoed.
                        out.insert(
                            key.clone(),
                            toml::Value::String(
                                crate::config::PLAINTEXT_PASSWORD_NOT_SHOWN.to_string(),
                            ),
                        );
                    } else {
                        out.insert(key.clone(), shown(raw, path, child, passwords));
                    }
                    path.pop();
                }
                toml::Value::Table(out)
            }
            toml::Value::Array(items) => toml::Value::Array(
                items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| {
                        path.push(Step::Index(index));
                        let item = shown(raw, path, item, passwords);
                        path.pop();
                        item
                    })
                    .collect(),
            ),
            _ => value.clone(),
        }
    }
    let mut start: Vec<Step> = path.iter().map(|key| Step::Key(key.clone())).collect();
    shown(raw, &mut start, value, passwords)
}

/// The table `in_use` (what dux uses at `path`) as `get` prints it: every
/// entry, each one the file `file` leaves out marked with where it came
/// from, and, with `hide_names`, every name that breaks its map's rule a
/// marker naming its line (see [`printed_value`]). The mark comes from the
/// same corrections the lines on stderr say (`corrections`), so the two
/// never disagree: an entry a correction wrote from another setting is
/// `# from <that setting>`, and only one no correction wrote is
/// `# default`.
fn render_in_use(
    raw: &str,
    path: &[String],
    in_use: &toml::Value,
    file: &toml::Value,
    corrections: &[Correction],
    hide_names: bool,
) -> String {
    let mut leaves = Vec::new();
    leaves_of(in_use, &mut path.to_vec(), &mut leaves);
    let marks: Vec<(Vec<String>, String)> = leaves
        .into_iter()
        .filter(|(at, _)| value_at(file, at).is_none())
        .map(|(at, _)| {
            let made = corrections.iter().find(|correction| correction.path == at);
            let mark = match made {
                Some(Correction {
                    from: Some(from), ..
                }) => format!(" # from {}", crate::config::shown_path(raw, from)),
                Some(_) => " # corrected, see below".to_string(),
                None => " # default".to_string(),
            };
            (at, mark)
        })
        .collect();
    let text = if hide_names {
        printed_value(raw, path, in_use, ValueForm::Get).unwrap_or_else(|| NOT_SHOWN.to_string())
    } else {
        // `--show` prints names as the file writes them, but never a
        // plaintext password.
        render(&plaintext_masked(path, in_use))
    };
    let Ok(mut doc) = text.parse::<DocumentMut>() else {
        return text;
    };
    fn mark(
        table: &mut dyn toml_edit::TableLike,
        at: &mut Vec<String>,
        marks: &[(Vec<String>, String)],
    ) {
        for (key, item) in table.iter_mut() {
            at.push(key.get().to_string());
            match item {
                Item::Value(value) => {
                    if let Some((_, mark)) = marks.iter().find(|(path, _)| path == at) {
                        value.decor_mut().set_suffix(mark.as_str());
                    }
                }
                Item::Table(child) => mark(child, at, marks),
                _ => {}
            }
            at.pop();
        }
    }
    mark(doc.as_table_mut(), &mut path.to_vec(), &marks);
    doc.to_string().trim_end().to_string()
}

fn get_report_inner(raw: &str, key: &Key, root: Option<&Path>) -> Result<GetReport> {
    use crate::config::Surface;
    let path = stored_path(key);
    let doc: toml::Table = toml::from_str(raw).map_err(|e| {
        anyhow::anyhow!(
            "config.toml is not valid TOML: {}",
            crate::config::describe_toml_error(raw, &e)
        )
    })?;
    let file = toml::Value::Table(doc);
    let node = value_at(&file, &path);
    // A table is printed through the one formatter: a name that breaks its
    // map's rule is a marker naming its line (see [`printed_value`]).
    let in_file = node.map(|node| {
        printed_value(raw, &path, node, ValueForm::Get).unwrap_or_else(|| NOT_SHOWN.to_string())
    });
    let problems = crate::config::start_problems_of(raw);
    let refusal = |surface: Surface| {
        let reasons: Vec<&str> = problems
            .iter()
            .filter(|problem| problem.stops(surface))
            .map(|problem| problem.detail.as_str())
            .collect();
        (!reasons.is_empty()).then(|| reasons.join("; "))
    };
    let refused_by: Vec<(Surface, String)> = [Surface::TerminalUi, Surface::DuxServer]
        .into_iter()
        .filter_map(|surface| refusal(surface).map(|why| (surface, why)))
        .collect();
    let unknown = |reason: String, in_file: Option<String>| GetReport {
        value: GetValue::Unknown { in_file, reason },
        corrections: Vec::new(),
        refused_by: Vec::new(),
        with_names: None,
    };
    // What a starting surface runs with: the file through the load a start
    // uses. A file that does not load stops every surface.
    let effective = match crate::config::effective_config_from_text(raw) {
        Ok(config) if refused_by.len() < 2 => config,
        Ok(_) => {
            let named: Vec<String> = problems.iter().map(|p| p.message.clone()).collect();
            return Ok(unknown(named.join("; "), in_file));
        }
        Err(problem) => {
            let named: Vec<String> = problems.iter().map(|p| p.message.clone()).collect();
            let reason = if named.is_empty() {
                problem.reason().to_string()
            } else {
                named.join("; ")
            };
            return Ok(unknown(reason, in_file));
        }
    };
    // `[keys]` is the terminal UI's alone: a file the terminal UI will not
    // start with has no binding in use anywhere.
    if path.first().map(String::as_str) == Some("keys")
        && refused_by
            .iter()
            .any(|(surface, _)| *surface == Surface::TerminalUi)
    {
        let named: Vec<String> = problems
            .iter()
            .filter(|problem| problem.stops(Surface::TerminalUi))
            .map(|problem| problem.message.clone())
            .collect();
        return Ok(unknown(named.join("; "), in_file));
    }
    // What dux makes of a value where it is used (a level it does not know
    // read as info, a ceiling, `0` meaning the default), by the same
    // `effective_*` function every runtime reader goes through.
    let use_time = crate::config_effective::use_time_corrections_at(&effective, root);
    let mut effective = serde_json::to_value(&effective).ok();
    for correction in &use_time {
        if let Some(slot) = correction
            .path
            .iter()
            .try_fold(effective.as_mut(), |node, segment| {
                Some(node.and_then(|node| node.get_mut(segment)))
            })
            .flatten()
        {
            *slot = correction.used.clone();
        }
    }
    // `[keys]` is the terminal UI's alone, so it is what the terminal UI
    // runs with, by its own resolution: its key migrations, and every
    // action's default where the file has none.
    let terminal_ui_starts = !refused_by
        .iter()
        .any(|(surface, _)| *surface == Surface::TerminalUi);
    let keys_resolved = terminal_ui_starts
        .then(|| crate::config::terminal_ui_keys(raw))
        .flatten();
    if let (Some(keys), Some(serde_json::Value::Object(map))) = (&keys_resolved, effective.as_mut())
        && let Ok(keys) = serde_json::to_value(keys)
    {
        map.insert("keys".to_string(), keys);
    }
    // The value in use at `at`, as TOML (a table without the nulls its unset
    // optional settings carry, which TOML has no way to write).
    let effective_at = |at: &[String]| -> Option<toml::Value> {
        let mut used = effective.clone();
        for segment in at {
            used = used.and_then(|n| n.get(segment).cloned());
        }
        match used {
            Some(serde_json::Value::Null) | None => None,
            Some(json) => toml::Value::try_from(without_nulls(json)).ok(),
        }
    };
    let used_at = |at: &[String]| effective_at(at).map(|value| render(&value));
    let used = used_at(&path);
    // Every path below is a list of segments, never a dotted string split
    // again: a name may hold a dot.
    let mut sourced = crate::config::load_corrections_with_sources(raw);
    sourced.extend(
        use_time
            .into_iter()
            .map(|correction| crate::config::LoadCorrection {
                path: correction.path,
                reason: correction.reason,
                from: None,
            }),
    );
    // The terminal UI's own `[keys]` migrations are corrections it alone
    // makes (see `surface_of` below).
    if keys_resolved.is_some() {
        sourced.extend(crate::config::terminal_ui_key_corrections_with_sources(raw));
    }
    let source_of = |at: &[String]| {
        sourced
            .iter()
            .find(|correction| correction.path == at)
            .and_then(|correction| correction.from.clone())
    };
    let corrected: Vec<(Vec<String>, String)> = sourced
        .iter()
        .map(|correction| (correction.path.clone(), correction.reason.clone()))
        .collect();
    // A correction under `[keys]` is the terminal UI's alone.
    let surface_of = |at: &[String]| {
        (at.first().map(String::as_str) == Some("keys")).then_some(Surface::TerminalUi)
    };
    let inside =
        |key: &[String], scope: &[String]| key.len() > scope.len() && key.starts_with(scope);
    let Some(in_file) = in_file else {
        // Left out of the file, but the load may still write it, from a
        // deprecated key; that is said with where it came from.
        let carried = used.as_ref().and_then(|_| {
            corrected
                .iter()
                .find(|(key, _)| *key == path)
                .map(|(_, reason)| Correction {
                    from: source_of(&path),
                    surface: surface_of(&path),
                    path: path.clone(),
                    in_file: None,
                    value_hidden: false,
                    used: used.clone(),
                    reason: reason.clone(),
                })
        });
        return Ok(GetReport {
            value: used.map_or(GetValue::Unset, GetValue::Default),
            corrections: carried.into_iter().collect(),
            refused_by,
            with_names: None,
        });
    };
    // What the load uses in place of what the file says is found by
    // comparing the two, for every key: the load's own record says why
    // (the correction or recovery step that covers it, a sibling included),
    // and a difference it has no record of is still reported.
    let covering = |key: &[String]| {
        corrected
            .iter()
            .find(|(scope, _)| key == scope.as_slice() || inside(key, scope))
    };
    let reason_at = |key: &[String]| {
        covering(key).map_or_else(
            || "that is how dux server's load reads the file".to_string(),
            |(_, reason)| reason.clone(),
        )
    };
    if let Some(table) = node.filter(|node| node.is_table()) {
        // A table is shown as dux uses it: every entry, the ones the file
        // leaves to their defaults (stock providers among them) marked as
        // defaults, and every entry the file writes whose value the load
        // changes or drops listed beside it. An entry dropped whole (a
        // provider, a retired block) is listed once, at the entry.
        let mut leaves = Vec::new();
        leaves_of(table, &mut path.clone(), &mut leaves);
        let mut corrections: Vec<Correction> = Vec::new();
        for (at, value) in leaves {
            if corrections
                .iter()
                .any(|done| done.used.is_none() && inside(&at, &done.path))
            {
                continue;
            }
            let in_file = render(value);
            let used = used_at(&at);
            if !compared(value) || used.as_deref() == Some(in_file.as_str()) {
                continue;
            }
            let whole = covering(&at).filter(|(scope, _)| {
                inside(&at, scope) && inside(scope, &path) && used_at(scope).is_none()
            });
            corrections.push(match whole {
                Some((scope, reason)) => Correction {
                    from: None,
                    surface: surface_of(scope),
                    path: scope.clone(),
                    // Through the one value printer: the entry's own keys may
                    // not all be setting names, nor the entry's name itself.
                    in_file: value_at(&file, scope)
                        .and_then(|value| printed_value(raw, scope, value, ValueForm::Get)),
                    used: None,
                    value_hidden: crate::config::path_is_hidden(scope),
                    reason: reason.clone(),
                },
                None => {
                    let value_hidden = crate::config::path_is_hidden(&at);
                    Correction {
                        from: None,
                        surface: surface_of(&at),
                        reason: reason_at(&at),
                        // Through the one value printer: a list may hold
                        // tables whose keys are no setting names, and a key
                        // it does not name prints no value at all.
                        in_file: printed_value(raw, &at, value, ValueForm::Get),
                        used: if value_hidden {
                            used.map(|_| NOT_SHOWN.to_string())
                        } else {
                            used
                        },
                        value_hidden,
                        path: at.clone(),
                    }
                }
            });
        }
        // Entries the file leaves out that the load writes from a
        // deprecated key.
        for (key, reason) in &corrected {
            if inside(key, &path) && value_at(&file, key).is_none() {
                let used = used_at(key);
                if used.is_some() {
                    corrections.push(Correction {
                        from: source_of(key),
                        surface: surface_of(key),
                        path: key.clone(),
                        in_file: None,
                        value_hidden: false,
                        used,
                        reason: reason.clone(),
                    });
                }
            }
        }
        let in_use = effective_at(&path).unwrap_or_else(|| table.clone());
        let shown = render_in_use(raw, &path, &in_use, &file, &corrections, true);
        let named = render_in_use(raw, &path, &in_use, &file, &corrections, false);
        return Ok(GetReport {
            with_names: (named != shown).then_some(named),
            value: GetValue::Set(shown),
            corrections,
            refused_by,
        });
    }
    // A single setting: what the file says, unless the load uses something
    // else for it (or drops the entry holding it), which it says why for.
    if node.is_some_and(|node| !compared(node)) || used.as_deref() == Some(in_file.as_str()) {
        return Ok(GetReport {
            value: GetValue::Set(in_file),
            corrections: Vec::new(),
            refused_by,
            with_names: None,
        });
    }
    Ok(GetReport {
        value: used.clone().map_or(GetValue::Unset, GetValue::Set),
        corrections: vec![Correction {
            from: None,
            surface: surface_of(&path),
            reason: reason_at(&path),
            path: path.clone(),
            in_file: Some(in_file),
            value_hidden: false,
            used,
        }],
        refused_by,
        with_names: None,
    })
}

/// Every value inside `value` that is not itself a table, with its path
/// (`path` extended), in file order.
fn leaves_of<'a>(
    value: &'a toml::Value,
    path: &mut Vec<String>,
    found: &mut Vec<(Vec<String>, &'a toml::Value)>,
) {
    match value {
        toml::Value::Table(table) => {
            for (key, child) in table {
                path.push(key.clone());
                leaves_of(child, path, found);
                path.pop();
            }
        }
        leaf => found.push((path.clone(), leaf)),
    }
}

/// Whether a value from the file can be compared with what the load uses
/// by its text: an array of tables (the projects) reads back with every
/// default filled in, which is not a correction, so it is not.
fn compared(value: &toml::Value) -> bool {
    !matches!(value, toml::Value::Array(items) if items.iter().any(toml::Value::is_table))
}

/// `json` with every null left out of its tables, recursively.
fn without_nulls(json: serde_json::Value) -> serde_json::Value {
    match json {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.into_iter()
                .filter(|(_, value)| !value.is_null())
                .map(|(key, value)| (key, without_nulls(value)))
                .collect(),
        ),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(without_nulls).collect())
        }
        other => other,
    }
}

/// The value at the dotted `path` inside `value`, if there is one.
fn value_at<'a>(value: &'a toml::Value, path: &[String]) -> Option<&'a toml::Value> {
    path.iter()
        .try_fold(value, |node, segment| node.get(segment.as_str()))
}

fn render(value: &toml::Value) -> String {
    match value {
        toml::Value::String(text) => text.clone(),
        toml::Value::Table(table) => toml::to_string(table)
            .unwrap_or_default()
            .trim_end()
            .to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMENTED: &str = "\
# dux configuration

[ui]
# How wide the left pane is.
left_width_pct = 20 # keep it narrow

[server]
# The port.
port = 3890
";

    fn temp_config(body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, body).expect("seed");
        (dir, path)
    }

    #[test]
    fn a_known_setting_is_plain_and_the_password_is_a_secret_stored_as_its_hash() {
        assert_eq!(
            lookup("ui.left_width_pct").unwrap().policy,
            WritePolicy::Plain
        );
        assert_eq!(
            lookup("server.auth.password").unwrap().policy,
            WritePolicy::Secret(SecretKind::PasswordHash {
                stores_at: &["server", "auth", "password_hash"]
            })
        );
        assert_eq!(
            lookup("server.auth.password_hash").unwrap().policy,
            WritePolicy::Plain
        );
        assert!(lookup("server.auth").unwrap().is_table());
    }

    #[test]
    fn an_unknown_setting_is_refused_with_the_closest_one() {
        let err = lookup("ui.left_widht_pct").unwrap_err();
        assert_eq!(
            err,
            KeyError::Unknown {
                known: "ui".to_string(),
                suggestion: Some("ui.left_width_pct".to_string())
            }
        );
        assert!(
            err.to_string().contains("did you mean ui.left_width_pct?"),
            "{err}"
        );
        // The message the docs quote, word for word.
        assert_eq!(
            lookup("server.prot").unwrap_err().to_string(),
            "server has no setting below it with that name; did you mean server.port? Values \
             are never given in the path."
        );
        assert_eq!(
            lookup("server.zzzzzz").unwrap_err().to_string(),
            "server has no setting below it with that name; `dux config path` shows the file, \
             whose comments list every setting. Values are never given in the path."
        );
        let err = lookup("server.auth.pasword").unwrap_err();
        assert!(err.to_string().contains("server.auth.password"), "{err}");
        let far = lookup("completely.unrelated.thing").unwrap_err();
        assert!(
            matches!(
                far,
                KeyError::Unknown {
                    suggestion: None,
                    ..
                }
            ),
            "{far:?}"
        );
    }

    #[test]
    fn malformed_paths_are_refused() {
        for path in [
            "",
            "ui.",
            ".ui",
            "ui..theme",
            "ui.th eme",
            "providers.my.tool.command\"",
        ] {
            assert!(
                matches!(lookup(path), Err(KeyError::Malformed { .. })),
                "{path:?}"
            );
        }
    }

    #[test]
    fn providers_are_addressed_by_name_and_env_by_variable() {
        assert!(lookup("providers.claude.command").is_ok());
        assert!(
            lookup("providers.my-new-tool.command").is_ok(),
            "a new provider is fine"
        );
        assert!(matches!(
            lookup("providers.claude.comand"),
            Err(KeyError::Unknown { .. })
        ));
        assert!(lookup("env.GITHUB_TOKEN").is_ok());
    }

    /// Environment values are usually tokens: never taken from the command
    /// line, never printed by default, and so is anything that holds them.
    #[test]
    fn env_values_are_secrets_and_their_holders_are_sensitive() {
        let token = lookup("env.GITHUB_TOKEN").unwrap();
        assert_eq!(token.policy, WritePolicy::Secret(SecretKind::Text));
        assert!(token.is_sensitive());
        assert!(
            parse_value(&token, "ghp_x").is_err(),
            "never a command-line value"
        );
        assert!(lookup("env").unwrap().is_sensitive());
        assert!(lookup("projects").unwrap().is_sensitive());
        assert!(!lookup("server.port").unwrap().is_sensitive());
        assert!(
            !lookup("server.auth.password").unwrap().is_sensitive(),
            "its get prints the hash, which is not the password"
        );
    }

    #[test]
    fn a_secret_text_value_is_stored_as_given_keeping_comments() {
        let (_dir, path) = temp_config("[env]\n# my token\nGITHUB_TOKEN = \"old\"\n");
        set_secret_text(
            &path,
            &lookup("env.GITHUB_TOKEN").unwrap(),
            &Password::new("ghp_new".to_string()),
        )
        .unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after, "[env]\n# my token\nGITHUB_TOKEN = \"ghp_new\"\n");
    }

    #[test]
    fn values_are_read_by_the_settings_type() {
        let port = lookup("server.port").unwrap();
        assert_eq!(parse_value(&port, "4000").unwrap().as_integer(), Some(4000));
        let err = parse_value(&port, "70000").unwrap_err();
        assert!(err.contains("server.port"), "{err}");
        assert!(parse_value(&port, "four").is_err());

        let flag = lookup("server.access_log").unwrap();
        assert_eq!(parse_value(&flag, "false").unwrap().as_bool(), Some(false));
        assert!(parse_value(&flag, "nope").is_err());

        let title = lookup("server.title").unwrap();
        assert_eq!(
            parse_value(&title, "42").unwrap().as_str(),
            Some("42"),
            "text is verbatim"
        );

        let hosts = lookup("server.allowed_hosts").unwrap();
        let list = parse_value(&hosts, "[\"a.example\", \"b.example\"]").unwrap();
        assert_eq!(list.as_array().map(|a| a.len()), Some(2));
        assert!(parse_value(&hosts, "a.example").is_err());

        let start = lookup("defaults.start_directory").unwrap();
        assert_eq!(
            parse_value(&start, "~/code").unwrap().as_str(),
            Some("~/code")
        );
    }

    #[test]
    fn allowed_values_are_enforced() {
        let require = lookup("server.auth.require").unwrap();
        assert!(parse_value(&require, "tailnet").is_ok());
        let err = parse_value(&require, "lan").unwrap_err();
        assert!(err.contains("network, tailnet, everywhere"), "{err}");
        let paste = lookup("providers.claude.web_dragdrop_paste").unwrap();
        assert!(parse_value(&paste, "bare").is_ok());
        assert!(parse_value(&paste, "quoted").is_err());
    }

    /// `set` and `get` read a fixed-value setting with one parser: every value
    /// `set` lists is accepted and reads as itself, and a value it refuses is
    /// one `get` says dux uses something else for.
    #[test]
    fn set_accepts_exactly_what_dux_reads_as_written() {
        let paths: &[&[&str]] = &[
            &["logging", "level"],
            &["ui", "agent_sort"],
            &["ui", "pr_banner_position"],
            &["ui", "compose_bar"],
            &["capabilities", "terminal_identity"],
            &["capabilities", "clipboard_passthrough"],
            &["providers", "claude", "web_dragdrop_paste"],
            &["editor", "default"],
            &["server", "tailscale"],
            &["server", "color"],
            &["server", "favicon"],
        ];
        let file = |path: &[&str], value: &str| {
            let (table, key) = path.split_at(path.len() - 1);
            format!(
                "[{}]\n{} = {}\n",
                table.join("."),
                key[0],
                toml::Value::String(value.to_string())
            )
        };
        for path in paths {
            let segments: Vec<String> = path.iter().map(|s| (*s).to_string()).collect();
            let fixed = crate::config_effective::fixed_values(&segments)
                .unwrap_or_else(|| panic!("{path:?} has a fixed set"));
            let key = lookup(&path.join(".")).unwrap();
            for value in &fixed.listed {
                assert!(parse_value(&key, value).is_ok(), "{path:?} refuses {value}");
                let report = get_report(&file(path, value), &key).unwrap();
                assert_eq!(report.corrections, Vec::new(), "{path:?} = {value}");
            }
            for value in ["zz-unknown", "UPPER"] {
                if fixed.accepts(value) {
                    continue;
                }
                assert!(
                    parse_value(&key, value).is_err(),
                    "{path:?} accepts {value}"
                );
                let report = get_report(&file(path, value), &key).unwrap();
                assert!(
                    !report.corrections.is_empty()
                        || matches!(report.value, GetValue::Set(ref v) if v != value),
                    "{path:?} = {value}: {report:?}"
                );
            }
        }
    }

    #[test]
    fn logging_level_takes_exactly_the_levels_the_logger_knows() {
        let key = lookup("logging.level").unwrap();
        for level in ["debug", "info", "warn", "error"] {
            assert!(parse_value(&key, level).is_ok(), "{level}");
        }
        for level in ["loud", "DEBUG", " info", "trace"] {
            let err = parse_value(&key, level).unwrap_err();
            assert!(err.contains("debug, info, warn, error"), "{err}");
        }
    }

    #[test]
    fn tables_secrets_and_the_hand_managed_sections_are_not_set_from_the_command_line() {
        assert!(parse_value(&lookup("server.auth").unwrap(), "x").is_err());
        let err = parse_value(&lookup("server.auth.password").unwrap(), "hunter2").unwrap_err();
        assert!(err.contains("never given on the command line"), "{err}");
        let err = parse_value(&lookup("projects").unwrap(), "[]").unwrap_err();
        assert!(err.contains("projects"), "{err}");
    }

    /// A check run under the write's lock sees the document the value would
    /// go into (so the password's minimums are read from the very text the
    /// hash is written to), and when it refuses, nothing is written.
    #[test]
    fn a_check_under_the_lock_sees_the_document_and_a_refusal_writes_nothing() {
        let (_dir, path) = temp_config("[server.auth]\nminimum_password_length = 40\n");
        let before = std::fs::read_to_string(&path).unwrap();
        let mut seen = None;
        let result = write_value_checked(
            &path,
            MissingConfig::CreateDocumented,
            &password_hash_path(),
            Value::from("x"),
            |doc| {
                seen = Some(password_policy_of(&doc.to_string())?.minimum_length);
                anyhow::bail!("refused")
            },
        );
        assert!(result.is_err());
        assert_eq!(seen, Some(40));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    /// Every refusal of a path that is not well formed repeats only the
    /// longest start of it that names a setting or table, never the rest.
    #[test]
    fn a_malformed_path_repeats_only_its_known_start() {
        for (typed, known) in [
            ("env.ghp-SECRET123", Some("env")),
            ("ui..ghpSECRET123", Some("ui")),
            ("server.auth..ghpSECRET123", Some("server.auth")),
            ("ui.ghp SECRET123", Some("ui")),
            (".ghpSECRET123", None),
            ("ghpSECRET123..x", None),
            ("ui.left_width_pct=ghpSECRET123", Some("ui.left_width_pct")),
        ] {
            let said = lookup(typed).unwrap_err().to_string();
            assert!(!said.contains("SECRET123"), "{typed}: {said}");
            if let Some(known) = known {
                assert!(
                    said.contains(&format!("starts with {known};")),
                    "{typed}: {said}"
                );
            }
        }
    }

    /// A set whose result dux's load would reset is refused, and the file is
    /// left as it was.
    #[test]
    fn a_set_dux_would_reset_is_refused() {
        let (_dir, path) = temp_config("[ui]\nleft_width_pct = 25\n");
        let before = std::fs::read_to_string(&path).unwrap();
        let error = set_plain(&path, &lookup("ui.terminal_font_size").unwrap(), "500")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("dux would use 14 instead of 500 for ui.terminal_font_size"),
            "{error}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        // Its default, and any value dux uses as written, is set.
        set_plain(&path, &lookup("ui.terminal_font_size").unwrap(), "14").unwrap();
        set_plain(&path, &lookup("ui.terminal_font_size").unwrap(), "20").unwrap();
    }

    /// A use-time correction counts as much as a load one: every correction
    /// `get` would report for the value refuses the set.
    #[test]
    fn a_set_dux_would_read_as_another_value_is_refused() {
        let (_dir, path) = temp_config("");
        let error = set_plain(&path, &lookup("logging.keep").unwrap(), "5000")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("dux would use 1000 instead of 5000 for logging.keep"),
            "{error}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
    }

    /// A set inside a section the load already reset over another of its
    /// values is taken, and says it stays at its default until then.
    #[test]
    fn a_set_inside_a_section_already_reset_is_taken_and_held_back() {
        let (_dir, path) = temp_config("[ui]\nleft_width_pct = \"a\"\nright_width_pct = \"b\"\n");
        let report = set_plain(&path, &lookup("ui.theme").unwrap(), "nord").unwrap();
        let held = report.held_back.expect("held back");
        assert!(held.contains("dux server resets all of [ui]"), "{held}");
        let report = set_plain(&path, &lookup("ui.terminal_font_size").unwrap(), "20").unwrap();
        assert!(report.held_back.is_some());
    }

    /// A file shown line by line: a value below a hidden key or in `[env]` is
    /// its key and `(not shown)`, a comment in `[env]` is a placeholder, a
    /// value over several lines is one line, a binding that is not a key is
    /// held back, and a plaintext password is never shown, `show` or not.
    #[test]
    fn a_file_shown_line_by_line_holds_back_what_a_printer_does() {
        let raw = "[ui]\nleft_width_pct = 25\n\n[env]\n# ghp_COMMENT_SECRET\nTOKEN = \"ghp_VALUE_SECRET\"\nLIST = [\n  \"ghp_MULTI_SECRET\",\n]\n\n[keys]\nquit = [\"ghp_BINDING_SECRET\"]\nnew_agent = [\"n\"]\n\n[server.auth]\npassword = \"ghp_PLAIN_SECRET\"\n";
        let understood = |binding: &str| binding.len() == 1;
        let shown = shown_file_text(raw, false, &understood).expect("parses");
        for secret in [
            "ghp_COMMENT_SECRET",
            "ghp_VALUE_SECRET",
            "ghp_MULTI_SECRET",
            "ghp_BINDING_SECRET",
            "ghp_PLAIN_SECRET",
        ] {
            assert!(!shown.contains(secret), "{secret}:\n{shown}");
        }
        for line in [
            "left_width_pct = 25\n",
            "TOKEN = (not shown)\n",
            "LIST = (not shown)\n",
            "# (comment not shown)\n",
            "quit = (not shown)\n",
            "new_agent = [\"n\"]\n",
            "password = (a plaintext password; not shown, and not read)\n",
        ] {
            assert!(shown.contains(line), "{line:?}:\n{shown}");
        }
        let with_show = shown_file_text(raw, true, &understood).expect("parses");
        assert!(with_show.contains("ghp_VALUE_SECRET"), "{with_show}");
        assert!(with_show.contains("ghp_COMMENT_SECRET"), "{with_show}");
        assert!(!with_show.contains("ghp_PLAIN_SECRET"), "{with_show}");
    }

    /// A held-back table's header is its header alone: a comment after it
    /// is a placeholder, as is one after a held-back value.
    #[test]
    fn a_held_back_header_and_value_lose_their_trailing_comments() {
        let raw = "[env] # ghp_HEADER_SECRET\nA = \"v\" # ghp_VALUE_COMMENT\n\n[[projects]] # ghp_LIST_SECRET\npath = \"/tmp/x\"\n";
        let shown = shown_file_text(raw, false, &|_| true).expect("parses");
        for secret in ["ghp_HEADER_SECRET", "ghp_VALUE_COMMENT", "ghp_LIST_SECRET"] {
            assert!(!shown.contains(secret), "{secret}:\n{shown}");
        }
        for line in [
            "[env] # (comment not shown)\n",
            "A = (not shown) # (comment not shown)\n",
            "[[projects]] # (comment not shown)\n",
            "path = (not shown)\n",
        ] {
            assert!(shown.contains(line), "{line:?}:\n{shown}");
        }
    }

    /// A file that is not TOML shows none of its text, `show` or not, and
    /// says where the parse stopped and why.
    #[test]
    fn a_file_that_is_not_toml_shows_nothing_of_itself() {
        let raw = "[server.auth]\npassword = \"ghp_BROKEN_SECRET\"\nbroken =\n";
        for show in [false, true] {
            let error = format!(
                "{:#}",
                shown_file_text(raw, show, &|_| true).expect_err("not TOML")
            );
            assert!(!error.contains("ghp_BROKEN_SECRET"), "{error}");
            assert!(error.contains("not valid TOML (line 3"), "{error}");
            assert!(error.contains("fix it first"), "{error}");
        }
    }

    #[test]
    fn set_keeps_every_comment_and_only_changes_its_key() {
        let (_dir, path) = temp_config(COMMENTED);
        let report = set_plain(&path, &lookup("ui.left_width_pct").unwrap(), "25").unwrap();
        assert_eq!(report.previous.as_deref(), Some("20"));
        assert_eq!(report.now, "25");
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            after,
            COMMENTED.replace(
                "left_width_pct = 20 # keep it narrow",
                "left_width_pct = 25 # keep it narrow"
            ),
        );
    }

    #[test]
    fn set_creates_missing_tables() {
        let (_dir, path) = temp_config(COMMENTED);
        set_plain(&path, &lookup("server.auth.require").unwrap(), "everywhere").unwrap();
        set_plain(
            &path,
            &lookup("providers.mytool.command").unwrap(),
            "mytool",
        )
        .unwrap();
        let parsed: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            parsed.server.auth.require,
            crate::config::AuthRequire::Everywhere
        );
        assert_eq!(parsed.providers.get("mytool").unwrap().command, "mytool");
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("# The port.")
        );
    }

    #[test]
    fn set_writes_into_an_inline_table() {
        let (_dir, path) = temp_config("[providers]\nmytool = { command = \"a\" }\n");
        set_plain(&path, &lookup("providers.mytool.command").unwrap(), "b").unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("mytool = { command = \"b\" }"), "{after}");
    }

    #[test]
    fn set_refuses_a_value_that_would_break_server_auth_and_writes_nothing() {
        let (_dir, path) = temp_config(COMMENTED);
        for (key, raw) in [
            ("server.auth.minimum_password_score", "9"),
            ("server.auth.blocked_addresses", "[\"not-an-address\"]"),
            ("server.auth.max_password_bytes", "5"),
            ("server.auth.password_hash", "hunter2"),
        ] {
            assert!(
                set_plain(&path, &lookup(key).unwrap(), raw).is_err(),
                "{key} = {raw}"
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), COMMENTED, "{key}");
        }
    }

    #[test]
    fn set_password_stores_a_hash_that_verifies_and_keeps_comments() {
        let (_dir, path) = temp_config(COMMENTED);
        let password = Password::new("correct horse battery staple".to_string());
        let strength = set_password(&path, &password, &[]).expect("set");
        assert!(strength.strength.score >= 2);
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("# How wide the left pane is."), "{after}");
        let auth = crate::config::auth_section_of(&after).expect("valid");
        let hash = auth.password_hash().expect("a password is set");
        assert_eq!(crate::auth::verify_password(&password, hash), Ok(true));
        assert!(
            !after.contains("correct horse"),
            "the plaintext never reaches the file"
        );
    }

    #[test]
    fn set_password_refuses_one_below_the_configured_minimums() {
        let (_dir, path) = temp_config("[server.auth]\nminimum_password_length = 40\n");
        let err = set_password(
            &path,
            &Password::new("correct horse battery staple".to_string()),
            &[],
        )
        .expect_err("too short for this file's minimum");
        let SetPasswordError::BelowMinimums(check) = &err else {
            panic!("{err:?}");
        };
        assert!(matches!(
            check.failures[0],
            crate::auth::MinimumFailure::TooShort { minimum: 40, .. }
        ));
        assert!(err.to_string().contains("minimum_password_length"), "{err}");
        let weak = set_password(&path, &Password::new("password".to_string()), &[]).unwrap_err();
        assert!(weak.to_string().contains("weak"), "{weak}");
        assert!(!std::fs::read_to_string(&path).unwrap().contains("argon2"));
    }

    #[test]
    fn get_reads_the_file_falls_back_to_the_default_and_shows_the_password_hash() {
        let raw = "[server]\nport = 4000\n";
        assert_eq!(
            get(raw, &lookup("server.port").unwrap()).unwrap(),
            GetValue::Set("4000".into())
        );
        assert_eq!(
            get(raw, &lookup("server.host").unwrap()).unwrap(),
            GetValue::Default("127.0.0.1".into())
        );
        assert_eq!(
            get(raw, &lookup("providers.nothere.command").unwrap()).unwrap(),
            GetValue::Unset
        );
        // A file dux refuses (this hash is not a real one): `get` still
        // prints what the file says, and says why the value in use is
        // unknown.
        let broken = "[server]\nport = 4000\n\n[server.auth]\npassword_hash = \"$argon2id$x\"\n";
        let unknown = |path: &str| match get(broken, &lookup(path).unwrap()).unwrap() {
            GetValue::Unknown { in_file, reason } => {
                assert!(reason.contains("password_hash"), "{reason}");
                in_file
            }
            other => panic!("{path}: expected Unknown, got {other:?}"),
        };
        assert_eq!(unknown("server.port").as_deref(), Some("4000"));
        assert_eq!(unknown("server.host"), None);
        assert_eq!(
            unknown("server.auth.password").as_deref(),
            Some("$argon2id$x")
        );
        assert_eq!(unknown("providers.nothere.command"), None);
        let table = unknown("server.auth").expect("a table is printed");
        assert!(table.contains("password_hash"), "{table}");
    }

    /// A section with two invalid values can be repaired one value at a
    /// time: a set is refused only for a problem it would ADD, and it says
    /// which problems are left.
    #[test]
    fn a_set_repairs_one_of_two_broken_values_and_names_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[server.auth]\nsession_idle_seconds = 0\nmax_tracked_addresses = 0\n",
        )
        .unwrap();
        let report = set_plain(
            &path,
            &lookup("server.auth.session_idle_seconds").unwrap(),
            "60",
        )
        .expect("the first fix is allowed");
        assert_eq!(
            report.remaining_problems.len(),
            1,
            "{:?}",
            report.remaining_problems
        );
        assert!(
            report.remaining_problems[0]
                .message
                .contains("max_tracked_addresses")
        );
        let report = set_plain(
            &path,
            &lookup("server.auth.max_tracked_addresses").unwrap(),
            "10000",
        )
        .expect("the second fix is allowed");
        assert!(
            report.remaining_problems.is_empty(),
            "{:?}",
            report.remaining_problems
        );
        crate::config::auth_section_of(&std::fs::read_to_string(&path).unwrap())
            .expect("the section is valid again");
    }

    /// A set that adds a problem is refused even while another one is
    /// already there, and nothing is written.
    #[test]
    fn a_set_that_adds_a_problem_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let text = "[server.auth]\nsession_idle_seconds = 0\n";
        std::fs::write(&path, text).unwrap();
        let error = set_plain(
            &path,
            &lookup("server.auth.max_tracked_addresses").unwrap(),
            "0",
        )
        .expect_err("refused");
        assert!(
            format!("{error:#}").contains("max_tracked_addresses"),
            "{error:#}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }

    /// The rule that minimum_password_length fits in max_password_bytes is
    /// checked against the file's own values with the new one applied,
    /// never the defaults, in both directions.
    #[test]
    fn cross_field_rules_are_checked_against_the_files_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[server.auth]\nminimum_password_length = 8\n").unwrap();
        set_plain(
            &path,
            &lookup("server.auth.max_password_bytes").unwrap(),
            "8",
        )
        .expect("8 fits a minimum of 8");
        std::fs::write(&path, "[server.auth]\nmax_password_bytes = 4096\n").unwrap();
        set_plain(
            &path,
            &lookup("server.auth.minimum_password_length").unwrap(),
            "2000",
        )
        .expect("2000 fits a maximum of 4096");
        let error = set_plain(
            &path,
            &lookup("server.auth.minimum_password_length").unwrap(),
            "5000",
        )
        .expect_err("5000 does not fit 4096");
        assert!(
            format!("{error:#}").contains("max_password_bytes"),
            "{error:#}"
        );
    }

    /// Every problem in `[server.auth]` is seen key by key, so a type error
    /// in one key (an unknown `require`) never hides a problem a set adds
    /// in another: that set is refused and nothing is written.
    #[test]
    fn a_set_adding_a_problem_is_refused_beside_a_type_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let text = "[server.auth]\nrequire = \"lan\"\n";
        std::fs::write(&path, text).unwrap();
        let error = set_plain(
            &path,
            &lookup("server.auth.minimum_password_score").unwrap(),
            "9",
        )
        .expect_err("a score of 9 is a new problem");
        assert!(
            format!("{error:#}").contains("minimum_password_score"),
            "{error:#}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }

    /// Beside a type error, a set that repairs another key is allowed and
    /// names every problem left, the type error included; fixing that one
    /// is then allowed too.
    #[test]
    fn every_problem_beside_a_type_error_is_named_and_can_be_fixed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[server.auth]\nrequire = \"lan\"\nsession_idle_seconds = 0\n",
        )
        .unwrap();
        let report = set_plain(
            &path,
            &lookup("server.auth.session_idle_seconds").unwrap(),
            "60",
        )
        .expect("repairs one problem");
        assert_eq!(report.remaining_problems.len(), 1, "{report:?}");
        assert!(
            report.remaining_problems[0].message.contains("require"),
            "{report:?}"
        );
        let report = set_plain(&path, &lookup("server.auth.require").unwrap(), "network")
            .expect("fixes the problem it was told about");
        assert!(report.remaining_problems.is_empty(), "{report:?}");
        crate::config::auth_section_of(&std::fs::read_to_string(&path).unwrap())
            .expect("valid again");
    }

    /// A new password meets the minimums the file sets, even while an
    /// unrelated key of the section is invalid.
    #[test]
    fn a_new_password_meets_the_files_minimum_beside_an_unrelated_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[server.auth]\nminimum_password_length = 30\nrequire = \"lan\"\n",
        )
        .unwrap();
        // 20 characters, strong by zxcvbn, below the configured 30.
        let password = Password::new("vq8#Lz!t2Wm9rK@x4Np&".to_string());
        let error = set_password(&path, &password, &[]).expect_err("too short");
        assert!(
            matches!(error, SetPasswordError::BelowMinimums(_)),
            "{error}"
        );
        assert!(
            !std::fs::read_to_string(&path)
                .unwrap()
                .contains("$argon2id$")
        );
        let policy = current_password_policy(&path).expect("readable policy");
        assert_eq!(policy.minimum_length, 30);
    }

    /// A password policy key that is itself unreadable or invalid stops a
    /// new password, naming the key to fix first: dux cannot know what the
    /// password has to meet.
    #[test]
    fn a_broken_password_policy_key_stops_a_new_password() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let password = Password::new("vq8#Lz!t2Wm9rK@x4Np&-long-enough".to_string());
        for (text, key) in [
            (
                "[server.auth]\nminimum_password_length = \"thirty\"\n",
                "minimum_password_length",
            ),
            (
                "[server.auth]\nminimum_password_score = 9\n",
                "minimum_password_score",
            ),
            (
                "[server.auth]\nmax_password_bytes = 0\n",
                "max_password_bytes",
            ),
        ] {
            std::fs::write(&path, text).unwrap();
            let error = set_password(&path, &password, &[]).expect_err("refused");
            assert!(error.to_string().contains(key), "{key}: {error}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        }
    }

    /// Setting one field of a built-in provider the file does not list
    /// writes that provider as dux runs it, with the one change: its command
    /// stays.
    #[test]
    fn setting_one_field_of_an_unlisted_built_in_provider_keeps_its_command() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\nleft_width_pct = 20\n").unwrap();
        let before = crate::config::load_config_file(&path).unwrap();
        set_plain(
            &path,
            &lookup("providers.claude.args").unwrap(),
            "[\"--verbose\"]",
        )
        .expect("set");
        let after = crate::config::load_config_file(&path).unwrap();
        let mut expected = before.providers.commands["claude"].clone();
        expected.args = vec!["--verbose".to_string()];
        assert_eq!(after.providers.commands["claude"], expected);
    }

    /// A provider dux does not know gets its command first: any other field
    /// set on it alone is refused, and so is any set that leaves a provider
    /// with an empty command.
    #[test]
    fn a_provider_is_never_left_without_a_command() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let text = "[ui]\nleft_width_pct = 20\n";
        std::fs::write(&path, text).unwrap();
        let error = set_plain(&path, &lookup("providers.mine.args").unwrap(), "[\"-x\"]")
            .expect_err("no command yet");
        assert!(
            format!("{error:#}").contains("providers.mine.command"),
            "{error:#}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);

        set_plain(&path, &lookup("providers.mine.command").unwrap(), "mine").expect("command");
        set_plain(&path, &lookup("providers.mine.args").unwrap(), "[\"-x\"]").expect("then args");
        let config = crate::config::load_config_file(&path).unwrap();
        assert_eq!(config.providers.commands["mine"].command, "mine");

        // A new entry without a command is refused too.
        set_plain(&path, &lookup("providers.other.command").unwrap(), "")
            .expect_err("a new provider with an empty command");

        let before = std::fs::read_to_string(&path).unwrap();
        for name in ["mine", "claude"] {
            let error = set_plain(
                &path,
                &lookup(&format!("providers.{name}.command")).unwrap(),
                "",
            )
            .expect_err("an empty command");
            assert!(format!("{error:#}").contains("command"), "{error:#}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        }
    }

    /// `get` reports the value dux runs with: a provider the file lists
    /// without a command runs an empty one (the shipped command fills only a
    /// provider the file does not list), never the shipped default.
    #[test]
    fn get_reports_the_command_a_listed_provider_actually_runs() {
        let text = "[providers.claude]\nargs = [\"--verbose\"]\n";
        let key = lookup("providers.claude.command").unwrap();
        assert_eq!(get(text, &key).unwrap(), GetValue::Default(String::new()));
        let unlisted = "[ui]\nleft_width_pct = 20\n";
        assert_eq!(
            get(unlisted, &key).unwrap(),
            GetValue::Default("claude".to_string())
        );
    }

    /// On a file dux cannot load, `get` still reads the file's own text, and
    /// says the value dux would use cannot be worked out, naming why.
    #[test]
    fn get_on_a_file_dux_cannot_load_says_the_value_in_use_is_unknown() {
        let text = "[ui]\nleft_width_pct = 25\n\n[server.auth]\nrequire = \"lan\"\n";
        let GetValue::Unknown { in_file, reason } =
            get(text, &lookup("ui.left_width_pct").unwrap()).unwrap()
        else {
            panic!("expected Unknown");
        };
        assert_eq!(in_file.as_deref(), Some("25"));
        assert!(reason.contains("require"), "{reason}");
        let GetValue::Unknown { in_file, .. } =
            get(text, &lookup("ui.right_width_pct").unwrap()).unwrap()
        else {
            panic!("expected Unknown");
        };
        assert_eq!(in_file, None);
    }

    /// A set is refused when the file would then stop dux from starting
    /// where it did not before, with the start's own words, and nothing is
    /// written. A problem the file already had does not block an unrelated
    /// set, and it is listed among the problems left.
    #[test]
    fn a_set_that_would_stop_dux_from_starting_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let text = "[server]\nport = 3890\n";
        std::fs::write(&path, text).unwrap();
        let error = set_plain(&path, &lookup("server.host").unwrap(), "localhost")
            .expect_err("a hostname is not an IP literal");
        let start = crate::config::start_check_problems(&{
            let mut config = Config::default();
            config.server.host = "localhost".to_string();
            config
        });
        assert!(
            format!("{error:#}").contains(&start[0]),
            "{error:#} / {start:?}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);

        std::fs::write(&path, "[server]\nhost = \"localhost\"\n").unwrap();
        let report = set_plain(&path, &lookup("ui.left_width_pct").unwrap(), "30")
            .expect("an unrelated set is allowed");
        assert_eq!(report.remaining_problems.len(), 1, "{report:?}");
        assert!(
            report.remaining_problems[0].message.contains("localhost"),
            "{report:?}"
        );
    }

    /// An environment variable is named by the rule dux starts with, so a
    /// name dux would refuse is refused as a setting path.
    #[test]
    fn an_env_name_dux_would_refuse_is_not_a_setting() {
        assert!(lookup("env.MY-TOKEN").is_err());
        assert!(lookup("env.1TOKEN").is_err());
        assert!(lookup("env.MY_TOKEN").is_ok());
    }

    /// A password stored beside a problem the file already had reports that
    /// problem, so the caller can say dux will not start with the file.
    #[test]
    fn a_password_set_reports_the_problems_left() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[server.auth]\nrequire = \"lan\"\n").unwrap();
        let password = Password::new("correct horse battery staple".to_string());
        let set = set_password(&path, &password, &[]).expect("no new problem");
        assert_eq!(
            set.remaining_problems.len(),
            1,
            "{:?}",
            set.remaining_problems
        );
        assert!(set.remaining_problems[0].message.contains("require"));
        let problems = set_secret_text(&path, &lookup("env.TOKEN").unwrap(), &password)
            .expect("no new problem");
        assert_eq!(problems.len(), 1, "{problems:?}");
    }

    /// Every bad environment variable is its own start problem, so a second
    /// one is a new problem: a set adding it is refused and nothing is
    /// written.
    #[test]
    fn a_second_broken_env_value_is_refused() {
        let (_dir, path) = temp_config("[env]\nA = \"${\"\n");
        let before = std::fs::read_to_string(&path).unwrap();
        let error = set_secret_text(
            &path,
            &lookup("env.B").unwrap(),
            &Password::new("${".to_string()),
        )
        .expect_err("a second broken env value");
        assert!(format!("{error:#}").contains('B'), "{error:#}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    /// With two broken env values, each is reported by name, and repairing
    /// either is allowed; the other stays listed.
    #[test]
    fn repairing_one_of_two_broken_env_values_is_allowed() {
        let (_dir, path) = temp_config("[env]\nA = \"${\"\nB = \"${\"\n");
        let problems = crate::config::start_problems_of(&std::fs::read_to_string(&path).unwrap());
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(
            problems.iter().all(|p| !p.message.contains("${")),
            "no values: {problems:?}"
        );
        let remaining = set_secret_text(
            &path,
            &lookup("env.A").unwrap(),
            &Password::new("fixed".to_string()),
        )
        .expect("repairing a reported problem");
        assert_eq!(remaining.len(), 1, "{remaining:?}");
        assert!(remaining[0].message.contains('B'), "{remaining:?}");
    }

    /// The same per variable inside a project's env.
    #[test]
    fn broken_project_env_values_are_problems_of_their_own() {
        let problems = crate::config::start_problems_of(
            "[[projects]]\nid = \"p\"\npath = \"/p\"\nname = \"api\"\n[projects.env]\nA = \"${\"\nB = \"${\"\n",
        );
        assert_eq!(problems.len(), 2, "{problems:?}");
        // Placed by its line, never by its name, path or id.
        assert!(
            problems
                .iter()
                .all(|p| p.message.contains("the project on line 1")
                    && !p.message.contains("api")
                    && !p.message.contains("/p")),
            "{problems:?}"
        );
    }

    /// Each start check runs on its own: a password hash in the wrong place
    /// is one problem, listed once, and does not hide the others, so a set
    /// adding a bad host beside it is still refused.
    #[test]
    fn a_set_adding_a_start_problem_is_refused_beside_a_stray_password_hash() {
        let (_dir, path) = temp_config("[server]\npassword_hash = \"x\"\n");
        let text = std::fs::read_to_string(&path).unwrap();
        let before = crate::config::start_problems_of(&text);
        assert_eq!(before.len(), 1, "{before:?}");
        let error = set_plain(&path, &lookup("server.host").unwrap(), "not-an-ip")
            .expect_err("a bad host is a new problem");
        assert!(format!("{error:#}").contains("not-an-ip"), "{error:#}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        let with_host = "[server]\npassword_hash = \"x\"\nhost = \"not-an-ip\"\n";
        assert_eq!(crate::config::start_problems_of(with_host).len(), 2);
    }

    /// `dux server` refuses `[server] port = 0`, and the start checks use the
    /// same rule, so `set` refuses it.
    #[test]
    fn a_port_dux_server_will_not_start_with_is_refused() {
        let (_dir, path) = temp_config("[server]\nport = 3890\n");
        let error = set_plain(&path, &lookup("server.port").unwrap(), "0").expect_err("port 0");
        let server = crate::config::ServerConfig {
            port: 0,
            ..Default::default()
        };
        let start = crate::config::resolve_server_plan(
            &server,
            &crate::config::ServerCliOverrides::default(),
            None,
        )
        .expect_err("dux server refuses port 0")
        .to_string();
        assert!(format!("{error:#}").contains(&start), "{error:#}\n{start}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[server]\nport = 3890\n"
        );
    }

    /// A problem is known by what it is about, never by where it sits:
    /// removing the first of two broken `blocked_addresses` entries is a
    /// repair, though the one left moves from entry 2 to entry 1, and adding
    /// a new broken entry is refused.
    #[test]
    fn removing_one_of_two_broken_blocked_addresses_is_a_repair() {
        let (_dir, path) = temp_config(
            "[server.auth]\nblocked_addresses = [\"not-an-address\", \"10.0.0.0/99\"]\n",
        );
        let key = lookup("server.auth.blocked_addresses").unwrap();
        let report = set_plain(&path, &key, "[\"10.0.0.0/99\"]").expect("a repair");
        assert_eq!(report.remaining_problems.len(), 1, "{report:?}");
        assert!(
            report.remaining_problems[0].message.contains("entry 1"),
            "{report:?}"
        );
        set_plain(&path, &key, "[\"10.0.0.0/99\", \"also-not-an-address\"]")
            .expect_err("a new broken entry");
    }

    /// A wrong-typed setting is one `dux server` resets to its default but
    /// the terminal UI refuses to start over: it is a start problem worded
    /// for each, never showing the value, listed by a set that does not
    /// touch it, and `get` answers for `dux server`, which starts with the
    /// file, naming the terminal UI as the surface that refuses it.
    #[test]
    fn a_wrong_typed_setting_is_a_terminal_ui_start_problem() {
        let (_dir, path) = temp_config("[ui]\nleft_width_pct = \"wide\"\n");
        let text = std::fs::read_to_string(&path).unwrap();
        let problems = crate::config::start_problems_of(&text);
        assert_eq!(problems.len(), 1, "{problems:?}");
        let message = &problems[0].message;
        assert!(
            message.contains("the terminal UI will not start"),
            "{message}"
        );
        assert!(message.contains("[ui] left_width_pct"), "{message}");
        assert!(
            message.contains("dux server reads it as its default"),
            "{message}"
        );
        assert!(!message.contains("wide"), "{message}");
        let report = set_plain(&path, &lookup("server.port").unwrap(), "4000")
            .expect("a problem already there does not block");
        let remaining: Vec<&str> = report
            .remaining_problems
            .iter()
            .map(|problem| problem.message.as_str())
            .collect();
        assert_eq!(remaining, vec![message.as_str()]);
        let report = get_report(&text, &lookup("ui.left_width_pct").unwrap()).unwrap();
        assert_eq!(report.value, GetValue::Set("20".to_string()), "{report:?}");
        assert_eq!(report.corrections.len(), 1, "{report:?}");
        assert_eq!(report.corrections[0].in_file.as_deref(), Some("wide"));
        assert_eq!(report.refused_by.len(), 1, "{report:?}");
        assert_eq!(report.refused_by[0].0, crate::config::Surface::TerminalUi);
        assert!(
            report.refused_by[0].1.contains("[ui] left_width_pct"),
            "{report:?}"
        );
    }

    /// A new port 0 is refused in `dux server`'s words, and never said to
    /// stop the terminal UI, which starts with it.
    #[test]
    fn a_port_of_zero_is_named_as_stopping_dux_server() {
        let (_dir, path) = temp_config("[server]\nport = 3890\n");
        let error = set_plain(&path, &lookup("server.port").unwrap(), "0").expect_err("port 0");
        assert!(
            format!("{error:#}").contains("dux server will not start"),
            "{error:#}"
        );
        assert!(
            !format!("{error:#}").contains("the terminal UI will not start"),
            "{error:#}"
        );
    }

    /// A set is judged by the key it sets: a port of 0 the file already had
    /// never blocks repairing the host beside it, nor changing the host.
    #[test]
    fn a_port_of_zero_already_there_never_blocks_a_host_set() {
        let (_dir, path) = temp_config("[server]\nhost = \"localhost\"\nport = 0\n");
        let report = set_plain(&path, &lookup("server.host").unwrap(), "127.0.0.1")
            .expect("repairing the host");
        assert_eq!(
            report.remaining_problems.len(),
            1,
            "{:?}",
            report.remaining_problems
        );
        let (_dir, path) = temp_config("[server]\nport = 0\n");
        set_plain(&path, &lookup("server.host").unwrap(), "0.0.0.0")
            .expect("changing the host beside a port of 0");
    }

    /// Duplicate project ids stop both surfaces (the project sync refuses
    /// them), so they are a start problem and `get` cannot report the value
    /// in use.
    #[test]
    fn duplicate_project_ids_are_a_start_problem() {
        let body = "[[projects]]\nid = \"same\"\npath = \"/tmp/review19-a\"\n\n\
                    [[projects]]\nid = \"same\"\npath = \"/tmp/review19-b\"\n";
        let config = crate::config::effective_config_from_text(body).expect("loads");
        crate::config_sync::validate_project_records("config.toml", &config.projects)
            .expect_err("dux refuses to start with duplicate project ids");
        let problems = crate::config::start_problems_of(body);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].stops_terminal_ui && problems[0].stops_dux_server);
        let got = get(body, &lookup("server.port").unwrap()).unwrap();
        assert!(matches!(got, GetValue::Unknown { .. }), "{got:?}");
    }

    /// Wrong-typed settings are listed field by field, so repairing one of
    /// two in a section is allowed and leaves the other listed.
    #[test]
    fn repairing_one_of_two_wrong_typed_settings_in_a_section_is_allowed() {
        let (_dir, path) =
            temp_config("[ui]\nleft_width_pct = \"wide\"\nright_width_pct = \"wide\"\n");
        let before = crate::config::start_problems_of(&std::fs::read_to_string(&path).unwrap());
        assert_eq!(before.len(), 2, "{before:?}");
        let report =
            set_plain(&path, &lookup("ui.left_width_pct").unwrap(), "20").expect("a repair");
        assert_eq!(
            report.remaining_problems.len(),
            1,
            "{:?}",
            report.remaining_problems
        );
        assert!(
            report.remaining_problems[0]
                .message
                .contains("right_width_pct")
        );
    }

    /// A cross-key rule that could not be judged before (one of its keys out
    /// of range) was not satisfied, so repairing that key is allowed even
    /// though the rule now reads as broken.
    #[test]
    fn repairing_a_key_is_allowed_when_its_cross_key_rule_could_not_be_judged_before() {
        let (_dir, path) =
            temp_config("[server.auth]\nminimum_password_length = 2000\nmax_password_bytes = 0\n");
        let report = set_plain(
            &path,
            &lookup("server.auth.max_password_bytes").unwrap(),
            "1024",
        )
        .expect("repairing the out-of-range value");
        assert_eq!(
            report.remaining_problems.len(),
            1,
            "{:?}",
            report.remaining_problems
        );
        assert!(
            report.remaining_problems[0]
                .message
                .contains("minimum_password_length")
        );
    }

    /// `get` warns about any problem that stops a surface, not only a
    /// wrong-typed setting: a host that is not an IP stops both.
    #[test]
    fn get_does_not_report_values_for_a_file_dux_will_not_start_with() {
        let body = "[server]\nhost = \"localhost\"\nport = 4000\n";
        assert!(!crate::config::start_problems_of(body).is_empty());
        let GetValue::Unknown { reason, .. } = get(body, &lookup("server.port").unwrap()).unwrap()
        else {
            panic!("expected Unknown");
        };
        assert!(reason.contains("localhost"), "{reason}");
    }

    // ── The web's writes: a compare-and-set password and a ban ──────────────

    const STRONG: &str = "orbit velvet quarry lantern cobalt";

    fn hash_in(path: &Path) -> String {
        let raw = std::fs::read_to_string(path).unwrap();
        crate::config::auth_section_of(&raw).unwrap().password_hash
    }

    #[test]
    fn a_password_is_set_only_over_the_hash_the_caller_checked() {
        let (_dir, path) = temp_config(COMMENTED);
        let first = set_password_if_current(&path, "", &Password::new(STRONG.to_string()), &[])
            .expect("first password over none");
        assert!(first.strength.score >= 2);
        assert!(first.remaining_problems.is_empty());
        let hash = hash_in(&path);
        assert!(hash.starts_with("$argon2id$"), "{hash}");
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("# The port.")
        );

        // A writer that checked an older hash (or none) loses, and writes nothing.
        let stale = set_password_if_current(
            &path,
            "",
            &Password::new("another long passphrase entirely here".to_string()),
            &[],
        )
        .unwrap_err();
        assert!(
            matches!(stale, SetPasswordError::ChangedMeanwhile),
            "{stale}"
        );
        assert_eq!(hash_in(&path), hash);

        set_password_if_current(
            &path,
            &hash,
            &Password::new("another long passphrase entirely here".to_string()),
            &[],
        )
        .expect("over the hash it checked");
        assert_ne!(hash_in(&path), hash);
    }

    /// A problem the file already had elsewhere never refuses a ban: a ban that
    /// cannot land is held in memory by the caller, and refusing it here
    /// would turn an unrelated typo into an unblockable guesser.
    #[test]
    fn a_ban_lands_in_a_file_with_an_unrelated_problem() {
        let (_dir, path) = temp_config("[server.auth]\nsession_idle_seconds = 0\n");
        assert_eq!(
            append_blocked_address(&path, "203.0.113.9".parse().unwrap(), 10).unwrap(),
            BanWrite::Written
        );
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"203.0.113.9\""), "{raw}");
        assert!(
            raw.contains("session_idle_seconds = 0"),
            "the problem is left alone"
        );
    }

    #[test]
    fn a_compare_and_set_password_still_meets_the_files_minimums() {
        let (_dir, path) = temp_config(COMMENTED);
        let err = set_password_if_current(&path, "", &Password::new("password1234".into()), &[])
            .unwrap_err();
        assert!(matches!(err, SetPasswordError::BelowMinimums(_)), "{err}");
        assert_eq!(hash_in(&path), "");
    }

    /// A /64 written as one range takes the place of the single addresses
    /// inside it in the same save, leaving every other entry and comment as
    /// it was; nothing is written for what an entry already covers.
    #[test]
    fn a_range_folds_the_addresses_inside_it_and_nothing_covered_is_written() {
        let (_dir, path) = temp_config(
            "[server.auth]\nblocked_addresses = [\n  # a scanner I keep seeing\n  \"203.0.113.7\",\n  \
             \"2001:db8:1:2::5\",\n  \"2001:db8:1:2::9\",\n  \"198.51.100.0/24\", # the old office\n]\n",
        );
        let ip: std::net::IpAddr = "2001:db8:1:2::77".parse().unwrap();
        assert_eq!(
            append_blocked_entry(&path, ip, "2001:db8:1:2::/64", 10).unwrap(),
            BanWrite::Written
        );
        let raw = std::fs::read_to_string(&path).unwrap();
        let auth = crate::config::auth_section_of(&raw).unwrap();
        assert_eq!(
            auth.blocked_addresses,
            ["203.0.113.7", "198.51.100.0/24", "2001:db8:1:2::/64"],
            "{raw}"
        );
        assert!(raw.contains("# a scanner I keep seeing"), "{raw}");
        assert!(raw.contains("# the old office"), "{raw}");

        // Covered already: the range, or an address inside it.
        for (ip, entry) in [
            ("2001:db8:1:2::1", "2001:db8:1:2::/64"),
            ("2001:db8:1:2::3", "2001:db8:1:2::3"),
            ("198.51.100.9", "198.51.100.9"),
        ] {
            assert_eq!(
                append_blocked_entry(&path, ip.parse().unwrap(), entry, 10).unwrap(),
                BanWrite::AlreadyBlocked,
                "{entry}"
            );
        }
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            raw,
            "nothing written"
        );

        // A range that would only fit once the addresses inside it are folded
        // still fits.
        let (_dir, path) = temp_config(
            "[server.auth]\nblocked_addresses = [\"2001:db8:9:9::1\", \"2001:db8:9:9::2\"]\n",
        );
        assert_eq!(
            append_blocked_entry(
                &path,
                "2001:db8:9:9::3".parse().unwrap(),
                "2001:db8:9:9::/64",
                2
            )
            .unwrap(),
            BanWrite::Written
        );
    }

    #[test]
    fn a_ban_is_appended_once_keeping_the_comments_and_stops_at_the_limit() {
        let (_dir, path) = temp_config(COMMENTED);
        let ip: std::net::IpAddr = "203.0.113.7".parse().unwrap();
        assert_eq!(
            append_blocked_address(&path, ip, 10).unwrap(),
            BanWrite::Written
        );
        assert_eq!(
            append_blocked_address(&path, ip, 10).unwrap(),
            BanWrite::AlreadyBlocked
        );
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("# How wide the left pane is."), "{raw}");
        let auth = crate::config::auth_section_of(&raw).unwrap();
        assert_eq!(auth.blocked_addresses, vec!["203.0.113.7".to_string()]);

        // An address inside a range already there is already blocked.
        std::fs::write(
            &path,
            "[server.auth]\nblocked_addresses = [\"198.51.100.0/24\"]\n",
        )
        .unwrap();
        assert_eq!(
            append_blocked_address(&path, "198.51.100.9".parse().unwrap(), 10).unwrap(),
            BanWrite::AlreadyBlocked
        );
        assert_eq!(
            append_blocked_address(&path, ip, 1).unwrap(),
            BanWrite::AtLimit,
            "the list is full"
        );
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("203.0.113.7"), "{raw}");
        // An IPv4-mapped address is written as the IPv4 address it carries.
        assert_eq!(
            append_blocked_address(&path, "::ffff:192.0.2.1".parse().unwrap(), 10).unwrap(),
            BanWrite::Written
        );
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("\"192.0.2.1\"")
        );
    }
}

#[cfg(test)]
mod get_reports_what_dux_uses_tests {
    use super::*;

    /// A field missing from an entry is the entry's problem, named with the
    /// field, and `get` gives the same reason a start does.
    #[test]
    fn a_missing_field_is_blamed_on_its_table_by_name() {
        let raw = "[macros.greet]\ntext = \"hi\"\n";
        let messages: Vec<String> = crate::config::start_problems_of(raw)
            .into_iter()
            .map(|problem| problem.message)
            .collect();
        assert!(
            messages
                .iter()
                .any(|m| m.contains("[macros] greet: missing field surface")),
            "{messages:?}"
        );
        assert!(
            messages.iter().all(|m| !m.contains("greet.text")),
            "{messages:?}"
        );
        let report = get_report(raw, &lookup("macros").unwrap()).unwrap();
        let reasons: Vec<&str> = report
            .corrections
            .iter()
            .map(|correction| correction.reason.as_str())
            .collect();
        assert!(
            reasons
                .iter()
                .any(|r| r.contains("[macros] greet: missing field surface")),
            "{report:?}"
        );
        assert!(
            reasons.iter().all(|r| !r.contains("greet.text")),
            "{report:?}"
        );
    }

    /// A sibling whose own type is wrong is still named beside a table
    /// missing a field.
    #[test]
    fn a_wrong_typed_field_beside_a_missing_one_is_still_named() {
        let raw = "[macros.greet]\ntext = 5\n";
        let messages: Vec<String> = crate::config::start_problems_of(raw)
            .into_iter()
            .map(|problem| problem.message)
            .collect();
        assert!(
            messages.iter().any(|m| m.contains("greet.text")),
            "{messages:?}"
        );
    }

    #[test]
    fn get_reports_the_corrected_terminal_font_size_dux_uses() {
        let raw = "[ui]\nterminal_font_size = 500\n";
        let used = crate::config::effective_config_from_text(raw)
            .unwrap()
            .ui
            .terminal_font_size;
        assert_ne!(used, 500, "precondition: the load corrects it");
        let got = get(raw, &lookup("ui.terminal_font_size").unwrap()).unwrap();
        assert_eq!(
            got,
            GetValue::Set(used.to_string()),
            "get must say what dux uses"
        );
    }

    #[test]
    fn get_reports_the_corrected_tailscale_mode_dux_uses() {
        let raw = "[server]\ntailscale = \"maybe\"\n";
        let used = crate::config::effective_config_from_text(raw)
            .unwrap()
            .server
            .tailscale;
        assert_eq!(used, "auto");
        let got = get(raw, &lookup("server.tailscale").unwrap()).unwrap();
        assert_eq!(got, GetValue::Set(used), "get must say what dux uses");
    }

    #[test]
    fn get_reports_no_command_for_a_retired_stock_provider_dux_prunes() {
        let raw = "[providers.gemini]\ncommand = \"gemini\"\nargs = []\nresume_args = [\"--resume\"]\nresume_wait_timeout_ms = 0\ninstall_hint = \"brew install gemini-cli\"\n";
        let config = crate::config::effective_config_from_text(raw).unwrap();
        assert!(
            config.providers.get("gemini").is_none(),
            "precondition: pruned at load"
        );
        let got = get(raw, &lookup("providers.gemini.command").unwrap()).unwrap();
        assert_eq!(
            got,
            GetValue::Unset,
            "dux has no gemini provider, get must not report one"
        );
    }

    #[test]
    fn set_serve_while_tui_never_makes_the_terminal_ui_refuse_a_file_it_started_with() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[server]\nport = 0\nserve_while_tui = false\n").unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            crate::config::start_refusal(&before, crate::config::Surface::TerminalUi),
            None,
            "precondition: the terminal UI starts with this file"
        );
        let result = set_plain(&path, &lookup("server.serve_while_tui").unwrap(), "true");
        let after = std::fs::read_to_string(&path).unwrap();
        let refusal = crate::config::start_refusal(&after, crate::config::Surface::TerminalUi);
        assert!(
            refusal.is_none(),
            "set returned {result:?} and wrote a file the terminal UI refuses: {refusal:?}"
        );
    }
}
