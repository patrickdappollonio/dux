//! Companion-terminal lifecycle on the headless `Engine`. Companion terminals are
//! plain PTYs distinct from agent providers: they have no launch/resume flow and
//! no provider semantics — they simply run the configured terminal command. A
//! terminal is owned by either an agent session (spawned in that agent's
//! worktree) or a project (a "project terminal", spawned at the project's repo
//! root with no agent attached). The TUI spawns session-owned terminals via
//! `App::spawn_companion_terminal_for_session`; this mirrors that flow for
//! headless callers (the web server) and adds the project-owned flavor.

use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::model::{CompanionTerminal, TerminalOwner};
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

        self.spawn_terminal(
            TerminalOwner::Session(session_id.to_string()),
            Path::new(&session.worktree_path),
            &env,
        )
    }

    /// Spawn a new project terminal at the given project's repo root and register
    /// it in `companion_terminals`. Returns the generated `(terminal_id, label)`.
    ///
    /// A project terminal is a plain shell with no agent attached: same terminal
    /// command, same resolved environment (global env merged with the project's
    /// env), owned by the project instead of a session. It deliberately does NOT
    /// run the project's `startup_command` (that is worktree provisioning for
    /// new agents, not a shell rc).
    pub fn create_project_terminal(&mut self, project_id: &str) -> Result<(String, String)> {
        let project = self
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .cloned()
            .context("unknown project")?;

        if !Path::new(&project.path).is_dir() {
            bail!(
                "the project's path \"{}\" does not exist on disk, so a project terminal cannot be opened there",
                project.path
            );
        }

        let env =
            crate::config::resolve_agent_env(&self.config.env, &project.env).unwrap_or_default();

        self.spawn_terminal(
            TerminalOwner::Project(project_id.to_string()),
            Path::new(&project.path),
            &env,
        )
    }

    /// Shared spawn for both owners: run the configured terminal command at
    /// `cwd` with `env` and register the PTY under a fresh `term-N` id.
    fn spawn_terminal(
        &mut self,
        owner: TerminalOwner,
        cwd: &Path,
        env: &[(String, String)],
    ) -> Result<(String, String)> {
        // A companion terminal is a plain shell, not an agent, so it opts out of
        // agent-signal tracking: its bytes are never scanned for OSC/bell
        // attention signals (which it does not consume) and it can never raise a
        // spurious attention flag.
        let client = PtyClient::spawn_with_env_opts(
            &self.config.terminal.command,
            &self.config.terminal.args,
            cwd,
            24,
            80,
            self.config.ui.agent_scrollback_lines,
            crate::pty::PtySpawnOptions {
                env,
                track_agent_signals: false,
                // A companion shell still gets the terminal identity so it sees the
                // same terminal an agent would.
                identity: &self.resolved_identity(),
            },
        )?;

        self.terminal_counter += 1;
        let terminal_id = format!("term-{}", self.terminal_counter);
        let label = format!("Terminal {}", self.terminal_counter);

        self.companion_terminals.insert(
            terminal_id.clone(),
            CompanionTerminal {
                owner,
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
    use crate::model::TerminalOwner;

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
        assert_eq!(terminal.owner, TerminalOwner::Session("s1".to_string()));
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

    #[test]
    fn create_project_terminal_spawns_at_project_root_with_project_owner() {
        let (mut engine, _tmp) = test_engine();

        // A real project directory the PTY can `cwd` into.
        let repo = tempfile::tempdir().expect("project dir");
        engine
            .projects
            .push(sample_project("p1", repo.path().to_string_lossy().as_ref()));

        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];

        let (terminal_id, label) = engine
            .create_project_terminal("p1")
            .expect("create project terminal");

        assert_eq!(terminal_id, "term-1");
        assert_eq!(label, "Terminal 1");

        let terminal = engine
            .companion_terminals
            .get(&terminal_id)
            .expect("terminal registered");
        assert_eq!(terminal.owner, TerminalOwner::Project("p1".to_string()));
        assert_eq!(terminal.label, "Terminal 1");
        assert!(terminal.foreground_cmd.is_none());
    }

    #[test]
    fn create_project_terminal_unknown_project_errors() {
        let (mut engine, _tmp) = test_engine();
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];

        let err = engine
            .create_project_terminal("missing")
            .expect_err("missing project should error");
        assert!(
            err.to_string().contains("unknown project"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn create_project_terminal_path_missing_errors() {
        let (mut engine, _tmp) = test_engine();
        engine
            .projects
            .push(sample_project("p1", "/definitely/not/a/real/path"));
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];

        let err = engine
            .create_project_terminal("p1")
            .expect_err("path-missing project should error");
        assert!(
            err.to_string().contains("does not exist on disk"),
            "unexpected error: {err}"
        );
        assert!(
            engine.companion_terminals.is_empty(),
            "no terminal should have been registered"
        );
    }
}
