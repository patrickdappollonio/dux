//! Serializable projection of `Engine` state for web clients. Selection, focus,
//! and scroll position are intentionally excluded — those are client-side state
//! under the independent-navigation model. This is a one-way `core -> client`
//! view; it never deserializes.

use serde::Serialize;

use crate::engine::Engine;
use crate::model::{AgentSession, ChangedFile, PrInfo, PrState, Project, ProjectBranchStatus};

/// The full chrome snapshot a web client needs to draw projects, sessions, and
/// the changed-files lists.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ViewModel {
    pub projects: Vec<ProjectView>,
    pub sessions: Vec<SessionView>,
    pub changed_files: ChangedFilesView,
    /// Global environment variables from `[env]` in `config.toml`, applied to
    /// every spawned provider/terminal. Surfaced so a client can pre-fill an
    /// edit dialog.
    pub global_env: std::collections::BTreeMap<String, String>,
    /// Configured provider command names, sorted. Surfaced so a client can
    /// populate a per-project default-provider picker.
    pub available_providers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProjectView {
    pub id: String,
    pub name: String,
    pub path: String,
    pub default_provider: String,
    /// Explicit per-project provider override (None = inherits the global default).
    pub explicit_default_provider: Option<String>,
    pub auto_reopen_agents: Option<bool>,
    pub startup_command: Option<String>,
    pub env: std::collections::BTreeMap<String, String>,
    pub current_branch: String,
    /// "leading" | "not_leading" | "unknown"
    pub branch_status: String,
    pub path_missing: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionView {
    pub id: String,
    pub project_id: String,
    pub title: Option<String>,
    pub provider: String,
    pub branch_name: String,
    pub worktree_path: String,
    /// "active" | "detached" | "exited"
    pub status: String,
    pub auto_reopen_enabled: bool,
    /// Associated GitHub pull request, if one is tracked for this session.
    pub pr: Option<PrView>,
    /// Companion terminals open for this session, sorted by `id` for stability.
    pub terminals: Vec<TerminalView>,
    /// Whether the session's PTY has emitted any output yet. The web UI shows a
    /// readiness spinner until this is true.
    pub has_output: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TerminalView {
    pub id: String,
    pub label: String,
    /// Whether the terminal's PTY has emitted any output yet.
    pub has_output: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PrView {
    pub number: u64,
    /// "open" | "merged" | "closed"
    pub state: String,
    pub title: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct ChangedFilesView {
    pub staged: Vec<ChangedFileView>,
    pub unstaged: Vec<ChangedFileView>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChangedFileView {
    pub status: String,
    pub path: String,
    pub additions: usize,
    pub deletions: usize,
    pub binary: bool,
}

impl ProjectView {
    fn from_project(p: &Project) -> Self {
        Self {
            id: p.id.clone(),
            name: p.name.clone(),
            path: p.path.clone(),
            default_provider: p.default_provider.as_str().to_string(),
            explicit_default_provider: p
                .explicit_default_provider
                .as_ref()
                .map(|pk| pk.as_str().to_string()),
            auto_reopen_agents: p.auto_reopen_agents,
            startup_command: p.startup_command.clone(),
            env: p.env.clone(),
            current_branch: p.current_branch.clone(),
            branch_status: match p.branch_status {
                ProjectBranchStatus::Leading => "leading",
                ProjectBranchStatus::NotLeading => "not_leading",
                ProjectBranchStatus::Unknown => "unknown",
            }
            .to_string(),
            path_missing: p.path_missing,
        }
    }
}

impl SessionView {
    fn from_session(
        s: &AgentSession,
        pr: Option<&PrInfo>,
        terminals: Vec<TerminalView>,
        has_output: bool,
    ) -> Self {
        Self {
            id: s.id.clone(),
            project_id: s.project_id.clone(),
            title: s.title.clone(),
            provider: s.provider.as_str().to_string(),
            branch_name: s.branch_name.clone(),
            worktree_path: s.worktree_path.clone(),
            status: s.status.as_str().to_string(),
            auto_reopen_enabled: s.auto_reopen_enabled,
            pr: pr.map(PrView::from_pr),
            terminals,
            has_output,
        }
    }
}

impl PrView {
    fn from_pr(pr: &PrInfo) -> Self {
        Self {
            number: pr.number,
            state: match pr.state {
                PrState::Open => "open",
                PrState::Merged => "merged",
                PrState::Closed => "closed",
            }
            .to_string(),
            title: pr.title.clone(),
            url: pr.url.clone(),
        }
    }
}

impl ChangedFileView {
    fn from_file(f: &ChangedFile) -> Self {
        Self {
            status: f.status.clone(),
            path: f.path.clone(),
            additions: f.additions,
            deletions: f.deletions,
            binary: f.binary,
        }
    }
}

impl Engine {
    /// Project current engine state into a serializable snapshot for web clients.
    pub fn view_model(&self) -> ViewModel {
        let mut available_providers: Vec<String> =
            self.config.providers.commands.keys().cloned().collect();
        available_providers.sort();
        ViewModel {
            projects: self
                .projects
                .iter()
                .map(ProjectView::from_project)
                .collect(),
            sessions: self
                .sessions
                .iter()
                .map(|s| {
                    let mut terminals: Vec<TerminalView> = self
                        .companion_terminals
                        .iter()
                        .filter(|(_, t)| t.session_id == s.id)
                        .map(|(id, t)| TerminalView {
                            id: id.clone(),
                            label: t.label.clone(),
                            has_output: t.client.has_output(),
                        })
                        .collect();
                    terminals.sort_by(|a, b| a.id.cmp(&b.id));
                    let has_output = self
                        .providers
                        .get(&s.id)
                        .map(|p| p.has_output())
                        .unwrap_or(false);
                    SessionView::from_session(s, self.pr_statuses.get(&s.id), terminals, has_output)
                })
                .collect(),
            changed_files: ChangedFilesView {
                staged: self
                    .staged_files
                    .iter()
                    .map(ChangedFileView::from_file)
                    .collect(),
                unstaged: self
                    .unstaged_files
                    .iter()
                    .map(ChangedFileView::from_file)
                    .collect(),
            },
            global_env: self.config.env.clone(),
            available_providers,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{sample_project, sample_session, test_engine};

    #[test]
    fn projects_sessions_and_changed_files_are_projected() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        engine.sessions.push(sample_session("s1", "p1", "feature"));
        engine.staged_files.push(ChangedFile {
            status: "M".to_string(),
            path: "src/lib.rs".to_string(),
            additions: 3,
            deletions: 1,
            binary: false,
        });

        let vm = engine.view_model();

        assert_eq!(vm.projects.len(), 1);
        assert_eq!(vm.projects[0].id, "p1");
        assert_eq!(vm.projects[0].default_provider, "claude");
        assert_eq!(vm.projects[0].branch_status, "leading");
        assert_eq!(vm.sessions.len(), 1);
        assert_eq!(vm.sessions[0].id, "s1");
        assert_eq!(vm.sessions[0].branch_name, "feature");
        assert_eq!(vm.sessions[0].status, "detached");
        assert_eq!(vm.changed_files.staged.len(), 1);
        assert_eq!(vm.changed_files.staged[0].path, "src/lib.rs");
        assert_eq!(vm.changed_files.staged[0].additions, 3);
        assert_eq!(vm.changed_files.unstaged.len(), 0);
    }

    #[test]
    fn project_settings_fields_are_projected() {
        use crate::model::ProviderKind;

        let (mut engine, _tmp) = test_engine();
        let mut project = sample_project("p1", "/repo");
        project.explicit_default_provider = Some(ProviderKind::new("codex"));
        project.auto_reopen_agents = Some(true);
        project.startup_command = Some("npm install".to_string());
        project.env.insert("KEY".to_string(), "value".to_string());
        engine.projects.push(project);

        let vm = engine.view_model();

        let p = &vm.projects[0];
        assert_eq!(p.explicit_default_provider.as_deref(), Some("codex"));
        assert_eq!(p.auto_reopen_agents, Some(true));
        assert_eq!(p.startup_command.as_deref(), Some("npm install"));
        assert_eq!(p.env.get("KEY").map(String::as_str), Some("value"));
    }

    #[test]
    fn project_without_settings_has_none() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));

        let vm = engine.view_model();

        let p = &vm.projects[0];
        assert!(p.explicit_default_provider.is_none());
        assert!(p.auto_reopen_agents.is_none());
        assert!(p.startup_command.is_none());
        assert!(p.env.is_empty());
    }

    #[test]
    fn session_pr_status_is_projected() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        engine.sessions.push(sample_session("s1", "p1", "feature"));
        engine.pr_statuses.insert(
            "s1".to_string(),
            PrInfo {
                number: 42,
                state: PrState::Merged,
                title: "Add the thing".to_string(),
                host: "github.com".to_string(),
                owner_repo: "owner/repo".to_string(),
                url: "https://github.com/owner/repo/pull/42".to_string(),
            },
        );

        let vm = engine.view_model();

        let pr = vm.sessions[0]
            .pr
            .as_ref()
            .expect("session should carry projected PR");
        assert_eq!(pr.number, 42);
        assert_eq!(pr.state, "merged");
        assert_eq!(pr.title, "Add the thing");
        assert_eq!(pr.url, "https://github.com/owner/repo/pull/42");
    }

    #[test]
    fn session_without_pr_has_none() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        engine.sessions.push(sample_session("s1", "p1", "feature"));

        let vm = engine.view_model();

        assert!(vm.sessions[0].pr.is_none());
    }

    #[test]
    fn companion_terminals_are_projected_onto_their_session() {
        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feature");
        session.worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];

        let (terminal_id, label) = engine
            .create_companion_terminal("s1")
            .expect("create companion terminal");

        let vm = engine.view_model();
        let terminals = &vm.sessions[0].terminals;
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0].id, terminal_id);
        assert_eq!(terminals[0].label, label);
    }

    #[test]
    fn session_without_provider_is_not_ready() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        engine.sessions.push(sample_session("s1", "p1", "feature"));

        let vm = engine.view_model();

        assert!(!vm.sessions[0].has_output);
    }

    #[test]
    fn running_provider_marks_session_ready() {
        use std::time::Duration;

        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feature");
        session.worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);

        // Spawn a real `cat` PTY as the session's provider. `cat` echoes input,
        // so writing to it guarantees the child emits output we can latch on.
        let client = crate::pty::PtyClient::spawn_with_env(
            "cat",
            &[],
            worktree.path(),
            24,
            80,
            engine.config.ui.agent_scrollback_lines,
            &[],
        )
        .expect("spawn cat provider");
        engine.providers.insert("s1".to_string(), client);

        // Before any output, the session is not ready.
        assert!(!engine.view_model().sessions[0].has_output);

        engine
            .providers
            .get("s1")
            .expect("provider exists")
            .write_bytes(b"hello\n")
            .expect("write to provider");

        // Poll for up to ~2s while the reader thread processes the echo.
        let mut became_ready = false;
        for _ in 0..40 {
            if engine.view_model().sessions[0].has_output {
                became_ready = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        assert!(became_ready, "session should become ready after output");
    }

    #[test]
    fn global_env_is_projected() {
        let (mut engine, _tmp) = test_engine();
        engine
            .config
            .env
            .insert("FOO".to_string(), "bar".to_string());

        let vm = engine.view_model();

        assert_eq!(vm.global_env.get("FOO").map(String::as_str), Some("bar"));
    }

    #[test]
    fn available_providers_lists_configured_defaults_sorted() {
        let (engine, _tmp) = test_engine();

        let vm = engine.view_model();

        // A default Config configures these four providers.
        for provider in ["claude", "codex", "gemini", "opencode"] {
            assert!(
                vm.available_providers.iter().any(|p| p == provider),
                "available_providers should contain {provider}: {:?}",
                vm.available_providers
            );
        }
        // The list is sorted.
        let mut sorted = vm.available_providers.clone();
        sorted.sort();
        assert_eq!(vm.available_providers, sorted);
    }

    #[test]
    fn view_model_serializes_to_json() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        let vm = engine.view_model();
        let json = serde_json::to_string(&vm).expect("serialize");
        assert!(json.contains("\"id\":\"p1\""), "json: {json}");
        assert!(
            json.contains("\"branch_status\":\"leading\""),
            "json: {json}"
        );
    }
}
