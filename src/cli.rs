use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, anyhow, bail};

use crate::config::{self, Config, DuxPaths};
use crate::git;
use crate::keybindings::RuntimeBindings;
use crate::logger;
use crate::purge::{
    self, PurgeConfig, PurgePlan, PurgeReport, build_plan, build_plans_for_all,
    confirm_interactive, execute, notify_amq_peers_of_purge,
};
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

// ---------------------------------------------------------------------------
// dux session — GDPR hard-purge command (audit02 Phase 10, P0-J)
// ---------------------------------------------------------------------------

/// Top-level dispatcher for `dux session <sub>`. Mirrors [`run`] but for
/// the session-management surface. Today the only mutating subcommand
/// is `purge`; future additions (e.g. `dux session export`) live here.
///
/// All subcommands assume the caller has already acquired the
/// single-instance lock. See `main.rs::main` for the lock acquisition
/// (it gates the dispatch to this function).
pub fn run_session(args: &[String], paths: &DuxPaths) -> Result<()> {
    let sub = args.first().map(|s| s.as_str()).unwrap_or("");
    match sub {
        "purge" => {
            let rest = &args[1..];
            reject_unknown_flags_and_positional(rest, &["--yes", "--dry-run", "--hard"])?;
            run_session_purge(paths, rest)
        }
        "purge-all" => {
            let rest = &args[1..];
            reject_unknown_flags(rest, &["--yes", "--dry-run"])?;
            let yes = rest.iter().any(|a| a == "--yes");
            let dry_run = rest.iter().any(|a| a == "--dry-run");
            run_session_purge_all(paths, yes, dry_run)
        }
        "" | "--help" | "-h" => {
            print_session_help();
            Ok(())
        }
        other => bail!("unknown session subcommand: {other}\nRun `dux session --help` for usage."),
    }
}

/// Like [`reject_unknown_flags`] but tolerant of one positional
/// argument (the purge target). Anything starting with `-` that is
/// not in `known` is rejected.
fn reject_unknown_flags_and_positional(args: &[String], known: &[&str]) -> Result<()> {
    for arg in args {
        if arg.starts_with('-') && !known.contains(&arg.as_str()) {
            bail!("unknown flag: {arg}");
        }
    }
    Ok(())
}

fn print_session_help() {
    println!(
        "\
dux session — manage individual sessions

Subcommands:
  dux session purge --hard <target> [--yes] [--dry-run]
                       Permanently delete a single session and ALL its on-disk
                       data. <target> is a session uuid or branch name.
                       Cascades into worktree, provider chat dirs (claude/codex/
                       gemini), AMQ inbox, log redact (Phase 09), and sqlite row.
  dux session purge-all [--yes] [--dry-run]
                       Run purge against every session. Useful for full reset.

Flags:
  --hard               Required for `purge`. Affirms the destructive intent.
  --yes                Skip the interactive 'PURGE <branch>' confirmation.
  --dry-run            Print the plan and exit without changing anything.

Exit codes:
  0  success (or dry-run)
  1  any item in the cascade reported an error
  2  invalid arguments / unknown target / aborted at confirmation"
    );
}

fn run_session_purge(paths: &DuxPaths, args: &[String]) -> Result<()> {
    let mut yes = false;
    let mut dry_run = false;
    let mut hard = false;
    let mut target: Option<String> = None;
    for arg in args {
        match arg.as_str() {
            "--yes" => yes = true,
            "--dry-run" => dry_run = true,
            "--hard" => hard = true,
            s if s.starts_with('-') => bail!("unknown flag: {s}"),
            other => {
                if target.is_some() {
                    bail!("unexpected positional argument: {other}");
                }
                target = Some(other.to_string());
            }
        }
    }
    if !hard {
        bail!(
            "`dux session purge` requires --hard to affirm the destructive intent.\n\
             This command permanently deletes worktree bytes, provider chat history,\n\
             AMQ inbox, log records, and the sqlite row. There is no undo.\n\
             Re-run as: dux session purge --hard <target>"
        );
    }
    let Some(target) = target else {
        bail!("missing target: dux session purge --hard <session-id-or-branch>");
    };

    if !paths.sessions_db_path.exists() {
        bail!(
            "no sessions database found at {} — nothing to purge",
            paths.sessions_db_path.display()
        );
    }

    let storage = SessionStore::open(&paths.sessions_db_path)?;
    let purge_config = PurgeConfig::default_layout();
    let plan = build_plan(&storage, paths, &purge_config, &target)?;

    println!("{}", format_plan(&plan, dry_run));

    if !yes && !dry_run {
        let confirmed = confirm_interactive(&plan)?;
        if !confirmed {
            eprintln!("aborted: confirmation phrase did not match");
            std::process::exit(2);
        }
    }

    // Notify peers BEFORE deleting the AMQ inbox so they have a chance
    // to drain the final message; skipped on dry-run.
    if !dry_run {
        notify_amq_peers_of_purge(&plan.branch);
    }

    let report = execute(&plan, &storage, paths, dry_run)?;
    eprint!("{}", report.summary());
    if report.had_errors() {
        std::process::exit(1);
    }
    Ok(())
}

