//! What the delete-agent dialog says when git could not count the commits its
//! branch box would destroy, built once as prose for both surfaces.
//!
//! The terminal UI renders it in its Confirm Delete Agent dialog, and the
//! browser's `lib/deleteAgent.ts` builds the same segments for
//! `DeleteSessionDialog`. The two are pinned against each other by
//! `tests/fixtures/prose_cross_language.json`.

use crate::branch_history::BranchOrigin;
use crate::prose::Prose;

/// The sentence saying the unpushed commits on `branches` could not be
/// counted, with git's `reason`. Every branch is a name; the reason is relayed
/// error text and stays plain. Two branches (a drifted agent) take the plural.
pub fn unpushed_count_failure_prose(branches: &[&str], reason: &str) -> Prose {
    let mut prose = Prose::new().text("dux could not count the unpushed commits on ");
    for (index, branch) in branches.iter().enumerate() {
        if index > 0 {
            prose.push_text(" and ");
        }
        prose.push_name(*branch);
    }
    prose.push_text(format!(" ({reason}); "));
    prose.text(if branches.len() > 1 {
        "check them before deleting them."
    } else {
        "check it before deleting it."
    })
}

/// What git answered about the commits a box would delete: a count and
/// whether the repository has anything to have pushed them to, or why it
/// could not count.
pub type UnpushedAnswer = Result<crate::git::UnpushedCommits, String>;

/// `[a]`, `[a] and [b]`, `[a], [b] and [c]` with `joiner` before the last.
fn push_list(prose: &mut Prose, names: &[&str], joiner: &str) {
    for (index, name) in names.iter().enumerate() {
        if index > 0 {
            prose.push_text(if index + 1 == names.len() {
                joiner
            } else {
                ", "
            });
        }
        prose.push_name(*name);
    }
}

/// The first box's label.
pub fn current_branch_label_prose(branch: &str) -> Prose {
    Prose::new().text("Also delete the branch ").name(branch)
}

/// The second box's label, naming every branch a tick deletes.
pub fn other_branches_label_prose(branches: &[&str]) -> Prose {
    let mut prose = Prose::new().text(if branches.len() == 1 {
        "Also delete the other branch this agent was on: "
    } else {
        "Also delete the other branches this agent was on: "
    });
    push_list(&mut prose, branches, " and ");
    prose
}

/// The sentence about how much a box's tick destroys, `None` when git could
/// not count (the failure sentence says that) or there was no answer yet.
/// Whether a delete-dialog box shows ticked, which decides whether its warning
/// says what unticking leaves and what a delete loses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxState {
    /// Ticked: the branches go.
    Ticked,
    /// Unticked: the branches stay.
    Unticked,
}

/// What deleting `branches` (every box ticked right now) loses: their
/// commits on no remote and no local branch that stays, as git counted them
/// from the commits the dialog pinned, or why git could not count. `None`
/// when git counted no such commit, since there is nothing to warn about.
pub fn branch_loss_prose(branches: &[&str], lost: &UnpushedAnswer) -> Option<Prose> {
    let answer = match lost {
        Ok(answer) => *answer,
        Err(reason) => return Some(unpushed_count_failure_prose(branches, reason)),
    };
    let lost = match answer.count {
        0 => return None,
        1 => "loses 1 commit that is".to_string(),
        count => format!("loses {count} commits that are"),
    };
    let (nowhere, tail) = if answer.has_remote_refs {
        ("on no remote and no branch that stays", "")
    } else {
        (
            "on no branch that stays",
            ", and this repository has no remote",
        )
    };
    let mut prose = Prose::new().text("Deleting ");
    push_list(&mut prose, branches, " and ");
    Some(prose.text(format!(" {lost} {nowhere}{tail}.")))
}

fn push_sentence(prose: &mut Prose, sentence: Prose) {
    if !prose.is_empty() {
        prose.push_text(" ");
    }
    *prose = std::mem::take(prose).then(sentence);
}

