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

// ---------------------------------------------------------------------------
// CLI dispatch
// ---------------------------------------------------------------------------

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
            reject_unknown_flags(&args[1..], &["--yes"])?;
            run_regenerate(paths, yes)
        }
        "restore-docs" => {
            let yes = args[1..].iter().any(|a| a == "--yes");
            reject_unknown_flags(&args[1..], &["--yes"])?;
            run_restore_docs(paths, yes)
        }
        "path" => {
            println!("{}", paths.config_path.display());
            Ok(())
        }
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
dux config — manage the dux configuration file

Subcommands:
  dux config path          Print the config file path
  dux config diff          Show settings that differ from defaults (summary;
                           [env] and project details are summarized, never
                           printed, so it is safe to paste into a bug report)
  dux config diff --raw    Show a unified diff against the default config.
                           This prints the WHOLE config, [env] values included:
                           redact it before sharing.
  dux config reset         Remove config and logs (keeps agents and worktrees)
  dux config reset --all   Full factory reset: remove config, logs, sessions, and worktrees
  dux config regenerate    Preview a fresh default config (shows diff)
  dux config regenerate --yes
                           Overwrite the config file with fresh defaults
  dux config restore-docs  Preview re-adding the explanatory comments to your
                           config, keeping every value you have set
  dux config restore-docs --yes
                           Apply it (writes a timestamped backup first)"
    );
}

// ---------------------------------------------------------------------------
// dux config reset
// ---------------------------------------------------------------------------

fn run_reset(paths: &DuxPaths, all: bool) -> Result<()> {
    if !paths.root.exists() {
        println!("nothing to reset: {} does not exist", paths.root.display());
        return Ok(());
    }

    let log_path = resolve_reset_log_path(paths);

    if all {
        reset_agent_data(paths)?;
    }

    remove_file_with_message(&log_path)?;
    prune_empty_ancestors(&log_path, &paths.root)?;
    remove_file_with_message(&paths.config_path)?;
    prune_empty_ancestors(&paths.config_path, &paths.root)?;

    // The lockfile (`dux.lock`) is intentionally left in place. Unlinking
    // it while holding the flock would orphan the inode: a new process
    // could create a fresh file at the same path (different inode) and
    // successfully flock it, breaking the single-instance guarantee. The
    // stale lockfile is harmless — the next launch takes it over
    // transparently — so `remove_root_if_empty` will simply skip removal
    // of root when the lockfile is the sole remaining entry.
    remove_root_if_empty_with_message(&paths.root)?;

    println!("reset complete");
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
    let current: Config = toml::from_str(&current_raw).with_context_path(&paths.config_path)?;

    if raw {
        run_diff_raw(&current_raw, &current)?;
    } else {
        run_diff_summary(&current)?;
    }
    Ok(())
}

fn run_diff_raw(_current_raw: &str, current: &Config) -> Result<()> {
    let bindings = RuntimeBindings::from_keys_config(&current.keys);
    let default_rendered = config::render_default_config();
    // Re-render current config to normalize it before diffing.
    let current_rendered = render_config_for_diff(current, &bindings);
    if current_rendered == default_rendered {
        println!("config matches defaults — no differences");
        return Ok(());
    }
    print_unified_diff("default", "current", &default_rendered, &current_rendered);
    Ok(())
}

fn run_diff_summary(current: &Config) -> Result<()> {
    let changes = collect_config_changes(current);
    if changes.is_empty() {
        println!("config matches defaults — no differences");
    } else {
        for line in &changes {
            println!("  {line}");
        }
    }
    Ok(())
}

