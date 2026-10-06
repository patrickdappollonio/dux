use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};

use crate::config::{self, Config, DuxPaths};
use crate::git;
use crate::keybindings::RuntimeBindings;
use crate::logger;
use crate::storage::SessionStore;
use dux_core::project_browser::canonical_or_original;
use dux_core::text::count_of;

// ---------------------------------------------------------------------------
// CLI dispatch
// ---------------------------------------------------------------------------

/// Whether the arguments after `dux config` ask for help: `-h` or `--help`
/// anywhere, except after a literal `--`, where words are a value's.
pub fn asks_for_help(args: &[String]) -> bool {
    args.iter()
        .take_while(|a| a.as_str() != "--")
        .any(|a| a == "-h" || a == "--help")
}

pub fn run(args: &[String], paths: &DuxPaths) -> Result<()> {
    let sub = args.first().map(|s| s.as_str()).unwrap_or("");
    match sub {
        "reset" => {
            let all = args[1..].iter().any(|a| a == "--all");
            reject_unknown_flags(&args[1..], &["--all"])?;
            run_reset(paths, all)
        }
        "diff" => {
            let raw = args[1..].iter().any(|a| a == "--raw");
            reject_unknown_flags(&args[1..], &["--raw"])?;
            run_diff(paths, raw)
        }
        "regenerate" => {
            let yes = args[1..].iter().any(|a| a == "--yes");
            let show = args[1..].iter().any(|a| a == "--show");
            reject_unknown_flags(&args[1..], &["--yes", "--show"])?;
            run_regenerate(paths, yes, show)
        }
        "restore-docs" => {
            let yes = args[1..].iter().any(|a| a == "--yes");
            let show = args[1..].iter().any(|a| a == "--show");
            reject_unknown_flags(&args[1..], &["--yes", "--show"])?;
            run_restore_docs(paths, yes, show)
        }
        "path" => {
            println!("{}", paths.config_path.display());
            Ok(())
        }
        "get" => crate::config_cli::run_get(
            &args[1..],
            paths,
            &mut std::io::stdout(),
            &mut std::io::stderr(),
        ),
        "set" => crate::config_cli::run_set(
            &args[1..],
            paths,
            &mut crate::config_cli::TerminalSecrets,
            &mut std::io::stdout(),
        ),
        "" | "--help" | "-h" => {
            print_config_help();
            Ok(())
        }
        other => bail!("unknown config subcommand: {other}\nRun `dux config --help` for usage."),
    }
}

fn reject_unknown_flags(args: &[String], known: &[&str]) -> Result<()> {
    for arg in args {
        if arg.starts_with('-') && !known.contains(&arg.as_str()) {
            bail!("unknown flag: {arg}");
        }
    }
    Ok(())
}

fn print_config_help() {
    println!(
        "\
dux config: manage the dux configuration file

Subcommands:
  dux config path          Print the config file path
  dux config get <setting> Print one setting's value, for example
                           `dux config get server.port`. Values that can
                           hold secrets (env, projects) need --show
  dux config set <setting> <value>
                           Change one setting, keeping the file's comments,
                           and tell a running dux to reload. Lists are one
                           TOML array: '[\"a\", \"b\"]'
  dux config set server.auth.password
                           Set the web UI password: asked for twice without
                           echo, with a strength meter. Never a command-line
                           argument; add --stdin to pipe it in instead.
                           env.<NAME> values are asked for the same way
  dux config diff          Show settings that differ from defaults (summary;
                           [env] and project details are summarized, never
                           printed, so it is safe to paste into a bug report)
  dux config diff --raw    Show a unified diff against the default config.
                           This prints the WHOLE config, [env] values included:
                           redact it before sharing.
  dux config reset         Remove config and logs (keeps agents and worktrees)
  dux config reset --all   Full factory reset: remove config, logs, sessions, and worktrees
                           (deletes nothing if a program dux started still runs)
  dux config regenerate    Preview a fresh default config (shows diff; [env]
                           and other sensitive values are hidden unless you
                           add --show)
  dux config regenerate --yes
                           Overwrite the config file with fresh defaults
  dux config restore-docs  Preview re-adding the explanatory comments to your
                           config, keeping every value you have set (hides
                           values like regenerate; --show reveals them)
  dux config restore-docs --yes
                           Apply it (writes a timestamped backup first)"
    );
}

// ---------------------------------------------------------------------------
// dux config reset
// ---------------------------------------------------------------------------

fn run_reset(paths: &DuxPaths, all: bool) -> Result<()> {
    run_reset_reporting(paths, all)?;
    println!("reset complete");
    Ok(())
}

/// A folder or program in the way of the factory reset, and why.
#[derive(Debug)]
struct ResetLeftover {
    path: PathBuf,
    reason: String,
}

/// Why `reset --all` deleted nothing: something is still running (a program
/// dux started, or any program of this user working in a folder the reset
/// would delete), or a check whose answer dux could not get, which counts the
/// same because it cannot know nothing runs. Carried as the command's error,
/// so it is printed once and the exit code is 1.
#[derive(Debug, Default)]
struct ResetRefused {
    running: Vec<ResetLeftover>,
    unknown: Option<String>,
}

impl ResetRefused {
    fn unknown(reason: impl std::fmt::Display) -> Self {
        Self {
            running: Vec::new(),
            unknown: Some(format!(
                "could not check which programs are still running: {reason}"
            )),
        }
    }
}

impl std::fmt::Display for ResetRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(reason) = &self.unknown {
            return write!(
                f,
                "dux config reset --all deleted nothing: {reason}. Fix that and run the reset \
                 again, or delete the files yourself."
            );
        }
        write!(
            f,
            "dux config reset --all deleted nothing: something is still running. Stop what is \
             listed, then run the reset again."
        )?;
        for program in &self.running {
            write!(f, "\n  {}: {}", program.path.display(), program.reason)?;
        }
        Ok(())
    }
}

impl std::error::Error for ResetRefused {}

/// Why `reset --all` stopped part-way: a folder could not be removed. Nothing
/// after it was touched, the database and the config included, so a second
/// run starts from what is left.
#[derive(Debug)]
struct ResetFailed {
    removed: Vec<PathBuf>,
    failed: ResetLeftover,
}

impl std::fmt::Display for ResetFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "dux config reset --all stopped part-way: {} could not be removed ({}). The \
             database and the config were kept.",
            self.failed.path.display(),
            self.failed.reason
        )?;
        if self.removed.is_empty() {
            write!(f, " Nothing was removed before it.")?;
        } else {
            write!(f, " Already removed:")?;
            for path in &self.removed {
                write!(f, "\n  {}", path.display())?;
            }
        }
        write!(
            f,
            "\nSomething may still be using it: stop whatever is running there, then delete it \
             yourself (if git still lists it as a worktree of its project, run `git worktree \
             remove --force -- {}` in that project's folder) and run the reset again.",
            self.failed.path.display()
        )
    }
}

impl std::error::Error for ResetFailed {}

/// The reset itself. Prints what it removed as it goes.
fn run_reset_reporting(paths: &DuxPaths, all: bool) -> Result<()> {
    let log_path = resolve_reset_log_path(paths);

    if all {
        reset_agent_data(paths)?;
    }

    if let Err(error) = remove_file_with_message(&log_path) {
        eprintln!("warning: {error}");
    }
    prune_empty_ancestors(&log_path, &paths.root)?;
    remove_file_with_message(&paths.config_path)?;
    prune_empty_ancestors(&paths.config_path, &paths.root)?;

    // The lockfile (`dux.lock`) is left in place: unlinking it while holding
    // the flock would orphan the inode, letting a new process create and flock
    // a fresh file at the same path and break the single-instance guarantee.
    // `remove_root_if_empty` therefore skips the root when the lockfile is the
    // sole remaining entry.
    remove_root_if_empty_with_message(&paths.root)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// dux config diff
// ---------------------------------------------------------------------------

fn run_diff(paths: &DuxPaths, raw: bool) -> Result<()> {
    if !paths.config_path.exists() {
        println!("no config file found at {}", paths.config_path.display());
        return Ok(());
    }

    let current_raw =
        fs::read_to_string(&paths.config_path).with_context_path(&paths.config_path)?;
    let mut current: Config = toml::from_str(&current_raw).map_err(|e| {
        anyhow!(
            "{}: {}",
            paths.config_path.display(),
            dux_core::config::describe_toml_error(&current_raw, &e)
        )
    })?;
    // The text it was read from, so a name the summary may not print is
    // placed by its line.
    current.source_text = dux_core::config::SourceText::of(&current_raw);

    if raw {
        run_diff_raw(&current_raw, &current)?;
    } else {
        run_diff_summary(&current)?;
    }
    Ok(())
}

fn run_diff_raw(_current_raw: &str, current: &Config) -> Result<()> {
    print!("{}", raw_diff_text(current));
    Ok(())
}

/// What `dux config diff --raw` prints for `current`.
pub(crate) fn raw_diff_text(current: &Config) -> String {
    let bindings = RuntimeBindings::from_keys_config(&current.keys);
    let default_rendered = config::render_default_config();
    // Re-render current config to normalize it before diffing.
    let current_rendered = render_config_for_diff(current, &bindings);
    if current_rendered == default_rendered {
        return "config matches defaults, so there are no differences\n".to_string();
    }
    unified_diff("default", "current", &default_rendered, &current_rendered)
}

fn run_diff_summary(current: &Config) -> Result<()> {
    let changes = collect_config_changes(current);
    if changes.is_empty() {
        println!("config matches defaults, so there are no differences");
    } else {
        for line in &changes {
            println!("  {line}");
        }
    }
    Ok(())
}

/// Every setting whose current value differs from the default, as display lines.
///
/// Derived, never hand-maintained: `current` and [`Config::default()`] are both
/// projected to `serde_json::Value` and walked structurally, so a config key
/// added anywhere in the struct tree is reported without being registered here.
///
/// `serde_json` and not `toml` on purpose. TOML has no null and its serializer
/// omits a `None` struct field, which would make every default-`None` setting
/// (`defaults.start_directory`, the optional provider fields) invisible to the
/// comparison. JSON keeps them as an explicit null.
///
/// What is compared is the parsed file against [`Config::default()`]: neither
/// `load_config` nor `ProvidersConfig::ensure_defaults` runs, so the summary
/// reports what the file says rather than what dux normalizes it into, with no
/// value clamping and no shipped provider injected into a config that does not
/// name it.
pub(crate) fn collect_config_changes(current: &Config) -> Vec<String> {
    let (Ok(default_json), Ok(current_json)) = (
        serde_json::to_value(Config::default()),
        serde_json::to_value(current),
    ) else {
        return Vec::new();
    };

    // Every path is printed through the one formatter, against the file's
    // own text where the config carries it (for the line a hidden name is
    // on): this summary is meant to be pasted, so a name that is not a
    // setting name (a token pasted where a key goes) never reaches it.
    let raw = current.source_text.as_str().unwrap_or_default();
    let mut found: Vec<(String, String)> = Vec::new();
    let mut path: Vec<String> = Vec::new();
    diff_node(raw, &mut found, &mut path, &default_json, &current_json);

    // Map iteration order differs by container (`IndexMap` for providers and
    // macros, `BTreeMap` for env and keys), so the order must be imposed here
    // rather than inherited. Sorting on the structural path, not on the rendered
    // line, keeps the ordering a property of the setting and not of its value.
    found.sort();
    found.into_iter().map(|(_, line)| line).collect()
}

/// What the differ does with one subtree.
///
/// There is deliberately no third "ignore this subtree" policy: a setting dux
/// reads and never reports is silent drift. Something too sensitive or too
/// unstable to print is [`Policy::Summarize`]d, which still tells the user that
/// it changed.
enum Policy {
    /// An ordinary settings subtree: descend and report the leaves that differ.
    Recurse,
    /// Report that the subtree changed, never descend, never format its values.
    Summarize(Summary),
}

/// How a summarized subtree describes itself.
enum Summary {
    /// `env: changed`. The bare fact, with no shape to it at all.
    Changed,
    /// `macros: 2 macros configured`, for the given singular and plural nouns.
    Count(&'static str, &'static str),
}

/// How a key present on only one side is reported.
enum MissingStyle {
    /// `providers.foo: (added)` / `providers.foo: (removed)`. For a table whose
    /// entries are whole settings blocks, where printing the block would be
    /// noise.
    Marker,
    /// `keys.quit: (new) -> [ctrl-q]` / `keys.quit: [ctrl-q] -> (removed)`.
    Valued,
}

/// The policy for the subtree at `path`.
fn policy_for(path: &[String]) -> Policy {
    use dux_core::config_keys::SensitiveAuthSetting;
    let segments: Vec<&str> = path.iter().map(String::as_str).collect();
    match segments.as_slice() {
        // Holds API tokens. The value must never reach the terminal, a log, or a
        // pasted bug report, so this reports the fact and nothing else.
        ["env"] => Policy::Summarize(Summary::Changed),
        // An array index is not a stable identity and `ProjectConfig::id` can be
        // generated at deserialize time, so there is no honest per-project path
        // to print. Projects also carry their own `env`, which must stay
        // unprinted for the reason above.
        ["projects"] => Policy::Summarize(Summary::Count("project", "projects")),
        // A macro body is arbitrary user prose, frequently long and multi-line.
        // Counting them is what this command has always done.
        ["macros"] => Policy::Summarize(Summary::Count("macro", "macros")),
        // The auth settings no summary prints (the one list the previews
        // read too): the hash only as changed, the addresses as a count.
        _ => match dux_core::config_keys::sensitive_auth_setting(path) {
            Some(SensitiveAuthSetting::PasswordHash) => Policy::Summarize(Summary::Changed),
            Some(SensitiveAuthSetting::BlockedAddresses) => {
                Policy::Summarize(Summary::Count("address", "addresses"))
            }
            None => Policy::Recurse,
        },
    }
}

/// How a key missing from one side of the subtree at `path` is reported.
fn missing_style_for(path: &[String]) -> MissingStyle {
    let segments: Vec<&str> = path.iter().map(String::as_str).collect();
    match segments.as_slice() {
        ["providers"] => MissingStyle::Marker,
        _ => MissingStyle::Valued,
    }
}

fn diff_node(
    raw: &str,
    found: &mut Vec<(String, String)>,
    path: &mut Vec<String>,
    default: &serde_json::Value,
    current: &serde_json::Value,
) {
    if default == current {
        return;
    }

    match policy_for(path) {
        Policy::Summarize(summary) => {
            let dotted = dux_core::config::shown_path(raw, path);
            let line = match summary {
                Summary::Changed => format!("{dotted}: changed"),
                Summary::Count(singular, plural) => {
                    format!(
                        "{dotted}: {} configured",
                        dux_core::text::count_of_with(collection_len(current), singular, plural)
                    )
                }
            };
            found.push((dotted, line));
        }
        Policy::Recurse => match (default, current) {
            (serde_json::Value::Object(default_map), serde_json::Value::Object(current_map)) => {
                let style = missing_style_for(path);
                let names: BTreeSet<&String> =
                    default_map.keys().chain(current_map.keys()).collect();
                for name in names {
                    path.push(name.clone());
                    match (default_map.get(name), current_map.get(name)) {
                        (Some(d), Some(c)) => diff_node(raw, found, path, d, c),
                        (Some(d), None) => {
                            push_missing(raw, found, path, &style, Side::DefaultOnly, d)
                        }
                        (None, Some(c)) => {
                            push_missing(raw, found, path, &style, Side::CurrentOnly, c)
                        }
                        (None, None) => {}
                    }
                    path.pop();
                }
            }
            // Every other shape, arrays included, is one value. `terminal.args`
            // and `server.allowed_hosts` are settings in their own right, not
            // parents of a `terminal.args.0`.
            _ => {
                let dotted = dux_core::config::shown_path(raw, path);
                let line = format!(
                    "{dotted}: {} -> {}",
                    format_value(raw, path, default),
                    format_value(raw, path, current)
                );
                found.push((dotted, line));
            }
        },
    }
}

/// Which side of the comparison holds a key the other side lacks.
enum Side {
    DefaultOnly,
    CurrentOnly,
}

fn push_missing(
    raw: &str,
    found: &mut Vec<(String, String)>,
    path: &[String],
    style: &MissingStyle,
    side: Side,
    value: &serde_json::Value,
) {
    let dotted = dux_core::config::shown_path(raw, path);
    let line = match (style, side) {
        (MissingStyle::Marker, Side::CurrentOnly) => format!("{dotted}: (added)"),
        (MissingStyle::Marker, Side::DefaultOnly) => format!("{dotted}: (removed)"),
        (MissingStyle::Valued, Side::CurrentOnly) => {
            format!("{dotted}: (new) -> {}", format_value(raw, path, value))
        }
        (MissingStyle::Valued, Side::DefaultOnly) => {
            format!("{dotted}: {} -> (removed)", format_value(raw, path, value))
        }
    };
    found.push((dotted, line));
}

fn collection_len(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Array(items) => items.len(),
        serde_json::Value::Object(map) => map.len(),
        _ => 0,
    }
}

