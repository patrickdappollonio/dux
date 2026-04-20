use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};

use crate::config::{self, Config, DuxPaths};
use crate::git;
use crate::keybindings::RuntimeBindings;
use crate::logger;
use crate::storage::SessionStore;

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
  dux config diff          Show settings that differ from defaults (summary)
  dux config diff --raw    Show a unified diff against the default config
  dux config reset         Remove config and logs (keeps agents and worktrees)
  dux config reset --all   Full factory reset: remove config, logs, sessions, and worktrees
  dux config regenerate    Preview a fresh default config (shows diff)
  dux config regenerate --yes
                           Overwrite the config file with fresh defaults"
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
    let defaults = Config::default();
    let mut changes = Vec::new();

    // [defaults]
    diff_str(
        &mut changes,
        "defaults.provider",
        &defaults.defaults.provider,
        &current.defaults.provider,
    );
    diff_opt_str(
        &mut changes,
        "defaults.start_directory",
        defaults.defaults.start_directory.as_deref(),
        current.defaults.start_directory.as_deref(),
    );
    diff_opt_str(
        &mut changes,
        "defaults.commit_prompt",
        defaults.defaults.commit_prompt.as_deref(),
        current.defaults.commit_prompt.as_deref(),
    );

    // [logging]
    diff_str(
        &mut changes,
        "logging.level",
        &defaults.logging.level,
        &current.logging.level,
    );
    diff_str(
        &mut changes,
        "logging.path",
        &defaults.logging.path,
        &current.logging.path,
    );

    // [ui]
    diff_u16(
        &mut changes,
        "ui.left_width_pct",
        defaults.ui.left_width_pct,
        current.ui.left_width_pct,
    );
    diff_u16(
        &mut changes,
        "ui.right_width_pct",
        defaults.ui.right_width_pct,
        current.ui.right_width_pct,
    );
    diff_u16(
        &mut changes,
        "ui.terminal_pane_height_pct",
        defaults.ui.terminal_pane_height_pct,
        current.ui.terminal_pane_height_pct,
    );
    diff_u16(
        &mut changes,
        "ui.staged_pane_height_pct",
        defaults.ui.staged_pane_height_pct,
        current.ui.staged_pane_height_pct,
    );
    diff_u16(
        &mut changes,
        "ui.commit_pane_height_pct",
        defaults.ui.commit_pane_height_pct,
        current.ui.commit_pane_height_pct,
    );
    diff_usize(
        &mut changes,
        "ui.agent_scrollback_lines",
        defaults.ui.agent_scrollback_lines,
        current.ui.agent_scrollback_lines,
    );
    diff_u16(
        &mut changes,
        "ui.branch_sync_interval",
        defaults.ui.branch_sync_interval,
        current.ui.branch_sync_interval,
    );

    // [editor]
    diff_str(
        &mut changes,
        "editor.default",
        &defaults.editor.default,
        &current.editor.default,
    );

    // [terminal]
    diff_str(
        &mut changes,
        "terminal.command",
        &defaults.terminal.command,
        &current.terminal.command,
    );
    let default_args = defaults
        .terminal
        .args
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let current_args = current
        .terminal
        .args
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    diff_str(
        &mut changes,
        "terminal.args",
        &format!("[{default_args}]"),
        &format!("[{current_args}]"),
    );

    // [keys]
    diff_bool(
        &mut changes,
        "keys.show_terminal_keys",
        defaults.keys.show_terminal_keys,
        current.keys.show_terminal_keys,
    );
    diff_keybindings(
        &mut changes,
        &defaults.keys.bindings,
        &current.keys.bindings,
    );

    // [providers.*]
    diff_providers(&mut changes, &defaults, current);

    // [[projects]]
    if !current.projects.is_empty() {
        changes.push(format!(
            "projects: {} project(s) configured",
            current.projects.len()
        ));
    }

    // [macros]
    if !current.macros.entries.is_empty() {
        changes.push(format!(
            "macros: {} macro(s) configured",
            current.macros.entries.len()
        ));
    }

    if changes.is_empty() {
        println!("config matches defaults — no differences");
    } else {
        for line in &changes {
            println!("  {line}");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// dux config regenerate
// ---------------------------------------------------------------------------

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
    fs::write(&paths.config_path, fresh).with_context_path(&paths.config_path)?;
    println!("config regenerated at {}", paths.config_path.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// Diff helpers
// ---------------------------------------------------------------------------

fn diff_str(changes: &mut Vec<String>, key: &str, default: &str, current: &str) {
    if default != current {
        let d = truncate_display(default, 40);
        let c = truncate_display(current, 40);
        changes.push(format!("{key}: {d} -> {c}"));
    }
}

fn diff_opt_str(
    changes: &mut Vec<String>,
    key: &str,
    default: Option<&str>,
    current: Option<&str>,
) {
    let d = default.unwrap_or("(unset)");
    let c = current.unwrap_or("(unset)");
    if d != c {
        let d = truncate_display(d, 40);
        let c = truncate_display(c, 40);
        changes.push(format!("{key}: {d} -> {c}"));
    }
}

fn diff_u16(changes: &mut Vec<String>, key: &str, default: u16, current: u16) {
    if default != current {
        changes.push(format!("{key}: {default} -> {current}"));
    }
}

fn diff_usize(changes: &mut Vec<String>, key: &str, default: usize, current: usize) {
    if default != current {
        changes.push(format!("{key}: {default} -> {current}"));
    }
}

fn diff_bool(changes: &mut Vec<String>, key: &str, default: bool, current: bool) {
    if default != current {
        changes.push(format!("{key}: {default} -> {current}"));
    }
}

fn diff_keybindings(
    changes: &mut Vec<String>,
    default: &BTreeMap<String, Vec<String>>,
    current: &BTreeMap<String, Vec<String>>,
) {
    for (action, default_keys) in default {
        match current.get(action) {
            Some(current_keys) if current_keys != default_keys => {
                changes.push(format!(
                    "keys.{action}: [{}] -> [{}]",
                    default_keys.join(", "),
                    current_keys.join(", "),
                ));
            }
            None => {
                changes.push(format!(
                    "keys.{action}: [{}] -> (removed)",
                    default_keys.join(", ")
                ));
            }
            _ => {}
        }
    }
    for action in current.keys() {
        if !default.contains_key(action) {
            let keys = &current[action];
            changes.push(format!("keys.{action}: (new) -> [{}]", keys.join(", "),));
        }
    }
}

fn diff_providers(changes: &mut Vec<String>, defaults: &Config, current: &Config) {
    for (name, default_cfg) in &defaults.providers.commands {
        match current.providers.get(name) {
            Some(current_cfg) => {
                if default_cfg.command != current_cfg.command {
                    changes.push(format!(
                        "providers.{name}.command: {} -> {}",
                        default_cfg.command, current_cfg.command
                    ));
                }
            }
            None => {
                changes.push(format!("providers.{name}: (removed)"));
            }
        }
    }
    for name in current.providers.commands.keys() {
        if !defaults.providers.commands.contains_key(name) {
            changes.push(format!("providers.{name}: (added)"));
        }
    }
}

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
    if paths.sessions_db_path.exists() {
        match SessionStore::open(&paths.sessions_db_path) {
            Ok(store) => match store.load_sessions() {
                Ok(sessions) => {
                    for session in &sessions {
                        remove_session_worktree(paths, session);
                    }
                    println!("removed {} session worktree(s)", sessions.len());
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

    remove_dir_with_message(&paths.worktrees_root)?;
    remove_file_with_message(&paths.sessions_db_path)?;
    Ok(())
}

fn remove_session_worktree(paths: &DuxPaths, session: &crate::model::AgentSession) {
    let worktree = Path::new(&session.worktree_path);
    if !git::is_under(&paths.worktrees_root, worktree) {
        eprintln!(
            "warning: skipping worktree outside of managed root: {}",
            session.worktree_path
        );
        return;
    }

    if worktree.exists() {
        let _ = std::process::Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(worktree)
            .output();
        if worktree.exists() {
            let _ = fs::remove_dir_all(worktree);
        }
    }

    if let Some(project_path) = session.project_path.as_deref() {
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(project_path)
            .arg("branch")
            .arg("-D")
            .arg(&session.branch_name)
            .output();
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
    use std::fs;

    use chrono::Utc;
    use tempfile::TempDir;

    use super::*;
    use crate::config::{self, Config};
    use crate::keybindings::RuntimeBindings;
    use crate::model::{AgentSession, ProviderKind, SessionStatus};

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
        assert_eq!(sessions[0].worktree_path, worktree.to_string_lossy());
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
    fn diff_str_records_change() {
        let mut changes = Vec::new();
        diff_str(&mut changes, "test.key", "old", "new");
        assert_eq!(changes.len(), 1);
        assert!(changes[0].contains("old -> new"));
    }

    #[test]
    fn diff_str_ignores_equal() {
        let mut changes = Vec::new();
        diff_str(&mut changes, "test.key", "same", "same");
        assert!(changes.is_empty());
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
                    project_id: "proj".to_string(),
                    project_path: None,
                    provider: ProviderKind::new("claude"),
                    source_branch: "main".to_string(),
                    branch_name: format!("branch-{id}"),
                    worktree_path: worktree.to_string_lossy().to_string(),
                    title: None,
                    started_providers: Vec::new(),
                    status: SessionStatus::Active,
                    created_at: now,
                    updated_at: now,
                })
                .expect("session");
            worktree
        }
    }
}