/// Every setting whose current value differs from the default, as display lines.
///
/// DERIVED, never hand-maintained. `current` and [`Config::default()`] are both
/// projected to `serde_json::Value` and walked structurally, so a config key
/// added anywhere in the struct tree is reported without anyone registering it
/// here. The previous version was a hand-written list, and it had already
/// drifted: every web-only `[ui]` field was missing from it and therefore
/// silently absent from `dux config diff`.
///
/// `serde_json` and not `toml` on purpose. TOML has no null and its serializer
/// simply omits a `None` struct field, which would make every default-`None`
/// setting (`defaults.start_directory`, the optional provider fields) invisible
/// to the comparison. JSON keeps them as an explicit null.
///
/// WHAT IS COMPARED is the PARSED FILE against [`Config::default()`]. This
/// deliberately does not call `load_config` or `ProvidersConfig::ensure_defaults`,
/// so the summary reports what the file says rather than what dux normalizes it
/// into: no value clamping, and no shipped provider injected into a config that
/// does not name it. That was the old behavior too; it is stated here so it is a
/// decision rather than an accident.
fn collect_config_changes(current: &Config) -> Vec<String> {
    let (Ok(default_json), Ok(current_json)) = (
        serde_json::to_value(Config::default()),
        serde_json::to_value(current),
    ) else {
        return Vec::new();
    };

    let mut found: Vec<(String, String)> = Vec::new();
    let mut path: Vec<String> = Vec::new();
    diff_node(&mut found, &mut path, &default_json, &current_json);

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
/// reads and never reports is exactly the silent drift this rewrite removed.
/// Something too sensitive or too unstable to print is [`Policy::Summarize`]d,
/// which still tells the user that it changed.
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
    /// `macros: 2 macro(s) configured`, for the given singular noun.
    Count(&'static str),
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
    let segments: Vec<&str> = path.iter().map(String::as_str).collect();
    match segments.as_slice() {
        // Holds API tokens. The value must never reach the terminal, a log, or a
        // pasted bug report, so this reports the fact and nothing else.
        ["env"] => Policy::Summarize(Summary::Changed),
        // An array index is not a stable identity and `ProjectConfig::id` can be
        // generated at deserialize time, so there is no honest per-project path
        // to print. Projects also carry their own `env`, which must stay
        // unprinted for the reason above.
        ["projects"] => Policy::Summarize(Summary::Count("project")),
        // A macro body is arbitrary user prose, frequently long and multi-line.
        // Counting them is what this command has always done.
        ["macros"] => Policy::Summarize(Summary::Count("macro")),
        _ => Policy::Recurse,
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
            let dotted = join_path(path);
            let line = match summary {
                Summary::Changed => format!("{dotted}: changed"),
                Summary::Count(noun) => {
                    format!("{dotted}: {} {noun}(s) configured", collection_len(current))
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
                        (Some(d), Some(c)) => diff_node(found, path, d, c),
                        (Some(d), None) => push_missing(found, path, &style, Side::DefaultOnly, d),
                        (None, Some(c)) => push_missing(found, path, &style, Side::CurrentOnly, c),
                        (None, None) => {}
                    }
                    path.pop();
                }
            }
            // Every other shape, arrays included, is one value. `terminal.args`
            // and `server.allowed_hosts` are settings in their own right, not
            // parents of a `terminal.args.0`.
            _ => {
                let dotted = join_path(path);
                let line = format!(
                    "{dotted}: {} -> {}",
                    format_value(default),
                    format_value(current)
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
    found: &mut Vec<(String, String)>,
    path: &[String],
    style: &MissingStyle,
    side: Side,
    value: &serde_json::Value,
) {
    let dotted = join_path(path);
    let line = match (style, side) {
        (MissingStyle::Marker, Side::CurrentOnly) => format!("{dotted}: (added)"),
        (MissingStyle::Marker, Side::DefaultOnly) => format!("{dotted}: (removed)"),
        (MissingStyle::Valued, Side::CurrentOnly) => {
            format!("{dotted}: (new) -> {}", format_value(value))
        }
        (MissingStyle::Valued, Side::DefaultOnly) => {
            format!("{dotted}: {} -> (removed)", format_value(value))
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

/// Join structural segments into a dotted path.
///
/// Segments are structural and are never reparsed out of a rendered string:
/// provider and macro names are user-controlled keys and may contain a dot
/// themselves. A segment that is not a bare TOML key (ASCII letters, digits,
/// `_`, `-`) is quoted, so `providers."my agent.v2".command` reads
/// unambiguously. The quoting is JSON string quoting, which escapes the quote
/// and the backslash the same way a TOML basic string does.
fn join_path(path: &[String]) -> String {
    path.iter()
        .map(|segment| quote_segment(segment))
        .collect::<Vec<_>>()
        .join(".")
}

fn quote_segment(segment: &str) -> String {
    let bare = !segment.is_empty()
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if bare {
        segment.to_string()
    } else {
        serde_json::Value::String(segment.to_string()).to_string()
    }
}

/// Render one value the way the summary shows it: unquoted, one line, truncated.
fn format_value(value: &serde_json::Value) -> String {
    let rendered = match value {
        serde_json::Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(format_element)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        other => format_element(other),
    };
    truncate_display(&rendered, 40)
}

fn format_element(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        // An absent optional setting. Matches what this command has always
        // printed for an unset `defaults.start_directory`.
        serde_json::Value::Null => "(unset)".to_string(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// dux config regenerate
// ---------------------------------------------------------------------------

#[allow(deprecated)] // blessed sync-direct: `dux config regenerate` is a CLI-only, one-shot boot tool
fn run_regenerate(paths: &DuxPaths, yes: bool) -> Result<()> {
    let fresh = config::render_default_config();

    if !yes {
        if paths.config_path.exists() {
            let current =
                fs::read_to_string(&paths.config_path).with_context_path(&paths.config_path)?;
            if current == fresh {
                println!("config already matches defaults — nothing to do");
                return Ok(());
            }
            print_unified_diff("current", "default", &current, &fresh);
            println!("\nRun `dux config regenerate --yes` to overwrite with these defaults.");
        } else {
            println!("no config file exists; regenerate --yes will create one at:");
            println!("  {}", paths.config_path.display());
        }
        return Ok(());
    }

    paths.ensure_dirs()?;
    dux_core::config_write::write_config_secure(&paths.config_path, &fresh)
        .with_context_path(&paths.config_path)?;
    println!("config regenerated at {}", paths.config_path.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// dux config restore-docs
// ---------------------------------------------------------------------------

/// Re-apply the commented template to the existing config, keeping every value.
///
/// Non-destructive by default (preview only), mirroring `dux config regenerate`:
/// `--yes` commits. Unlike `regenerate`, this never falls back to defaults — an
/// unparseable config is refused outright, because the whole point of the
/// command is to be the safe alternative to a defaults-based rewrite.
#[allow(deprecated)] // blessed sync-direct: CLI-only, one-shot, runs before any engine/queue exists
fn run_restore_docs(paths: &DuxPaths, yes: bool) -> Result<()> {
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
        println!("config documentation is already up to date — nothing to do");
        return Ok(());
    }

    if !yes {
        print_unified_diff("current", "restored", &raw, &restored.text);
        print_restore_report(&restored);
        println!("\nRun `dux config restore-docs --yes` to apply this (a timestamped backup");
        println!("of your current config is written first).");
        return Ok(());
    }

    // Back up BEFORE committing. The writer below is atomic, which protects
    // against a torn file, but not against "the result was not what I wanted".
    let backup_path = backup_config(&paths.config_path, &raw)?;

    dux_core::config_write::write_config_secure(&paths.config_path, &restored.text)
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

    #[allow(deprecated)] // blessed sync-direct: CLI-only one-shot; also gives the backup 0600
    dux_core::config_write::write_config_secure(&candidate, raw).with_context_path(&candidate)?;
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

fn print_unified_diff(label_a: &str, label_b: &str, a: &str, b: &str) {
    let diff = similar::TextDiff::from_lines(a, b);
    println!("--- {label_a}");
    println!("+++ {label_b}");
    for hunk in diff.unified_diff().context_radius(3).iter_hunks() {
        println!("{hunk}");
    }
}

// ---------------------------------------------------------------------------
// Agent data reset
// ---------------------------------------------------------------------------

fn reset_agent_data(paths: &DuxPaths) -> Result<()> {
    // Folders a STANDALONE agent occupies. Collected because the sweep of the
    // whole worktrees root below is otherwise indiscriminate: nothing stops a
    // user pointing a standalone agent at a directory inside dux's managed
    // area, and dux resets what dux MADE, which that directory is not.
    let mut occupied_folders: Vec<PathBuf> = Vec::new();
    if paths.sessions_db_path.exists() {
        match SessionStore::open(&paths.sessions_db_path) {
            Ok(store) => match store.load_sessions() {
                Ok(sessions) => {
                    // A STANDALONE agent's folder is the user's and is never
                    // removed, not even by a factory reset: dux resets what dux
                    // made, and it did not make that directory. Its record goes
                    // with the database below like every other agent's.
                    //
                    // The filter is on the workspace, not on the managed-root
                    // path check inside the removal: a standalone agent pointed
                    // AT a directory under dux's managed root would sail past
                    // that check and have the ground deleted from under it.
                    let mut removed = 0usize;
                    for session in &sessions {
                        match session.workspace.as_managed() {
                            Some(managed) => {
                                remove_session_worktree(paths, managed);
                                removed += 1;
                            }
                            None => occupied_folders
                                .push(canonical_or_original(Path::new(session.directory()))),
                        }
                    }
                    println!("removed {removed} session worktree(s)");
                }
                Err(error) => {
                    eprintln!("warning: could not load sessions from database: {error}");
                }
            },
            Err(error) => {
                eprintln!("warning: could not open session database: {error}");
            }
        }
    }

    // The sweep that finishes the job: whatever the per-session loop could not
    // account for (a worktree whose row was already gone, a stray directory)
    // goes with the root.
    //
    // Except a folder a standalone agent occupies. Removing the root wholesale
    // undid the filter above one line later, which is the exact scenario
    // `remove_session_worktree`'s doc comment warns about, arriving by the
    // other door. When one is in the way, the root's other entries are removed
    // individually and the root itself is left standing around them.
    if occupied_folders.is_empty() {
        remove_dir_with_message(&paths.worktrees_root)?;
    } else {
        remove_worktrees_root_sparing(&paths.worktrees_root, &occupied_folders)?;
    }
    remove_file_with_message(&paths.sessions_db_path)?;
    Ok(())
}

/// Clear the managed worktrees root, leaving every entry that CONTAINS OR IS a
/// folder a standalone agent occupies.
///
/// Containment, not equality: an agent pointed at `worktrees/a/b` must keep
/// `worktrees/a` too, or removing the parent takes the child with it. Compared
/// canonically, so a symlinked spelling cannot slip past.
///
/// Continue-on-error, like the rest of the reset: one undeletable entry must
/// not stop the others.
fn remove_worktrees_root_sparing(root: &Path, occupied: &[PathBuf]) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    let Ok(entries) = fs::read_dir(root) else {
        eprintln!(
            "warning: could not read {} to reset it; left as is",
            root.display()
        );
        return Ok(());
    };
    let mut kept = 0usize;
    for entry in entries.flatten() {
        let path = canonical_or_original(&entry.path());
        if occupied.iter().any(|folder| folder.starts_with(&path)) {
            kept += 1;
            continue;
        }
        let removed = if entry.path().is_dir() {
            fs::remove_dir_all(entry.path())
        } else {
            fs::remove_file(entry.path())
        };
        if let Err(err) = removed {
            eprintln!(
                "warning: could not remove {}: {err}",
                entry.path().display()
            );
        }
    }
    println!(
        "reset {} but kept {kept} entr{} a standalone agent is running in",
        root.display(),
        if kept == 1 { "y" } else { "ies" }
    );
    Ok(())
}

/// Remove one agent's MANAGED worktree during a factory reset.
///
/// It takes a [`ManagedWorkspace`], not a session, and that is the guard: this
/// function ends in an unconditional `remove_dir_all`, so a standalone agent's
/// folder must not be nameable here at all. The managed-root path check below
/// is not enough on its own, because a standalone agent pointed at a directory
/// under dux's managed root would pass it.
fn remove_session_worktree(paths: &DuxPaths, managed: &dux_core::model::ManagedWorkspace) {
    let worktree = Path::new(&managed.worktree_path);
    if !git::is_under(&paths.worktrees_root, worktree) {
        eprintln!(
            "warning: skipping worktree outside of managed root: {}",
            managed.worktree_path
        );
        return;
    }

    // Route through the shared core removal so the worktree is removed with the
    // correct `-C <repo>`, the repo's worktree registration is pruned, and the
    // branch is deleted afterward. The old inline copy ran `git worktree remove`
    // in the CLI's own cwd (no `-C`), which hit the wrong repo, failed, and left
    // a stale worktree ref that made the branch undeletable. Continue-on-error is
    // preserved: a factory reset must press on past any single failure.
    if let Some(project_path) = managed.project_path.as_deref() {
        // The same branch-ownership gate the engine's delete applies, for the
        // same reason. Both halves matter: a reset that left a drifted agent's
        // own original branch behind would not be a reset, and a reset that
        // deleted the user's `develop` because an agent was once attached to it
        // would not be a reset either, it would be data loss. dux resets what
        // dux made.
        if managed.branch_provenance.dux_may_delete_branch() {
            let _ = git::remove_worktree(
                Path::new(project_path),
                worktree,
                &managed.branch_name,
                Some(managed.initial_branch.as_str()),
            );
        } else {
            let _ = git::remove_worktree_keep_branch(Path::new(project_path), worktree);
        }
    }

    // Belt-and-suspenders for the factory-reset guarantee: ensure the directory is
    // gone even when there is no owning repo to drive git (an orphan with no
    // `project_path`) or git could not remove it. Core `remove_worktree` never
    // filesystem-deletes, so this stays the CLI's own last resort.
    if worktree.exists() {
        let _ = fs::remove_dir_all(worktree);
    }
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

fn remove_dir_with_message(path: &Path) -> Result<()> {
    if remove_dir_if_present(path)? {
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

fn remove_dir_if_present(path: &Path) -> Result<bool> {
    match fs::remove_dir_all(path) {
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
    use crate::config::{self, Config};
    use crate::keybindings::RuntimeBindings;
    use crate::model::{AgentSession, ProviderKind, SessionStatus};

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
            serde_json::Value::String(s) => vec![serde_json::json!(format!("{s}-mutated"))],
            serde_json::Value::Array(items) => {
                let mut grown = items.clone();
                grown.push(serde_json::json!("dux-diff-probe"));
                vec![serde_json::Value::Array(grown)]
            }
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
    fn config_diff_quotes_a_provider_name_that_is_not_a_bare_key() {
        let mut config = Config::default();
        config.providers.commands.insert(
            "my agent.v2".to_string(),
            config::ProviderCommandConfig::default(),
        );

        assert_eq!(
            collect_config_changes(&config),
            vec!["providers.\"my agent.v2\": (added)".to_string()]
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
        assert_eq!(
            changes,
            vec!["projects: 1 project(s) configured".to_string()]
        );
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
        assert_eq!(changes, vec!["macros: 1 macro(s) configured".to_string()]);
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
        let harness = ResetHarness::new();
        harness.write_config_with_log_path("logs/custom.log");
        harness.write_log("logs/custom.log");
        harness.create_session("agent-1");

        run_reset(&harness.paths, true).expect("reset");

        assert!(!harness.paths.root.exists());
    }

    #[test]
    fn reset_succeeds_when_paths_are_already_missing() {
        let harness = ResetHarness::new();
        fs::create_dir_all(&harness.paths.root).expect("root");

        run_reset(&harness.paths, false).expect("reset");

        assert!(!harness.paths.root.exists());
    }

    #[test]
    fn reset_all_removes_worktrees_without_database() {
        let harness = ResetHarness::new();
        fs::create_dir_all(harness.paths.worktrees_root.join("orphan")).expect("orphan worktree");

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
        let harness = ResetHarness::new();
        let original = bare_user_config_fixture();
        fs::write(&harness.paths.config_path, &original).expect("seed");

        run(
            &["restore-docs".to_string(), "--yes".to_string()],
            &harness.paths,
        )
        .expect("apply");

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
            let out = std::process::Command::new("git")
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
        );

        assert!(!worktree.exists(), "the worktree directory must be removed");
        let branches = std::process::Command::new("git")
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
            let out = std::process::Command::new("git")
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
        );

        assert!(!worktree.exists(), "the worktree directory must be removed");
        let branches = std::process::Command::new("git")
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
        let worktrees = std::process::Command::new("git")
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
}
