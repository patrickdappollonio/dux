//! Engine lifecycle housekeeping shared by all surfaces: detecting and cleaning
//! up PTY child processes (agent providers and companion terminals) that have
//! exited. The TUI has its own richer exit handling (resume-fallback, UI focus);
//! this is the minimal headless-safe cleanup the web server's engine loop calls
//! each tick so exited agents/terminals don't linger in `providers` /
//! `companion_terminals` (and therefore the ViewModel).

use std::time::Instant;

use crate::model::SessionStatus;
use crate::pty::PtyClient;

use super::Engine;

/// Which kind of PTY was pruned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrunedPtyKind {
    Agent,
    Terminal,
}

/// A request to remove an agent's worktree, deferred until that agent's PTY has
/// actually exited — so files are never deleted out from under a still-running
/// process (which would also risk git-lock failures). Carried on the agent's
/// [`TerminatingPty`] and dispatched by `reap_terminating_ptys` once the PTY is
/// reaped. `None` for terminals and for keep-worktree deletes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeferredWorktreeRemoval {
    pub session_id: String,
    pub project_path: String,
    pub worktree_path: String,
    pub branch_name: String,
    /// The Busy status message to show while the removal runs (set when the
    /// worker is finally spawned, after the PTY is reaped).
    pub busy_message: String,
}

/// A PTY that was SIGTERMed by an individual delete/close and is being given a
/// grace period to exit before being force-killed. Held (not dropped) because
/// `PtyClient::drop` hard-kills with no grace; `reap_terminating_ptys` drops it
/// once it exits or its deadline passes. This is the non-blocking, per-PTY
/// counterpart to `shutdown_ptys`'s whole-app blocking wait.
pub struct TerminatingPty {
    pub client: PtyClient,
    pub deadline: Instant,
    pub kind: PrunedPtyKind,
    /// Session id (agent) or terminal id (companion terminal).
    pub id: String,
    pub label: String,
    /// Deferred worktree removal to dispatch once this PTY is reaped (agent
    /// deletes with `delete_worktree` only).
    pub worktree_removal: Option<DeferredWorktreeRemoval>,
}

/// A PTY that `prune_exited_ptys` removed because its child process exited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrunedPty {
    pub kind: PrunedPtyKind,
    /// The session id (for an agent) or terminal id (for a companion terminal).
    pub id: String,
    /// A human-facing label: the session's branch name (agent) or the
    /// terminal's label (companion terminal).
    pub label: String,
}

/// Outcome of [`Engine::shutdown_ptys`], so a caller can echo the result to its
/// own surface (e.g. the TUI to its restored terminal) using the same pure
/// formatters this routine logs with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShutdownReport {
    pub agents_total: usize,
    pub terminals_total: usize,
    /// Agents that exited within the grace period (the rest were SIGKILLed).
    pub agents_exited: usize,
    /// Terminals that exited within the grace period (the rest were SIGKILLed).
    pub terminals_exited: usize,
    pub elapsed: std::time::Duration,
    /// True when at least one child had to be force-killed (SIGKILL) because it
    /// had not exited by the time the grace window was up — equivalently,
    /// `agents_exited < agents_total || terminals_exited < terminals_total`. With
    /// a grace of `0` the wait is skipped, so any not-yet-exited child sets this.
    pub timed_out: bool,
}

