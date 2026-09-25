//! Standalone background-job functions that spawn a CLI provider PTY for a
//! new or relaunching agent. Called from the App's
//! `dispatch_create_agent_request` and `dispatch_agent_launch` worker
//! threads; both functions post results back via `worker_tx`.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use chrono::Utc;
use uuid::Uuid;

use crate::config::{Config, DuxPaths, check_provider_available, provider_config};
use crate::model::{
    AgentSession, AgentWorkspace, BranchProvenance, FolderWorkspace, ManagedWorkspace,
    SessionStatus,
};
use crate::startup::{StartupCommandRun, run_startup_command};
use crate::worker::{
    AgentLaunchFailedData, AgentLaunchKind, AgentLaunchReadyData, AgentLaunchRequest,
    CreateAgentRequest, WorkerEvent,
};
use crate::{gh, git, logger};

/// What to do when the copy's HEAD-equality guard fails (the source checkout
/// and the new worktree are on different commits, so the status delta would
/// not describe the same base tree).
enum HeadMismatch {
    /// Skip the copy and append a visible note to the create status (fresh
    /// agents: the project checkout simply is not on that branch's commit).
    SkipWithNote { branch: String },
    /// Fail the creation (forks: equal HEADs hold by construction, so a
    /// mismatch means the source moved mid-create).
    Fail,
}

/// Roll back a create that failed after its worktree existed.
///
/// The worktree goes whenever dux made the directory (`owns_worktree`), but the
/// branch goes only when dux also minted it: attaching to `develop` makes dux the
/// owner of the directory and of nothing else. The provenance is read from the
/// session rather than carried a second time, so the rollback and the delete
/// cannot disagree about who owns the branch.
///
/// No drifted birth branch is ever passed: the agent was born moments ago and
/// cannot have moved.
///
/// It takes a [`ManagedWorkspace`], not a session, so a standalone agent's folder
/// cannot be handed to it: the user's folder must survive a failed create
/// untouched, and a caller holding a folder workspace cannot name the argument.
fn rollback_created_worktree(repo_path: &Path, managed: &ManagedWorkspace) {
    let worktree_path = Path::new(&managed.worktree_path);
    if managed.branch_provenance.dux_may_delete_branch() {
        let _ = git::remove_worktree(repo_path, worktree_path, &managed.branch_name, None);
    } else {
        let _ = git::remove_worktree_keep_branch(repo_path, worktree_path);
    }
}

/// A copy of uncommitted changes planned by a per-request arm and executed in
/// the common tail, after the provider availability check (so a missing
/// provider does not throw away completed copy work with the worktree).
struct PendingCopy {
    /// The project checkout, or the fork's source worktree.
    source: PathBuf,
    /// Short human description of the source for progress/error messages.
    source_desc: crate::status_text::StatusText,
    on_head_mismatch: HeadMismatch,
}

struct ManagedCreatePlan {
    project: crate::model::Project,
    provider: crate::model::ProviderKind,
    source_branch: String,
    status_message: crate::status_text::StatusText,
    /// Which surfaces withhold `status_message`. A create whose whole answer is
    /// "a row appeared and its pane launched" confirms nothing; one whose
    /// sentence carries a fact the screen never shows (what was copied, that a
    /// branch was attached rather than minted) stays loud.
    status_quiet: crate::statusline::QuietSurfaces,
    branch_name: String,
    worktree_path: PathBuf,
    owns_worktree: bool,
    title: Option<String>,
    launch_with_resume: bool,
    pending_copy: Option<PendingCopy>,
    /// A warning sentence that leads the final, ahead of the success line, and
    /// makes it a warning rather than an info: something the user must act on
    /// (a fresh copy that lacks commits the busy branch has).
    status_lead: Option<crate::status_text::StatusText>,
    /// The pull request to pin the agent to once it is committed; see
    /// [`crate::worker::PullRequestPin`].
    pull_request_pin: Option<Box<crate::worker::PullRequestPin>>,
    branch_provenance: BranchProvenance,
}

/// How many review names a fresh copy of a pull request tries before giving
/// up: `<branch>-review`, then `<branch>-review-2` up to this number. The limit
/// bounds the git calls the worker makes.
const REVIEW_NAME_LIMIT: u32 = 20;

/// The `n`th review name for `busy`: `<busy>-review`, `<busy>-review-2`, ...
fn review_name(busy: &str, n: u32) -> String {
    if n == 1 {
        format!("{busy}-review")
    } else {
        format!("{busy}-review-{n}")
    }
}

/// A pull request whose own branch is checked out elsewhere, so the new agent
/// gets a fresh copy of the pull request on a branch of its own instead.
struct FreshCopy {
    /// The pull request's branch, which another worktree holds.
    busy_branch: String,
    holder: git::BranchHolder,
    /// The review branch the copy is made on.
    new_branch: String,
}

impl FreshCopy {
    /// The holder's folder as shown to the user, home written as `~`.
    fn holder_label(&self) -> String {
        crate::home_path::shorten_home(&self.holder.worktree.path)
    }

    /// Where the busy branch is held, finishing a sentence that has just named
    /// it: a listed worktree whose folder is gone still reserves the branch
    /// until git prunes it, and the project folder is named as such.
    fn holder_clause(&self) -> crate::status_text::StatusText {
        let label = self.holder_label();
        if std::fs::symlink_metadata(&self.holder.worktree.path).is_err() {
            crate::status_text![
                " is still reserved by a working copy whose folder is gone (",
                q(label),
                "); ",
                n("git worktree prune"),
                " frees it."
            ]
        } else if self.holder.is_project_folder {
            crate::status_text![" is checked out in the project folder ", q(label), "."]
        } else {
            crate::status_text![" is checked out at ", q(label), "."]
        }
    }
}

/// What the busy-branch check decided for a pull-request create.
enum BusyBranchCheck {
    /// Nothing holds the chosen name (or the check could not run): today's
    /// attach-or-fetch path runs unchanged.
    Free,
    /// The pull request's own branch is held elsewhere: make a fresh copy.
    FreshCopy(FreshCopy),
    /// The create was refused and the failure already sent.
    Refused,
}

struct CreatePlanContext<'a> {
    paths: &'a DuxPaths,
    worker_tx: &'a Sender<WorkerEvent>,
    create_key: &'a str,
    creation_notes: &'a mut Vec<crate::status_text::StatusText>,
}