/// Render one value the way the summary shows it: through the one value
/// printer (so a value at or below a key the formatter does not name is
/// never printed), unquoted, one line, truncated.
fn format_value(raw: &str, path: &[String], value: &serde_json::Value) -> String {
    use dux_core::config_keys::{NOT_SHOWN, ValueForm, printed_value};
    if dux_core::config::path_is_hidden(path) {
        return NOT_SHOWN.to_string();
    }
    let rendered = match value {
        // An absent optional setting. Matches what this command has always
        // printed for an unset `defaults.start_directory`.
        serde_json::Value::Null => "(unset)".to_string(),
        value => toml::Value::try_from(value)
            .ok()
            .and_then(|value| printed_value(raw, path, &value, ValueForm::Summary))
            .unwrap_or_else(|| NOT_SHOWN.to_string()),
    };
    truncate_display(&rendered, 40)
}

// ---------------------------------------------------------------------------
// dux config regenerate
// ---------------------------------------------------------------------------

#[allow(deprecated)] // blessed sync-direct: `dux config regenerate` is a CLI-only, one-shot boot tool
fn run_regenerate(paths: &DuxPaths, yes: bool, show: bool) -> Result<()> {
    let fresh = config::render_default_config();

    if !yes {
        if paths.config_path.exists() {
            let current =
                fs::read_to_string(&paths.config_path).with_context_path(&paths.config_path)?;
            if current == fresh {
                println!("config already matches defaults, so there is nothing to do");
                return Ok(());
            }
            print!("{}", regenerate_preview(&current, &fresh, show));
            if let Some(note) = regenerate_password_note(&current) {
                println!("\n{note}");
            }
            println!("\nRun `dux config regenerate --yes` to overwrite with these defaults.");
        } else {
            println!("no config file exists; regenerate --yes will create one at:");
            println!("  {}", paths.config_path.display());
        }
        return Ok(());
    }

    let note = fs::read_to_string(&paths.config_path)
        .ok()
        .and_then(|current| regenerate_password_note(&current));
    paths.ensure_dirs()?;
    dux_core::config_write::write_config_secure(&paths.config_path, &fresh)
        .with_context_path(&paths.config_path)?;
    println!("config regenerated at {}", paths.config_path.display());
    if let Some(note) = note {
        println!("{note}");
    }
    Ok(())
}

/// Fresh defaults have no password, so regenerating a config that has one
/// opens the web UI to whoever can reach it. Said in words rather than left
/// to one line of the diff.
///
/// Looks at the text itself too, so a password inside a section dux cannot
/// load (or a file that is not TOML) is still warned about.
fn regenerate_password_note(current: &str) -> Option<&'static str> {
    let loaded = dux_core::config::auth_section_of(current)
        .ok()
        .is_some_and(|auth| auth.has_password());
    let written = current.lines().any(|line| {
        let Some(at) = line.find("password_hash") else {
            return false;
        };
        let Some(value) = line[at + "password_hash".len()..]
            .trim_start()
            .strip_prefix('=')
        else {
            return false;
        };
        let value = value.trim();
        !value.is_empty() && !value.starts_with("\"\"") && !value.starts_with("''")
    });
    (loaded || written).then_some(()).map(|_| {
        "Note: this removes the web UI password (server.auth.password_hash). Set it again \
             afterwards with `dux config set server.auth.password`."
    })
}

// ---------------------------------------------------------------------------
// dux config restore-docs
// ---------------------------------------------------------------------------

/// Re-apply the commented template to the existing config, keeping every value.
///
/// Non-destructive by default (preview only), mirroring `dux config regenerate`:
/// `--yes` commits. Unlike `regenerate`, this never falls back to defaults: an
/// unparseable config is refused outright, because the whole point of the
/// command is to be the safe alternative to a defaults-based rewrite.
#[allow(deprecated)] // blessed sync-direct: CLI-only, one-shot, runs before any engine/queue exists
fn run_restore_docs(paths: &DuxPaths, yes: bool, show: bool) -> Result<()> {
    if !paths.config_path.exists() {
        println!("no config file found at {}", paths.config_path.display());
        println!("dux writes a fully commented config the first time it starts.");
        return Ok(());
    }

    let raw = fs::read_to_string(&paths.config_path).with_context_path(&paths.config_path)?;

    // REFUSE on an unparseable config. Falling through to a defaults-based
    // regeneration here would destroy exactly the data (projects, macros,
    // provider commands, env values) this command exists to protect.
    let restored = config::restore_documentation(&raw).map_err(|e| {
        anyhow!(
            "{e:#}\n\n\
             Your config.toml has NOT been modified.\n\
             Fix the syntax error at {} and run this again. If you would rather \
             start over from defaults and lose your current settings, that is \
             `dux config regenerate --yes`.",
            paths.config_path.display()
        )
    })?;

    if restored.is_noop(&raw) {
        println!("config documentation is already up to date, so there is nothing to do");
        return Ok(());
    }

    if !yes {
        print!("{}", restore_docs_preview(&raw, &restored.text, show));
        print_restore_report(&restored);
        println!("\nRun `dux config restore-docs --yes` to apply this (a timestamped backup");
        println!("of your current config is written first).");
        return Ok(());
    }

    // The config file exists, so its folder does; tighten it before the write
    // lock, the backup and the new file are created in it.
    dux_core::file_modes::create_private_dir_all(&paths.root)?;

    // Back up BEFORE committing. The writer below is atomic, which protects
    // against a torn file, but not against "the result was not what I wanted".
    // Both happen inside the config write lock, on the file as it is NOW (a
    // `dux config set` that landed since the preview above included), so the
    // [server.auth] written is the one the file holds at that moment.
    let (backup_path, restored) =
        dux_core::config_write::replace_config_file(&paths.config_path, |current| {
            let current = current.ok_or_else(|| {
                anyhow!(
                    "{} disappeared; nothing was written",
                    paths.config_path.display()
                )
            })?;
            let restored = config::restore_documentation(current)?;
            let backup_path = backup_config(&paths.config_path, current)?;
            Ok((restored.text.clone(), (backup_path, restored)))
        })
        .with_context_path(&paths.config_path)?;

    println!("documentation restored in {}", paths.config_path.display());
    println!("backup of the previous config: {}", backup_path.display());
    print_restore_report(&restored);
    Ok(())
}

/// Print what the restore changed beyond adding comments. A dropped section is
/// reported even though its data was inert: a silent drop is still data loss.
fn print_restore_report(restored: &config::RestoredConfig) {
    if !restored.dropped.is_empty() {
        println!("\nRemoved (dux no longer reads these):");
        for path in &restored.dropped {
            println!("  [{path}]");
        }
    }
    if !restored.preserved.is_empty() {
        println!("\nKept as-is (not settings dux knows, carried over unchanged):");
        for path in &restored.preserved {
            println!("  {path}");
        }
    }
    // Not reachable through any config the canonical renderer can produce, and
    // printed anyway: a key that could not be placed is data loss, and the one
    // thing worse than losing it is losing it quietly.
    if !restored.unplaceable.is_empty() {
        println!(
            "\nCOULD NOT BE KEPT (this is a dux bug, please report it, and \
             recover these from the backup above):"
        );
        for path in &restored.unplaceable {
            println!("  {path}");
        }
    }
}

/// Write `raw` beside the config as `config.toml.backup-<UTC timestamp>`.
///
/// Never overwrites: if a backup with this second-resolution name already
/// exists, a counter is appended, so repeated runs cannot clobber an earlier
/// safety copy. Created 0600 like the config itself, since it holds the same
/// potential secrets.
fn backup_config(config_path: &Path, raw: &str) -> Result<PathBuf> {
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let base = config_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.toml".to_string());
    let dir = config_path
        .parent()
        .ok_or_else(|| anyhow!("config path {} has no parent", config_path.display()))?;

    let mut candidate = dir.join(format!("{base}.backup-{stamp}"));
    let mut counter = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{base}.backup-{stamp}-{counter}"));
        counter += 1;
    }

    // Called inside the config write lock (see `run_restore_docs`); 0600.
    dux_core::config_write::write_beside_config_locked(&candidate, raw)
        .with_context_path(&candidate)?;
    Ok(candidate)
}

// ---------------------------------------------------------------------------
// Diff helpers
// ---------------------------------------------------------------------------

/// Truncate a display string, replacing the end with "..." if too long.
fn truncate_display(s: &str, max: usize) -> String {
    // For multiline values just show first line.
    let first_line = s.lines().next().unwrap_or(s);
    if first_line.chars().count() > max {
        let truncated: String = first_line.chars().take(max).collect();
        format!("{truncated}...")
    } else if s.contains('\n') {
        format!("{first_line}...")
    } else {
        s.to_string()
    }
}

fn render_config_for_diff(config: &Config, bindings: &RuntimeBindings) -> String {
    // Use the same render_config used for default to ensure comparable output.
    // This is a re-render of the current config through the canonical renderer.
    config::render_config_with(config, bindings)
}

fn unified_diff(label_a: &str, label_b: &str, a: &str, b: &str) -> String {
    let diff = similar::TextDiff::from_lines(a, b);
    let mut out = format!("--- {label_a}\n+++ {label_b}\n");
    for hunk in diff.unified_diff().context_radius(3).iter_hunks() {
        out.push_str(&format!("{hunk}\n"));
    }
    out
}

/// The user's file `raw` as a preview may print it: every line through the
/// one value printer's rules (see `config_keys::shown_file_text`), so a
/// plaintext password is never shown and, without `show`, nothing below a
/// hidden key or in `[env]` or `[projects]` either.
fn shown_for_preview(raw: &str, show: bool) -> Result<String> {
    dux_core::config_keys::shown_file_text(raw, show, &crate::config::binding_is_understood)
}

/// The diff of `a` against `b`, each shown as a printer may show it; when
/// either is not TOML, only why nothing of it is shown.
fn preview(label_a: &str, label_b: &str, a: &str, b: &str, show: bool) -> String {
    match (shown_for_preview(a, show), shown_for_preview(b, show)) {
        (Ok(a), Ok(b)) => unified_diff(label_a, label_b, &a, &b),
        (Err(error), _) | (_, Err(error)) => format!("{error:#}\n"),
    }
}

/// What `dux config regenerate` previews: the user's file against the
/// fresh default, each side shown as a printer may show it.
pub(crate) fn regenerate_preview(current: &str, fresh: &str, show: bool) -> String {
    preview("current", "default", current, fresh, show)
}

/// What `dux config restore-docs` previews: the user's file against the
/// documented one, each side shown as a printer may show it.
pub(crate) fn restore_docs_preview(raw: &str, restored: &str, show: bool) -> String {
    preview("current", "restored", raw, restored, show)
}

// ---------------------------------------------------------------------------
// Agent data reset
// ---------------------------------------------------------------------------

