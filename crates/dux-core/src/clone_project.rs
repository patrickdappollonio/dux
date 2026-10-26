//! Cloning a repository as a new project and starting its first agent there.
//!
//! One request is one operation with one final. The checks that need neither
//! git nor the destination run on the engine thread when the request arrives
//! (`Engine::begin_clone_project`), which also takes the destination's
//! in-flight key. A worker then checks the destination and the name with git,
//! clones, and reads what it cloned ([`run_clone_job`]). Its
//! `WorkerEvent::RepositoryCloned` clears the key; a clone that stopped short
//! of a project ends there, and one that is ready to add becomes
//! `EventReaction::AddProjectAfterClone`, which exactly one surface drives
//! (`Engine::finish_clone`): it adds the project through the inline add every
//! path uses and dispatches the agent create, whose busy and final take over
//! from the clone's own spinner.

use crate::add_project_plan::DefaultCheckout;
use crate::project_browser::project_base_for_add;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::Duration;

use crate::engine::{Final, HandlerStatusOp};
use crate::git::CloneProcesses;
use crate::pr_reference::PullRequestCloneTarget;
use crate::status_text::{ReviewCopyOutlook, StatusAsk, StatusText};
use crate::worker::WorkerEvent;

/// What a client asks for: clone `url` into `path` and start an agent named
/// `agent_name` there, or a random name when it is blank, as in the New agent
/// dialog. `random_name` is that dialog's checkbox, as the client sent it; a
/// blank name gets a random one either way.
///
/// With `pull_request` the clone is for a new agent from that pull request:
/// the engine builds the address itself and ignores `url`, and a blank name
/// stays blank until the worker has looked the pull request up. With
/// `add_existing` as well, `path` is a folder that already holds a checkout of
/// the repository: nothing is cloned, the folder becomes the project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloneRequest {
    pub url: String,
    pub path: String,
    pub agent_name: Option<String>,
    pub random_name: bool,
    pub pull_request: Option<PullRequestCloneTarget>,
    pub add_existing: bool,
    /// With `pull_request`: the agreement to a review copy given before the
    /// clone or the add.
    pub review_copy: UpfrontReviewCopy,
}

/// The agreement to a review copy that came with a request made before the
/// project existed, from a dialog that showed the warning or a command-line
/// flag. `reason` is the word the warning's reason began with (see
/// [`crate::status_text::ReviewCopyOutlook::Needed`]); an agreement with no
/// reason, the flag's, covers whatever copy turns out to be needed.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UpfrontReviewCopy {
    #[serde(default)]
    pub agreed: bool,
    #[serde(default)]
    pub reason: Option<String>,
    /// The branch the warning said the copy would be made on. An agreement that
    /// names none covers the reason under any name.
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub by_flag: bool,
}

impl UpfrontReviewCopy {
    pub fn consent(&self) -> crate::worker::ReviewCopyConsent {
        crate::worker::ReviewCopyConsent::from_upfront(
            crate::worker::ReviewCopyAnswer {
                agreed: self.agreed,
                by_flag: self.by_flag,
            },
            self.reason.clone(),
            self.branch.clone(),
        )
    }
}

/// What the operation does to its folder, which every sentence it can print
/// is worded by: a clone never says it added, and an add never says it cloned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CloneMode {
    #[default]
    Clone,
    AddExisting,
}

/// What a folder offered for a pull request's repository holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrDestination {
    /// Missing or an empty folder: a clone can go there.
    Free,
    /// The top of a working copy whose `origin` is the repository: it can
    /// become the project as it is.
    Checkout,
}

/// How a clone's operation ended. The worker answers one of the first three
/// when it stopped short of adding a project; the follow-up answers the rest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloneOutcome {
    /// A check refused the request before git ran.
    Refused(StatusText),
    /// git failed, stalled or was stopped; nothing was added.
    Failed(StatusText),
    /// Some of it was done and the rest was not: the clone is on disk, and
    /// the sentence says what is missing and what to do next.
    Kept(StatusText),
    /// The agent create took over; its own final is the one the user sees.
    HandedOff,
}

impl CloneOutcome {
    /// The final the clone's spinner resolves to. A kept clone is sticky: the
    /// work is half done and the way on is outside the toast.
    pub fn final_status(&self) -> Final {
        match self {
            Self::Refused(text) | Self::Failed(text) => Final::error(text.clone()),
            Self::Kept(text) => Final::warning(text.clone()).sticky(),
            Self::HandedOff => Final::clear(),
        }
    }
}

/// A clone that is waiting for its worker, with whether a browser asked for
/// it (whose follow-up the web layer then drives).
pub struct PendingClone {
    pub op: HandlerStatusOp<CloneOutcome>,
    pub from_web: bool,
    /// The repository key a pull request clone holds, released once the
    /// project is registered or the clone ends without one; `None` for an
    /// ordinary clone.
    pub repository_key: Option<String>,
    pub mode: CloneMode,
    /// The folder the operation clones into or adds, to word a refusal by what
    /// the holder of that folder is doing.
    pub destination: PathBuf,
    /// The agreement to a review copy the request came with, handed to the
    /// agent's create after the project exists.
    pub review_copy: crate::worker::ReviewCopyConsent,
}

/// The engine's clones: the operations waiting for their worker, and the git
/// processes running, which quitting dux stops.
#[derive(Default)]
pub struct Clones {
    pub pending: HashMap<String, PendingClone>,
    pub processes: CloneProcesses,
    /// Extra variables for every clone's git, empty outside tests.
    pub git_env: Vec<(String, String)>,
}

/// A request that passed the checks the engine thread runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedClone {
    pub url: String,
    /// The destination's identity: its parent canonicalized, plus its name.
    pub destination: PathBuf,
    pub agent_name: String,
}

