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
    /// An Argon2id hash of the input, stored at `stores_at` (a dot path).
    PasswordHash { stores_at: &'static str },
    /// Stored as typed, but never taken from the command line and never
    /// printed unless asked for: `env.<NAME>`, where API tokens live.
    Text,
}

/// Write-only settings that have no field of their own in `Config`.
pub const VIRTUAL_KEYS: &[(&str, WritePolicy)] = &[(
    "server.auth.password",
    WritePolicy::Secret(SecretKind::PasswordHash {
        stores_at: "server.auth.password_hash",
    }),
)];

/// String settings whose value must be one of a known set. Only settings
/// whose own reader names the set appear here; the rest are free text.
const ALLOWED_VALUES: &[(&str, &[&str])] = &[
    ("server.tailscale", &["auto", "yes", "no"]),
    ("server.color", &["auto", "always", "never"]),
    ("server.auth.require", &["network", "tailnet", "everywhere"]),
    ("server.auth.cookie_secure", &["auto", "always", "never"]),
    ("ui.compose_bar", &["auto", "always", "never"]),
    (
        "ui.agent_sort",
        &[
            "active",
            "updated",
            "created",
            "name",
            "name_desc",
            "manual",
        ],
    ),
    (
        "capabilities.clipboard_passthrough",
        &["focused", "always", "off"],
    ),
    (
        "providers.*.web_dragdrop_paste",
        &[
            "bare",
            "single_quoted",
            "double_quoted",
            "backslash_escaped",
        ],
    ),
];

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
    /// The dotted path.
    pub fn dotted(&self) -> String {
        self.path.join(".")
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
    /// Not a well-formed path.
    Malformed(String),
    /// No such setting; `suggestion` is the closest one there is.
    Unknown {
        path: String,
        suggestion: Option<String>,
    },
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(reason) => f.write_str(reason),
            Self::Unknown { path, suggestion } => {
                write!(f, "there is no setting called {path}")?;
                match suggestion {
                    Some(close) => write!(f, "; did you mean {close}?"),
                    None => write!(
                        f,
                        "; `dux config path` shows the file, whose comments list every setting"
                    ),
                }
            }
        }
    }
}

impl std::error::Error for KeyError {}

/// Find the setting `path` names.
pub fn lookup(path: &str) -> Result<Key, KeyError> {
    // `name=value` is how other tools take a setting. What follows `=` may be
    // a password, so it is never repeated: only the name is.
    if let Some((name, _)) = path.split_once('=') {
        return Err(KeyError::Malformed(format!(
            "a value goes after the setting's name and a space, not after `=`: write `dux \
             config set {name} <value>` (a secret such as server.auth.password is asked for, or \
             read with --stdin). What followed `=` is not repeated here."
        )));
    }
    let segments = split_path(path)?;
    if let Some((_, policy)) = VIRTUAL_KEYS.iter().find(|(name, _)| *name == path) {
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
        return Err(KeyError::Malformed(format!(
            "{path}: an environment variable name must match [A-Za-z_][A-Za-z0-9_]*, the rule \
             dux starts with"
        )));
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
        None => Err(KeyError::Unknown {
            path: path.to_string(),
            suggestion: suggest(path),
        }),
    }
}

fn split_path(path: &str) -> Result<Vec<String>, KeyError> {
    if path.is_empty() {
        return Err(KeyError::Malformed(
            "a setting's path cannot be empty".to_string(),
        ));
    }
    let segments: Vec<String> = path.split('.').map(str::to_string).collect();
    for segment in &segments {
        if segment.is_empty() {
            return Err(KeyError::Malformed(format!(
                "{path} has an empty part; write the path as table.key, like server.port"
            )));
        }
        if !segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(KeyError::Malformed(format!(
                "{path} has a part with characters other than letters, digits, _ and -; \
                 a key whose name needs them cannot be set this way, so edit config.toml"
            )));
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
        _ => {}
    }
    let mut node = default_tree();
    for part in &parts {
        node = node.get(*part)?.clone();
    }
    Some(shape_of_json(&node))
}

/// Every setting a path can name, for suggestions.
fn known_paths() -> Vec<String> {
    fn walk(node: &serde_json::Value, path: &mut Vec<String>, out: &mut Vec<String>) {
        if let serde_json::Value::Object(map) = node {
            for (name, child) in map {
                path.push(name.clone());
                out.push(path.join("."));
                walk(child, path, out);
                path.pop();
            }
        }
    }
    let mut out: Vec<String> = VIRTUAL_KEYS
        .iter()
        .map(|(name, _)| name.to_string())
        .collect();
    walk(&default_tree(), &mut Vec::new(), &mut out);
    out
}

