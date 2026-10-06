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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::Duration;

use crate::engine::{Final, HandlerStatusOp};
use crate::git::CloneProcesses;
use crate::status_text::StatusText;
use crate::worker::WorkerEvent;

/// What a client asks for: clone `url` into `path` and start an agent named
/// `agent_name` there, or a random name when it is blank, as in the New agent
/// dialog. `random_name` is that dialog's checkbox, as the client sent it; a
/// blank name gets a random one either way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloneRequest {
    pub url: String,
    pub path: String,
    pub agent_name: Option<String>,
    pub random_name: bool,
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
}

/// The engine's clones: the operations waiting for their worker, and the git
/// processes running, which quitting dux stops.
#[derive(Default)]
pub struct Clones {
    pub pending: HashMap<String, PendingClone>,
    pub processes: CloneProcesses,
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
/// one. `projects` are the workspace's project
/// paths and `home` expands a leading `~`.
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
                ". Use only letters, digits, dashes, underscores and slashes; it must start \
                 with a letter or digit, must not contain \"//\", and must not end with \"/\"."
            ]);
        }
        typed.to_string()
    } else {
        crate::git::docker_style_name()
    };
    let destination = destination_identity(&request.path, home)?;
    // Both sides canonicalized, so a project saved under another spelling of
    // the folder (through a symbolic link, say) still matches.
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
    Ok(PreparedClone {
        url: url.to_string(),
        destination,
        agent_name,
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
    /// The branch the clone checked out: the remote's default.
    pub branch: String,
    /// The base new worktrees branch from.
    pub leading_branch: String,
    /// A branch named like the agent already exists in the clone.
    pub name_taken: bool,
}

/// What a clone's first agent create carries about the clone, so it refuses
/// an existing branch and its failure says what the clone already did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClonedFor {
    /// The address, with any user and token removed.
    pub address: String,
    pub path: PathBuf,
    pub project_name: String,
    pub agent_name: String,
}

impl ClonedFor {
    /// `reason` the agent is missing, after what the clone did.
    pub fn agent_missing(&self, reason: StatusText) -> StatusText {
        crate::status_text![
            "Cloned ",
            n(self.address),
            " into ",
            n(self.path.display()),
            " and added project ",
            q(self.project_name),
            ", but didn't create agent ",
            q(self.agent_name),
            ": ",
            reason
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
}

/// Everything the worker needs.
pub struct CloneJob {
    pub status_op_id: String,
    pub url: String,
    pub destination: PathBuf,
    pub agent_name: String,
    pub stall: Duration,
    pub processes: CloneProcesses,
}

/// Check the destination and the agent name with git, clone, and read what
/// was cloned; posts `WorkerEvent::RepositoryCloned` either way.
pub fn run_clone_job(job: CloneJob, worker_tx: &Sender<WorkerEvent>) {
    let result = clone_and_inspect(&job);
    let _ = worker_tx.send(WorkerEvent::RepositoryCloned {
        status_op_id: Some(job.status_op_id),
        path: job.destination,
        agent_name: job.agent_name,
        result,
    });
}

fn clone_and_inspect(job: &CloneJob) -> Result<ClonedRepository, CloneOutcome> {
    let address = redact_remote_userinfo(&job.url);
    let dest = &job.destination;
    let parent = dest.parent().unwrap_or(dest);
    if !crate::git::is_valid_branch_name(parent, &job.agent_name) {
        return Err(CloneOutcome::Refused(crate::status_text![
            "git won't take ",
            q(job.agent_name),
            " as a branch name, so it can't name the agent. Pick another name."
        ]));
    }
    let claim = claim_destination(dest).map_err(CloneOutcome::Refused)?;
    crate::logger::info(&format!("cloning {address} into {}", dest.display()));
    if let Err(error) = crate::git::clone_repository(&job.url, dest, job.stall, &job.processes) {
        crate::logger::error(&format!(
            "clone of {address} into {} failed: {error:?}",
            dest.display()
        ));
        claim.release_unused();
        return Err(CloneOutcome::Failed(clone_failure(&address, dest, &error)));
    }
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
        crate::project_browser::project_base_for_add(dest, Some(&branch), false).into_branch();
    let name_taken = matches!(
        crate::git::create_agent_branch_preflight(dest, &job.agent_name),
        crate::git::CreateAgentBranchPlan::ExistingBranch { .. }
    );
    Ok(ClonedRepository {
        address,
        branch,
        leading_branch,
        name_taken,
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

/// Refuse a destination that exists and is not an empty folder, or is a
/// symbolic link: a clone goes only into a new or an empty folder.
fn check_destination(dest: &Path) -> Result<(), StatusText> {
    let not_empty = || {
        crate::status_text![
            n(dest.display()),
            " already exists and isn't an empty folder. An earlier clone may have been \
             interrupted there; dux leaves that folder for you to remove. Remove it, or pick \
             another destination."
        ]
    };
    match std::fs::symlink_metadata(dest) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
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
            Ok(mut entries) => match entries.next() {
                None => Ok(()),
                Some(_) => Err(not_empty()),
            },
            Err(_) => Err(not_empty()),
        },
        Ok(_) => Err(not_empty()),
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
