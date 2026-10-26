//! Renaming an agent: the one implementation both surfaces call.
//!
//! [`Engine::prepare_branch_rename`] decides and writes the title;
//! [`Engine::rename_agent`] turns that decision into the status the user reads
//! and, when the git branch is renamed too, runs `git branch -m` on a
//! background worker whose completion lands as `BranchRenameCompleted`.

use std::path::Path;

use crate::prose::Prose;
use crate::status_text::StatusText;
use crate::worker::WorkerEvent;

use super::{
    BackgroundSpawn, BackgroundWorkerSpec, BranchRenameDispatch, BranchRenamePlan,
    BranchRenameRejection, Engine, EventReaction, Final, InFlightKey, ResolvedFinal, StatusUpdate,
    status_op,
};

/// The sentence a refused rename answers with, the same on every surface.
pub fn branch_rename_refusal(rejection: &BranchRenameRejection) -> &'static str {
    match rejection {
        BranchRenameRejection::EmptyName => {
            "Type the name to rename the git branch to. An empty name only resets the agent's \
             name to its branch name, so untick \"Also rename the git branch\" to do that."
        }
        BranchRenameRejection::MalformedName => crate::git::AGENT_NAME_RULE,
        BranchRenameRejection::AlreadyInFlight => {
            "A rename is already in progress for this agent. Wait for it to finish before \
             renaming again."
        }
        BranchRenameRejection::WorktreeBeingRemoved => {
            "This agent's worktree is being removed, so dux did not rename its branch. \
             Nothing new can start in a worktree while it is being removed."
        }
    }
}

/// The line under the rename dialog's "Also rename the git branch" box.
///
/// Renaming moves the branch on this machine only, so when the branch has an
/// open pull request or a pushed copy, the line says which of them stays on the
/// old name; otherwise it only says what the box does.
pub fn rename_branch_note_prose(branch: &str, open_pr: Option<u64>, pushed: BranchPushed) -> Prose {
    let pushed = pushed == BranchPushed::Pushed;
    let lead = Prose::new().text("Renames ").name(branch);
    match (open_pr, pushed) {
        (None, false) => lead.text(" to the new name."),
        (Some(number), pushed) => {
            let stays = Prose::new()
                .text(" on this machine only: pull request ")
                .name(format!("#{number}"));
            lead.then(stays).text(if pushed {
                " and the pushed branch stay on the old name."
            } else {
                " stays on the old name."
            })
        }
        (None, true) => {
            lead.text(" on this machine only: the pushed branch on the remote keeps the old name.")
        }
    }
}

/// What a rename does about the agent's git branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchRename {
    /// Rename the branch to the new name as well.
    AlsoTheBranch,
    /// Change only the agent's name.
    NameOnly,
}

/// Whether a branch has a pushed copy on the remote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchPushed {
    /// It has been pushed.
    Pushed,
    /// Nothing of it is on the remote.
    NotPushed,
}

fn branch_rename_failed(old_branch: &str, new_branch: &str, err: &str) -> StatusText {
    crate::status_text![
        "Could not rename git branch ",
        q(old_branch),
        " to ",
        q(new_branch),
        format!(", so the agent's name was put back: {err}")
    ]
}

impl Engine {
    /// The number of the OPEN pull request dux knows for this agent's branch,
    /// for the rename dialog's warning. From the pull-request sync dux already
    /// runs; nothing is asked of GitHub here.
    pub fn rename_open_pull_request(&self, session_id: &str) -> Option<u64> {
        self.pr_statuses
            .get(session_id)
            .filter(|pr| pr.state == crate::model::PrState::Open)
            .map(|pr| pr.number)
    }

    /// Where to ask whether this agent's branch has been pushed, and under
    /// which name: its worktree and branch, or `None` for an agent without one.
    pub fn rename_pushed_probe(&self, session_id: &str) -> Option<(String, String)> {
        let managed = self
            .sessions
            .iter()
            .find(|s| s.id == session_id)?
            .workspace
            .as_managed()?;
        Some((managed.worktree_path.clone(), managed.branch_name.clone()))
    }