/// Check `request` without git or the destination itself, resolving the agent
/// name: a blank one gets a random name here, so nothing later sees an empty
/// one, except for a pull request, whose blank name waits for the worker.
/// `projects` are the workspace's project paths and `home` expands a leading
/// `~`.
pub fn prepare_clone(
    request: &CloneRequest,
    home: Option<&Path>,
    projects: &[&str],
) -> Result<PreparedClone, StatusText> {
    let url = request.url.trim();
    if url.is_empty() {
        return Err("Paste the address of the repository to clone.".into());
    }
    let typed = request.agent_name.as_deref().unwrap_or("").trim();
    let agent_name = if !typed.is_empty() {
        if !crate::git::is_valid_agent_name(typed) {
            return Err(crate::status_text![
                "Invalid agent name ",
                q(typed),
                ". ",
                crate::git::AGENT_NAME_RULE
            ]);
        }
        typed.to_string()
    } else if request.pull_request.is_some() {
        // The pull request's own branch names the agent, which only the
        // worker's lookup can say.
        String::new()
    } else {
        crate::git::docker_style_name()
    };
    let destination = destination_identity(&request.path, home)?;
    refuse_project_destination(&destination, projects)?;
    Ok(PreparedClone {
        url: url.to_string(),
        destination,
        agent_name,
    })
}

/// Refuse a destination that is already a project. Both sides are
/// canonicalized, so a project saved under another spelling of the folder
/// (through a symbolic link, say) still matches.
fn refuse_project_destination(destination: &Path, projects: &[&str]) -> Result<(), StatusText> {
    let destination_text = destination.to_string_lossy();
    if projects
        .iter()
        .any(|project| crate::project_browser::same_directory(project, &destination_text))
    {
        return Err(crate::status_text![
            n(destination.display()),
            " is already a project in the workspace. Pick another destination for the clone."
        ]);
    }
    Ok(())
}

/// What `raw` holds, judged for the repository of `target`, for a pull request
/// that would clone there or add what is there. Runs git, so only on a worker.
/// The folder is never changed.
pub fn inspect_pr_destination(
    raw: &str,
    target: &PullRequestCloneTarget,
    policy: &crate::gh::GithubHostPolicy,
    home: Option<&Path>,
    projects: &[&str],
) -> Result<PrDestination, StatusText> {
    let destination = destination_identity(raw, home)?;
    refuse_project_destination(&destination, projects)?;
    if destination_occupancy(&destination)? == Occupancy::Empty {
        return Ok(PrDestination::Free);
    }
    let strip = |name: &str| name.strip_suffix(".git").unwrap_or(name).to_lowercase();
    let is_checkout = matches!(
        crate::git::repo_path_kind(&destination),
        crate::git::RepoPathKind::WorkTreeRoot
    ) && matches!(
        crate::git::resolve_remote_github_repo(&destination, policy),
        crate::git::RemoteResolution::Allowed(remote)
            if remote.host.eq_ignore_ascii_case(&target.host)
                && strip(&remote.owner_repo) == strip(&target.owner_repo)
    );
    if is_checkout {
        return Ok(PrDestination::Checkout);
    }
    Err(crate::status_text![
        n(destination.display()),
        " already holds something that isn't a checkout of ",
        n(format!("{}/{}", target.host, target.owner_repo)),
        ". Pick another folder."
    ])
}

/// The pull request a folder check is for, when the answer for a checkout
/// should also say whether the agent will need a review copy there.
pub struct FolderReviewCopyQuery<'a> {
    pub worktrees_root: &'a Path,
    pub number: u64,
    pub head_branch: &'a str,
    pub head_commit: Option<&'a str>,
    /// The name the agent will be given: one spelled like a review copy of the
    /// branch asks for a copy, until a local branch of that name exists.
    pub agent_name: Option<&'a str>,
}

/// How long a folder check waits for the repository to say whether a review
/// copy will be needed, the bound a `gh` call has.
const REVIEW_COPY_CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// What the folder holds, the folder as dux will use it, and, for a checkout
/// and a `review` query, whether the agent will need a review copy there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrDestinationCheck {
    pub state: PrDestination,
    pub path: PathBuf,
    pub review_copy: Option<ReviewCopyOutlook>,
}

/// [`inspect_pr_destination`] plus the folder's canonical spelling and the
/// review copy outlook for a checkout: the one answer both the browser's
/// folder check and the terminal UI's give. Runs git, so only on a worker. The
/// outlook is told from the folder as it is, never from a write, and a folder
/// that does not answer in time leaves the question open for the create to ask.
pub fn check_pr_destination(
    raw: &str,
    target: &PullRequestCloneTarget,
    policy: &crate::gh::GithubHostPolicy,
    projects: &[&str],
    review: Option<FolderReviewCopyQuery<'_>>,
) -> Result<PrDestinationCheck, StatusText> {
    let home = std::env::home_dir();
    let state = inspect_pr_destination(raw, target, policy, home.as_deref(), projects)?;
    let path = destination_identity(raw, home.as_deref())?;
    // A name typed like a review copy is asked about whatever the folder
    // holds; the pull request's own branch only where there is a checkout.
    let review_copy = match review {
        Some(query)
            if query
                .agent_name
                .is_some_and(|name| name != query.head_branch)
                || state == PrDestination::Checkout =>
        {
            let (folder, root) = (path.clone(), query.worktrees_root.to_path_buf());
            let (branch, commit) = (
                query.head_branch.to_string(),
                query.head_commit.map(String::from),
            );
            let number = query.number;
            let checkout = state == PrDestination::Checkout;
            let typed = query
                .agent_name
                .filter(|name| *name != query.head_branch)
                .map(String::from);
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let outlook = match typed {
                    Some(name) => crate::agent_job::typed_name_review_copy_outlook(
                        checkout.then_some(folder.as_path()),
                        &branch,
                        &name,
                        number,
                    ),
                    None => crate::agent_job::folder_review_copy_outlook(
                        &folder,
                        &root,
                        &branch,
                        commit.as_deref(),
                        number,
                    ),
                };
                let _ = tx.send(outlook);
            });
            Some(
                rx.recv_timeout(REVIEW_COPY_CHECK_TIMEOUT)
                    .unwrap_or(ReviewCopyOutlook::Unknown),
            )
        }
        _ => None,
    };
    Ok(PrDestinationCheck {
        state,
        path,
        review_copy,
    })
}

