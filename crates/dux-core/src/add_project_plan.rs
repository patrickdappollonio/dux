//! The add-project preflight DECISION, core-owned so the TUI and the web can
//! never disagree on WHICH action + warning fires for a given inspection.
//!
//! Split of responsibility (per the CLAUDE.md tenets):
//! core owns the DECISION and returns a STABLE TYPED code carrying its
//! structured data (path, default-branch name); each surface maps the code to
//! its own copy. The exception is the branch warning, whose sentences both
//! surfaces print word for word: those live in [`crate::add_project_prose`].
//! The decision is pinned by the shared test-vector matrix below (the
//! `agent_search` style), so the branch-selection logic stays single-source.
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
    Blocked {
        /// The containing work tree's root, when known.
        root: Option<String>,
    },
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
    NotOnDefaultBranch {
        /// The repository's default branch.
        default_branch: String,
    },
    /// The repo is on a non-default branch but dux cannot identify the default,
    /// so it warns without offering a switch.
    NotOnDefaultBranchUnknown,
}

/// The composed add-project plan: the typed action + warning code, plus the
/// `can_checkout_default` rule (only the known-default warning may offer a
/// switch). Rendered per surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddProjectPlan {
    /// What adding the path will do, or why it cannot.
    pub action: AddProjectAction,
    /// What the add should warn about.
    pub warning: AddProjectWarning,
    /// Whether the surface may offer to check the default branch out first.
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

/// The branch a project's new worktrees start from, decided once when the
/// project is added (and again when the user checks out its default branch).
///
/// The kind is part of the answer because each surface picks the TONE of its
/// "New worktrees will branch from ..." line from it: the remote's default is
/// what a user expects and reads as neutral, anything else is a warning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectBase {
    /// The remote's default branch (`origin/HEAD`).
    RemoteDefault(String),
    /// Any other branch: the one the folder is on, or the `main` fallback when
    /// nothing better is known.
    Other(String),
}

impl ProjectBase {
    /// The branch's name.
    pub fn branch(&self) -> &str {
        match self {
            Self::RemoteDefault(branch) | Self::Other(branch) => branch,
        }
    }

    /// The branch's name, by value.
    pub fn into_branch(self) -> String {
        match self {
            Self::RemoteDefault(branch) | Self::Other(branch) => branch,
        }
    }

    /// Whether it is the remote's default branch.
    pub fn is_remote_default(&self) -> bool {
        match self {
            Self::RemoteDefault(_) => true,
            Self::Other(_) => false,
        }
    }
}

/// Whether a project add first checks the folder out at the remote's default
/// branch: the add dialog's "Check out the default branch before adding" box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefaultCheckout {
    /// The box is ticked: the folder moves to the default branch.
    CheckOutDefault,
    /// The box is unticked: the folder stays on the branch it is on.
    StayOnCurrent,
}

/// Which branch becomes a project's base when it is added.
///
/// `check_out_default` is the add dialog's "Check out the default branch
/// before adding" box: ticked, the folder moves to the default and the project
/// branches from it; unticked, the project branches from wherever the folder
/// is, which is exactly what the dialog tells the user. A detached HEAD has no
/// branch to stay on, so it takes the default when one is known and `main`
/// otherwise, the same fallback every other path uses.
/// Pure: the caller runs the git probes.
pub fn project_base_at_add(
    current_branch: Option<&str>,
    remote_default: Option<&str>,
    check_out_default: DefaultCheckout,
) -> ProjectBase {
    match (current_branch, remote_default) {
        (_, Some(default)) if check_out_default == DefaultCheckout::CheckOutDefault => {
            ProjectBase::RemoteDefault(default.to_string())
        }
        (Some(current), Some(default)) if current == default => {
            ProjectBase::RemoteDefault(default.to_string())
        }
        (Some(current), _) => ProjectBase::Other(current.to_string()),
        (None, Some(default)) => ProjectBase::RemoteDefault(default.to_string()),
        (None, None) => ProjectBase::Other("main".to_string()),
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
        let mut all = vectors_that_stop_short();
        all.extend(vectors_that_are_ready());
        all
    }

    /// The rows whose folder cannot be added as it is.
    fn vectors_that_stop_short() -> Vec<Vector> {
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
        ]
    }

    /// The rows whose folder is ready to add, each with its branch warning.
    fn vectors_that_are_ready() -> Vec<Vector> {
        vec![
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

    /// One row of the base-branch matrix: what the add knows and which branch
    /// new worktrees must then start from.
    struct BaseVector {
        name: &'static str,
        current_branch: Option<&'static str>,
        remote_default: Option<&'static str>,
        check_out_default: DefaultCheckout,
        expect: ProjectBase,
    }

    fn base_vectors() -> Vec<BaseVector> {
        vec![
            BaseVector {
                name: "known default, box ticked: the default branch",
                current_branch: Some("feature"),
                remote_default: Some("main"),
                check_out_default: DefaultCheckout::CheckOutDefault,
                expect: ProjectBase::RemoteDefault("main".to_string()),
            },
            BaseVector {
                name: "known default, box unticked: the branch the folder is on",
                current_branch: Some("feature"),
                remote_default: Some("main"),
                check_out_default: DefaultCheckout::StayOnCurrent,
                expect: ProjectBase::Other("feature".to_string()),
            },
            BaseVector {
                name: "no known default (heuristic): the current branch",
                current_branch: Some("wip"),
                remote_default: None,
                check_out_default: DefaultCheckout::StayOnCurrent,
                expect: ProjectBase::Other("wip".to_string()),
            },
            BaseVector {
                name: "already on the known default: the default branch",
                current_branch: Some("main"),
                remote_default: Some("main"),
                check_out_default: DefaultCheckout::StayOnCurrent,
                expect: ProjectBase::RemoteDefault("main".to_string()),
            },
            BaseVector {
                name: "detached HEAD with a known default: the default branch",
                current_branch: None,
                remote_default: Some("trunk"),
                check_out_default: DefaultCheckout::StayOnCurrent,
                expect: ProjectBase::RemoteDefault("trunk".to_string()),
            },
            BaseVector {
                name: "detached HEAD checked out to the known default: the default branch",
                current_branch: None,
                remote_default: Some("trunk"),
                check_out_default: DefaultCheckout::CheckOutDefault,
                expect: ProjectBase::RemoteDefault("trunk".to_string()),
            },
            BaseVector {
                name: "detached HEAD, nothing known: the main fallback",
                current_branch: None,
                remote_default: None,
                check_out_default: DefaultCheckout::StayOnCurrent,
                expect: ProjectBase::Other("main".to_string()),
            },
            BaseVector {
                name: "a checkout asked for with no known default cannot happen: current wins",
                current_branch: Some("wip"),
                remote_default: None,
                check_out_default: DefaultCheckout::CheckOutDefault,
                expect: ProjectBase::Other("wip".to_string()),
            },
        ]
    }

    #[test]
    fn project_base_at_add_matches_the_base_branch_matrix() {
        for v in base_vectors() {
            assert_eq!(
                project_base_at_add(v.current_branch, v.remote_default, v.check_out_default),
                v.expect,
                "{}",
                v.name
            );
        }
    }

    #[test]
    fn a_project_base_names_its_branch_whichever_kind_it_is() {
        assert_eq!(ProjectBase::RemoteDefault("main".into()).branch(), "main");
        assert_eq!(ProjectBase::Other("wip".into()).into_branch(), "wip");
        assert!(ProjectBase::RemoteDefault("main".into()).is_remote_default());
        assert!(!ProjectBase::Other("wip".into()).is_remote_default());
    }
}
