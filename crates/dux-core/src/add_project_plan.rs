//! The add-project preflight DECISION, core-owned so the TUI and the web can
//! never disagree on WHICH action + warning fires for a given inspection.
//!
//! Split of responsibility (per the CLAUDE.md tenets and the wave-4 decision):
//! core owns the DECISION and returns a STABLE TYPED code carrying its
//! structured data (path, default-branch name); each surface keeps its OWN
//! rendered strings, mapping the code to its own copy however it sees fit. The
//! prose is deliberately NOT centralized here. The decision is pinned by the
//! shared test-vector matrix below (the `agent_search` style), so the two
//! surfaces' string maps can evolve independently while the branch-selection
//! logic stays single-source.
//!
//! Precedence (highest first): blocked > init > unborn-commit > known-checkout >
//! heuristic-warn > ready. The `can_checkout_default` rule rides the plan: only
//! the known-default warning may offer to check out the default branch first.

use crate::git::RepoPathKind;
use crate::worker::BranchWarningKind;

/// The raw git observations an add-project preflight collects, fed to
/// [`add_project_plan`]. Pure data so the decision is unit-testable without
/// running git (each surface runs the git probes, then calls the plan).
#[derive(Clone, Debug)]
pub struct AddProjectInspection {
    /// How the path classifies (`repo_path_kind`).
    pub path_kind: RepoPathKind,
    /// The repo's current branch, or `None` for a detached HEAD (no branch to
    /// compare, so no "not on default" warning).
    pub current_branch: Option<String>,
    /// The result of `branch_warning_kind` for the current branch, or `None`
    /// when the branch is the default (or the repo is detached / has no commits).
    pub branch_warning: Option<BranchWarningKind>,
    /// Whether the repo has at least one commit (a repo with an unborn HEAD
    /// cannot back a worktree until it gets an initial commit).
    pub has_commits: bool,
}

/// The add-project action the surface must take, as a typed code (never a
/// rendered string).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddProjectAction {
    /// The path is not a git repository: offer to initialize one.
    Init,
    /// The path is inside an existing work tree or git dir: blocked from being
    /// added as its own project. `root` names the containing work-tree root when
    /// known (a git-dir hit carries `None`).
    Blocked { root: Option<String> },
    /// A repo with an unborn HEAD (no commits): offer to make an initial commit
    /// before adding.
    NeedsInitialCommit,
    /// The path is a bare or work-tree root ready to add (subject to the warning).
    Ready,
}

/// The add-project warning code (never a rendered string). The surface renders
/// its own copy from this.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddProjectWarning {
    /// No warning: the repo is on its default branch (or the action is not
    /// `Ready`).
    None,
    /// The repo is on a non-default branch and dux resolved the default for
    /// certain, so it may offer to check it out first.
    NotOnDefaultBranch { default_branch: String },
    /// The repo is on a non-default branch but dux cannot identify the default,
    /// so it warns without offering a switch.
    NotOnDefaultBranchUnknown,
}

/// The composed add-project plan: the typed action + warning code, plus the
/// `can_checkout_default` rule (only the known-default warning may offer a
/// switch). Rendered per surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddProjectPlan {
    pub action: AddProjectAction,
    pub warning: AddProjectWarning,
    pub can_checkout_default: bool,
}