impl CreatePlanContext<'_> {
    fn plan(&mut self, request: CreateAgentRequest) -> Option<ManagedCreatePlan> {
        match request {
            CreateAgentRequest::Standalone { .. } => {
                logger::error("standalone create reached managed provisioning");
                self.send_failure(
                    "Could not create the standalone agent: dux took the wrong internal path. Nothing was created and no folder was touched. Please report this with the contents of dux.log."
                        .to_string(),
                );
                None
            }
            CreateAgentRequest::NewProject {
                project,
                custom_name,
                use_existing_branch,
                pull_before_create,
                copy_uncommitted_changes,
            } => self.plan_new_project(
                project,
                custom_name,
                use_existing_branch,
                pull_before_create,
                copy_uncommitted_changes,
            ),
            CreateAgentRequest::PullRequest {
                project,
                host,
                owner_repo,
                number,
                title,
                state,
                head_branch,
                custom_name,
                use_existing_branch,
            } => self.plan_pull_request(
                project,
                host,
                owner_repo,
                number,
                title,
                state,
                head_branch,
                custom_name,
                use_existing_branch,
            ),
            CreateAgentRequest::ForkSession {
                project,
                source_session,
                source_label,
                custom_name,
            } => self.plan_fork_session(project, source_session, source_label, custom_name),
            CreateAgentRequest::ExistingManagedWorktree {
                project,
                worktree_path,
                branch_name,
                custom_name,
            } => self.plan_existing_worktree(project, worktree_path, branch_name, custom_name),
            CreateAgentRequest::ForkExternalWorktree {
                project,
                source_worktree_path,
                source_label,
                source_branch,
                custom_name,
            } => self.plan_external_worktree(
                project,
                source_worktree_path,
                source_label,
                source_branch,
                custom_name,
            ),
        }
    }

    fn send_failure(&self, message: impl Into<crate::status_text::StatusText>) {
        let _ = self.worker_tx.send(WorkerEvent::CreateAgentFailed {
            status_op_id: self.create_key.to_string(),
            message: message.into(),
        });
    }

    fn send_progress(&self, message: impl Into<crate::status_text::StatusText>) {
        let _ = self.worker_tx.send(WorkerEvent::CreateAgentProgress {
            status_op_id: self.create_key.to_string(),
            message: message.into(),
        });
    }

    fn try_pre_create_pull(
        &mut self,
        project: &crate::model::Project,
        repo_path: &Path,
        leading_branch: &str,
    ) {
        self.send_progress(crate::status_text![
            "Pulling latest changes for project ",
            q(project.name),
            " before creating the agent..."
        ]);
        if let Err(err) = git::switch_branch_if_needed(repo_path, leading_branch) {
            logger::error(&format!(
                "pre-create branch switch failed for {}: {err}",
                project.path
            ));
            self.creation_notes.push(crate::status_text![
                "Warning: could not switch the project checkout to ",
                q(leading_branch),
                format!(": {}. The agent starts from the local branch state.", err)
            ]);
            return;
        }
        match git::has_origin_remote(repo_path) {
            // A base origin does not have (a project added on a local branch)
            // has nothing to pull; that is steady state, not a failure. Only
            // origin's own "no" skips: if origin cannot be asked, the pull
            // runs and reports the failure itself.
            Ok(true) if matches!(git::origin_has_branch(repo_path, leading_branch), Ok(false)) => {
                logger::info(&format!(
                    "skipping pre-create pull for {}: origin has no branch \"{leading_branch}\"",
                    project.path
                ));
            }
            Ok(true) => {
                if let Err(err) = git::pull_branch(repo_path, leading_branch) {
                    logger::error(&format!(
                        "pre-create pull failed for {}: {err}",
                        project.path
                    ));
                    self.creation_notes.push(crate::status_text![
                        "Warning: could not pull ",
                        q(leading_branch),
                        format!(
                            " from origin: {}. The agent starts from the local branch state.",
                            err
                        )
                    ]);
                }
            }
            Ok(false) => logger::info(&format!(
                "skipping pre-create pull for {}: no origin remote",
                project.path
            )),
            Err(err) => {
                logger::error(&format!(
                    "pre-create origin check failed for {}: {err}",
                    project.path
                ));
                self.creation_notes.push(format!(
                    "Warning: could not check for an origin remote: {err}. The agent starts from the local branch state."
                ).into());
            }
        }
    }

    fn create_new_project_worktree(
        &self,
        project: &crate::model::Project,
        repo_path: &Path,
        leading_branch: &str,
        resolved_name: &str,
        attach_existing: bool,
    ) -> Option<(String, PathBuf)> {
        let result = if attach_existing {
            git::create_worktree_existing_branch(
                repo_path,
                &self.paths.worktrees_root,
                &project.name,
                resolved_name,
            )
        } else {
            git::create_worktree_from_start_point(
                repo_path,
                &self.paths.worktrees_root,
                &project.name,
                Some(leading_branch),
                Some(resolved_name),
            )
        };
        match result {
            Ok(worktree) => Some(worktree),
            Err(err) => {
                let operation = if attach_existing {
                    "worktree creation (existing branch)"
                } else {
                    "worktree creation"
                };
                logger::error(&format!("{operation} failed for {}: {err}", project.path));
                let action = if attach_existing {
                    "attach to existing branch"
                } else {
                    "create a new worktree"
                };
                self.send_failure(crate::status_text![
                    format!("Failed to {} for project ", action),
                    q(project.name),
                    format!(": {}", err)
                ]);
                None
            }
        }
    }

    fn plan_new_project(
        &mut self,
        project: crate::model::Project,
        custom_name: Option<String>,
        use_existing_branch: bool,
        pull_before_create: bool,
        copy_uncommitted_changes: bool,
    ) -> Option<ManagedCreatePlan> {
        let repo_path = PathBuf::from(&project.path);
        let leading_branch = project.leading_branch.clone().unwrap_or_else(|| {
            let current =
                (!project.current_branch.is_empty()).then_some(project.current_branch.as_str());
            crate::project_browser::leading_branch_for_project(&repo_path, current)
        });
        if pull_before_create {
            self.try_pre_create_pull(&project, &repo_path, &leading_branch);
        }
        let title = custom_name.clone();
        let resolved_name = custom_name.unwrap_or_else(git::docker_style_name);
        let attach_existing =
            use_existing_branch || git::branch_exists(&repo_path, &resolved_name).is_some();
        if !attach_existing && git::repo_commit_state(&repo_path) == git::CommitState::Unborn {
            self.send_failure(crate::status_text!["Cannot create agent for ", q(project.name), ": the repository at ", n(repo_path.display()), " has no commits yet. Create an initial commit (for example with git commit --allow-empty -m \"Initial commit\"), then try again."]);
            return None;
        }
        if !attach_existing && !git::local_branch_exists(&repo_path, &leading_branch) {
            self.send_failure(crate::status_text![
                "Cannot create agent for ",
                q(project.name),
                ": leading branch ",
                q(leading_branch),
                " no longer exists locally. Restore that branch or re-add the project."
            ]);
            return None;
        }
        self.send_progress(if attach_existing {
            crate::status_text![
                "Attaching to existing branch ",
                q(resolved_name),
                " for project ",
                q(project.name),
                "..."
            ]
        } else {
            crate::status_text![
                "Creating a new worktree for project ",
                q(project.name),
                "..."
            ]
        });
        let (branch_name, worktree_path) = self.create_new_project_worktree(
            &project,
            &repo_path,
            &leading_branch,
            &resolved_name,
            attach_existing,
        )?;
        let status_message = if attach_existing {
            crate::status_text![
                "Attached to existing branch ",
                q(branch_name),
                " in project ",
                q(project.name),
                ". The worktree is ready in a fresh session."
            ]
        } else {
            crate::status_text![
                "Created ",
                n(project.default_provider.as_str()),
                " agent ",
                q(branch_name),
                " in project ",
                q(project.name),
                ". The new worktree is ready in a fresh session."
            ]
        };
        let pending_copy = copy_uncommitted_changes.then(|| PendingCopy {
            source: repo_path,
            source_desc: crate::status_text!["project ", q(project.name)],
            on_head_mismatch: HeadMismatch::SkipWithNote {
                branch: if attach_existing {
                    resolved_name
                } else {
                    leading_branch.clone()
                },
            },
        });
        Some(ManagedCreatePlan {
            provider: project.default_provider.clone(),
            source_branch: if attach_existing {
                project.current_branch.clone()
            } else {
                leading_branch
            },
            project,
            status_message,
            // Attaching to a branch that already existed is nowhere on screen,
            // so that arm stays loud. A plain create says only what the new row
            // and its streaming pane already say.
            status_quiet: if attach_existing {
                crate::statusline::QuietSurfaces::LOUD
            } else {
                crate::statusline::QuietSurfaces::BOTH
            },
            branch_name,
            worktree_path,
            owns_worktree: true,
            title,
            launch_with_resume: false,
            pending_copy,
            status_lead: None,
            pull_request_pin: None,
            branch_provenance: if attach_existing {
                BranchProvenance::AttachedExisting
            } else {
                BranchProvenance::CreatedByDux
            },
        })
    }

    /// Decide what to do when the chosen branch name is checked out somewhere,
    /// before today's attach-or-fetch path would hand it to `git worktree add`
    /// and relay git's refusal.
    ///
    /// Git reserves a branch for the worktree that has it checked out (the
    /// project folder included), for one in the middle of a rebase or a bisect
    /// that started from it, and for a listed worktree whose folder is gone. A
    /// busy name that is the pull request's own branch gets a fresh copy of the
    /// pull request on the first free review name; any other busy name is one
    /// the user typed, and filling `main-review` with the pull request's code
    /// would be wrong, so that is refused with the place it is checked out.
    ///
    /// A check that cannot run is logged and treated as "free": the check is an
    /// improvement on a path that works, never a new way to fail.
    fn check_busy_pr_branch(
        &self,
        project: &crate::model::Project,
        repo_path: &Path,
        resolved_name: &str,
        head_branch: &str,
        number: u64,
    ) -> BusyBranchCheck {
        let holder = match git::worktree_holding_branch(repo_path, resolved_name) {
            Ok(Some(holder)) => holder,
            Ok(None) => return BusyBranchCheck::Free,
            Err(err) => {
                logger::error(&format!(
                    "could not check whether branch \"{resolved_name}\" is checked out in {}: {err:#}",
                    project.path
                ));
                return BusyBranchCheck::Free;
            }
        };
        let holder_label = crate::home_path::shorten_home(&holder.worktree.path);
        if resolved_name != head_branch {
            self.send_failure(crate::status_text![
                "Branch ",
                q(resolved_name),
                " is checked out at ",
                q(holder_label),
                ". Choose another name for the agent from PR ",
                n(format!("#{}", number)),
                "."
            ]);
            return BusyBranchCheck::Refused;
        }
        let free = (1..=REVIEW_NAME_LIMIT)
            .map(|n| review_name(resolved_name, n))
            .find(|candidate| self.review_name_is_free(project, repo_path, candidate));
        let Some(new_branch) = free else {
            self.send_failure(crate::status_text![
                "Branch ",
                q(resolved_name),
                " is checked out at ",
                q(holder_label),
                " and every name up to ",
                q(review_name(resolved_name, REVIEW_NAME_LIMIT)),
                " is taken. Type another name for the agent from PR ",
                n(format!("#{}", number)),
                "."
            ]);
            return BusyBranchCheck::Refused;
        };
        logger::info(&format!(
            "branch \"{resolved_name}\" of PR #{number} is checked out at {}; making a fresh copy on \"{new_branch}\"",
            holder.worktree.path.display()
        ));
        BusyBranchCheck::FreshCopy(FreshCopy {
            busy_branch: resolved_name.to_string(),
            holder,
            new_branch,
        })
    }

    /// Whether a fresh copy can be made on branch `candidate`: no branch has
    /// the name, locally or on `origin`; no branch lives below it (git stores
    /// refs as paths, so `<candidate>/x` blocks `<candidate>`); and nothing is
    /// at the folder its worktree would get. Only a folder that is certainly
    /// absent is free, and a question that cannot be answered counts as
    /// taken: passing a free name over costs a `-2`, while using a name that
    /// was not free fails the create part-way.
    fn review_name_is_free(
        &self,
        project: &crate::model::Project,
        repo_path: &Path,
        candidate: &str,
    ) -> bool {
        if git::branch_exists(repo_path, candidate).is_some() {
            return false;
        }
        match git::refs_exist_below(repo_path, candidate) {
            Ok(false) => {}
            Ok(true) => return false,
            Err(err) => {
                logger::error(&format!(
                    "could not check for branches below \"{candidate}\" in {}; passing the name over: {err:#}",
                    project.path
                ));
                return false;
            }
        }
        let folder =
            git::managed_worktree_path(&self.paths.worktrees_root, &project.name, candidate);
        match std::fs::symlink_metadata(&folder) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => true,
            Ok(_) => false,
            Err(err) => {
                logger::error(&format!(
                    "could not check the folder {} for \"{candidate}\"; passing the name over: {err:#}",
                    folder.display()
                ));
                false
            }
        }
    }

    /// Say what a fresh copy is, once its worktree exists. Returns the
    /// warning sentence that leads the final, when there is one.
    ///
    /// The explanation rides as a creation note, which also makes the create
    /// speak on both surfaces: nothing on screen says the agent is a copy, or
    /// why. When the copy is behind the busy branch, the count of commits it
    /// lacks is the returned lead, which the job puts FIRST and which makes the
    /// final a warning. That the copy is linked to its pull request is said
    /// by the engine, which makes the link, and only once it is made.
    fn announce_fresh_copy(
        &mut self,
        repo_path: &Path,
        copy: &FreshCopy,
        number: u64,
    ) -> Option<crate::status_text::StatusText> {
        let missing = self.commits_the_copy_lacks(repo_path, copy);
        self.creation_notes.push(crate::status_text![
            "Fresh copy of PR ",
            n(format!("#{}", number)),
            " on its own branch ",
            q(copy.new_branch),
            ": ",
            q(copy.busy_branch),
            copy.holder_clause()
        ]);
        if missing == 0 {
            return None;
        }
        let commits = if missing == 1 { " commit" } else { " commits" };
        Some(crate::status_text![
            "This copy is missing ",
            n(missing),
            commits,
            " that ",
            q(copy.busy_branch),
            " has and GitHub does not show yet."
        ])
    }

    /// How many commits the busy branch has that its fresh copy lacks, counted
    /// only when the copy is BEHIND the busy branch (the copy's tip is an
    /// ancestor of it). Otherwise the two are different lines of work that
    /// share a name, such as a fork's pull request whose branch is called
    /// `main`, or a pull request that was force-pushed or rebased, and the
    /// commits one has and the other lacks are not "missing" from anything.
    /// Zero stands for "nothing to warn about", including when git cannot
    /// answer (logged).
    fn commits_the_copy_lacks(&self, repo_path: &Path, copy: &FreshCopy) -> u32 {
        match git::branch_is_ancestor(repo_path, &copy.new_branch, &copy.busy_branch) {
            Ok(true) => {}
            Ok(false) => {
                logger::info(&format!(
                    "fresh copy \"{}\" is not behind \"{}\", so no commits are counted as missing",
                    copy.new_branch, copy.busy_branch
                ));
                return 0;
            }
            Err(err) => {
                logger::error(&format!(
                    "could not tell whether fresh copy \"{}\" is behind \"{}\": {err:#}",
                    copy.new_branch, copy.busy_branch
                ));
                return 0;
            }
        }
        match git::commits_missing_from(repo_path, &copy.busy_branch, &copy.new_branch) {
            Ok(count) => count,
            Err(err) => {
                logger::error(&format!(
                    "could not count the commits \"{}\" has and its fresh copy \"{}\" lacks: {err:#}",
                    copy.busy_branch, copy.new_branch
                ));
                0
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn plan_pull_request(
        &mut self,
        project: crate::model::Project,
        host: String,
        owner_repo: String,
        number: u64,
        title: String,
        state: String,
        head_branch: String,
        custom_name: Option<String>,
        use_existing_branch: bool,
    ) -> Option<ManagedCreatePlan> {
        Some({
            let repo_path = PathBuf::from(&project.path);
            // A typed name is the agent's durable title; falling back to the PR
            // head branch means no user-authored name, so leave title empty.
            // (Named `agent_title` to avoid shadowing the PR `title` above.)
            let mut agent_title = custom_name.clone();
            let mut resolved_name = custom_name.unwrap_or_else(|| head_branch.clone());
            let fresh_copy = match self.check_busy_pr_branch(
                &project,
                &repo_path,
                &resolved_name,
                &head_branch,
                number,
            ) {
                BusyBranchCheck::Refused => return None,
                BusyBranchCheck::Free => None,
                BusyBranchCheck::FreshCopy(copy) => {
                    // The copy is named after the branch it actually got, so
                    // the sidebar does not show two rows with the busy name.
                    resolved_name = copy.new_branch.clone();
                    agent_title = Some(copy.new_branch.clone());
                    Some(copy)
                }
            };
            // Two separate questions. `attach_existing` decides how the worktree
            // is made: a name git already resolves is checked out rather than
            // re-fetched, which keeps a same-repo PR branch tracking origin.
            //
            // `local_branch_existed` decides whose branch it is. Against a
            // remote-only ref, `git worktree add` DWIMs `refs/heads/<name>` into
            // existence, so dux made that local branch and deleting the agent
            // takes it. Only a local branch that was there first is the user's,
            // and the ref on origin is never touched either way.
            //
            // A fresh copy's name was chosen because no branch has it, so the
            // copy always takes the fetch path and its branch is always dux's.
            let local_branch_existed = git::local_branch_exists(&repo_path, &resolved_name);
            let attach_existing = fresh_copy.is_none()
                && (use_existing_branch
                    || git::branch_exists(&repo_path, &resolved_name).is_some());

            if attach_existing {
                let _ = self.worker_tx.send(WorkerEvent::CreateAgentProgress {
                    status_op_id: self.create_key.to_string(),
                    message: crate::status_text![
                        "Attaching to existing branch ",
                        q(resolved_name),
                        " for PR ",
                        n(format!("#{}", number)),
                        " in project ",
                        q(project.name),
                        "..."
                    ],
                });
            } else {
                let _ = self.worker_tx.send(WorkerEvent::CreateAgentProgress {
                    status_op_id: self.create_key.to_string(),
                    message: crate::status_text![
                        "Fetching PR ",
                        n(format!("#{}", number)),
                        " from ",
                        n(owner_repo),
                        " into branch ",
                        q(resolved_name),
                        "..."
                    ],
                });
                if let Err(err) = git::fetch_pull_request_head(&repo_path, number, &resolved_name) {
                    logger::error(&format!(
                        "PR worktree fetch failed for {} #{}: {err}",
                        owner_repo, number
                    ));
                    let _ = self.worker_tx.send(WorkerEvent::CreateAgentFailed {
                        status_op_id: self.create_key.to_string(),
                        message: crate::status_text![
                            "Failed to fetch PR ",
                            n(format!("#{}", number)),
                            " from ",
                            n(owner_repo),
                            format!(": {}", err)
                        ],
                    });
                    return None;
                }
            }

            let (branch_name, worktree_path) = match git::create_worktree_existing_branch(
                &repo_path,
                &self.paths.worktrees_root,
                &project.name,
                &resolved_name,
            ) {
                Ok(result) => result,
                Err(err) => {
                    logger::error(&format!(
                        "PR worktree creation failed for {} #{}: {err}",
                        owner_repo, number
                    ));
                    // The responsibility boundary: no worktree exists, so the
                    // provenance-driven rollback will not run and nothing else
                    // would remove the ref the fetch minted. Past this point the
                    // rollback owns the branch and this cleanup must not run.
                    //
                    // Only when dux minted it: a local branch that was there
                    // first is the user's and survives the failure.
                    if !local_branch_existed {
                        git::delete_created_branch_best_effort(&repo_path, &resolved_name);
                    }
                    let _ = self.worker_tx.send(WorkerEvent::CreateAgentFailed {
                        status_op_id: self.create_key.to_string(),
                        message: crate::status_text![
                            "Failed to create a worktree for PR ",
                            n(format!("#{}", number)),
                            " in project ",
                            q(project.name),
                            format!(": {}", err)
                        ],
                    });
                    return None;
                }
            };
            let created = crate::status_text![
                "Created ",
                n(project.default_provider.as_str()),
                " agent ",
                q(branch_name),
                " from PR ",
                n(format!("#{}", number)),
                " (",
                n(title),
                ") in project ",
                q(project.name),
                "."
            ];
            let (status_lead, pull_request_pin) = match &fresh_copy {
                None => (None, None),
                Some(copy) => {
                    let lead = self.announce_fresh_copy(&repo_path, copy, number);
                    let pin = crate::worker::PullRequestPin {
                        host: host.clone(),
                        owner_repo: owner_repo.clone(),
                        number,
                        title: title.clone(),
                        state: state.clone(),
                        busy_branch: copy.busy_branch.clone(),
                        new_branch: copy.new_branch.clone(),
                    };
                    (lead, Some(Box::new(pin)))
                }
            };
            logger::info(&format!(
                "created PR worktree from {} #{} ({state}) {}",
                owner_repo,
                number,
                gh::pull_request_url(&host, &owner_repo, number)
            ));
            ManagedCreatePlan {
                project: project.clone(),
                provider: project.default_provider.clone(),
                source_branch: project.current_branch.clone(),
                status_message: created,
                // Quiet on both: the new row carries the pull-request chip and
                // its pane launches and streams. A fresh copy's note makes it
                // loud when the job folds the notes in.
                status_quiet: crate::statusline::QuietSurfaces::BOTH,
                branch_name,
                worktree_path,
                owns_worktree: true,
                title: agent_title,
                launch_with_resume: false,
                pending_copy: None,
                status_lead,
                pull_request_pin,
                // Whether the fetch minted `refs/heads/<name>` or the worktree
                // add DWIMed it out of `origin/<name>`, dux made the local branch
                // and it is dux's to clean up. Only a local branch that was
                // already there is the user's and stays.
                branch_provenance: if local_branch_existed {
                    BranchProvenance::AttachedExisting
                } else {
                    BranchProvenance::CreatedByDux
                },
            }
        })
    }

    fn plan_fork_session(
        &self,
        project: crate::model::Project,
        source_session: Box<AgentSession>,
        source_label: String,
        custom_name: Option<String>,
    ) -> Option<ManagedCreatePlan> {
        Some({
            let Some(custom_name) = custom_name else {
                let _ = self.worker_tx.send(WorkerEvent::CreateAgentFailed {
                    status_op_id: self.create_key.to_string(),
                    message: "Forking an agent requires choosing a name first."
                        .to_string()
                        .into(),
                });
                return None;
            };
            // Forking copies a managed worktree onto a new branch, so a
            // standalone agent has nothing to fork. The wire's ForkSession arm
            // already refuses one with a purposeful message; this is the
            // structural restatement, so no folder path can reach the branch
            // machinery below even if a future caller forgets that gate.
            let (Some(source_worktree), Some(source_branch_name)) = (
                source_session.managed_worktree().map(PathBuf::from),
                source_session.branch_name().map(str::to_string),
            ) else {
                let _ = self.worker_tx.send(WorkerEvent::CreateAgentFailed {
                            status_op_id: self.create_key.to_string(),
                            message: crate::status_text!["Agent ", q(source_label), " is a standalone agent, so there is no branch or managed worktree to fork. \
                                 Add its folder as a project if you want several agents working on it, and fork one of those instead."],
                        });
                return None;
            };
            let _ = self.worker_tx.send(WorkerEvent::CreateAgentProgress {
                status_op_id: self.create_key.to_string(),
                message: crate::status_text![
                    "Creating a forked worktree from agent ",
                    q(source_label),
                    "..."
                ],
            });
            let source_head = match git::head_commit(&source_worktree) {
                Ok(head) => head,
                Err(err) => {
                    logger::error(&format!(
                        "failed to resolve HEAD for {}: {err}",
                        source_worktree.display()
                    ));
                    let _ = self.worker_tx.send(WorkerEvent::CreateAgentFailed {
                        status_op_id: self.create_key.to_string(),
                        message: crate::status_text![
                            "Failed to inspect the source worktree for agent ",
                            q(source_label),
                            format!(": {}", err)
                        ],
                    });
                    return None;
                }
            };
            let repo_path = PathBuf::from(&project.path);
            let (branch_name, worktree_path) = match git::create_worktree_from_start_point(
                &repo_path,
                &self.paths.worktrees_root,
                &project.name,
                Some(&source_head),
                Some(&custom_name),
            ) {
                Ok(result) => result,
                Err(err) => {
                    logger::error(&format!(
                        "fork worktree creation failed for {}: {err}",
                        project.path
                    ));
                    let _ = self.worker_tx.send(WorkerEvent::CreateAgentFailed {
                        status_op_id: self.create_key.to_string(),
                        message: crate::status_text![
                            "Failed to create a forked worktree from agent ",
                            q(source_label),
                            format!(": {}", err)
                        ],
                    });
                    return None;
                }
            };
            let status_message = crate::status_text![
                "Forked ",
                n(source_session.provider.as_str()),
                " agent ",
                q(branch_name),
                " from ",
                q(source_label),
                " in project ",
                q(project.name),
                ". The new worktree starts with the copied uncommitted and untracked changes (gitignored files are not copied) and a fresh session."
            ];
            // Equal HEADs hold by construction (the worktree was just created
            // from `source_head`); the copy itself runs in the common tail.
            let pending_copy = Some(PendingCopy {
                source: source_worktree,
                source_desc: crate::status_text!["agent ", q(source_label)],
                on_head_mismatch: HeadMismatch::Fail,
            });
            ManagedCreatePlan {
                project,
                provider: source_session.provider,
                source_branch: source_branch_name,
                status_message,
                // Loud: what a fork copied, and what it left behind, is nowhere
                // on screen.
                status_quiet: crate::statusline::QuietSurfaces::LOUD,
                branch_name,
                worktree_path,
                owns_worktree: true,
                // A fork always requires a chosen name; persist it as the
                // agent's durable title.
                title: Some(custom_name),
                launch_with_resume: false,
                pending_copy,
                status_lead: None,
                pull_request_pin: None,
                // A fork always creates a new branch off the source's HEAD.
                branch_provenance: BranchProvenance::CreatedByDux,
            }
        })
    }

    fn plan_existing_worktree(
        &self,
        project: crate::model::Project,
        worktree_path: PathBuf,
        branch_name: String,
        custom_name: Option<String>,
    ) -> Option<ManagedCreatePlan> {
        Some({
            let agent_name = custom_name.clone().unwrap_or_else(|| branch_name.clone());
            let _ = self.worker_tx.send(WorkerEvent::CreateAgentProgress {
                status_op_id: self.create_key.to_string(),
                message: crate::status_text![
                    "Launching ",
                    n(project.default_provider.as_str()),
                    " in existing worktree ",
                    q(worktree_path.display()),
                    "..."
                ],
            });
            let status_message = crate::status_text![
                "Imported ",
                n(project.default_provider.as_str()),
                " agent ",
                q(agent_name),
                " from existing managed worktree for project ",
                q(project.name),
                "."
            ];
            ManagedCreatePlan {
                project: project.clone(),
                provider: project.default_provider.clone(),
                source_branch: branch_name.clone(),
                status_message,
                // Quiet on both: the new row appears and its pane launches.
                status_quiet: crate::statusline::QuietSurfaces::BOTH,
                branch_name,
                worktree_path,
                owns_worktree: false,
                title: custom_name,
                launch_with_resume: true,
                pending_copy: None,
                status_lead: None,
                pull_request_pin: None,
                // Adopting an existing worktree adopts its branch too: it
                // predates the agent and is not dux's to delete.
                branch_provenance: BranchProvenance::Adopted,
            }
        })
    }

    fn plan_external_worktree(
        &self,
        project: crate::model::Project,
        source_worktree_path: PathBuf,
        source_label: String,
        source_branch: String,
        custom_name: Option<String>,
    ) -> Option<ManagedCreatePlan> {
        Some({
            let _ = self.worker_tx.send(WorkerEvent::CreateAgentProgress {
                status_op_id: self.create_key.to_string(),
                message: crate::status_text![
                    "Creating a managed worktree from external worktree ",
                    q(source_label),
                    "..."
                ],
            });
            let source_head = match git::head_commit(&source_worktree_path) {
                Ok(head) => head,
                Err(err) => {
                    logger::error(&format!(
                        "failed to resolve HEAD for {}: {err}",
                        source_worktree_path.display()
                    ));
                    let _ = self.worker_tx.send(WorkerEvent::CreateAgentFailed {
                        status_op_id: self.create_key.to_string(),
                        message: crate::status_text![
                            "Failed to inspect external worktree ",
                            q(source_label),
                            format!(": {}", err)
                        ],
                    });
                    return None;
                }
            };
            let repo_path = PathBuf::from(&project.path);
            let (branch_name, worktree_path) = match git::create_worktree_from_start_point(
                &repo_path,
                &self.paths.worktrees_root,
                &project.name,
                Some(&source_head),
                custom_name.as_deref(),
            ) {
                Ok(result) => result,
                Err(err) => {
                    logger::error(&format!(
                        "external worktree fork creation failed for {}: {err}",
                        project.path
                    ));
                    let _ = self.worker_tx.send(WorkerEvent::CreateAgentFailed {
                        status_op_id: self.create_key.to_string(),
                        message: crate::status_text![
                            "Failed to create a managed worktree from external worktree ",
                            q(source_label),
                            format!(": {}", err)
                        ],
                    });
                    return None;
                }
            };
            let status_message = crate::status_text![
                "Created ",
                n(project.default_provider.as_str()),
                " agent ",
                q(branch_name),
                " from external worktree ",
                q(source_label),
                " in project ",
                q(project.name),
                ". Uncommitted and untracked changes were copied into the managed worktree (gitignored files are not copied)."
            ];
            // Equal HEADs hold by construction (the worktree was just created
            // from the external worktree's head); the copy runs in the tail.
            let pending_copy = Some(PendingCopy {
                source: source_worktree_path,
                source_desc: crate::status_text!["external worktree ", q(source_label)],
                on_head_mismatch: HeadMismatch::Fail,
            });
            ManagedCreatePlan {
                project: project.clone(),
                provider: project.default_provider.clone(),
                source_branch,
                status_message,
                // Loud: the copy rule, and what it skipped, is invisible.
                status_quiet: crate::statusline::QuietSurfaces::LOUD,
                branch_name,
                worktree_path,
                owns_worktree: true,
                // A typed name becomes the agent's durable title; None leaves the
                // display tracking the branch (an auto-derived worktree name).
                title: custom_name,
                launch_with_resume: false,
                pending_copy,
                status_lead: None,
                pull_request_pin: None,
                // The managed worktree is created here on a new branch; the
                // external worktree it was seeded from is untouched.
                branch_provenance: BranchProvenance::CreatedByDux,
            }
        })
    }
}

enum CopyHeadCheck {
    Equal,
    Different,
    Failed(anyhow::Error),
}

fn compare_copy_heads(source: &Path, destination: &Path) -> CopyHeadCheck {
    match (git::head_commit(source), git::head_commit(destination)) {
        (Ok(source_head), Ok(destination_head)) if source_head == destination_head => {
            CopyHeadCheck::Equal
        }
        (Ok(_), Ok(_)) => CopyHeadCheck::Different,
        (Err(error), _) | (_, Err(error)) => CopyHeadCheck::Failed(error),
    }
}

fn rollback_managed_create(repo_path: &Path, session: &AgentSession, owns_worktree: bool) {
    if owns_worktree && let Some(managed) = session.workspace.as_managed() {
        rollback_created_worktree(repo_path, managed);
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_copy_mismatch(
    copy: PendingCopy,
    check_error: Option<anyhow::Error>,
    session: &AgentSession,
    repo_path: &Path,
    owns_worktree: bool,
    worker_tx: &Sender<WorkerEvent>,
    create_key: &str,
    creation_notes: &mut Vec<crate::status_text::StatusText>,
) -> bool {
    match copy.on_head_mismatch {
        HeadMismatch::SkipWithNote { branch } => {
            creation_notes.push(match check_error {
                Some(error) => format!(
                    "Uncommitted changes were not copied: could not verify the checkout's commit: {error}."
                ).into(),
                None => crate::status_text!["Uncommitted changes were not copied: the project checkout is not on ", q(branch), "'s commit."],
            });
            true
        }
        HeadMismatch::Fail => {
            if check_error.is_none() {
                logger::error(&format!(
                    "uncommitted-changes copy aborted: {} is no longer on the commit {} was created from",
                    copy.source.display(),
                    session.directory()
                ));
            }
            rollback_managed_create(repo_path, session, owns_worktree);
            let message = match check_error {
                Some(error) => crate::status_text![
                    "Failed to copy uncommitted changes from ",
                    &copy.source_desc,
                    format!(
                        ": could not verify the source worktree's commit: {}.",
                        error
                    )
                ],
                None => crate::status_text![
                    "Failed to copy uncommitted changes from ",
                    &copy.source_desc,
                    ": the source moved to a different commit during creation."
                ],
            };
            let _ = worker_tx.send(WorkerEvent::CreateAgentFailed {
                status_op_id: create_key.to_string(),
                message,
            });
            false
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_pending_copy(
    copy: PendingCopy,
    session: &AgentSession,
    repo_path: &Path,
    owns_worktree: bool,
    worker_tx: &Sender<WorkerEvent>,
    create_key: &str,
    creation_notes: &mut Vec<crate::status_text::StatusText>,
) -> bool {
    let _ = worker_tx.send(WorkerEvent::CreateAgentProgress {
        status_op_id: create_key.to_string(),
        message: crate::status_text![
            "Copying uncommitted and untracked changes from ",
            &copy.source_desc,
            " into the new worktree (gitignored files are not copied)..."
        ],
    });
    let worktree = Path::new(session.directory());
    match compare_copy_heads(&copy.source, worktree) {
        CopyHeadCheck::Equal => match git::copy_uncommitted_changes(&copy.source, worktree) {
            Ok(summary) => {
                if !summary.skipped_paths.is_empty() {
                    creation_notes.push(format!(
                        "Some paths were not copied (submodules, embedded repositories, or special files): {}.",
                        summary.skipped_paths.join(", ")
                    ).into());
                }
                true
            }
            Err(error) => {
                logger::error(&format!(
                    "failed to copy uncommitted changes from {} into {}: {error}",
                    copy.source.display(),
                    session.directory()
                ));
                rollback_managed_create(repo_path, session, owns_worktree);
                let _ = worker_tx.send(WorkerEvent::CreateAgentFailed {
                    status_op_id: create_key.to_string(),
                    message: crate::status_text![
                        "Failed to copy uncommitted changes from ",
                        &copy.source_desc,
                        format!(": {}", error)
                    ],
                });
                false
            }
        },
        CopyHeadCheck::Different => handle_copy_mismatch(
            copy,
            None,
            session,
            repo_path,
            owns_worktree,
            worker_tx,
            create_key,
            creation_notes,
        ),
        CopyHeadCheck::Failed(error) => {
            logger::error(&format!(
                "uncommitted-changes copy: could not resolve HEAD for {} or {}: {error}",
                copy.source.display(),
                session.directory()
            ));
            handle_copy_mismatch(
                copy,
                Some(error),
                session,
                repo_path,
                owns_worktree,
                worker_tx,
                create_key,
                creation_notes,
            )
        }
    }
}

/// Launch a standalone agent in a caller-owned folder using only global env.
/// It performs no worktree provisioning, and rollback never touches the folder.
#[allow(clippy::too_many_arguments)]
fn run_create_standalone_agent_job(
    folder: PathBuf,
    title: String,
    provider: crate::model::ProviderKind,
    _paths: DuxPaths,
    config: Config,
    worker_tx: Sender<WorkerEvent>,
    term_size: (u16, u16),
    create_key: String,
    identity: crate::term_identity::TerminalIdentity,
) {
    let folder_label = crate::home_path::shorten_home(&folder);
    if !folder.is_dir() {
        let _ = worker_tx.send(WorkerEvent::CreateAgentFailed {
            status_op_id: create_key,
            message: crate::status_text![
                "Cannot create a standalone agent in ",
                q(folder_label),
                ": that folder does not \
                 exist, or is not a directory dux can read. Pick a folder that is already there; \
                 dux never creates one."
            ],
        });
        return;
    }
    let session = AgentSession {
        id: Uuid::new_v4().to_string(),
        slot_tab_id: Uuid::new_v4().to_string(),
        provider: provider.clone(),
        workspace: AgentWorkspace::Folder(FolderWorkspace {
            folder_path: folder.to_string_lossy().to_string(),
        }),
        // Always set. Every row label falls back through the branch name when
        // there is no title, and this agent has no branch, so a title-less
        // standalone agent would render as a nameless row.
        title: Some(title.clone()),
        started_providers: Vec::new(),
        desired_running: true,
        auto_reopen_enabled: true,
        status: SessionStatus::Active,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        last_focused_tab: None,
    };
    let provider_cfg = provider_config(&config, &session.provider);
    if let Err(hint) = check_provider_available(&provider_cfg) {
        logger::error(&format!("provider not found for {}: {hint}", session.id));
        // Nothing to roll back: no directory was created, no branch was minted.
        let _ = worker_tx.send(WorkerEvent::CreateAgentFailed {
            status_op_id: create_key,
            message: hint.into(),
        });
        return;
    }
    // The global environment, with no project overlay. See the doc above.
    let env =
        match crate::config::resolve_agent_env(&config.env, &std::collections::BTreeMap::new()) {
            Ok(env) => env,
            Err(err) => {
                let _ = worker_tx.send(WorkerEvent::CreateAgentFailed {
                    status_op_id: create_key,
                    message: format!(
                        "Invalid global environment variables, so the standalone agent was not \
                     started: {err:#}"
                    )
                    .into(),
                });
                return;
            }
        };
    let status_message = crate::status_text![
        "Created standalone agent ",
        q(title),
        " running ",
        n(provider.as_str()),
        " in ",
        q(folder_label),
        ". \
         dux does not manage a branch or a worktree for it, and never creates, moves \
         or removes that folder."
    ];
    let _ = worker_tx.send(WorkerEvent::CreateAgentProgress {
        status_op_id: create_key.clone(),
        message: crate::status_text![
            "Launching ",
            n(provider.as_str()),
            " in ",
            q(folder_label),
            "..."
        ],
    });
    // crossterm::terminal::size() returns (cols, rows).
    let (cols, rows) = term_size;
    let request = AgentLaunchRequest {
        // Create is always the session-slot tab. (Evaluated before `session` is moved.)
        tab_id: session.slot_tab_id().to_owned(),
        provider: session.provider.clone(),
        // A brand-new standalone agent starts a fresh conversation; the dynamic
        // per-provider resume rule applies to later launches like any agent's.
        resume: false,
        session,
        provider_config: provider_cfg,
        env,
        identity,
        pty_size: (rows, cols),
        scrollback_lines: config.ui.agent_scrollback_lines,
        kind: AgentLaunchKind::Create {
            status_message,
            status_warns: false,
            status_notes: None,
            pull_request_pin: None,
            // There is no repository behind this agent. The field is only read
            // by the rollback, which `owns_worktree: false` switches off.
            repo_path: String::new(),
            // dux did not make this directory, so a failed launch must never
            // remove it. Same flag, same reason, as adopting an existing
            // managed worktree.
            owns_worktree: false,
            startup_result: None,
            status_op_id: create_key,
        },
        wants_fullscreen: false,
        // Loud: dux's promise about never touching the user's folder is the
        // whole point of the sentence and is nowhere on screen.
        status_quiet: crate::statusline::QuietSurfaces::LOUD,
    };
    run_agent_launch_job(request, worker_tx);
}

#[allow(clippy::too_many_arguments)]
fn launch_managed_create(
    plan: ManagedCreatePlan,
    paths: DuxPaths,
    config: Config,
    worker_tx: Sender<WorkerEvent>,
    term_size: (u16, u16),
    create_key: String,
    identity: crate::term_identity::TerminalIdentity,
    mut creation_notes: Vec<crate::status_text::StatusText>,
) {
    let ManagedCreatePlan {
        project,
        provider,
        source_branch,
        status_message,
        status_quiet,
        branch_name,
        worktree_path,
        owns_worktree,
        title,
        launch_with_resume,
        pending_copy,
        status_lead,
        pull_request_pin,
        branch_provenance,
    } = plan;
    let repo_path = PathBuf::from(&project.path);
    if owns_worktree {
        logger::info(&format!(
            "created worktree {} on branch {}",
            worktree_path.display(),
            branch_name
        ));
    } else {
        logger::info(&format!(
            "reusing worktree {} on branch {} for new provider session",
            worktree_path.display(),
            branch_name
        ));
    }
    let started_providers = if launch_with_resume {
        vec![provider.as_str().to_string()]
    } else {
        Vec::new()
    };
    // Every arm of this function provisions a worktree, so the managed shape is
    // the only one this tail can produce; the standalone path never reaches
    // here at all.
    let managed = ManagedWorkspace {
        project_id: project.id.clone(),
        project_path: Some(project.path.clone()),
        source_branch,
        // The agent is born on `branch_name`; record that as its immutable
        // original branch. The branch-sync poller and intentional renames
        // update `branch_name` later but must never touch `initial_branch`.
        initial_branch: branch_name.clone(),
        branch_provenance,
        branch_name,
        worktree_path: worktree_path.to_string_lossy().to_string(),
    };
    let session = AgentSession {
        id: Uuid::new_v4().to_string(),
        slot_tab_id: Uuid::new_v4().to_string(),
        provider,
        workspace: AgentWorkspace::Managed(managed.clone()),
        title,
        started_providers,
        desired_running: true,
        auto_reopen_enabled: true,
        status: SessionStatus::Active,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        last_focused_tab: None,
    };
    let provider_cfg = provider_config(&config, &session.provider);
    if let Err(hint) = check_provider_available(&provider_cfg) {
        logger::error(&format!("provider not found for {}: {hint}", session.id));
        if owns_worktree && let Some(managed) = session.workspace.as_managed() {
            rollback_created_worktree(&repo_path, managed);
        }
        let _ = worker_tx.send(WorkerEvent::CreateAgentFailed {
            status_op_id: create_key.clone(),
            message: hint.into(),
        });
        return;
    }
    // The planned copy of uncommitted changes runs here, after the provider
    // availability check (so a missing provider does not discard completed
    // copy work) and before the startup command (which must see the files).
    if let Some(copy) = pending_copy
        && !apply_pending_copy(
            copy,
            &session,
            &repo_path,
            owns_worktree,
            &worker_tx,
            &create_key,
            &mut creation_notes,
        )
    {
        return;
    }
    // Notes ride the keyed create-op final so they surface as the visible
    // status/toast, never log-only. A note is a fact the screen does not show,
    // so it makes even an otherwise quiet create speak. A leading warning goes
    // ahead of the success line, so the terminal UI's two-row status footer
    // cannot cut the one part the user must act on, and it makes the final a
    // warning so it stays on screen longer.
    //
    // The lead and the notes are also kept on their own, so a create whose
    // startup command fails can still say them after its failure sentence.
    let join = |joined: crate::status_text::StatusText, part: crate::status_text::StatusText| {
        crate::status_text![joined, " ", part]
    };
    let status_warns = status_lead.is_some();
    let trailing_notes = creation_notes.into_iter().reduce(join);
    let status_notes = status_lead
        .clone()
        .into_iter()
        .chain(trailing_notes.clone())
        .reduce(join);
    let status_quiet = if status_notes.is_some() {
        crate::statusline::QuietSurfaces::LOUD
    } else {
        status_quiet
    };
    let status_message = status_lead
        .into_iter()
        .chain(std::iter::once(status_message))
        .chain(trailing_notes)
        .reduce(join)
        .unwrap_or_default();
    let env = match crate::config::resolve_agent_env(&config.env, &project.env) {
        Ok(env) => env,
        Err(err) => {
            // The worktree exists by now, so a failed create removes it like
            // every other failure past this point: left behind, it (and a
            // branch dux made) would push the next attempt onto another name
            // for no visible reason.
            if owns_worktree && let Some(managed) = session.workspace.as_managed() {
                rollback_created_worktree(&repo_path, managed);
            }
            let _ = worker_tx.send(WorkerEvent::CreateAgentFailed {
                status_op_id: create_key.clone(),
                message: crate::status_text![
                    "Invalid environment variables for project ",
                    q(project.name),
                    format!(": {:#}", err)
                ],
            });
            return;
        }
    };
    let startup_result = project
        .startup_command
        .as_deref()
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .map(|command| {
            let _ = worker_tx.send(WorkerEvent::CreateAgentProgress {
                status_op_id: create_key.clone(),
                message: crate::status_text![
                    "Running startup command for agent ",
                    q(session.display_label()),
                    "..."
                ],
            });
            run_startup_command(
                &paths,
                StartupCommandRun {
                    project: project.clone(),
                    session: session.clone(),
                    managed: managed.clone(),
                    command: command.to_string(),
                    terminal: config.startup_command_terminal.clone(),
                    env: env.clone(),
                },
            )
        });
    if let Some(result) = &startup_result {
        match &result.status {
            Ok(()) => logger::info(&format!(
                "startup command succeeded for {} (log: {})",
                result.session_id,
                result.log_path.display()
            )),
            Err(err) => logger::error(&format!(
                "startup command failed for {}: {err} (log: {})",
                result.session_id,
                result.log_path.display()
            )),
        }
    }
    let launch_message = if launch_with_resume {
        format!(
            "Continuing {} in the existing worktree...",
            session.provider.as_str()
        )
    } else {
        format!(
            "Launching {} in a fresh session...",
            session.provider.as_str()
        )
    };
    let _ = worker_tx.send(WorkerEvent::CreateAgentProgress {
        status_op_id: create_key.clone(),
        message: launch_message.into(),
    });
    // crossterm::terminal::size() returns (cols, rows).
    let (cols, rows) = term_size;
    let request = AgentLaunchRequest {
        // Create is always the session-slot tab, effective provider ==
        // session.provider. (Evaluated before `session` is moved.)
        tab_id: session.slot_tab_id().to_owned(),
        provider: session.provider.clone(),
        session,
        provider_config: provider_cfg,
        env,
        identity,
        resume: launch_with_resume,
        pty_size: (rows, cols),
        scrollback_lines: config.ui.agent_scrollback_lines,
        kind: AgentLaunchKind::Create {
            status_message,
            status_warns,
            status_notes: status_notes.map(Box::new),
            pull_request_pin,
            repo_path: repo_path.to_string_lossy().to_string(),
            owns_worktree,
            startup_result,
            status_op_id: create_key.clone(),
        },
        // A freshly created agent lands focused-but-minimized;
        // only fullscreen-seeking gestures set this, and create is never one.
        wants_fullscreen: false,
        status_quiet,
    };
    run_agent_launch_job(request, worker_tx);
}

#[allow(clippy::too_many_arguments)]
pub fn run_create_agent_job(
    request: CreateAgentRequest,
    paths: DuxPaths,
    config: Config,
    worker_tx: Sender<WorkerEvent>,
    term_size: (u16, u16),
    status_op_id: String,
    identity: crate::term_identity::TerminalIdentity,
) {
    // The opaque id of the shared create-agent `HandlerStatusOp` keys every
    // progress/failure event and is carried in `AgentLaunchKind::Create` so the
    // launch-ready/failed handler can resolve the op's final on the same id.
    let create_key = status_op_id;
    // Non-fatal notes (best-effort pull problems, skipped copies) accumulated
    // across the job and appended to the create status message, so they ride
    // the keyed create-op final and stay visible.
    let mut creation_notes: Vec<crate::status_text::StatusText> = Vec::new();
    // Standalone agents bypass worktree provisioning and its disk rollbacks.
    if let CreateAgentRequest::Standalone {
        folder,
        title,
        provider,
    } = request
    {
        run_create_standalone_agent_job(
            folder, title, provider, paths, config, worker_tx, term_size, create_key, identity,
        );
        return;
    }
    let plan = {
        let mut context = CreatePlanContext {
            paths: &paths,
            worker_tx: &worker_tx,
            create_key: &create_key,
            creation_notes: &mut creation_notes,
        };
        let Some(plan) = context.plan(request) else {
            return;
        };
        plan
    };
    launch_managed_create(
        plan,
        paths,
        config,
        worker_tx,
        term_size,
        create_key,
        identity,
        creation_notes,
    );
}
pub fn run_agent_launch_job(request: AgentLaunchRequest, worker_tx: Sender<WorkerEvent>) {
    let launch_args = request.provider_config.interactive_args(request.resume);
    let (rows, cols) = request.pty_size;
    logger::debug(&format!(
        "spawning PTY {:?} {:?} in {} ({}x{}, resume_supported={})",
        request.provider_config.command,
        launch_args,
        request.session.directory(),
        cols,
        rows,
        request.provider_config.supports_session_resume()
    ));

    if let Err(message) = check_provider_available(&request.provider_config) {
        logger::error(&format!(
            "provider availability check failed for {}: {message}",
            request.session.id
        ));
        if let AgentLaunchKind::Create {
            repo_path,
            owns_worktree,
            ..
        } = &request.kind
            && *owns_worktree
            && let Some(managed) = request.session.workspace.as_managed()
        {
            rollback_created_worktree(Path::new(repo_path), managed);
        }
        let _ = worker_tx.send(WorkerEvent::AgentLaunchFailed(Box::new(
            AgentLaunchFailedData { request, message },
        )));
        return;
    }

    let client = match crate::pty::PtyClient::spawn_with_env_opts(
        &request.provider_config.command,
        &launch_args,
        Path::new(request.session.directory()),
        rows,
        cols,
        request.scrollback_lines,
        crate::pty::PtySpawnOptions {
            env: &request.env,
            track_agent_signals: true,
            identity: &request.identity,
        },
    ) {
        Ok(client) => client,
        Err(err) => {
            logger::error(&format!(
                "PTY spawn failed for {}: {err}",
                request.session.id
            ));
            if let AgentLaunchKind::Create {
                repo_path,
                owns_worktree,
                ..
            } = &request.kind
                && *owns_worktree
                && let Some(managed) = request.session.workspace.as_managed()
            {
                rollback_created_worktree(Path::new(repo_path), managed);
            }
            let message = if matches!(request.kind, AgentLaunchKind::Create { .. }) {
                format!("Failed to start {}: {err}", request.provider_config.command)
            } else {
                err.to_string()
            };
            let _ = worker_tx.send(WorkerEvent::AgentLaunchFailed(Box::new(
                AgentLaunchFailedData { request, message },
            )));
            return;
        }
    };
    logger::info(&format!("PTY session started for {}", request.session.id));
    let _ = worker_tx.send(WorkerEvent::AgentLaunchReady(Box::new(
        AgentLaunchReadyData { request, client },
    )));
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::mpsc;

    use super::*;
    use crate::model::{Project, ProjectBranchStatus, ProviderKind};

    /// Initialize a throwaway git repo with a single commit on `main`.
    fn init_test_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let run = |args: &[&str]| {
            let out = crate::git::test_support::git_command()
                .args(args)
                .current_dir(p)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?} failed");
        };
        run(&["init", "-b", "main"]);
        run(&["config", "user.name", "test"]);
        run(&["config", "user.email", "t@t"]);
        run(&["commit", "--allow-empty", "-m", "init"]);
        dir
    }

    fn test_project(repo: &Path) -> Project {
        Project {
            id: "proj-1".to_string(),
            name: "repo".to_string(),
            path: repo.to_string_lossy().to_string(),
            explicit_default_provider: None,
            // `provider_config` falls back to the provider name as the command,
            // so a provider literally named "cat" spawns `cat`, a harmless PTY
            // process that stays alive on stdin, available on any Unix PATH.
            default_provider: ProviderKind::new("cat"),
            leading_branch: Some("main".to_string()),
            auto_reopen_agents: None,
            startup_command: None,
            env: BTreeMap::new(),
            current_branch: "main".to_string(),
            branch_status: ProjectBranchStatus::Leading,
            path_missing: false,
            created_at: None,
        }
    }

    /// Everything a create job emitted, for asserting on success, failure,
    /// the final status message, and the progress trail alike.
    struct JobRun {
        session: Option<AgentSession>,
        status_message: Option<String>,
        status_quiet: crate::statusline::QuietSurfaces,
        /// Whether the create's final is a warning rather than an info.
        status_warns: bool,
        failure: Option<String>,
        progress: Vec<String>,
        /// Keeps the temporary worktrees root alive so tests can inspect the
        /// created worktree's contents.
        _paths_root: tempfile::TempDir,
    }

    /// Drive `run_create_agent_job` for an arbitrary request against `repo`
    /// and collect every event the job emitted.
    fn drive_create_job_run(repo: &Path, request: CreateAgentRequest) -> JobRun {
        drive_create_job_run_with_setup(repo, request, |_| {})
    }

    /// Same, with a hook that runs against the job's `DuxPaths` before the job
    /// starts, so a test can sabotage the worktree destination and force a
    /// deterministic `git worktree add` failure.
    fn drive_create_job_run_with_setup(
        repo: &Path,
        request: CreateAgentRequest,
        setup: impl FnOnce(&DuxPaths),
    ) -> JobRun {
        let paths_root = tempfile::tempdir().unwrap();
        let paths = DuxPaths {
            root: paths_root.path().to_path_buf(),
            config_path: paths_root.path().join("config.toml"),
            sessions_db_path: paths_root.path().join("sessions.sqlite3"),
            worktrees_root: paths_root.path().join("worktrees"),
            lock_path: paths_root.path().join("dux.lock"),
        };
        std::fs::create_dir_all(&paths.worktrees_root).unwrap();
        setup(&paths);

        let _ = repo; // repo is referenced by the request; kept alive by caller.
        let (tx, rx) = mpsc::channel();
        run_create_agent_job(
            request,
            paths,
            Config::default(),
            tx,
            (80, 24),
            "op-1".to_string(),
            crate::term_identity::TerminalIdentity::default(),
        );
        let mut run = JobRun {
            session: None,
            status_message: None,
            status_quiet: crate::statusline::QuietSurfaces::LOUD,
            status_warns: false,
            failure: None,
            progress: Vec::new(),
            _paths_root: paths_root,
        };
        while let Ok(event) = rx.try_recv() {
            match event {
                WorkerEvent::AgentLaunchReady(data) => {
                    run.session = Some(data.request.session.clone());
                    if let AgentLaunchKind::Create {
                        status_message,
                        status_warns,
                        ..
                    } = &data.request.kind
                    {
                        run.status_message = Some(status_message.to_string());
                        run.status_quiet = data.request.status_quiet;
                        run.status_warns = *status_warns;
                    }
                }
                WorkerEvent::CreateAgentFailed { message, .. } => {
                    run.failure = Some(message.to_string());
                }
                WorkerEvent::CreateAgentProgress { message, .. } => {
                    run.progress.push(message.to_string());
                }
                _ => {}
            }
        }
        run
    }

    /// Drive `run_create_agent_job` and return the `AgentSession` it
    /// constructed. Panics with the failure message if the job emits
    /// `CreateAgentFailed` instead.
    fn drive_create_job(repo: &Path, request: CreateAgentRequest) -> AgentSession {
        let run = drive_create_job_run(repo, request);
        if let Some(message) = run.failure {
            panic!("create job failed: {message}");
        }
        run.session
            .expect("the job should emit an AgentLaunchReady with the session")
    }

    /// Drive `run_create_agent_job` for a `NewProject` request and return the
    /// `AgentSession` the job constructed.
    fn create_session_for(custom_name: Option<String>) -> AgentSession {
        let repo = init_test_repo();
        let project = test_project(repo.path());
        let request = CreateAgentRequest::NewProject {
            project,
            custom_name,
            use_existing_branch: false,
            pull_before_create: false,
            copy_uncommitted_changes: false,
        };
        drive_create_job(repo.path(), request)
    }

    /// Create a branch `name` (pointing at HEAD) in `repo` so an "attach to
    /// existing branch" path can find it.
    fn create_branch(repo: &Path, name: &str) {
        let out = crate::git::test_support::git_command()
            .args(["branch", name])
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(out.status.success(), "git branch {name} failed");
    }

    /// A minimal `AgentSession` rooted at `worktree` (a real git worktree so
    /// `head_commit`/`copy_uncommitted_changes` succeed), for the fork arms.
    fn fork_source_session(worktree: &Path) -> AgentSession {
        AgentSession {
            id: "src-1".to_string(),
            slot_tab_id: "src-1-slot".to_string(),
            provider: ProviderKind::new("cat"),
            title: None,
            started_providers: Vec::new(),
            desired_running: false,
            auto_reopen_enabled: true,
            status: SessionStatus::Detached,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_focused_tab: None,
            workspace: crate::model::AgentWorkspace::Managed(crate::model::ManagedWorkspace {
                project_id: "proj-1".to_string(),
                project_path: None,
                source_branch: "main".to_string(),
                branch_name: "src-branch".to_string(),
                initial_branch: "src-branch".to_string(),
                branch_provenance: crate::model::BranchProvenance::CreatedByDux,
                worktree_path: worktree.to_string_lossy().to_string(),
            }),
        }
    }

    #[test]
    fn fork_without_a_name_fails_before_creating_a_worktree() {
        let repo = init_test_repo();
        let source = fork_source_session(repo.path());
        let run = drive_create_job_run(
            repo.path(),
            CreateAgentRequest::ForkSession {
                project: test_project(repo.path()),
                source_session: Box::new(source),
                source_label: "source".into(),
                custom_name: None,
            },
        );

        assert!(run.session.is_none());
        assert_eq!(
            run.failure.as_deref(),
            Some("Forking an agent requires choosing a name first.")
        );
        assert!(run.progress.is_empty());
    }

    #[test]
    fn new_project_without_commits_fails_with_initial_commit_guidance() {
        let repo = tempfile::tempdir().unwrap();
        git_in(repo.path(), &["init", "-b", "main"]);
        let run = drive_create_job_run(
            repo.path(),
            CreateAgentRequest::NewProject {
                project: test_project(repo.path()),
                custom_name: Some("agent".into()),
                use_existing_branch: false,
                pull_before_create: false,
                copy_uncommitted_changes: false,
            },
        );

        assert!(run.session.is_none());
        assert!(
            run.failure
                .as_deref()
                .is_some_and(|message| message.contains("has no commits yet"))
        );
    }

    /// THE RECORD-ONLY ROLLBACK PIN. A standalone create that fails must leave
    /// the user's folder exactly as it found it: nothing removed, nothing
    /// added, not even the hidden housekeeping dux writes elsewhere.
    ///
    /// The failure injected is a provider that does not exist, which is the
    /// real failure mode (a misconfigured `command`) and the one that reaches
    /// the rollback in the managed path.
    #[test]
    fn a_failed_standalone_create_leaves_the_users_folder_exactly_as_it_was() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("notes.txt"), "mine\n").unwrap();
        let before = folder_snapshot(folder.path());

        let mut config = Config::default();
        config.providers.commands.insert(
            "ghost".to_string(),
            crate::config::ProviderCommandConfig {
                command: "dux-no-such-provider-binary".to_string(),
                ..Default::default()
            },
        );
        let run = drive_standalone_job(folder.path(), config);

        assert!(
            run.failure.is_some(),
            "the create must fail so the rollback path is the one under test"
        );
        assert!(
            folder.path().is_dir(),
            "the folder itself must survive a failed create"
        );
        assert_eq!(
            folder_snapshot(folder.path()),
            before,
            "a failed standalone create must add and remove nothing in the user's folder"
        );
    }

    /// A standalone agent gets the GLOBAL environment, with no project overlay.
    /// The failure this pins is the quiet one: the project-lookup code paths
    /// fall through `unwrap_or_default` on a miss, which for a project-less
    /// agent yields an EMPTY environment rather than the global one.
    #[test]
    fn a_standalone_agent_launches_with_the_global_environment() {
        let folder = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config
            .env
            .insert("DUX_TEST_GLOBAL".to_string(), "from-global".to_string());
        // A real, harmless binary: the launch request is only built once the
        // provider is found, and it exiting immediately is fine here.
        config.providers.commands.insert(
            "ghost".to_string(),
            crate::config::ProviderCommandConfig {
                command: "true".to_string(),
                ..Default::default()
            },
        );
        let run = drive_standalone_job(folder.path(), config);

        let env = run.launch_env.expect("the launch request carries an env");
        assert!(
            env.iter()
                .any(|(k, v)| k == "DUX_TEST_GLOBAL" && v == "from-global"),
            "the global environment must reach a project-less agent, got {env:?}"
        );
    }

    /// Every entry under `path`, recursively, as sorted relative paths. Used to
    /// prove a failed create touched nothing.
    fn folder_snapshot(path: &Path) -> Vec<String> {
        fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let p = entry.path();
                out.push(
                    p.strip_prefix(root)
                        .unwrap_or(&p)
                        .to_string_lossy()
                        .to_string(),
                );
                if p.is_dir() {
                    walk(&p, root, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(path, path, &mut out);
        out.sort();
        out
    }

    /// Drive a standalone create to completion and collect what came back.
    fn drive_standalone_job(folder: &Path, config: Config) -> StandaloneRun {
        let tmp = tempfile::tempdir().unwrap();
        let paths = DuxPaths {
            config_path: tmp.path().join("config.toml"),
            sessions_db_path: tmp.path().join("sessions.sqlite3"),
            worktrees_root: tmp.path().join("worktrees"),
            lock_path: tmp.path().join("dux.lock"),
            root: tmp.path().to_path_buf(),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        run_create_agent_job(
            CreateAgentRequest::Standalone {
                folder: folder.to_path_buf(),
                title: "notes".to_string(),
                provider: ProviderKind::new("ghost"),
            },
            paths,
            config,
            tx,
            (80, 24),
            "op-standalone".to_string(),
            crate::term_identity::TerminalIdentity::default(),
        );
        let mut run = StandaloneRun::default();
        while let Ok(event) = rx.try_recv() {
            match event {
                WorkerEvent::CreateAgentFailed { message, .. } => {
                    run.failure = Some(message.to_string())
                }
                WorkerEvent::AgentLaunchReady(ready) => {
                    run.launch_env = Some(ready.request.env.clone());
                }
                WorkerEvent::AgentLaunchFailed(failed) => {
                    run.launch_env = Some(failed.request.env.clone());
                    run.failure.get_or_insert(failed.message.clone());
                }
                _ => {}
            }
        }
        run
    }

    #[derive(Default)]
    struct StandaloneRun {
        failure: Option<String>,
        launch_env: Option<Vec<(String, String)>>,
    }

    #[test]
    fn pull_request_arm_sets_title_and_initial_branch_from_typed_name() {
        let repo = init_test_repo();
        // Attach to an existing local branch so the arm avoids the network fetch.
        create_branch(repo.path(), "pr-agent");
        let request = CreateAgentRequest::PullRequest {
            project: test_project(repo.path()),
            host: "github.com".to_string(),
            owner_repo: "owner/repo".to_string(),
            number: 42,
            title: "Fix the bug".to_string(),
            state: "OPEN".to_string(),
            head_branch: "pr-head".to_string(),
            custom_name: Some("pr-agent".to_string()),
            use_existing_branch: true,
        };
        let session = drive_create_job(repo.path(), request);
        // The typed name is durable identity (title) and names the branch; the
        // birth branch is recorded immutably and equals the created branch.
        assert_eq!(session.title.as_deref(), Some("pr-agent"));
        assert_eq!(session.branch_name().unwrap(), "pr-agent");
        assert_eq!(session.initial_branch().unwrap(), "pr-agent");
    }

    #[test]
    fn fork_session_arm_sets_title_and_initial_branch_from_typed_name() {
        let repo = init_test_repo();
        // The source worktree is the repo itself (a git dir with a HEAD commit).
        let source = fork_source_session(repo.path());
        let request = CreateAgentRequest::ForkSession {
            project: test_project(repo.path()),
            source_session: Box::new(source),
            source_label: "src agent".to_string(),
            custom_name: Some("forked-agent".to_string()),
        };
        let session = drive_create_job(repo.path(), request);
        assert_eq!(session.title.as_deref(), Some("forked-agent"));
        assert_eq!(session.branch_name().unwrap(), "forked-agent");
        assert_eq!(session.initial_branch().unwrap(), "forked-agent");
    }

    #[test]
    fn fork_external_worktree_arm_sets_title_and_initial_branch_from_typed_name() {
        let repo = init_test_repo();
        let request = CreateAgentRequest::ForkExternalWorktree {
            project: test_project(repo.path()),
            source_worktree_path: repo.path().to_path_buf(),
            source_label: "ext worktree".to_string(),
            source_branch: "main".to_string(),
            custom_name: Some("external-agent".to_string()),
        };
        let session = drive_create_job(repo.path(), request);
        assert_eq!(session.title.as_deref(), Some("external-agent"));
        assert_eq!(session.branch_name().unwrap(), "external-agent");
        assert_eq!(session.initial_branch().unwrap(), "external-agent");
    }

    #[test]
    fn fork_external_worktree_arm_without_name_keeps_title_none() {
        let repo = init_test_repo();
        let request = CreateAgentRequest::ForkExternalWorktree {
            project: test_project(repo.path()),
            source_worktree_path: repo.path().to_path_buf(),
            source_label: "ext worktree".to_string(),
            source_branch: "main".to_string(),
            custom_name: None,
        };
        let session = drive_create_job(repo.path(), request);
        // No typed name: title stays None, but the auto-derived branch is still
        // recorded as the immutable initial branch.
        assert_eq!(session.title, None);
        assert_eq!(session.initial_branch(), session.branch_name());
        assert!(!session.branch_name().unwrap().is_empty());
    }

    #[test]
    fn a_named_new_agent_stores_the_typed_name_as_title() {
        let session = create_session_for(Some("server-mode".to_string()));
        // The typed name is durable identity (title), and it also names the branch.
        assert_eq!(session.title.as_deref(), Some("server-mode"));
        assert_eq!(session.branch_name().unwrap(), "server-mode");
        // The birth branch is recorded immutably and equals the created branch.
        assert_eq!(session.initial_branch().unwrap(), "server-mode");
    }

    #[test]
    fn an_auto_named_agent_keeps_title_none() {
        let session = create_session_for(None);
        // An auto pet-name leaves title empty so the display keeps tracking the
        // branch, but the pet name still becomes the immutable initial branch.
        assert_eq!(session.title, None);
        assert_eq!(session.initial_branch(), session.branch_name());
        assert!(!session.branch_name().unwrap().is_empty());
    }

    // ── branch provenance ────────────────────────────────────────
    //
    // Every create arm decides, once, whether the agent's branch is dux's to
    // delete later. Getting this wrong destroys a user's branch on delete, so
    // all of the reachable outcomes are pinned here.

    #[test]
    fn a_fresh_agent_owns_the_branch_dux_minted_for_it() {
        let session = create_session_for(Some("fresh".to_string()));
        assert_eq!(
            session.branch_provenance().unwrap(),
            crate::model::BranchProvenance::CreatedByDux
        );
    }

    #[test]
    fn attaching_to_an_existing_branch_records_it_as_pre_existing() {
        let repo = init_test_repo();
        create_branch(repo.path(), "develop");
        let request = CreateAgentRequest::NewProject {
            project: test_project(repo.path()),
            custom_name: Some("develop".to_string()),
            use_existing_branch: true,
            pull_before_create: false,
            copy_uncommitted_changes: false,
        };
        let session = drive_create_job(repo.path(), request);
        assert_eq!(
            session.branch_provenance().unwrap(),
            crate::model::BranchProvenance::AttachedExisting,
            "the user's branch existed first, so deleting the agent must keep it"
        );
    }

    #[test]
    fn an_auto_named_agent_that_collides_with_an_existing_branch_attaches() {
        // The last-mile `branch_exists` check turns a pet name that happens to
        // match a real branch into an attach; the provenance must follow it,
        // because the user never asked to hand that branch to dux.
        let repo = init_test_repo();
        create_branch(repo.path(), "already-here");
        let request = CreateAgentRequest::NewProject {
            project: test_project(repo.path()),
            custom_name: Some("already-here".to_string()),
            // NOT confirmed by the dialog: the arm discovers the branch itself.
            use_existing_branch: false,
            pull_before_create: false,
            copy_uncommitted_changes: false,
        };
        let session = drive_create_job(repo.path(), request);
        assert_eq!(
            session.branch_provenance().unwrap(),
            crate::model::BranchProvenance::AttachedExisting
        );
    }

    #[test]
    fn a_pull_request_agent_attached_to_an_existing_branch_keeps_it() {
        let repo = init_test_repo();
        create_branch(repo.path(), "pr-agent");
        let request = CreateAgentRequest::PullRequest {
            project: test_project(repo.path()),
            host: "github.com".to_string(),
            owner_repo: "owner/repo".to_string(),
            number: 42,
            title: "Fix the bug".to_string(),
            state: "OPEN".to_string(),
            head_branch: "pr-head".to_string(),
            custom_name: Some("pr-agent".to_string()),
            use_existing_branch: true,
        };
        let session = drive_create_job(repo.path(), request);
        assert_eq!(
            session.branch_provenance().unwrap(),
            crate::model::BranchProvenance::AttachedExisting
        );
    }

    #[test]
    fn a_pull_request_agent_owns_a_branch_it_fetched() {
        // The fetch arm mints `refs/heads/<name>` itself, so that ref is dux's
        // to clean up (and leaving it behind is what makes recreating the agent
        // collide). Simulated with a local "remote" so no network is needed.
        let (_origin, repo) = pr_repo_with_fake_origin();
        let session = drive_create_job(repo.path(), pull_request_request(repo.path(), None, false));
        assert_eq!(session.branch_name().unwrap(), "pr-head");
        assert_eq!(
            session.branch_provenance().unwrap(),
            crate::model::BranchProvenance::CreatedByDux,
            "dux minted this local branch from the PR head"
        );
    }

    #[test]
    fn a_pull_request_agent_owns_the_local_branch_dux_checked_out_from_origin() {
        // A SAME-REPO pull request whose head branch has already been fetched:
        // `refs/remotes/origin/pr-head` is there, `refs/heads/pr-head` is not.
        // The arm attaches instead of fetching (git's worktree add DWIMs the
        // local branch into existence), so the local ref is still dux's own
        // work and deleting the agent must take it. The branch on origin is a
        // different ref and is never touched either way.
        let (_origin, repo) = pr_repo_with_fake_origin();
        git_in(repo.path(), &["fetch", "origin"]);
        assert!(
            !crate::git::local_branch_exists(repo.path(), "pr-head"),
            "the local branch must not exist before the create"
        );
        let session = drive_create_job(repo.path(), pull_request_request(repo.path(), None, false));
        assert!(
            crate::git::local_branch_exists(repo.path(), "pr-head"),
            "dux made this local branch as part of creating the agent"
        );
        assert_eq!(
            session.branch_provenance().unwrap(),
            crate::model::BranchProvenance::CreatedByDux,
            "no local branch existed first, so this one is dux's to delete"
        );
    }

    #[test]
    fn a_pull_request_agent_keeps_a_local_branch_the_user_already_had() {
        // The user had checked the PR branch out by hand. The arm discovers it
        // (no dialog consent involved) and the branch stays the user's.
        let (_origin, repo) = pr_repo_with_fake_origin();
        git_in(repo.path(), &["fetch", "origin"]);
        git_in(repo.path(), &["branch", "--", "pr-head", "origin/pr-head"]);
        let session = drive_create_job(repo.path(), pull_request_request(repo.path(), None, false));
        assert_eq!(
            session.branch_provenance().unwrap(),
            crate::model::BranchProvenance::AttachedExisting,
            "the branch existed before the agent, so deleting the agent keeps it"
        );
    }

    #[test]
    fn a_fork_pull_request_owns_the_branch_the_fetch_minted() {
        // A FORK pull request: the head branch lives on someone else's repo, so
        // origin has `refs/pull/42/head` and no branch of that name at all. The
        // arm fetches, and the branch it mints is dux's.
        let (_origin, repo) = pr_repo_with_fake_origin_fork();
        git_in(repo.path(), &["fetch", "origin"]);
        assert!(
            crate::git::branch_exists(repo.path(), "pr-head").is_none(),
            "a fork PR's head branch is on no ref of origin's"
        );
        let session = drive_create_job(repo.path(), pull_request_request(repo.path(), None, false));
        assert_eq!(
            session.branch_provenance().unwrap(),
            crate::model::BranchProvenance::CreatedByDux
        );
    }

    #[test]
    fn a_failed_pr_worktree_deletes_a_branch_the_attach_would_have_minted() {
        // The same-repo case again, this time with the worktree add failing.
        // Nothing local existed first, so nothing local may be left behind.
        let (_origin, repo) = pr_repo_with_fake_origin();
        git_in(repo.path(), &["fetch", "origin"]);
        let run = drive_create_job_run_with_setup(
            repo.path(),
            pull_request_request(repo.path(), None, false),
            |paths| block_worktree_path(paths, "pr-head"),
        );
        assert!(run.failure.is_some(), "the worktree add must still fail");
        assert!(
            !crate::git::local_branch_exists(repo.path(), "pr-head"),
            "a local branch that did not exist before the create must not outlive it"
        );
    }

    /// A repo whose `origin` is a local repo carrying `refs/pull/42/head`, so
    /// the PR arm's fetch resolves without a network.
    fn pr_repo_with_fake_origin() -> (tempfile::TempDir, tempfile::TempDir) {
        let origin = init_test_repo();
        crate::git::test_support::git_command()
            .args(["checkout", "-b", "pr-head"])
            .current_dir(origin.path())
            .output()
            .unwrap();
        crate::git::test_support::git_command()
            .args(["commit", "--allow-empty", "-m", "pr work"])
            .current_dir(origin.path())
            .output()
            .unwrap();
        git_in(
            origin.path(),
            &["update-ref", "refs/pull/42/head", "refs/heads/pr-head"],
        );

        let repo = init_test_repo();
        git_in(
            repo.path(),
            &[
                "remote",
                "add",
                "origin",
                origin.path().to_string_lossy().as_ref(),
            ],
        );
        (origin, repo)
    }

    /// The same fake origin with the head branch REMOVED from it, which is what
    /// a fork's pull request looks like from the base repo: the commits are
    /// reachable through `refs/pull/42/head` and through no branch of origin's.
    fn pr_repo_with_fake_origin_fork() -> (tempfile::TempDir, tempfile::TempDir) {
        let (origin, repo) = pr_repo_with_fake_origin();
        git_in(origin.path(), &["checkout", "--detach"]);
        git_in(origin.path(), &["branch", "-D", "--", "pr-head"]);
        (origin, repo)
    }

    fn pull_request_request(
        repo: &Path,
        custom_name: Option<&str>,
        use_existing: bool,
    ) -> CreateAgentRequest {
        CreateAgentRequest::PullRequest {
            project: test_project(repo),
            host: "github.com".to_string(),
            owner_repo: "owner/repo".to_string(),
            number: 42,
            title: "Fix the bug".to_string(),
            state: "OPEN".to_string(),
            head_branch: "pr-head".to_string(),
            custom_name: custom_name.map(str::to_string),
            use_existing_branch: use_existing,
        }
    }

    /// Put a plain FILE where the worktree wants to go: `git worktree add`
    /// then fails deterministically, with no network, no timing, and no load.
    fn block_worktree_path(paths: &DuxPaths, branch: &str) {
        let project_root = paths.worktrees_root.join("repo");
        std::fs::create_dir_all(&project_root).unwrap();
        std::fs::write(project_root.join(branch), b"in the way").unwrap();
    }

    #[test]
    fn a_failed_pr_worktree_deletes_the_branch_the_fetch_minted() {
        // The fetch mints `refs/heads/pr-head` BEFORE the worktree exists, so
        // no session (and no provenance-driven rollback) covers this window.
        // Left behind, the ref makes the next create under the same name hit
        // the "branch already exists" attach prompt.
        let (_origin, repo) = pr_repo_with_fake_origin();
        let run = drive_create_job_run_with_setup(
            repo.path(),
            pull_request_request(repo.path(), None, false),
            |paths| block_worktree_path(paths, "pr-head"),
        );
        let failure = run
            .failure
            .expect("the worktree add must still fail loudly");
        assert!(
            failure.contains("Failed to create a worktree for PR #42"),
            "the error must still surface: {failure}"
        );
        assert!(
            !crate::git::local_branch_exists(repo.path(), "pr-head"),
            "the branch dux minted moments ago must not outlive the failed create"
        );
    }

    #[test]
    fn a_failed_pr_worktree_keeps_a_branch_that_already_existed() {
        // Attaching means the branch was the user's first. A failed create
        // must not take it with it.
        let (_origin, repo) = pr_repo_with_fake_origin();
        create_branch(repo.path(), "pr-agent");
        let run = drive_create_job_run_with_setup(
            repo.path(),
            pull_request_request(repo.path(), Some("pr-agent"), true),
            |paths| block_worktree_path(paths, "pr-agent"),
        );
        assert!(run.failure.is_some(), "the worktree add must still fail");
        assert!(
            crate::git::local_branch_exists(repo.path(), "pr-agent"),
            "dux did not create this branch, so a failed create must keep it"
        );
    }

    // ── a pull request whose branch is checked out elsewhere ─────

    /// Check the pull request's branch out in a second worktree, the way a
    /// first agent on the pull request holds it. Returns the folder that keeps
    /// the worktree alive and the worktree's path.
    fn hold_pr_branch_in_another_worktree(repo: &Path) -> (tempfile::TempDir, PathBuf) {
        git_in(repo, &["fetch", "origin"]);
        let holder_root = tempfile::tempdir().unwrap();
        let holder = holder_root.path().join("first-agent");
        git_in(
            repo,
            &[
                "worktree",
                "add",
                holder.to_string_lossy().as_ref(),
                "-b",
                "pr-head",
                "origin/pr-head",
            ],
        );
        (holder_root, holder)
    }

    /// Commit `count` empty commits in `worktree`, which the pull request on
    /// GitHub does not have.
    fn commit_local_work(worktree: &Path, count: usize) {
        for n in 0..count {
            git_in(
                worktree,
                &["commit", "--allow-empty", "-m", &format!("local work {n}")],
            );
        }
    }

    fn pr_commit(origin: &Path) -> String {
        git_stdout(origin, &["rev-parse", "refs/pull/42/head"])
    }

    /// Everything a worktree listing says, for "nothing was created" checks.
    fn worktree_paths(repo: &Path) -> Vec<PathBuf> {
        crate::git::list_worktrees(repo)
            .unwrap()
            .into_iter()
            .map(|w| w.path)
            .collect()
    }

    #[test]
    fn a_pull_request_branch_checked_out_in_another_worktree_gets_a_fresh_copy() {
        let (origin, repo) = pr_repo_with_fake_origin();
        let (_holder_root, holder) = hold_pr_branch_in_another_worktree(repo.path());
        let busy_tip = git_stdout(repo.path(), &["rev-parse", "refs/heads/pr-head"]);

        let run = drive_create_job_run(repo.path(), pull_request_request(repo.path(), None, false));

        assert!(
            run.failure.is_none(),
            "creation must succeed: {:?}",
            run.failure
        );
        let session = run.session.expect("a session");
        assert_eq!(session.branch_name().unwrap(), "pr-head-review");
        assert_eq!(
            session.branch_provenance().unwrap(),
            crate::model::BranchProvenance::CreatedByDux,
            "dux made the copy's branch, so it is dux's to delete"
        );
        let copy = Path::new(session.directory());
        assert_eq!(
            git_stdout(copy, &["rev-parse", "HEAD"]),
            pr_commit(origin.path()),
            "the copy is the pull request as GitHub shows it"
        );
        assert_eq!(
            git_stdout(copy, &["status", "--porcelain"]),
            "",
            "the copy starts clean"
        );
        assert_eq!(
            git_stdout(&holder, &["symbolic-ref", "--short", "HEAD"]),
            "pr-head",
            "the first agent's worktree is still on its branch"
        );
        assert_eq!(
            git_stdout(repo.path(), &["rev-parse", "refs/heads/pr-head"]),
            busy_tip,
            "the busy branch did not move"
        );
        let message = run.status_message.expect("a create message");
        let folder = crate::home_path::shorten_home(&holder);
        assert!(
            message.ends_with(&format!(
                ". Fresh copy of PR #42 on its own branch \"pr-head-review\": \"pr-head\" is checked out at \"{folder}\"."
            )),
            "the note says what happened and why: {message}"
        );
        assert!(
            !message.contains("It is linked"),
            "the worker cannot know the link will hold, so it never claims it: {message}"
        );
        assert!(
            !message.contains("This copy is missing"),
            "nothing is missing, so no count is claimed: {message}"
        );
        assert_eq!(
            run.status_quiet,
            crate::statusline::QuietSurfaces::LOUD,
            "the screen cannot vouch for a fresh copy, so it speaks"
        );
        assert!(!run.status_warns, "a complete copy is an info");
    }

    #[test]
    fn a_pull_request_branch_checked_out_in_the_project_folder_gets_a_fresh_copy() {
        let (origin, repo) = pr_repo_with_fake_origin();
        git_in(repo.path(), &["fetch", "origin"]);
        git_in(
            repo.path(),
            &["checkout", "-b", "pr-head", "origin/pr-head"],
        );

        let run = drive_create_job_run(repo.path(), pull_request_request(repo.path(), None, false));

        assert!(
            run.failure.is_none(),
            "creation must succeed: {:?}",
            run.failure
        );
        let session = run.session.expect("a session");
        assert_eq!(session.branch_name().unwrap(), "pr-head-review");
        assert_eq!(
            git_stdout(Path::new(session.directory()), &["rev-parse", "HEAD"]),
            pr_commit(origin.path())
        );
        assert_eq!(
            git_stdout(repo.path(), &["symbolic-ref", "--short", "HEAD"]),
            "pr-head",
            "the project folder is still on the branch"
        );
        let message = run.status_message.expect("a create message");
        assert!(
            message.contains(": \"pr-head\" is checked out in the project folder "),
            "the note names the project folder: {message}"
        );
    }

    #[test]
    fn a_pull_request_branch_held_by_a_rebase_gets_a_fresh_copy() {
        let (origin, repo) = pr_repo_with_fake_origin();
        let (_holder_root, holder) = hold_pr_branch_in_another_worktree(repo.path());
        commit_local_work(&holder, 1);
        // A rebase that stops on its first step leaves the worktree detached,
        // listing no branch, while git still reserves `pr-head`.
        let out = crate::git::test_support::git_command()
            .args(["rebase", "--exec", "exit 1", "HEAD~1"])
            .current_dir(&holder)
            .output()
            .unwrap();
        assert!(!out.status.success(), "the rebase must stop part-way");

        let run = drive_create_job_run(repo.path(), pull_request_request(repo.path(), None, false));

        assert!(
            run.failure.is_none(),
            "creation must succeed: {:?}",
            run.failure
        );
        let session = run.session.expect("a session");
        assert_eq!(session.branch_name().unwrap(), "pr-head-review");
        assert_eq!(
            git_stdout(Path::new(session.directory()), &["rev-parse", "HEAD"]),
            pr_commit(origin.path())
        );
        assert!(
            holder.join(".git").exists()
                && git_stdout(repo.path(), &["worktree", "list", "--porcelain"])
                    .contains("detached"),
            "the rebase is left exactly where it stopped"
        );
    }

    #[test]
    fn a_pull_request_branch_held_by_a_worktree_whose_folder_is_gone_gets_a_fresh_copy() {
        let (_origin, repo) = pr_repo_with_fake_origin();
        let (_holder_root, holder) = hold_pr_branch_in_another_worktree(repo.path());
        std::fs::remove_dir_all(&holder).unwrap();

        let run = drive_create_job_run(repo.path(), pull_request_request(repo.path(), None, false));

        assert!(
            run.failure.is_none(),
            "creation must succeed: {:?}",
            run.failure
        );
        assert_eq!(
            run.session.unwrap().branch_name().unwrap(),
            "pr-head-review"
        );
        let message = run.status_message.expect("a create message");
        let folder = crate::home_path::shorten_home(&holder);
        assert!(
            message.contains(&format!(
                ": \"pr-head\" is still reserved by a working copy whose folder is gone (\"{folder}\"); git worktree prune frees it."
            )),
            "the note says how to free the branch: {message}"
        );
    }

    #[test]
    fn a_fresh_copy_skips_a_review_name_that_is_already_a_branch() {
        let (_origin, repo) = pr_repo_with_fake_origin();
        let (_holder_root, _holder) = hold_pr_branch_in_another_worktree(repo.path());
        create_branch(repo.path(), "pr-head-review");

        let run = drive_create_job_run(repo.path(), pull_request_request(repo.path(), None, false));

        assert!(
            run.failure.is_none(),
            "creation must succeed: {:?}",
            run.failure
        );
        assert_eq!(
            run.session.unwrap().branch_name().unwrap(),
            "pr-head-review-2"
        );
    }

    #[test]
    fn a_fresh_copy_skips_a_review_name_whose_folder_is_occupied() {
        let (_origin, repo) = pr_repo_with_fake_origin();
        let (_holder_root, _holder) = hold_pr_branch_in_another_worktree(repo.path());

        let run = drive_create_job_run_with_setup(
            repo.path(),
            pull_request_request(repo.path(), None, false),
            |paths| {
                std::fs::create_dir_all(paths.worktrees_root.join("repo").join("pr-head-review"))
                    .unwrap();
            },
        );

        assert!(
            run.failure.is_none(),
            "creation must succeed: {:?}",
            run.failure
        );
        assert_eq!(
            run.session.unwrap().branch_name().unwrap(),
            "pr-head-review-2"
        );
    }

    /// Git stores refs as paths, so a branch below the review name, such as
    /// `pr-head-review/x`, keeps `pr-head-review` itself from being created.
    #[test]
    fn a_fresh_copy_skips_a_review_name_with_a_branch_below_it() {
        let (_origin, repo) = pr_repo_with_fake_origin();
        let (_holder_root, _holder) = hold_pr_branch_in_another_worktree(repo.path());
        create_branch(repo.path(), "pr-head-review/x");

        let run = drive_create_job_run(repo.path(), pull_request_request(repo.path(), None, false));

        assert!(
            run.failure.is_none(),
            "creation must succeed: {:?}",
            run.failure
        );
        assert_eq!(
            run.session.unwrap().branch_name().unwrap(),
            "pr-head-review-2"
        );
    }

    /// Only a folder that is certainly absent is free. When the question
    /// cannot be answered (here, the project's worktree folder is a plain
    /// file, so asking about anything inside it fails with "not a directory"),
    /// the name counts as taken, and nothing is fetched into a branch.
    #[test]
    fn a_fresh_copy_counts_a_review_folder_it_cannot_inspect_as_taken() {
        let (_origin, repo) = pr_repo_with_fake_origin();
        let (_holder_root, _holder) = hold_pr_branch_in_another_worktree(repo.path());
        let branches_before = branches_in(repo.path());

        let run = drive_create_job_run_with_setup(
            repo.path(),
            pull_request_request(repo.path(), None, false),
            |paths| std::fs::write(paths.worktrees_root.join("repo"), b"in the way").unwrap(),
        );

        let failure = run.failure.expect("the create must fail");
        assert!(
            failure.ends_with(
                " and every name up to \"pr-head-review-20\" is taken. Type another name for the agent from PR #42."
            ),
            "{failure}"
        );
        assert_eq!(
            branches_in(repo.path()),
            branches_before,
            "no branch is fetched"
        );
    }

    /// A pull request from a fork whose head branch is called `main`, while
    /// the project folder's own `main` has moved on past the pull request's
    /// base. The copy is not behind `main`: they are different lines of work
    /// that share a name, so no commit is "missing" and nothing warns.
    #[test]
    fn a_fork_pull_request_named_like_the_project_folders_branch_gets_no_missing_count() {
        let origin = init_test_repo();
        git_in(origin.path(), &["checkout", "--detach"]);
        git_in(
            origin.path(),
            &["commit", "--allow-empty", "-m", "fork work"],
        );
        git_in(origin.path(), &["update-ref", "refs/pull/42/head", "HEAD"]);
        git_in(origin.path(), &["checkout", "main"]);
        git_in(
            origin.path(),
            &["commit", "--allow-empty", "-m", "main moves on"],
        );
        let repo = init_test_repo();
        git_in(
            repo.path(),
            &[
                "remote",
                "add",
                "origin",
                origin.path().to_string_lossy().as_ref(),
            ],
        );
        git_in(repo.path(), &["fetch", "origin"]);
        git_in(repo.path(), &["reset", "--hard", "origin/main"]);
        let mut request = pull_request_request(repo.path(), Some("main"), false);
        if let CreateAgentRequest::PullRequest { head_branch, .. } = &mut request {
            *head_branch = "main".to_string();
        }

        let run = drive_create_job_run(repo.path(), request);

        assert!(
            run.failure.is_none(),
            "creation must succeed: {:?}",
            run.failure
        );
        assert_eq!(run.session.unwrap().branch_name().unwrap(), "main-review");
        let message = run.status_message.expect("a create message");
        let folder = crate::home_path::shorten_home(repo.path());
        assert!(
            message.starts_with("Created "),
            "no warning leads the line: {message}"
        );
        assert!(
            message.ends_with(&format!(
                ". Fresh copy of PR #42 on its own branch \"main-review\": \"main\" is checked out in the project folder \"{folder}\"."
            )),
            "{message}"
        );
        assert!(!message.contains("missing"), "{message}");
        assert!(
            !run.status_warns,
            "an unrelated branch is no reason to warn"
        );
        assert_eq!(run.status_quiet, crate::statusline::QuietSurfaces::LOUD);
    }

    /// A project added from a LINKED worktree: its folder is that worktree,
    /// so the repository's own folder holding the branch is named by its
    /// path, never as "the project folder".
    #[test]
    fn a_fresh_copy_in_a_project_on_a_linked_worktree_names_the_repository_folder_by_path() {
        let (_origin, repo) = pr_repo_with_fake_origin();
        git_in(repo.path(), &["fetch", "origin"]);
        git_in(
            repo.path(),
            &["checkout", "-b", "pr-head", "origin/pr-head"],
        );
        let project_root = tempfile::tempdir().unwrap();
        let project_folder = project_root.path().join("project");
        git_in(
            repo.path(),
            &[
                "worktree",
                "add",
                project_folder.to_string_lossy().as_ref(),
                "main",
            ],
        );

        let run = drive_create_job_run(
            &project_folder,
            pull_request_request(&project_folder, None, false),
        );

        assert!(
            run.failure.is_none(),
            "creation must succeed: {:?}",
            run.failure
        );
        let message = run.status_message.expect("a create message");
        let folder = crate::home_path::shorten_home(repo.path());
        assert!(
            message.ends_with(&format!(": \"pr-head\" is checked out at \"{folder}\".")),
            "{message}"
        );
        assert!(!message.contains("project folder"), "{message}");
    }

    #[test]
    fn a_fresh_copy_with_every_review_name_taken_fails_and_creates_nothing() {
        let (_origin, repo) = pr_repo_with_fake_origin();
        let (_holder_root, _holder) = hold_pr_branch_in_another_worktree(repo.path());
        create_branch(repo.path(), "pr-head-review");
        for n in 2..=20 {
            create_branch(repo.path(), &format!("pr-head-review-{n}"));
        }
        let branches_before = branches_in(repo.path());
        let worktrees_before = worktree_paths(repo.path());

        let run = drive_create_job_run(repo.path(), pull_request_request(repo.path(), None, false));

        let failure = run.failure.expect("the create must fail");
        assert!(
            failure.starts_with("Branch \"pr-head\" is checked out at ")
                && failure.ends_with(
                    " and every name up to \"pr-head-review-20\" is taken. Type another name for the agent from PR #42."
                ),
            "the refusal says why and what to do: {failure}"
        );
        assert!(run.session.is_none());
        assert_eq!(
            branches_in(repo.path()),
            branches_before,
            "no branch is created"
        );
        assert_eq!(
            worktree_paths(repo.path()),
            worktrees_before,
            "no worktree is created"
        );
    }

    #[test]
    fn a_typed_name_checked_out_elsewhere_that_is_not_the_pr_branch_is_refused() {
        let (_origin, repo) = pr_repo_with_fake_origin();
        let branches_before = branches_in(repo.path());
        let worktrees_before = worktree_paths(repo.path());

        // `main` is checked out in the project folder.
        let run = drive_create_job_run(
            repo.path(),
            pull_request_request(repo.path(), Some("main"), false),
        );

        let failure = run.failure.expect("the create must fail");
        let folder = crate::home_path::shorten_home(repo.path());
        assert_eq!(
            failure,
            format!(
                "Branch \"main\" is checked out at \"{folder}\". Choose another name for the agent from PR #42."
            )
        );
        assert!(run.session.is_none());
        assert_eq!(
            branches_in(repo.path()),
            branches_before,
            "no branch is created"
        );
        assert_eq!(
            worktree_paths(repo.path()),
            worktrees_before,
            "no worktree is created"
        );
    }

    #[test]
    fn a_failed_fresh_copy_worktree_deletes_the_review_branch_the_fetch_minted() {
        // The busy-branch variant of
        // `a_failed_pr_worktree_deletes_the_branch_the_fetch_minted`. Anything
        // in the way of the copy's own folder only moves it to another name, so
        // the REPOSITORY's worktree bookkeeping is made a file instead: the name
        // is free, the fetch mints the branch, and only then does git refuse to
        // record the new worktree. The project folder holds the branch, so no
        // linked worktree needs that bookkeeping to exist.
        let (_origin, repo) = pr_repo_with_fake_origin();
        git_in(repo.path(), &["fetch", "origin"]);
        git_in(
            repo.path(),
            &["checkout", "-b", "pr-head", "origin/pr-head"],
        );
        std::fs::write(repo.path().join(".git").join("worktrees"), b"in the way").unwrap();

        let run = drive_create_job_run(repo.path(), pull_request_request(repo.path(), None, false));

        let failure = run.failure.expect("the worktree step must fail loudly");
        assert!(
            failure.contains("Failed to create a worktree for PR #42"),
            "the error must still surface: {failure}"
        );
        assert!(
            run.progress
                .iter()
                .any(|p| p.contains("into branch \"pr-head-review\"")),
            "the fetch ran into the review branch first: {:?}",
            run.progress
        );
        assert!(
            !crate::git::local_branch_exists(repo.path(), "pr-head-review"),
            "the branch dux minted moments ago must not outlive the failed create"
        );
    }

    #[test]
    fn a_fresh_copy_missing_commits_ends_in_a_warning_that_leads_with_the_count() {
        let (_origin, repo) = pr_repo_with_fake_origin();
        let (_holder_root, holder) = hold_pr_branch_in_another_worktree(repo.path());
        commit_local_work(&holder, 2);

        let run = drive_create_job_run(repo.path(), pull_request_request(repo.path(), None, false));

        assert!(
            run.failure.is_none(),
            "creation must succeed: {:?}",
            run.failure
        );
        let message = run.status_message.expect("a create message");
        assert!(
            message.starts_with(
                "This copy is missing 2 commits that \"pr-head\" has and GitHub does not show yet. "
            ),
            "the warning goes first, so a two-row footer cannot cut it: {message}"
        );
        assert!(run.status_warns, "a copy that lacks work is a warning");
        assert_eq!(run.status_quiet, crate::statusline::QuietSurfaces::LOUD);
    }

    #[test]
    fn a_fresh_copy_missing_one_commit_says_commit_in_the_singular() {
        let (_origin, repo) = pr_repo_with_fake_origin();
        let (_holder_root, holder) = hold_pr_branch_in_another_worktree(repo.path());
        commit_local_work(&holder, 1);

        let run = drive_create_job_run(repo.path(), pull_request_request(repo.path(), None, false));

        let message = run.status_message.expect("a create message");
        assert!(
            message.starts_with("This copy is missing 1 commit that "),
            "{message}"
        );
    }

    #[test]
    fn pull_request_arm_titles_a_fresh_copy_after_its_new_branch() {
        // The busy-branch variant of
        // `pull_request_arm_sets_title_and_initial_branch_from_typed_name`: the
        // terminal UI sends the head branch as the typed name, and the copy is
        // titled after the branch it actually got, so two sidebar rows do not
        // both read `pr-head`.
        let (_origin, repo) = pr_repo_with_fake_origin();
        let (_holder_root, _holder) = hold_pr_branch_in_another_worktree(repo.path());

        let session = drive_create_job(
            repo.path(),
            pull_request_request(repo.path(), Some("pr-head"), false),
        );

        assert_eq!(session.title.as_deref(), Some("pr-head-review"));
        assert_eq!(session.branch_name().unwrap(), "pr-head-review");
        assert_eq!(session.initial_branch().unwrap(), "pr-head-review");
    }

    #[test]
    fn an_invalid_project_environment_removes_the_worktree_and_the_branch_dux_made() {
        let (_origin, repo) = pr_repo_with_fake_origin();
        let (_holder_root, _holder) = hold_pr_branch_in_another_worktree(repo.path());
        let mut request = pull_request_request(repo.path(), None, false);
        if let CreateAgentRequest::PullRequest { project, .. } = &mut request {
            project
                .env
                .insert("NOT A VALID NAME".to_string(), "x".to_string());
        }
        let worktrees_before = worktree_paths(repo.path());
        let mut copy_path = None;

        let run = drive_create_job_run_with_setup(repo.path(), request, |paths| {
            copy_path = Some(paths.worktrees_root.join("repo").join("pr-head-review"));
        });

        let failure = run.failure.expect("the create must fail");
        assert!(
            failure.starts_with("Invalid environment variables for project \"repo\""),
            "{failure}"
        );
        assert!(
            !copy_path.unwrap().exists(),
            "the worktree the failed create made is removed"
        );
        assert_eq!(worktree_paths(repo.path()), worktrees_before);
        assert!(
            !crate::git::local_branch_exists(repo.path(), "pr-head-review"),
            "the branch dux made for the failed create is deleted"
        );
    }

    /// Drive a create through the engine, the way either surface does, and
    /// feed every worker event back until the create settles.
    fn drive_create_through_engine(
        engine: &mut crate::engine::Engine,
        request: CreateAgentRequest,
    ) -> Vec<crate::engine::EventReaction> {
        engine
            .apply(crate::engine::Command::DispatchCreateAgentRequest {
                request: Box::new(request),
                busy_message: "Creating...".to_string().into(),
                term_size: (24, 80),
            })
            .expect("the dispatch is accepted");
        let mut reactions = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let event = engine
                .worker_rx
                .recv_timeout(remaining)
                .expect("the create settles in time");
            let settled = matches!(
                event,
                WorkerEvent::AgentLaunchReady(_)
                    | WorkerEvent::AgentLaunchFailed(_)
                    | WorkerEvent::CreateAgentFailed { .. }
            );
            reactions.push(engine.process_worker_event(event));
            if settled {
                return reactions;
            }
        }
    }

    /// The create op's keyed final among an engine run's reactions.
    fn create_final(reactions: &[crate::engine::EventReaction]) -> crate::engine::StatusUpdate {
        fn walk(reaction: &crate::engine::EventReaction) -> Option<crate::engine::StatusUpdate> {
            match reaction {
                crate::engine::EventReaction::Status(update) if update.key.is_some() => {
                    Some(update.clone())
                }
                crate::engine::EventReaction::Multi(all) => all.iter().rev().find_map(walk),
                _ => None,
            }
        }
        reactions
            .iter()
            .rev()
            .find_map(walk)
            .expect("the create resolves to a keyed final")
    }

    #[test]
    fn a_fresh_copy_is_pinned_to_its_pull_request_and_an_ordinary_create_is_not() {
        let (mut engine, _engine_dir) = crate::engine::test_support::test_engine();
        let (_origin, repo) = pr_repo_with_fake_origin();
        let reactions = drive_create_through_engine(
            &mut engine,
            pull_request_request(repo.path(), None, false),
        );
        let ordinary_final = create_final(&reactions);
        assert_eq!(ordinary_final.tone, crate::statusline::StatusTone::Info);
        assert_eq!(
            ordinary_final.quiet_on,
            crate::statusline::QuietSurfaces::BOTH,
            "an ordinary create stays quiet, as before"
        );
        let ordinary = engine
            .sessions
            .iter()
            .find(|s| s.branch_name() == Some("pr-head"))
            .expect("the ordinary create committed")
            .id
            .clone();
        assert!(
            !engine.pr_overrides.contains_key(&ordinary),
            "an ordinary create finds its pull request by branch name, as before"
        );

        // The ordinary agent now holds `pr-head`, exactly the case the ask is
        // about: a second agent from the same pull request, after the first
        // one committed work it has not pushed.
        let first_worktree = PathBuf::from(
            engine
                .sessions
                .iter()
                .find(|s| s.id == ordinary)
                .unwrap()
                .directory(),
        );
        commit_local_work(&first_worktree, 2);
        let reactions = drive_create_through_engine(
            &mut engine,
            pull_request_request(repo.path(), None, false),
        );
        let copy_final = create_final(&reactions);
        assert_eq!(
            copy_final.tone,
            crate::statusline::StatusTone::Warning,
            "a copy that lacks work ends in a warning: {}",
            copy_final.message
        );
        assert_eq!(copy_final.quiet_on, crate::statusline::QuietSurfaces::LOUD);
        assert!(
            copy_final
                .message
                .starts_with("This copy is missing 2 commits that \"pr-head\" has"),
            "{}",
            copy_final.message
        );
        assert!(
            copy_final.message.ends_with(
                " It is linked to PR #42; a push from it goes to its own branch \"pr-head-review\", never to \"pr-head\"."
            ),
            "the link is claimed once it is made: {}",
            copy_final.message
        );
        let copy = engine
            .sessions
            .iter()
            .find(|s| s.branch_name() == Some("pr-head-review"))
            .expect("the fresh copy committed")
            .id
            .clone();
        let pin = engine
            .pr_overrides
            .get(&copy)
            .expect("the copy is pinned to its pull request");
        assert_eq!(pin.pr_number, 42);
        assert_eq!(pin.owner_repo, "owner/repo");
        assert_eq!(pin.state, "OPEN");
        assert_eq!(pin.title, "Fix the bug");
        let stored = engine.session_store.load_pr_overrides().unwrap();
        assert!(
            stored
                .iter()
                .any(|row| row.session_id == copy && row.pr_number == 42),
            "the pin is persisted"
        );
    }

    /// The link is made in the engine, after the worker said its piece, so a
    /// link that cannot be made must never be announced. A state dux cannot
    /// track makes the attach fail deterministically.
    #[test]
    fn a_fresh_copy_that_cannot_be_linked_says_so_and_never_claims_the_link() {
        let (mut engine, _engine_dir) = crate::engine::test_support::test_engine();
        let (_origin, repo) = pr_repo_with_fake_origin();
        let (_holder_root, _holder) = hold_pr_branch_in_another_worktree(repo.path());
        let mut request = pull_request_request(repo.path(), None, false);
        if let CreateAgentRequest::PullRequest { state, .. } = &mut request {
            *state = "DRAFTISH".to_string();
        }

        let reactions = drive_create_through_engine(&mut engine, request);

        let copy_final = create_final(&reactions);
        assert_eq!(
            copy_final.tone,
            crate::statusline::StatusTone::Warning,
            "an unlinked copy is something to act on: {}",
            copy_final.message
        );
        assert!(
            !copy_final.message.contains("It is linked"),
            "{}",
            copy_final.message
        );
        assert!(
            copy_final
                .message
                .contains(" dux could not link it to PR #42: ")
                && copy_final
                    .message
                    .ends_with(". Attach the pull request to the agent by hand."),
            "{}",
            copy_final.message
        );
        let copy = engine
            .sessions
            .iter()
            .find(|s| s.branch_name() == Some("pr-head-review"))
            .expect("the copy is committed all the same")
            .id
            .clone();
        assert!(!engine.pr_overrides.contains_key(&copy));
    }

    /// A startup command that fails ends the create in a sticky error, and
    /// what the create's notes said (here, that the agent is a fresh copy
    /// lacking two commits) still reaches the user after that sentence.
    #[test]
    fn a_fresh_copy_whose_startup_command_fails_keeps_its_notes() {
        let (mut engine, _engine_dir) = crate::engine::test_support::test_engine();
        let (_origin, repo) = pr_repo_with_fake_origin();
        let (_holder_root, holder) = hold_pr_branch_in_another_worktree(repo.path());
        commit_local_work(&holder, 2);
        let mut request = pull_request_request(repo.path(), None, false);
        if let CreateAgentRequest::PullRequest { project, .. } = &mut request {
            project.startup_command = Some("exit 3".to_string());
        }

        let reactions = drive_create_through_engine(&mut engine, request);

        let failed = create_final(&reactions);
        assert_eq!(failed.tone, crate::statusline::StatusTone::Error);
        assert!(failed.sticky, "a failed startup command stays on screen");
        assert!(
            failed
                .message
                .starts_with("Startup command failed for agent \"pr-head-review\": "),
            "the startup failure still leads: {}",
            failed.message
        );
        let (startup, notes) = failed
            .message
            .split_once(" Open the startup command logs for details. ")
            .unwrap_or_else(|| panic!("the notes follow the failure: {}", failed.message));
        assert!(!startup.contains("Fresh copy"), "{}", failed.message);
        assert!(
            notes.starts_with(
                "This copy is missing 2 commits that \"pr-head\" has and GitHub does not show yet. Fresh copy of PR #42 on its own branch \"pr-head-review\": \"pr-head\" is checked out at "
            ),
            "{}",
            failed.message
        );
        assert!(
            notes.ends_with(
                " It is linked to PR #42; a push from it goes to its own branch \"pr-head-review\", never to \"pr-head\"."
            ),
            "{}",
            failed.message
        );
    }

    /// An ordinary create whose startup command fails says exactly what it
    /// always said: it has no notes to add.
    #[test]
    fn an_ordinary_create_whose_startup_command_fails_adds_nothing() {
        let (mut engine, _engine_dir) = crate::engine::test_support::test_engine();
        let (_origin, repo) = pr_repo_with_fake_origin();
        let mut request = pull_request_request(repo.path(), None, false);
        if let CreateAgentRequest::PullRequest { project, .. } = &mut request {
            project.startup_command = Some("exit 3".to_string());
        }

        let reactions = drive_create_through_engine(&mut engine, request);

        let failed = create_final(&reactions);
        assert_eq!(failed.tone, crate::statusline::StatusTone::Error);
        assert!(
            failed
                .message
                .ends_with(". Open the startup command logs for details."),
            "{}",
            failed.message
        );
    }

    #[test]
    fn a_forked_agent_owns_its_new_branch() {
        let repo = init_test_repo();
        let source = fork_source_session(repo.path());
        let request = CreateAgentRequest::ForkSession {
            project: test_project(repo.path()),
            source_session: Box::new(source),
            source_label: "src agent".to_string(),
            custom_name: Some("forked-agent".to_string()),
        };
        let session = drive_create_job(repo.path(), request);
        assert_eq!(
            session.branch_provenance().unwrap(),
            crate::model::BranchProvenance::CreatedByDux
        );
    }

    #[test]
    fn forking_an_external_worktree_owns_the_managed_branch_it_creates() {
        let repo = init_test_repo();
        let request = CreateAgentRequest::ForkExternalWorktree {
            project: test_project(repo.path()),
            source_worktree_path: repo.path().to_path_buf(),
            source_label: "ext worktree".to_string(),
            source_branch: "main".to_string(),
            custom_name: Some("external-agent".to_string()),
        };
        let session = drive_create_job(repo.path(), request);
        assert_eq!(
            session.branch_provenance().unwrap(),
            crate::model::BranchProvenance::CreatedByDux
        );
    }

    #[test]
    fn adopting_an_existing_worktree_adopts_its_branch_too() {
        let repo = init_test_repo();
        let request = CreateAgentRequest::ExistingManagedWorktree {
            project: test_project(repo.path()),
            worktree_path: repo.path().to_path_buf(),
            branch_name: "main".to_string(),
            custom_name: None,
        };
        let session = drive_create_job(repo.path(), request);
        assert_eq!(
            session.branch_provenance().unwrap(),
            crate::model::BranchProvenance::Adopted,
            "the adopted worktree's branch predates the agent"
        );
    }

    /// Every branch in `repo`, as `git branch --list` sees them.
    fn branches_in(repo: &Path) -> String {
        let out = crate::git::test_support::git_command()
            .args(["branch", "--list"])
            .current_dir(repo)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    #[test]
    fn a_failed_attach_create_leaves_the_pre_existing_branch_alone() {
        // Regression: the create rollback removed the worktree AND deleted the
        // branch whenever dux had made the directory. For an attach, dux made
        // the directory but NOT the branch, so a create that failed on a
        // missing provider destroyed the user's branch on the way out.
        let repo = init_test_repo();
        create_branch(repo.path(), "develop");
        let mut project = test_project(repo.path());
        project.default_provider = ProviderKind::new("definitely-not-a-real-command-dux");
        let request = CreateAgentRequest::NewProject {
            project,
            custom_name: Some("develop".to_string()),
            use_existing_branch: true,
            pull_before_create: false,
            copy_uncommitted_changes: false,
        };

        let run = drive_create_job_run(repo.path(), request);

        assert!(
            run.failure.is_some(),
            "the unavailable provider must fail the job"
        );
        let branches = branches_in(repo.path());
        assert!(
            branches.contains("develop"),
            "the rollback must not delete a branch that existed before the agent: {branches}"
        );
    }

    #[test]
    fn a_failed_fresh_create_still_cleans_up_the_branch_it_minted() {
        // The other half of the gate: a branch dux created moments ago is dux's
        // to remove, or the retry collides with "branch already exists".
        let repo = init_test_repo();
        let mut project = test_project(repo.path());
        project.default_provider = ProviderKind::new("definitely-not-a-real-command-dux");
        let request = CreateAgentRequest::NewProject {
            project,
            custom_name: Some("brand-new".to_string()),
            use_existing_branch: false,
            pull_before_create: false,
            copy_uncommitted_changes: false,
        };

        let run = drive_create_job_run(repo.path(), request);

        assert!(run.failure.is_some(), "the job must fail");
        let branches = branches_in(repo.path());
        assert!(
            !branches.contains("brand-new"),
            "a branch dux minted for the failed agent must be cleaned up: {branches}"
        );
    }

    // ── uncommitted-changes copy and best-effort pull ────────────

    fn git_in(cwd: &Path, args: &[&str]) {
        let out = crate::git::test_support::git_command()
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed in {}: {}",
            args,
            cwd.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn git_stdout(cwd: &Path, args: &[&str]) -> String {
        let out = crate::git::test_support::git_command()
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn new_project_request(
        repo: &Path,
        pull_before_create: bool,
        copy_uncommitted_changes: bool,
    ) -> CreateAgentRequest {
        CreateAgentRequest::NewProject {
            project: test_project(repo),
            custom_name: Some("copy-target".to_string()),
            use_existing_branch: false,
            pull_before_create,
            copy_uncommitted_changes,
        }
    }

    /// A plain create says only what the new row and its streaming pane already
    /// say, so it is quiet on both surfaces. Attaching to a branch that already
    /// existed is nowhere on screen, so that one speaks.
    #[test]
    fn a_plain_create_is_quiet_on_both_surfaces_and_an_attach_is_not() {
        let repo = init_test_repo();
        let run = drive_create_job_run(repo.path(), new_project_request(repo.path(), false, false));
        assert!(run.failure.is_none(), "creation must succeed");
        assert!(run.status_message.unwrap().starts_with("Created "));
        assert_eq!(run.status_quiet, crate::statusline::QuietSurfaces::BOTH);

        let repo = init_test_repo();
        create_branch(repo.path(), "already-here");
        let attach = CreateAgentRequest::NewProject {
            project: test_project(repo.path()),
            custom_name: Some("already-here".to_string()),
            use_existing_branch: true,
            pull_before_create: false,
            copy_uncommitted_changes: false,
        };
        let run = drive_create_job_run(repo.path(), attach);
        assert!(run.failure.is_none(), "creation must succeed");
        assert!(run.status_message.unwrap().starts_with("Attached to "));
        assert_eq!(run.status_quiet, crate::statusline::QuietSurfaces::LOUD);
    }

    /// The two other creates whose whole answer is "a row appeared and its pane
    /// launched": from a pull request (the row carries the chip) and from an
    /// existing managed worktree.
    #[test]
    fn the_pull_request_and_import_creates_are_quiet_on_both_surfaces() {
        let (_origin, repo) = pr_repo_with_fake_origin();
        let run = drive_create_job_run(repo.path(), pull_request_request(repo.path(), None, false));
        assert!(run.failure.is_none(), "creation must succeed");
        assert!(
            run.status_message
                .as_deref()
                .expect("a create message")
                .contains("from PR #42")
        );
        assert_eq!(run.status_quiet, crate::statusline::QuietSurfaces::BOTH);

        let repo = init_test_repo();
        let run = drive_create_job_run(
            repo.path(),
            CreateAgentRequest::ExistingManagedWorktree {
                project: test_project(repo.path()),
                worktree_path: repo.path().to_path_buf(),
                branch_name: "main".to_string(),
                custom_name: None,
            },
        );
        assert!(run.failure.is_none(), "creation must succeed");
        assert!(
            run.status_message
                .as_deref()
                .expect("a create message")
                .starts_with("Imported ")
        );
        assert_eq!(run.status_quiet, crate::statusline::QuietSurfaces::BOTH);
    }

    /// The two creates whose sentence carries a copy rule the screen never
    /// shows: what travelled from the source, and what was deliberately left.
    #[test]
    fn the_fork_and_external_worktree_creates_stay_loud() {
        let repo = init_test_repo();
        let run = drive_create_job_run(
            repo.path(),
            CreateAgentRequest::ForkSession {
                project: test_project(repo.path()),
                source_session: Box::new(fork_source_session(repo.path())),
                source_label: "src agent".to_string(),
                custom_name: Some("forked-agent".to_string()),
            },
        );
        assert!(run.failure.is_none(), "creation must succeed");
        assert!(
            run.status_message
                .as_deref()
                .expect("a create message")
                .contains("gitignored files are not copied")
        );
        assert_eq!(run.status_quiet, crate::statusline::QuietSurfaces::LOUD);

        let repo = init_test_repo();
        let run = drive_create_job_run(
            repo.path(),
            CreateAgentRequest::ForkExternalWorktree {
                project: test_project(repo.path()),
                source_worktree_path: repo.path().to_path_buf(),
                source_label: "ext worktree".to_string(),
                source_branch: "main".to_string(),
                custom_name: Some("external-agent".to_string()),
            },
        );
        assert!(run.failure.is_none(), "creation must succeed");
        assert!(
            run.status_message
                .as_deref()
                .expect("a create message")
                .contains("gitignored files are not copied")
        );
        assert_eq!(run.status_quiet, crate::statusline::QuietSurfaces::LOUD);
    }

    /// A creation note is a fact the screen does not show, so it makes even an
    /// otherwise quiet create speak.
    #[test]
    fn a_creation_note_makes_a_quiet_create_speak() {
        let repo = init_test_repo();
        std::fs::write(repo.path().join("keep.txt"), "on main\n").unwrap();
        git_in(repo.path(), &["add", "-A"]);
        git_in(repo.path(), &["commit", "-m", "base"]);
        git_in(repo.path(), &["switch", "-c", "feature"]);
        std::fs::write(repo.path().join("feature-only.txt"), "feature\n").unwrap();
        git_in(repo.path(), &["add", "-A"]);
        git_in(repo.path(), &["commit", "-m", "feature commit"]);
        std::fs::remove_file(repo.path().join("keep.txt")).unwrap();

        let mut project = test_project(repo.path());
        project.current_branch = "feature".to_string();
        let run = drive_create_job_run(
            repo.path(),
            CreateAgentRequest::NewProject {
                project,
                custom_name: Some("note-carrier".to_string()),
                use_existing_branch: false,
                pull_before_create: false,
                copy_uncommitted_changes: true,
            },
        );
        assert!(run.failure.is_none(), "creation must succeed");
        assert!(run.status_message.unwrap().contains("were not copied"));
        assert_eq!(run.status_quiet, crate::statusline::QuietSurfaces::LOUD);
    }

    /// Happy path: the checkout's dirt travels; gitignored files do not.
    #[test]
    fn fresh_agent_copies_uncommitted_changes_from_checkout() {
        let repo = init_test_repo();
        std::fs::write(repo.path().join(".gitignore"), "*.log\n").unwrap();
        std::fs::write(repo.path().join("tracked.txt"), "base\n").unwrap();
        git_in(repo.path(), &["add", "-A"]);
        git_in(repo.path(), &["commit", "-m", "base"]);
        std::fs::write(repo.path().join("tracked.txt"), "dirty\n").unwrap();
        std::fs::write(repo.path().join("note.txt"), "untracked\n").unwrap();
        std::fs::write(repo.path().join("junk.log"), "ignored\n").unwrap();

        let run = drive_create_job_run(repo.path(), new_project_request(repo.path(), false, true));
        assert!(run.failure.is_none(), "creation must succeed");
        let worktree = PathBuf::from(run.session.as_ref().unwrap().directory());
        assert_eq!(
            std::fs::read_to_string(worktree.join("tracked.txt")).unwrap(),
            "dirty\n"
        );
        assert_eq!(
            std::fs::read_to_string(worktree.join("note.txt")).unwrap(),
            "untracked\n"
        );
        assert!(!worktree.join("junk.log").exists());
    }

    /// PROVEN DEFECT: with the checkout on a different commit than the new
    /// worktree, applying the status delta would delete files the worktree
    /// legitimately has. The HEAD guard must skip the copy with a note.
    #[test]
    fn fresh_agent_skips_copy_when_checkout_head_differs() {
        let repo = init_test_repo();
        std::fs::write(repo.path().join("keep.txt"), "on main\n").unwrap();
        git_in(repo.path(), &["add", "-A"]);
        git_in(repo.path(), &["commit", "-m", "base"]);
        // Park the checkout on `feature` at a DIFFERENT commit, with an
        // uncommitted `rm` of a file that exists on `main`.
        git_in(repo.path(), &["switch", "-c", "feature"]);
        std::fs::write(repo.path().join("feature-only.txt"), "feature\n").unwrap();
        git_in(repo.path(), &["add", "-A"]);
        git_in(repo.path(), &["commit", "-m", "feature commit"]);
        std::fs::remove_file(repo.path().join("keep.txt")).unwrap();

        let mut project = test_project(repo.path());
        project.current_branch = "feature".to_string();
        let request = CreateAgentRequest::NewProject {
            project,
            custom_name: Some("copy-target".to_string()),
            use_existing_branch: false,
            pull_before_create: false,
            copy_uncommitted_changes: true,
        };
        let run = drive_create_job_run(repo.path(), request);
        assert!(run.failure.is_none(), "creation must succeed");
        let session = run.session.unwrap();
        let worktree = PathBuf::from(session.directory());
        assert!(
            worktree.join("keep.txt").exists(),
            "the uncommitted rm must NOT be applied across different commits"
        );
        let status = run.status_message.unwrap();
        assert!(
            status.contains("were not copied"),
            "the skip must be visible in the status message, got: {status}"
        );
    }

    /// Ticking "copy my uncommitted changes" must not move the user's branch:
    /// with the pull off there is no switch at all.
    #[test]
    fn copy_only_creation_does_not_switch_the_shared_checkout() {
        let repo = init_test_repo();
        git_in(repo.path(), &["switch", "-c", "feature"]);
        assert_eq!(
            git_stdout(repo.path(), &["symbolic-ref", "--short", "HEAD"]),
            "feature"
        );

        let mut project = test_project(repo.path());
        project.current_branch = "feature".to_string();
        let request = CreateAgentRequest::NewProject {
            project,
            custom_name: Some("copy-target".to_string()),
            use_existing_branch: false,
            pull_before_create: false,
            copy_uncommitted_changes: true,
        };
        let run = drive_create_job_run(repo.path(), request);
        assert!(run.failure.is_none(), "creation must succeed");
        assert_eq!(
            git_stdout(repo.path(), &["symbolic-ref", "--short", "HEAD"]),
            "feature",
            "a copy-only creation must not switch the shared checkout"
        );
    }

    #[test]
    fn fresh_agent_with_copy_disabled_copies_nothing() {
        let repo = init_test_repo();
        std::fs::write(repo.path().join("note.txt"), "untracked\n").unwrap();

        let run = drive_create_job_run(repo.path(), new_project_request(repo.path(), false, false));
        assert!(run.failure.is_none(), "creation must succeed");
        let worktree = PathBuf::from(run.session.as_ref().unwrap().directory());
        assert!(!worktree.join("note.txt").exists());
    }

    /// A failed pull is a warning note, not a creation failure.
    #[test]
    fn fresh_agent_creation_survives_pull_failure() {
        let repo = init_test_repo();
        git_in(
            repo.path(),
            &["remote", "add", "origin", "/nonexistent/dux-test-origin"],
        );

        let run = drive_create_job_run(repo.path(), new_project_request(repo.path(), true, false));
        assert!(run.failure.is_none(), "creation must survive a failed pull");
        let status = run.status_message.unwrap();
        assert!(
            status.contains("could not pull"),
            "the pull failure must be visible in the status message, got: {status}"
        );
    }

    /// A leading branch origin has never had (a project added on a local
    /// feature branch) has nothing to pull: the pull is skipped quietly
    /// rather than reported as a failure on every create.
    #[test]
    fn fresh_agent_skips_the_pull_quietly_when_origin_has_no_such_branch() {
        let repo = init_test_repo();
        let bare = tempfile::tempdir().unwrap();
        git_in(bare.path(), &["init", "--bare", "-b", "main"]);
        git_in(
            repo.path(),
            &["remote", "add", "origin", bare.path().to_str().unwrap()],
        );
        git_in(repo.path(), &["push", "origin", "main"]);
        git_in(repo.path(), &["switch", "-c", "feature"]);

        let mut project = test_project(repo.path());
        project.leading_branch = Some("feature".to_string());
        project.current_branch = "feature".to_string();
        let request = CreateAgentRequest::NewProject {
            project,
            custom_name: Some("local-base".to_string()),
            use_existing_branch: false,
            pull_before_create: true,
            copy_uncommitted_changes: false,
        };
        let run = drive_create_job_run(repo.path(), request);

        assert!(
            run.failure.is_none(),
            "creation must succeed: {:?}",
            run.failure
        );
        assert!(run.session.is_some(), "the agent is created");
        let status = run.status_message.unwrap();
        assert!(
            !status.contains("Warning"),
            "a branch origin never had is not a pull failure: {status}"
        );
    }

    /// Whether origin has the branch is asked of origin itself: a clone that
    /// has never fetched it still pulls the newer commit.
    #[test]
    fn fresh_agent_pulls_a_branch_origin_has_even_when_this_clone_never_fetched_it() {
        let repo = init_test_repo();
        let bare = tempfile::tempdir().unwrap();
        git_in(bare.path(), &["init", "--bare", "-b", "main"]);
        git_in(
            repo.path(),
            &["remote", "add", "origin", bare.path().to_str().unwrap()],
        );
        git_in(repo.path(), &["push", "origin", "main"]);
        // Someone else pushes a newer commit from their own clone.
        let other = tempfile::tempdir().unwrap();
        git_in(other.path(), &["clone", bare.path().to_str().unwrap(), "."]);
        git_in(other.path(), &["config", "user.name", "test"]);
        git_in(other.path(), &["config", "user.email", "t@t"]);
        std::fs::write(other.path().join("upstream.txt"), "newer\n").unwrap();
        git_in(other.path(), &["add", "-A"]);
        git_in(other.path(), &["commit", "-m", "newer upstream"]);
        git_in(other.path(), &["push", "origin", "main"]);
        // And this clone holds no tracking ref for it.
        git_in(
            repo.path(),
            &["update-ref", "-d", "refs/remotes/origin/main"],
        );

        let run = drive_create_job_run(repo.path(), new_project_request(repo.path(), true, false));

        assert!(
            run.failure.is_none(),
            "creation must succeed: {:?}",
            run.failure
        );
        let worktree = PathBuf::from(run.session.unwrap().directory());
        assert!(
            worktree.join("upstream.txt").exists(),
            "the pull must have run and brought in origin's newer commit"
        );
    }

    /// A dirty checkout does not block the pull, and git itself fast-forwards
    /// when nothing conflicts.
    #[test]
    fn fresh_agent_creation_survives_dirty_checkout_and_still_pulls() {
        let repo = init_test_repo();
        std::fs::write(repo.path().join("tracked.txt"), "base\n").unwrap();
        git_in(repo.path(), &["add", "-A"]);
        git_in(repo.path(), &["commit", "-m", "base"]);

        // A bare origin one commit ahead on an unrelated file.
        let bare = tempfile::tempdir().unwrap();
        git_in(bare.path(), &["init", "--bare", "-b", "main"]);
        git_in(
            repo.path(),
            &["remote", "add", "origin", bare.path().to_str().unwrap()],
        );
        git_in(repo.path(), &["push", "origin", "main"]);
        let staging = tempfile::tempdir().unwrap();
        git_in(
            staging.path(),
            &["clone", bare.path().to_str().unwrap(), "."],
        );
        git_in(staging.path(), &["config", "user.name", "test"]);
        git_in(staging.path(), &["config", "user.email", "t@t"]);
        std::fs::write(staging.path().join("upstream.txt"), "ahead\n").unwrap();
        git_in(staging.path(), &["add", "-A"]);
        git_in(staging.path(), &["commit", "-m", "upstream"]);
        git_in(staging.path(), &["push", "origin", "main"]);

        // A tracked local edit that would have tripped the old dirty gate.
        std::fs::write(repo.path().join("tracked.txt"), "dirty\n").unwrap();

        let run = drive_create_job_run(repo.path(), new_project_request(repo.path(), true, true));
        assert!(
            run.failure.is_none(),
            "a dirty checkout must not block creation: {:?}",
            run.failure
        );
        let session = run.session.unwrap();
        let worktree = PathBuf::from(session.directory());
        assert!(
            worktree.join("upstream.txt").exists(),
            "the pull must have fast-forwarded"
        );
        assert_eq!(
            std::fs::read_to_string(worktree.join("tracked.txt")).unwrap(),
            "dirty\n",
            "the local edit still travels"
        );
    }

    /// The local-only flagship case: no origin means a log-only pull skip
    /// (no warning), and the copy still runs.
    #[test]
    fn fresh_agent_skips_pull_without_origin_and_still_copies() {
        let repo = init_test_repo();
        std::fs::write(repo.path().join("note.txt"), "untracked\n").unwrap();

        let run = drive_create_job_run(repo.path(), new_project_request(repo.path(), true, true));
        assert!(run.failure.is_none());
        let session = run.session.unwrap();
        let worktree = PathBuf::from(session.directory());
        assert_eq!(
            std::fs::read_to_string(worktree.join("note.txt")).unwrap(),
            "untracked\n"
        );
        let status = run.status_message.unwrap();
        assert!(
            !status.contains("Warning"),
            "a missing origin is steady state, not a warning: {status}"
        );
    }

    /// No per-path exceptions: attaching to an existing branch copies when
    /// the checkout and the branch are on the same commit, and skips with a
    /// note when they are not.
    #[test]
    fn attach_existing_branch_copies_when_heads_match_and_skips_when_not() {
        // Same tip: the dirt travels.
        let repo = init_test_repo();
        create_branch(repo.path(), "same-tip");
        std::fs::write(repo.path().join("note.txt"), "untracked\n").unwrap();
        let request = CreateAgentRequest::NewProject {
            project: test_project(repo.path()),
            custom_name: Some("same-tip".to_string()),
            use_existing_branch: true,
            pull_before_create: false,
            copy_uncommitted_changes: true,
        };
        let run = drive_create_job_run(repo.path(), request);
        assert!(run.failure.is_none());
        let worktree = PathBuf::from(run.session.unwrap().directory());
        assert_eq!(
            std::fs::read_to_string(worktree.join("note.txt")).unwrap(),
            "untracked\n"
        );

        // Different tip: skipped with a visible note.
        let repo = init_test_repo();
        create_branch(repo.path(), "other-tip");
        git_in(repo.path(), &["commit", "--allow-empty", "-m", "advance"]);
        std::fs::write(repo.path().join("note.txt"), "untracked\n").unwrap();
        let request = CreateAgentRequest::NewProject {
            project: test_project(repo.path()),
            custom_name: Some("other-tip".to_string()),
            use_existing_branch: true,
            pull_before_create: false,
            copy_uncommitted_changes: true,
        };
        let run = drive_create_job_run(repo.path(), request);
        assert!(run.failure.is_none());
        let worktree = PathBuf::from(run.session.unwrap().directory());
        assert!(!worktree.join("note.txt").exists());
        let status = run.status_message.unwrap();
        assert!(
            status.contains("were not copied"),
            "the skip must be visible: {status}"
        );
    }

    /// Gitignored files do not travel on forks.
    #[test]
    fn fork_copy_excludes_gitignored_files() {
        let repo = init_test_repo();
        std::fs::write(repo.path().join(".gitignore"), "*.log\n").unwrap();
        git_in(repo.path(), &["add", "-A"]);
        git_in(repo.path(), &["commit", "-m", "gitignore"]);
        std::fs::write(repo.path().join("note.txt"), "untracked\n").unwrap();
        std::fs::write(repo.path().join("junk.log"), "ignored\n").unwrap();

        let source = fork_source_session(repo.path());
        let request = CreateAgentRequest::ForkSession {
            project: test_project(repo.path()),
            source_session: Box::new(source),
            source_label: "src agent".to_string(),
            custom_name: Some("forked-agent".to_string()),
        };
        let run = drive_create_job_run(repo.path(), request);
        assert!(
            run.failure.is_none(),
            "the fork must succeed: {:?}",
            run.failure
        );
        let worktree = PathBuf::from(run.session.as_ref().unwrap().directory());
        assert_eq!(
            std::fs::read_to_string(worktree.join("note.txt")).unwrap(),
            "untracked\n"
        );
        assert!(!worktree.join("junk.log").exists());
    }

    /// Ordering: the copy runs AFTER the provider availability check, so an
    /// unavailable provider fails before any copy progress is reported.
    #[test]
    fn provider_failure_after_worktree_creation_does_not_report_copied_then_discard() {
        let repo = init_test_repo();
        std::fs::write(repo.path().join("note.txt"), "untracked\n").unwrap();
        let mut project = test_project(repo.path());
        project.default_provider = ProviderKind::new("definitely-not-a-real-command-dux");
        let request = CreateAgentRequest::NewProject {
            project,
            custom_name: Some("copy-target".to_string()),
            use_existing_branch: false,
            pull_before_create: false,
            copy_uncommitted_changes: true,
        };
        let run = drive_create_job_run(repo.path(), request);
        assert!(
            run.failure.is_some(),
            "the unavailable provider must fail the job"
        );
        assert!(
            !run.progress
                .iter()
                .any(|message| message.contains("Copying uncommitted")),
            "no copy progress may be reported before the provider check: {:?}",
            run.progress
        );
    }
}
