//! The bodies of the "delete project?" and "remove project?" confirmations,
//! built once as prose for both surfaces, and the status lines that answer
//! them, built from parts so a toast can chip the project's name.
//!
//! The terminal UI renders them in its Confirm Delete Project and Confirm
//! Remove Project dialogs, and the browser's `lib/projectConfirm.ts` builds the
//! same segments for `DeleteProjectDialog` and `RemoveProjectDialog`. The two
//! are pinned against each other by `tests/fixtures/prose_cross_language.json`.
//!
//! `project_name` is `None` only where a surface has no name to show; the
//! sentence then says "this project" rather than inventing one.

use crate::prose::Prose;
use crate::status_text;
use crate::status_text::StatusText;
use crate::text::count_of;

fn project(prose: Prose, project_name: Option<&str>) -> Prose {
    match project_name {
        Some(name) => prose.quoted(name),
        None => prose.text("this project"),
    }
}

/// The worktree-deleting cascade: the project, every agent in it and their
/// worktrees go, and the source checkout stays. The agents and worktrees are
/// named only when there are any, because a project with no agents has no
/// worktrees to mention.
pub fn delete_project_confirm_prose(project_name: Option<&str>, agent_count: usize) -> Prose {
    let lead = project(Prose::new().text("This deletes "), project_name);
    let lead = if agent_count > 0 {
        let worktrees = if agent_count == 1 {
            "its worktree"
        } else {
            "their worktrees"
        };
        lead.text(format!(
            ", its {}, and {worktrees} on disk",
            count_of(agent_count, "agent")
        ))
    } else {
        lead
    };
    lead.text(" from dux. This is irreversible. The source checkout is kept.")
}

/// The record-only removal: the project leaves dux, any agents it still holds
/// leave with it, and every worktree stays on disk.
pub fn remove_project_confirm_prose(project_name: Option<&str>, agent_count: usize) -> Prose {
    let lead = project(Prose::new().text("This removes "), project_name);
    let lead = if agent_count > 0 {
        lead.text(format!(
            " and deletes its {}",
            count_of(agent_count, "agent")
        ))
    } else {
        lead
    };
    lead.text(" from dux. Worktrees on disk are kept.")
}

/// The status line after the "delete project?" confirmation is dismissed:
/// nothing was deleted, and whatever the cascade would have taken is named as
/// still here, because a silent close is indistinguishable from a delete that
/// quietly did nothing.
pub fn delete_project_cancelled_message(project_name: &str, agent_count: usize) -> StatusText {
    let kept = match agent_count {
        0 => String::new(),
        1 => ": its agent and its worktree are still here".to_string(),
        n => format!(": its {n} agents and their worktrees are still here"),
    };
    status_text![
        "Cancelled deleting project ",
        q(project_name),
        ". Nothing was deleted",
        kept,
        "."
    ]
}

/// The status line after the "remove project?" confirmation is dismissed.
pub fn remove_project_cancelled_message(project_name: &str) -> StatusText {
    status_text![
        "Cancelled removing project ",
        q(project_name),
        ". Nothing was removed."
    ]
}

/// What a confirmed delete or remove says when its project vanished while the
/// dialog was up. `verb` is the act that had nothing left to act on.
pub fn project_gone_message(project_name: &str, verb: ProjectGoneVerb) -> StatusText {
    let verb = match verb {
        ProjectGoneVerb::Delete => "delete",
        ProjectGoneVerb::Remove => "remove",
    };
    status_text![
        "Project ",
        q(project_name),
        format!(" is gone, so there was nothing to {verb}.")
    ]
}