    /// Rename an agent, and its git branch when `rename_branch` is set and the
    /// agent has one. An empty `name` resets the label to the branch name (a
    /// standalone agent's to its folder's name).
    ///
    /// Answers with the reaction to apply: a final for a rename that is already
    /// done, or the keyed busy of the branch rename now running. A refusal is an
    /// error carrying the sentence to show.
    pub fn rename_agent(
        &mut self,
        session_id: &str,
        name: &str,
        branch_rename: BranchRename,
    ) -> anyhow::Result<EventReaction> {
        let Some(branch) = self
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .map(|s| s.branch_name().map(str::to_string))
        else {
            anyhow::bail!("unknown session: {session_id}");
        };
        let scope = self.current_origin.clone();
        let status = match self.prepare_branch_rename(session_id, name, branch_rename) {
            BranchRenamePlan::Rejected(rejection) => {
                anyhow::bail!(branch_rename_refusal(&rejection))
            }
            BranchRenamePlan::Noop => return Ok(EventReaction::RebuildLeftItems),
            BranchRenamePlan::Unchanged => {
                let label = self
                    .sessions
                    .iter()
                    .find(|s| s.id == session_id)
                    .map(super::super::model::AgentSession::display_label)
                    .unwrap_or_default();
                crate::status_text![
                    "Agent ",
                    q(label),
                    " already has that name, so nothing changed."
                ]
            }
            BranchRenamePlan::NotSaved(err) => {
                anyhow::bail!("Could not save the agent's new name, so it keeps its old one: {err}")
            }
            BranchRenamePlan::RenameBranch(dispatch) => {
                return self.dispatch_branch_rename(dispatch);
            }
            BranchRenamePlan::TitleWritten {
                name,
                sync_branches,
            } => {
                if sync_branches {
                    self.update_branch_sync_sessions();
                }
                match branch {
                    Some(branch) if branch == name => crate::status_text![
                        "The git branch is already called ",
                        q(name),
                        ", so nothing was renamed in git. The agent shows its branch name."
                    ],
                    Some(branch) => crate::status_text![
                        "Renamed agent to ",
                        q(name),
                        ". Its git branch is still called ",
                        q(branch),
                        "."
                    ],
                    None => crate::status_text!["Renamed agent to ", q(name), "."],
                }
            }
            BranchRenamePlan::TitleCleared => {
                if let Some(branch) = branch {
                    crate::status_text![
                        "Cleared the custom name. Agent shows its branch name ",
                        q(branch),
                        " again."
                    ]
                } else {
                    // A standalone agent has no branch to fall back to; its
                    // label falls back to its folder's name instead.
                    let label = self
                        .sessions
                        .iter()
                        .find(|s| s.id == session_id)
                        .map(super::super::model::AgentSession::display_label)
                        .unwrap_or_default();
                    crate::status_text![
                        "Cleared the custom name. Agent shows its folder's name ",
                        q(label),
                        " again."
                    ]
                }
            }
        };
        // Loud on both surfaces: a rename lands on one row's label in a long
        // list, which can sit outside the visible scroll area, and a reset lands
        // on a fallback the user did not type and only this sentence names.
        Ok(EventReaction::Multi(vec![
            EventReaction::RebuildLeftItems,
            EventReaction::Status(StatusUpdate::info(status).with_scope(scope)),
        ]))
    }