/// The destination `raw` names, after expanding a leading `~`: its parent
/// canonicalized plus its own name, so two spellings of one folder (a trailing
/// slash, a symlinked parent) are one destination.
pub fn destination_identity(raw: &str, home: Option<&Path>) -> Result<PathBuf, StatusText> {
    let trimmed = raw.trim();
    let expanded = match (trimmed.strip_prefix('~'), home) {
        (Some(""), Some(home)) => home.to_path_buf(),
        (Some(rest), Some(home)) if rest.starts_with('/') => {
            home.join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(trimmed),
    };
    if trimmed.is_empty() || !expanded.is_absolute() {
        return Err(crate::status_text![
            "The destination ",
            q(trimmed),
            " isn't an absolute path. Type the full path of the folder to clone into."
        ]);
    }
    let (Some(parent), Some(name)) = (expanded.parent(), expanded.file_name()) else {
        return Err(crate::status_text![
            "The destination ",
            n(expanded.display()),
            " names no folder to clone into. Type the full path of a new folder."
        ]);
    };
    match parent.canonicalize() {
        Ok(parent) if parent.is_dir() => Ok(parent.join(name)),
        _ => Err(crate::status_text![
            "The folder ",
            n(parent.display()),
            " that would hold the clone doesn't exist. Create it first, or pick another \
             destination."
        ]),
    }
}

/// What the worker learned about a clone that is ready to become a project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClonedRepository {
    /// The address, with any user and token removed, for the messages.
    pub address: String,
    /// The branch the clone checked out: the remote's default. For a folder
    /// that was added, the branch it has checked out.
    pub branch: String,
    /// The base new worktrees branch from.
    pub leading_branch: String,
    /// Set when `leading_branch` is the folder's own pick rather than the
    /// remote's default, so the user is told.
    pub leading_branch_guess: Option<LeadingBranchGuess>,
    /// A branch named like the agent already exists in the clone. Never set
    /// for a pull request, whose create decides for itself what to do with a
    /// branch of that name.
    pub name_taken: bool,
    /// The pull request the worker looked up again, for a clone that ends in
    /// an agent from one.
    pub pull_request: Option<ClonedPullRequest>,
}

/// Why an added folder leads with a branch other than the remote's default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeadingBranchGuess {
    /// The remote's default branch when it could be learned, but the folder
    /// has no branch of that name; `None` when nothing could say.
    pub remote_default: Option<String>,
    /// The leading branch is the one the folder has checked out; otherwise
    /// nothing is checked out and it is `main`.
    pub checked_out: bool,
}

/// The pull request a clone's worker looked up, and the agent name it settled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClonedPullRequest {
    pub target: PullRequestCloneTarget,
    pub title: String,
    pub state: String,
    pub head_branch: String,
    /// The typed name, or the head branch when none was typed.
    pub agent_name: String,
    /// The pull request's head commit, when `gh` reported it.
    pub head_oid: Option<String>,
}

/// What a clone's first agent create carries about the clone, so it refuses
/// an existing branch and its failure says what the clone already did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClonedFor {
    /// The address, with any user and token removed.
    pub address: String,
    pub path: PathBuf,
    pub project_name: String,
    pub project_id: String,
    pub agent_name: String,
    pub mode: CloneMode,
    /// The pull request the agent is for, when it is for one.
    pub pull_request: Option<PullRequestCloneTarget>,
}

impl ClonedFor {
    /// `reason` the agent is missing, after what the clone did.
    pub fn agent_missing(&self, reason: StatusText) -> StatusText {
        self.agent_missing_named(None, reason)
    }

    /// What was done to the folder and the project, worded by the mode, up to
    /// where the sentence goes on about the agent.
    fn done_so_far(&self) -> StatusText {
        match self.mode {
            CloneMode::Clone => crate::status_text![
                "Cloned ",
                n(&self.address),
                " into ",
                n(self.path.display()),
                " and added project ",
                q(&self.project_name)
            ],
            CloneMode::AddExisting => crate::status_text![
                "Added ",
                n(self.path.display()),
                " as project ",
                q(&self.project_name)
            ],
        }
    }

    /// [`Self::agent_missing`] naming the agent the create actually tried to
    /// make when it got as far as settling a name, which a review copy of a
    /// pull request makes different from the one asked for.
    pub fn agent_missing_named(&self, attempted: Option<&str>, reason: StatusText) -> StatusText {
        let agent = attempted.unwrap_or(&self.agent_name);
        // Wrapping the reason copies its words, not the question it may put.
        let Some(StatusAsk::ReviewCopy(mut question)) = reason.ask().cloned() else {
            return crate::status_text![
                self.done_so_far(),
                ", but didn't create agent ",
                q(agent),
                ": ",
                reason
            ];
        };
        // The question is put again, where the project now is. Its sentence
        // does not send the user to a box they cannot see: the surface reopens
        // New agent from PR for the project, and the command line is told its
        // flag.
        let warning = crate::prose::Prose::from_segments(question.warning.clone());
        let how = if question.by_flag {
            crate::status_text![crate::agent_job::review_copy_how(
                crate::agent_job::AgreementChannel::CommandLineFlag
            )]
        } else {
            crate::status_text![
                " To go ahead anyway, use New agent from PR in project ",
                q(&self.project_name),
                " and tick Create a review copy."
            ]
        };
        question.project_id = Some(self.project_id.clone());
        question.pull_request = self.pull_request.clone();
        crate::status_text![
            self.done_so_far(),
            ". Agent ",
            q(agent),
            " still needs your answer on a review copy: ",
            warning,
            how
        ]
        .with_ask(StatusAsk::ReviewCopy(question))
    }

    /// The agent exists but its startup command failed, after what the clone
    /// did.
    pub fn startup_failed(&self, agent: &str, error: &str) -> StatusText {
        crate::status_text![
            self.done_so_far(),
            ". Agent ",
            q(agent),
            format!(
                " was created, but its startup command failed: {error}. Open the startup \
                 command logs for details."
            )
        ]
    }
}

/// A clone ready to be added as a project, as the follow-up receives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClonedProject {
    /// The clone's op, which the follow-up resolves.
    pub status_op_id: Option<String>,
    pub path: PathBuf,
    pub agent_name: String,
    pub cloned: ClonedRepository,
    /// The repository key the clone held, to release when the pending clone
    /// is gone.
    pub repository_key: Option<String>,
    pub mode: CloneMode,
}

/// What a pull request clone's worker checks and looks up before it claims the
/// folder.
pub struct PullRequestJob {
    pub target: PullRequestCloneTarget,
    /// The projects as they were when the request was accepted.
    pub projects: Vec<crate::model::Project>,
    pub policy: crate::gh::GithubHostPolicy,
    /// The program the lookup runs: `gh`, or a test's stand-in.
    pub gh_program: std::ffi::OsString,
    /// The agreement to a review copy the request came with.
    pub review_copy: crate::worker::ReviewCopyConsent,
    /// Where managed worktrees live, which decides what name a copy can take.
    pub worktrees_root: PathBuf,
}

