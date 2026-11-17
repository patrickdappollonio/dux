//! The projects the New standalone agent picker offers as folder shortcuts.
//!
//! Picking one only moves the folder explorer to the project's own folder; the
//! agent that results is an ordinary standalone agent (folder only, global
//! environment, no project environment variables, no startup command).
//!
//! Everything a surface could get wrong lives here once, so the terminal UI and
//! the browser cannot disagree: which projects are listed, in what order, which
//! cannot be picked and the words for why, the caption under the list, and how
//! a search query matches a row. The browser receives the rows ready-made and
//! computes none of it.
//!
//! The rows come in two steps, because only one of them is slow.
//! [`unchecked_shortcuts`] is instant: which projects, in what order, under
//! what name and folder label. It touches no file, so a surface that draws its
//! own first frame (the terminal UI) can show the list at once. What each row's
//! folder turns out to be takes the filesystem (is the folder there, does an
//! agent already work in it, by canonical path), so [`project_shortcuts`]
//! answers that on a worker, never on the engine thread or a UI thread, and
//! returns the same rows with their states. [`ShortcutInputs`] is the instant
//! snapshot both steps read. The browser is only ever sent the checked rows.

use std::path::Path;

use serde::Serialize;

use crate::model::{AgentSession, AgentWorkspace, Project};

/// The sentence under the list on both surfaces.
pub const CAPTION: &str =
    "Uses the project's folder only, not its environment variables or startup command.";

/// Whether a shortcut can be picked, and if not, why.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutState {
    /// The folder is there and no agent works in it.
    Available,
    /// A standalone agent already works in the folder, so creating another
    /// there would be refused (one agent per folder).
    AlreadyHasStandaloneAgent,
    /// A managed agent's working copy is the folder, which is refused the same
    /// way. It happens when a project was added from a linked worktree that an
    /// agent works in, a dux agent's own worktree included.
    AlreadyHasAgent,
    /// The project's folder is not on disk.
    FolderMissing,
}

impl ShortcutState {
    /// Whether the row can be picked: exactly when it has no reason not to be.
    pub fn is_pickable(self) -> bool {
        self.reason().is_none()
    }

    /// The words a greyed-out row shows for why it cannot be picked; `None`
    /// for a row that can.
    pub fn reason(self) -> Option<&'static str> {
        match self {
            Self::Available => None,
            Self::AlreadyHasStandaloneAgent => Some("already has a standalone agent"),
            Self::AlreadyHasAgent => Some("already has an agent"),
            Self::FolderMissing => Some("folder missing"),
        }
    }

    /// The state of a folder that is on disk and that `occupant` works in.
    fn occupied_by(occupant: OccupantKind) -> Self {
        match occupant {
            OccupantKind::Standalone => Self::AlreadyHasStandaloneAgent,
            OccupantKind::Managed => Self::AlreadyHasAgent,
        }
    }
}

/// What is known of a shortcut row's folder at a given moment, on a surface
/// that shows the rows before the worker has answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutCheck {
    /// The worker has not answered yet. The row is drawn as an ordinary row,
    /// not greyed out, since nothing is known against it; it cannot be picked
    /// until the answer is in.
    Pending,
    /// The worker stopped before answering, so the folder was never checked.
    Failed,
    /// The worker's answer.
    Checked(ShortcutState),
}

impl ShortcutCheck {
    /// Whether the row can be picked now: only once it is checked and found
    /// available.
    pub fn is_pickable(self) -> bool {
        match self {
            Self::Pending | Self::Failed => false,
            Self::Checked(state) => state.is_pickable(),
        }
    }

    /// The words a greyed-out row shows; `None` for a row that is not greyed
    /// out, which a row still being checked is not.
    pub fn greyed_reason(self) -> Option<&'static str> {
        match self {
            Self::Pending => None,
            Self::Failed => Some("could not be checked"),
            Self::Checked(state) => state.reason(),
        }
    }

    /// Why a pick of this row does nothing right now; `None` when it can be
    /// picked.
    pub fn refusal(self) -> Option<&'static str> {
        match self {
            Self::Pending => Some("its folder is still being checked, try again in a moment"),
            Self::Failed | Self::Checked(_) => self.greyed_reason(),
        }
    }
}