    /// Run the git half of a rename the engine already prepared: a keyed busy,
    /// `git branch -m` off the engine thread, and `BranchRenameCompleted` with
    /// the final. A worker that never starts unwinds the optimistic title.
    fn dispatch_branch_rename(
        &mut self,
        dispatch: BranchRenameDispatch,
    ) -> anyhow::Result<EventReaction> {
        let BranchRenameDispatch {
            session_id,
            worktree_path,
            old_branch,
            new_branch,
            previous_title,
        } = dispatch;
        let scope = self.current_origin.clone();

        let success_old = old_branch.clone();
        let success_new = new_branch.clone();
        let failure_old = old_branch.clone();
        let failure_new = new_branch.clone();
        let op = status_op(crate::status_text![
            "Renaming agent and its git branch to ",
            q(new_branch),
            "\u{2026}"
        ])
        .on_success(move |(): &()| {
            Final::info(crate::status_text![
                "Renamed agent and its git branch to ",
                q(success_new),
                ". The branch was called ",
                q(success_old),
                "."
            ])
        })
        .on_failure(move |e: &String| {
            Final::error(branch_rename_failed(&failure_old, &failure_new, e))
        });
        let op_key = op.key().to_string();
        let pending = self.begin_status_op(&op).with_scope(scope.clone());

        // The panic path synthesises the same completion, so the title is
        // reverted and the in-flight marker and expectation are cleared even
        // when the worker unwinds.
        let panic_sid = session_id.clone();
        let panic_old = old_branch.clone();
        let panic_new = new_branch.clone();
        let panic_previous_title = previous_title.clone();
        let panic_scope = scope.clone();
        let revert_previous_title = previous_title.clone();
        let job_sid = session_id.clone();
        let spawned = self.spawn_background_worker(
            BackgroundWorkerSpec {
                label: format!("branch-rename[{session_id}]"),
                in_flight_key: Some(InFlightKey::BranchRename(session_id.clone())),
                panic_event: Some(Box::new(move |reason| WorkerEvent::BranchRenameCompleted {
                    session_id: panic_sid,
                    status: ResolvedFinal::error(
                        op_key,
                        branch_rename_failed(&panic_old, &panic_new, &reason),
                    )
                    .with_scope(panic_scope),
                    new_branch: panic_new,
                    previous_title: panic_previous_title,
                    result: Err(reason),
                })),
            },
            move |tx| {
                let result =
                    crate::git::rename_branch(Path::new(&worktree_path), &old_branch, &new_branch)
                        .map_err(|e| e.to_string());
                let status = op.resolve(&result).with_scope(scope);
                let _ = tx.send(WorkerEvent::BranchRenameCompleted {
                    session_id: job_sid,
                    new_branch,
                    previous_title,
                    result,
                    status,
                });
            },
        );
        if spawned != BackgroundSpawn::Spawned {
            // No completion will ever land, so nothing would answer the busy,
            // revert the title or release the expectation: do all of it here.
            if let Some(key) = pending.key.as_deref() {
                self.retire_status_key(key);
            }
            self.revert_optimistic_rename(&session_id, revert_previous_title);
            anyhow::bail!(
                "Could not start the branch-rename worker; reverted the agent name. \
                 Please try again."
            );
        }
        Ok(EventReaction::Multi(vec![
            EventReaction::RebuildLeftItems,
            EventReaction::Status(pending),
        ]))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::engine::test_support::{
        sample_project, sample_session, sample_standalone_session, test_engine,
    };
    use crate::statusline::{StatusScope, StatusTone};

    fn git(dir: &Path, args: &[&str]) {
        let out = crate::test_git::fixture_git()
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// The statuses a reaction carries, in order.
    fn statuses(reaction: &EventReaction) -> Vec<StatusUpdate> {
        match reaction {
            EventReaction::Status(update) => vec![update.clone()],
            EventReaction::Multi(items) => items.iter().flat_map(statuses).collect(),
            _ => Vec::new(),
        }
    }

    fn only_status(reaction: &EventReaction) -> StatusUpdate {
        let mut all = statuses(reaction);
        assert_eq!(all.len(), 1, "one status, got {all:?}");
        all.remove(0)
    }

    /// The whole branch rename through the one implementation: a keyed busy
    /// scoped to whoever asked, `git branch -m` on a worker, and a completion
    /// that moves the branch, leaves no custom title behind and answers the
    /// busy under the same key and scope.
    #[test]
    fn renaming_the_branch_runs_git_and_leaves_the_label_following_it() {
        let (mut engine, tmp) = test_engine();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "--initial-branch=old-name"]);
        git(&repo, &["config", "user.email", "t@example.com"]);
        git(&repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("f.txt"), "hi").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "init"]);
        engine
            .projects
            .push(sample_project("p1", repo.to_str().unwrap()));
        let mut session = sample_session("s1", "p1", "old-name");
        session.title = None;
        if let Some(managed) = session.workspace.as_managed_mut() {
            managed.worktree_path = repo.to_string_lossy().to_string();
        }
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);
        engine.current_origin = StatusScope::Connection("c1".into());

        let reaction = engine
            .rename_agent("s1", "new-name", crate::engine::BranchRename::AlsoTheBranch)
            .expect("the rename starts");
        let busy = only_status(&reaction);
        assert_eq!(busy.tone, StatusTone::Busy);
        assert_eq!(busy.scope, StatusScope::Connection("c1".into()));
        assert_eq!(
            busy.message,
            "Renaming agent and its git branch to \"new-name\"\u{2026}"
        );
        assert!(engine.is_in_flight(&InFlightKey::BranchRename("s1".into())));
        let again = engine
            .rename_agent("s1", "other", crate::engine::BranchRename::AlsoTheBranch)
            .expect_err("refused");
        assert_eq!(
            again.to_string(),
            branch_rename_refusal(&BranchRenameRejection::AlreadyInFlight)
        );

        engine.current_origin = StatusScope::All;
        let event = engine
            .worker_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the worker answers");
        let done = engine.process_worker_event(event);
        let fin = only_status(&done);
        assert_eq!(fin.tone, StatusTone::Info, "{}", fin.message);
        assert_eq!(fin.key, busy.key, "the final answers the busy");
        assert_eq!(fin.scope, StatusScope::Connection("c1".into()));
        assert_eq!(
            fin.message,
            "Renamed agent and its git branch to \"new-name\". The branch was called \"old-name\"."
        );
        assert_eq!(engine.sessions[0].branch_name(), Some("new-name"));
        assert_eq!(engine.sessions[0].title, None);
        assert!(crate::git::local_branch_exists(&repo, "new-name"));
        assert!(!engine.is_in_flight(&InFlightKey::BranchRename("s1".into())));
    }

    /// An engine with one agent on branch `old-name` in a real repository that
    /// also has a branch called `taken`, stored in the database.
    fn engine_with_a_repository() -> (Engine, crate::test_scratch::ScratchDir) {
        let (mut engine, tmp) = test_engine();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "--initial-branch=old-name"]);
        git(&repo, &["config", "user.email", "t@example.com"]);
        git(&repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("f.txt"), "hi").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "init"]);
        git(&repo, &["branch", "taken"]);
        engine
            .projects
            .push(sample_project("p1", repo.to_str().unwrap()));
        let mut session = sample_session("s1", "p1", "old-name");
        session.title = None;
        if let Some(managed) = session.workspace.as_managed_mut() {
            managed.worktree_path = repo.to_string_lossy().to_string();
        }
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);
        (engine, tmp)
    }

    /// The database refuses every later write to the agent's row.
    fn refuse_session_writes(engine: &Engine) {
        rusqlite::Connection::open(&engine.paths.sessions_db_path)
            .unwrap()
            .execute_batch(
                "create trigger refuse_session_update before update on agent_sessions \
                 begin select raise(abort, 'disk is full'); end",
            )
            .unwrap();
    }

    /// The rename's worker answers, and the reaction to its completion.
    fn rename_lands(engine: &mut Engine) -> EventReaction {
        let event = engine
            .worker_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the worker answers");
        engine.process_worker_event(event)
    }

    /// Git renamed the branch but dux could not save it: that is said, at a
    /// warning, with what to do, and the agent in memory stays what the
    /// database holds, so the branch check notices the new name and saves it
    /// once it can.
    #[test]
    fn a_branch_rename_dux_could_not_save_is_reported() {
        let (mut engine, _tmp) = engine_with_a_repository();
        engine
            .rename_agent("s1", "new-name", crate::engine::BranchRename::AlsoTheBranch)
            .expect("the rename starts");
        refuse_session_writes(&engine);

        let fin = only_status(&rename_lands(&mut engine));
        assert_eq!(fin.tone, StatusTone::Warning, "{}", fin.message);
        assert!(
            fin.message.contains(
                "Renamed git branch \"old-name\" to \"new-name\", but dux could not save the \
                 agent's new branch name: "
            ),
            "{}",
            fin.message
        );
        assert!(fin.message.contains("disk is full"), "{}", fin.message);
        let stored = engine.session_store.load_sessions().unwrap();
        assert_eq!(engine.sessions[0].branch_name(), stored[0].branch_name());
        assert_eq!(engine.sessions[0].title, stored[0].title);
        assert!(!engine.is_in_flight(&InFlightKey::BranchRename("s1".into())));
    }

    /// A branch rename git refused puts the old name back, and when dux cannot
    /// save that either, the final says the name stayed rather than claiming
    /// it was put back, and memory keeps what the database holds.
    #[test]
    fn a_name_dux_could_not_put_back_is_reported() {
        let (mut engine, _tmp) = engine_with_a_repository();
        engine
            .rename_agent("s1", "taken", crate::engine::BranchRename::AlsoTheBranch)
            .expect("the rename starts");
        refuse_session_writes(&engine);

        let fin = only_status(&rename_lands(&mut engine));
        assert_eq!(fin.tone, StatusTone::Error, "{}", fin.message);
        assert!(
            !fin.message.contains("was put back"),
            "nothing was put back: {}",
            fin.message
        );
        assert!(
            fin.message
                .contains("dux could not put the agent's old name back either"),
            "{}",
            fin.message
        );
        let stored = engine.session_store.load_sessions().unwrap();
        assert_eq!(engine.sessions[0].title, stored[0].title);
        assert_eq!(engine.sessions[0].title.as_deref(), Some("taken"));
    }

    /// Unticked, the branch is left alone and the sentence says so.
    #[test]
    fn a_title_only_rename_says_the_branch_kept_its_name() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let reaction = engine
            .rename_agent("s1", " shiny ", crate::engine::BranchRename::NameOnly)
            .unwrap();
        let status = only_status(&reaction);
        assert_eq!(status.tone, StatusTone::Info);
        assert_eq!(
            status.message,
            "Renamed agent to \"shiny\". Its git branch is still called \"feat\"."
        );
        assert_eq!(engine.sessions[0].title.as_deref(), Some("shiny"));
        assert_eq!(engine.sessions[0].branch_name(), Some("feat"));
        assert!(!engine.is_in_flight(&InFlightKey::BranchRename("s1".into())));
    }

    /// An empty name goes back to the branch name, or a standalone agent's
    /// folder name, and the sentence names whichever it landed on.
    #[test]
    fn an_empty_name_resets_to_the_branch_or_the_folder() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        engine
            .sessions
            .push(sample_standalone_session("sa1", "/home/someone/My Notes"));

        let status = only_status(
            &engine
                .rename_agent("s1", "  ", crate::engine::BranchRename::NameOnly)
                .unwrap(),
        );
        assert_eq!(engine.sessions[0].title, None);
        assert_eq!(
            status.message,
            "Cleared the custom name. Agent shows its branch name \"feat\" again."
        );

        // The box means nothing for an agent with no branch, so it is no reason
        // to refuse the reset.
        let status = only_status(
            &engine
                .rename_agent("sa1", "", crate::engine::BranchRename::AlsoTheBranch)
                .unwrap(),
        );
        assert_eq!(engine.sessions[1].title, None);
        assert_eq!(
            status.message,
            "Cleared the custom name. Agent shows its folder's name \"My-Notes\" again."
        );
    }

    /// An empty name cannot be a branch, so asking to rename the branch to it
    /// is refused and nothing changes.
    #[test]
    fn an_empty_name_with_the_branch_box_ticked_is_refused() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let err = engine
            .rename_agent("s1", " ", crate::engine::BranchRename::AlsoTheBranch)
            .expect_err("refused");
        assert_eq!(
            err.to_string(),
            branch_rename_refusal(&BranchRenameRejection::EmptyName)
        );
        assert_eq!(engine.sessions[0].title.as_deref(), Some("s1-title"));
    }

    /// Saving the dialog with nothing changed says so, rather than claiming a
    /// custom name was cleared that was never there, and writes nothing.
    #[test]
    fn saving_an_unchanged_name_says_nothing_changed() {
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat");
        session.title = None;
        engine.sessions.push(session);
        let mut named = sample_session("s2", "p1", "other");
        named.title = Some("custom".to_string());
        engine.sessions.push(named);

        for (id, name, rename_branch) in [
            ("s1", "", false),
            ("s1", "feat", false),
            ("s1", "feat", true),
            ("s2", "custom", false),
        ] {
            let status = only_status(
                &engine
                    .rename_agent(
                        id,
                        name,
                        if rename_branch {
                            crate::engine::BranchRename::AlsoTheBranch
                        } else {
                            crate::engine::BranchRename::NameOnly
                        },
                    )
                    .unwrap(),
            );
            assert_eq!(status.tone, StatusTone::Info);
            assert!(
                status
                    .message
                    .ends_with("already has that name, so nothing changed."),
                "{id} {name:?} {rename_branch}: {}",
                status.message
            );
        }
        assert_eq!(engine.sessions[0].title, None);
        assert_eq!(engine.sessions[1].title.as_deref(), Some("custom"));
    }

    /// Asking to rename the branch to the name it already has renames nothing
    /// in git, says so, and leaves the label following the branch.
    #[test]
    fn renaming_the_branch_to_its_own_name_renames_nothing() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let status = only_status(
            &engine
                .rename_agent("s1", "feat", crate::engine::BranchRename::AlsoTheBranch)
                .unwrap(),
        );
        assert_eq!(
            status.message,
            "The git branch is already called \"feat\", so nothing was renamed in git. \
             The agent shows its branch name."
        );
        assert_eq!(engine.sessions[0].title, None);
        assert!(!engine.is_in_flight(&InFlightKey::BranchRename("s1".into())));
    }

    /// Unticked, a name equal to the branch still leaves the label following
    /// the branch rather than pinned to a copy of its name, in memory and in
    /// the database, exactly as the ticked box does.
    #[test]
    fn a_name_equal_to_the_branch_stores_no_title_whichever_way_the_box_is() {
        for rename_branch in [false, true] {
            let (mut engine, _tmp) = test_engine();
            let session = sample_session("s1", "p1", "feat");
            engine.session_store.upsert_session(&session).unwrap();
            engine.sessions.push(session);

            let status = only_status(
                &engine
                    .rename_agent(
                        "s1",
                        "feat",
                        if rename_branch {
                            crate::engine::BranchRename::AlsoTheBranch
                        } else {
                            crate::engine::BranchRename::NameOnly
                        },
                    )
                    .unwrap(),
            );
            assert_eq!(status.tone, StatusTone::Info, "{rename_branch}");
            assert_eq!(engine.sessions[0].title, None, "{rename_branch}");
            let stored = engine.session_store.load_sessions().unwrap();
            assert_eq!(stored[0].title, None, "persisted, {rename_branch}");
        }
    }

    /// A name the database refuses to keep is not reported as a rename: the
    /// agent keeps its old name in memory too, and the refusal says why, for
    /// the title-only rename and for the branch rename alike.
    #[test]
    fn a_name_the_database_cannot_keep_is_refused_and_put_back() {
        for rename_branch in [false, true] {
            let (mut engine, _tmp) = test_engine();
            let session = sample_session("s1", "p1", "feat");
            engine.session_store.upsert_session(&session).unwrap();
            engine.sessions.push(session);
            rusqlite::Connection::open(&engine.paths.sessions_db_path)
                .unwrap()
                .execute_batch(
                    "create trigger refuse_session_update before update on agent_sessions \
                     begin select raise(abort, 'disk is full'); end",
                )
                .unwrap();

            let err = engine
                .rename_agent(
                    "s1",
                    "renamed",
                    if rename_branch {
                        crate::engine::BranchRename::AlsoTheBranch
                    } else {
                        crate::engine::BranchRename::NameOnly
                    },
                )
                .expect_err("refused");
            assert!(
                err.to_string().contains("disk is full"),
                "{rename_branch}: {err}"
            );
            assert_eq!(
                engine.sessions[0].title.as_deref(),
                Some("s1-title"),
                "{rename_branch}"
            );
            assert!(engine.rename_expected.is_empty(), "{rename_branch}");
            assert!(
                !engine.is_in_flight(&InFlightKey::BranchRename("s1".into())),
                "{rename_branch}"
            );
            assert!(
                engine.worktree_ops().holders("/tmp/s1-worktree").is_empty(),
                "the worktree is not left held, {rename_branch}"
            );
        }
    }

    /// A worker that never starts leaves nothing behind: the title is put back
    /// and no expectation or in-flight marker is left.
    #[test]
    fn a_branch_rename_that_never_starts_is_unwound() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let live_before = engine.live_status_keys.len();
        crate::engine::fail_next_worker_spawn();
        let err = engine
            .rename_agent("s1", "renamed", crate::engine::BranchRename::AlsoTheBranch)
            .expect_err("refused");
        assert!(err.to_string().contains("reverted the agent name"), "{err}");
        assert_eq!(
            engine.live_status_keys.len(),
            live_before,
            "no busy is left heartbeating"
        );
        assert_eq!(engine.sessions[0].title.as_deref(), Some("s1-title"));
        assert!(engine.rename_expected.is_empty());
        assert!(!engine.is_in_flight(&InFlightKey::BranchRename("s1".into())));
    }

    #[test]
    fn an_unknown_agent_is_an_unknown_session() {
        let (mut engine, _tmp) = test_engine();
        let err = engine
            .rename_agent("ghost", "x", crate::engine::BranchRename::NameOnly)
            .expect_err("refused");
        assert_eq!(err.to_string(), "unknown session: ghost");
    }
}