fn run_session_purge_all(paths: &DuxPaths, yes: bool, dry_run: bool) -> Result<()> {
    if !paths.sessions_db_path.exists() {
        bail!(
            "no sessions database found at {} — nothing to purge",
            paths.sessions_db_path.display()
        );
    }
    let storage = SessionStore::open(&paths.sessions_db_path)?;
    let purge_config = PurgeConfig::default_layout();
    let plans = build_plans_for_all(&storage, paths, &purge_config)?;

    println!("about to purge {} session(s)", plans.len());
    for (i, plan) in plans.iter().enumerate() {
        println!("--- session {} of {} ---", i + 1, plans.len());
        println!("{}", format_plan(plan, dry_run));
    }

    if !yes && !dry_run {
        // For purge-all the confirmation phrase is "PURGE ALL" rather
        // than per-branch — typing every branch name is impractical
        // and "ALL" is the magic word that maps to the whole list.
        eprint!("Type 'PURGE ALL' to confirm: ");
        let _ = std::io::Write::flush(&mut std::io::stderr());
        let mut input = String::new();
        std::io::stdin()
            .read_line(&mut input)
            .map_err(|e| anyhow!("failed to read confirmation: {e}"))?;
        if input.trim() != "PURGE ALL" {
            eprintln!("aborted: confirmation phrase did not match");
            std::process::exit(2);
        }
    }

    let mut any_errors = false;
    for plan in &plans {
        if !dry_run {
            notify_amq_peers_of_purge(&plan.branch);
        }
        let report = execute(plan, &storage, paths, dry_run)?;
        eprint!("{}", report.summary());
        any_errors |= report.had_errors();
    }
    if any_errors {
        std::process::exit(1);
    }
    Ok(())
}

fn format_plan(plan: &PurgePlan, dry_run: bool) -> String {
    let header = if dry_run {
        format!(
            "DRY-RUN purge plan for session {} (branch {}):",
            plan.session_id, plan.branch
        )
    } else {
        format!(
            "purge plan for session {} (branch {}):",
            plan.session_id, plan.branch
        )
    };
    let mut s = String::new();
    s.push_str(&header);
    s.push('\n');
    for item in &plan.items {
        s.push_str("  - ");
        s.push_str(&item.describe());
        s.push('\n');
    }
    s
}