fn reset_agent_data(paths: &DuxPaths) -> Result<()> {
    // Folders a standalone agent occupies: the sweep of the whole worktrees
    // root below is otherwise indiscriminate, and nothing stops a user pointing
    // a standalone agent at a directory inside dux's managed area, which dux
    // did not make.
    let mut occupied_folders: Vec<PathBuf> = Vec::new();
    // The same folders as recorded (never resolved), for the link rule: a
    // link in the root is kept when one of these is at or through it.
    let mut recorded_folders: Vec<PathBuf> = Vec::new();
    let seen_welcome = last_seen_welcome(paths);
    let mut sessions: Vec<dux_core::model::AgentSession> = Vec::new();
    // What dux recorded starting, this boot only: a session from another boot
    // is void.
    let mut recorded: Vec<dux_core::process_sessions::StoredSession> = Vec::new();
    if paths.sessions_db_path.exists() {
        let store = SessionStore::open(&paths.sessions_db_path).map_err(|error| {
            ResetRefused::unknown(format_args!(
                "the session database could not be opened: {error}"
            ))
        })?;
        sessions = store.load_sessions().map_err(|error| {
            ResetRefused::unknown(format_args!(
                "the sessions could not be loaded from the database: {error}"
            ))
        })?;
        // A standalone agent's folder is the user's and is never removed, not
        // even by a factory reset; its record goes with the database below
        // like every other agent's.
        //
        // The filter is on the workspace, not on the managed-root path check
        // inside the removal: a standalone agent pointed at a directory under
        // dux's managed root sails past that check and has the ground deleted
        // from under it.
        //
        // Collect in a pass of its own, before any removal: a managed worktree
        // that contains or is such a folder ends in an unconditional
        // `remove_dir_all`, so a half-filled list makes the folder's survival
        // depend on row order.
        for session in &sessions {
            if session.workspace.as_managed().is_none() {
                occupied_folders.push(canonical_or_original(Path::new(session.directory())));
                recorded_folders.push(PathBuf::from(session.directory()));
            }
        }
        // A project's repository is the user's too: a worktree that holds one
        // (a clone made inside it) is kept.
        if let Ok(projects) = store.load_projects() {
            occupied_folders.extend(
                projects
                    .iter()
                    .map(|project| canonical_or_original(Path::new(&project.path))),
            );
            recorded_folders.extend(projects.iter().map(|project| PathBuf::from(&project.path)));
        }
        recorded = store
            .load_process_registry_strict()
            .map_err(|error| ResetRefused::unknown(format_args!("{error}")))?
            .into_iter()
            .filter(|entry| entry.session.is_this_boot())
            .collect();
    }

    // Everything dux started is stopped first, then one last look decides
    // whether anything is left in the way. Only then is anything deleted: a
    // reset that stopped half-way would leave a confusing mix of old and new.
    let removing = folders_reset_removes(paths, &sessions, &occupied_folders);
    stop_recorded_programs(paths, &recorded)?;
    ensure_nothing_runs(&recorded, &removing)?;

    let mut removed: Vec<PathBuf> = Vec::new();
    for session in &sessions {
        let Some(managed) = session.workspace.as_managed() else {
            continue;
        };
        match remove_session_worktree(paths, managed, &occupied_folders) {
            SessionWorktreeReset::Removed => removed.push(PathBuf::from(&managed.worktree_path)),
            SessionWorktreeReset::Skipped => {}
            SessionWorktreeReset::Left(failed) => {
                return Err(ResetFailed { removed, failed }.into());
            }
        }
    }
    println!("{}", removed_worktrees_line(removed.len()));

    // The sweep that finishes the job: whatever the per-session loop could not
    // account for (a worktree whose row was already gone, a stray directory)
    // goes with the root, except a folder a standalone agent occupies, which
    // removing the root wholesale would undo the filter above for. It stops at
    // the first entry it cannot remove, before the database and config go.
    sweep_worktrees_root(
        &paths.worktrees_root,
        &occupied_folders,
        &recorded_folders,
        &mut removed,
    )?;
    remove_file_with_message(&paths.sessions_db_path)?;
    // The welcome screen was already seen, and a reset does not make it new
    // again: the fresh database starts with that one record.
    if let Some(version) = seen_welcome {
        let kept = SessionStore::open(&paths.sessions_db_path)
            .and_then(|store| store.set_last_seen_version(&version));
        if let Err(error) = kept {
            eprintln!(
                "warning: could not keep the record that you saw the welcome screen, so it \
                 opens again on the next start: {error}"
            );
        }
    }
    Ok(())
}

/// The release whose welcome screen was last seen, read before the database
/// is deleted. Absent when there is no database or it holds no such record.
fn last_seen_welcome(paths: &DuxPaths) -> Option<String> {
    if !paths.sessions_db_path.exists() {
        return None;
    }
    let store = SessionStore::open(&paths.sessions_db_path).ok()?;
    store.last_seen_version().ok().flatten()
}

/// Every folder the reset will remove: each agent's managed worktree under
/// the worktrees root that holds nothing the user owns, and each entry of the
/// root itself that holds nothing the user owns (a standalone agent's folder,
/// a project's repository). Decided once, before anything is ended, so the
/// processes ended and the folders removed are the same set.
fn folders_reset_removes(
    paths: &DuxPaths,
    sessions: &[dux_core::model::AgentSession],
    occupied: &[PathBuf],
) -> Vec<PathBuf> {
    let holds_something = |folder: &Path| {
        occupied
            .iter()
            .any(|kept| dux_core::worktree_ops::folder_contains(folder, kept))
    };
    let mut removing: Vec<PathBuf> = sessions
        .iter()
        .filter_map(|session| session.workspace.as_managed())
        .map(|managed| PathBuf::from(&managed.worktree_path))
        .filter(|worktree| git::is_under(&paths.worktrees_root, worktree))
        .filter(|worktree| !holds_something(worktree))
        .collect();
    // Each entry at its own LEXICAL path. A link entry is only unlinked, so
    // it removes no folder: nothing working in its target is ever ended.
    if let Ok(entries) = fs::read_dir(&paths.worktrees_root) {
        removing.extend(
            entries
                .flatten()
                .filter(|entry| !entry.file_type().is_ok_and(|kind| kind.is_symlink()))
                .map(|entry| entry.path())
                .filter(|entry| !holds_something(entry)),
        );
    }
    // A managed worktree that is itself a link is only unlinked too.
    removing.retain(|folder| {
        !fs::symlink_metadata(folder).is_ok_and(|meta| meta.file_type().is_symlink())
    });
    removing
}

/// The grace a factory reset gives each process before forcing it. A config
/// dux cannot load still gets a reset: the grace falls back to the default
/// rather than stopping the reset over an unrelated setting.
fn reset_grace(paths: &DuxPaths) -> std::time::Duration {
    let timeout_seconds = match dux_core::config::load_config(paths) {
        Ok(config) => config.shutdown_timeout_seconds,
        Err(error) => {
            eprintln!(
                "warning: could not read config.toml, so processes get the default grace \
                 period before they are forced to stop: {error}"
            );
            dux_core::config::Config::default().shutdown_timeout_seconds
        }
    };
    dux_core::config::shutdown_grace(timeout_seconds)
}

/// Stop every program dux recorded starting (this boot only), wherever it
/// works and whoever's folder it is in, with the same SIGTERM, configured
/// grace and SIGKILL as an agent delete's removal. A program is found by the
/// identity dux recorded (its session and start time), never by a guess, and
/// the process list is read strictly: one that cannot be read refuses the
/// reset. One that survives the grace period refuses it too.
fn stop_recorded_programs(
    paths: &DuxPaths,
    recorded: &[dux_core::process_sessions::StoredSession],
) -> std::result::Result<(), ResetRefused> {
    use dux_core::process_sessions as ps;
    if recorded.is_empty() {
        return Ok(());
    }
    let grace = reset_grace(paths);
    let mut running: Vec<ResetLeftover> = Vec::new();
    for entry in recorded {
        let table = ps::read_process_table_strict().map_err(ResetRefused::unknown)?;
        let members = ps::members(
            &table,
            &[entry.session],
            &entry.survivors,
            std::process::id(),
        );
        if members.is_empty() {
            continue;
        }
        let left = ps::end_exactly_with(&members, grace, &ps::read_process_table_strict)
            .map_err(ResetRefused::unknown)?;
        if !left.is_empty() {
            running.push(ResetLeftover {
                path: entry.folder.clone(),
                reason: format!("{} that dux started would not stop", ps::describe(&left)),
            });
        }
    }
    if running.is_empty() {
        Ok(())
    } else {
        Err(ResetRefused {
            running,
            ..Default::default()
        })
    }
}

/// The last look before anything is deleted: nothing dux recorded starting
/// runs any more (a child forked while the rest shut down is still a member),
/// and no program of this user works in a folder the reset is about to remove,
/// recorded or not. Reads the process list fresh and strictly.
fn ensure_nothing_runs(
    recorded: &[dux_core::process_sessions::StoredSession],
    removing: &[PathBuf],
) -> std::result::Result<(), ResetRefused> {
    use dux_core::process_sessions as ps;
    let table = ps::read_process_table_strict().map_err(ResetRefused::unknown)?;
    let mut running: Vec<ResetLeftover> = Vec::new();
    for entry in recorded {
        let members = ps::members(
            &table,
            &[entry.session],
            &entry.survivors,
            std::process::id(),
        );
        if !members.is_empty() {
            running.push(ResetLeftover {
                path: entry.folder.clone(),
                reason: format!(
                    "{} that dux started is still running",
                    ps::describe(&members)
                ),
            });
        }
    }
    if !removing.is_empty() {
        for who in ps::occupants_of(&table, removing).map_err(ResetRefused::unknown)? {
            running.push(ResetLeftover {
                path: who.cwd,
                reason: format!("process {} ({}) is working there", who.pid, who.command),
            });
        }
    }
    if running.is_empty() {
        Ok(())
    } else {
        Err(ResetRefused {
            running,
            ..Default::default()
        })
    }
}

/// Clear the managed worktrees root entry by entry, leaving every entry that
/// CONTAINS OR IS a folder a standalone agent occupies. It stops at the first
/// entry that could not be removed, naming it and everything removed so far
/// (`removed` carries the earlier removals in and the sweep's own out). The
/// root itself goes when nothing is left in it.
///
/// Containment, not equality: an agent pointed at `worktrees/a/b` must keep
/// `worktrees/a` too, or removing the parent takes the child with it. Compared
/// canonically, so a symlinked spelling cannot slip past.
fn sweep_worktrees_root(
    root: &Path,
    occupied: &[PathBuf],
    recorded: &[PathBuf],
    removed: &mut Vec<PathBuf>,
) -> std::result::Result<(), ResetFailed> {
    if !root.exists() {
        return Ok(());
    }
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            return Err(ResetFailed {
                removed: std::mem::take(removed),
                failed: ResetLeftover {
                    path: root.to_path_buf(),
                    reason: format!("its contents could not be read: {error}"),
                },
            });
        }
    };
    let mut kept = 0usize;
    for entry in entries.flatten() {
        // Every entry is judged at its own LEXICAL path. A link is only
        // unlinked (its target stays), so it is kept only when something is
        // recorded at or through the link itself, never for what its target
        // holds.
        let path = entry.path();
        let link = entry.file_type().is_ok_and(|kind| kind.is_symlink());
        let keep = if link {
            recorded
                .iter()
                .any(|folder| dux_core::engine::recorded_at_or_through_link(&path, folder))
        } else {
            occupied
                .iter()
                .any(|folder| dux_core::worktree_ops::folder_contains(&path, folder))
        };
        if keep {
            kept += 1;
            continue;
        }
        // A link is removed as a link, whatever it points at.
        let gone = if !link && entry.path().is_dir() {
            fs::remove_dir_all(entry.path())
        } else {
            fs::remove_file(entry.path())
        };
        match gone {
            Ok(()) => removed.push(entry.path()),
            Err(error) => {
                return Err(ResetFailed {
                    removed: std::mem::take(removed),
                    failed: ResetLeftover {
                        path: entry.path(),
                        reason: error.to_string(),
                    },
                });
            }
        }
    }
    if kept > 0 {
        println!(
            "reset {} but kept {kept} entr{} a standalone agent is running in",
            root.display(),
            if kept == 1 { "y" } else { "ies" }
        );
    } else if remove_dir_if_empty(root).unwrap_or(false) {
        println!("removed {}", root.display());
    }
    Ok(())
}

/// Whether a managed worktree must be left standing because it IS, or CONTAINS,
/// a folder a standalone agent occupies.
///
/// The same rule [`sweep_worktrees_root`] applies to the root's own
/// entries, and compared the same way: canonically, so a symlinked spelling
/// cannot slip past. A worktree strictly INSIDE an occupied folder is not spared
/// here, deliberately: dux made that worktree and resets what it made, and the
/// user's folder itself is still standing around it afterwards.
fn worktree_holds_occupied_folder(worktree: &Path, occupied: &[PathBuf]) -> bool {
    occupied
        .iter()
        .any(|folder| dux_core::worktree_ops::folder_contains(worktree, folder))
}

/// Remove one agent's managed worktree during a factory reset. Returns whether
/// it was actually removed, so the caller's count cannot claim a skip.
///
/// It takes a [`ManagedWorkspace`], not a session, and that is the guard: this
/// function ends in an unconditional `remove_dir_all`, and the managed-root
/// path check below is not enough on its own, because a standalone agent
/// pointed at a directory under dux's managed root would pass it.
///
/// `occupied` closes the other door: the worktree itself may be, or contain, a
/// folder a standalone agent occupies, and the `remove_dir_all` would take the
/// user's folder with it, so the whole removal is skipped and the reason
/// printed.
/// The factory reset's stdout summary, counting the worktrees it removed.
fn removed_worktrees_line(removed: usize) -> String {
    format!("removed {}", count_of(removed, "session worktree"))
}

/// What the reset did with one agent's managed worktree.
enum SessionWorktreeReset {
    Removed,
    /// Deliberately left alone, and already said why.
    Skipped,
    /// It could not be removed; reported with the rest at the end.
    Left(ResetLeftover),
}

fn remove_session_worktree(
    paths: &DuxPaths,
    managed: &dux_core::model::ManagedWorkspace,
    occupied: &[PathBuf],
) -> SessionWorktreeReset {
    let worktree = Path::new(&managed.worktree_path);
    if !git::is_under(&paths.worktrees_root, worktree) {
        eprintln!(
            "warning: skipping worktree outside of managed root: {}",
            managed.worktree_path
        );
        return SessionWorktreeReset::Skipped;
    }
    if worktree_holds_occupied_folder(worktree, occupied) {
        eprintln!(
            "warning: keeping {}: a standalone agent is running in that directory or one \
             inside it, and dux never removes a folder it did not make",
            managed.worktree_path
        );
        return SessionWorktreeReset::Skipped;
    }

    // Route through the shared core removal so the worktree is removed with the
    // correct `-C <repo>`, the repo's worktree registration is pruned, and the
    // branch is deleted afterward. Continue-on-error: a factory reset must
    // press on past any single failure.
    if let Some(project_path) = managed.project_path.as_deref() {
        // The same branch-ownership gate the engine's delete applies: a reset
        // that left a drifted agent's own original branch behind would not be a
        // reset, and one that deleted the user's `develop` because an agent was
        // once attached to it would be data loss.
        let remove = || {
            if managed.branch_provenance.dux_may_delete_branch() {
                git::remove_worktree(
                    Path::new(project_path),
                    worktree,
                    &managed.branch_name,
                    Some(managed.initial_branch.as_str()),
                )
                .map(|_| ())
            } else {
                git::remove_worktree_keep_branch(Path::new(project_path), worktree)
            }
        };
        let first = remove();
        // Belt-and-suspenders for the factory-reset guarantee: ensure the
        // directory is gone even when git could not remove it. Core removal
        // never filesystem-deletes, so this stays the CLI's own last resort.
        if let Some(leftover) = remove_leftover_folder(worktree) {
            return leftover;
        }
        // git refused while it still had the worktree registered (a lock, a
        // transient failure), so its registration, and the branch it was
        // asked to delete, are still there. With the folder gone, the same
        // call forgets exactly this registration (never a repository-wide
        // prune) and deletes the branches provenance allows.
        if first
            .as_ref()
            .err()
            .is_some_and(|err| err.downcast_ref::<git::RemovalWorthRetrying>().is_some())
            && let Err(err) = remove()
        {
            eprintln!(
                "warning: removed {} but could not forget its registration in {project_path}: \
                 {err:#}",
                worktree.display()
            );
        }
        return SessionWorktreeReset::Removed;
    }

    // An orphan with no `project_path`: no repository to drive git.
    if let Some(leftover) = remove_leftover_folder(worktree) {
        return leftover;
    }
    SessionWorktreeReset::Removed
}

/// Delete what is left of a worktree folder, the reset's last resort; the
/// leftover when it cannot.
fn remove_leftover_folder(worktree: &Path) -> Option<SessionWorktreeReset> {
    if worktree.exists()
        && let Err(error) = fs::remove_dir_all(worktree)
    {
        return Some(SessionWorktreeReset::Left(ResetLeftover {
            path: worktree.to_path_buf(),
            reason: error.to_string(),
        }));
    }
    None
}