/// The warning under the first box, `None` for a branch dux can prove is its
/// own.
pub fn current_branch_warning_prose(origin: BranchOrigin) -> Option<Prose> {
    let why = match origin {
        BranchOrigin::Dux => return None,
        BranchOrigin::Attached | BranchOrigin::Before => {
            Some("This branch existed before the agent.")
        }
        BranchOrigin::Adopted => Some("This branch came with the worktree this agent adopted."),
        BranchOrigin::NotDux => Some("This branch is not one dux created."),
        BranchOrigin::Unknown => {
            Some("dux cannot tell when this branch was made, so it may not be this agent's.")
        }
        BranchOrigin::After => None,
    };
    why.map(|why| Prose::new().text(why))
}

/// The warning under the second box: who dux cannot vouch for, how much a
/// tick destroys, and what unticking leaves.
pub fn other_branches_warning_prose(others: &[(&str, BranchOrigin)], state: BoxState) -> Prose {
    let mut prose = Prose::new();
    let named = |pick: &dyn Fn(BranchOrigin) -> bool| -> Vec<&str> {
        others
            .iter()
            .filter(|(_, origin)| pick(*origin))
            .map(|(name, _)| *name)
            .collect()
    };
    let before = named(&|o| matches!(o, BranchOrigin::Attached | BranchOrigin::Before));
    if !before.is_empty() {
        let mut sentence = Prose::new();
        push_list(&mut sentence, &before, " and ");
        sentence.push_text(" existed before the agent.");
        push_sentence(&mut prose, sentence);
    }
    let adopted = named(&|o| o == BranchOrigin::Adopted);
    if !adopted.is_empty() {
        let mut sentence = Prose::new();
        push_list(&mut sentence, &adopted, " and ");
        sentence.push_text(" came with the worktree this agent adopted.");
        push_sentence(&mut prose, sentence);
    }
    let not_dux = named(&|o| o == BranchOrigin::NotDux);
    if !not_dux.is_empty() {
        let mut sentence = Prose::new();
        push_list(&mut sentence, &not_dux, " and ");
        sentence.push_text(if not_dux.len() == 1 {
            " is not one dux created."
        } else {
            " are not ones dux created."
        });
        push_sentence(&mut prose, sentence);
    }
    let after = named(&|o| o == BranchOrigin::After);
    if !after.is_empty() {
        let mut sentence = Prose::new().text("dux did not make ");
        push_list(&mut sentence, &after, " or ");
        sentence.push_text(", and cannot tell who did.");
        push_sentence(&mut prose, sentence);
    }
    let unknown = named(&|o| o == BranchOrigin::Unknown);
    if !unknown.is_empty() {
        let mut sentence = Prose::new().text("dux cannot tell when ");
        push_list(&mut sentence, &unknown, " and ");
        sentence.push_text(if unknown.len() == 1 {
            " was made."
        } else {
            " were made."
        });
        push_sentence(&mut prose, sentence);
    }
    let many = others.len() > 1;
    if state == BoxState::Unticked {
        push_sentence(
            &mut prose,
            Prose::new().text(if many {
                "Unticked, they stay as ordinary branches."
            } else {
                "Unticked, it stays as an ordinary branch."
            }),
        );
    }
    prose
}

/// What the branch boxes say while the dialog is still asking git about them.
pub fn branches_checking_prose() -> Prose {
    Prose::new().text("Looking for unpushed work on the branches\u{2026}")
}