/// Everything the worker needs.
pub struct CloneJob {
    pub status_op_id: String,
    pub url: String,
    pub destination: PathBuf,
    /// Blank for a pull request clone whose name is the pull request's branch.
    pub agent_name: String,
    pub stall: Duration,
    pub processes: CloneProcesses,
    pub pull_request: Option<PullRequestJob>,
    /// Held by a pull request clone; carried so the answer can release it.
    pub repository_key: Option<String>,
    pub mode: CloneMode,
    /// Extra variables for git's environment. Empty outside tests, which use
    /// it to point an address at a local remote.
    pub git_env: Vec<(String, String)>,
}

/// Check the destination and the agent name with git, clone, and read what
/// was cloned; posts `WorkerEvent::RepositoryCloned` either way.
pub fn run_clone_job(job: CloneJob, worker_tx: &Sender<WorkerEvent>) {
    let result = clone_and_inspect(&job);
    // A pull request settles the agent's name on the worker, from the typed
    // name or the pull request's branch.
    let agent_name = match &result {
        Ok(ClonedRepository {
            pull_request: Some(pull_request),
            ..
        }) => pull_request.agent_name.clone(),
        _ => job.agent_name,
    };
    let _ = worker_tx.send(WorkerEvent::RepositoryCloned {
        status_op_id: Some(job.status_op_id),
        path: job.destination,
        agent_name,
        repository_key: job.repository_key,
        mode: job.mode,
        result,
    });
}

/// What a clone for a pull request settles before it claims any folder: no
/// project has become a checkout of the repository since the request was
/// accepted, the pull request still exists (this answer wins over the one the
/// offer showed), and the agent has a usable name.
fn settle_pull_request(
    job: &CloneJob,
    pull_request: &PullRequestJob,
    parent: &Path,
) -> Result<ClonedPullRequest, CloneOutcome> {
    let target = &pull_request.target;
    let reference = crate::pr_reference::TypedReference {
        host: Some(target.host.clone()),
        owner_repo: Some(target.owner_repo.clone()),
        number: Some(target.number),
    };
    let repository = format!("{}/{}", target.host, target.owner_repo);
    let (did_not, nothing_was) = match job.mode {
        CloneMode::Clone => ("clone", " Nothing was cloned."),
        CloneMode::AddExisting => ("add", " Nothing was added."),
    };
    let resolution = crate::pr_reference::resolve_reference_projects(
        &reference,
        &pull_request.projects,
        &pull_request.policy,
    );
    if let Some(project) = resolution.matches.first() {
        return Err(CloneOutcome::Refused(crate::status_text![
            q(project.name),
            " is already a checkout of ",
            n(repository),
            format!(
                ", so dux did not {did_not} it again. Use New agent from PR again; it will \
                 find that project now."
            )
        ]));
    }
    let lookup = crate::worker::PullRequestLookup {
        host: target.host.clone(),
        owner_repo: target.owner_repo.clone(),
        number: target.number,
    };
    let metadata =
        crate::gh::look_up_pull_request(&pull_request.gh_program, &lookup).map_err(|said| {
            let said = said.trim_end();
            let stop = if said.ends_with(['.', '!', '?']) {
                ""
            } else {
                "."
            };
            CloneOutcome::Refused(crate::status_text![format!("{said}{stop}"), nothing_was])
        })?;
    let typed = job.agent_name.trim();
    let agent_name = if typed.is_empty() {
        let branch = metadata.head_ref_name.as_str();
        if !crate::git::is_valid_agent_name(branch)
            || !crate::git::is_valid_branch_name(parent, branch)
        {
            return Err(CloneOutcome::Refused(crate::status_text![
                "PR ",
                n(format!("#{}", target.number)),
                " has head branch ",
                q(branch),
                format!(
                    ", which is not a usable agent name. Type a name for the agent instead. \
                     {}{nothing_was}",
                    crate::git::AGENT_NAME_RULE
                )
            ]));
        }
        branch.to_string()
    } else {
        typed.to_string()
    };
    // A clone has the repository's default branch checked out and no other
    // local branch, so an agent on the pull request's own branch needs a copy
    // when that is the default branch, and so does one named like a copy of it.
    // An add is told from the folder instead, once it is known to be a
    // checkout. An agreement that covers whatever copy is needed makes the
    // question moot, so it skips the lookup.
    if job.mode == CloneMode::Clone
        && !matches!(
            pull_request.review_copy,
            crate::worker::ReviewCopyConsent::Blanket
        )
    {
        let outlook = if agent_name == metadata.head_ref_name {
            let default_branch = crate::gh::look_up_default_branch(
                &pull_request.gh_program,
                &target.host,
                &target.owner_repo,
            )
            .ok();
            let project = crate::wire::display_project_name("", &job.destination.to_string_lossy());
            crate::agent_job::clone_review_copy_outlook(
                &metadata.head_ref_name,
                default_branch.as_deref(),
                target.number,
                &|candidate| {
                    crate::agent_job::worktree_folder_is_free(
                        &pull_request.worktrees_root,
                        &project,
                        candidate,
                    )
                },
            )
        } else {
            crate::agent_job::typed_copy_review_outlook(
                &metadata.head_ref_name,
                &agent_name,
                target.number,
            )
            .unwrap_or(ReviewCopyOutlook::NotNeeded)
        };
        refuse_unagreed_review_copy(
            &pull_request.review_copy,
            outlook,
            CloneMode::Clone,
            target,
            nothing_was,
        )?;
    }
    Ok(ClonedPullRequest {
        target: target.clone(),
        title: metadata.title,
        state: metadata.state,
        head_branch: metadata.head_ref_name,
        agent_name,
        head_oid: metadata.head_ref_oid,
    })
}

