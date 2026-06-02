//! Companion-terminal lifecycle on the headless `Engine`. Companion terminals are
//! plain PTYs spawned in a session's worktree, distinct from agent providers:
//! they have no launch/resume flow and no provider semantics — they simply run
//! the configured terminal command. The TUI spawns these via
//! `App::spawn_companion_terminal_for_session`; this mirrors that flow for
//! headless callers (the web server).

use std::path::Path;

use anyhow::{Context, Result};

use crate::model::CompanionTerminal;
use crate::pty::PtyClient;

use super::Engine;

impl Engine {
    /// Spawn a new companion terminal in the given session's worktree and register
    /// it in `companion_terminals`. Returns the generated `(terminal_id, label)`.
    ///
    /// The terminal runs `config.terminal.command`/`args` with the session's
    /// resolved environment (global env merged with the owning project's env).
    /// This is the headless equivalent of the TUI's
    /// `spawn_companion_terminal_for_session` + insert.
    pub fn create_companion_terminal(&mut self, session_id: &str) -> Result<(String, String)> {
        let session = self
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .cloned()
            .context("unknown session")?;

        let env = self
            .projects
            .iter()
            .find(|project| project.id == session.project_id)
            .and_then(|project| {
                crate::config::resolve_agent_env(&self.config.env, &project.env).ok()
            })
            .unwrap_or_default();

        let client = PtyClient::spawn_with_env(
            &self.config.terminal.command,
            &self.config.terminal.args,
            Path::new(&session.worktree_path),
            24,
            80,
            self.config.ui.agent_scrollback_lines,
            &env,
        )?;

        self.terminal_counter += 1;
        let terminal_id = format!("term-{}", self.terminal_counter);
        let label = format!("Terminal {}", self.terminal_counter);

        self.companion_terminals.insert(
            terminal_id.clone(),
            CompanionTerminal {
                session_id: session_id.to_string(),
                label: label.clone(),
                foreground_cmd: None,
                client,
            },
        );

        Ok((terminal_id, label))
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::test_support::{sample_project, sample_session, test_engine};

    #[test]
    fn create_companion_terminal_spawns_and_registers() {
        let (mut engine, _tmp) = test_engine();

        // A real worktree directory the PTY can `cwd` into.
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feature");
        session.worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);

        // `cat` is always on PATH and simply echoes — a safe stand-in terminal.
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];

        let (terminal_id, label) = engine
            .create_companion_terminal("s1")
            .expect("create companion terminal");

        assert_eq!(terminal_id, "term-1");
        assert_eq!(label, "Terminal 1");
        assert_eq!(engine.terminal_counter, 1);

        let terminal = engine
            .companion_terminals
            .get(&terminal_id)
            .expect("terminal registered");
        assert_eq!(terminal.session_id, "s1");
        assert_eq!(terminal.label, "Terminal 1");
        assert!(terminal.foreground_cmd.is_none());
    }

    #[test]
    fn create_companion_terminal_unknown_session_errors() {
        let (mut engine, _tmp) = test_engine();
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];

        let err = engine
            .create_companion_terminal("missing")
            .expect_err("missing session should error");
        assert!(
            err.to_string().contains("unknown session"),
            "unexpected error: {err}"
        );
    }
}