/// What git answered about the branches the boxes offer (`current`, the
/// first box's, when there is one, and `others`), said before anything is
/// ticked: whether deleting them would lose work that exists nowhere else.
/// The count is the one a tick of every offered box would lose, so it agrees
/// with the loss warning.
///
/// The commits are said to be on the branches whose own box's count is not
/// zero, or on every offered branch when only their union holds them. More
/// than two such branches are counted rather than named, and the second box
/// counts as a whole, since git counts it as one.
///
/// `None` when no box offers a branch (the agent is on the project's own), so
/// there is nothing to say and the surface shows no answer at all.
///
/// # Errors
///
/// The sentence saying git could not count the commits, when the count the
/// offered boxes would lose is an error.
pub fn branches_checked_prose(
    current: Option<&str>,
    current_lost: &UnpushedAnswer,
    others: &[&str],
    others_lost: &UnpushedAnswer,
    both_lost: &UnpushedAnswer,
) -> Result<Option<Prose>, Prose> {
    let offered: Vec<&str> = current.into_iter().chain(others.iter().copied()).collect();
    if offered.is_empty() {
        return Ok(None);
    }
    let lost = match (current, others.is_empty()) {
        (Some(_), false) => both_lost,
        (Some(_), true) => current_lost,
        (None, _) => others_lost,
    };
    let answer = match lost {
        Ok(answer) => *answer,
        Err(reason) => return Err(unpushed_count_failure_prose(&offered, reason)),
    };
    let no_remote = !answer.has_remote_refs;
    if answer.count == 0 {
        let mut prose = Prose::new().text(if no_remote {
            "Everything on "
        } else {
            "No unpushed work on "
        });
        match (offered.len(), no_remote) {
            (1 | 2, _) => push_list(
                &mut prose,
                &offered,
                if no_remote { " and " } else { " or " },
            ),
            (many, true) => prose.push_text(format!("the {many} branches")),
            (many, false) => prose.push_text(format!("any of the {many} branches")),
        }
        if no_remote {
            prose.push_text(" is also on another branch, and this repository has no remote");
        }
        return Ok(Some(prose));
    }
    let holds = |lost: &UnpushedAnswer| matches!(lost, Ok(lost) if lost.count > 0);
    let mut holding: Vec<&str> = current
        .filter(|_| holds(current_lost))
        .into_iter()
        .chain(others.iter().copied().filter(|_| holds(others_lost)))
        .collect();
    if holding.is_empty() {
        holding = offered;
    }
    let one = answer.count == 1;
    let mut prose = Prose::new().text(if one {
        "1 commit on ".to_string()
    } else {
        format!("{} commits on ", answer.count)
    });
    if holding.len() > 2 {
        prose.push_text(format!("{} branches", holding.len()));
    } else {
        push_list(&mut prose, &holding, " and ");
    }
    prose.push_text(match (no_remote, one) {
        (false, true) => " isn't pushed anywhere",
        (false, false) => " aren't pushed anywhere",
        (true, true) => " is on no other branch, and this repository has no remote",
        (true, false) => " are on no other branch, and this repository has no remote",
    });
    Ok(Some(prose))
}

/// What the branch boxes say when git could not be asked about them.
pub fn branches_check_failed_prose(reason: &str) -> Prose {
    Prose::new().text(format!(
        "dux could not check the branches ({reason}), so they will be kept."
    ))
}

/// What the branch boxes say when git answered about every branch but the one
/// the worktree is on, which is the only one kept on that account.
pub fn current_branch_check_failed_prose(branch: &str, reason: &str) -> Prose {
    Prose::new()
        .text("dux could not check ")
        .name(branch)
        .text(format!(" ({reason}), so it will be kept."))
}

/// The line naming branches the agent was on that are the project's own,
/// which no box offers and dux never deletes.
pub fn protected_branches_prose(branches: &[&str]) -> Prose {
    let mut prose = Prose::new();
    push_list(&mut prose, branches, " and ");
    prose.text(if branches.len() == 1 {
        " is the project's own branch, so dux keeps it."
    } else {
        " are the project's own branches, so dux keeps them."
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_offered_branch_leaves_nothing_to_say() {
        let none = Ok(crate::git::UnpushedCommits {
            count: 0,
            has_remote_refs: true,
        });
        assert!(matches!(
            branches_checked_prose(None, &none, &[], &none, &none),
            Ok(None)
        ));
    }
}