/// Refuse, before anything is cloned or added, a pull request whose agent
/// will need a review copy that nobody agreed to, or one that no free name can
/// be given to. An agreement counts only for the reason and the copy branch it
/// was shown.
///
/// The refusal carries the question, so the dialog that was closed when the
/// request was accepted can reopen with it: `warned` is the reason and the copy
/// branch, as an outlook gives them, and no project exists yet.
fn refuse_unagreed_review_copy(
    consent: &crate::worker::ReviewCopyConsent,
    outlook: ReviewCopyOutlook,
    mode: CloneMode,
    target: &crate::pr_reference::PullRequestCloneTarget,
    nothing_was: &str,
) -> Result<(), CloneOutcome> {
    match outlook {
        ReviewCopyOutlook::Needed {
            reason,
            copy_branch,
            warning,
            ..
        } => {
            if consent.covers_shown(&reason, &copy_branch) {
                return Ok(());
            }
            let by_flag = matches!(consent, crate::worker::ReviewCopyConsent::WithheldByFlag);
            let ask = StatusAsk::ReviewCopy(Box::new(crate::status_text::ReviewCopyAsk {
                warned: format!("{reason}:{copy_branch}"),
                warning: warning.clone(),
                request: None,
                by_flag,
                project_id: None,
                pull_request: Some(target.clone()),
            }));
            Err(CloneOutcome::Refused(
                crate::status_text![
                    crate::prose::Prose::from_segments(warning),
                    crate::agent_job::review_copy_how_before(
                        if by_flag {
                            crate::agent_job::AgreementChannel::CommandLineFlag
                        } else {
                            crate::agent_job::AgreementChannel::DialogBox
                        },
                        mode
                    ),
                    nothing_was
                ]
                .with_ask(ask),
            ))
        }
        ReviewCopyOutlook::NoFreeName { message, .. } => {
            Err(CloneOutcome::Refused(crate::status_text![
                crate::prose::Prose::from_segments(message),
                nothing_was
            ]))
        }
        ReviewCopyOutlook::NotNeeded | ReviewCopyOutlook::Unknown => Ok(()),
    }
}

/// Give a clone made through a test's stand-in transport the same rewrite, so
/// the fetch that follows reaches the stand-in too: git records the address it
/// was given, and the rewrite applies only while the variables are set.
fn keep_env_config(dest: &Path, env: &[(String, String)]) {
    let count = env
        .iter()
        .find(|(key, _)| key == "GIT_CONFIG_COUNT")
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    let lookup = |name: String| env.iter().find(|(key, _)| *key == name).map(|(_, v)| v);
    for index in 0..count {
        let (Some(key), Some(value)) = (
            lookup(format!("GIT_CONFIG_KEY_{index}")),
            lookup(format!("GIT_CONFIG_VALUE_{index}")),
        ) else {
            continue;
        };
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(dest)
            .args(["config", "--add", "--", key, value])
            .output();
    }
}

fn clone_and_inspect(job: &CloneJob) -> Result<ClonedRepository, CloneOutcome> {
    let address = redact_remote_userinfo(&job.url);
    let dest = &job.destination;
    let parent = dest.parent().unwrap_or(dest);
    let pull_request = match &job.pull_request {
        Some(request) => Some(settle_pull_request(job, request, parent)?),
        None => None,
    };
    let agent_name = pull_request
        .as_ref()
        .map_or(job.agent_name.as_str(), |settled| {
            settled.agent_name.as_str()
        });
    if !crate::git::is_valid_branch_name(parent, agent_name) {
        return Err(CloneOutcome::Refused(crate::status_text![
            "git won't take ",
            q(agent_name),
            " as a branch name, so it can't name the agent. Pick another name."
        ]));
    }
    if job.mode == CloneMode::AddExisting {
        return add_existing_checkout(job, address, pull_request);
    }
    let claim = claim_destination(dest).map_err(CloneOutcome::Refused)?;
    crate::logger::info(&format!("cloning {address} into {}", dest.display()));
    let env: Vec<(&str, &str)> = job
        .git_env
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    if let Err(error) =
        crate::git::clone_repository_with_env(&job.url, dest, job.stall, &job.processes, &env)
    {
        crate::logger::error(&format!(
            "clone of {address} into {} failed: {error:?}",
            dest.display()
        ));
        claim.release_unused();
        let mut said = clone_failure(&address, dest, &error);
        if let (Some(request), crate::git::CloneError::Failed(_)) = (&job.pull_request, &error)
            && job.url.starts_with("git@")
        {
            let host = &request.target.host;
            said = crate::status_text![
                said,
                " If this is the first time this machine talks to ",
                n(host),
                " over SSH, connect once in a terminal (",
                n(format!("ssh -T git@{host}")),
                ") and try again."
            ];
        }
        return Err(CloneOutcome::Failed(said));
    }
    keep_env_config(dest, &job.git_env);
    match crate::git::repo_commit_state(dest) {
        crate::git::CommitState::Born => {}
        _ if crate::git::has_remote_tracking_refs(dest) => {
            return Err(CloneOutcome::Kept(crate::status_text![
                "Cloned ",
                n(address),
                " into ",
                n(dest.display()),
                ", but its default branch could not be checked out, so dux did not add it as \
                 a project. The clone is still there: check a branch out in it, then use Add \
                 project."
            ]));
        }
        _ => {
            return Err(CloneOutcome::Kept(crate::status_text![
                "Cloned ",
                n(address),
                " into ",
                n(dest.display()),
                ", but the repository has no commits yet, so dux did not add it as a project. \
                 The clone is still there: use Add project on it, which offers to create a \
                 first commit."
            ]));
        }
    }
    let branch = crate::git::current_branch_opt(dest)
        .ok()
        .flatten()
        .unwrap_or_default();
    let leading_branch =
        project_base_for_add(dest, Some(&branch), DefaultCheckout::StayOnCurrent).into_branch();
    // A pull request's create decides for itself what to do with a branch of
    // that name, so only a plain agent is refused one here.
    let name_taken = pull_request.is_none()
        && matches!(
            crate::git::create_agent_branch_preflight(dest, agent_name),
            crate::git::CreateAgentBranchPlan::ExistingBranch { .. }
        );
    Ok(ClonedRepository {
        address,
        branch,
        leading_branch,
        leading_branch_guess: None,
        name_taken,
        pull_request,
    })
}

