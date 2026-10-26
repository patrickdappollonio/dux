//! The sentences both surfaces say about a working copy that is gone from disk,
//! and about putting one back.
//!
//! A managed agent can lose the directory it lives in: a coding CLI that merges
//! its own branch and deletes it takes the worktree with it, from inside. dux
//! cannot prevent that (its own worktree manager already refuses to remove an
//! occupied directory), so what is owed is an honest verdict and a way out.
//!
//! The copy lives here, in one place, because the changes panel, the agent row,
//! the info panel, the confirmation dialog and the status line all say it, on
//! two surfaces, and a second wording is a second thing that can be wrong.

use std::path::Path;

use anyhow::{Result, anyhow};

use crate::home_path::shorten_home;
use crate::prose::Prose;

/// What recreating a working copy did to the agent's branch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecreatedBranch {
    /// The branch was still in the repository and is checked out again. The
    /// agent's commits are exactly where it left them.
    CheckedOut,
    /// The branch was gone locally but still on the remote, so it was created
    /// again from the remote-tracking ref. It holds everything that had been
    /// pushed and nothing after that.
    RecreatedFromRemote(String),
    /// The branch was gone everywhere, so it was created again from the
    /// project's source branch. It holds none of the commits it held before.
    RecreatedFrom(String),
}

/// The refusal when something already occupies the folder dux was about to
/// check a branch out into: whatever is there is somebody's, so dux leaves it
/// alone. `then` says what the user can do about it.
pub(crate) fn occupied_folder_refusal(folder: &Path, then: &str) -> crate::status_text::StatusText {
    crate::status_text![
        q(shorten_home(folder)),
        format!(" already exists, so dux left it alone. {then}")
    ]
}

/// Put a managed working copy back at the path it already had, from the branch
/// it already names.
///
/// The SAME path is the whole point: the coding CLIs key their conversation
/// history by directory path, so a working copy recreated anywhere else leaves
/// the agent unable to resume the conversation the user is trying to get back
/// to. What is not coming back is the content: this is a checkout, not a
/// restore, and whatever was uncommitted in the deleted directory is gone.
///
/// Refuses rather than overwrites when something already occupies the path.
/// dux is putting a directory back, and a directory that is already there is
/// somebody's, whether a working copy that reappeared or an unrelated folder.
pub fn recreate_working_copy(
    repo_path: &Path,
    worktree_path: &Path,
    branch_name: &str,
    source_branch: &str,
) -> Result<RecreatedBranch> {
    match crate::git::directory_presence(worktree_path) {
        crate::git::DirectoryPresence::Missing => {}
        crate::git::DirectoryPresence::Present => {
            return Err(anyhow!(
                "{}",
                occupied_folder_refusal(
                    worktree_path,
                    "Move or remove it if you want dux to check the branch out there again."
                )
            ));
        }
        // The same fail-closed rule the verdict uses: a stat that failed for a
        // reason other than "nothing is there" means dux does not know whether
        // the old working copy is still sitting at this path, and checking a
        // branch out over it is not a guess dux may take.
        crate::git::DirectoryPresence::Indeterminate => {
            return Err(anyhow!(
                "dux could not read {}, so it does not know whether the old working copy is still \
                 there and left it alone. Check that the path is readable and that whatever it \
                 lives on is mounted, then try again.",
                shorten_home(worktree_path)
            ));
        }
    }
    // git still holds the registration of the directory the agent deleted, and
    // with it the branch as checked out, so both arms below fail without this.
    // Only THIS path's registration: a repository-wide prune takes any sibling
    // agent whose directory is unreachable at that instant with it.
    crate::git::forget_missing_worktree_registration(repo_path, worktree_path)?;
    // LOCAL only. `branch_exists` also answers Some for a branch that survives
    // only as a remote-tracking ref, and checking that out would put the agent
    // on a detached HEAD rather than on its branch. A branch that is gone
    // locally but still on the remote gets its own arm, because its pushed
    // commits are recoverable and starting again from the source branch would
    // throw them away without saying so.
    match crate::git::branch_exists(repo_path, branch_name) {
        Some(crate::git::BranchLocation::Local) => {
            crate::git::add_worktree_existing_branch_at(repo_path, worktree_path, branch_name)?;
            Ok(RecreatedBranch::CheckedOut)
        }
        Some(crate::git::BranchLocation::Remote) => {
            let remote_ref = format!("refs/remotes/origin/{branch_name}");
            crate::git::add_worktree_new_branch_at(
                repo_path,
                worktree_path,
                branch_name,
                Some(&remote_ref),
            )?;
            Ok(RecreatedBranch::RecreatedFromRemote(format!(
                "origin/{branch_name}"
            )))
        }
        None => {
            crate::git::add_worktree_new_branch_at(
                repo_path,
                worktree_path,
                branch_name,
                Some(source_branch),
            )?;
            Ok(RecreatedBranch::RecreatedFrom(source_branch.to_string()))
        }
    }
}

/// Why the changes region is quiet for a MANAGED agent whose working copy is
/// gone. Names the path, says what happened to a process still running there,
/// and says what has to happen before the agent can run again.
pub(crate) fn missing_working_copy_reason(worktree: &Path) -> String {
    format!(
        "The working copy at {} no longer exists on disk, so dux cannot show this agent's \
         changes. Anything still running in it keeps running in a directory that is gone, and \
         this agent cannot restart or resume here until the working copy is recreated.",
        shorten_home(worktree)
    )
}

/// Why the changes region is quiet for a managed agent whose working copy dux
/// could not stat at all.
///
/// Deliberately hedged, and deliberately not the missing sentence: an
/// unreadable parent, a stale handle and a mount that is down all fail the same
/// stat a deletion does, and telling the user their working copy was deleted
/// when it is sitting safely on a mount that is merely down would have them
/// recreate a branch that never needed recreating.
pub(crate) fn unreachable_working_copy_reason(worktree: &Path) -> String {
    format!(
        "dux could not read the working copy at {}, so it cannot say whether this agent has \
         changes. Check that the path is readable and that whatever it lives on is mounted, then \
         reopen this panel.",
        shorten_home(worktree)
    )
}

/// The same fact for a STANDALONE agent, whose directory is the user's own.
/// dux never creates, moves or removes that folder, so the remedy is the user's
/// rather than a button.
pub(crate) fn missing_folder_reason(folder: &Path) -> String {
    format!(
        "The folder {} no longer exists on disk, so dux cannot show this agent's changes. \
         Anything still running in it keeps running in a directory that is gone. Restore the \
         folder, or delete this agent and create a new one pointing at the folder you want.",
        shorten_home(folder)
    )
}

