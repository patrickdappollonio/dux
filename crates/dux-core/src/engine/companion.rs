//! Companion-terminal lifecycle on the headless `Engine`. Companion terminals are
//! plain PTYs distinct from agent providers: they have no launch/resume flow and
//! no provider semantics; they simply run the configured terminal command. A
//! terminal is owned by an agent session (spawned in that agent's worktree), a
//! project (a "project terminal", spawned at the project's repo root with no
//! agent attached), or nothing at all (a "standalone terminal", spawned in the
//! user's home directory with neither). The TUI spawns session-owned terminals
//! via `App::spawn_companion_terminal_for_session`; this mirrors that flow for
//! headless callers (the web server) and adds the project-owned and standalone
//! flavors.

use std::path::{Path, PathBuf};

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
    pub fn create_companion_terminal(
        &mut self,
        session_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<(String, String)> {
        let session = self
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .cloned()
            .context("unknown session")?;

        // A standalone agent belongs to no project, so a terminal opened on it
        // gets the GLOBAL environment with no project overlay, exactly like a
        // standalone terminal. Reading a project id that is not there and
        // falling through to `unwrap_or_default` would silently hand it an
        // EMPTY environment instead, which is a different and much worse thing.
        let env = match session.project_id() {
            Some(project_id) => self
                .projects
                .iter()
                .find(|project| project.id == project_id)
                .and_then(|project| {
                    crate::config::resolve_agent_env(&self.config.env, &project.env).ok()
                })
                .unwrap_or_default(),
            None => crate::config::resolve_agent_env(
                &self.config.env,
                &std::collections::BTreeMap::new(),
            )
            .unwrap_or_default(),
        };

        self.spawn_terminal(
            TerminalOwner::Session(session_id.to_string()),
            Path::new(session.directory()),
            &env,
            rows,
            cols,
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
    pub fn create_project_terminal(
        &mut self,
        project_id: &str,
        rows: u16,
        cols: u16,
    ) -> Result<(String, String)> {
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
            rows,
            cols,
        )
    }

    /// Spawn a new standalone terminal in the user's home directory and register
    /// it in `companion_terminals`. Returns the generated `(terminal_id, label)`.
    ///
    /// A standalone terminal belongs to nothing: no agent, no project. So it
    /// takes its directory from [`crate::home_path::standalone_terminal_dir`]
    /// (the home directory, or `/` when that cannot be resolved) rather than
    /// from an owner's path, and it gets the GLOBAL environment with no project
    /// overlay, because there is no project to overlay it with. The two owned
    /// kinds above merge `config.env` with their project's `env`; this one has
    /// only the global half, and that is the whole difference.
    ///
    /// Like a project terminal it deliberately does NOT run any
    /// `startup_command`: that is worktree provisioning for new agents, not a
    /// shell rc, and a standalone terminal has no project to take one from.
    pub fn create_standalone_terminal(&mut self, rows: u16, cols: u16) -> Result<(String, String)> {
        let dir = crate::home_path::standalone_terminal_dir();
        // The global half only. `resolve_agent_env` merges a project's env over
        // the global one; passing an empty map is exactly "there is no project".
        let env =
            crate::config::resolve_agent_env(&self.config.env, &std::collections::BTreeMap::new())
                .unwrap_or_default();

        self.spawn_terminal(TerminalOwner::Standalone, &dir, &env, rows, cols)
    }

    /// Shared spawn for every owner: run the configured terminal command at
    /// `cwd` with `env` and register the PTY under a fresh `term-N` id.
    fn spawn_terminal(
        &mut self,
        owner: TerminalOwner,
        cwd: &Path,
        env: &[(String, String)],
        rows: u16,
        cols: u16,
    ) -> Result<(String, String)> {
        // A companion terminal is a plain shell, not an agent, so it opts out of
        // agent-signal tracking: its bytes are never scanned for OSC/bell
        // attention signals (which it does not consume) and it can never raise a
        // spurious attention flag.
        //
        // `rows`/`cols` come from the caller so a TUI spawn matches the visible
        // pane on the first frame (no initial reflow of the shell); headless web
        // callers pass a default size and rely on the client's first resize.
        let client = PtyClient::spawn_with_env_opts(
            &self.config.terminal.command,
            &self.config.terminal.args,
            cwd,
            rows,
            cols,
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
                // Reuse the monotonic terminal counter so the default drag order
                // equals creation order; reorders rewrite this later.
                sort_order: self.terminal_counter as u64,
                created_at: chrono::Utc::now(),
            },
        );

        Ok((terminal_id, label))
    }

    /// Where a file dropped onto the pane currently showing `pty_id` should be
    /// saved.
    ///
    /// `pty_id` is whatever the browser pane is attached to, which is the only
    /// identifier it reliably has: a terminal id, an agent's session id (the
    /// session-slot tab), or an extra tab's id. Resolving all three here keeps
    /// the upload route from having to know how tabs relate to sessions.
    ///
    /// The two answers are deliberately different in kind, and they answer
    /// different INTENTS.
    ///
    /// An AGENT is "look at this for me": the file goes to that agent's upload
    /// directory (`ui.upload_directory`, inside the worktree and ignored by
    /// git), so it never touches the user's git status and it dies with the
    /// agent. Every tab of one agent shares one worktree, so which tab is on
    /// screen does not change the answer.
    ///
    /// A TERMINAL is unchanged: it gets a PLAN rather than a path, because the
    /// real answer is the live working directory of a shell that may have been
    /// `cd`'d anywhere, and that must not be computed on this thread. A
    /// terminal is where the user is working, so a file dropped on one lands
    /// there, whoever owns the terminal. A STANDALONE terminal has no worktree
    /// at all, and cannot reach the upload branch: it is matched here, first,
    /// as a terminal.
    pub fn file_drop_destination(
        &self,
        pty_id: &str,
    ) -> Option<crate::file_drop::FileDropDestination> {
        if let Some(terminal) = self.companion_terminals.get(pty_id) {
            return Some(crate::file_drop::FileDropDestination::Terminal(
                terminal.client.working_directory(),
            ));
        }
        let session = self.session_behind_pty(pty_id)?;
        Some(crate::file_drop::FileDropDestination::AgentUploads {
            worktree: session.directory().into(),
            // Normalized on every read rather than trusted: the pure normalizer
            // is the read-path half of the warn-once-at-load pair, so a config
            // that never went through `load_config` (a test, an in-memory
            // Config) still resolves a usable directory.
            relative: crate::config::normalized_upload_directory(&self.config.ui.upload_directory),
            write_gitignore: self.upload_seed_allowed(session),
        })
    }

    /// Whether the hidden upload directory dux creates inside an agent's
    /// working directory should be seeded with a self-gitignoring `.gitignore`.
    ///
    /// A managed worktree keeps today's behavior: the configured preference
    /// decides, and the directory is always inside a repository anyway.
    ///
    /// A STANDALONE agent's folder is the user's, so the preference is ANDed
    /// with "can git see this path at all". The rule is git visibility rather
    /// than "is this a working repository", because a folder sitting inside
    /// somebody else's repository is exactly where untracked uploads would
    /// pollute their `git status`. A plain folder gets no junk written into it,
    /// and a folder dux could not classify gets nothing either: writing into
    /// the user's directory on a guess is the one direction that cannot be
    /// undone by dux, which never cleans the folder up.
    ///
    /// THE UNPROBED WINDOW, and how it heals. A drop that lands before the
    /// folder has been classified still CREATES the upload directory (that is
    /// `DropDir::open_uploads`'s job and it is not conditional), just without
    /// the `.gitignore`. For a folder that turns out to be a repository, those
    /// uploads show up as untracked files until the next drop into the same
    /// agent: `open_uploads` runs again with the verdict in hand, and its
    /// `.gitignore` create is `O_CREAT | O_EXCL`, so it seeds the directory that
    /// is already there rather than needing a fresh one. Nothing is lost in the
    /// meantime and nothing is written on a guess. A retroactive seed the moment
    /// the verdict lands was considered and not taken: it would put a filesystem
    /// write on the engine actor thread (or need its own worker plus a
    /// seed-an-existing-directory entry point) to close a window the next drop
    /// closes for free.
    fn upload_seed_allowed(&self, session: &crate::model::AgentSession) -> bool {
        if !self.config.ui.upload_write_gitignore {
            return false;
        }
        match &session.workspace {
            crate::model::AgentWorkspace::Managed(_) => true,
            crate::model::AgentWorkspace::Folder(_) => {
                self.folder_repo_status(&session.id).git_can_see_path()
            }
        }
    }

    /// Where a file dropped onto the EDITOR'S FILE TREE should be saved: the
    /// tree directory the user dropped on, inside that agent's worktree.
    ///
    /// The other intent. [`Self::file_drop_destination`] answers "look at this
    /// for me" with the invisible upload directory; this answers "add this file
    /// to my project" with the place the user pointed at, as an ordinary file
    /// git can see.
    ///
    /// A TERMINAL id answers with its own root, because a terminal now has a
    /// file tree: its editor is rooted at the directory the terminal was spawned
    /// in. The root is that SPAWN directory and never the live working
    /// directory, which is the one place this deliberately parts company with
    /// [`Self::file_drop_destination`] above. A drop on the terminal itself is
    /// "put this where I am typing", so it follows the shell; a drop on the
    /// editor's tree is "add this file where I pointed", and the tree it was
    /// pointed at is drawn from the pinned root. Following the shell here would
    /// mean the same click landed somewhere else after a `cd`.
    ///
    /// `relative` is carried through UNVALIDATED on purpose: the guards belong
    /// next to the walk that opens the directory (`DropDir::open_tree_dir`), on
    /// the blocking pool, not on the engine thread.
    pub fn file_drop_tree_destination(
        &self,
        pty_id: &str,
        relative: &str,
    ) -> Option<crate::file_drop::FileDropDestination> {
        if let Some(terminal) = self.companion_terminals.get(pty_id) {
            return Some(crate::file_drop::FileDropDestination::WorktreeDirectory {
                worktree: terminal.client.spawn_dir().to_path_buf(),
                relative: relative.to_string(),
            });
        }
        let session = self.session_behind_pty(pty_id)?;
        Some(crate::file_drop::FileDropDestination::WorktreeDirectory {
            worktree: session.directory().into(),
            relative: relative.to_string(),
        })
    }

    /// The agent pane whose changed files a drop on `pty_id` could affect: its
    /// session id and its worktree, or `None` when there is no agent behind the
    /// pane at all.
    ///
    /// This answers OWNERSHIP only, never whether the file actually landed in
    /// that worktree. A terminal's directory is discovered from a live process
    /// and the shell may have been `cd`'d anywhere, so containment is checked by
    /// the caller against the FINAL path, once the file exists.
    ///
    /// A terminal owned by a PROJECT or by NOTHING answers `None`, because
    /// neither has an agent pane listing changed files. The match is exhaustive
    /// so a fourth kind of owner has to be answered for here.
    pub fn file_drop_refresh_target(&self, pty_id: &str) -> Option<(String, PathBuf)> {
        // The two branches resolve DIFFERENT keyspaces and must not share a
        // lookup: a companion terminal names its owner by SESSION id, while a
        // bare pane id is a TAB id. No tab id is ever a session id, so a shared
        // lookup would silently answer for the wrong entity.
        let session = match self.companion_terminals.get(pty_id) {
            Some(terminal) => match terminal.owner.as_ref() {
                crate::model::TerminalOwnerRef::Session(id) => self.session_by_id(id)?,
                crate::model::TerminalOwnerRef::Project(_)
                | crate::model::TerminalOwnerRef::Standalone => return None,
            },
            None => self.session_behind_pty(pty_id)?,
        };
        Some((session.id.clone(), PathBuf::from(session.directory())))
    }

    /// The agent session a pane's pty id belongs to: the agent whose
    /// session-slot tab it is, or the session owning that extra tab. Routed
    /// through `owning_session_for_tab` so this pane-side lookup and the rest of
    /// the engine resolve a bare tab id the same single way.
    fn session_behind_pty(&self, pty_id: &str) -> Option<&crate::model::AgentSession> {
        let session_id = self.owning_session_for_tab(pty_id)?;
        self.session_by_id(&session_id)
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::test_support::{sample_project, sample_session, test_engine};
    use crate::ids::TabId;
    use crate::model::TerminalOwner;

    #[test]
    fn a_drop_on_any_tab_of_an_agent_lands_in_that_agent_s_upload_directory() {
        // Every tab of one agent shares one worktree, so which tab is on screen
        // must not change where the file lands. Both a slot tab id and an extra
        // tab id resolve back to the owning agent through
        // `owning_session_for_tab`, which is the half a route would otherwise
        // have to know about.
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feature");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine.agent_tabs.insert(
            TabId::new("tab-9"),
            crate::model::AgentTab {
                id: "tab-9".to_string(),
                session_id: "s1".to_string(),
                provider: crate::model::ProviderKind::new("claude"),
                sort_order: 1,
                created_at: chrono::Utc::now(),
            },
        );

        for pty_id in ["s1-slot", "tab-9"] {
            let dest = engine
                .file_drop_destination(pty_id)
                .unwrap_or_else(|| panic!("{pty_id} should resolve to a destination"));
            match dest {
                crate::file_drop::FileDropDestination::AgentUploads {
                    worktree: root,
                    relative,
                    write_gitignore,
                } => {
                    assert_eq!(root, worktree.path(), "for {pty_id}");
                    assert_eq!(
                        relative,
                        crate::config::DEFAULT_UPLOAD_DIRECTORY,
                        "for {pty_id}"
                    );
                    assert!(write_gitignore, "for {pty_id}");
                }
                other => panic!("{pty_id} resolved to {other:?}, not the upload directory"),
            }
        }

        assert!(
            engine.file_drop_destination("nobody").is_none(),
            "an unknown pty id must not resolve to a directory"
        );
    }

    #[test]
    fn an_agent_destination_carries_the_configured_upload_directory_and_gitignore_choice() {
        // The two settings have to reach the destination, or configuring them
        // does nothing at all. A configured value that is unusable degrades
        // through the pure normalizer here, since an in-memory Config never
        // went through `load_config`'s warn-and-correct.
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feature");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);

        engine.config.ui.upload_directory = "tmp/dropped".to_string();
        engine.config.ui.upload_write_gitignore = false;
        match engine.file_drop_destination("s1-slot") {
            Some(crate::file_drop::FileDropDestination::AgentUploads {
                relative,
                write_gitignore,
                ..
            }) => {
                assert_eq!(relative, "tmp/dropped");
                assert!(!write_gitignore);
            }
            other => panic!("resolved to {other:?}"),
        }

        engine.config.ui.upload_directory = "/etc".to_string();
        match engine.file_drop_destination("s1-slot") {
            Some(crate::file_drop::FileDropDestination::AgentUploads { relative, .. }) => {
                assert_eq!(
                    relative,
                    crate::config::DEFAULT_UPLOAD_DIRECTORY,
                    "an absolute upload directory must degrade to the default"
                );
            }
            other => panic!("resolved to {other:?}"),
        }
    }

    /// The upload seed for a STANDALONE agent follows "can git see this path",
    /// which is a different question from "does the changes panel work here".
    ///
    /// A folder INSIDE somebody else's repository is exactly where untracked
    /// uploads would pollute their `git status`, so it gets the seed even
    /// though its own panel stays quiet. A plain folder gets no junk written
    /// into it, and a folder dux could not classify gets nothing either:
    /// writing into the user's directory on a guess is the one direction dux
    /// cannot undo, because it never cleans the folder up.
    #[test]
    fn the_upload_seed_for_a_standalone_agent_follows_git_visibility() {
        let (mut engine, _tmp) = test_engine();
        let folder = tempfile::tempdir().expect("folder");
        engine
            .sessions
            .push(crate::engine::test_support::sample_standalone_session(
                "sa1",
                folder.path().to_string_lossy().as_ref(),
            ));
        engine.config.ui.upload_write_gitignore = true;

        let seeded = |engine: &super::Engine| match engine.file_drop_destination("sa1-slot") {
            Some(crate::file_drop::FileDropDestination::AgentUploads {
                write_gitignore, ..
            }) => write_gitignore,
            other => panic!("resolved to {other:?}"),
        };

        // Unprobed, so unknown: fail closed.
        assert!(!seeded(&engine));

        for (status, expected) in [
            (crate::git::FolderRepoStatus::WorkingRepo, true),
            (
                crate::git::FolderRepoStatus::InsideRepoRootedElsewhere,
                true,
            ),
            (crate::git::FolderRepoStatus::NoRepo, false),
            (crate::git::FolderRepoStatus::Indeterminate, false),
        ] {
            engine
                .folder_repo_statuses
                .insert("sa1".to_string(), status);
            assert_eq!(seeded(&engine), expected, "{status:?}");
        }

        // And the preference still wins outright: switching it off writes
        // nothing anywhere, repository or not.
        engine.config.ui.upload_write_gitignore = false;
        engine
            .folder_repo_statuses
            .insert("sa1".to_string(), crate::git::FolderRepoStatus::WorkingRepo);
        assert!(!seeded(&engine));
    }

    #[test]
    fn a_terminal_of_every_owner_keeps_the_live_working_directory() {
        // The narrowing has to stop at agents. A terminal is where the user is
        // working, whoever owns it, so none of the three kinds may resolve to an
        // upload directory. The standalone one is the case that has no worktree
        // at all, so an upload destination could not even be built for it.
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feature");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];

        let (session_terminal, _) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("session terminal");
        let (project_terminal, _) = engine
            .create_project_terminal("p1", 24, 80)
            .expect("project terminal");
        let (standalone_terminal, _) = engine
            .create_standalone_terminal(24, 80)
            .expect("standalone terminal");

        for pty_id in [&session_terminal, &project_terminal, &standalone_terminal] {
            match engine.file_drop_destination(pty_id) {
                Some(crate::file_drop::FileDropDestination::Terminal(_)) => {}
                other => panic!("{pty_id} resolved to {other:?}, not a live lookup"),
            }
        }
    }

    #[test]
    fn a_drop_on_a_terminal_resolves_to_a_live_lookup_not_a_stored_path() {
        // The distinction that matters: a terminal must NOT hand back a fixed
        // path, because a shell's directory changes the moment someone types
        // `cd`. It hands back a plan that asks the live process.
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feature");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];

        let (terminal_id, _) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("create companion terminal");

        match engine.file_drop_destination(&terminal_id) {
            Some(crate::file_drop::FileDropDestination::Terminal(plan)) => {
                assert_eq!(plan.spawn_dir, worktree.path());
                assert!(
                    plan.shell_pid.is_some(),
                    "the plan must carry a live process to ask, or it can only \
                     ever report the spawn directory"
                );
            }
            other => panic!("a terminal resolved to {other:?}, not a live lookup"),
        }
    }

    #[test]
    fn a_tree_drop_on_a_terminal_lands_under_the_terminal_s_pinned_spawn_directory() {
        // A terminal now HAS a file tree: its editor is rooted at the directory
        // it started in. So a tree drop naming a terminal means what it says,
        // and it means the same thing an agent's tree drop means, add this file
        // where I pointed. The root is the SPAWN directory, never the live one,
        // for the same reason the editor is pinned there.
        let (mut engine, _tmp) = test_engine();
        let repo = tempfile::tempdir().expect("repo dir");
        engine
            .projects
            .push(sample_project("p1", repo.path().to_string_lossy().as_ref()));
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];

        let (terminal_id, _) = engine
            .create_project_terminal("p1", 24, 80)
            .expect("project terminal");

        match engine.file_drop_tree_destination(&terminal_id, "docs") {
            Some(crate::file_drop::FileDropDestination::WorktreeDirectory {
                worktree: root,
                relative,
            }) => {
                assert_eq!(root, repo.path());
                assert_eq!(relative, "docs");
            }
            other => panic!("a terminal tree drop resolved to {other:?}"),
        }

        assert!(
            engine
                .file_drop_tree_destination("nobody", "docs")
                .is_none(),
            "an unknown pty id still has no tree to drop on"
        );
    }

    #[test]
    fn only_a_pane_with_an_agent_behind_it_has_changed_files_to_refresh() {
        // A dropped file is invisible in the Changes pane until something asks
        // for a recompute, and only an agent HAS a changes pane. A terminal
        // owned by a project, or by nothing at all, has no agent behind it, so
        // there is nothing to refresh however useful the file may be.
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feature");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine.agent_tabs.insert(
            TabId::new("tab-9"),
            crate::model::AgentTab {
                id: "tab-9".to_string(),
                session_id: "s1".to_string(),
                provider: crate::model::ProviderKind::new("claude"),
                sort_order: 1,
                created_at: chrono::Utc::now(),
            },
        );
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];

        let (session_terminal, _) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("session terminal");
        let (project_terminal, _) = engine
            .create_project_terminal("p1", 24, 80)
            .expect("project terminal");
        let (standalone_terminal, _) = engine
            .create_standalone_terminal(24, 80)
            .expect("standalone terminal");

        for pty_id in ["s1-slot", "tab-9", session_terminal.as_str()] {
            assert_eq!(
                engine.file_drop_refresh_target(pty_id),
                Some(("s1".to_string(), std::path::PathBuf::from(worktree.path()))),
                "{pty_id} belongs to agent s1"
            );
        }
        for pty_id in [
            project_terminal.as_str(),
            standalone_terminal.as_str(),
            "nobody",
        ] {
            assert_eq!(
                engine.file_drop_refresh_target(pty_id),
                None,
                "{pty_id} has no agent pane behind it"
            );
        }
    }

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
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);

        // `cat` is always on PATH and simply echoes — a safe stand-in terminal.
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];

        let (terminal_id, label) = engine
            .create_companion_terminal("s1", 24, 80)
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
    fn companion_terminals_spawn_with_monotonic_sort_order_and_created_at() {
        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feature");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);

        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];

        let before = chrono::Utc::now();
        let (first, _) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("first terminal");
        let (second, _) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("second terminal");
        let after = chrono::Utc::now();

        let t1 = &engine.companion_terminals[&first];
        let t2 = &engine.companion_terminals[&second];

        // Default order equals creation order: the counter-derived sort_order is
        // strictly increasing across spawns.
        assert_eq!(t1.sort_order, 1);
        assert_eq!(t2.sort_order, 2);
        assert!(t1.sort_order < t2.sort_order);

        // created_at is stamped at spawn, within the observed window.
        assert!(t1.created_at >= before && t1.created_at <= after);
        assert!(t2.created_at >= t1.created_at && t2.created_at <= after);
    }

    #[test]
    fn terminal_is_working_tracks_a_running_foreground_app() {
        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feature");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];

        let (id, _) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("terminal");

        // Idle shell prompt: no foreground app, no streaming, no typing -> idle.
        engine
            .companion_terminals
            .get_mut(&id)
            .unwrap()
            .foreground_cmd = None;
        assert!(
            !engine.terminal_is_working(&id),
            "an idle terminal at the shell prompt is not working"
        );

        // A foreground app is running (the name changed): busy even with no output.
        engine
            .companion_terminals
            .get_mut(&id)
            .unwrap()
            .foreground_cmd = Some("vim".to_string());
        assert!(
            engine.terminal_is_working(&id),
            "a running foreground app reads as working even while quiet"
        );

        // Typing into the terminal takes precedence over the running app.
        engine.note_pty_input(&id);
        assert!(
            !engine.terminal_is_working(&id),
            "typing suppresses the working cue"
        );
        engine.pty_input.remove(&id);

        // An empty foreground_cmd is treated as no app.
        engine
            .companion_terminals
            .get_mut(&id)
            .unwrap()
            .foreground_cmd = Some(String::new());
        assert!(
            !engine.terminal_is_working(&id),
            "an empty foreground_cmd is not a running app"
        );

        // An unknown terminal id is never working.
        assert!(!engine.terminal_is_working("term-nope"));
    }

    /// Scrolling a terminal must behave the way scrolling an agent does for the
    /// half that is an INFERENCE (output text), and must not touch the half that
    /// is a FACT (a foreground app is running). A `vim` that repaints because the
    /// user scrolled it is still `vim` running, so the row keeps saying Running;
    /// the point of the pointer window is only to stop reading the repaint itself
    /// as evidence.
    #[test]
    fn scrolling_a_terminal_suppresses_the_repaint_but_not_a_running_app() {
        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feature");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];

        let (id, _) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("terminal");

        // An idle shell that repaints because the user scrolled it: no app, no
        // progress report, so the repaint is not evidence of anything.
        engine
            .companion_terminals
            .get_mut(&id)
            .unwrap()
            .foreground_cmd = None;
        engine.note_pty_write(&id, b"\x1b[<64;10;5M");
        // Stamp the activity at the pointer stamp's OWN instant rather than
        // reading the clock again. The activity window outlives the pointer one,
        // so two adjacent `Instant::now()` calls would make the assertion depend
        // on how long the gap between these lines happened to be.
        let scrolled_at = engine.pty_pointer[&id].at;
        engine.pty_activity.insert(id.clone(), scrolled_at);
        assert!(
            !engine.terminal_is_working(&id),
            "a repaint caused by the user's own scroll must not read as Running"
        );
        assert!(
            !engine.is_typing(&id),
            "and scrolling must never read as Typing"
        );

        // Now a real app is running in it. Scrolling changes nothing about that.
        engine
            .companion_terminals
            .get_mut(&id)
            .unwrap()
            .foreground_cmd = Some("vim".to_string());
        assert!(
            engine.terminal_is_working(&id),
            "a running foreground app is a fact, not an inference from output, \
             so scrolling must not hide it"
        );
    }

    #[test]
    fn create_companion_terminal_unknown_session_errors() {
        let (mut engine, _tmp) = test_engine();
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];

        let err = engine
            .create_companion_terminal("missing", 24, 80)
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
            .create_project_terminal("p1", 24, 80)
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
    fn create_standalone_terminal_opens_in_the_home_directory_owning_nothing() {
        // The journey: the user asks for a terminal that belongs to nothing. It
        // opens in their home directory, carries the standalone owner, and needs
        // no project and no agent to exist first.
        let (mut engine, _tmp) = test_engine();
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];

        let (terminal_id, label) = engine
            .create_standalone_terminal(24, 80)
            .expect("create standalone terminal");

        assert_eq!(terminal_id, "term-1");
        assert_eq!(label, "Terminal 1");

        let terminal = engine
            .companion_terminals
            .get(&terminal_id)
            .expect("terminal registered");
        assert_eq!(terminal.owner, crate::model::TerminalOwner::Standalone);
        assert_eq!(
            terminal.client.spawn_dir(),
            crate::home_path::standalone_terminal_dir(),
            "a standalone terminal opens where the home-directory rule says"
        );
    }

    #[test]
    fn a_standalone_terminal_gets_the_global_env_and_no_project_overlay() {
        // Two projects exist, each with its own env. A standalone terminal
        // belongs to neither, so it must see the global env and nothing else.
        let (mut engine, _tmp) = test_engine();
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];
        engine
            .config
            .env
            .insert("DUX_GLOBAL".to_string(), "global".to_string());
        let mut p = sample_project("p1", "/tmp/p1");
        p.env
            .insert("DUX_PROJECT".to_string(), "project".to_string());
        engine.projects.push(p);

        // Measured rather than reasoned about: the shell itself reports what it
        // was handed, so this asserts the environment the CHILD really got and
        // not the argument the call site assembled.
        engine.config.terminal.command = "sh".to_string();
        engine.config.terminal.args = vec![
            "-c".to_string(),
            "echo \"global=[$DUX_GLOBAL] project=[$DUX_PROJECT]\"".to_string(),
        ];

        let (id, _) = engine
            .create_standalone_terminal(24, 80)
            .expect("create standalone terminal");

        let output = read_until(&engine.companion_terminals[&id].client, "global=");
        assert!(
            output.contains("global=[global]"),
            "the global env reaches a standalone terminal; saw: {output}"
        );
        assert!(
            output.contains("project=[]"),
            "no project's env overlays a terminal that belongs to no project; saw: {output}"
        );
    }

    /// Poll a PTY's visible text until `needle` appears, bounded. Terminal output
    /// arrives on the reader thread, so the alternative to polling is asserting
    /// against a buffer that may simply not have been filled yet.
    fn read_until(client: &crate::pty::PtyClient, needle: &str) -> String {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let text = client.visible_text_excerpt(usize::MAX);
            if text.contains(needle) || std::time::Instant::now() >= deadline {
                return text;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn create_project_terminal_unknown_project_errors() {
        let (mut engine, _tmp) = test_engine();
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];

        let err = engine
            .create_project_terminal("missing", 24, 80)
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
            .create_project_terminal("p1", 24, 80)
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