/// The status line when a confirm arrives after the project gained agents the
/// dialog never showed: nothing was done, the dialog stays open with the true
/// count, and the user is asked again. Consent covers what was on screen.
pub fn project_gained_agents_message(
    project_name: &str,
    agent_count: usize,
    verb: ProjectGoneVerb,
) -> StatusText {
    let (done, act) = match verb {
        ProjectGoneVerb::Delete => ("deleted", "delete"),
        ProjectGoneVerb::Remove => ("removed", "remove"),
    };
    let them = if agent_count == 1 { "it" } else { "them" };
    status_text![
        "Project ",
        q(project_name),
        format!(
            " gained an agent while the dialog was open, so nothing was {done}. It now has \
             {}; confirm again to {act} {them}.",
            count_of(agent_count, "agent")
        )
    ]
}

/// Which confirmed act found its project gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectGoneVerb {
    /// Deleting the project, its agents' worktrees included.
    Delete,
    /// Removing the project from the workspace.
    Remove,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_delete_body_names_the_cascade_only_when_there_is_one() {
        assert_eq!(
            delete_project_confirm_prose(Some("dux"), 0).plain(),
            "This deletes \"dux\" from dux. This is irreversible. The source checkout is kept."
        );
        assert_eq!(
            delete_project_confirm_prose(Some("dux"), 2).plain(),
            "This deletes \"dux\", its 2 agents, and their worktrees on disk from dux. This is \
             irreversible. The source checkout is kept."
        );
    }

    #[test]
    fn the_gained_agents_line_names_the_new_count_and_asks_again() {
        assert_eq!(
            project_gained_agents_message("dux", 1, ProjectGoneVerb::Delete).message(),
            "Project \"dux\" gained an agent while the dialog was open, so nothing was \
             deleted. It now has 1 agent; confirm again to delete it."
        );
        assert_eq!(
            project_gained_agents_message("dux", 3, ProjectGoneVerb::Remove).message(),
            "Project \"dux\" gained an agent while the dialog was open, so nothing was \
             removed. It now has 3 agents; confirm again to remove them."
        );
    }

    #[test]
    fn the_remove_body_keeps_the_worktrees() {
        assert_eq!(
            remove_project_confirm_prose(Some("dux"), 1).plain(),
            "This removes \"dux\" and deletes its 1 agent from dux. Worktrees on disk are kept."
        );
        assert_eq!(
            remove_project_confirm_prose(None, 0).plain(),
            "This removes this project from dux. Worktrees on disk are kept."
        );
    }

    /// The cancel and gone lines carry the project as a name part, so the web
    /// can chip it, while their plain spelling is the terminal UI's sentence
    /// byte for byte.
    #[test]
    fn the_cancel_and_gone_lines_carry_the_project_as_a_part() {
        use crate::prose::ProseSegment;
        let cases = [
            (
                delete_project_cancelled_message("dux", 0),
                "Cancelled deleting project \"dux\". Nothing was deleted.",
            ),
            (
                delete_project_cancelled_message("dux", 1),
                "Cancelled deleting project \"dux\". Nothing was deleted: its agent and its \
                 worktree are still here.",
            ),
            (
                delete_project_cancelled_message("dux", 3),
                "Cancelled deleting project \"dux\". Nothing was deleted: its 3 agents and their \
                 worktrees are still here.",
            ),
            (
                remove_project_cancelled_message("dux"),
                "Cancelled removing project \"dux\". Nothing was removed.",
            ),
            (
                project_gone_message("dux", ProjectGoneVerb::Delete),
                "Project \"dux\" is gone, so there was nothing to delete.",
            ),
            (
                project_gone_message("dux", ProjectGoneVerb::Remove),
                "Project \"dux\" is gone, so there was nothing to remove.",
            ),
        ];
        for (status, plain) in cases {
            assert_eq!(status.message(), plain);
            let names: Vec<&ProseSegment> = status
                .segments()
                .expect("built from parts")
                .iter()
                .filter(|segment| matches!(segment, ProseSegment::Name { .. }))
                .collect();
            assert_eq!(
                names,
                [&ProseSegment::Name {
                    name: "dux".to_string(),
                    quoted: true
                }],
                "exactly the project is a name in {plain}"
            );
        }
    }
}