/// The short label the agent row and the info panel show, where there is room
/// for a marker rather than a sentence.
pub const MISSING_WORKING_COPY_LABEL: &str = "working copy missing";

/// The one line the info panel shows on both surfaces when a MANAGED agent's
/// working copy is gone.
///
/// Short rather than the changes panel's full sentence: the info panel already
/// names the worktree path on the row above, and a paragraph in a definition
/// list is a paragraph nobody reads. Pinned verbatim against the browser's copy,
/// like the confirmation body.
pub const MISSING_WORKING_COPY_INFO_LINE: &str =
    "The working copy no longer exists on disk. Recreate it to get this agent running again.";

/// The same line for a STANDALONE agent, whose folder is the user's own and
/// which dux will not put back.
pub const MISSING_FOLDER_INFO_LINE: &str =
    "The folder no longer exists on disk. Restore it, or delete this agent.";

/// Why the changes region is quiet for one agent, or `None` when it is not.
///
/// The one place both surfaces and the wire ask, so a directory that is gone
/// reads the same everywhere. `kind` picks which remedy the missing-directory
/// sentence offers, because dux can put its own working copy back and never
/// touches the user's own folder.
pub fn quiet_reason(
    status: crate::git::FolderRepoStatus,
    directory: &Path,
    kind: crate::model::AgentWorkspaceKind,
) -> Option<String> {
    if status.changes_panel_works() {
        return None;
    }
    let managed = kind == crate::model::AgentWorkspaceKind::Managed;
    Some(match status {
        crate::git::FolderRepoStatus::Missing if managed => missing_working_copy_reason(directory),
        crate::git::FolderRepoStatus::Missing => missing_folder_reason(directory),
        // A managed working copy dux could not stat is not "dux could not
        // consult git about this folder": nothing was asked of git at all.
        crate::git::FolderRepoStatus::Indeterminate if managed => {
            unreachable_working_copy_reason(directory)
        }
        other => other.quiet_reason().to_string(),
    })
}

/// The provider as a sentence names it: its own word with the first letter
/// capitalized, char-safe because a provider block's name is the user's text.
fn provider_name(provider: &str) -> String {
    let mut chars = provider.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Each provider once, in the order first met, as a sentence names it.
fn distinct_names(providers: &[String]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for provider in providers {
        let name = provider_name(provider);
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// Several provider names as one subject, each marked as a name, with `last`
/// before the final one: "Codex", "Codex or Opencode", "Codex, Opencode and
/// Copilot".
fn providers_joined(names: &[String], last: &str) -> Prose {
    let mut joined = Prose::new();
    for (index, name) in names.iter().enumerate() {
        if index > 0 {
            joined.push_text(if index == names.len() - 1 { last } else { ", " });
        }
        joined.push_name(name.clone());
    }
    joined
}

/// What one tab still running in a deleted working copy does once the copy is
/// back, decided per CLI.
///
/// Measured against the real CLIs. The Claude CLI never notices its directory
/// going, and once one exists at the same path again it keeps working in it with
/// its conversation intact. The Codex CLI answers every turn with "invalid cwd"
/// from the moment the directory goes and stays stuck there after the recreate,
/// until it is quit and resumed in the new folder. `OpenCode` and Copilot have not
/// been measured, so they get the cautious answer rather than a promise, and so
/// does any other CLI configured as a provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RunningTabFate {
    /// It follows the folder by itself: Claude.
    Follows,
    /// It cannot follow the folder, and its CLI picks its conversation back up
    /// in the same directory, so stopping and starting it again continues it.
    Restartable,
    /// It cannot follow the folder, and its CLI has no way to pick a
    /// conversation back up, so restarting it would lose the conversation.
    LeftRunning,
}

impl RunningTabFate {
    /// `command` is the file name of the provider's command, which names the
    /// CLI whatever the provider is called, and `resume` is whether the
    /// provider's configuration carries resume arguments, the same answer
    /// every launch asks.
    pub(crate) fn of(command: &str, resume: ConversationResume) -> Self {
        if command.eq_ignore_ascii_case("claude") {
            Self::Follows
        } else if resume == ConversationResume::Resumes {
            Self::Restartable
        } else {
            Self::LeftRunning
        }
    }
}

/// Whether a provider's CLI picks a conversation back up in the same directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversationResume {
    /// Its configuration carries resume arguments, so it continues.
    Resumes,
    /// It has no way to pick a conversation back up, so it starts fresh.
    StartsFresh,
}

/// The providers of the tabs running in a deleted working copy, one entry per
/// tab, sorted by what each does once the copy is back. The confirmation, its
/// restart checkbox and the final are all written from this, on both surfaces.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct RunningTabs {
    /// The providers of the tabs that follow the folder by themselves.
    pub follows: Vec<String>,
    /// The providers of the tabs that pick their conversation back up when restarted.
    pub restartable: Vec<String>,
    /// The providers of the tabs that are left running as they are.
    pub left_running: Vec<String>,
}

impl RunningTabs {
    /// Sort one running tab of `provider`, whose command's file name is
    /// `command`, by its fate.
    pub fn push(&mut self, provider: &str, command: &str, resume: ConversationResume) {
        let provider = provider.to_string();
        match RunningTabFate::of(command, resume) {
            RunningTabFate::Follows => self.follows.push(provider),
            RunningTabFate::Restartable => self.restartable.push(provider),
            RunningTabFate::LeftRunning => self.left_running.push(provider),
        }
    }

    /// Whether no tab is running there.
    pub fn is_empty(&self) -> bool {
        self.follows.is_empty() && self.restartable.is_empty() && self.left_running.is_empty()
    }
}

/// Sentences joined with one space.
fn sentences(parts: impl IntoIterator<Item = Prose>) -> Prose {
    let mut joined = Prose::new();
    for part in parts.into_iter().filter(|part| !part.is_empty()) {
        if !joined.is_empty() {
            joined.push_text(" ");
        }
        joined = joined.then(part);
    }
    joined
}

/// What the tabs still running in the deleted directory do once the working copy
/// is back, in the sentences the confirmation and the final both say.
///
/// Written from the tabs that are actually RUNNING, not the agent's slot-tab
/// mirror: a dormant claude slot beside a live codex extra is the stuck CLI, and
/// naming the mirror would promise the user the opposite of what happens. One
/// sentence per fate present, each naming every provider it is about.
pub fn recreate_running_tab_clause(tabs: &RunningTabs) -> String {
    recreate_running_tab_prose(tabs).plain()
}