/// One project as a shortcut row before its folder has been checked: who it
/// is, what it is called and how its folder is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UncheckedShortcut {
    /// The project's id.
    pub project_id: String,
    /// The project's name.
    pub name: String,
    /// The folder as shown: the path with the home directory collapsed.
    pub folder_label: String,
    /// The folder's absolute path on the machine dux runs on.
    pub path: String,
}

/// The rows before their folders are checked, and the caption.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UncheckedShortcutList {
    /// The sentence under the list.
    pub caption: &'static str,
    /// One row per project, in the engine's project order.
    pub rows: Vec<UncheckedShortcut>,
}

/// One project offered as a folder shortcut.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProjectShortcut {
    /// The project's id.
    pub project_id: String,
    /// The project's name.
    pub name: String,
    /// The folder as shown: the path with the home directory collapsed.
    pub folder_label: String,
    /// The folder's absolute path on the machine dux runs on, where the
    /// explorer goes when the row is picked.
    pub path: String,
    /// Whether the row can be picked.
    pub state: ShortcutState,
    /// [`ShortcutState::reason`], carried on the wire so the browser prints
    /// the same words without owning them.
    pub reason: Option<&'static str>,
}

/// What the picker shows above its folder explorer: the rows and the caption.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProjectShortcutList {
    /// The sentence under the list.
    pub caption: &'static str,
    /// One row per project, in the engine's project order. Empty when there
    /// are no projects, in which case the surface draws no list at all.
    pub rows: Vec<ProjectShortcut>,
}

/// One project as the worker needs it.
#[derive(Clone, Debug)]
pub struct ShortcutProject {
    /// The project's id.
    pub id: String,
    /// The project's name.
    pub name: String,
    /// The project's folder.
    pub path: String,
}

/// Which kind of agent works in a directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OccupantKind {
    /// A standalone agent: the directory is the user's folder.
    Standalone,
    /// A managed agent: the directory is its working copy.
    Managed,
}

impl OccupantKind {
    /// The kind of agent a workspace belongs to.
    pub fn of(workspace: &AgentWorkspace) -> Self {
        match workspace {
            AgentWorkspace::Managed(_) => Self::Managed,
            AgentWorkspace::Folder(_) => Self::Standalone,
        }
    }
}

/// The directory one agent works in, and which kind of agent it is.
#[derive(Clone, Debug)]
pub struct AgentDirectory {
    /// The directory, as the agent records it.
    pub path: String,
    /// The kind of agent working there.
    pub kind: OccupantKind,
}

/// The engine state the rows are built from, cloned off the engine in an
/// instant so the filesystem work can run elsewhere.
#[derive(Clone, Debug, Default)]
pub struct ShortcutInputs {
    /// The projects, in the engine's order (the order the sidebar groups them in).
    pub projects: Vec<ShortcutProject>,
    /// The directory every agent works in, managed and standalone alike:
    /// exactly what the create-time refusal compares a folder against.
    pub agent_directories: Vec<AgentDirectory>,
}

impl ShortcutInputs {
    /// Snapshot the projects and the agents' directories.
    pub fn snapshot(projects: &[Project], sessions: &[AgentSession]) -> Self {
        Self {
            projects: projects
                .iter()
                .map(|project| ShortcutProject {
                    id: project.id.clone(),
                    name: project.name.clone(),
                    path: project.path.clone(),
                })
                .collect(),
            agent_directories: sessions
                .iter()
                .map(|session| AgentDirectory {
                    path: session.directory().to_string(),
                    kind: OccupantKind::of(&session.workspace),
                })
                .collect(),
        }
    }
}

/// Which of `directories` is the same directory on disk as `folder`, if any.
///
/// THE one-agent-per-folder question, asked by the create-time refusal
/// (`Engine::plan_standalone_agent`) and by the shortcut rows, so a row is
/// greyed out exactly when creating there would be refused. Canonical paths, so
/// a symlink, a `..` component or a trailing slash is not a way around it; a
/// path that cannot be canonicalized is compared as written.
///
/// Reads the filesystem. Lazy: it stops at the first match.
pub fn directory_occupant<'a>(
    folder: &Path,
    directories: impl IntoIterator<Item = &'a str>,
) -> Option<usize> {
    let wanted = crate::project_browser::canonical_or_original(folder);
    directories.into_iter().position(|directory| {
        crate::project_browser::canonical_or_original(Path::new(directory)) == wanted
    })
}