fn suggest(path: &str) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for candidate in known_paths() {
        let distance = edit_distance(path, &candidate);
        if best.as_ref().is_none_or(|(d, _)| distance < *d) {
            best = Some((distance, candidate));
        }
    }
    let (distance, candidate) = best?;
    let allowed = (path.chars().count() / 3).max(2);
    (distance <= allowed).then_some(candidate)
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

fn allowed_values(segments: &[String]) -> Option<&'static [&'static str]> {
    ALLOWED_VALUES.iter().find_map(|(pattern, values)| {
        let pattern: Vec<&str> = pattern.split('.').collect();
        (pattern.len() == segments.len()
            && pattern
                .iter()
                .zip(segments)
                .all(|(p, s)| *p == "*" || p == s))
        .then_some(*values)
    })
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
    if let Some(allowed) = allowed_values(&key.path)
        && !allowed.contains(&raw)
    {
        return Err(format!(
            "{} takes one of {}, not {raw}",
            key.dotted(),
            allowed.join(", ")
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
    let (leaf, parents) = path.split_last().context("empty path")?;
    let mut item: &mut Item = doc.as_item_mut();
    let mut walked: Vec<&str> = Vec::new();
    for segment in parents {
        walked.push(segment);
        let inline = item.is_inline_table();
        let table = item.as_table_like_mut().with_context(|| {
            format!(
                "{} is not a table in config.toml",
                walked[..walked.len() - 1].join(".")
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
                walked.join("."),
                path.join(".")
            );
        }
    }
    let table = item
        .as_table_like_mut()
        .with_context(|| format!("{} is not a table in config.toml", parents.join(".")))?;
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
    pub path: String,
    /// The value the file had before, as TOML, or `None` when it had none.
    pub previous: Option<String>,
    /// The value written, as TOML.
    pub now: String,
    /// What still stops a surface starting with the file after the write,
    /// each with the surfaces it stops: a set may repair one of several
    /// broken values, and says what is left.
    pub remaining_problems: Vec<crate::config::StartProblem>,
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
    let now = bare(&value);
    // A set may leave problems `[server.auth]` already had, so a section
    // with several broken values can be repaired one value at a time; one
    // that adds a problem is refused.
    let (previous, remaining_problems) = crate::config_write::mutate_config_file_repairing(
        config_path,
        missing,
        &path.join("."),
        |doc| {
            let previous = value_in_doc(doc, path);
            prepare_provider(doc, path)?;
            set_in_doc(doc, path, value)?;
            check_provider_command(doc, path)?;
            Ok(previous)
        },
    )?;
    Ok(SetReport {
        path: path.join("."),
        previous,
        now,
        remaining_problems,
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
fn check_provider_command(doc: &DocumentMut, path: &[String]) -> Result<()> {
    let [section, name, _] = path else {
        return Ok(());
    };
    if section != "providers" {
        return Ok(());
    }
    let file: toml::Table =
        toml::from_str(&doc.to_string()).context("failed to read the change back")?;
    // The command on its own: a problem in another field of the provider is
    // that field's, and never reads as a missing command.
    let command = file
        .get("providers")
        .and_then(|providers| providers.get(name))
        .and_then(|provider| provider.get("command"))
        .and_then(toml::Value::as_str);
    if command.is_none_or(|command| command.trim().is_empty()) {
        anyhow::bail!(
            "that would leave providers.{name} with no command, so dux could not start it. \
             Nothing was written."
        );
    }
    Ok(())
}

fn value_in_doc(doc: &DocumentMut, path: &[String]) -> Option<String> {
    let mut item = doc.as_item();
    for segment in path {
        item = item.as_table_like()?.get(segment)?;
    }
    item.as_value().map(bare)
}

/// A value's TOML text without the comments and spacing around it.
fn bare(value: &Value) -> String {
    let mut value = value.clone();
    value.decor_mut().clear();
    value.to_string()
}

/// Why a password was not set.
#[derive(Debug)]
pub enum SetPasswordError {
    /// It misses a configured minimum; the check says which and how strong
    /// it is.
    BelowMinimums(MinimumCheck),
    /// Reading the policy, hashing or writing failed.
    Failed(anyhow::Error),
}

impl fmt::Display for SetPasswordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BelowMinimums(check) => {
                let reasons: Vec<String> = check.failures.iter().map(ToString::to_string).collect();
                write!(f, "that password was not set: {}", reasons.join("; and "))?;
                if let Some(hint) = &check.strength.hint {
                    write!(f, ". {hint}")?;
                }
                Ok(())
            }
            Self::Failed(error) => write!(f, "{error:#}"),
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
    let defaults = crate::config::ServerAuthConfig::default().password_policy();
    let raw = match std::fs::read_to_string(config_path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(defaults),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", config_path.display()));
        }
    };
    let file: toml::Table = toml::from_str(&raw).map_err(|e| {
        anyhow::anyhow!(
            "config.toml is not valid TOML ({}), so dux cannot tell what a new password has to \
             meet; fix it first",
            crate::config::describe_toml_error(&raw, &e)
        )
    })?;
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
/// holds up another writer.
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
    let report = write_value(config_path, missing, &path, Value::from(hash))
        .map_err(SetPasswordError::Failed)?;
    Ok(PasswordSet {
        strength: check.strength,
        remaining_problems: report.remaining_problems,
    })
}

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
    stores_at.split('.').map(str::to_string).collect()
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
    /// The dotted setting it is about: the key asked for, or an entry inside
    /// the table asked for.
    pub path: String,
    /// What the file says, as TOML text (a string unquoted), or `None`
    /// when the file leaves the setting out and the load writes it from a
    /// deprecated key.
    pub in_file: Option<String>,
    /// What dux uses, or `None` when the load drops the entry holding it.
    pub used: Option<String>,
    /// Why dux uses something else: the load's own sentence.
    pub reason: String,
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
/// not start with it is named with why. A whole table is shown as the file
/// writes it, with each entry inside it the load changes or drops listed.
/// Reads the text itself rather than a loaded config, so it works on a file
/// a surface would refuse to start with: `get` is how you look at the broken
/// part. For a [`WritePolicy::Secret`] key it reads where the secret is
/// stored (the password's hash).
pub fn get_report(raw: &str, key: &Key) -> Result<GetReport> {
    let mut report = get_report_inner(raw, key)?;
    // The table as the file names its entries, for `--show`, when the
    // formatter replaced any name.
    let path = stored_path(key);
    if let Ok(file) = toml::from_str::<toml::Table>(raw)
        && let Some(node) = value_at(&toml::Value::Table(file), &path)
        && node.is_table()
    {
        let named = render(node);
        if named != render_shown(raw, &path, node) {
            report.with_names = Some(named);
        }
    }
    Ok(report)
}

/// Where `key`'s value is stored in the file: the key itself, or for the
/// password, where its hash goes.
fn stored_path(key: &Key) -> Vec<String> {
    match key.policy {
        WritePolicy::Secret(SecretKind::PasswordHash { stores_at }) => {
            stores_at.split('.').map(str::to_string).collect()
        }
        WritePolicy::Secret(SecretKind::Text) | WritePolicy::Plain => key.path.clone(),
    }
}

/// `value`, at `path` in the file `raw`, as `get` prints it: a table whose
/// entries are names in a map with a naming rule prints each name that
/// breaks the rule as a marker naming its line (the formatter's own words),
/// never the name, which may be a token pasted in the wrong place.
fn render_shown(raw: &str, path: &[String], value: &toml::Value) -> String {
    fn shown(raw: &str, path: &mut Vec<String>, value: &toml::Value) -> toml::Value {
        let toml::Value::Table(table) = value else {
            return value.clone();
        };
        let mut out = toml::Table::new();
        for (key, child) in table {
            path.push(key.clone());
            let name = if crate::config::name_is_hidden(raw, path) {
                format!("<{}>", crate::config::shown_path(raw, path))
            } else {
                key.clone()
            };
            out.insert(name, shown(raw, path, child));
            path.pop();
        }
        toml::Value::Table(out)
    }
    render(&shown(raw, &mut path.to_vec(), value))
}

fn get_report_inner(raw: &str, key: &Key) -> Result<GetReport> {
    use crate::config::Surface;
    let path: Vec<String> = match key.policy {
        WritePolicy::Secret(SecretKind::PasswordHash { stores_at }) => {
            stores_at.split('.').map(str::to_string).collect()
        }
        WritePolicy::Secret(SecretKind::Text) | WritePolicy::Plain => key.path.clone(),
    };
    let doc: toml::Table = toml::from_str(raw).map_err(|e| {
        anyhow::anyhow!(
            "config.toml is not valid TOML: {}",
            crate::config::describe_toml_error(raw, &e)
        )
    })?;
    let file = toml::Value::Table(doc);
    let node = value_at(&file, &path);
    // A table is printed through the one formatter: a name that breaks its
    // map's rule is a marker naming its line (see [`render_shown`]).
    let in_file = node.map(|node| render_shown(raw, &path, node));
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
    let effective = serde_json::to_value(&effective).ok();
    let used_at = |at: &[String]| -> Option<String> {
        let mut used = effective.clone();
        for segment in at {
            used = used.and_then(|n| n.get(segment).cloned());
        }
        match used {
            Some(serde_json::Value::Null) | None => None,
            Some(json) => toml::Value::try_from(json).ok().map(|value| render(&value)),
        }
    };
    let used = used_at(&path);
    let corrected: Vec<(String, String)> = crate::config::load_corrections_of(raw);
    let dotted = path.join(".");
    let Some(in_file) = in_file else {
        // Left out of the file, but the load may still write it, from a
        // deprecated key; that is said with where it came from.
        let carried = used.as_ref().and_then(|_| {
            corrected
                .iter()
                .find(|(key, _)| *key == dotted)
                .map(|(_, reason)| Correction {
                    path: dotted.clone(),
                    in_file: None,
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
    let covering = |key: &str| {
        corrected
            .iter()
            .find(|(scope, _)| key == scope || key.starts_with(&format!("{scope}.")))
    };
    let reason_at = |key: &str| {
        covering(key).map_or_else(
            || "that is how dux server's load reads the file".to_string(),
            |(_, reason)| reason.clone(),
        )
    };
    let split = |key: &str| -> Vec<String> { key.split('.').map(str::to_string).collect() };
    if let Some(table) = node.filter(|node| node.is_table()) {
        // A table is shown as the file writes it (the load filling in its
        // defaults is not a correction), with every entry inside it whose
        // value the load changes or drops listed. An entry dropped whole (a
        // provider, a retired block) is listed once, at the entry.
        let mut leaves = Vec::new();
        leaves_of(table, &mut path.clone(), &mut leaves);
        let mut corrections: Vec<Correction> = Vec::new();
        for (at, value) in leaves {
            let key = at.join(".");
            if corrections
                .iter()
                .any(|done| done.used.is_none() && key.starts_with(&format!("{}.", done.path)))
            {
                continue;
            }
            let in_file = render(value);
            let used = used_at(&at);
            if !compared(value) || used.as_deref() == Some(in_file.as_str()) {
                continue;
            }
            let whole = covering(&key).filter(|(scope, _)| {
                *scope != key
                    && scope.starts_with(&format!("{dotted}."))
                    && used_at(&split(scope)).is_none()
            });
            corrections.push(match whole {
                Some((scope, reason)) => Correction {
                    path: scope.clone(),
                    in_file: value_at(&file, &split(scope)).map(render),
                    used: None,
                    reason: reason.clone(),
                },
                None => Correction {
                    reason: reason_at(&key),
                    path: key,
                    in_file: Some(in_file),
                    used,
                },
            });
        }
        // Entries the file leaves out that the load writes from a
        // deprecated key.
        let prefix = format!("{dotted}.");
        for (key, reason) in &corrected {
            if key.starts_with(&prefix) && value_at(&file, &split(key)).is_none() {
                let used = used_at(&split(key));
                if used.is_some() {
                    corrections.push(Correction {
                        path: key.clone(),
                        in_file: None,
                        used,
                        reason: reason.clone(),
                    });
                }
            }
        }
        return Ok(GetReport {
            value: GetValue::Set(render_shown(raw, &path, table)),
            corrections,
            refused_by,
            with_names: None,
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
            reason: reason_at(&dotted),
            path: dotted,
            in_file: Some(in_file),
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
                stores_at: "server.auth.password_hash"
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
                path: "ui.left_widht_pct".to_string(),
                suggestion: Some("ui.left_width_pct".to_string())
            }
        );
        assert!(
            err.to_string().contains("did you mean ui.left_width_pct?"),
            "{err}"
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
                matches!(lookup(path), Err(KeyError::Malformed(_))),
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

    #[test]
    fn tables_secrets_and_the_hand_managed_sections_are_not_set_from_the_command_line() {
        assert!(parse_value(&lookup("server.auth").unwrap(), "x").is_err());
        let err = parse_value(&lookup("server.auth.password").unwrap(), "hunter2").unwrap_err();
        assert!(err.contains("never given on the command line"), "{err}");
        let err = parse_value(&lookup("projects").unwrap(), "[]").unwrap_err();
        assert!(err.contains("projects"), "{err}");
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
        assert!(
            problems.iter().all(|p| p.message.contains("api")),
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
}

#[cfg(test)]
mod get_reports_what_dux_uses_tests {
    use super::*;

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
