//! One test process's run: its id, the heartbeat that says it is alive, and
//! the cleanup of what it (or a dead earlier run) left in Docker.
//!
//! Several runs may share one Docker daemon at once (agent worktrees run the
//! suite side by side), so nothing here ever touches another LIVE run's
//! containers, networks or images:
//!
//! - Every container and network a run creates carries [`RUN_LABEL`] with the
//!   run's own random id.
//! - The run holds an exclusive lock on `<cache>/dux-journeys/runs/<id>.lock`
//!   for as long as its process lives (the kernel drops it when the process
//!   dies, however it dies). That lock is the heartbeat: a run is dead exactly
//!   when its lock file exists and can be locked by somebody else.
//! - At setup, a run removes the containers and networks of DEAD runs only. A
//!   run whose lock file is missing is left alone: it belongs to another user,
//!   another machine or another container sharing the daemon, whose cache this
//!   run cannot see. Those are what the documented manual cleanup is for.
//! - On SIGINT, SIGTERM or SIGQUIT (a Ctrl-C) the run removes its own
//!   containers and networks, then dies the way the signal would have killed
//!   it. Every call tolerates a container that is already gone. A SIGKILL
//!   cannot be caught; the next run's sweep removes what that left.
//!
//! Assumptions: every run on this machine as this user resolves the same cache
//! directory (`$XDG_CACHE_HOME`, else `$HOME/.cache`), and that directory is on
//! a filesystem where `flock` works (any local one).

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// The label naming the run that created a container or network.
pub const RUN_LABEL: &str = "dux-journeys.run";

/// This user's cache directory for the suite (`~/.cache/dux-journeys`),
/// independent of `TMPDIR`.
pub fn cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .expect("neither XDG_CACHE_HOME nor HOME is set; the journeys need a cache directory");
    base.join("dux-journeys")
}

fn runs_dir() -> PathBuf {
    cache_dir().join("runs")
}

struct Run {
    id: String,
    // Held, never read: the open, locked file IS the heartbeat.
    _heartbeat: File,
}

static RUN: OnceLock<Run> = OnceLock::new();

/// This process's run id, starting the run (heartbeat and signal cleanup) the
/// first time it is asked for.
pub fn run_id() -> &'static str {
    &RUN.get_or_init(|| {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let dir = runs_dir();
        std::fs::create_dir_all(&dir)
            .unwrap_or_else(|err| panic!("create the run directory {}: {err}", dir.display()));
        let path = dir.join(format!("{id}.lock"));
        let heartbeat = File::create(&path)
            .unwrap_or_else(|err| panic!("create the run heartbeat {}: {err}", path.display()));
        heartbeat
            .lock()
            .unwrap_or_else(|err| panic!("lock the run heartbeat {}: {err}", path.display()));
        install_signal_cleanup(id.clone());
        Run {
            id,
            _heartbeat: heartbeat,
        }
    })
    .id
}

/// What a run's heartbeat says about it.
#[derive(Debug, PartialEq, Eq)]
pub enum Liveness {
    /// Its lock is held: the run's process is alive.
    Alive,
    /// Its lock file exists and is free: the run is over.
    Dead,
    /// No lock file this user can see: not ours to judge.
    Unknown,
}

/// Read the heartbeat at `path`.
pub fn liveness(path: &Path) -> Liveness {
    let Ok(file) = OpenOptions::new().read(true).open(path) else {
        return Liveness::Unknown;
    };
    match file.try_lock() {
        Ok(()) => Liveness::Dead,
        Err(TryLockError::WouldBlock) => Liveness::Alive,
        Err(TryLockError::Error(_)) => Liveness::Unknown,
    }
}

fn docker(args: &[&str]) -> std::process::Output {
    Command::new("docker")
        .args(args)
        .output()
        .expect("run docker (is Docker installed and running?)")
}

fn listed(args: &[&str]) -> Vec<String> {
    String::from_utf8_lossy(&docker(args).stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Remove every container, then every network, labelled with `run`. Errors are
/// ignored on purpose: a container may already be gone, which is the goal.
pub fn remove_run(run: &str) {
    let filter = format!("label={RUN_LABEL}={run}");
    for id in listed(&["ps", "-aq", "--filter", &filter]) {
        let _ = docker(&["rm", "-f", "-v", &id]);
    }
    for id in listed(&["network", "ls", "-q", "--filter", &filter]) {
        let _ = docker(&["network", "rm", &id]);
    }
}

/// Remove what dead runs left. Every heartbeat file in this user's run
/// directory whose run is [`Liveness::Dead`] has that run's containers and
/// networks removed, and then the file itself (a run that ended normally left
/// nothing but the file). Live runs, and runs with no heartbeat here, are left
/// alone.
pub fn sweep_dead_runs() {
    let Ok(entries) = std::fs::read_dir(runs_dir()) else {
        return;
    };
    let me = run_id();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(run) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".lock"))
            .map(str::to_string)
        else {
            continue;
        };
        if run == me || !run.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        if liveness(&path) == Liveness::Dead {
            remove_run(&run);
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// On SIGINT, SIGTERM or SIGQUIT: remove this run's containers and networks,
/// then let the signal do what it would have done.
fn install_signal_cleanup(run: String) {
    use signal_hook::consts::{SIGINT, SIGQUIT, SIGTERM};
    let mut signals = match signal_hook::iterator::Signals::new([SIGINT, SIGTERM, SIGQUIT]) {
        Ok(signals) => signals,
        Err(err) => {
            eprintln!(
                "dux-journeys: cannot watch for Ctrl-C ({err}); the next run cleans up instead"
            );
            return;
        }
    };
    std::thread::spawn(move || {
        if let Some(signal) = signals.forever().next() {
            eprintln!("dux-journeys: interrupted; removing this run's containers and networks");
            remove_run(&run);
            let _ = std::fs::remove_file(runs_dir().join(format!("{run}.lock")));
            let _ = signal_hook::low_level::emulate_default_handler(signal);
            std::process::exit(130);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dux-journeys-liveness-{}-{name}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("run.lock")
    }

    #[test]
    fn a_held_heartbeat_is_alive_and_a_released_one_is_dead() {
        let path = scratch("held");
        let held = File::create(&path).unwrap();
        held.lock().unwrap();
        assert_eq!(liveness(&path), Liveness::Alive);
        drop(held);
        assert_eq!(liveness(&path), Liveness::Dead);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_missing_heartbeat_is_nobody_this_run_may_judge() {
        let path = scratch("missing");
        assert_eq!(liveness(&path), Liveness::Unknown);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