/// The add half of the worker, in place of the claim and the clone: the folder
/// must still be a checkout of the repository, and the project leads with the
/// remote's default branch, whichever branch the folder has out. Nothing in the
/// folder is created, moved or removed.
fn add_existing_checkout(
    job: &CloneJob,
    address: String,
    pull_request: Option<ClonedPullRequest>,
) -> Result<ClonedRepository, CloneOutcome> {
    let (Some(request), Some(pull_request)) = (&job.pull_request, pull_request) else {
        return Err(CloneOutcome::Refused(
            "add_existing needs a pull request.".into(),
        ));
    };
    let dest = &job.destination;
    let refuse =
        |why: StatusText| CloneOutcome::Refused(crate::status_text![why, " Nothing was added."]);
    let projects: Vec<&str> = request.projects.iter().map(|p| p.path.as_str()).collect();
    match inspect_pr_destination(
        &dest.to_string_lossy(),
        &request.target,
        &request.policy,
        None,
        &projects,
    ) {
        Ok(PrDestination::Checkout) => {}
        Ok(PrDestination::Free) => {
            return Err(refuse(crate::status_text![
                n(dest.display()),
                " no longer holds a checkout of ",
                n(format!(
                    "{}/{}",
                    request.target.host, request.target.owner_repo
                )),
                "."
            ]));
        }
        Err(refused) => return Err(refuse(refused)),
    }
    let outlook = if pull_request.agent_name == pull_request.head_branch {
        crate::agent_job::folder_review_copy_outlook(
            dest,
            &request.worktrees_root,
            &pull_request.head_branch,
            pull_request.head_oid.as_deref(),
            request.target.number,
        )
    } else {
        crate::agent_job::typed_name_review_copy_outlook(
            Some(dest),
            &pull_request.head_branch,
            &pull_request.agent_name,
            request.target.number,
        )
    };
    refuse_unagreed_review_copy(
        &request.review_copy,
        outlook,
        CloneMode::AddExisting,
        &request.target,
        " Nothing was added.",
    )?;
    crate::logger::info(&format!("adding {} as a project", dest.display()));
    let unreadable = || {
        refuse(crate::status_text![
            "git could not read the history of ",
            n(dest.display()),
            ", so dux can't tell which branch could lead a project there."
        ])
    };
    match crate::git::repo_commit_state(dest) {
        crate::git::CommitState::Born => {}
        crate::git::CommitState::Unborn => {
            crate::logger::info(&format!("not adding {}: no commits", dest.display()));
            return Err(refuse(crate::status_text![
                n(dest.display()),
                " has no commits yet, so there is no branch to lead a project with. Make a \
                 first commit in it, then use New agent from PR again."
            ]));
        }
        crate::git::CommitState::Indeterminate => return Err(unreadable()),
    }
    let Ok(checked_out) = crate::git::current_branch_opt(dest) else {
        return Err(unreadable());
    };
    let env: Vec<(&str, &str)> = job
        .git_env
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let remote_default = crate::git::remote_default_branch(dest)
        .or_else(|| crate::git::remote_default_branch_from_remote(dest, &env));
    // The remote's default leads only when the folder has a branch of that
    // name: every later plain agent branches from the leading branch and is
    // refused when it does not exist locally.
    let local_default = remote_default
        .as_deref()
        .filter(|default| crate::git::local_branch_exists(dest, default));
    let (leading_branch, leading_branch_guess) = match crate::add_project_plan::project_base_at_add(
        checked_out.as_deref(),
        local_default,
        crate::add_project_plan::DefaultCheckout::CheckOutDefault,
    ) {
        crate::add_project_plan::ProjectBase::RemoteDefault(branch) => (branch, None),
        crate::add_project_plan::ProjectBase::Other(branch) => {
            // The branch checked out exists; the detached fallback is
            // only a name until it is looked for.
            if checked_out.is_none() && !crate::git::local_branch_exists(dest, &branch) {
                crate::logger::info(&format!(
                    "not adding {}: detached HEAD and no local {branch}",
                    dest.display()
                ));
                let tried = remote_default.as_deref().unwrap_or("main");
                let known = if remote_default.is_some() {
                    ", the remote's default"
                } else {
                    ""
                };
                return Err(refuse(crate::status_text![
                    n(dest.display()),
                    " has no branch to lead a project with: HEAD is detached and the \
                         folder has no local branch named ",
                    n(tried),
                    format!(
                        "{known}. Check a branch out in it, then use New agent from PR \
                             again."
                    )
                ]));
            }
            let guess = LeadingBranchGuess {
                remote_default: remote_default.clone(),
                checked_out: checked_out.is_some(),
            };
            (branch, Some(guess))
        }
    };
    keep_env_config(dest, &job.git_env);
    Ok(ClonedRepository {
        address,
        branch: checked_out.unwrap_or_default(),
        leading_branch,
        leading_branch_guess,
        name_taken: false,
        pull_request: Some(pull_request),
    })
}

/// The destination a clone goes into, and whether dux made it.
#[derive(Debug)]
pub struct DestinationClaim {
    path: PathBuf,
    created: bool,
}

impl DestinationClaim {
    /// Give the destination back after a clone that did not finish: the
    /// folder dux made goes, but only while it is still empty (a partial clone
    /// stays for the user to look at), and a folder dux did not make is never
    /// touched. `remove_dir` removes only an empty directory and never follows
    /// a symbolic link.
    pub fn release_unused(self) {
        if self.created {
            let _ = std::fs::remove_dir(&self.path);
        }
    }
}

/// Claim `dest` for a clone, right before git runs. A missing destination is
/// created here with one `mkdir`, which fails if anything (a link included)
/// appeared there, so git clones into a folder nobody else put in place. An
/// existing one must still be an empty folder and not a symbolic link; the
/// moment between this look and git starting is accepted, as anyone who can
/// race it can already run commands as the user.
pub fn claim_destination(dest: &Path) -> Result<DestinationClaim, StatusText> {
    match std::fs::create_dir(dest) {
        Ok(()) => {
            return Ok(DestinationClaim {
                path: dest.to_path_buf(),
                created: true,
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(crate::status_text![
                "Couldn't create ",
                n(dest.display()),
                format!(" to clone into: {error}.")
            ]);
        }
    }
    check_destination(dest)?;
    Ok(DestinationClaim {
        path: dest.to_path_buf(),
        created: false,
    })
}

/// Whether a destination that is not a symbolic link holds anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Occupancy {
    /// Missing, or an empty folder.
    Empty,
    /// A file, or a folder with something in it (or one dux cannot read).
    Occupied,
}

/// What `dest` holds, refusing a symbolic link (a clone goes only into a new or
/// an empty folder, and a link is neither) and a path that cannot be looked at.
fn destination_occupancy(dest: &Path) -> Result<Occupancy, StatusText> {
    match std::fs::symlink_metadata(dest) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Occupancy::Empty),
        Err(error) => Err(crate::status_text![
            "Couldn't look at ",
            n(dest.display()),
            format!(": {error}.")
        ]),
        Ok(meta) if meta.file_type().is_symlink() => Err(crate::status_text![
            n(dest.display()),
            " is a symbolic link. dux clones only into a new or an empty folder."
        ]),
        Ok(meta) if meta.is_dir() => match std::fs::read_dir(dest) {
            Ok(mut entries) => Ok(if entries.next().is_none() {
                Occupancy::Empty
            } else {
                Occupancy::Occupied
            }),
            Err(_) => Ok(Occupancy::Occupied),
        },
        Ok(_) => Ok(Occupancy::Occupied),
    }
}