/// [`recreate_running_tab_clause`] with each provider marked as a name.
fn recreate_running_tab_prose(tabs: &RunningTabs) -> Prose {
    let follows = (!tabs.follows.is_empty()).then(|| {
        Prose::new()
            .text("A running ")
            .then(providers_joined(&distinct_names(&tabs.follows), " or "))
            .text(" tab keeps working in the recreated copy by itself.")
    });
    let restartable = (!tabs.restartable.is_empty()).then(|| {
        Prose::new()
            .text("A running ")
            .then(providers_joined(&distinct_names(&tabs.restartable), " or "))
            .text(
                " tab cannot follow the folder by itself; restarting it continues its \
                 conversation in the recreated copy.",
            )
    });
    let left_running =
        (!tabs.left_running.is_empty()).then(|| left_running_prose(&tabs.left_running));
    sentences([follows, restartable, left_running].into_iter().flatten())
}

/// A running tab whose CLI cannot pick a conversation back up is never
/// restarted: the sentence says so, and what to do instead.
fn left_running_prose(providers: &[String]) -> Prose {
    Prose::new()
        .text("A running ")
        .then(providers_joined(&distinct_names(providers), " or "))
        .text(
            " tab is left running, because restarting it would lose its conversation; if it \
             cannot follow the folder, stop it and start it again yourself.",
        )
}

/// The label of the confirmation's restart checkbox, naming the providers of the
/// tabs it restarts: "Restart the Codex tab so it continues in the new copy",
/// "Restart the Codex and Opencode tabs so they continue in the new copy".
///
/// `restartable` holds one entry per tab, so two tabs of one provider read as
/// plural while the provider is named once. Pinned against the browser's
/// `recreateRestartCheckboxProse` by `tests/fixtures/prose_cross_language.json`.
pub fn recreate_restart_checkbox_prose(restartable: &[String]) -> Prose {
    let tail = if restartable.len() == 1 {
        " tab so it continues in the new copy"
    } else {
        " tabs so they continue in the new copy"
    };
    Prose::new()
        .text("Restart the ")
        .then(providers_joined(&distinct_names(restartable), " and "))
        .text(tail)
}

/// What recreating the working copy costs, said plainly in the confirmation.
///
/// The uncommitted work in the old directory is gone with the directory: dux is
/// checking the branch out again, not restoring what was deleted. The
/// conversation may survive because the coding CLIs key their history by
/// directory path, which is the whole reason the working copy is recreated at
/// the SAME path rather than a fresh one.
/// All three branch arms are stated because asking git which one applies would
/// run a subprocess to open a dialog. The final status names the arm that
/// actually ran.
///
/// `conversation` is the agent's provider's own answer rather than a
/// constant: a provider that keeps no directory-scoped history (copilot ships
/// with no resume arguments) would have this dialog promising something that
/// cannot happen, and the same path buys it nothing.
///
/// `running` are the tabs running right now, or the slot tab alone when none
/// is: the dialog is about to ask for a checkout under whatever is live, and
/// with nothing live it still says what starting one would mean.
pub fn recreate_confirm_body(
    worktree: &Path,
    branch_name: &str,
    source_branch: &str,
    conversation: ConversationResume,
    running: &RunningTabs,
) -> String {
    recreate_confirm_prose(worktree, branch_name, source_branch, conversation, running).plain()
}

/// [`recreate_confirm_body`] as prose, the path, every branch and every
/// provider marked so each surface draws them as chips. Pinned against the
/// browser's `recreateConfirmProse` by `tests/fixtures/prose_cross_language.json`.
pub fn recreate_confirm_prose(
    worktree: &Path,
    branch_name: &str,
    source_branch: &str,
    conversation: ConversationResume,
    running: &RunningTabs,
) -> Prose {
    let conversation = if conversation == ConversationResume::Resumes {
        "The conversation may resume, because the agent's CLI keys its history by directory path \
         and dux recreates the working copy at the same path."
    } else {
        "The conversation will not resume: this agent's CLI has no way to pick a conversation \
         back up, so it starts fresh wherever it runs."
    };
    Prose::new()
        .text("Recreate the working copy for this agent at ")
        .name(shorten_home(worktree))
        .text("?\n\nIf branch ")
        .quoted(branch_name)
        .text(
            " still exists locally, dux checks it out there again. If it is gone locally but \
             still on the remote, dux creates it again from ",
        )
        .quoted(format!("origin/{branch_name}"))
        .text(
            ", holding everything that had been pushed. If it is gone everywhere, dux creates \
             it again from ",
        )
        .quoted(source_branch)
        .text(
            ", and the commits that branch held are not coming back.\n\nAny code changes that \
             were in the old directory are gone either way: this puts the directory back, not \
             its contents. ",
        )
        .text(conversation)
        .text("\n\n")
        .then(recreate_running_tab_prose(running))
        .text(
            " A terminal still open in the old directory keeps working in a directory that is \
             gone; close it and open one in the recreated copy.",
        )
}

/// The busy sentence while the recreate runs.
pub(crate) fn recreate_busy_message(agent_label: &str) -> crate::status_text::StatusText {
    crate::status_text![
        "Recreating the working copy for agent ",
        q(agent_label),
        "..."
    ]
}

/// What dux did to the tabs it was asked to restart after a recreate, one entry
/// per tab, each the provider the tab started again with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TabRestartReport {
    /// Started again, picking its conversation back up.
    pub continued: Vec<String>,
    /// Started again with a fresh conversation, because another tab of the same
    /// CLI got the conversation back: only one can.
    pub fresh: Vec<String>,
    /// Not started again: closed, already running again, or refused by the
    /// time its old process had gone.
    pub(crate) not_restarted: Vec<String>,
    /// Not started again, because somebody stopped or closed the tab, or
    /// stopped its agent, while the old process shut down.
    pub stopped: Vec<String>,
    /// Started again to pick its conversation back up, which did not take, so
    /// it was started again with a fresh conversation.
    pub(crate) fell_back: Vec<String>,
    /// Started again to pick its conversation back up, and exited right away
    /// after saying something, which its tab shows.
    pub ended: Vec<String>,
    /// Started again, but the launch failed: the provider and why.
    pub failed: Vec<(String, String)>,
    /// Restartable tabs that came up while the copy was being recreated, after
    /// the restart was asked for, and so were left running.
    pub(crate) came_up_later: Vec<String>,
    /// Tabs the restart was asked for whose process exited while the copy was
    /// being recreated, so there was nothing left to restart.
    pub(crate) exited_during_checkout: Vec<String>,
    /// The wait the old processes were given, when any of them outlasted it and
    /// was force-closed.
    pub(crate) forced_after_seconds: Option<u64>,
    /// The agent was deleted while its tabs shut down, so nothing started.
    pub(crate) agent_deleted: bool,
}