/// `"1 agent"` / `"2 agents"` — pluralize `word` for `n`.
fn pluralize(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// The line logged (and echoed by surfaces) when graceful shutdown begins.
pub fn format_shutdown_start(
    agents: usize,
    terminals: usize,
    grace: std::time::Duration,
) -> String {
    format!(
        "Requesting {} and {} to gracefully shut down, timeout {}s.",
        pluralize(agents, "agent"),
        pluralize(terminals, "terminal"),
        grace.as_secs()
    )
}

/// The final line logged (and echoed) when shutdown finishes: a clean notice
/// when every child exited within the grace period, otherwise the force-closing
/// notice naming how many exited cleanly versus how many are being killed.
pub fn format_shutdown_result(report: &ShutdownReport) -> String {
    if report.timed_out {
        // saturating_sub: exited <= total always holds for a report this engine
        // builds, but the struct is public and constructible, so never risk a
        // usize underflow that would print a giant number into the log.
        let remaining_agents = report.agents_total.saturating_sub(report.agents_exited);
        let remaining_terminals = report
            .terminals_total
            .saturating_sub(report.terminals_exited);
        format!(
            "{} and {} exited successfully. Force-closing {} and {}, then exiting...",
            pluralize(report.agents_exited, "agent"),
            pluralize(report.terminals_exited, "terminal"),
            pluralize(remaining_agents, "agent"),
            pluralize(remaining_terminals, "terminal"),
        )
    } else {
        format!(
            "All {} and {} exited gracefully in {:.1}s.",
            pluralize(report.agents_total, "agent"),
            pluralize(report.terminals_total, "terminal"),
            report.elapsed.as_secs_f64()
        )
    }
}

impl Engine {
    /// Detect agent providers and companion terminals whose child PTY has exited,
    /// remove them from the engine, mark exited agents' sessions `Detached`, and
    /// return what was pruned (so callers can surface a status). Pure engine
    /// state mutation — no UI, no network. Safe to call every tick.
    pub fn prune_exited_ptys(&mut self) -> Vec<PrunedPty> {
        let mut pruned = Vec::new();

        // Agent providers (keyed by session id). Capture each exited client's
        // exit-success so a clean exit can clear `desired_running` (matching the
        // TUI), which keeps a deliberately-exited agent from auto-reopening.
        let exited_agents: Vec<(String, Option<bool>)> = self
            .providers
            .iter_mut()
            .filter_map(|(id, client)| {
                let exit_success = client.try_wait().map(|status| status.success());
                if exit_success.is_some() || client.is_exited() {
                    Some((id.clone(), exit_success))
                } else {
                    None
                }
            })
            .collect();
        for (session_id, exit_success) in exited_agents {
            self.providers.remove(&session_id);
            // Drop the activity and input stamps with the provider — without
            // this, a long-running server leaks one map entry per exited agent.
            self.pty_activity.remove(&session_id);
            self.pty_input.remove(&session_id);
            let label = self
                .sessions
                .iter()
                .find(|s| s.id == session_id)
                .map(|s| s.branch_name.clone())
                .unwrap_or_else(|| session_id.clone());
            if exit_success == Some(true) {
                self.mark_session_desired_running(&session_id, false);
            }
            self.mark_session_status(&session_id, SessionStatus::Detached);
            pruned.push(PrunedPty {
                kind: PrunedPtyKind::Agent,
                id: session_id,
                label,
            });
        }

        // Companion terminals (keyed by terminal id).
        let exited_terminals: Vec<(String, String)> = self
            .companion_terminals
            .iter_mut()
            .filter_map(|(id, terminal)| {
                if terminal.client.is_exited() || terminal.client.try_wait().is_some() {
                    Some((id.clone(), terminal.label.clone()))
                } else {
                    None
                }
            })
            .collect();
        for (terminal_id, label) in exited_terminals {
            self.companion_terminals.remove(&terminal_id);
            pruned.push(PrunedPty {
                kind: PrunedPtyKind::Terminal,
                id: terminal_id,
                label,
            });
        }

        pruned
    }

    /// The grace `Duration` an individual delete/close gives a child to exit
    /// before the background reaper force-kills it. Uses the global top-level
    /// `shutdown_timeout_seconds` (engine-wide; the close/delete handlers are
    /// shared by both surfaces and cannot tell TUI from web). Background, so the
    /// value only bounds force-kill latency, never blocks the UI.
    fn individual_close_grace(&self) -> std::time::Duration {
        crate::config::shutdown_grace(self.config.shutdown_timeout_seconds)
    }

    /// SIGTERM a companion terminal and move it into the terminating set for a
    /// non-blocking background reap, instead of dropping it from the map (which
    /// would hard-SIGKILL via `PtyClient::drop`). Returns the terminal's label,
    /// or `None` if it was not found.
    pub fn begin_close_companion_terminal(&mut self, terminal_id: &str) -> Option<String> {
        let term = self.companion_terminals.remove(terminal_id)?;
        let label = term.label.clone();
        term.client.terminate();
        let deadline = Instant::now() + self.individual_close_grace();
        self.terminating_ptys.push(TerminatingPty {
            client: term.client,
            deadline,
            kind: PrunedPtyKind::Terminal,
            id: terminal_id.to_string(),
            label: label.clone(),
            worktree_removal: None,
        });
        Some(label)
    }

    /// SIGTERM an agent provider and move it into the terminating set for a
    /// background reap. `label` is kept for the reap log; `worktree_removal` is
    /// dispatched once the PTY is reaped (agent delete with `delete_worktree`).
    ///
    /// Returns the `worktree_removal` back **unhandled** when the session has no
    /// live provider (the agent already exited or never started): there is no PTY
    /// to wait for, so the caller must dispatch the removal immediately rather
    /// than let it be lost. Returns `None` when it was captured on a terminating
    /// entry (or there was nothing to remove).
    #[must_use]
    pub fn begin_close_provider(
        &mut self,
        session_id: &str,
        label: String,
        worktree_removal: Option<DeferredWorktreeRemoval>,
    ) -> Option<DeferredWorktreeRemoval> {
        let Some(client) = self.providers.remove(session_id) else {
            return worktree_removal;
        };
        client.terminate();
        let deadline = Instant::now() + self.individual_close_grace();
        self.terminating_ptys.push(TerminatingPty {
            client,
            deadline,
            kind: PrunedPtyKind::Agent,
            id: session_id.to_string(),
            label,
            worktree_removal,
        });
        None
    }

    /// SIGTERM every companion terminal belonging to a session and move them all
    /// into the terminating set (used when the owning agent is deleted).
    pub fn begin_close_session_terminals(&mut self, session_id: &str) {
        let ids: Vec<String> = self
            .companion_terminals
            .iter()
            .filter(|(_, t)| t.session_id == session_id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.begin_close_companion_terminal(&id);
        }
    }

    /// Drop every terminating PTY that has exited, and force-kill (then drop) any
    /// whose grace deadline has passed. Called once per engine tick on both
    /// surfaces (alongside `prune_exited_ptys`). Returns the deferred worktree
    /// removals for any reaped agents so the caller can dispatch them; logs each
    /// reap at debug. A no-op when nothing is terminating.
    pub fn reap_terminating_ptys(&mut self) -> Vec<DeferredWorktreeRemoval> {
        if self.terminating_ptys.is_empty() {
            return Vec::new();
        }
        let now = Instant::now();
        let mut dispatch = Vec::new();
        let mut remaining = Vec::with_capacity(self.terminating_ptys.len());
        for mut entry in std::mem::take(&mut self.terminating_ptys) {
            let exited = entry.client.is_exited() || entry.client.try_wait().is_some();
            if exited {
                crate::logger::debug(&format!(
                    "reaped terminating {:?} {} (\"{}\") after a clean exit",
                    entry.kind, entry.id, entry.label
                ));
            } else if now >= entry.deadline {
                entry.client.force_terminate();
                crate::logger::debug(&format!(
                    "force-killed terminating {:?} {} (\"{}\") after the grace period elapsed",
                    entry.kind, entry.id, entry.label
                ));
            } else {
                remaining.push(entry);
                continue;
            }
            // Reaped: hand back any deferred worktree removal, then drop the
            // client (its `Drop` SIGKILL is a benign no-op now — already gone).
            if let Some(req) = entry.worktree_removal.take() {
                dispatch.push(req);
            }
        }
        self.terminating_ptys = remaining;
        dispatch
    }

    /// Boot-time normalization of persisted session statuses (the headless
    /// counterpart of the TUI's `restore_sessions`): nothing is running yet, so
    /// a session whose worktree still exists is `Detached`; one whose worktree
    /// vanished is `Exited`. Statuses persist via `mark_session_status`. Unlike
    /// the TUI this does not auto-reopen anything — the web resumes on subscribe.
    pub fn normalize_restored_sessions(&mut self) {
        let ids: Vec<(String, bool)> = self
            .sessions
            .iter()
            .map(|s| {
                (
                    s.id.clone(),
                    std::path::Path::new(&s.worktree_path).exists(),
                )
            })
            .collect();
        for (id, exists) in ids {
            let status = if exists {
                SessionStatus::Detached
            } else {
                SessionStatus::Exited
            };
            self.mark_session_status(&id, status);
        }
    }

    /// Gracefully wind down every running PTY for server shutdown: SIGTERM each
    /// child (agents save state for a later resume), wait up to `grace` for
    /// exits, and mark agent sessions Detached (persisted). `desired_running`
    /// is left untouched — a server shutdown is not the user stopping the
    /// agent. Any child still alive when `grace` elapses is force-killed
    /// (SIGKILL) on the spot so the logged result is truthful; `PtyClient::drop`
    /// remains the backstop. Logs a start and a result line to `dux.log` and
    /// returns a [`ShutdownReport`] so callers can echo the same lines to their
    /// own surface. A grace of `0` skips the wait and force-kills immediately.
    /// With nothing running, it is a silent no-op (no signals, no logs).
    pub fn shutdown_ptys(&mut self, grace: std::time::Duration) -> ShutdownReport {
        self.shutdown_ptys_interruptible(grace, None)
    }

    /// Like [`shutdown_ptys`](Self::shutdown_ptys), but the grace wait also ends
    /// early if `abort` flips to `true` — the second-signal escape hatch: a quit
    /// already in its (possibly long) wait can be cut short by another SIGINT/
    /// SIGTERM so a child that ignores SIGTERM cannot trap the operator. On an
    /// abort the surviving children are force-killed immediately, exactly as on a
    /// deadline timeout.
    pub fn shutdown_ptys_interruptible(
        &mut self,
        grace: std::time::Duration,
        abort: Option<&std::sync::atomic::AtomicBool>,
    ) -> ShutdownReport {
        let agents_total = self.providers.len();
        let terminals_total = self.companion_terminals.len();

        if agents_total == 0 && terminals_total == 0 {
            return ShutdownReport {
                agents_total: 0,
                terminals_total: 0,
                agents_exited: 0,
                terminals_exited: 0,
                elapsed: std::time::Duration::ZERO,
                timed_out: false,
            };
        }

        crate::logger::info(&format_shutdown_start(agents_total, terminals_total, grace));

        for client in self.providers.values() {
            client.terminate();
        }
        for terminal in self.companion_terminals.values() {
            terminal.client.terminate();
        }

        let aborted = || abort.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst));
        let start = std::time::Instant::now();
        let deadline = start + grace;
        // grace == 0 means "force immediately": SIGTERM was still sent above, but
        // we skip the wait loop and go straight to the force-kill tally below. An
        // `abort` flip (a second termination signal) also ends the wait early.
        if !grace.is_zero() && !aborted() {
            loop {
                let providers_done = self
                    .providers
                    .values_mut()
                    .all(|c| c.is_exited() || c.try_wait().is_some());
                let terminals_done = self
                    .companion_terminals
                    .values_mut()
                    .all(|t| t.client.is_exited() || t.client.try_wait().is_some());
                if (providers_done && terminals_done)
                    || std::time::Instant::now() >= deadline
                    || aborted()
                {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }

        // Tally how many exited cleanly and SIGKILL any survivor now, so the
        // result line reflects reality at the moment it is logged rather than
        // deferring every straggler to `Drop`.
        let mut agents_exited = 0usize;
        for client in self.providers.values_mut() {
            if client.is_exited() || client.try_wait().is_some() {
                agents_exited += 1;
            } else {
                client.force_terminate();
            }
        }
        let mut terminals_exited = 0usize;
        for terminal in self.companion_terminals.values_mut() {
            if terminal.client.is_exited() || terminal.client.try_wait().is_some() {
                terminals_exited += 1;
            } else {
                terminal.client.force_terminate();
            }
        }

        let report = ShutdownReport {
            agents_total,
            terminals_total,
            agents_exited,
            terminals_exited,
            elapsed: start.elapsed(),
            timed_out: agents_exited < agents_total || terminals_exited < terminals_total,
        };
        crate::logger::info(&format_shutdown_result(&report));

        let ids: Vec<String> = self.providers.keys().cloned().collect();
        for id in ids {
            self.mark_session_status(&id, SessionStatus::Detached);
        }

        report
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    use super::PrunedPtyKind;
    use super::TerminatingPty;
    use super::{format_shutdown_result, format_shutdown_start};
    use crate::engine::test_support::{sample_project, sample_session, test_engine};
    use crate::model::SessionStatus;
    use crate::pty::PtyClient;

    /// Spawn a real `cat`-backed PtyClient in the given working directory.
    /// `cat` echoes stdin and exits 0 on EOF, and exits on SIGTERM — making it
    /// a safe stand-in for both clean-exit and shutdown tests.
    fn spawn_cat(cwd: &Path) -> PtyClient {
        PtyClient::spawn_with_env("cat", &[], cwd, 24, 80, 1000, &[]).expect("spawn cat")
    }

    #[test]
    fn prune_removes_exited_companion_terminal() {
        let (mut engine, _tmp) = test_engine();

        // A real worktree directory the PTY can `cwd` into.
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session.worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);

        // `cat` echoes stdin and exits on EOF — a safe stand-in terminal.
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];

        let (terminal_id, _label) = engine
            .create_companion_terminal("s1")
            .expect("create companion terminal");
        assert_eq!(terminal_id, "term-1");
        assert!(engine.companion_terminals.contains_key("term-1"));

        // Ctrl-D (EOF) in canonical mode causes `cat` to exit.
        engine
            .companion_terminals
            .get("term-1")
            .unwrap()
            .client
            .write_bytes(b"\x04")
            .unwrap();

        // Poll until the prune detects the exit (or the terminal is gone).
        let deadline = Instant::now() + Duration::from_secs(3);
        let pruned = loop {
            let pruned = engine.prune_exited_ptys();
            if !pruned.is_empty() || !engine.companion_terminals.contains_key("term-1") {
                break pruned;
            }
            assert!(
                Instant::now() < deadline,
                "companion terminal never reported exit"
            );
            sleep(Duration::from_millis(50));
        };

        assert!(
            pruned
                .iter()
                .any(|p| p.kind == PrunedPtyKind::Terminal && p.id == "term-1"),
            "expected a pruned terminal entry for term-1, got {pruned:?}"
        );
        assert!(
            !engine.companion_terminals.contains_key("term-1"),
            "term-1 should have been removed from companion_terminals"
        );
    }

    #[test]
    fn prune_clears_desired_running_on_clean_exit() {
        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session.worktree_path = worktree.path().to_string_lossy().to_string();
        session.desired_running = true;
        // Store the session so `mark_*` persists cleanly.
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        // A clean-exiting agent provider (cat exits 0 on EOF).
        let client = spawn_cat(worktree.path());
        engine.providers.insert("s1".to_string(), client);
        // The activity and input stamps must die with the provider — a
        // long-running server would otherwise leak one entry per exited agent.
        engine.pty_activity.insert("s1".to_string(), Instant::now());
        engine.pty_input.insert("s1".to_string(), Instant::now());

        // Ctrl-D (EOF) makes cat exit with status 0.
        engine
            .providers
            .get_mut("s1")
            .unwrap()
            .write_bytes(b"\x04")
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let pruned = engine.prune_exited_ptys();
            if pruned.iter().any(|p| p.id == "s1") {
                break;
            }
            assert!(Instant::now() < deadline, "agent provider never exited");
            sleep(Duration::from_millis(50));
        }

        let session = engine.sessions.iter().find(|s| s.id == "s1").unwrap();
        assert!(
            !session.desired_running,
            "a clean exit should clear desired_running"
        );
        assert_eq!(session.status, SessionStatus::Detached);
        assert!(
            !engine.pty_activity.contains_key("s1"),
            "pruning an exited agent must clear its activity stamp"
        );
        assert!(
            !engine.pty_input.contains_key("s1"),
            "pruning an exited agent must clear its input stamp"
        );
    }

    #[test]
    fn prune_keeps_desired_running_on_nonclean_exit() {
        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session.worktree_path = worktree.path().to_string_lossy().to_string();
        session.desired_running = true;
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        // A provider that exits non-zero immediately.
        let client = PtyClient::spawn_with_env(
            "sh",
            &["-c".to_string(), "exit 1".to_string()],
            worktree.path(),
            24,
            80,
            1000,
            &[],
        )
        .expect("spawn sh");
        engine.providers.insert("s1".to_string(), client);

        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let pruned = engine.prune_exited_ptys();
            if pruned.iter().any(|p| p.id == "s1") {
                break;
            }
            assert!(Instant::now() < deadline, "agent provider never exited");
            sleep(Duration::from_millis(50));
        }

        let session = engine.sessions.iter().find(|s| s.id == "s1").unwrap();
        assert!(
            session.desired_running,
            "a non-clean exit should leave desired_running set"
        );
        assert_eq!(session.status, SessionStatus::Detached);
    }

    #[test]
    fn normalize_restored_sessions_marks_detached_and_exited() {
        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));

        let mut present = sample_session("present", "p1", "here");
        present.worktree_path = worktree.path().to_string_lossy().to_string();
        present.status = SessionStatus::Active;
        engine.session_store.upsert_session(&present).unwrap();
        engine.sessions.push(present);

        let mut gone = sample_session("gone", "p1", "gone");
        gone.worktree_path = worktree
            .path()
            .join("does-not-exist")
            .to_string_lossy()
            .to_string();
        gone.status = SessionStatus::Active;
        engine.session_store.upsert_session(&gone).unwrap();
        engine.sessions.push(gone);

        engine.normalize_restored_sessions();

        let present = engine.sessions.iter().find(|s| s.id == "present").unwrap();
        assert_eq!(present.status, SessionStatus::Detached);
        let gone = engine.sessions.iter().find(|s| s.id == "gone").unwrap();
        assert_eq!(gone.status, SessionStatus::Exited);
    }

    #[test]
    fn shutdown_ptys_terminates_children() {
        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session.worktree_path = worktree.path().to_string_lossy().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        // A provider that does not exit on its own — it must be SIGTERMed.
        let client = spawn_cat(worktree.path());
        engine.providers.insert("s1".to_string(), client);

        engine.shutdown_ptys(Duration::from_secs(2));

        let client = engine.providers.get_mut("s1").unwrap();
        assert!(
            client.is_exited() || client.try_wait().is_some(),
            "cat should have exited after SIGTERM"
        );
        let session = engine.sessions.iter().find(|s| s.id == "s1").unwrap();
        assert_eq!(session.status, SessionStatus::Detached);
    }

    #[test]
    fn shutdown_ptys_terminates_companion_terminal() {
        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session.worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);

        // A `cat`-backed companion terminal that won't exit on its own; it must
        // be SIGTERMed by shutdown_ptys, just like an agent provider.
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];
        let (terminal_id, _label) = engine
            .create_companion_terminal("s1")
            .expect("create companion terminal");
        assert!(engine.companion_terminals.contains_key(&terminal_id));

        engine.shutdown_ptys(Duration::from_secs(2));

        let terminal = engine.companion_terminals.get_mut(&terminal_id).unwrap();
        assert!(
            terminal.client.is_exited() || terminal.client.try_wait().is_some(),
            "the companion terminal's cat should have exited after SIGTERM"
        );
    }

    /// A child that ignores SIGTERM (so it must be SIGKILLed) and never exits on
    /// its own. The `trap` makes the shell ignore TERM; the `echo` then emits a
    /// marker AFTER the trap is installed, so a caller can poll `has_output()` to
    /// know the trap is live before signalling — otherwise a SIGTERM that lands
    /// during shell startup (before `trap` runs) would kill it by default and the
    /// test would not exercise the force-kill path. The busy loop keeps it alive.
    fn spawn_sigterm_ignorer(cwd: &Path) -> PtyClient {
        PtyClient::spawn_with_env(
            "sh",
            &[
                "-c".to_string(),
                "trap '' TERM; echo ready; while true; do :; done".to_string(),
            ],
            cwd,
            24,
            80,
            1000,
            &[],
        )
        .expect("spawn sigterm-ignorer")
    }

    /// Block until the SIGTERM-ignorer has printed its readiness marker (proof
    /// the `trap` is installed) or a timeout elapses.
    fn wait_until_ready(engine: &crate::engine::Engine, id: &str) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !engine
            .providers
            .get(id)
            .expect("provider present")
            .has_output()
        {
            assert!(
                Instant::now() < deadline,
                "sigterm-ignorer never signalled ready (trap not installed)"
            );
            sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn shutdown_ptys_reports_clean_exit() {
        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session.worktree_path = worktree.path().to_string_lossy().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        // `cat` exits promptly on SIGTERM, so the grace window is never hit.
        engine
            .providers
            .insert("s1".to_string(), spawn_cat(worktree.path()));

        let report = engine.shutdown_ptys(Duration::from_secs(5));

        assert_eq!(report.agents_total, 1);
        assert_eq!(report.terminals_total, 0);
        assert_eq!(report.agents_exited, 1, "cat should exit on SIGTERM");
        assert!(
            !report.timed_out,
            "a SIGTERM-respecting child is not a timeout"
        );
        assert!(
            report.elapsed < Duration::from_secs(5),
            "a clean exit should return well before the deadline"
        );
    }

    #[test]
    fn shutdown_ptys_force_kills_stragglers_and_reports_timeout() {
        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session.worktree_path = worktree.path().to_string_lossy().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        // An agent that ignores SIGTERM: it must be force-killed at the deadline.
        engine
            .providers
            .insert("s1".to_string(), spawn_sigterm_ignorer(worktree.path()));
        // Ensure the trap is installed before signalling, so SIGTERM doesn't kill
        // the shell during startup and bypass the force-kill path under test.
        wait_until_ready(&engine, "s1");

        let report = engine.shutdown_ptys(Duration::from_millis(300));

        assert!(report.timed_out, "a SIGTERM-ignoring child must time out");
        assert_eq!(report.agents_total, 1);
        assert_eq!(
            report.agents_exited, 0,
            "the straggler had not exited cleanly"
        );
        assert!(
            report.elapsed >= Duration::from_millis(300),
            "a timeout must wait out the full grace period"
        );

        // force_terminate sent SIGKILL to the group; the child must now die.
        let client = engine.providers.get_mut("s1").unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !(client.is_exited() || client.try_wait().is_some()) {
            assert!(
                Instant::now() < deadline,
                "force_terminate's SIGKILL should have reaped the straggler"
            );
            sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn shutdown_ptys_interruptible_aborts_the_wait_early() {
        // The second-signal escape hatch: an abort flip during the grace wait cuts
        // it short and force-kills, instead of waiting out the full (here 30s)
        // timeout behind a SIGTERM-ignoring child.
        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session.worktree_path = worktree.path().to_string_lossy().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        engine
            .providers
            .insert("s1".to_string(), spawn_sigterm_ignorer(worktree.path()));
        wait_until_ready(&engine, "s1");

        // Flip the abort ~200ms into the (30s) wait from another thread.
        let abort = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let abort_setter = std::sync::Arc::clone(&abort);
        std::thread::spawn(move || {
            sleep(Duration::from_millis(200));
            abort_setter.store(true, std::sync::atomic::Ordering::SeqCst);
        });

        let report = engine.shutdown_ptys_interruptible(Duration::from_secs(30), Some(&abort));

        assert!(report.timed_out, "an aborted wait force-kills survivors");
        assert!(
            report.elapsed < Duration::from_secs(5),
            "the abort must cut the 30s wait short, took {:?}",
            report.elapsed
        );

        // The straggler is still reaped by the force-kill.
        let client = engine.providers.get_mut("s1").unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !(client.is_exited() || client.try_wait().is_some()) {
            assert!(
                Instant::now() < deadline,
                "the aborted shutdown should still SIGKILL the straggler"
            );
            sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn shutdown_ptys_no_children_is_a_silent_noop() {
        let (mut engine, _tmp) = test_engine();
        let report = engine.shutdown_ptys(Duration::from_secs(30));
        assert_eq!(report.agents_total, 0);
        assert_eq!(report.terminals_total, 0);
        assert!(!report.timed_out);
        assert_eq!(report.elapsed, Duration::ZERO);
    }

    #[test]
    fn shutdown_ptys_force_kills_straggler_terminal() {
        // The terminals tally/force-kill block is coded separately from the
        // agents one; exercise it directly so a copy-paste slip there is caught.
        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session.worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);

        // A companion terminal backed by the SIGTERM-ignorer instead of `cat`.
        engine.config.terminal.command = "sh".to_string();
        engine.config.terminal.args = vec![
            "-c".to_string(),
            "trap '' TERM; echo ready; while true; do :; done".to_string(),
        ];
        let (terminal_id, _label) = engine
            .create_companion_terminal("s1")
            .expect("create companion terminal");

        // Wait until the trap is installed (marker printed) before signalling.
        let deadline = Instant::now() + Duration::from_secs(3);
        while !engine
            .companion_terminals
            .get(&terminal_id)
            .unwrap()
            .client
            .has_output()
        {
            assert!(Instant::now() < deadline, "terminal ignorer never readied");
            sleep(Duration::from_millis(20));
        }

        let report = engine.shutdown_ptys(Duration::from_millis(300));

        assert!(
            report.timed_out,
            "a SIGTERM-ignoring terminal must time out"
        );
        assert_eq!(report.terminals_total, 1);
        assert_eq!(report.agents_total, 0);
        assert_eq!(
            report.terminals_exited, 0,
            "the straggler terminal had not exited cleanly"
        );

        // force_terminate's SIGKILL must reap it.
        let term = engine.companion_terminals.get_mut(&terminal_id).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !(term.client.is_exited() || term.client.try_wait().is_some()) {
            assert!(
                Instant::now() < deadline,
                "force_terminate should have reaped the straggler terminal"
            );
            sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn shutdown_ptys_mixed_clean_and_straggler() {
        // One agent exits cleanly on SIGTERM, the other ignores it: the report
        // must count them separately and still flag timed_out, proving the
        // wait-loop's `.all(...)` aggregation does not short-circuit on the first
        // exited child.
        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        for id in ["clean", "straggler"] {
            let mut session = sample_session(id, "p1", "feat");
            session.worktree_path = worktree.path().to_string_lossy().to_string();
            engine.session_store.upsert_session(&session).unwrap();
            engine.sessions.push(session);
        }

        engine
            .providers
            .insert("clean".to_string(), spawn_cat(worktree.path()));
        engine.providers.insert(
            "straggler".to_string(),
            spawn_sigterm_ignorer(worktree.path()),
        );
        wait_until_ready(&engine, "straggler");

        let report = engine.shutdown_ptys(Duration::from_millis(300));

        assert_eq!(report.agents_total, 2);
        assert_eq!(
            report.agents_exited, 1,
            "only the cat agent exits on SIGTERM"
        );
        assert!(report.timed_out, "the straggler forces a timeout");
        assert!(
            report.elapsed >= Duration::from_millis(300),
            "the loop must wait the full grace for the straggler, not stop early"
        );
    }

    #[test]
    fn shutdown_ptys_grace_zero_force_kills_without_waiting() {
        // grace == 0 means "force immediately": the wait loop is skipped, so a
        // straggler is SIGKILLed at once and reported timed_out, with near-zero
        // elapsed.
        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session.worktree_path = worktree.path().to_string_lossy().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        engine
            .providers
            .insert("s1".to_string(), spawn_sigterm_ignorer(worktree.path()));
        wait_until_ready(&engine, "s1");

        let report = engine.shutdown_ptys(Duration::ZERO);

        assert!(
            report.timed_out,
            "grace 0 with a live child is a forced close"
        );
        assert_eq!(report.agents_exited, 0);
        assert!(
            report.elapsed < Duration::from_millis(50),
            "grace 0 must not enter the 50ms poll loop, got {:?}",
            report.elapsed
        );

        // The child must still be reaped by the immediate SIGKILL.
        let client = engine.providers.get_mut("s1").unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !(client.is_exited() || client.try_wait().is_some()) {
            assert!(
                Instant::now() < deadline,
                "grace-0 force_terminate should have reaped the child"
            );
            sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn begin_close_companion_terminal_moves_to_terminating_and_reaps_on_exit() {
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session.worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];
        let (tid, _label) = engine
            .create_companion_terminal("s1")
            .expect("create companion terminal");
        assert!(engine.companion_terminals.contains_key(&tid));

        // Graceful close: out of the live map, into terminating, SIGTERM sent.
        let label = engine.begin_close_companion_terminal(&tid);
        assert!(label.is_some(), "returns the closed terminal's label");
        assert!(
            !engine.companion_terminals.contains_key(&tid),
            "the terminal leaves the live map immediately (UI updates now)"
        );
        assert_eq!(engine.terminating_ptys.len(), 1);
        assert_eq!(engine.terminating_ptys[0].kind, PrunedPtyKind::Terminal);
        assert!(engine.terminating_ptys[0].worktree_removal.is_none());

        // `cat` exits on SIGTERM, so the reaper drops it well before any deadline.
        let deadline = Instant::now() + Duration::from_secs(3);
        while !engine.terminating_ptys.is_empty() {
            let dispatched = engine.reap_terminating_ptys();
            assert!(dispatched.is_empty(), "a terminal has no deferred worktree");
            assert!(Instant::now() < deadline, "terminal was never reaped");
            sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn reap_force_kills_a_straggler_past_its_deadline() {
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        let client = spawn_sigterm_ignorer(worktree.path());
        // Wait until the trap is installed (marker printed) before relying on the
        // force-kill: a SIGTERM during shell startup would kill it for the wrong
        // reason and the test wouldn't exercise force_terminate.
        let ready_by = Instant::now() + Duration::from_secs(3);
        while !client.has_output() {
            assert!(Instant::now() < ready_by, "ignorer never readied");
            sleep(Duration::from_millis(20));
        }
        // Push it as already past its deadline, so one reap must force-kill it.
        engine.terminating_ptys.push(TerminatingPty {
            client,
            deadline: Instant::now() - Duration::from_millis(1),
            kind: PrunedPtyKind::Terminal,
            id: "t1".to_string(),
            label: "scratch".to_string(),
            worktree_removal: None,
        });

        let dispatched = engine.reap_terminating_ptys();
        assert!(dispatched.is_empty());
        assert!(
            engine.terminating_ptys.is_empty(),
            "a past-deadline straggler is force-killed and removed in one reap"
        );
    }

    #[test]
    fn reap_terminating_ptys_is_a_noop_when_empty() {
        let (mut engine, _tmp) = test_engine();
        assert!(engine.reap_terminating_ptys().is_empty());
    }

    #[test]
    fn begin_delete_session_gracefully_closes_agent_and_defers_worktree() {
        use crate::engine::BeginDeleteSessionOutcome;
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session.worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        // A live agent PTY that exits on SIGTERM.
        engine
            .providers
            .insert("s1".to_string(), spawn_cat(worktree.path()));

        let outcome = engine.begin_delete_session("s1", true);
        assert!(
            matches!(outcome, BeginDeleteSessionOutcome::AsyncStarted { .. }),
            "a worktree-removing delete returns the deferred (AsyncStarted) outcome"
        );
        // The agent PTY is gracefully closed: out of `providers`, into the
        // terminating set, with the worktree removal captured for after it exits.
        assert!(!engine.providers.contains_key("s1"));
        assert_eq!(engine.terminating_ptys.len(), 1);
        assert_eq!(engine.terminating_ptys[0].kind, PrunedPtyKind::Agent);
        let req = engine.terminating_ptys[0]
            .worktree_removal
            .as_ref()
            .expect("worktree removal deferred onto the terminating agent");
        assert_eq!(req.session_id, "s1");
        assert_eq!(req.worktree_path, worktree.path().to_string_lossy());

        // Once the agent exits (SIGTERM), the reaper hands the removal back to be
        // dispatched — never before.
        let deadline = Instant::now() + Duration::from_secs(3);
        let removals = loop {
            let r = engine.reap_terminating_ptys();
            if !r.is_empty() {
                break r;
            }
            assert!(Instant::now() < deadline, "agent never reaped");
            sleep(Duration::from_millis(20));
        };
        assert_eq!(removals.len(), 1);
        assert_eq!(removals[0].session_id, "s1");
    }

    #[test]
    fn begin_delete_session_removes_worktree_immediately_when_no_live_agent() {
        use crate::engine::BeginDeleteSessionOutcome;
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session.worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        // No provider inserted: the agent already exited.

        let outcome = engine.begin_delete_session("s1", true);
        assert!(matches!(
            outcome,
            BeginDeleteSessionOutcome::AsyncStarted { .. }
        ));
        // Nothing to reap (no PTY), and the removal was dispatched right away
        // rather than lost — the in-flight guard proves the worker was spawned.
        assert!(engine.terminating_ptys.is_empty());
        assert!(
            engine.pending_deletions.contains("s1"),
            "the worktree removal is dispatched immediately when there is no PTY"
        );
    }

    #[test]
    fn format_shutdown_start_pluralizes() {
        assert_eq!(
            format_shutdown_start(1, 1, Duration::from_secs(30)),
            "Requesting 1 agent and 1 terminal to gracefully shut down, timeout 30s."
        );
        assert_eq!(
            format_shutdown_start(2, 0, Duration::from_secs(5)),
            "Requesting 2 agents and 0 terminals to gracefully shut down, timeout 5s."
        );
    }

    #[test]
    fn format_shutdown_result_clean_and_forced() {
        let clean = super::ShutdownReport {
            agents_total: 2,
            terminals_total: 1,
            agents_exited: 2,
            terminals_exited: 1,
            elapsed: Duration::from_millis(340),
            timed_out: false,
        };
        assert_eq!(
            format_shutdown_result(&clean),
            "All 2 agents and 1 terminal exited gracefully in 0.3s."
        );

        let forced = super::ShutdownReport {
            agents_total: 3,
            terminals_total: 2,
            agents_exited: 1,
            terminals_exited: 2,
            elapsed: Duration::from_secs(30),
            timed_out: true,
        };
        assert_eq!(
            format_shutdown_result(&forced),
            "1 agent and 2 terminals exited successfully. \
             Force-closing 2 agents and 0 terminals, then exiting..."
        );
    }
}