/// Refuse a destination that exists and is not an empty folder, or is a
/// symbolic link: a clone goes only into a new or an empty folder.
fn check_destination(dest: &Path) -> Result<(), StatusText> {
    match destination_occupancy(dest)? {
        Occupancy::Empty => Ok(()),
        Occupancy::Occupied => Err(crate::status_text![
            n(dest.display()),
            " already exists and isn't an empty folder. An earlier clone may have been \
             interrupted there; dux leaves that folder for you to remove. Remove it, or pick \
             another destination."
        ]),
    }
}

/// The sentence for a clone git did not finish.
fn clone_failure(address: &str, dest: &Path, error: &crate::git::CloneError) -> StatusText {
    match error {
        crate::git::CloneError::Failed(said) => crate::status_text![
            "Couldn't clone ",
            n(address),
            format!(
                ": {}. Nothing was added to the workspace.",
                said.trim_end_matches('.')
            )
        ],
        crate::git::CloneError::Stalled(after) => crate::status_text![
            "Stopped cloning ",
            n(address),
            format!(
                " after git printed nothing for {} seconds. Nothing was added to the \
                 workspace. ",
                after.as_secs()
            ),
            n(dest.display()),
            " may hold part of the clone; dux leaves it for you to remove. If the remote is \
             just slow, raise [git] clone_stall_seconds and try again."
        ],
        crate::git::CloneError::Stopped => crate::status_text![
            "Stopped cloning ",
            n(address),
            " because dux is quitting. ",
            n(dest.display()),
            " may hold part of the clone; dux leaves it for you to remove."
        ],
    }
}

/// The address with the `user[:password]@` of every URL-form address in it
/// removed, so a token pasted into an address never reaches a message, a toast
/// or the log. Takes any text, git's own messages included, since those quote
/// the address too. An scp-style `user@host:path` keeps its user, which is not
/// a secret.
///
/// Written out rather than parsed with `url::Url`, which would re-spell the
/// address it hands back and cannot find an address inside a sentence: the
/// authority after `://` ends at the first `/`, `?` or `#` (or, inside a
/// sentence, a space), and whatever it holds up to its last `@` is the user
/// and password, whatever characters those hold: a quote or an apostrophe in a
/// token is still the token.
pub fn redact_remote_userinfo(text: &str) -> String {
    let mut redacted = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(scheme_end) = rest.find("://") {
        let authority_start = scheme_end + "://".len();
        redacted.push_str(&rest[..authority_start]);
        let after = &rest[authority_start..];
        let authority_len = after
            .find(|c: char| matches!(c, '/' | '?' | '#') || c.is_whitespace())
            .unwrap_or(after.len());
        let authority = &after[..authority_len];
        match authority.rfind('@') {
            Some(at) => redacted.push_str(&authority[at + 1..]),
            None => redacted.push_str(authority),
        }
        rest = &after[authority_len..];
    }
    redacted.push_str(rest);
    redacted
}