impl TabRestartReport {
    /// Whether a tab dux was asked to restart is not running now for a reason
    /// the user has to act on.
    pub fn needs_attention(&self) -> bool {
        !self.failed.is_empty() || !self.ended.is_empty()
    }
}

/// Why a launch of a tab is refused while a working-copy recreate waits to
/// start it again.
pub(crate) fn restart_pending_launch_refusal(
    agent_label: &str,
    provider: &str,
) -> crate::status_text::StatusText {
    the_tabs("The ", &[provider.to_string()])
        .text(" of agent ")
        .quoted(agent_label)
        .text(
            " is being restarted after its working copy was recreated; it starts again by \
             itself once its old process has exited.",
        )
        .into()
}

/// The final of a recreate whose agent was deleted while its tabs were
/// shutting down to be restarted.
pub(crate) fn recreate_agent_deleted_message(agent_label: &str) -> crate::status_text::StatusText {
    crate::status_text![
        "Agent ",
        q(agent_label),
        " was deleted while dux was restarting its tabs in its recreated working copy, so \
         none of them was started again."
    ]
}

/// The spinner while dux waits for the tabs it is restarting to shut down.
pub(crate) fn recreate_restart_busy_message(
    agent_label: &str,
    restartable: &[String],
) -> crate::status_text::StatusText {
    let tail = if restartable.len() == 1 {
        " tab so it continues in the recreated copy…"
    } else {
        " tabs so they continue in the recreated copy…"
    };
    Prose::new()
        .text("Recreated the working copy for agent ")
        .quoted(agent_label)
        .text("; restarting its ")
        .then(providers_joined(&distinct_names(restartable), " and "))
        .text(tail)
        .into()
}

/// "the Codex tab" / "the Codex and Opencode tabs", for the restart report,
/// after `lead`.
fn the_tabs(lead: &str, providers: &[String]) -> Prose {
    Prose::new()
        .text(lead)
        .then(providers_joined(&distinct_names(providers), " and "))
        .text(if providers.len() == 1 {
            " tab"
        } else {
            " tabs"
        })
}

/// The restart half of the final: what happened to every tab dux restarted, in
/// words, beside the sentences about the tabs it left alone.
fn restart_report_prose(live: &RunningTabs, report: &TabRestartReport) -> Prose {
    let one = |tabs: &[String]| tabs.len() == 1;
    let follows = (!live.follows.is_empty()).then(|| {
        recreate_running_tab_prose(&RunningTabs {
            follows: live.follows.clone(),
            ..RunningTabs::default()
        })
    });
    let continued = (!report.continued.is_empty()).then(|| {
        the_tabs("Restarted the ", &report.continued).text(if one(&report.continued) {
            ", which continues its conversation in the recreated copy."
        } else {
            ", which continue their conversations in the recreated copy."
        })
    });
    let fresh = (!report.fresh.is_empty()).then(|| {
        the_tabs("The ", &report.fresh).text(if one(&report.fresh) {
            " started a fresh conversation instead, because only one tab per CLI can pick \
                 its conversation back up."
        } else {
            " started fresh conversations instead, because only one tab per CLI can pick \
                 its conversation back up."
        })
    });
    let not_restarted = (!report.not_restarted.is_empty()).then(|| {
        the_tabs("dux did not restart the ", &report.not_restarted).text(
            ", because the tab or its agent changed while the old process shut down; start \
             it yourself if you still want it running.",
        )
    });
    let fell_back = (!report.fell_back.is_empty()).then(|| {
        the_tabs("The ", &report.fell_back).text(if one(&report.fell_back) {
            " could not pick its conversation back up, so dux started it with a fresh \
             conversation."
        } else {
            " could not pick their conversations back up, so dux started them with fresh \
             conversations."
        })
    });
    let ended = (!report.ended.is_empty()).then(|| {
        the_tabs("The ", &report.ended).text(if one(&report.ended) {
            " started again but exited right away; its tab shows what it said."
        } else {
            " started again but exited right away; their tabs show what they said."
        })
    });
    let stopped = (!report.stopped.is_empty()).then(|| {
        the_tabs("dux did not restart the ", &report.stopped).text(if one(&report.stopped) {
            ", because it was stopped while its old process shut down."
        } else {
            ", because they were stopped while their old processes shut down."
        })
    });
    let failed = report.failed.iter().map(|(provider, error)| {
        the_tabs("dux could not start the ", std::slice::from_ref(provider)).text(format!(
            " again: {}. Start it yourself once that is fixed.",
            error.trim_end_matches('.')
        ))
    });
    let came_up_later = (!report.came_up_later.is_empty()).then(|| {
        let one = one(&report.came_up_later);
        the_tabs("The ", &report.came_up_later).text(if one {
            " that started while the copy was being recreated was left running, because it \
             started after the restart was asked for; restart it yourself to continue its \
             conversation in the recreated copy."
        } else {
            " that started while the copy was being recreated were left running, because they \
             started after the restart was asked for; restart them yourself to continue their \
             conversations in the recreated copy."
        })
    });
    let exited = (!report.exited_during_checkout.is_empty()).then(|| {
        the_tabs("The ", &report.exited_during_checkout).text(
            if one(&report.exited_during_checkout) {
                " stopped while the copy was being recreated, so dux did not restart it; start \
                 it again to continue its conversation in the recreated copy."
            } else {
                " stopped while the copy was being recreated, so dux did not restart them; \
                 start them again to continue their conversations in the recreated copy."
            },
        )
    });
    let forced = report.forced_after_seconds.map(|seconds| {
        Prose::new().text(format!(
            "An old process did not exit within {seconds} seconds and was force-closed first."
        ))
    });
    let left_running =
        (!live.left_running.is_empty()).then(|| left_running_prose(&live.left_running));
    sentences(
        [follows, continued, fresh, fell_back, ended]
            .into_iter()
            .flatten()
            .chain(failed)
            .chain(
                [
                    stopped,
                    not_restarted,
                    exited,
                    came_up_later,
                    forced,
                    left_running,
                ]
                .into_iter()
                .flatten(),
            ),
    )
}