/// The rows for `inputs` before any folder is checked: order, names and
/// folder labels. Instant; reads no file.
pub fn unchecked_shortcuts(inputs: &ShortcutInputs) -> UncheckedShortcutList {
    unchecked_shortcuts_with_home(inputs, home::home_dir().as_deref())
}

/// [`unchecked_shortcuts`] against a given home directory.
pub(crate) fn unchecked_shortcuts_with_home(
    inputs: &ShortcutInputs,
    home: Option<&Path>,
) -> UncheckedShortcutList {
    UncheckedShortcutList {
        caption: CAPTION,
        rows: inputs
            .projects
            .iter()
            .map(|project| UncheckedShortcut {
                project_id: project.id.clone(),
                name: project.name.clone(),
                folder_label: crate::home_path::shorten_home_from(Path::new(&project.path), home),
                path: project.path.clone(),
            })
            .collect(),
    }
}

/// The shortcut rows for `inputs` with every folder checked: the rows of
/// [`unchecked_shortcuts`], each with its state. Reads the filesystem: call it
/// on a worker.
pub fn project_shortcuts(inputs: &ShortcutInputs) -> ProjectShortcutList {
    project_shortcuts_with_home(inputs, home::home_dir().as_deref())
}

/// [`project_shortcuts`] against a given home directory.
pub(crate) fn project_shortcuts_with_home(
    inputs: &ShortcutInputs,
    home: Option<&Path>,
) -> ProjectShortcutList {
    let unchecked = unchecked_shortcuts_with_home(inputs, home);
    let rows = unchecked
        .rows
        .into_iter()
        .map(|row| {
            let folder = Path::new(&row.path);
            // Missing is asked first: a folder that is gone cannot be picked
            // whatever is recorded against its path.
            let state = if folder.is_dir() {
                directory_occupant(
                    folder,
                    inputs
                        .agent_directories
                        .iter()
                        .map(|agent| agent.path.as_str()),
                )
                .and_then(|index| inputs.agent_directories.get(index))
                .map_or(ShortcutState::Available, |agent| {
                    ShortcutState::occupied_by(agent.kind)
                })
            } else {
                ShortcutState::FolderMissing
            };
            ProjectShortcut {
                project_id: row.project_id,
                name: row.name,
                folder_label: row.folder_label,
                path: row.path,
                state,
                reason: state.reason(),
            }
        })
        .collect();
    ProjectShortcutList {
        caption: unchecked.caption,
        rows,
    }
}