/// The folder name `git clone` picks for `address`, or `None` when the address
/// names none: trailing slashes and a trailing `.git` come off, and the name is
/// what follows the last `/`, or the `:` of an scp-style `host:path`.
pub fn clone_dir_name(address: &str) -> Option<String> {
    let trimmed = address.trim().trim_end_matches('/');
    let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    let trimmed = trimmed.trim_end_matches('/');
    let name = trimmed.rsplit(['/', ':']).next().unwrap_or(trimmed);
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct DirNameFixture {
        cases: Vec<DirNameCase>,
    }

    #[derive(serde::Deserialize)]
    struct DirNameCase {
        what: String,
        address: String,
        name: Option<String>,
    }

    #[test]
    fn the_shared_clone_folder_name_fixture_holds() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/clone_dir_name_cross_language.json");
        let fixture: DirNameFixture =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        for case in fixture.cases {
            assert_eq!(
                clone_dir_name(&case.address),
                case.name,
                "{}: {:?}",
                case.what,
                case.address
            );
        }
    }

    /// dux creates a missing destination itself, so nothing can appear there
    /// between the check and git, and after a clone that did not finish it
    /// removes only that folder, and only while it is empty.
    #[test]
    fn a_clone_claims_a_missing_destination_and_gives_back_only_its_own_empty_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();

        let made = root.join("made");
        let claim = claim_destination(&made).expect("a missing destination is claimed");
        assert!(made.is_dir(), "dux did not create the destination");
        claim.release_unused();
        assert!(!made.exists(), "the empty folder dux made was left behind");

        let partial = root.join("partial");
        let claim = claim_destination(&partial).unwrap();
        std::fs::write(partial.join("half-cloned"), "x").unwrap();
        claim.release_unused();
        assert!(
            partial.join("half-cloned").exists(),
            "a folder with content went"
        );

        let users = root.join("users");
        std::fs::create_dir(&users).unwrap();
        let claim = claim_destination(&users).expect("an empty folder is used as is");
        claim.release_unused();
        assert!(users.is_dir(), "a folder dux did not make was removed");
    }

    #[test]
    fn a_folder_is_judged_by_what_it_holds_against_the_repository_asked_for() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let working_copy = |name: &str, origin: &str, branch: &str| {
            let dir = root.join(name);
            std::fs::create_dir(&dir).unwrap();
            for args in [
                vec!["init", "-q", "-b", branch],
                vec!["remote", "add", "origin", origin],
                vec![
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "commit",
                    "-q",
                    "--allow-empty",
                    "-m",
                    "init",
                ],
            ] {
                let out = crate::git::test_support::git_command()
                    .arg("-C")
                    .arg(&dir)
                    .args(&args)
                    .output()
                    .unwrap();
                assert!(out.status.success(), "git {args:?}: {out:?}");
            }
            dir
        };
        let ours = working_copy("ours", "https://github.com/Acme/Widget.git", "feature");
        let other = working_copy("other", "https://github.com/acme/other.git", "main");
        let full = root.join("full");
        std::fs::create_dir(&full).unwrap();
        std::fs::write(full.join("notes"), "x").unwrap();
        let empty = root.join("empty");
        std::fs::create_dir(&empty).unwrap();
        let linked = root.join("linked");
        std::os::unix::fs::symlink(&empty, &linked).unwrap();
        let target = PullRequestCloneTarget {
            host: "github.com".to_string(),
            owner_repo: "acme/widget".to_string(),
            number: 3,
        };
        let policy = crate::gh::GithubHostPolicy::Hosts(["github.com".to_string()].into());
        let project = ours.to_string_lossy().into_owned();
        let inspect = |path: &Path, projects: &[&str]| {
            inspect_pr_destination(&path.to_string_lossy(), &target, &policy, None, projects)
        };

        assert_eq!(inspect(&root.join("missing"), &[]), Ok(PrDestination::Free));
        assert_eq!(inspect(&empty, &[]), Ok(PrDestination::Free));
        // A checkout is a checkout whichever branch it has out.
        assert_eq!(inspect(&ours, &[]), Ok(PrDestination::Checkout));
        for (what, path, words) in [
            (
                "a working copy of another repository",
                &other,
                "already holds something that isn't a checkout of github.com/acme/widget. \
                 Pick another folder.",
            ),
            (
                "a folder with files",
                &full,
                "already holds something that isn't a checkout of github.com/acme/widget. \
                 Pick another folder.",
            ),
            (
                "a symbolic link",
                &linked,
                "is a symbolic link. dux clones only into a new or an empty folder.",
            ),
        ] {
            let refused = inspect(path, &[]).expect_err(what).to_string();
            assert!(refused.contains(words), "{what}: {refused}");
            assert!(
                refused.starts_with(&path.display().to_string()),
                "{what}: {refused}"
            );
        }
        // A checkout is told whether a name typed like a review copy of the
        // pull request's branch needs the copy: it does until a local branch of
        // that name exists, and a name that is not spelled like a copy does not.
        let ask = |name: &str| {
            check_pr_destination(
                &ours.to_string_lossy(),
                &target,
                &policy,
                &[],
                Some(FolderReviewCopyQuery {
                    worktrees_root: &root.join("worktrees"),
                    number: 3,
                    head_branch: "fix-docs",
                    head_commit: None,
                    agent_name: Some(name),
                }),
            )
            .expect("a checkout")
            .review_copy
        };
        let Some(ReviewCopyOutlook::Needed {
            reason,
            copy_branch,
            ..
        }) = ask("fix-docs-review")
        else {
            panic!("a typed review copy name needs the copy");
        };
        assert_eq!(
            (reason.as_str(), copy_branch.as_str()),
            ("typed", "fix-docs-review")
        );
        assert_eq!(ask("my-own-name"), Some(ReviewCopyOutlook::NotNeeded));
        let made = crate::git::test_support::git_command()
            .arg("-C")
            .arg(&ours)
            .args(["branch", "fix-docs-review"])
            .output()
            .unwrap();
        assert!(made.status.success());
        assert_eq!(ask("fix-docs-review"), Some(ReviewCopyOutlook::NotNeeded));

        let refused = inspect(&ours, &[project.as_str()])
            .expect_err("a folder that is already a project")
            .to_string();
        assert!(
            refused.ends_with(
                "is already a project in the workspace. Pick another destination for the clone."
            ),
            "{refused}"
        );
    }

    #[test]
    fn a_leading_tilde_in_the_destination_is_the_home_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(tmp.path()).unwrap();
        assert_eq!(
            destination_identity("~/projects", Some(&home)),
            Ok(home.join("projects"))
        );
        let refused = destination_identity("~other/projects", Some(&home)).unwrap_err();
        assert!(
            refused.to_string().contains("isn't an absolute path"),
            "{refused}"
        );
    }

    #[test]
    fn a_user_and_token_are_hidden_wherever_an_address_appears() {
        for (what, text, hidden) in [
            (
                "a user and token",
                "https://me:ghp_secret@github.com/owner/repo.git",
                "https://github.com/owner/repo.git",
            ),
            (
                "a user alone",
                "https://me@example.com/repo",
                "https://example.com/repo",
            ),
            (
                "an @ in the password",
                "https://me:p@ss@example.com:8443/repo",
                "https://example.com:8443/repo",
            ),
            (
                "inside a git message, quoted",
                "fatal: unable to access 'https://me:tok@example.com/x/': 403",
                "fatal: unable to access 'https://example.com/x/': 403",
            ),
            (
                "two addresses in one line",
                "ssh://a:b@h1/x and https://c:d@h2/y",
                "ssh://h1/x and https://h2/y",
            ),
            (
                "an @ in the path is not a user",
                "https://example.com/@scope/repo",
                "https://example.com/@scope/repo",
            ),
            (
                "an scp-style user is not a secret",
                "git@github.com:owner/repo.git",
                "git@github.com:owner/repo.git",
            ),
            ("a local path", "/srv/git/repo.git", "/srv/git/repo.git"),
            (
                "an apostrophe in the token",
                "https://alice:tok'en@example.invalid/repo.git",
                "https://example.invalid/repo.git",
            ),
            (
                "a quote and a colon in the password",
                "https://alice:p\"w:d@example.invalid/repo.git",
                "https://example.invalid/repo.git",
            ),
            (
                "an apostrophe in a token git quotes",
                "fatal: unable to access 'https://alice:tok'en@example.invalid/x/': 403",
                "fatal: unable to access 'https://example.invalid/x/': 403",
            ),
            (
                "a quoted address with no path",
                "fatal: 'https://alice:tok@example.invalid' failed",
                "fatal: 'https://example.invalid' failed",
            ),
        ] {
            assert_eq!(redact_remote_userinfo(text), hidden, "{what}");
        }
    }
}