// ---------------------------------------------------------------------------
// File / directory helpers
// ---------------------------------------------------------------------------

fn resolve_reset_log_path(paths: &DuxPaths) -> PathBuf {
    let logging = if paths.config_path.exists() {
        fs::read_to_string(&paths.config_path)
            .ok()
            .and_then(|raw| toml::from_str::<config::Config>(&raw).ok())
            .map(|config| config.logging)
            .unwrap_or_default()
    } else {
        config::LoggingConfig::default()
    };
    logger::resolve_log_path(&logging, paths)
}

fn remove_file_with_message(path: &Path) -> Result<()> {
    if remove_file_if_present(path)? {
        println!("removed {}", path.display());
    }
    Ok(())
}

fn remove_root_if_empty_with_message(path: &Path) -> Result<()> {
    if remove_dir_if_empty(path)? {
        println!("removed {}", path.display());
    }
    Ok(())
}

fn remove_file_if_present(path: &Path) -> Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(anyhow!("failed to remove {}: {error}", path.display())),
    }
}

fn remove_dir_if_empty(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let mut entries = fs::read_dir(path)
        .map_err(|error| anyhow!("failed to inspect {}: {error}", path.display()))?;
    if entries.next().is_some() {
        return Ok(false);
    }
    fs::remove_dir(path)
        .map_err(|error| anyhow!("failed to remove {}: {error}", path.display()))?;
    Ok(true)
}

