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
/// type and range decide (a port above 65535, a negative count).
fn check_alone(key: &Key, value: &Value) -> Result<(), String> {
    let mut doc = DocumentMut::new();
    set_in_doc(&mut doc, &key.path, value.clone()).map_err(|e| e.to_string())?;
    toml::from_str::<Config>(&doc.to_string())
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
}

/// Write one plain setting into the config file at `config_path`, through
/// the coordinated mutation path. `raw` is parsed by [`parse_value`].
pub fn set_plain(config_path: &Path, key: &Key, raw: &str) -> Result<SetReport> {
    let value = parse_value(key, raw).map_err(anyhow::Error::msg)?;
    write_value(config_path, &key.path, value)
}

fn write_value(config_path: &Path, path: &[String], value: Value) -> Result<SetReport> {
    let now = bare(&value);
    let previous = crate::config_write::mutate_config_file(config_path, |doc| {
        let previous = value_in_doc(doc, path);
        set_in_doc(doc, path, value)?;
        Ok(previous)
    })?;
    Ok(SetReport {
        path: path.join("."),
        previous,
        now,
    })
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

/// The password minimums `config_path` asks for right now. A file with no
/// readable `[server.auth]` (missing, or broken) gets the defaults; the write
/// itself re-checks the whole section before anything lands.
pub fn current_password_policy(config_path: &Path) -> crate::auth::PasswordPolicy {
    std::fs::read_to_string(config_path)
        .ok()
        .and_then(|raw| crate::config::auth_section_of(&raw).ok())
        .unwrap_or_default()
        .password_policy()
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
    let policy = current_password_policy(config_path);
    let check = crate::auth::check_minimums(password, &policy, user_inputs);
    if !check.passes() {
        return Err(SetPasswordError::BelowMinimums(check));
    }
    let hash = crate::auth::hash_password(password)
        .map_err(|e| SetPasswordError::Failed(anyhow::Error::msg(e.to_string())))?;
    let path: Vec<String> = password_hash_path();
    write_value(config_path, &path, Value::from(hash)).map_err(SetPasswordError::Failed)?;
    Ok(check.strength)
}

/// Store a [`SecretKind::Text`] value (an environment value) as given,
/// through the coordinated mutation path. The caller never prints it.
pub fn set_secret_text(config_path: &Path, key: &Key, value: &Password) -> Result<()> {
    if key.policy != WritePolicy::Secret(SecretKind::Text) {
        anyhow::bail!("{} is not a setting stored as typed text", key.dotted());
    }
    write_value(config_path, &key.path, Value::from(value.expose())).map(|_| ())
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
            crate::config::redact_toml_error(&e.to_string())
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
}