// Re-export PurgeReport so external tests can inspect outcomes via
// `dux::cli` if useful. The struct itself lives in `crate::purge`.
#[allow(dead_code)]
type _PurgeReportPubReexport = PurgeReport;
#[allow(dead_code)]
fn _purge_used_re_exports() {
    // Touch every imported symbol so a future refactor that drops one
    // gets a compiler nag instead of a silent dead-code warning.
    let _ = purge::PurgeOutcome::Done;
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
// dux doctor — Audit02 Phase 20 (P2-11)
// ---------------------------------------------------------------------------
//
// The bulk of the diagnostic logic lives in `dux-amq/scripts/dux-amq-doctor`
// (a bash script — installed by `dux-amq/install.sh`). The Rust wrapper
// here exists for two reasons:
//
//   1. Single entry point. `dux doctor` is more discoverable than a
//      separately-named binary; users don't have to remember "the
//      script lives next to dux on $PATH".
//   2. Sqlite integrity. The bash script optionally shells out to
//      `sqlite3`; on hosts without it, we still need to surface an
//      integrity result. The Rust side opens the live `SessionStore`
//      and runs `PRAGMA integrity_check` directly — no external CLI
//      required, and we can also count active sessions correctly using
//      the same model code the TUI uses.
//
// We *append* the Rust-side block after the bash output (rather than
// merging — that would require parsing JSON and stitching, which adds
// complexity for no operator benefit). If `dux-amq-doctor` isn't on
// PATH we still print the Rust-side block on its own.

/// Resolve the bash doctor script. Search order:
///   1. `DUX_AMQ_DOCTOR_BIN` env var (test override + dev iteration).
///   2. `dux-amq-doctor` on `$PATH` (the install.sh-installed copy).
///   3. `<exe-dir>/../dux-amq/scripts/dux-amq-doctor` (running from
///      a `cargo run` build tree).
///
/// Returns `None` if no candidate exists; the caller falls back to a
/// Rust-only diagnostic dump.
fn resolve_doctor_script() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("DUX_AMQ_DOCTOR_BIN") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }
    if let Ok(out) = Command::new("which").arg("dux-amq-doctor").output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                return Some(PathBuf::from(s));
            }
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("../dux-amq/scripts/dux-amq-doctor");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Top-level entry point for the `dux doctor` subcommand.
pub fn run_doctor(paths: &DuxPaths, json: bool, anonymize: bool) -> Result<()> {
    // 1. Run the bash script, if available, and forward its output
    //    verbatim. We don't capture-and-reprint: the script's coloring
    //    relies on it owning stdout (it auto-disables when `! -t 1`).
    let bash_ran = if let Some(script) = resolve_doctor_script() {
        let mut cmd = Command::new(&script);
        if json {
            cmd.arg("--json");
        }
        if anonymize {
            cmd.arg("--anonymize");
        }
        match cmd.status() {
            Ok(status) if status.success() => true,
            Ok(status) => {
                eprintln!(
                    "warning: dux-amq-doctor exited with {} — falling back to Rust-only output",
                    status.code().unwrap_or(-1),
                );
                false
            }
            Err(err) => {
                eprintln!("warning: failed to spawn {}: {err}", script.display());
                false
            }
        }
    } else {
        false
    };

    // 2. Append the Rust-side section. In JSON mode we emit a *second*
    //    JSON object on its own line; consumers that want a single
    //    object can `jq -s 'add'`. We deliberately don't try to
    //    re-parse and merge the bash output — that's brittle and the
    //    operator-facing use case (eyeballing a dump) doesn't need it.
    if json {
        emit_rust_section_json(paths, bash_ran)?;
    } else {
        emit_rust_section_text(paths, bash_ran)?;
    }
    Ok(())
}

fn emit_rust_section_text(paths: &DuxPaths, bash_ran: bool) -> Result<()> {
    if !bash_ran {
        println!(
            "(dux-amq-doctor not on PATH; emitting Rust-only diagnostics. \
             Re-run `dux-amq/install.sh` to install the full triage tool.)"
        );
    }
    println!("\n== Sessions DB (Rust-side) ==");
    let snap = collect_sessions_snapshot(paths);
    println!("{:<16} {}", "path:", paths.sessions_db_path.display());
    println!("{:<16} {}", "integrity:", snap.integrity);
    println!("{:<16} {}", "active:", snap.active);
    println!("{:<16} {}", "detached:", snap.detached);
    println!("{:<16} {}", "exited:", snap.exited);
    println!("{:<16} {}", "orphaned:", snap.orphaned_worktrees);
    Ok(())
}

fn emit_rust_section_json(paths: &DuxPaths, _bash_ran: bool) -> Result<()> {
    let snap = collect_sessions_snapshot(paths);
    // Hand-roll the JSON to avoid pulling in serde_json::json! macro
    // chains. The string fields are integrity status names that come
    // from a bounded set we control, so manual escaping is safe; if
    // that ever changes we should switch to `serde_json::to_string`.
    let body = serde_json::json!({
        "sessions_db_rust": {
            "path": paths.sessions_db_path.display().to_string(),
            "integrity": snap.integrity,
            "active": snap.active,
            "detached": snap.detached,
            "exited": snap.exited,
            "orphaned_worktrees": snap.orphaned_worktrees,
        }
    });
    println!("{body}");
    Ok(())
}