fn prune_empty_ancestors(path: &Path, root: &Path) -> Result<()> {
    let Ok(relative) = path.strip_prefix(root) else {
        return Ok(());
    };
    if relative.as_os_str().is_empty() {
        return Ok(());
    }

    let mut current = path.parent();
    while let Some(dir) = current {
        if dir == root {
            break;
        }
        if !remove_dir_if_empty(dir)? {
            break;
        }
        current = dir.parent();
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Convenience extension for anyhow context on paths
// ---------------------------------------------------------------------------

trait WithContextPath<T> {
    fn with_context_path(self, path: &Path) -> Result<T>;
}

impl<T, E: std::fmt::Display> WithContextPath<T> for std::result::Result<T, E> {
    fn with_context_path(self, path: &Path) -> Result<T> {
        self.map_err(|e| anyhow!("{}: {e}", path.display()))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    use chrono::Utc;
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn a_help_flag_asks_for_help_until_a_literal_double_dash() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(asks_for_help(&args(&["get", "--help"])));
        assert!(asks_for_help(&args(&["set", "k", "v", "-h"])));
        assert!(!asks_for_help(&args(&["set", "k", "--", "--help"])));
        assert!(!asks_for_help(&args(&["set", "k", "v"])));
    }
    use crate::config::{self, Config};
    use crate::keybindings::RuntimeBindings;
    use crate::model::{AgentSession, ProviderKind, SessionStatus};

    /// A factory reset still runs on a config.toml dux cannot load: the grace
    /// it gives each process falls back to the default instead of stopping the
    /// reset, and a loadable file's own setting is honoured.
    #[test]
    fn a_reset_grace_falls_back_to_the_default_when_the_config_will_not_load() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = DuxPaths {
            root: tmp.path().to_path_buf(),
            config_path: tmp.path().join("config.toml"),
            sessions_db_path: tmp.path().join("sessions.sqlite3"),
            worktrees_root: tmp.path().join("worktrees"),
            lock_path: tmp.path().join("dux.lock"),
        };
        let default_grace = dux_core::config::shutdown_grace(
            dux_core::config::Config::default().shutdown_timeout_seconds,
        );
        fs::write(&paths.config_path, "this is [not toml\n").expect("write");
        assert!(dux_core::config::load_config(&paths).is_err());
        assert_eq!(reset_grace(&paths), default_grace);

        fs::write(&paths.config_path, "shutdown_timeout_seconds = 3\n").expect("write");
        assert_eq!(
            reset_grace(&paths),
            dux_core::config::shutdown_grace(3),
            "a loadable file's own grace is used",
        );
    }

    /// Review 21: a startup command's job that dux left running (recorded in
    /// the saved process registry for agent m1, in m1's worktree) is still
    /// running when `dux config reset --all` runs. The reset ends it first and
    /// then removes everything, so nothing is left running in a folder that
    /// is gone.
    #[test]
    fn review21_factory_reset_removes_a_worktree_a_recorded_dux_job_still_runs_in() {
        use std::os::unix::process::CommandExt;
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = DuxPaths {
            root: tmp.path().to_path_buf(),
            config_path: tmp.path().join("config.toml"),
            sessions_db_path: tmp.path().join("sessions.sqlite3"),
            worktrees_root: tmp.path().join("worktrees"),
            lock_path: tmp.path().join("dux.lock"),
        };
        let worktree = paths.worktrees_root.join("proj").join("feat");
        fs::create_dir_all(&worktree).expect("worktree");
        let mut command = std::process::Command::new("sleep");
        command.arg("60").current_dir(&worktree);
        unsafe {
            command.pre_exec(|| {
                rustix::process::setsid().map_err(std::io::Error::from)?;
                Ok(())
            });
        }
        let mut job = command.spawn().expect("spawn the job");
        let session = dux_core::process_sessions::ProcessSession::started_now(job.id());
        let now = Utc::now();
        let store = SessionStore::open(&paths.sessions_db_path).expect("store");
        store
            .upsert_session(&AgentSession {
                id: "m1".to_string(),
                slot_tab_id: "m1-slot".to_string(),
                provider: ProviderKind::new("claude"),
                workspace: dux_core::model::AgentWorkspace::Managed(
                    dux_core::model::ManagedWorkspace {
                        project_id: "p1".to_string(),
                        project_path: None,
                        source_branch: "main".to_string(),
                        branch_name: "feat".to_string(),
                        initial_branch: "feat".to_string(),
                        branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
                        worktree_path: worktree.to_string_lossy().to_string(),
                    },
                ),
                title: None,
                started_providers: Vec::new(),
                desired_running: false,
                auto_reopen_enabled: false,
                status: SessionStatus::Detached,
                created_at: now,
                updated_at: now,
                last_focused_tab: None,
            })
            .expect("upsert managed");
        store
            .replace_process_registry(&[dux_core::process_sessions::StoredSession {
                owner: Some("m1".to_string()),
                session,
                folder: worktree.clone(),
                standalone: false,
                survivors: Vec::new(),
                label: Some("an agent's startup command".to_string()),
            }])
            .expect("save the registry");
        drop(store);

        run_reset(&paths, true).expect("reset");

        let still_running = job.try_wait().expect("try_wait").is_none();
        let _ = job.kill();
        let _ = job.wait();
        assert!(
            !still_running,
            "the recorded dux job (pid {}) was left running",
            job.id()
        );
        assert!(
            !worktree.exists(),
            "its worktree is removed once it stopped"
        );
        assert!(!paths.sessions_db_path.exists(), "the database goes too");
    }

    /// A project whose repository lives under the worktrees root is KEPT by
    /// the reset (a project's repository is the user's), but the project
    /// terminal's program recorded there is still stopped: the reset stops
    /// everything dux started.
    #[test]
    fn a_factory_reset_stops_a_project_terminals_program_in_a_kept_repository() {
        use std::os::unix::process::CommandExt;
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = DuxPaths {
            root: tmp.path().to_path_buf(),
            config_path: tmp.path().join("config.toml"),
            sessions_db_path: tmp.path().join("sessions.sqlite3"),
            worktrees_root: tmp.path().join("worktrees"),
            lock_path: tmp.path().join("dux.lock"),
        };
        let project_repo = paths.worktrees_root.join("vendor");
        fs::create_dir_all(&project_repo).expect("project dir");
        fs::write(project_repo.join("keep.txt"), "mine\n").expect("seed");
        let mut command = std::process::Command::new("sleep");
        command.arg("60").current_dir(&project_repo);
        unsafe {
            command.pre_exec(|| {
                rustix::process::setsid().map_err(std::io::Error::from)?;
                Ok(())
            });
        }
        let mut job = command.spawn().expect("spawn the job");
        let session = dux_core::process_sessions::ProcessSession::started_now(job.id());
        let store = SessionStore::open(&paths.sessions_db_path).expect("store");
        store
            .upsert_project(&dux_core::config::ProjectConfig {
                id: "p2".to_string(),
                path: project_repo.to_string_lossy().into_owned(),
                name: Some("vendor".to_string()),
                default_provider: None,
                leading_branch: None,
                auto_reopen_agents: None,
                startup_command: None,
                env: Default::default(),
            })
            .expect("upsert project");
        store
            .replace_process_registry(&[dux_core::process_sessions::StoredSession {
                owner: Some(dux_core::process_sessions::UNOWNED_PTYS.to_string()),
                session,
                folder: project_repo.clone(),
                standalone: false,
                survivors: Vec::new(),
                label: Some("a project terminal".to_string()),
            }])
            .expect("save the registry");
        drop(store);

        let _ = reset_agent_data(&paths);

        let alive = job.try_wait().expect("try_wait").is_none();
        let _ = job.kill();
        let _ = job.wait();
        assert!(
            project_repo.join("keep.txt").exists(),
            "the repository is kept"
        );
        assert!(
            !alive,
            "the project terminal's job (pid {}) working in the project's repository {} \
             was left running",
            job.id(),
            project_repo.display()
        );
    }

    /// A project terminal's session (started at the project's repository,
    /// outside the worktrees root) has two processes: one job working in agent
    /// m1's worktree, and another working in the repository itself. Both are
    /// stopped, wherever they work.
    #[test]
    fn a_factory_reset_stops_a_terminal_job_working_outside_the_worktrees() {
        use std::os::unix::process::CommandExt;
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = DuxPaths {
            root: tmp.path().to_path_buf(),
            config_path: tmp.path().join("config.toml"),
            sessions_db_path: tmp.path().join("sessions.sqlite3"),
            worktrees_root: tmp.path().join("worktrees"),
            lock_path: tmp.path().join("dux.lock"),
        };
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).expect("repo");
        let worktree = paths.worktrees_root.join("proj").join("feat");
        fs::create_dir_all(&worktree).expect("worktree");
        // The leader works in the repository; its child in the worktree.
        let mut command = std::process::Command::new("sh");
        command
            .arg("-c")
            .arg(format!(
                "(cd '{}' && exec sleep 61) & exec sleep 62",
                worktree.display()
            ))
            .current_dir(&repo);
        unsafe {
            command.pre_exec(|| {
                rustix::process::setsid().map_err(std::io::Error::from)?;
                Ok(())
            });
        }
        let mut job = command.spawn().expect("spawn the job");
        let session = dux_core::process_sessions::ProcessSession::started_now(job.id());
        std::thread::sleep(std::time::Duration::from_millis(300));
        let store = SessionStore::open(&paths.sessions_db_path).expect("store");
        store
            .replace_process_registry(&[dux_core::process_sessions::StoredSession {
                owner: Some(dux_core::process_sessions::UNOWNED_PTYS.to_string()),
                session,
                folder: repo.clone(),
                standalone: false,
                survivors: Vec::new(),
                label: Some("a project terminal".to_string()),
            }])
            .expect("save the registry");
        drop(store);

        let _ = reset_agent_data(&paths);

        let repo_job_alive = job.try_wait().expect("try_wait").is_none();
        // End the whole group we started, whatever the reset did.
        let _ = rustix::process::kill_process_group(
            rustix::process::Pid::from_raw(job.id() as i32).unwrap(),
            rustix::process::Signal::KILL,
        );
        let _ = job.kill();
        let _ = job.wait();
        assert!(
            !repo_job_alive,
            "a job a project terminal left working in the repository {} (pid {}) was left \
             running",
            repo.display(),
            job.id()
        );
    }

    /// Review 22: a project terminal's session (recorded under the unowned
    /// key, started at the project's repository, OUTSIDE the worktrees root)
    /// left a job running whose working directory is agent m1's worktree.
    /// The reset only looks at sessions started under the worktrees root, so
    /// it neither ends this job nor keeps the folder it works in.
    #[test]
    fn review22_factory_reset_removes_a_worktree_a_recorded_terminal_job_works_in() {
        use std::os::unix::process::CommandExt;
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = DuxPaths {
            root: tmp.path().to_path_buf(),
            config_path: tmp.path().join("config.toml"),
            sessions_db_path: tmp.path().join("sessions.sqlite3"),
            worktrees_root: tmp.path().join("worktrees"),
            lock_path: tmp.path().join("dux.lock"),
        };
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).expect("repo");
        let worktree = paths.worktrees_root.join("proj").join("feat");
        fs::create_dir_all(&worktree).expect("worktree");
        let mut command = std::process::Command::new("sleep");
        command.arg("60").current_dir(&worktree);
        unsafe {
            command.pre_exec(|| {
                rustix::process::setsid().map_err(std::io::Error::from)?;
                Ok(())
            });
        }
        let mut job = command.spawn().expect("spawn the job");
        let session = dux_core::process_sessions::ProcessSession::started_now(job.id());
        let now = Utc::now();
        let store = SessionStore::open(&paths.sessions_db_path).expect("store");
        store
            .upsert_session(&AgentSession {
                id: "m1".to_string(),
                slot_tab_id: "m1-slot".to_string(),
                provider: ProviderKind::new("claude"),
                workspace: dux_core::model::AgentWorkspace::Managed(
                    dux_core::model::ManagedWorkspace {
                        project_id: "p1".to_string(),
                        project_path: None,
                        source_branch: "main".to_string(),
                        branch_name: "feat".to_string(),
                        initial_branch: "feat".to_string(),
                        branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
                        worktree_path: worktree.to_string_lossy().to_string(),
                    },
                ),
                title: None,
                started_providers: Vec::new(),
                desired_running: false,
                auto_reopen_enabled: false,
                status: SessionStatus::Detached,
                created_at: now,
                updated_at: now,
                last_focused_tab: None,
            })
            .expect("upsert managed");
        store
            .replace_process_registry(&[dux_core::process_sessions::StoredSession {
                owner: Some(dux_core::process_sessions::UNOWNED_PTYS.to_string()),
                session,
                folder: repo.clone(),
                standalone: false,
                survivors: Vec::new(),
                label: Some("a project terminal".to_string()),
            }])
            .expect("save the registry");
        drop(store);

        let _ = reset_agent_data(&paths);

        let still_running = job.try_wait().expect("try_wait").is_none();
        let _ = job.kill();
        let _ = job.wait();
        assert!(
            worktree.exists() || !still_running,
            "the reset removed {} while the dux job recorded by a project terminal (pid {}) was still working in it",
            worktree.display(),
            job.id()
        );
    }

    /// A process a standalone agent's run left running is stopped like every
    /// other recorded one, and the reset then goes on; the agent's folder is
    /// the user's and stays.
    #[test]
    fn a_factory_reset_stops_a_recorded_standalone_process_and_keeps_its_folder() {
        use std::os::unix::process::CommandExt;
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = DuxPaths {
            root: tmp.path().join("dux"),
            config_path: tmp.path().join("dux").join("config.toml"),
            sessions_db_path: tmp.path().join("dux").join("sessions.sqlite3"),
            worktrees_root: tmp.path().join("dux").join("worktrees"),
            lock_path: tmp.path().join("dux").join("dux.lock"),
        };
        fs::create_dir_all(&paths.root).expect("root");
        let folder = tmp.path().join("my-notes");
        fs::create_dir_all(&folder).expect("folder");
        let mut command = std::process::Command::new("sleep");
        command.arg("60").current_dir(&folder);
        // SAFETY: `setsid` is async-signal-safe and touches no Rust state.
        unsafe {
            command.pre_exec(|| {
                rustix::process::setsid().map_err(std::io::Error::from)?;
                Ok(())
            });
        }
        struct Kill(std::process::Child);
        impl Drop for Kill {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut job = Kill(command.spawn().expect("spawn the job"));
        let session = dux_core::process_sessions::ProcessSession::started_now(job.0.id());
        let store = SessionStore::open(&paths.sessions_db_path).expect("store");
        store
            .replace_process_registry(&[dux_core::process_sessions::StoredSession {
                owner: Some("s1".to_string()),
                session,
                folder: folder.clone(),
                standalone: true,
                survivors: Vec::new(),
                label: None,
            }])
            .expect("save the registry");
        drop(store);

        run_reset(&paths, true).expect("reset");

        assert!(
            job.0.try_wait().expect("try_wait").is_some(),
            "the standalone agent's process was stopped"
        );
        assert!(folder.exists(), "its folder is kept");
        assert!(!paths.sessions_db_path.exists(), "the database goes");
    }

    /// If dux cannot read what it recorded starting, it cannot know nothing is
    /// running, so the reset deletes nothing and says why.
    #[test]
    fn a_factory_reset_deletes_nothing_when_the_recorded_programs_cannot_be_read() {
        // A body that is not text, and a text body that is not a registry.
        for body in ["x'ff'", "'this is not json'"] {
            let harness = ResetHarness::new();
            harness.write_config_with_log_path("logs/custom.log");
            let worktree = harness.create_session("agent-1");
            let store = SessionStore::open(&harness.paths.sessions_db_path).expect("store");
            store.replace_process_registry(&[]).expect("registry row");
            drop(store);
            let conn = rusqlite::Connection::open(&harness.paths.sessions_db_path).expect("raw");
            conn.execute(
                &format!("update process_registry set body = {body} where id = 1"),
                [],
            )
            .expect("make the body unreadable");
            drop(conn);

            let refusal = run_reset(&harness.paths, true).expect_err("the reset refuses");

            let message = format!("{refusal:#}");
            assert!(
                message.contains("could not check which programs are still running"),
                "{body}: {message}"
            );
            assert!(message.contains("deleted nothing"), "{body}: {message}");
            assert!(worktree.exists(), "{body}");
            assert!(harness.paths.config_path.exists(), "{body}");
            assert!(harness.paths.sessions_db_path.exists(), "{body}");
        }
    }

    /// A factory reset must not remove a STANDALONE agent's folder, even when
    /// the user pointed that agent at a directory inside dux's own managed
    /// area. The per-session loop already skips it; the sweep of the whole
    /// worktrees root afterwards did not, so the guard held for one line and
    /// then the directory went anyway.
    ///
    /// Nothing refuses a folder under the managed root at creation, so this is
    /// reachable, not theoretical.
    #[test]
    fn a_factory_reset_keeps_a_standalone_agents_folder_inside_the_managed_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = DuxPaths {
            root: tmp.path().to_path_buf(),
            config_path: tmp.path().join("config.toml"),
            sessions_db_path: tmp.path().join("sessions.sqlite3"),
            worktrees_root: tmp.path().join("worktrees"),
            lock_path: tmp.path().join("dux.lock"),
        };
        // A managed worktree dux made, and a standalone folder the user chose
        // that happens to live beside it under the same root.
        let managed = paths.worktrees_root.join("proj").join("feat");
        let occupied = paths.worktrees_root.join("my-notes");
        fs::create_dir_all(&managed).expect("managed dir");
        fs::create_dir_all(&occupied).expect("occupied dir");
        fs::write(occupied.join("notes.txt"), "mine\n").expect("seed a file");

        let now = Utc::now();
        let store = SessionStore::open(&paths.sessions_db_path).expect("store");
        store
            .upsert_session(&AgentSession {
                id: "sa1".to_string(),
                slot_tab_id: "sa1-slot".to_string(),
                provider: ProviderKind::new("claude"),
                workspace: dux_core::model::AgentWorkspace::Folder(
                    dux_core::model::FolderWorkspace {
                        folder_path: occupied.to_string_lossy().to_string(),
                    },
                ),
                title: Some("my-notes".to_string()),
                started_providers: Vec::new(),
                desired_running: false,
                auto_reopen_enabled: false,
                status: SessionStatus::Detached,
                created_at: now,
                updated_at: now,
                last_focused_tab: None,
            })
            .expect("upsert standalone");
        drop(store);

        reset_agent_data(&paths).expect("reset");

        assert!(
            occupied.exists(),
            "a standalone agent's folder is the user's and survives a factory reset"
        );
        assert_eq!(
            fs::read_to_string(occupied.join("notes.txt")).expect("the file survives"),
            "mine\n",
            "and so does everything in it"
        );
        assert!(
            !managed.exists(),
            "dux's own managed worktree is still reset: it made that one"
        );
    }

    /// The other door into the same data loss: the folder is not beside the
    /// managed worktrees, it is INSIDE one. The removal of that worktree ends in
    /// an unconditional `remove_dir_all`, so it took the user's folder with it,
    /// and which of the two rows the loader returned first decided whether it
    /// happened at all.
    ///
    /// Reachable: point a standalone agent at an empty directory under the
    /// managed root, then create a managed agent whose worktree lands on it.
    #[test]
    fn a_factory_reset_keeps_a_standalone_folder_inside_a_managed_worktree() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = DuxPaths {
            root: tmp.path().to_path_buf(),
            config_path: tmp.path().join("config.toml"),
            sessions_db_path: tmp.path().join("sessions.sqlite3"),
            worktrees_root: tmp.path().join("worktrees"),
            lock_path: tmp.path().join("dux.lock"),
        };
        let managed_worktree = paths.worktrees_root.join("proj").join("feat");
        let occupied = managed_worktree.join("notes");
        fs::create_dir_all(&occupied).expect("occupied dir");
        fs::write(occupied.join("notes.txt"), "mine\n").expect("seed a file");

        let now = Utc::now();
        let store = SessionStore::open(&paths.sessions_db_path).expect("store");
        // The MANAGED row first, so the loader's order is the hostile one even
        // without relying on how it sorts.
        store
            .upsert_session(&AgentSession {
                id: "m1".to_string(),
                slot_tab_id: "m1-slot".to_string(),
                provider: ProviderKind::new("claude"),
                workspace: dux_core::model::AgentWorkspace::Managed(
                    dux_core::model::ManagedWorkspace {
                        project_id: "p1".to_string(),
                        // No owning repo, so the removal is the CLI's own
                        // `remove_dir_all` and no git subprocess runs.
                        project_path: None,
                        source_branch: "main".to_string(),
                        branch_name: "feat".to_string(),
                        initial_branch: "feat".to_string(),
                        branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
                        worktree_path: managed_worktree.to_string_lossy().to_string(),
                    },
                ),
                title: None,
                started_providers: Vec::new(),
                desired_running: false,
                auto_reopen_enabled: false,
                status: SessionStatus::Detached,
                created_at: now,
                updated_at: now,
                last_focused_tab: None,
            })
            .expect("upsert managed");
        store
            .upsert_session(&AgentSession {
                id: "sa1".to_string(),
                slot_tab_id: "sa1-slot".to_string(),
                provider: ProviderKind::new("claude"),
                workspace: dux_core::model::AgentWorkspace::Folder(
                    dux_core::model::FolderWorkspace {
                        folder_path: occupied.to_string_lossy().to_string(),
                    },
                ),
                title: Some("notes".to_string()),
                started_providers: Vec::new(),
                desired_running: false,
                auto_reopen_enabled: false,
                status: SessionStatus::Detached,
                created_at: now,
                updated_at: now,
                last_focused_tab: None,
            })
            .expect("upsert standalone");
        drop(store);

        reset_agent_data(&paths).expect("reset");

        assert_eq!(
            fs::read_to_string(occupied.join("notes.txt")).expect("the folder survives"),
            "mine\n",
            "a standalone agent's folder survives even when a managed worktree encloses it"
        );
    }

    /// A dux project's repository inside a managed worktree survives a
    /// factory reset: the project is the user's, like a standalone folder.
    #[test]
    fn a_factory_reset_keeps_a_projects_repository_inside_a_managed_worktree() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = DuxPaths {
            root: tmp.path().to_path_buf(),
            config_path: tmp.path().join("config.toml"),
            sessions_db_path: tmp.path().join("sessions.sqlite3"),
            worktrees_root: tmp.path().join("worktrees"),
            lock_path: tmp.path().join("dux.lock"),
        };
        let managed_worktree = paths.worktrees_root.join("proj").join("feat");
        let project_repo = managed_worktree.join("vendor");
        fs::create_dir_all(&project_repo).expect("project dir");
        fs::write(project_repo.join("keep.txt"), "mine\n").expect("seed a file");
        let now = Utc::now();
        let store = SessionStore::open(&paths.sessions_db_path).expect("store");
        store
            .upsert_session(&AgentSession {
                id: "m1".to_string(),
                slot_tab_id: "m1-slot".to_string(),
                provider: ProviderKind::new("claude"),
                workspace: dux_core::model::AgentWorkspace::Managed(
                    dux_core::model::ManagedWorkspace {
                        project_id: "p1".to_string(),
                        project_path: None,
                        source_branch: "main".to_string(),
                        branch_name: "feat".to_string(),
                        initial_branch: "feat".to_string(),
                        branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
                        worktree_path: managed_worktree.to_string_lossy().to_string(),
                    },
                ),
                title: None,
                started_providers: Vec::new(),
                desired_running: false,
                auto_reopen_enabled: false,
                status: SessionStatus::Detached,
                created_at: now,
                updated_at: now,
                last_focused_tab: None,
            })
            .expect("upsert managed");
        store
            .upsert_project(&dux_core::config::ProjectConfig {
                id: "p2".to_string(),
                path: project_repo.to_string_lossy().into_owned(),
                name: Some("vendor".to_string()),
                default_provider: None,
                leading_branch: None,
                auto_reopen_agents: None,
                startup_command: None,
                env: Default::default(),
            })
            .expect("upsert project");
        drop(store);

        reset_agent_data(&paths).expect("reset");

        assert!(
            project_repo.join("keep.txt").exists(),
            "a project's repository survives a reset even when a managed worktree encloses it"
        );
    }

    /// The skip rule itself, on the four ways two paths can overlap. Only the
    /// two where removing the worktree would take the user's folder with it are
    /// spared.
    #[test]
    fn the_reset_skip_rule_spares_a_worktree_that_is_or_holds_an_occupied_folder() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let worktree = root.join("wt");
        fs::create_dir_all(worktree.join("inner")).expect("dirs");
        fs::create_dir_all(root.join("elsewhere")).expect("dirs");

        // Equal: the standalone agent is running in the worktree itself.
        assert!(worktree_holds_occupied_folder(
            &worktree,
            std::slice::from_ref(&worktree)
        ));
        // The worktree CONTAINS the folder: removing it recursively takes the
        // folder with it.
        assert!(worktree_holds_occupied_folder(
            &worktree,
            &[worktree.join("inner")]
        ));
        // The folder CONTAINS the worktree: dux made the worktree, so it goes,
        // and the user's folder is still there around it.
        assert!(!worktree_holds_occupied_folder(
            &worktree.join("inner"),
            std::slice::from_ref(&worktree)
        ));
        // Disjoint.
        assert!(!worktree_holds_occupied_folder(
            &worktree,
            &[root.join("elsewhere")]
        ));
        // Nothing occupied at all is the ordinary reset.
        assert!(!worktree_holds_occupied_folder(&worktree, &[]));
    }

    #[test]
    fn config_diff_reports_nothing_for_a_default_config() {
        assert!(
            collect_config_changes(&Config::default()).is_empty(),
            "{:#?}",
            collect_config_changes(&Config::default())
        );
    }

    #[test]
    fn config_diff_reports_the_first_load_screen_opt_outs() {
        // Both keys are hand-registered in `collect_config_changes`; a key that
        // is missing there is one `dux config diff` silently ignores.
        let mut config = Config::default();
        config.ui.disable_automated_welcome_screen = true;
        config.ui.disable_release_notes = true;

        // Match the EXACT rendered line, not a substring of the key: a
        // `contains("ui.disable_release_notes")` check also matches a typo'd
        // `ui.disable_release_notes_xyz`, which makes the test useless as a guard.
        let changes = collect_config_changes(&config);
        assert!(
            changes.contains(&"ui.disable_automated_welcome_screen: false -> true".to_string()),
            "{changes:#?}"
        );
        assert!(
            changes.contains(&"ui.disable_release_notes: false -> true".to_string()),
            "{changes:#?}"
        );
        assert_eq!(changes.len(), 2, "nothing else should have changed");
    }

    // -----------------------------------------------------------------------
    // dux config diff (summary)
    // -----------------------------------------------------------------------

    /// A string no default value contains, so its presence in the output can
    /// only have come from the fixture that planted it.
    const SENTINEL: &str = "sentinel-do-not-print-me-9f3a";

    /// Every path the differ must be able to report, discovered by walking the
    /// serialized default config rather than by anyone listing them.
    ///
    /// Returns `(dotted path, config with exactly that leaf mutated)`.
    fn mutated_leaf_fixtures() -> Vec<(String, Config)> {
        let default = serde_json::to_value(Config::default()).expect("serialize default config");
        assert_eq!(
            serde_json::from_value::<Config>(default.clone()).expect("round-trip default config"),
            Config::default(),
            "the differ compares a JSON projection, so the projection must be lossless"
        );

        let mut fixtures = Vec::new();
        let mut path = Vec::new();
        walk_default_leaves(&default, &default, &mut path, &mut fixtures);
        assert!(
            fixtures.len() > 40,
            "the walk found only {} leaves; the config is much bigger than that",
            fixtures.len()
        );
        fixtures
    }

    /// Top-level subtrees the differ deliberately summarizes instead of
    /// descending into. They are covered by their own fixtures below, because
    /// their reported line is not a leaf path.
    const SUMMARIZED_SUBTREES: &[&str] = &["env", "projects", "macros"];

    fn walk_default_leaves(
        root: &serde_json::Value,
        node: &serde_json::Value,
        path: &mut Vec<String>,
        out: &mut Vec<(String, Config)>,
    ) {
        if path.len() == 1 && SUMMARIZED_SUBTREES.contains(&path[0].as_str()) {
            return;
        }
        match node {
            serde_json::Value::Object(map) => {
                assert!(
                    !map.is_empty(),
                    "no mutation policy for the empty object at {}: decide whether it \
                     recurses or is summarized, then teach this walk about it",
                    path.join(".")
                );
                for (name, child) in map {
                    path.push(name.clone());
                    walk_default_leaves(root, child, path, out);
                    path.pop();
                }
            }
            other => {
                let dotted = path.join(".");
                let mutated = mutate_leaf(root, path, other).unwrap_or_else(|| {
                    panic!(
                        "no candidate mutation for the leaf at {dotted} ({other}); \
                         add one so this exhaustiveness check keeps working"
                    )
                });
                out.push((dotted, mutated));
            }
        }
    }

    /// Replace the leaf at `path` with a different value of the same shape and
    /// deserialize the result. `None` when nothing produced a valid `Config`.
    fn mutate_leaf(
        root: &serde_json::Value,
        path: &[String],
        leaf: &serde_json::Value,
    ) -> Option<Config> {
        let candidates: Vec<serde_json::Value> = match leaf {
            serde_json::Value::Bool(b) => vec![serde_json::Value::Bool(!b)],
            serde_json::Value::Number(n) => {
                let raised = n.as_u64().map(|v| serde_json::json!(v + 1));
                let lowered = n
                    .as_u64()
                    .and_then(|v| v.checked_sub(1))
                    .map(|v| serde_json::json!(v));
                raised.into_iter().chain(lowered).collect()
            }
            // Settings that validate their string (an enum, a password hash)
            // refuse the appended form, so a few values that pass each kind of
            // check follow it.
            serde_json::Value::String(s) => vec![
                serde_json::json!(format!("{s}-mutated")),
                serde_json::json!("tailnet"),
                serde_json::json!("always"),
                serde_json::json!(a_valid_password_hash()),
            ],
            serde_json::Value::Array(items) => ["dux-diff-probe", "192.0.2.1"]
                .into_iter()
                .map(|probe| {
                    let mut grown = items.clone();
                    grown.push(serde_json::json!(probe));
                    serde_json::Value::Array(grown)
                })
                .collect(),
            // A null carries no type, so try each shape an `Option` field can take.
            serde_json::Value::Null => vec![
                serde_json::json!("dux-diff-probe"),
                serde_json::json!(4321),
                serde_json::json!(true),
                serde_json::json!(["dux-diff-probe"]),
            ],
            serde_json::Value::Object(_) => Vec::new(),
        };

        for candidate in candidates {
            let mut document = root.clone();
            let mut cursor = &mut document;
            for segment in path {
                cursor = cursor
                    .get_mut(segment)
                    .expect("path exists in the default document");
            }
            *cursor = candidate;
            if let Ok(config) = serde_json::from_value::<Config>(document) {
                return Some(config);
            }
        }
        None
    }

    #[test]
    fn config_diff_reports_the_tailscale_mode() {
        // The exhaustiveness walk below covers this structurally, but the walk
        // mutates a string by appending to it, and this particular setting has a
        // lenient deserializer that also accepts a boolean. Naming it here proves
        // the leaf is genuinely reachable and reported as a value change rather
        // than being quietly rejected on the way in.
        let mut config = Config::default();
        config.server.tailscale = "no".to_string();
        assert_eq!(
            collect_config_changes(&config),
            vec!["server.tailscale: auto -> no".to_string()]
        );
    }

    fn a_valid_password_hash() -> String {
        static HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        HASH.get_or_init(|| {
            dux_core::auth::hash_password(&dux_core::auth::Password::new(
                "correct horse battery staple".to_string(),
            ))
            .expect("hash")
        })
        .clone()
    }

    /// A pasted bug report must not carry the password hash: anyone holding
    /// it can guess offline. The summary says only that it changed.
    #[test]
    fn config_diff_never_prints_the_password_hash() {
        let mut config = Config::default();
        config.server.auth.password_hash = a_valid_password_hash();
        let changes = collect_config_changes(&config);
        assert_eq!(
            changes,
            vec!["server.auth.password_hash: changed".to_string()]
        );
        assert!(!changes.concat().contains("argon2id"), "{changes:?}");
    }

    /// Blocked addresses are other people's IP addresses; the summary counts
    /// them instead of listing them.
    #[test]
    fn config_diff_counts_blocked_addresses_without_printing_them() {
        let mut config = Config::default();
        config.server.auth.blocked_addresses =
            vec!["203.0.113.7".to_string(), "198.51.100.0/24".to_string()];
        let changes = collect_config_changes(&config);
        assert_eq!(
            changes,
            vec!["server.auth.blocked_addresses: 2 addresses configured".to_string()]
        );
    }

    #[test]
    fn regenerate_warns_that_it_removes_a_password() {
        let with = format!(
            "[server.auth]\npassword_hash = \"{}\"\n",
            a_valid_password_hash()
        );
        assert!(regenerate_password_note(&with).is_some_and(|n| n.contains("password")));
        assert_eq!(regenerate_password_note("[server]\nport = 1\n"), None);
        // Still warned about when the section around it is invalid, or the
        // file is not even TOML: the password is in there either way.
        for broken in [
            "[server.auth]\npassword_hash = \"$argon2id$v=19$whatever\"\nrequire = \"lan\"\n",
            "[server.auth\npassword_hash = \"x\"\n",
        ] {
            assert!(regenerate_password_note(broken).is_some(), "{broken}");
        }
        assert_eq!(
            regenerate_password_note("[server.auth]\npassword_hash = \"\"\n"),
            None,
            "an empty hash is no password"
        );
    }

    #[test]
    fn config_diff_reports_every_leaf_of_the_default_config() {
        let mut unreported = Vec::new();
        for (dotted, config) in mutated_leaf_fixtures() {
            let changes = collect_config_changes(&config);
            let prefix = format!("{dotted}: ");
            let matching: Vec<&String> =
                changes.iter().filter(|l| l.starts_with(&prefix)).collect();
            if matching.len() != 1 {
                unreported.push(format!("{dotted} -> {changes:?}"));
            }
        }
        assert!(
            unreported.is_empty(),
            "these settings are not reported by `dux config diff`:\n{}",
            unreported.join("\n")
        );
    }

    #[test]
    fn config_diff_marks_a_provider_present_only_in_the_current_config() {
        let mut config = Config::default();
        config.providers.commands.insert(
            "mine".to_string(),
            config::ProviderCommandConfig {
                command: "mine".to_string(),
                ..Default::default()
            },
        );

        assert_eq!(
            collect_config_changes(&config),
            vec!["providers.mine: (added)".to_string()]
        );
    }

    #[test]
    fn config_diff_marks_a_provider_present_only_in_the_default_config() {
        let mut config = Config::default();
        let removed = config
            .providers
            .commands
            .keys()
            .next()
            .expect("the default config ships providers")
            .clone();
        config.providers.commands.shift_remove(&removed);

        assert_eq!(
            collect_config_changes(&config),
            vec![format!("providers.{removed}: (removed)")]
        );
    }

    #[test]
    fn config_diff_recurses_into_a_provider_present_on_both_sides() {
        let mut config = Config::default();
        let entry = config
            .providers
            .commands
            .get_mut("claude")
            .expect("the default config ships a claude provider");
        entry.command = "claude-next".to_string();
        entry.args = vec!["--dangerously".to_string()];

        assert_eq!(
            collect_config_changes(&config),
            vec![
                "providers.claude.args: [] -> [--dangerously]".to_string(),
                "providers.claude.command: claude -> claude-next".to_string(),
            ]
        );
    }

    #[test]
    fn config_diff_never_names_a_provider_whose_name_breaks_the_rule() {
        let mut config = Config::default();
        config.providers.commands.insert(
            "my agent.v2".to_string(),
            config::ProviderCommandConfig::default(),
        );

        assert_eq!(
            collect_config_changes(&config),
            vec!["an entry of [providers] whose name is not shown: (added)".to_string()]
        );
    }

    #[test]
    fn config_diff_reports_an_array_as_one_value_not_as_indexed_paths() {
        let mut config = Config::default();
        config.server.allowed_hosts = vec!["dux.local".to_string(), "dux.lan".to_string()];
        config.terminal.args = vec!["-l".to_string(), "-i".to_string()];

        let changes = collect_config_changes(&config);
        assert!(
            changes.contains(&"server.allowed_hosts: [] -> [dux.local, dux.lan]".to_string()),
            "{changes:#?}"
        );
        assert!(
            changes.contains(&"terminal.args: [-l] -> [-l, -i]".to_string()),
            "{changes:#?}"
        );
        assert!(
            !changes
                .iter()
                .any(|l| l.contains(".0:") || l.contains(".1:")),
            "an array must never be reported as indexed paths: {changes:#?}"
        );
    }

    #[test]
    fn config_diff_never_prints_a_global_env_value() {
        let mut config = Config::default();
        config
            .env
            .insert("ANTHROPIC_API_KEY".to_string(), SENTINEL.to_string());

        let changes = collect_config_changes(&config);
        assert_eq!(changes, vec!["env: changed".to_string()]);
        assert!(
            !changes.join("\n").contains(SENTINEL),
            "an env value must never reach the summary"
        );
    }

    #[test]
    fn config_diff_never_prints_a_project_env_value() {
        let mut config = Config::default();
        let mut env = BTreeMap::new();
        env.insert("PROJECT_TOKEN".to_string(), SENTINEL.to_string());
        config.projects.push(config::ProjectConfig {
            id: "p1".to_string(),
            path: "/tmp/project".to_string(),
            name: None,
            default_provider: None,
            leading_branch: None,
            auto_reopen_agents: None,
            startup_command: None,
            env,
        });

        let changes = collect_config_changes(&config);
        assert_eq!(changes, vec!["projects: 1 project configured".to_string()]);
        assert!(
            !changes.join("\n").contains(SENTINEL),
            "a project env value must never reach the summary"
        );
    }

    #[test]
    fn config_diff_reports_macros_by_count_and_never_their_bodies() {
        let mut config = Config::default();
        config.macros.entries.insert(
            "review".to_string(),
            config::MacroEntry {
                text: SENTINEL.to_string(),
                surface: config::MacroSurface::Both,
            },
        );

        let changes = collect_config_changes(&config);
        assert_eq!(changes, vec!["macros: 1 macro configured".to_string()]);
        assert!(!changes.join("\n").contains(SENTINEL));
    }

    #[test]
    fn config_diff_reports_a_rebound_and_an_unbound_key_action() {
        let mut config = Config::default();
        config
            .keys
            .bindings
            .insert("quit".to_string(), vec!["ctrl-q".to_string()]);

        assert_eq!(
            collect_config_changes(&config),
            vec!["keys.quit: (new) -> [ctrl-q]".to_string()]
        );
    }

    /// `dux config diff` parses the file as written and never runs the load
    /// migrations, so a not-yet-folded `exit_interactive` row reaches the
    /// structural walk as an ordinary unknown key. It must be reported like any
    /// other binding rather than tripping the differ.
    #[test]
    fn config_diff_reports_an_unfolded_legacy_key_as_an_ordinary_binding() {
        let mut config = Config::default();
        config
            .keys
            .bindings
            .insert("exit_interactive".to_string(), vec!["ctrl-g".to_string()]);

        assert_eq!(
            collect_config_changes(&config),
            vec!["keys.exit_interactive: (new) -> [ctrl-g]".to_string()]
        );
    }

    #[test]
    fn config_diff_truncates_a_long_value_at_forty_characters() {
        let mut config = Config::default();
        config.editor.default = "e".repeat(45);

        let changes = collect_config_changes(&config);
        assert_eq!(
            changes,
            vec![format!(
                "editor.default: {} -> {}...",
                Config::default().editor.default,
                "e".repeat(40)
            )]
        );
    }

    #[test]
    fn config_diff_output_is_sorted_and_repeatable() {
        let mut config = Config::default();
        config.server.port = 9999;
        config.editor.default = "hx".to_string();
        config.ui.theme = "gruvbox".to_string();
        config.defaults.provider = "codex".to_string();

        let changes = collect_config_changes(&config);
        let mut sorted = changes.clone();
        sorted.sort();
        assert_eq!(changes, sorted, "output must be sorted by path");
        assert_eq!(
            changes,
            collect_config_changes(&config),
            "output must not depend on map iteration order"
        );
    }

    /// CHARACTERIZATION of a known inconsistency, not an endorsement of it.
    ///
    /// The two ways a `[ui]` width default can be reached must agree.
    ///
    /// `[ui]` carries `#[serde(default)]`, so a config whose `[ui]` table omits a
    /// width fills it from `UiConfig::default()`, while a fresh install gets the
    /// value the canonical template renders from `Config::default()`. These
    /// disagreed once (17/19 against 20/23), which gave the same setting two
    /// defaults depending on how the user arrived and made this command report a
    /// width the user had never written as changed. Both halves are asserted, and
    /// then the behaviour that actually matters: a sparse `[ui]` table reports no
    /// width at all.
    #[test]
    fn a_sparse_ui_table_reports_no_width_because_both_defaults_agree() {
        assert_eq!(
            (
                config::UiConfig::default().left_width_pct,
                config::UiConfig::default().right_width_pct
            ),
            (
                Config::default().ui.left_width_pct,
                Config::default().ui.right_width_pct
            ),
            "UiConfig::default() and the literal in Config::default() must not drift apart"
        );

        let sparse: Config =
            toml::from_str("[ui]\ntheme = \"dux\"\n").expect("parse sparse config");
        let changes = collect_config_changes(&sparse);
        let widths: Vec<&String> = changes
            .iter()
            .filter(|l| l.starts_with("ui.left_width_pct") || l.starts_with("ui.right_width_pct"))
            .collect();
        assert!(
            widths.is_empty(),
            "a width the user never wrote must not be reported as changed: {widths:#?}"
        );
    }

    #[test]
    fn reset_rejects_unknown_flags() {
        let error = reject_unknown_flags(&["--wat".to_string()], &["--all"]).unwrap_err();
        assert!(error.to_string().contains("unknown flag"));
    }

    #[test]
    fn default_reset_removes_config_and_logs_but_keeps_agent_data() {
        let harness = ResetHarness::new();
        harness.write_config_with_log_path("logs/custom.log");
        harness.write_log("logs/custom.log");
        let worktree = harness.create_session("agent-1");

        run_reset(&harness.paths, false).expect("reset");

        assert!(!harness.paths.config_path.exists());
        assert!(!harness.paths.root.join("logs/custom.log").exists());
        assert!(harness.paths.sessions_db_path.exists());
        assert!(worktree.exists());

        let _config = config::ensure_config(&harness.paths).expect("config recreated");
        let store = SessionStore::open(&harness.paths.sessions_db_path).expect("store");
        let sessions = store.load_sessions().expect("sessions");
        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0]
                .managed_worktree()
                .expect("managed test session"),
            worktree.to_string_lossy()
        );
    }

    #[test]
    fn reset_all_wipes_database_and_worktrees() {
        for seen in [None, Some("v0.6.0")] {
            let harness = ResetHarness::new();
            harness.write_config_with_log_path("logs/custom.log");
            harness.write_log("logs/custom.log");
            let worktree = harness.create_session("agent-1");
            if let Some(version) = seen {
                SessionStore::open(&harness.paths.sessions_db_path)
                    .expect("store")
                    .set_last_seen_version(version)
                    .expect("record the welcome screen as seen");
            }

            run_reset(&harness.paths, true).expect("reset");

            assert!(!worktree.exists());
            assert!(!harness.paths.config_path.exists());
            match seen {
                // Nothing to keep: the folder goes with everything else.
                None => assert!(!harness.paths.root.exists()),
                // The welcome screen stays dismissed: a fresh database holds
                // that one record and no agents.
                Some(version) => {
                    let store = SessionStore::open(&harness.paths.sessions_db_path).expect("store");
                    assert_eq!(store.last_seen_version().unwrap().as_deref(), Some(version));
                    assert!(store.load_sessions().unwrap().is_empty());
                }
            }
        }
    }

    #[test]
    fn reset_succeeds_when_paths_are_already_missing() {
        let harness = ResetHarness::new();
        fs::create_dir_all(&harness.paths.root).expect("root");

        run_reset(&harness.paths, false).expect("reset");

        assert!(!harness.paths.root.exists());
    }

    /// A worktree folder something is still writing into cannot be removed
    /// whole (git and `remove_dir_all` both answer "Directory not empty"). The
    /// reset stops there: the database and config stay, and it says what it
    /// removed and what it could not.
    #[test]
    fn reset_all_stops_at_a_worktree_it_cannot_remove_and_keeps_the_database_and_config() {
        use std::os::unix::fs::PermissionsExt;
        let harness = ResetHarness::new();
        harness.write_config_with_log_path("logs/custom.log");
        let removed = harness.create_session("agent-1");
        // An entry the sweep cannot delete: a folder whose own directory is
        // read-only still holds a file, the way a folder a process keeps
        // writing into refuses to empty.
        let stuck = harness.paths.worktrees_root.join("stuck");
        let locked = stuck.join("locked");
        fs::create_dir_all(&locked).expect("stuck folder");
        fs::write(locked.join("still-writing.log"), "x").expect("file");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).expect("chmod");

        let result = run_reset(&harness.paths, true);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).expect("chmod back");

        let refusal = result.expect_err("one stuck folder stops the reset");
        let message = format!("{refusal:#}");
        assert!(
            harness.paths.sessions_db_path.exists(),
            "the database stays"
        );
        assert!(harness.paths.config_path.exists(), "the config stays");
        assert!(message.contains(&stuck.display().to_string()), "{message}");
        assert!(message.contains("could not be removed"), "{message}");
        assert!(
            message.contains(&removed.display().to_string()),
            "{message}"
        );
        assert!(
            !removed.exists(),
            "what was removed before it stopped is gone"
        );
    }

    #[test]
    fn reset_all_removes_worktrees_without_database() {
        let harness = ResetHarness::new();
        let orphan = harness.paths.worktrees_root.join("orphan");
        fs::create_dir_all(&orphan).expect("orphan worktree");
        // Something nobody recorded works in the folder: the reset refuses and
        // names it, even with no database to say what dux started.
        let mut stranger = std::process::Command::new("sleep")
            .arg("60")
            .current_dir(&orphan)
            .spawn()
            .expect("spawn");

        let refusal = run_reset(&harness.paths, true).expect_err("the reset refuses");

        let message = format!("{refusal:#}");
        assert!(message.contains(&stranger.id().to_string()), "{message}");
        assert!(message.contains("sleep"), "{message}");
        assert!(message.contains(&orphan.display().to_string()), "{message}");
        assert!(orphan.exists());

        stranger.kill().expect("kill");
        stranger.wait().expect("reap");
        run_reset(&harness.paths, true).expect("reset");

        assert!(!harness.paths.root.exists());
    }

    #[test]
    fn diff_summary_reports_no_differences_for_defaults() {
        // Just verify it runs without error on defaults.
        let defaults = Config::default();
        run_diff_summary(&defaults).expect("diff summary");
    }

    #[test]
    fn config_path_subcommand() {
        // Just verify it doesn't error.
        let paths = DuxPaths {
            root: PathBuf::from("/tmp/test"),
            config_path: PathBuf::from("/tmp/test/config.toml"),
            sessions_db_path: PathBuf::from("/tmp/test/sessions.sqlite3"),
            worktrees_root: PathBuf::from("/tmp/test/worktrees"),
            lock_path: PathBuf::from("/tmp/test/dux.lock"),
        };
        let result = run(&["path".to_string()], &paths);
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // dux config restore-docs
    // -----------------------------------------------------------------------

    fn bare_user_config_fixture() -> String {
        std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bare_user_config.toml"
        ))
        .expect("read bare user config fixture")
    }

    /// Every backup this command wrote, oldest name first.
    fn backups(harness: &ResetHarness) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = fs::read_dir(&harness.paths.root)
            .expect("read dir")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.contains(".backup-"))
            })
            .collect();
        found.sort();
        found
    }

    #[test]
    fn restore_docs_preview_writes_nothing_and_leaves_the_file_alone() {
        let harness = ResetHarness::new();
        let original = bare_user_config_fixture();
        fs::write(&harness.paths.config_path, &original).expect("seed");

        run(&["restore-docs".to_string()], &harness.paths).expect("preview");

        assert_eq!(
            fs::read_to_string(&harness.paths.config_path).expect("read"),
            original,
            "preview must not modify the config"
        );
        assert!(
            backups(&harness).is_empty(),
            "preview must not write a backup"
        );
    }

    #[test]
    fn restore_docs_yes_writes_a_backup_containing_the_original_bytes() {
        use std::os::unix::fs::PermissionsExt;
        let harness = ResetHarness::new();
        let original = bare_user_config_fixture();
        fs::write(&harness.paths.config_path, &original).expect("seed");
        fs::set_permissions(&harness.paths.root, fs::Permissions::from_mode(0o755))
            .expect("loosen root");

        run(
            &["restore-docs".to_string(), "--yes".to_string()],
            &harness.paths,
        )
        .expect("apply");

        let root_mode = fs::metadata(&harness.paths.root)
            .expect("stat root")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(root_mode, 0o700, "the config folder must be tightened");

        // The config was rewritten with comments...
        let after = fs::read_to_string(&harness.paths.config_path).expect("read config");
        assert!(after.contains('#'), "config gained no comments");
        assert_ne!(after, original);

        // ...and exactly one backup holds the original bytes verbatim.
        let backups = backups(&harness);
        assert_eq!(backups.len(), 1, "expected one backup, got {backups:?}");
        assert_eq!(
            fs::read_to_string(&backups[0]).expect("read backup"),
            original,
            "the backup must be a byte-for-byte copy of the original"
        );
    }

    #[test]
    #[cfg(unix)]
    fn restore_docs_backup_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let harness = ResetHarness::new();
        fs::write(&harness.paths.config_path, bare_user_config_fixture()).expect("seed");

        run(
            &["restore-docs".to_string(), "--yes".to_string()],
            &harness.paths,
        )
        .expect("apply");

        // The backup carries the same potential secrets ([env] tokens) as the
        // config, so it must not be group/world readable either.
        let backup = backups(&harness).remove(0);
        let mode = fs::metadata(&backup).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "backup must be 0600, got {mode:o}");
    }

    #[test]
    fn restore_docs_never_clobbers_an_earlier_backup() {
        let harness = ResetHarness::new();
        fs::write(&harness.paths.config_path, bare_user_config_fixture()).expect("seed");
        run(
            &["restore-docs".to_string(), "--yes".to_string()],
            &harness.paths,
        )
        .expect("first apply");

        // Make the config restorable again, then run again inside the same
        // second so both runs compute the same timestamp.
        let mut second = fs::read_to_string(&harness.paths.config_path).expect("read");
        second.push_str("\n[a_fork_section]\nknob = 1\n");
        fs::write(&harness.paths.config_path, &second).expect("reseed");
        // Re-adding an orphan guarantees the second run is not a no-op.
        fs::write(
            &harness.paths.config_path,
            format!("{second}\n[auth]\nusers = []\n"),
        )
        .expect("reseed with orphan");

        run(
            &["restore-docs".to_string(), "--yes".to_string()],
            &harness.paths,
        )
        .expect("second apply");

        assert_eq!(
            backups(&harness).len(),
            2,
            "the second run must not overwrite the first backup"
        );
    }

    #[test]
    fn restore_docs_refuses_an_unparseable_config_and_leaves_it_byte_identical() {
        let harness = ResetHarness::new();
        let broken = "[server]\nport = = 8080\n[[[ nope\n";
        fs::write(&harness.paths.config_path, broken).expect("seed");

        let err = run(
            &["restore-docs".to_string(), "--yes".to_string()],
            &harness.paths,
        )
        .expect_err("must refuse a broken config");
        let message = format!("{err:#}");

        // It says why, and points at the path that would lose data instead.
        assert!(message.contains("not valid TOML"), "{message}");
        assert!(message.contains("has NOT been modified"), "{message}");
        assert!(message.contains("regenerate --yes"), "{message}");

        // The file is untouched, and no backup was written for a run that did
        // nothing.
        assert_eq!(
            fs::read_to_string(&harness.paths.config_path).expect("read"),
            broken,
            "a refused restore must leave the file byte-identical"
        );
        assert!(backups(&harness).is_empty());
    }

    #[test]
    fn restore_docs_is_a_noop_on_an_already_documented_config() {
        let harness = ResetHarness::new();
        harness.write_config_with_log_path("dux.log");
        let original = fs::read_to_string(&harness.paths.config_path).expect("read");

        run(
            &["restore-docs".to_string(), "--yes".to_string()],
            &harness.paths,
        )
        .expect("apply");

        assert_eq!(
            fs::read_to_string(&harness.paths.config_path).expect("read"),
            original,
            "a canonical config must not be rewritten"
        );
        assert!(
            backups(&harness).is_empty(),
            "a no-op must not write a backup"
        );
    }

    #[test]
    fn restore_docs_handles_a_missing_config_without_creating_one() {
        let harness = ResetHarness::new();
        assert!(!harness.paths.config_path.exists());

        run(&["restore-docs".to_string()], &harness.paths).expect("missing config is not an error");

        assert!(
            !harness.paths.config_path.exists(),
            "restore-docs must not create a config"
        );
    }

    #[test]
    fn restore_docs_rejects_unknown_flags() {
        let harness = ResetHarness::new();
        let err = run(
            &["restore-docs".to_string(), "--force".to_string()],
            &harness.paths,
        )
        .expect_err("unknown flag must be rejected");
        assert!(format!("{err:#}").contains("unknown flag"));
    }

    struct ResetHarness {
        _tempdir: TempDir,
        paths: DuxPaths,
    }

    impl ResetHarness {
        fn new() -> Self {
            let tempdir = TempDir::new().expect("tempdir");
            let root = tempdir.path().join("dux");
            fs::create_dir_all(&root).expect("root");
            let paths = DuxPaths {
                config_path: root.join("config.toml"),
                sessions_db_path: root.join("sessions.sqlite3"),
                worktrees_root: root.join("worktrees"),
                lock_path: root.join("dux.lock"),
                root,
            };
            Self {
                _tempdir: tempdir,
                paths,
            }
        }

        fn write_config_with_log_path(&self, log_path: &str) {
            let mut config = Config::default();
            config.logging.path = log_path.to_string();
            let bindings = RuntimeBindings::from_keys_config(&config.keys);
            let body = config::render_config_with(&config, &bindings);
            fs::write(&self.paths.config_path, body).expect("config");
        }

        fn write_log(&self, relative_path: &str) {
            let path = self.paths.root.join(relative_path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("log dir");
            }
            fs::write(path, "log").expect("log");
        }

        fn create_session(&self, id: &str) -> PathBuf {
            fs::create_dir_all(&self.paths.worktrees_root).expect("worktrees root");
            let worktree = self.paths.worktrees_root.join(id);
            fs::create_dir_all(&worktree).expect("worktree");

            let store = SessionStore::open(&self.paths.sessions_db_path).expect("store");
            let now = Utc::now();
            store
                .upsert_session(&AgentSession {
                    id: id.to_string(),
                    slot_tab_id: format!("{id}-slot"),
                    provider: ProviderKind::new("claude"),
                    title: None,
                    started_providers: Vec::new(),
                    desired_running: false,
                    auto_reopen_enabled: true,
                    status: SessionStatus::Active,
                    created_at: now,
                    updated_at: now,
                    last_focused_tab: None,
                    workspace: dux_core::model::AgentWorkspace::Managed(
                        dux_core::model::ManagedWorkspace {
                            project_id: "proj".to_string(),
                            project_path: None,
                            source_branch: "main".to_string(),
                            branch_name: format!("branch-{id}"),
                            initial_branch: format!("branch-{id}"),
                            branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
                            worktree_path: worktree.to_string_lossy().to_string(),
                        },
                    ),
                })
                .expect("session");
            worktree
        }
    }

    /// A factory reset resets what dux made. An agent attached to a branch the
    /// user already had gives up its worktree and keeps its branch: deleting
    /// `develop` because an agent once pointed at it is data loss, not a reset.
    #[test]
    fn factory_reset_keeps_a_branch_the_agent_did_not_create() {
        let tempdir = TempDir::new().expect("tempdir");
        let repo = tempdir.path().join("repo");
        fs::create_dir_all(&repo).expect("repo dir");
        let git = |cwd: &Path, args: &[&str]| {
            let out = dux_core::test_git::fixture_git()
                .args(args)
                .current_dir(cwd)
                .output()
                .expect("git");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "user.email", "t@example.com"]);
        git(&repo, &["config", "user.name", "Test User"]);
        git(&repo, &["commit", "--allow-empty", "-m", "initial"]);
        git(&repo, &["branch", "develop"]);

        let worktrees_root = tempdir.path().join("worktrees");
        fs::create_dir_all(&worktrees_root).expect("worktrees root");
        let worktree = worktrees_root.join("wt");
        git(
            &repo,
            &["worktree", "add", worktree.to_str().unwrap(), "develop"],
        );

        let paths = DuxPaths {
            config_path: tempdir.path().join("config.toml"),
            sessions_db_path: tempdir.path().join("sessions.sqlite3"),
            worktrees_root: worktrees_root.clone(),
            lock_path: tempdir.path().join("dux.lock"),
            root: tempdir.path().to_path_buf(),
        };
        let now = Utc::now();
        let session = AgentSession {
            id: "wt".to_string(),
            slot_tab_id: "wt-slot".to_string(),
            provider: ProviderKind::new("claude"),
            title: None,
            started_providers: Vec::new(),
            desired_running: false,
            auto_reopen_enabled: true,
            status: SessionStatus::Active,
            created_at: now,
            updated_at: now,
            last_focused_tab: None,
            workspace: dux_core::model::AgentWorkspace::Managed(
                dux_core::model::ManagedWorkspace {
                    project_id: "proj".to_string(),
                    project_path: Some(repo.to_string_lossy().to_string()),
                    source_branch: "main".to_string(),
                    branch_name: "develop".to_string(),
                    initial_branch: "develop".to_string(),
                    branch_provenance: dux_core::model::BranchProvenance::AttachedExisting,
                    worktree_path: worktree.to_string_lossy().to_string(),
                },
            ),
        };

        remove_session_worktree(
            &paths,
            session
                .workspace
                .as_managed()
                .expect("the fixture builds a managed agent"),
            &[],
        );

        assert!(!worktree.exists(), "the worktree directory must be removed");
        let branches = dux_core::test_git::fixture_git()
            .args(["-C", repo.to_str().unwrap(), "branch", "--list", "develop"])
            .output()
            .expect("git branch --list");
        assert!(
            String::from_utf8_lossy(&branches.stdout).contains("develop"),
            "a branch that existed before the agent must survive a factory reset",
        );
    }

    /// Convergence regression: factory-reset worktree removal must prune the
    /// repo's worktree registration and delete the branch, exactly as core
    /// `git::remove_worktree` does. The old CLI copy ran `git worktree remove`
    /// WITHOUT `-C <repo>` (so it hit the wrong repo, failed, and fell back to a
    /// bare `fs::remove_dir_all`) and never pruned, leaving a stale worktree ref
    /// that made the branch undeletable. This proves the branch is gone.
    #[test]
    fn factory_reset_worktree_removal_prunes_and_deletes_the_branch() {
        let tempdir = TempDir::new().expect("tempdir");
        let repo = tempdir.path().join("repo");
        fs::create_dir_all(&repo).expect("repo dir");
        let git = |cwd: &Path, args: &[&str]| {
            let out = dux_core::test_git::fixture_git()
                .args(args)
                .current_dir(cwd)
                .output()
                .expect("git");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "user.email", "t@example.com"]);
        git(&repo, &["config", "user.name", "Test User"]);
        git(&repo, &["commit", "--allow-empty", "-m", "initial"]);

        let worktrees_root = tempdir.path().join("worktrees");
        fs::create_dir_all(&worktrees_root).expect("worktrees root");
        let worktree = worktrees_root.join("wt");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "branch-wt",
                worktree.to_str().unwrap(),
            ],
        );

        let paths = DuxPaths {
            config_path: tempdir.path().join("config.toml"),
            sessions_db_path: tempdir.path().join("sessions.sqlite3"),
            worktrees_root: worktrees_root.clone(),
            lock_path: tempdir.path().join("dux.lock"),
            root: tempdir.path().to_path_buf(),
        };
        let now = Utc::now();
        let session = AgentSession {
            id: "wt".to_string(),
            slot_tab_id: "wt-slot".to_string(),
            provider: ProviderKind::new("claude"),
            title: None,
            started_providers: Vec::new(),
            desired_running: false,
            auto_reopen_enabled: true,
            status: SessionStatus::Active,
            created_at: now,
            updated_at: now,
            last_focused_tab: None,
            workspace: dux_core::model::AgentWorkspace::Managed(
                dux_core::model::ManagedWorkspace {
                    project_id: "proj".to_string(),
                    project_path: Some(repo.to_string_lossy().to_string()),
                    source_branch: "main".to_string(),
                    branch_name: "branch-wt".to_string(),
                    initial_branch: "branch-wt".to_string(),
                    branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
                    worktree_path: worktree.to_string_lossy().to_string(),
                },
            ),
        };

        remove_session_worktree(
            &paths,
            session
                .workspace
                .as_managed()
                .expect("the fixture builds a managed agent"),
            &[],
        );

        assert!(!worktree.exists(), "the worktree directory must be removed");
        let branches = dux_core::test_git::fixture_git()
            .args([
                "-C",
                repo.to_str().unwrap(),
                "branch",
                "--list",
                "branch-wt",
            ])
            .output()
            .expect("git branch --list");
        assert!(
            String::from_utf8_lossy(&branches.stdout).trim().is_empty(),
            "the branch must be deleted (a stale worktree ref would keep it undeletable)",
        );
        let worktrees = dux_core::test_git::fixture_git()
            .args([
                "-C",
                repo.to_str().unwrap(),
                "worktree",
                "list",
                "--porcelain",
            ])
            .output()
            .expect("git worktree list");
        // Match the removed worktree's FULL path, never the bare "wt" dir name:
        // `git worktree list` always names the main worktree, whose path is the
        // random tempfile dir, and a 2-char substring like "wt" matches that
        // random path by chance (a rare-but-real CI flake). The full path is
        // unique to the removed registration, so its absence is the real signal.
        let removed_registration = worktree.to_string_lossy();
        assert!(
            !String::from_utf8_lossy(&worktrees.stdout).contains(removed_registration.as_ref()),
            "no stale worktree registration for the removed path may remain in the repo",
        );
    }

    #[test]
    fn factory_reset_of_a_locked_worktree_still_forgets_it_and_deletes_the_branch() {
        let tempdir = TempDir::new().expect("tempdir");
        let repo = tempdir.path().join("repo");
        fs::create_dir_all(&repo).expect("repo dir");
        let git = |cwd: &Path, args: &[&str]| {
            let out = dux_core::test_git::fixture_git()
                .args(args)
                .current_dir(cwd)
                .output()
                .expect("git");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "user.email", "t@example.com"]);
        git(&repo, &["config", "user.name", "Test User"]);
        git(&repo, &["commit", "--allow-empty", "-m", "initial"]);

        let worktrees_root = tempdir.path().join("worktrees");
        fs::create_dir_all(&worktrees_root).expect("worktrees root");
        let worktree = worktrees_root.join("wt");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "branch-wt",
                worktree.to_str().unwrap(),
            ],
        );
        // git refuses to remove a locked worktree while it is still
        // registered: the reset deletes the folder itself, then forgets this
        // registration and deletes the branch the way a removal does.
        git(&repo, &["worktree", "lock", worktree.to_str().unwrap()]);

        let paths = DuxPaths {
            config_path: tempdir.path().join("config.toml"),
            sessions_db_path: tempdir.path().join("sessions.sqlite3"),
            worktrees_root: worktrees_root.clone(),
            lock_path: tempdir.path().join("dux.lock"),
            root: tempdir.path().to_path_buf(),
        };
        let now = Utc::now();
        let session = AgentSession {
            id: "wt".to_string(),
            slot_tab_id: "wt-slot".to_string(),
            provider: ProviderKind::new("claude"),
            title: None,
            started_providers: Vec::new(),
            desired_running: false,
            auto_reopen_enabled: true,
            status: SessionStatus::Active,
            created_at: now,
            updated_at: now,
            last_focused_tab: None,
            workspace: dux_core::model::AgentWorkspace::Managed(
                dux_core::model::ManagedWorkspace {
                    project_id: "proj".to_string(),
                    project_path: Some(repo.to_string_lossy().to_string()),
                    source_branch: "main".to_string(),
                    branch_name: "branch-wt".to_string(),
                    initial_branch: "branch-wt".to_string(),
                    branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
                    worktree_path: worktree.to_string_lossy().to_string(),
                },
            ),
        };

        remove_session_worktree(
            &paths,
            session
                .workspace
                .as_managed()
                .expect("the fixture builds a managed agent"),
            &[],
        );

        assert!(!worktree.exists(), "the worktree directory must be removed");
        let branches = dux_core::test_git::fixture_git()
            .args([
                "-C",
                repo.to_str().unwrap(),
                "branch",
                "--list",
                "branch-wt",
            ])
            .output()
            .expect("git branch --list");
        assert!(
            String::from_utf8_lossy(&branches.stdout).trim().is_empty(),
            "the branch must be deleted (a stale worktree ref would keep it undeletable)",
        );
        let worktrees = dux_core::test_git::fixture_git()
            .args([
                "-C",
                repo.to_str().unwrap(),
                "worktree",
                "list",
                "--porcelain",
            ])
            .output()
            .expect("git worktree list");
        // Match the removed worktree's FULL path, never the bare "wt" dir name:
        // `git worktree list` always names the main worktree, whose path is the
        // random tempfile dir, and a 2-char substring like "wt" matches that
        // random path by chance (a rare-but-real CI flake). The full path is
        // unique to the removed registration, so its absence is the real signal.
        let removed_registration = worktree.to_string_lossy();
        assert!(
            !String::from_utf8_lossy(&worktrees.stdout).contains(removed_registration.as_ref()),
            "no stale worktree registration for the removed path may remain in the repo",
        );
    }

    #[test]
    fn the_factory_reset_summary_counts_the_worktrees() {
        assert_eq!(removed_worktrees_line(0), "removed 0 session worktrees");
        assert_eq!(removed_worktrees_line(1), "removed 1 session worktree");
        assert_eq!(removed_worktrees_line(3), "removed 3 session worktrees");
    }

    /// A link in the worktrees root (worktrees kept on another disk, say)
    /// points at a folder outside it. The reset only unlinks the link and
    /// keeps the folder, but a terminal's recorded job working there is still
    /// stopped, like every program dux started.
    #[test]
    fn a_factory_reset_stops_a_job_in_the_target_of_a_link_it_only_unlinks() {
        use std::os::unix::process::CommandExt;
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = DuxPaths {
            root: tmp.path().to_path_buf(),
            config_path: tmp.path().join("config.toml"),
            sessions_db_path: tmp.path().join("sessions.sqlite3"),
            worktrees_root: tmp.path().join("worktrees"),
            lock_path: tmp.path().join("dux.lock"),
        };
        let elsewhere = tmp.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).expect("elsewhere");
        fs::write(elsewhere.join("keep.txt"), "mine\n").expect("seed");
        fs::create_dir_all(&paths.worktrees_root).expect("root");
        std::os::unix::fs::symlink(&elsewhere, paths.worktrees_root.join("ssd")).expect("link");
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).expect("home");
        let mut command = std::process::Command::new("sleep");
        command.arg("60").current_dir(&elsewhere);
        unsafe {
            command.pre_exec(|| {
                rustix::process::setsid().map_err(std::io::Error::from)?;
                Ok(())
            });
        }
        let mut job = command.spawn().expect("spawn the job");
        let session = dux_core::process_sessions::ProcessSession::started_now(job.id());
        let store = SessionStore::open(&paths.sessions_db_path).expect("store");
        store
            .replace_process_registry(&[dux_core::process_sessions::StoredSession {
                owner: Some(dux_core::process_sessions::UNOWNED_PTYS.to_string()),
                session,
                folder: home.clone(),
                standalone: false,
                survivors: Vec::new(),
                label: Some("a standalone terminal".to_string()),
            }])
            .expect("save the registry");
        drop(store);

        let _ = reset_agent_data(&paths);

        let alive = job.try_wait().expect("try_wait").is_none();
        let _ = job.kill();
        let _ = job.wait();
        assert!(
            elsewhere.join("keep.txt").exists(),
            "the link's target is kept"
        );
        assert!(
            !alive,
            "a terminal's job working in {}, a folder the reset keeps, was left running",
            elsewhere.display()
        );
    }

    /// An agent's terminal session started in its worktree left a job the
    /// user moved into their own project's repository (which the reset keeps).
    /// The job is stopped too: the reset stops everything dux started.
    #[test]
    fn a_factory_reset_stops_a_job_in_a_kept_repository_whose_session_started_in_a_worktree() {
        use std::os::unix::process::CommandExt;
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = DuxPaths {
            root: tmp.path().to_path_buf(),
            config_path: tmp.path().join("config.toml"),
            sessions_db_path: tmp.path().join("sessions.sqlite3"),
            worktrees_root: tmp.path().join("worktrees"),
            lock_path: tmp.path().join("dux.lock"),
        };
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).expect("repo");
        let worktree = paths.worktrees_root.join("proj").join("feat");
        fs::create_dir_all(&worktree).expect("worktree");
        // Leader in the worktree; its job in the user's repository.
        let mut command = std::process::Command::new("sh");
        command
            .arg("-c")
            .arg(format!(
                "(cd '{}' && exec sleep 61) & exec sleep 62",
                repo.display()
            ))
            .current_dir(&worktree);
        unsafe {
            command.pre_exec(|| {
                rustix::process::setsid().map_err(std::io::Error::from)?;
                Ok(())
            });
        }
        let mut job = command.spawn().expect("spawn");
        let session = dux_core::process_sessions::ProcessSession::started_now(job.id());
        std::thread::sleep(std::time::Duration::from_millis(300));
        let child_pid = dux_core::process_sessions::read_process_table()
            .into_iter()
            .find(|row| row.ppid == Some(job.id()))
            .map(|row| row.pid)
            .expect("the job in the repository");
        let store = SessionStore::open(&paths.sessions_db_path).expect("store");
        store
            .replace_process_registry(&[dux_core::process_sessions::StoredSession {
                owner: Some("m1".to_string()),
                session,
                folder: worktree.clone(),
                standalone: false,
                survivors: Vec::new(),
                label: Some("terminal of agent m1".to_string()),
            }])
            .expect("save the registry");
        drop(store);

        let _ = reset_agent_data(&paths);

        std::thread::sleep(std::time::Duration::from_millis(200));
        let repo_job_alive = dux_core::process_sessions::read_process_table()
            .iter()
            .any(|row| row.pid == child_pid && !row.exited);
        let _ = rustix::process::kill_process_group(
            rustix::process::Pid::from_raw(job.id() as i32).unwrap(),
            rustix::process::Signal::KILL,
        );
        let _ = job.kill();
        let _ = job.wait();
        assert!(
            !repo_job_alive,
            "a job (pid {child_pid}) working in the kept repository {} was left running",
            repo.display()
        );
    }
}