/// The success sentence, naming the path and what actually happened to the
/// branch.
///
/// Takes the outcome rather than an optional start point so the three arms
/// cannot collapse into two: a branch rebuilt from the remote holds the agent's
/// own pushed commits, and a branch rebuilt from the source branch holds none
/// of them, which is not a difference to leave to a caller's `Option`.
///
/// `live` are the tabs running when the checkout landed: empty says nothing was
/// running and the tail is about dormant tabs, and anything else names what
/// those processes do next. `restart` is what dux did when the confirmation
/// asked it to restart the tabs that cannot follow the folder, and replaces
/// their sentence.
pub(crate) fn recreate_success_message(
    agent_label: &str,
    worktree: &Path,
    branch_name: &str,
    outcome: &RecreatedBranch,
    live: &RunningTabs,
    restart: Option<&TabRestartReport>,
) -> crate::status_text::StatusText {
    let branch_outcome = match outcome {
        RecreatedBranch::RecreatedFrom(source) => crate::status_text![
            "branch ",
            q(branch_name),
            " was recreated from ",
            q(source),
            ", so it holds none of the \
             commits it held before"
        ],
        RecreatedBranch::RecreatedFromRemote(remote) => crate::status_text![
            "branch ",
            q(branch_name),
            " was gone locally and was recreated from ",
            q(remote),
            ", so it \
             holds what had been pushed there and nothing committed after that"
        ],
        RecreatedBranch::CheckedOut => {
            crate::status_text!["branch ", q(branch_name), " was checked out again"]
        }
    };
    let tail: crate::status_text::StatusText = match restart {
        Some(report) => restart_report_prose(live, report).into(),
        None if live.is_empty() => {
            "Its tabs stay dormant; start one when you want the agent running there.".into()
        }
        None => recreate_running_tab_prose(live).into(),
    };
    crate::status_text![
        "Recreated the working copy for agent ",
        q(agent_label),
        " at ",
        n(shorten_home(worktree)),
        ": ",
        branch_outcome,
        ". ",
        tail
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Running tabs as the engine sorts them: each `(provider, resumes)` pair is
    /// one tab, `resumes` being whether its provider has resume arguments.
    fn running(tabs: &[(&str, bool)]) -> RunningTabs {
        let mut running = RunningTabs::default();
        for (provider, resumes) in tabs {
            running.push(
                provider,
                provider,
                if *resumes {
                    crate::working_copy::ConversationResume::Resumes
                } else {
                    crate::working_copy::ConversationResume::StartsFresh
                },
            );
        }
        running
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values
            .iter()
            .map(std::string::ToString::to_string)
            .collect()
    }

    /// A branch name can come from a pull request somebody else opened; an
    /// override inside it must not reorder the confirm's own words.
    #[test]
    fn the_recreate_confirm_draws_a_crafted_branch_without_bidi_controls() {
        let prose = recreate_confirm_prose(
            Path::new("/tmp/wt"),
            "feat\u{202E}txt.exe",
            "main",
            crate::working_copy::ConversationResume::Resumes,
            &running(&[("claude", true)]),
        );
        let plain = prose.plain();
        assert!(
            !plain.chars().any(crate::bidi::is_bidi_control),
            "{plain:?}"
        );
        assert!(plain.contains("\"feattxt.exe\""), "{plain}");
        assert!(plain.contains("\"origin/feattxt.exe\""), "{plain}");
    }

    #[test]
    fn the_missing_reason_names_the_path_and_never_says_busy() {
        let reason = missing_working_copy_reason(Path::new("/tmp/worktrees/p/v0"));
        assert!(reason.contains("/tmp/worktrees/p/v0"), "{reason}");
        assert!(!reason.to_lowercase().contains("busy"), "{reason}");
        assert!(reason.contains("recreated"), "{reason}");
    }

    #[test]
    fn the_folder_reason_points_at_the_user_rather_than_a_button() {
        let reason = missing_folder_reason(Path::new("/tmp/mine"));
        assert!(reason.contains("/tmp/mine"), "{reason}");
        assert!(reason.contains("Restore the folder"), "{reason}");
        assert!(!reason.to_lowercase().contains("busy"), "{reason}");
    }

    #[test]
    fn the_quiet_reason_picks_the_remedy_from_the_kind() {
        use crate::git::FolderRepoStatus;
        let managed = quiet_reason(
            FolderRepoStatus::Missing,
            Path::new("/tmp/wt"),
            crate::model::AgentWorkspaceKind::Managed,
        )
        .expect("a missing working copy is quiet");
        assert!(managed.contains("recreated"), "{managed}");
        let folder = quiet_reason(
            FolderRepoStatus::Missing,
            Path::new("/tmp/mine"),
            crate::model::AgentWorkspaceKind::Folder,
        )
        .expect("a missing folder is quiet");
        assert!(folder.contains("Restore the folder"), "{folder}");
        assert_eq!(
            quiet_reason(
                FolderRepoStatus::WorkingRepo,
                Path::new("/tmp/wt"),
                crate::model::AgentWorkspaceKind::Managed
            ),
            None,
            "a working repository is not quiet"
        );
        let no_repo = quiet_reason(
            FolderRepoStatus::NoRepo,
            Path::new("/tmp/mine"),
            crate::model::AgentWorkspaceKind::Folder,
        )
        .expect("a plain folder is quiet");
        assert_eq!(no_repo, FolderRepoStatus::NoRepo.quiet_reason());
    }

    /// Pinned verbatim against `lib/recreateWorkingCopy.ts`, which carries the
    /// twin assertion, so a wording change fails on the side that changed.
    #[test]
    fn the_confirm_reads_the_same_on_both_surfaces() {
        assert_eq!(
            recreate_confirm_body(
                Path::new("/worktrees/repo/feat"),
                "feat",
                "main",
                crate::working_copy::ConversationResume::Resumes,
                &running(&[("claude", true)])
            ),
            "Recreate the working copy for this agent at /worktrees/repo/feat?\n\n\
             If branch \"feat\" still exists locally, dux checks it out there again. If it is \
             gone locally but still on the remote, dux creates it again from \"origin/feat\", \
             holding everything that had been pushed. If it is gone everywhere, dux creates it \
             again from \"main\", and the commits that branch held are not coming \
             back.\n\nAny code changes that were in the old directory are gone either way: this \
             puts the directory back, not its contents. The conversation may resume, because the \
             agent's CLI keys its history by directory path and dux recreates the working copy at \
             the same path.\n\nA running Claude tab keeps working in the recreated copy by \
             itself. A terminal still open in the old directory keeps working \
             in a directory that is gone; close it and open one in the recreated copy."
        );
    }

    /// Each CLI is sorted by what it does with the recreated folder: Claude
    /// follows it, a CLI that can resume is restartable, and one that cannot is
    /// left running, whatever its name, a custom provider included.
    #[test]
    fn a_running_tab_is_sorted_by_its_cli() {
        assert_eq!(
            RunningTabFate::of("claude", crate::working_copy::ConversationResume::Resumes),
            RunningTabFate::Follows
        );
        assert_eq!(
            RunningTabFate::of(
                "Claude",
                crate::working_copy::ConversationResume::StartsFresh
            ),
            RunningTabFate::Follows
        );
        assert_eq!(
            RunningTabFate::of("codex", crate::working_copy::ConversationResume::Resumes),
            RunningTabFate::Restartable
        );
        assert_eq!(
            RunningTabFate::of("opencode", crate::working_copy::ConversationResume::Resumes),
            RunningTabFate::Restartable
        );
        assert_eq!(
            RunningTabFate::of(
                "copilot",
                crate::working_copy::ConversationResume::StartsFresh
            ),
            RunningTabFate::LeftRunning
        );
        assert_eq!(
            RunningTabFate::of("my-cli", crate::working_copy::ConversationResume::Resumes),
            RunningTabFate::Restartable
        );
        assert_eq!(
            RunningTabFate::of(
                "my-cli",
                crate::working_copy::ConversationResume::StartsFresh
            ),
            RunningTabFate::LeftRunning
        );
    }

    /// The running-tab paragraph is the measured truth per CLI, one sentence per
    /// fate: Claude follows the folder on its own, a CLI that can resume
    /// continues once restarted, and one that cannot is left running with what
    /// to do about it.
    #[test]
    fn the_clause_says_what_each_running_tab_does() {
        let clause = |tabs: &[(&str, bool)]| recreate_running_tab_clause(&running(tabs));
        assert_eq!(
            clause(&[("claude", true)]),
            "A running Claude tab keeps working in the recreated copy by itself."
        );
        assert_eq!(
            clause(&[("codex", true)]),
            "A running Codex tab cannot follow the folder by itself; restarting it continues its \
             conversation in the recreated copy."
        );
        assert_eq!(
            clause(&[("codex", true), ("opencode", true), ("codex", true)]),
            "A running Codex or Opencode tab cannot follow the folder by itself; restarting it \
             continues its conversation in the recreated copy."
        );
        assert_eq!(
            clause(&[("copilot", false)]),
            "A running Copilot tab is left running, because restarting it would lose its \
             conversation; if it cannot follow the folder, stop it and start it again yourself."
        );
        assert_eq!(
            clause(&[("claude", true), ("codex", true), ("copilot", false)]),
            "A running Claude tab keeps working in the recreated copy by itself. A running Codex \
             tab cannot follow the folder by itself; restarting it continues its conversation in \
             the recreated copy. A running Copilot tab is left running, because restarting it \
             would lose its conversation; if it cannot follow the folder, stop it and start it \
             again yourself."
        );
    }

    /// The checkbox names every provider it restarts once, and reads plural by
    /// tab count, so two tabs of one CLI are "the Codex tabs".
    #[test]
    fn the_restart_checkbox_names_the_providers_it_restarts() {
        let label = |providers: &[&str]| recreate_restart_checkbox_prose(&strings(providers));
        assert_eq!(
            label(&["codex"]).plain(),
            "Restart the Codex tab so it continues in the new copy"
        );
        assert_eq!(
            label(&["codex", "opencode"]).plain(),
            "Restart the Codex and Opencode tabs so they continue in the new copy"
        );
        assert_eq!(
            label(&["codex", "codex"]).plain(),
            "Restart the Codex tabs so they continue in the new copy"
        );
        let two = label(&["codex", "opencode"]);
        let names: Vec<&str> = two
            .segments()
            .iter()
            .filter_map(|segment| match segment {
                crate::prose::ProseSegment::Name { name, .. } => Some(name.as_str()),
                crate::prose::ProseSegment::Text(_) => None,
            })
            .collect();
        assert_eq!(names, vec!["Codex", "Opencode"], "the names are chips");
    }

    #[test]
    fn the_confirm_says_the_changes_are_gone_on_both_branch_outcomes() {
        let body = recreate_confirm_body(
            Path::new("/tmp/wt"),
            "feat",
            "main",
            crate::working_copy::ConversationResume::Resumes,
            &running(&[("claude", true)]),
        );
        assert!(body.contains("checks it out there again"), "{body}");
        assert!(body.contains("from \"origin/feat\""), "{body}");
        assert!(body.contains("from \"main\""), "{body}");
        assert!(body.contains("not coming back"), "{body}");
        assert!(body.contains("are gone either way"), "{body}");
        assert!(body.contains("same path"), "{body}");
        assert!(body.contains("A running Claude tab"), "{body}");

        // A provider that keeps no directory-scoped history is told so rather
        // than promised a resume the same path cannot buy it.
        let no_resume = recreate_confirm_body(
            Path::new("/tmp/wt"),
            "feat",
            "main",
            crate::working_copy::ConversationResume::StartsFresh,
            &running(&[("copilot", false)]),
        );
        assert!(no_resume.contains("will not resume"), "{no_resume}");
        assert!(!no_resume.contains("may resume"), "{no_resume}");
        assert!(no_resume.contains("are gone either way"), "{no_resume}");
    }

    /// The branch is still in the repository: the working copy comes back at the
    /// SAME path, which is what lets the CLI find its conversation again.
    #[test]
    fn a_surviving_branch_is_checked_out_again_at_the_same_path() {
        let (repo, worktree) = repo_with_a_deleted_worktree();
        let outcome = recreate_working_copy(repo.path(), &worktree, "feat", "main")
            .expect("the branch is still there");
        assert_eq!(outcome, RecreatedBranch::CheckedOut);
        assert!(worktree.join("seed.txt").exists(), "the checkout landed");
        assert!(
            worktree.join("on-the-branch.txt").exists(),
            "and it is the agent's branch, with its commits"
        );
    }

    /// The agent deleted its branch as well. The working copy still comes back,
    /// from the project's source branch, and the answer says so rather than
    /// implying the commits survived.
    #[test]
    fn a_deleted_branch_is_minted_again_from_the_source_branch() {
        let (repo, worktree) = repo_with_a_deleted_worktree();
        crate::git::forget_missing_worktree_registration(repo.path(), &worktree)
            .expect("forget the registration so the branch can be deleted");
        run_git(repo.path(), &["branch", "-D", "feat"]);

        let outcome = recreate_working_copy(repo.path(), &worktree, "feat", "main")
            .expect("the source branch is there");
        assert_eq!(outcome, RecreatedBranch::RecreatedFrom("main".to_string()));
        assert!(worktree.join("seed.txt").exists());
        assert!(
            !worktree.join("on-the-branch.txt").exists(),
            "a branch minted from main holds none of the agent's commits"
        );
    }

    /// The regression that made the prune targeted. Measured on git 2.55:
    /// `git worktree prune` removes EVERY registration whose directory is
    /// unreachable at that instant, so recreating one agent's working copy while
    /// a sibling's directory was merely renamed away severed the sibling
    /// permanently, and moving its directory back left `git status` in it
    /// answering "not a git repository".
    #[test]
    fn recreating_one_working_copy_leaves_a_sibling_on_an_unreachable_path_alone() {
        let (repo, worktree) = repo_with_a_deleted_worktree();
        let sibling = repo.path().join("wt").join("v1");
        crate::git::add_worktree_new_branch_at(repo.path(), &sibling, "other", Some("main"))
            .expect("the sibling agent's working copy");
        let stashed = repo.path().join("wt").join("v1-unreachable");
        std::fs::rename(&sibling, &stashed).expect("the sibling's mount goes away");

        recreate_working_copy(repo.path(), &worktree, "feat", "main").expect("the recreate runs");

        std::fs::rename(&stashed, &sibling).expect("the sibling's mount comes back");
        let listed = crate::git::test_support::git_command()
            .args(["-C", &repo.path().to_string_lossy(), "worktree", "list"])
            .output()
            .expect("git runs");
        let listing = String::from_utf8_lossy(&listed.stdout);
        assert!(
            listing.contains("[other]"),
            "the sibling's registration survived: {listing}"
        );
        let status = crate::git::test_support::git_command()
            .args(["-C", &sibling.to_string_lossy(), "status", "--porcelain=v1"])
            .output()
            .expect("git runs");
        assert!(
            status.status.success(),
            "and its directory still works: {}",
            String::from_utf8_lossy(&status.stderr)
        );
    }

    /// A branch that survives only as a remote-tracking ref is NOT checked out:
    /// `branch_exists` answers Some for one, and taking the checkout arm would
    /// leave the agent on a detached HEAD rather than on its branch. It gets its
    /// own arm, because the commits it pushed are recoverable and starting again
    /// from the source branch would throw them away silently.
    #[test]
    fn a_branch_that_survives_only_on_the_remote_is_recreated_from_the_remote() {
        let (repo, worktree) = repo_with_a_deleted_worktree();
        // Give the repository an origin pointing at itself, so the agent's
        // branch has somewhere to have been pushed, then delete it locally.
        run_git(
            repo.path(),
            &["remote", "add", "origin", &repo.path().to_string_lossy()],
        );
        run_git(repo.path(), &["fetch", "-q", "origin"]);
        crate::git::forget_missing_worktree_registration(repo.path(), &worktree)
            .expect("forget the registration");
        run_git(repo.path(), &["branch", "-D", "--", "feat"]);
        assert_eq!(
            crate::git::branch_exists(repo.path(), "feat"),
            Some(crate::git::BranchLocation::Remote),
            "precondition: the branch is gone locally and still on the remote"
        );

        let outcome = recreate_working_copy(repo.path(), &worktree, "feat", "main")
            .expect("the remote-tracking ref is there");
        assert_eq!(
            outcome,
            RecreatedBranch::RecreatedFromRemote("origin/feat".to_string())
        );
        assert!(
            worktree.join("on-the-branch.txt").exists(),
            "the pushed commits came back"
        );
        let head = crate::git::test_support::git_command()
            .args([
                "-C",
                &worktree.to_string_lossy(),
                "symbolic-ref",
                "--quiet",
                "--short",
                "HEAD",
            ])
            .output()
            .expect("git runs");
        assert_eq!(
            String::from_utf8_lossy(&head.stdout).trim(),
            "feat",
            "and the agent is on its branch, not a detached HEAD"
        );
    }

    /// Something already occupies the path. dux is putting a directory back, and
    /// a directory that is already there is somebody's.
    #[test]
    fn an_occupied_path_is_refused_rather_than_overwritten() {
        let (repo, worktree) = repo_with_a_deleted_worktree();
        std::fs::create_dir_all(&worktree).expect("occupy the path");
        std::fs::write(worktree.join("mine.txt"), "do not touch").expect("write");

        let err = recreate_working_copy(repo.path(), &worktree, "feat", "main")
            .expect_err("an occupied path is refused");
        let message = format!("{err:#}");
        assert!(message.contains("already exists"), "{message}");
        assert_eq!(
            std::fs::read_to_string(worktree.join("mine.txt")).expect("still there"),
            "do not touch"
        );
    }

    /// A stat dux could not get an answer to must not be treated as a deletion:
    /// checking a branch out over a working copy that is merely unreachable is
    /// exactly the loss this refusal exists to prevent.
    #[test]
    fn an_unreadable_path_is_refused_rather_than_checked_out_over() {
        use std::os::unix::fs::PermissionsExt;

        let (repo, _worktree) = repo_with_a_deleted_worktree();
        let locked = repo.path().join("locked");
        std::fs::create_dir_all(locked.join("inner")).expect("inner");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).expect("lock");
        let result = recreate_working_copy(repo.path(), &locked.join("inner"), "feat", "main");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).expect("unlock");

        let err = result.expect_err("a path dux cannot read is refused");
        let message = format!("{err:#}");
        assert!(message.contains("could not read"), "{message}");
        assert!(message.contains("mounted"), "{message}");
    }

    /// The hedged sentence belongs to the managed kind too: a standalone folder
    /// dux could not stat keeps git's own wording, which is about consulting
    /// git, while a managed working copy never asked git anything.
    #[test]
    fn an_unreachable_directory_gets_the_hedged_sentence_for_its_kind() {
        use crate::git::FolderRepoStatus;
        let managed = quiet_reason(
            FolderRepoStatus::Indeterminate,
            Path::new("/mnt/wt"),
            crate::model::AgentWorkspaceKind::Managed,
        )
        .expect("an unreachable working copy is quiet");
        assert!(managed.contains("/mnt/wt"), "{managed}");
        assert!(managed.contains("mounted"), "{managed}");
        assert!(!managed.contains("no longer exists"), "{managed}");

        let folder = quiet_reason(
            FolderRepoStatus::Indeterminate,
            Path::new("/mnt/mine"),
            crate::model::AgentWorkspaceKind::Folder,
        )
        .expect("an unreachable folder is quiet");
        assert_eq!(folder, FolderRepoStatus::Indeterminate.quiet_reason());
    }

    fn run_git(repo: &Path, args: &[&str]) {
        let out = crate::git::test_support::git_command()
            .args(args)
            .current_dir(repo)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A repository with a worktree whose directory has been deleted from under
    /// it, exactly as an agent that merges its branch and removes its own
    /// worktree leaves things.
    fn repo_with_a_deleted_worktree() -> (tempfile::TempDir, std::path::PathBuf) {
        let repo = tempfile::tempdir().expect("repo");
        run_git(repo.path(), &["init", "-q", "-b", "main", "."]);
        run_git(repo.path(), &["config", "user.email", "dux@example.com"]);
        run_git(repo.path(), &["config", "user.name", "dux"]);
        std::fs::write(repo.path().join("seed.txt"), "seed").expect("seed");
        run_git(repo.path(), &["add", "seed.txt"]);
        run_git(repo.path(), &["commit", "-qm", "seed"]);

        let worktree = repo.path().join("wt").join("v0");
        std::fs::create_dir_all(worktree.parent().expect("parent")).expect("worktrees root");
        crate::git::add_worktree_new_branch_at(repo.path(), &worktree, "feat", Some("main"))
            .expect("the agent's working copy");
        std::fs::write(worktree.join("on-the-branch.txt"), "work").expect("work");
        run_git(&worktree, &["add", "on-the-branch.txt"]);
        run_git(&worktree, &["commit", "-qm", "work"]);
        std::fs::remove_dir_all(&worktree).expect("the agent deletes its own working copy");
        (repo, worktree)
    }

    #[test]
    fn the_success_message_names_the_path_and_the_branch_outcome() {
        let checked_out = recreate_success_message(
            "v0",
            Path::new("/tmp/wt"),
            "feat",
            &RecreatedBranch::CheckedOut,
            &RunningTabs::default(),
            None,
        );
        assert!(checked_out.contains("/tmp/wt"), "{checked_out}");
        assert!(checked_out.contains("checked out again"), "{checked_out}");
        assert!(checked_out.contains("dormant"), "{checked_out}");
        let minted = recreate_success_message(
            "v0",
            Path::new("/tmp/wt"),
            "feat",
            &RecreatedBranch::RecreatedFrom("main".to_string()),
            &RunningTabs::default(),
            None,
        );
        assert!(minted.contains("recreated from \"main\""), "{minted}");
        assert!(minted.contains("none of the"), "{minted}");
        // The remote arm must not read like either of the other two: the pushed
        // commits are back, and only what was never pushed is gone.
        let from_remote = recreate_success_message(
            "v0",
            Path::new("/tmp/wt"),
            "feat",
            &RecreatedBranch::RecreatedFromRemote("origin/feat".to_string()),
            &RunningTabs::default(),
            None,
        );
        assert!(
            from_remote.contains("recreated from \"origin/feat\""),
            "{from_remote}"
        );
        assert!(
            from_remote.contains("what had been pushed"),
            "{from_remote}"
        );
        assert!(!from_remote.contains("none of the"), "{from_remote}");
    }

    /// With something running at recreate time the final says what that tab does
    /// next, per provider; with nothing running it says the tabs are dormant,
    /// which is the older sentence and still the true one.
    #[test]
    fn the_success_message_answers_for_a_tab_that_was_running() {
        let claude = recreate_success_message(
            "v0",
            Path::new("/tmp/wt"),
            "feat",
            &RecreatedBranch::CheckedOut,
            &running(&[("claude", true)]),
            None,
        );
        assert!(
            claude.ends_with(
                "branch \"feat\" was checked out again. A running Claude tab keeps working in the \
                 recreated copy by itself."
            ),
            "{claude}"
        );
        assert!(!claude.contains("dormant"), "{claude}");

        let codex = recreate_success_message(
            "v0",
            Path::new("/tmp/wt"),
            "feat",
            &RecreatedBranch::CheckedOut,
            &running(&[("codex", true)]),
            None,
        );
        assert!(
            codex.ends_with(
                "A running Codex tab cannot follow the folder by itself; restarting it continues \
                 its conversation in the recreated copy."
            ),
            "{codex}"
        );

        let idle = recreate_success_message(
            "v0",
            Path::new("/tmp/wt"),
            "feat",
            &RecreatedBranch::CheckedOut,
            &RunningTabs::default(),
            None,
        );
        assert!(
            idle.ends_with(
                "Its tabs stay dormant; start one when you want the agent running \
                            there."
            ),
            "{idle}"
        );
    }

    /// After a restart the final says, in words, which tabs continued, which
    /// started fresh because a sibling of the same CLI got the conversation,
    /// which were left running, and which dux did not get to.
    #[test]
    fn the_success_message_reports_every_restarted_tab() {
        let live = running(&[
            ("claude", true),
            ("codex", true),
            ("codex", true),
            ("copilot", false),
        ]);
        let report = TabRestartReport {
            continued: strings(&["codex"]),
            fresh: strings(&["codex"]),
            not_restarted: Vec::new(),
            forced_after_seconds: None,
            ..Default::default()
        };
        let message = recreate_success_message(
            "v0",
            Path::new("/tmp/wt"),
            "feat",
            &RecreatedBranch::CheckedOut,
            &live,
            Some(&report),
        );
        assert!(
            message.ends_with(
                "branch \"feat\" was checked out again. A running Claude tab keeps working in the \
                 recreated copy by itself. Restarted the Codex tab, which continues its \
                 conversation in the recreated copy. The Codex tab started a fresh conversation \
                 instead, because only one tab per CLI can pick its conversation back up. A \
                 running Copilot tab is left running, because restarting it would lose its \
                 conversation; if it cannot follow the folder, stop it and start it again \
                 yourself."
            ),
            "{message}"
        );

        let rest = TabRestartReport {
            continued: strings(&["codex", "opencode"]),
            fresh: Vec::new(),
            not_restarted: strings(&["opencode"]),
            forced_after_seconds: Some(30),
            ..Default::default()
        };
        let message = recreate_success_message(
            "v0",
            Path::new("/tmp/wt"),
            "feat",
            &RecreatedBranch::CheckedOut,
            &running(&[("codex", true), ("opencode", true), ("opencode", true)]),
            Some(&rest),
        );
        assert!(
            message.ends_with(
                "Restarted the Codex and Opencode tabs, which continue their conversations in the \
                 recreated copy. dux did not restart the Opencode tab, because the tab or its \
                 agent changed while the old process shut down; start it yourself if you still \
                 want it running. An old process did not exit within 30 seconds and was \
                 force-closed first."
            ),
            "{message}"
        );
    }

    #[test]
    fn the_restart_busy_names_the_tabs_it_waits_for() {
        assert_eq!(
            recreate_restart_busy_message("v0", &strings(&["codex", "opencode"])),
            "Recreated the working copy for agent \"v0\"; restarting its Codex and Opencode tabs \
             so they continue in the recreated copy…"
        );
    }
}
