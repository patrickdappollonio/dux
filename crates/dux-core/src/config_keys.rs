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
    let segments = split_path(path)?;
    if let Some((_, policy)) = VIRTUAL_KEYS.iter().find(|(name, _)| *name == path) {
        return Ok(Key {
            path: segments,
            policy: *policy,
            shape: Shape::Text,
        });
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
    /// What is still wrong with `[server.auth]` after the write: a set may
    /// repair one of several broken values, and says what is left.
    pub remaining_problems: Vec<String>,
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
    let (previous, remaining_problems) =
        crate::config_write::mutate_config_file_repairing(config_path, missing, |doc| {
            let previous = value_in_doc(doc, path);
            prepare_provider(doc, path)?;
            set_in_doc(doc, path, value)?;
            check_provider_command(doc, path)?;
            Ok(previous)
        })?;
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
    let provider = file
        .get("providers")
        .and_then(|providers| providers.get(name))
        .cloned()
        .and_then(|provider| provider.try_into::<ProviderCommandConfig>().ok());
    if provider.is_none_or(|provider| provider.command.trim().is_empty()) {
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
    let problems = crate::config_auth::rule_problems_of(toml::Value::Table(policy.clone()));
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

/// Check `password` against the minimums in `config_path`, hash it, and store
/// the hash at `server.auth.password_hash` through the coordinated mutation
/// path. The hash is made before the file lock is taken, so a slow hash never
/// holds up another writer. Returns the password's strength.
pub fn set_password(
    config_path: &Path,
    password: &Password,
    user_inputs: &[&str],
) -> Result<Strength, SetPasswordError> {
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
) -> Result<Strength, SetPasswordError> {
    let policy = current_password_policy(config_path).map_err(SetPasswordError::Failed)?;
    let check = crate::auth::check_minimums(password, &policy, user_inputs);
    if !check.passes() {
        return Err(SetPasswordError::BelowMinimums(check));
    }
    let hash = crate::auth::hash_password(password)
        .map_err(|e| SetPasswordError::Failed(anyhow::Error::msg(e.to_string())))?;
    let path: Vec<String> = password_hash_path();
    write_value(config_path, missing, &path, Value::from(hash))
        .map_err(SetPasswordError::Failed)?;
    Ok(check.strength)
}

/// Store a [`SecretKind::Text`] value (an environment value) as given,
/// through the coordinated mutation path. The caller never prints it.
pub fn set_secret_text(config_path: &Path, key: &Key, value: &Password) -> Result<()> {
    set_secret_text_with(config_path, MissingConfig::CreateDocumented, key, value)
}

/// [`set_secret_text`] with a choice of what a missing file means.
pub fn set_secret_text_with(
    config_path: &Path,
    missing: MissingConfig<'_>,
    key: &Key,
    value: &Password,
) -> Result<()> {
    if key.policy != WritePolicy::Secret(SecretKind::Text) {
        anyhow::bail!("{} is not a setting stored as typed text", key.dotted());
    }
    write_value(config_path, missing, &key.path, Value::from(value.expose())).map(|_| ())
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
    /// The file leaves it out; dux uses this default (as above).
    Default(String),
    /// The file leaves it out and it has no default (an optional setting,
    /// or a provider or env entry that does not exist).
    Unset,
}

/// The value `key` has in the file `raw`, or its default when the file
/// leaves it out. Reads the text itself rather than a loaded config, so it
/// works on a file dux would refuse to start with: `get` is how you look at
/// the broken part. For a [`WritePolicy::Secret`] key it reads where the
/// secret is stored (the password's hash).
pub fn get(raw: &str, key: &Key) -> Result<GetValue> {
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
    let mut node = Some(toml::Value::Table(doc));
    for segment in &path {
        node = node.and_then(|n| n.get(segment).cloned());
    }
    if let Some(value) = node {
        return Ok(GetValue::Set(render(&value)));
    }
    let mut default = Some(default_tree());
    for segment in &path {
        default = default.and_then(|n| n.get(segment).cloned());
    }
    Ok(match default {
        Some(serde_json::Value::Null) | None => GetValue::Unset,
        Some(json) => match toml::Value::try_from(json) {
            Ok(value) => GetValue::Default(render(&value)),
            Err(_) => GetValue::Unset,
        },
    })
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
        assert!(strength.score >= 2);
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
        let raw = "[server]\nport = 4000\n\n[server.auth]\npassword_hash = \"$argon2id$x\"\n";
        assert_eq!(
            get(raw, &lookup("server.port").unwrap()).unwrap(),
            GetValue::Set("4000".into())
        );
        assert_eq!(
            get(raw, &lookup("server.host").unwrap()).unwrap(),
            GetValue::Default("127.0.0.1".into())
        );
        assert_eq!(
            get(raw, &lookup("server.auth.password").unwrap()).unwrap(),
            GetValue::Set("$argon2id$x".into()),
            "get works on a file dux would refuse, and prints the hash"
        );
        assert_eq!(
            get(raw, &lookup("providers.nothere.command").unwrap()).unwrap(),
            GetValue::Unset
        );
        let GetValue::Set(table) = get(raw, &lookup("server.auth").unwrap()).unwrap() else {
            panic!("a table is printed");
        };
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
        assert!(report.remaining_problems[0].contains("max_tracked_addresses"));
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
            report.remaining_problems[0].contains("require"),
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
}
