//! Engine lifecycle housekeeping shared by all surfaces: detecting and cleaning
//! up PTY child processes (agent providers and companion terminals) that have
//! exited. The TUI has its own richer exit handling (resume-fallback, UI focus);
//! this is the minimal headless-safe cleanup the web server's engine loop calls
//! each tick so exited agents/terminals don't linger in `providers` /
//! `companion_terminals` (and therefore the ViewModel).

use crate::model::SessionStatus;

use super::Engine;

/// Which kind of PTY was pruned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrunedPtyKind {
    Agent,
    Terminal,
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
            // Drop the activity stamp with the provider — without this, a
            // long-running server leaks one map entry per exited agent.
            self.pty_activity.remove(&session_id);
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
    /// agent. Stragglers are hard-killed when the PtyClients drop.
    pub fn shutdown_ptys(&mut self, grace: std::time::Duration) {
        for client in self.providers.values() {
            client.terminate();
        }
        for terminal in self.companion_terminals.values() {
            terminal.client.terminate();
        }
        let deadline = std::time::Instant::now() + grace;
        loop {
            let providers_done = self
                .providers
                .values_mut()
                .all(|c| c.is_exited() || c.try_wait().is_some());
            let terminals_done = self
                .companion_terminals
                .values_mut()
                .all(|t| t.client.is_exited() || t.client.try_wait().is_some());
            if (providers_done && terminals_done) || std::time::Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let ids: Vec<String> = self.providers.keys().cloned().collect();
        for id in ids {
            self.mark_session_status(&id, SessionStatus::Detached);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    use super::PrunedPtyKind;
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
        // The activity stamp must die with the provider — a long-running
        // server would otherwise leak one entry per exited agent.
        engine.pty_activity.insert("s1".to_string(), Instant::now());

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
}
