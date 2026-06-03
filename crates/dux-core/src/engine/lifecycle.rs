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

        // Agent providers (keyed by session id).
        let exited_agents: Vec<String> = self
            .providers
            .iter_mut()
            .filter_map(|(id, client)| {
                if client.is_exited() || client.try_wait().is_some() {
                    Some(id.clone())
                } else {
                    None
                }
            })
            .collect();
        for session_id in exited_agents {
            self.providers.remove(&session_id);
            let label = self
                .sessions
                .iter()
                .find(|s| s.id == session_id)
                .map(|s| s.branch_name.clone())
                .unwrap_or_else(|| session_id.clone());
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
}

#[cfg(test)]
mod tests {
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    use super::PrunedPtyKind;
    use crate::engine::test_support::{sample_project, sample_session, test_engine};

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
}