#[cfg(test)]
mod config_diff_names_tests {
    use super::*;
    const TOKEN: &str = "sk-proj-AbCdEf0123456789";

    /// `dux config diff` (the summary that is safe to paste into a bug
    /// report) must not print a provider name that breaks the provider-name
    /// rule: such a name may be a token pasted in the wrong place.
    #[test]
    fn config_diff_does_not_print_a_rule_breaking_provider_name() {
        let text = format!("[providers.\"{TOKEN} x\"]\ncommand = \"a\"\n");
        let config: Config = toml::from_str(&text).unwrap();
        let changes = collect_config_changes(&config).join("\n");
        assert!(!changes.contains(TOKEN), "{changes}");
    }

    /// The same for a `[keys]` name that is no action.
    #[test]
    fn config_diff_does_not_print_a_rule_breaking_keys_name() {
        let text = format!("[keys]\n\"{TOKEN} x\" = [\"ctrl-q\"]\n");
        let config: Config = toml::from_str(&text).unwrap();
        let changes = collect_config_changes(&config).join("\n");
        assert!(!changes.contains(TOKEN), "{changes}");
    }
}

#[cfg(test)]
#[path = "cli/preview_leak_tests.rs"]
mod preview_leak_tests;

#[cfg(test)]
#[path = "cli/plaintext_shape_preview_tests.rs"]
mod plaintext_shape_preview_tests;