/// Compose the add-project plan from a raw inspection, applying the precedence
/// blocked > init > unborn-commit > known-checkout > heuristic-warn > ready.
/// Pure: no git, no I/O.
pub fn add_project_plan(inspection: &AddProjectInspection) -> AddProjectPlan {
    // 1-2. Path classification wins first: a git-internal / subdir path is
    // blocked, a non-repo path offers init. Neither carries a branch warning.
    match &inspection.path_kind {
        RepoPathKind::InsideWorkTree { root } => {
            return AddProjectPlan {
                action: AddProjectAction::Blocked {
                    root: Some(root.to_string_lossy().to_string()),
                },
                warning: AddProjectWarning::None,
                can_checkout_default: false,
            };
        }
        RepoPathKind::InsideGitDir { .. } => {
            return AddProjectPlan {
                action: AddProjectAction::Blocked { root: None },
                warning: AddProjectWarning::None,
                can_checkout_default: false,
            };
        }
        RepoPathKind::NotARepo => {
            return AddProjectPlan {
                action: AddProjectAction::Init,
                warning: AddProjectWarning::None,
                can_checkout_default: false,
            };
        }
        // A bare/work-tree root, or an indeterminate git result, falls through to
        // the commit/branch rungs (indeterminate fails OPEN into a normal add, so
        // a transient git failure never hijacks the flow with the init path).
        RepoPathKind::BareRoot | RepoPathKind::WorkTreeRoot | RepoPathKind::Indeterminate => {}
    }

    // 3. Unborn HEAD (no commits): needs an initial commit before it can back a
    // worktree. The branch warning is moot until then.
    if !inspection.has_commits {
        return AddProjectPlan {
            action: AddProjectAction::NeedsInitialCommit,
            warning: AddProjectWarning::None,
            can_checkout_default: false,
        };
    }

    // 4-5. Ready to add. A detached HEAD (no current branch) carries no
    // "not on default" warning. Otherwise the branch-warning code decides:
    // Known offers a checkout, Heuristic warns without one.
    let (warning, can_checkout_default) =
        match (&inspection.current_branch, &inspection.branch_warning) {
            (Some(_), Some(BranchWarningKind::Known { default_branch })) => (
                AddProjectWarning::NotOnDefaultBranch {
                    default_branch: default_branch.clone(),
                },
                true,
            ),
            (Some(_), Some(BranchWarningKind::Heuristic)) => {
                (AddProjectWarning::NotOnDefaultBranchUnknown, false)
            }
            _ => (AddProjectWarning::None, false),
        };
    AddProjectPlan {
        action: AddProjectAction::Ready,
        warning,
        can_checkout_default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// One row of the shared decision matrix (the `agent_search` style): an
    /// inspection and the plan it MUST produce. Both surfaces render their own
    /// strings from these codes, but neither may disagree on which code fires.
    struct Vector {
        name: &'static str,
        inspection: AddProjectInspection,
        expect: AddProjectPlan,
    }

    fn vectors() -> Vec<Vector> {
        vec![
            Vector {
                name: "inside a work tree is blocked (root named)",
                inspection: AddProjectInspection {
                    path_kind: RepoPathKind::InsideWorkTree {
                        root: PathBuf::from("/repo"),
                    },
                    current_branch: None,
                    branch_warning: None,
                    has_commits: true,
                },
                expect: AddProjectPlan {
                    action: AddProjectAction::Blocked {
                        root: Some("/repo".to_string()),
                    },
                    warning: AddProjectWarning::None,
                    can_checkout_default: false,
                },
            },
            Vector {
                name: "inside a git dir is blocked (no root)",
                inspection: AddProjectInspection {
                    path_kind: RepoPathKind::InsideGitDir {
                        git_dir: PathBuf::from("/repo/.git"),
                    },
                    current_branch: None,
                    branch_warning: None,
                    has_commits: true,
                },
                expect: AddProjectPlan {
                    action: AddProjectAction::Blocked { root: None },
                    warning: AddProjectWarning::None,
                    can_checkout_default: false,
                },
            },
            Vector {
                name: "a plain folder offers init",
                inspection: AddProjectInspection {
                    path_kind: RepoPathKind::NotARepo,
                    current_branch: None,
                    branch_warning: None,
                    has_commits: false,
                },
                expect: AddProjectPlan {
                    action: AddProjectAction::Init,
                    warning: AddProjectWarning::None,
                    can_checkout_default: false,
                },
            },
            Vector {
                name: "an unborn repo needs an initial commit (branch warning moot)",
                inspection: AddProjectInspection {
                    path_kind: RepoPathKind::WorkTreeRoot,
                    current_branch: Some("feature".to_string()),
                    branch_warning: Some(BranchWarningKind::Heuristic),
                    has_commits: false,
                },
                expect: AddProjectPlan {
                    action: AddProjectAction::NeedsInitialCommit,
                    warning: AddProjectWarning::None,
                    can_checkout_default: false,
                },
            },
            Vector {
                name: "on a non-default branch with a known default: checkout offered",
                inspection: AddProjectInspection {
                    path_kind: RepoPathKind::WorkTreeRoot,
                    current_branch: Some("feature".to_string()),
                    branch_warning: Some(BranchWarningKind::Known {
                        default_branch: "main".to_string(),
                    }),
                    has_commits: true,
                },
                expect: AddProjectPlan {
                    action: AddProjectAction::Ready,
                    warning: AddProjectWarning::NotOnDefaultBranch {
                        default_branch: "main".to_string(),
                    },
                    can_checkout_default: true,
                },
            },
            Vector {
                name: "on a non-default branch, default unknown: heuristic warn, no checkout",
                inspection: AddProjectInspection {
                    path_kind: RepoPathKind::WorkTreeRoot,
                    current_branch: Some("wip".to_string()),
                    branch_warning: Some(BranchWarningKind::Heuristic),
                    has_commits: true,
                },
                expect: AddProjectPlan {
                    action: AddProjectAction::Ready,
                    warning: AddProjectWarning::NotOnDefaultBranchUnknown,
                    can_checkout_default: false,
                },
            },
            Vector {
                name: "on the default branch: ready, no warning",
                inspection: AddProjectInspection {
                    path_kind: RepoPathKind::WorkTreeRoot,
                    current_branch: Some("main".to_string()),
                    branch_warning: None,
                    has_commits: true,
                },
                expect: AddProjectPlan {
                    action: AddProjectAction::Ready,
                    warning: AddProjectWarning::None,
                    can_checkout_default: false,
                },
            },
            Vector {
                name: "detached HEAD on a committed repo: ready, no warning",
                inspection: AddProjectInspection {
                    path_kind: RepoPathKind::WorkTreeRoot,
                    current_branch: None,
                    branch_warning: None,
                    has_commits: true,
                },
                expect: AddProjectPlan {
                    action: AddProjectAction::Ready,
                    warning: AddProjectWarning::None,
                    can_checkout_default: false,
                },
            },
            Vector {
                name: "a bare root on default is ready",
                inspection: AddProjectInspection {
                    path_kind: RepoPathKind::BareRoot,
                    current_branch: Some("main".to_string()),
                    branch_warning: None,
                    has_commits: true,
                },
                expect: AddProjectPlan {
                    action: AddProjectAction::Ready,
                    warning: AddProjectWarning::None,
                    can_checkout_default: false,
                },
            },
        ]
    }

    #[test]
    fn add_project_plan_matches_the_shared_decision_matrix() {
        for v in vectors() {
            assert_eq!(add_project_plan(&v.inspection), v.expect, "{}", v.name);
        }
    }
}