/// How a search query matches a project row: a case-insensitive substring of
/// the project's name or of its folder AS THE ROW SHOWS IT. `needle` is
/// expected trimmed and lowercased; an empty one matches everything.
///
/// The folder is the text on the row, never a path behind it: a query for
/// what the user can read finds the row, and a part of the path the row has
/// collapsed away (the home directory behind a `~`) does not match every row
/// at once. Each caller passes what its own rows print.
///
/// The terminal UI's project chooser and its standalone picker's shortcut list
/// both match through this; the browser's list has no search.
pub fn project_matches_query(name: &str, folder_as_shown: &str, needle: &str) -> bool {
    name.to_lowercase().contains(needle) || folder_as_shown.to_lowercase().contains(needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(id: &str, name: &str, path: &Path) -> ShortcutProject {
        ShortcutProject {
            id: id.to_string(),
            name: name.to_string(),
            path: path.to_string_lossy().into_owned(),
        }
    }

    fn states(list: &ProjectShortcutList) -> Vec<(&str, ShortcutState)> {
        list.rows
            .iter()
            .map(|row| (row.project_id.as_str(), row.state))
            .collect()
    }

    #[test]
    fn rows_keep_the_projects_order_and_collapse_the_home_folder() {
        let home = tempfile::tempdir().expect("tempdir");
        let elsewhere = tempfile::tempdir().expect("tempdir");
        let zebra = home.path().join("code/zebra");
        std::fs::create_dir_all(&zebra).expect("mkdir");
        let apple = elsewhere.path().join("apple");
        std::fs::create_dir_all(&apple).expect("mkdir");
        let inputs = ShortcutInputs {
            projects: vec![project("z", "zebra", &zebra), project("a", "apple", &apple)],
            agent_directories: Vec::new(),
        };

        let list = project_shortcuts_with_home(&inputs, Some(home.path()));

        assert_eq!(
            states(&list),
            vec![
                ("z", ShortcutState::Available),
                ("a", ShortcutState::Available)
            ],
            "the engine's order, not an alphabetical one"
        );
        assert_eq!(list.rows[0].folder_label, "~/code/zebra");
        assert_eq!(list.rows[0].path, zebra.to_string_lossy());
        assert_eq!(list.rows[1].folder_label, apple.to_string_lossy());
        assert_eq!(list.rows[0].reason, None);
        assert_eq!(list.caption, CAPTION);
    }

    #[test]
    fn no_projects_means_no_rows() {
        let inputs = ShortcutInputs {
            projects: Vec::new(),
            agent_directories: vec![agent("/somewhere", OccupantKind::Standalone)],
        };
        assert_eq!(project_shortcuts_with_home(&inputs, None).rows, Vec::new());
    }

    fn agent(path: impl Into<String>, kind: OccupantKind) -> AgentDirectory {
        AgentDirectory {
            path: path.into(),
            kind,
        }
    }

    #[test]
    fn a_project_whose_folder_is_gone_cannot_be_picked() {
        let root = tempfile::tempdir().expect("tempdir");
        let gone = root.path().join("gone");
        let inputs = ShortcutInputs {
            projects: vec![project("g", "gone", &gone)],
            // Even with an agent recorded there, missing is what the row says.
            agent_directories: vec![agent(gone.to_string_lossy(), OccupantKind::Standalone)],
        };

        let list = project_shortcuts_with_home(&inputs, None);

        assert_eq!(list.rows[0].state, ShortcutState::FolderMissing);
        assert_eq!(list.rows[0].reason, Some("folder missing"));
        assert!(!list.rows[0].state.is_pickable());
    }

    #[test]
    fn a_folder_an_agent_works_in_cannot_be_picked_however_it_is_spelled() {
        let root = tempfile::tempdir().expect("tempdir");
        let taken = root.path().join("taken");
        let free = root.path().join("free");
        std::fs::create_dir_all(&taken).expect("mkdir");
        std::fs::create_dir_all(&free).expect("mkdir");
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&taken, &link).expect("symlink");

        for agent_directory in [
            taken.to_string_lossy().into_owned(),
            format!("{}/", taken.display()),
            link.to_string_lossy().into_owned(),
            format!("{}/../taken", free.display()),
        ] {
            let inputs = ShortcutInputs {
                projects: vec![project("t", "taken", &taken), project("f", "free", &free)],
                agent_directories: vec![agent(agent_directory.clone(), OccupantKind::Standalone)],
            };
            let list = project_shortcuts_with_home(&inputs, None);
            assert_eq!(
                states(&list),
                vec![
                    ("t", ShortcutState::AlreadyHasStandaloneAgent),
                    ("f", ShortcutState::Available),
                ],
                "agent recorded at {agent_directory}"
            );
            assert_eq!(list.rows[0].reason, Some("already has a standalone agent"));
            assert!(!list.rows[0].state.is_pickable());
            assert!(list.rows[1].state.is_pickable());
        }
    }

    /// A project added from a linked worktree a managed agent works in: the
    /// row is greyed out all the same, and does not call that agent standalone.
    #[test]
    fn a_folder_a_managed_agent_works_in_says_so() {
        let root = tempfile::tempdir().expect("tempdir");
        let worktree = root.path().join("worktree");
        std::fs::create_dir_all(&worktree).expect("mkdir");
        let inputs = ShortcutInputs {
            projects: vec![project("w", "worktree", &worktree)],
            agent_directories: vec![agent(worktree.to_string_lossy(), OccupantKind::Managed)],
        };

        let list = project_shortcuts_with_home(&inputs, None);

        assert_eq!(list.rows[0].state, ShortcutState::AlreadyHasAgent);
        assert_eq!(list.rows[0].reason, Some("already has an agent"));
        assert!(!list.rows[0].state.is_pickable());
    }

    /// The snapshot reads each agent's kind off its workspace.
    #[test]
    fn the_snapshot_tells_a_managed_agent_from_a_standalone_one() {
        use crate::engine::test_support::{sample_session, sample_standalone_session};
        let inputs = ShortcutInputs::snapshot(
            &[],
            &[
                sample_session("m", "p", "feat"),
                sample_standalone_session("s", "/srv/notes"),
            ],
        );
        let kinds: Vec<OccupantKind> = inputs
            .agent_directories
            .iter()
            .map(|agent| agent.kind)
            .collect();
        assert_eq!(kinds, vec![OccupantKind::Managed, OccupantKind::Standalone]);
        assert_eq!(inputs.agent_directories[1].path, "/srv/notes");
    }

    /// The rule the terminal UI's choosers filter by. The needle arrives
    /// trimmed and lowercased, as `SearchableList` hands it over, and the
    /// folder is the one the row shows.
    #[test]
    fn a_query_matches_the_name_or_the_folder_as_shown_whatever_their_case() {
        assert!(project_matches_query("Website", "~/site", "web"));
        assert!(project_matches_query("api", "~/Work/api", "work"));
        assert!(project_matches_query("notes", "~/notes", "~/notes"));
        assert!(project_matches_query("notes", "~/notes", ""));
        assert!(!project_matches_query("notes", "~/notes", "zzz"));
    }

    /// The label a row shows is what a query is matched against, so the part
    /// of the path collapsed behind the tilde finds nothing.
    #[test]
    fn a_query_for_the_collapsed_home_folder_matches_no_row() {
        let home = tempfile::tempdir().expect("tempdir");
        let inputs = ShortcutInputs {
            projects: vec![project("n", "notes", &home.path().join("notes"))],
            agent_directories: Vec::new(),
        };
        let list = unchecked_shortcuts_with_home(&inputs, Some(home.path()));
        let row = &list.rows[0];
        assert_eq!(row.folder_label, "~/notes");
        let home_name = home
            .path()
            .file_name()
            .expect("a name")
            .to_string_lossy()
            .to_lowercase();
        assert!(row.path.to_lowercase().contains(&home_name));
        assert!(!project_matches_query(
            &row.name,
            &row.folder_label,
            &home_name
        ));
        assert!(project_matches_query(&row.name, &row.folder_label, "~/no"));
    }

    /// The instant rows are the checked rows minus their states: same
    /// projects, order, names, labels and caption, and no file is read (the
    /// folders here do not exist).
    #[test]
    fn the_unchecked_rows_are_the_checked_rows_without_their_states() {
        let inputs = ShortcutInputs {
            projects: vec![
                project("z", "zebra", Path::new("/nowhere/zebra")),
                project("a", "apple", Path::new("/nowhere/apple")),
            ],
            agent_directories: Vec::new(),
        };
        let unchecked = unchecked_shortcuts_with_home(&inputs, Some(Path::new("/nowhere")));
        let checked = project_shortcuts_with_home(&inputs, Some(Path::new("/nowhere")));
        assert_eq!(unchecked.caption, checked.caption);
        let shown = |id: &str, name: &str, label: &str, path: &str| {
            (
                id.to_string(),
                name.to_string(),
                label.to_string(),
                path.to_string(),
            )
        };
        assert_eq!(
            unchecked
                .rows
                .iter()
                .map(|row| shown(&row.project_id, &row.name, &row.folder_label, &row.path))
                .collect::<Vec<_>>(),
            checked
                .rows
                .iter()
                .map(|row| shown(&row.project_id, &row.name, &row.folder_label, &row.path))
                .collect::<Vec<_>>(),
        );
        assert_eq!(unchecked.rows[0].folder_label, "~/zebra");
    }

    /// A row still being checked is neither greyed out nor pickable; one that
    /// could not be checked is greyed out and says so.
    #[test]
    fn a_row_is_pickable_only_once_checked_and_available() {
        assert!(!ShortcutCheck::Pending.is_pickable());
        assert_eq!(ShortcutCheck::Pending.greyed_reason(), None);
        assert!(ShortcutCheck::Pending.refusal().is_some());
        assert!(!ShortcutCheck::Failed.is_pickable());
        assert_eq!(
            ShortcutCheck::Failed.greyed_reason(),
            Some("could not be checked")
        );
        assert!(ShortcutCheck::Checked(ShortcutState::Available).is_pickable());
        assert_eq!(
            ShortcutCheck::Checked(ShortcutState::Available).refusal(),
            None
        );
        assert_eq!(
            ShortcutCheck::Checked(ShortcutState::FolderMissing).refusal(),
            Some("folder missing")
        );
    }
}