/// Lightweight snapshot of the sessions DB used only by `dux doctor`.
/// Counts are computed once per call; this is a cold-path operator tool
/// so we don't bother caching.
struct SessionsSnapshot {
    integrity: String,
    active: usize,
    detached: usize,
    exited: usize,
    /// Sessions whose `worktree_path` no longer exists on disk. A
    /// non-zero count is operator-visible: dux's session-cleanup code
    /// should have GC'd these.
    orphaned_worktrees: usize,
}

fn collect_sessions_snapshot(paths: &DuxPaths) -> SessionsSnapshot {
    if !paths.sessions_db_path.exists() {
        return SessionsSnapshot {
            integrity: "absent".to_string(),
            active: 0,
            detached: 0,
            exited: 0,
            orphaned_worktrees: 0,
        };
    }
    // SessionStore::open already runs PRAGMA integrity_check and
    // bails on failure; treat that as the authoritative result.
    match SessionStore::open(&paths.sessions_db_path) {
        Ok(store) => {
            let sessions = store.load_sessions().unwrap_or_default();
            let mut active = 0usize;
            let mut detached = 0usize;
            let mut exited = 0usize;
            let mut orphaned = 0usize;
            for s in &sessions {
                match s.status {
                    crate::model::SessionStatus::Active => active += 1,
                    crate::model::SessionStatus::Detached => detached += 1,
                    crate::model::SessionStatus::Exited => exited += 1,
                }
                if !Path::new(&s.worktree_path).exists() {
                    orphaned += 1;
                }
            }
            SessionsSnapshot {
                integrity: "ok".to_string(),
                active,
                detached,
                exited,
                orphaned_worktrees: orphaned,
            }
        }
        Err(err) => SessionsSnapshot {
            integrity: format!("error: {err}"),
            active: 0,
            detached: 0,
            exited: 0,
            orphaned_worktrees: 0,
        },
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
    fn doctor_snapshot_handles_missing_db() {
        let tempdir = TempDir::new().expect("tempdir");
        let root = tempdir.path().to_path_buf();
        let paths = DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"), // does not exist
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            root,
        };
        let snap = collect_sessions_snapshot(&paths);
        assert_eq!(snap.integrity, "absent");
        assert_eq!(snap.active, 0);
        assert_eq!(snap.orphaned_worktrees, 0);
    }

    #[test]
    fn doctor_snapshot_counts_by_status_and_orphans() {
        let tempdir = TempDir::new().expect("tempdir");
        let root = tempdir.path().to_path_buf();
        let worktrees_root = root.join("worktrees");
        std::fs::create_dir_all(&worktrees_root).expect("worktrees");
        let paths = DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: worktrees_root.clone(),
            lock_path: root.join("dux.lock"),
            root,
        };

        // Two healthy worktrees on disk; one orphaned (DB row points at
        // a path that doesn't exist).
        let live = worktrees_root.join("live-1");
        std::fs::create_dir_all(&live).expect("live");
        let live_two = worktrees_root.join("live-2");
        std::fs::create_dir_all(&live_two).expect("live2");
        let store = SessionStore::open(&paths.sessions_db_path).expect("store");
        let now = Utc::now();
        let mk = |id: &str, wt: &str, status: SessionStatus| AgentSession {
            id: id.to_string(),
            project_id: "p".to_string(),
            project_path: None,
            provider: ProviderKind::new("claude"),
            source_branch: "main".to_string(),
            branch_name: format!("b-{id}"),
            worktree_path: wt.to_string(),
            title: None,
            started_providers: Vec::new(),
            status,
            created_at: now,
            updated_at: now,
        };
        store
            .upsert_session(&mk("a1", live.to_str().unwrap(), SessionStatus::Active))
            .unwrap();
        store
            .upsert_session(&mk("a2", live.to_str().unwrap(), SessionStatus::Active))
            .unwrap();
        store
            .upsert_session(&mk(
                "d1",
                live_two.to_str().unwrap(),
                SessionStatus::Detached,
            ))
            .unwrap();
        store
            .upsert_session(&mk("x1", "/no/such/path-xyz-orphan", SessionStatus::Exited))
            .unwrap();
        drop(store);

        let snap = collect_sessions_snapshot(&paths);
        assert_eq!(snap.integrity, "ok");
        assert_eq!(snap.active, 2);
        assert_eq!(snap.detached, 1);
        assert_eq!(snap.exited, 1);
        assert_eq!(snap.orphaned_worktrees, 1);
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
