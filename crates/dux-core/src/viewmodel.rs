//! Serializable projection of `Engine` state for web clients. Selection, focus,
//! and scroll position are intentionally excluded — those are client-side state
//! under the independent-navigation model. This is a one-way `core -> client`
//! view; it never deserializes.

use serde::Serialize;

use crate::engine::Engine;
use crate::model::{AgentSession, ChangedFile, Project, ProjectBranchStatus};

/// The full chrome snapshot a web client needs to draw projects, sessions, and
/// the changed-files lists.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ViewModel {
    pub projects: Vec<ProjectView>,
    pub sessions: Vec<SessionView>,
    pub changed_files: ChangedFilesView,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProjectView {
    pub id: String,
    pub name: String,
    pub path: String,
    pub default_provider: String,
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
    fn from_session(s: &AgentSession) -> Self {
        Self {
            id: s.id.clone(),
            project_id: s.project_id.clone(),
            title: s.title.clone(),
            provider: s.provider.as_str().to_string(),
            branch_name: s.branch_name.clone(),
            worktree_path: s.worktree_path.clone(),
            status: s.status.as_str().to_string(),
            auto_reopen_enabled: s.auto_reopen_enabled,
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
        ViewModel {
            projects: self
                .projects
                .iter()
                .map(ProjectView::from_project)
                .collect(),
            sessions: self
                .sessions
                .iter()
                .map(SessionView::from_session)
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
