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
pub(crate) struct CloneRequest {
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
    /// Whether the user agreed to a review copy.
    #[serde(default)]
    pub agreed: bool,
    /// The word the warning's reason began with, or `None` for an agreement that covers any copy.
    #[serde(default)]
    pub reason: Option<String>,
    /// The branch the warning said the copy would be made on. An agreement that
    /// names none covers the reason under any name.
    #[serde(default)]
    pub branch: Option<String>,
    /// Whether the agreement came from a command-line flag rather than a dialog.
    #[serde(default)]
    pub by_flag: bool,
}

impl UpfrontReviewCopy {
    /// The agreement as the agent create weighs it.
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
    /// Clone the repository into the folder.
    #[default]
    Clone,
    /// Add the checkout already in the folder as the project.
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
    /// The clone's operation, which its outcome resolves.
    pub op: HandlerStatusOp<CloneOutcome>,
    /// Whether a browser asked for the clone.
    pub from_web: bool,
    /// The repository key a pull request clone holds, released once the
    /// project is registered or the clone ends without one; `None` for an
    /// ordinary clone.
    pub repository_key: Option<String>,
    /// Whether the folder is cloned into or added as it is.
    pub mode: CloneMode,
    /// The folder the operation clones into or adds, to word a refusal by what
    /// the holder of that folder is doing.
    pub destination: PathBuf,
    /// The agreement to a review copy the request came with, handed to the
    /// agent's create after the project exists.
    pub review_copy: crate::worker::ReviewCopyConsent,
}

impl std::fmt::Debug for PendingClone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingClone")
            .field("from_web", &self.from_web)
            .field("repository_key", &self.repository_key)
            .field("mode", &self.mode)
            .field("destination", &self.destination)
            .field("review_copy", &self.review_copy)
            .finish_non_exhaustive()
    }
}

/// The engine's clones: the operations waiting for their worker, and the git
/// processes running, which quitting dux stops.
#[derive(Default, Debug)]
pub struct Clones {
    /// The clones waiting for their worker, by operation id.
    pub pending: HashMap<String, PendingClone>,
    /// The git processes running for those clones.
    pub processes: CloneProcesses,
    /// Extra variables for every clone's git, empty outside tests.
    pub(crate) git_env: Vec<(String, String)>,
}

/// A request that passed the checks the engine thread runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PreparedClone {
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
pub(crate) fn prepare_clone(
    request: &CloneRequest,
    home: Option<&Path>,
    projects: &[&str],
) -> Result<PreparedClone, StatusText> {
    let url = trim_typed_address(&request.url);
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

/// The address to clone for what a person typed into the clone field, or
/// `None` when the text goes to git exactly as typed: the address of the
/// repository [`typed_clone_repository`] reads in it.
pub(crate) fn expand_typed_address(
    text: &str,
    protocols: &std::collections::BTreeMap<String, crate::gh::GitProtocol>,
    hosts: &crate::gh::GithubHosts,
) -> Option<String> {
    typed_clone_repository(text, hosts).map(|repository| repository.address(protocols))
}

/// What the browser's `String.prototype.trim` trims, a byte-order mark among
/// it, so this side and the browser's folder suggestion agree on where typed
/// text starts and ends, and the address handed to git carries none of it.
pub(crate) fn trim_typed_address(text: &str) -> &str {
    text.trim_matches(|c: char| {
        matches!(
            c,
            '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
                ..='\u{200a}'
                    | '\u{2028}'
                    | '\u{2029}'
                    | '\u{202f}'
                    | '\u{205f}'
                    | '\u{3000}'
                    | '\u{feff}'
        )
    })
}

/// A repository the clone field's text names in a spelling dux rewrites.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TypedCloneRepository {
    /// Lowercase, with a web port when the address gave one.
    host: String,
    owner: String,
    /// The repository's name with any `.git` off, which is also the folder the
    /// clone lands in.
    repo: String,
    /// `http` or `https` for a pasted web address, which keeps the scheme it
    /// was written with; `None` for a short form, cloned the way `gh` would.
    web_scheme: Option<&'static str>,
}

impl TypedCloneRepository {
    /// The address to clone from: the pasted scheme for a web address, else
    /// the one `gh repo clone` would use, ssh or https by `protocols`.
    fn address(
        &self,
        protocols: &std::collections::BTreeMap<String, crate::gh::GitProtocol>,
    ) -> String {
        let owner_repo = format!("{}/{}", self.owner, self.repo);
        match self.web_scheme {
            Some(scheme) => format!("{scheme}://{}/{owner_repo}.git", self.host),
            None => crate::pr_reference::clone_address(&self.host, &owner_repo, protocols),
        }
    }
}

/// The repository typed text names, when it is one of the spellings the clone
/// rewrites rather than handing to git as typed:
///
/// * `owner/repo` is on `github.com`, and `host/owner/repo` (a host with a dot
///   in it and no port) is on that host.
/// * On a GitHub host (`hosts`), whatever follows `owner/repo` is a page from a
///   browser and comes off: a path (`/issues`, `/tree/main`, `/pull/7`), a
///   query (`?tab=readme-ov-file`) or a fragment (`#readme`). A web address
///   keeps the scheme and port it was written with.
/// * Elsewhere nothing is cut, a query or a fragment included, and the text
///   goes as typed: a longer path can be the repository itself (a GitLab
///   subgroup is `group/subgroup/repo`).
///
/// Everything else goes as typed: a web address that is exactly
/// `owner/repo`, scp-style, `ssh://`, `git://`, `file://`, a local path
/// (`/`, `.`, `~` in front, so `./owner/repo` stays a folder), anything with a
/// user, a password or a percent escape in it, and anything outside printable
/// ASCII. A pull request number is ignored: the repository is cloned.
///
/// The spelling is read twice, and both readings must name the same
/// repository. Once by [`typed_shape`], a deliberately small grammar that the
/// browser's folder suggestion (`cloneDirName.ts`) repeats line for line, and
/// once by the pull-request field's parser, [`crate::pr_reference`], which
/// knows every way a browser or git could read the text. Where the parser
/// reads it differently, or refuses it, nothing is rewritten.
pub(crate) fn typed_clone_repository(
    text: &str,
    hosts: &crate::gh::GithubHosts,
) -> Option<TypedCloneRepository> {
    let text = trim_typed_address(text);
    let shape = typed_shape(text, hosts)?;
    let reference = crate::pr_reference::parse_typed_reference(text).ok()?;
    let parsed_host = reference.host.as_deref().unwrap_or("github.com");
    let owner_repo = format!("{}/{}", shape.owner, shape.repo);
    (parsed_host == shape.host && reference.owner_repo.as_deref() == Some(owner_repo.as_str()))
        .then_some(shape)
}

/// The grammar [`typed_clone_repository`] reads a spelling by, kept small
/// enough for the browser's twin to repeat exactly: see its rules there.
fn typed_shape(text: &str, hosts: &crate::gh::GithubHosts) -> Option<TypedCloneRepository> {
    if text.is_empty()
        || !text
            .bytes()
            .all(|b| (0x21..=0x7e).contains(&b) && b != b'\\')
        || text.starts_with(['/', '.', '~'])
    {
        return None;
    }
    let lowered = text.to_ascii_lowercase();
    let web_scheme = ["https", "http"]
        .into_iter()
        .find(|scheme| lowered.starts_with(&format!("{scheme}://")));
    let rest = &text[web_scheme.map_or(0, |scheme| scheme.len() + "://".len())..];
    // The page is everything before the first `?` or `#`; a fragment is what
    // follows the first `#`.
    let (page, tail) = rest.split_at(rest.find(['?', '#']).unwrap_or(rest.len()));
    let fragment = rest.split_once('#').map(|(_, fragment)| fragment);
    let segments: Vec<&str> = page.split('/').filter(|part| !part.is_empty()).collect();
    if segments.iter().any(|part| part.bytes().all(|b| b == b'.')) {
        return None;
    }
    let (host, path) = if let Some(scheme) = web_scheme {
        if page.contains('%') || fragment.is_some_and(|fragment| fragment.contains('%')) {
            return None;
        }
        let host = typed_web_host(page.split('/').next().unwrap_or_default(), scheme)?;
        // Only a page on a GitHub host is rewritten; anything else, and
        // an address that is exactly the repository, goes as typed.
        if !hosts.contains(&host) || (segments.len() <= 3 && tail.is_empty()) {
            return None;
        }
        (host, &segments[1..])
    } else {
        let first = *segments.first()?;
        let looks_like_host = first.contains(['.', ':']) || first.eq_ignore_ascii_case("localhost");
        let (host, path) = if looks_like_host {
            (first.to_ascii_lowercase(), &segments[1..])
        } else {
            ("github.com".to_string(), &segments[..])
        };
        let github = !looks_like_host || hosts.contains(&host);
        // Only a GitHub host has a page to cut a query or a fragment off;
        // elsewhere the text goes as typed. A colon is a port or scp, and an
        // `@` a user: neither has a short form.
        if (!github && !tail.is_empty()) || page.contains([':', '@']) {
            return None;
        }
        if looks_like_host
            && !(host.contains('.') && host.split('.').all(|label| !label.is_empty()))
        {
            return None;
        }
        // `owner/repo/...` with no host is a relative path as much as a
        // page, and on another host the longer path can be the repository.
        if path.len() > 2 && (!looks_like_host || !github) {
            return None;
        }
        (host, path)
    };
    let [owner, repo, ..] = path else {
        return None;
    };
    let repo = strip_dot_git_suffix(repo);
    let is_name = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    };
    if !is_name(owner) || !is_name(repo) || repo.eq_ignore_ascii_case(".git") {
        return None;
    }
    // A fragment that is all digits is a pull request number, and one too big
    // for a number is refused by the parser, so it is refused here too.
    if path.len() == 2
        && fragment.is_some_and(|fragment| {
            !fragment.is_empty()
                && fragment.bytes().all(|b| b.is_ascii_digit())
                && fragment.parse::<u64>().is_err()
        })
    {
        return None;
    }
    Some(TypedCloneRepository {
        host,
        owner: (*owner).to_string(),
        repo: repo.to_string(),
        web_scheme,
    })
}

/// A web address's host as the `url` crate spells it, or `None` for an
/// authority the small grammar does not read: letters, digits, dots and
/// hyphens in non-empty labels, a last label that cannot read as an IPv4
/// number, and an optional port that is dropped when it is the scheme's own.
fn typed_web_host(authority: &str, scheme: &str) -> Option<String> {
    let (name, port) = match authority.split_once(':') {
        Some((name, port)) => (name, Some(port)),
        None => (authority, None),
    };
    let labels: Vec<&str> = name.split('.').collect();
    let last = labels
        .last()
        .copied()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
        || labels.iter().any(|label| label.is_empty())
        || last.bytes().all(|b| b.is_ascii_digit())
        || last.starts_with("0x")
    {
        return None;
    }
    let name = name.to_ascii_lowercase();
    let port = match port.filter(|port| !port.is_empty()) {
        None => None,
        Some(port) => {
            if !port.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let Ok(port) = port.parse::<u64>() else {
                return None;
            };
            if port > 65535 {
                return None;
            }
            let default = if scheme == "https" { 443 } else { 80 };
            (port != default).then_some(port)
        }
    };
    Some(match port {
        Some(port) => format!("{name}:{port}"),
        None => name,
    })
}

/// `value` with a trailing `.git` off, in any case, unless that would leave
/// nothing: the parser's own rule.
fn strip_dot_git_suffix(value: &str) -> &str {
    let len = value.len();
    if len > 4 && value.is_char_boundary(len - 4) && value[len - 4..].eq_ignore_ascii_case(".git") {
        &value[..len - 4]
    } else {
        value
    }
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
pub(crate) fn inspect_pr_destination(
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
#[derive(Debug)]
pub struct FolderReviewCopyQuery<'a> {
    /// The directory managed worktrees are created under.
    pub worktrees_root: &'a Path,
    /// The pull request's number.
    pub number: u64,
    /// The pull request's head branch.
    pub head_branch: &'a str,
    /// The pull request's head commit, when known.
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
    /// What the folder holds.
    pub state: PrDestination,
    /// The folder, canonicalized as dux will use it.
    pub path: PathBuf,
    /// Whether the agent will need a review copy there, when the check could say.
    pub review_copy: Option<ReviewCopyOutlook>,
}

/// [`inspect_pr_destination`] plus the folder's canonical spelling and the
/// review copy outlook for a checkout: the one answer both the browser's
/// folder check and the terminal UI's give. Runs git, so only on a worker. The
/// outlook is told from the folder as it is, never from a write, and a folder
/// that does not answer in time leaves the question open for the create to ask.
///
/// # Errors
///
/// Fails with the sentence to show when the folder cannot take the clone or the add: it holds
/// something else, or cannot be named or inspected.
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
pub(crate) fn destination_identity(raw: &str, home: Option<&Path>) -> Result<PathBuf, StatusText> {
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
    pub(crate) remote_default: Option<String>,
    /// The leading branch is the one the folder has checked out; otherwise
    /// nothing is checked out and it is `main`.
    pub checked_out: bool,
}

/// The pull request a clone's worker looked up, and the agent name it settled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClonedPullRequest {
    /// The pull request.
    pub target: PullRequestCloneTarget,
    /// The pull request's title.
    pub title: String,
    /// The pull request's state as `gh` reported it.
    pub state: String,
    /// The pull request's head branch.
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
    /// The folder the repository was cloned into or added from.
    pub path: PathBuf,
    /// The new project's display name.
    pub project_name: String,
    /// The new project's id.
    pub project_id: String,
    /// The agent's name.
    pub agent_name: String,
    /// Whether the folder was cloned into or added as it is.
    pub mode: CloneMode,
    /// The pull request the agent is for, when it is for one.
    pub pull_request: Option<PullRequestCloneTarget>,
}

impl ClonedFor {
    /// `reason` the agent is missing, after what the clone did.
    pub(crate) fn agent_missing(&self, reason: StatusText) -> StatusText {
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
    pub(crate) fn agent_missing_named(
        &self,
        attempted: Option<&str>,
        reason: StatusText,
    ) -> StatusText {
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
        question.pull_request.clone_from(&self.pull_request);
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
    pub(crate) fn startup_failed(&self, agent: &str, error: &str) -> StatusText {
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
    /// The folder the repository was cloned into or added from.
    pub path: PathBuf,
    /// The agent's name.
    pub agent_name: String,
    /// What the clone left on disk.
    pub cloned: ClonedRepository,
    /// The repository key the clone held, to release when the pending clone
    /// is gone.
    pub repository_key: Option<String>,
    /// Whether the folder was cloned into or added as it is.
    pub mode: CloneMode,
}

/// What a pull request clone's worker checks and looks up before it claims the
/// folder.
pub(crate) struct PullRequestJob {
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
pub(crate) struct CloneJob {
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
    pub(crate) git_env: Vec<(String, String)>,
}

/// Check the destination and the agent name with git, clone, and read what
/// was cloned; posts `WorkerEvent::RepositoryCloned` either way.
pub(crate) fn run_clone_job(job: CloneJob, worker_tx: &Sender<WorkerEvent>) {
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
    let (did_not, nothing_was) = match job.mode {
        CloneMode::Clone => ("clone", " Nothing was cloned."),
        CloneMode::AddExisting => ("add", " Nothing was added."),
    };
    refuse_project_already_checked_out(pull_request, did_not)?;
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
    let agent_name =
        pull_request_agent_name(&job.agent_name, target, &metadata, parent, nothing_was)?;
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
        let outlook = cloned_review_copy_outlook(job, pull_request, &agent_name, &metadata);
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

/// Refuse a clone or an add (`did_not` names which) for a pull request whose
/// repository a project has become a checkout of since the request was
/// accepted.
///
/// # Errors
///
/// Fails, naming the project, when one is such a checkout.
fn refuse_project_already_checked_out(
    pull_request: &PullRequestJob,
    did_not: &str,
) -> Result<(), CloneOutcome> {
    let target = &pull_request.target;
    let reference = crate::pr_reference::TypedReference {
        host: Some(target.host.clone()),
        owner_repo: Some(target.owner_repo.clone()),
        number: Some(target.number),
    };
    let repository = format!("{}/{}", target.host, target.owner_repo);
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
    Ok(())
}

/// The agent's name for the pull request `target`: the `typed` one, or the pull
/// request's head branch when none was typed.
///
/// # Errors
///
/// Fails when no name was typed and the head branch is not a usable agent name
/// in the repository at `parent`; `nothing_was` ends the sentence.
fn pull_request_agent_name(
    typed: &str,
    target: &PullRequestCloneTarget,
    metadata: &crate::gh::PullRequestMetadata,
    parent: &Path,
    nothing_was: &str,
) -> Result<String, CloneOutcome> {
    let typed = typed.trim();
    if !typed.is_empty() {
        return Ok(typed.to_string());
    }
    let branch = metadata.head_ref_name.as_str();
    if !crate::git::is_valid_agent_name(branch) || !crate::git::is_valid_branch_name(parent, branch)
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
    Ok(branch.to_string())
}

/// Whether an agent named `agent_name` on a fresh clone for the pull request
/// needs a review copy: a clone has the repository's default branch checked out
/// and no other local branch, so an agent on the pull request's own branch needs
/// one when that is the default branch, and so does one named like a copy of it.
fn cloned_review_copy_outlook(
    job: &CloneJob,
    pull_request: &PullRequestJob,
    agent_name: &str,
    metadata: &crate::gh::PullRequestMetadata,
) -> ReviewCopyOutlook {
    let target = &pull_request.target;
    if agent_name == metadata.head_ref_name {
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
            agent_name,
            target.number,
        )
        .unwrap_or(ReviewCopyOutlook::NotNeeded)
    }
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
    refuse_unaddable_checkout(dest, request, &pull_request, refuse)?;
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
    let (leading_branch, leading_branch_guess) =
        leading_branch_of_checkout(job, checked_out.as_deref(), refuse)?;
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

/// Refuse to add `dest` for `request` when it no longer holds a checkout of the
/// repository, or when the agent for `pull_request` will need a review copy
/// nobody agreed to. `refuse` ends each sentence.
///
/// # Errors
///
/// Fails, saying why, in either case.
fn refuse_unaddable_checkout(
    dest: &Path,
    request: &PullRequestJob,
    pull_request: &ClonedPullRequest,
    refuse: impl Fn(StatusText) -> CloneOutcome,
) -> Result<(), CloneOutcome> {
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
    Ok(())
}

/// The branch that leads the project for the checkout `job` adds, which has
/// `checked_out` out, and how it was guessed: the remote's default when the
/// folder has a branch of that name, otherwise the branch checked out.
/// `refuse` ends the refusal's sentence.
///
/// # Errors
///
/// Fails when HEAD is detached and the folder has no local branch to lead with.
fn leading_branch_of_checkout(
    job: &CloneJob,
    checked_out: Option<&str>,
    refuse: impl Fn(StatusText) -> CloneOutcome,
) -> Result<(String, Option<LeadingBranchGuess>), CloneOutcome> {
    let dest = &job.destination;
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
        checked_out,
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
    Ok((leading_branch, leading_branch_guess))
}

/// The destination a clone goes into, and whether dux made it.
#[derive(Debug)]
pub(crate) struct DestinationClaim {
    path: PathBuf,
    created: bool,
}

impl DestinationClaim {
    /// Give the destination back after a clone that did not finish: the
    /// folder dux made goes, but only while it is still empty (a partial clone
    /// stays for the user to look at), and a folder dux did not make is never
    /// touched. `remove_dir` removes only an empty directory and never follows
    /// a symbolic link.
    pub(crate) fn release_unused(self) {
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
pub(crate) fn claim_destination(dest: &Path) -> Result<DestinationClaim, StatusText> {
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
pub(crate) fn redact_remote_userinfo(text: &str) -> String {
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

/// The folder name a clone of `address` lands in, or `None` when the address
/// names none. For a spelling the clone rewrites (see
/// [`typed_clone_repository`], asked with the same `hosts`), it is the
/// repository's name, so a page pasted from a browser
/// (`https://github.com/o/r/issues`) suggests `r`. Everything else follows
/// git's own rule: trailing slashes and a trailing `.git` come off, and the
/// name is what follows the last `/`, or the `:` of an scp-style `host:path`.
pub fn clone_dir_name(address: &str, hosts: &crate::gh::GithubHosts) -> Option<String> {
    if let Some(repository) = typed_clone_repository(address, hosts) {
        return Some(repository.repo);
    }
    let trimmed = trim_typed_address(address).trim_end_matches('/');
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
        /// The hosts the case is read with; `github.com` alone when absent.
        #[serde(default = "crate::gh::GithubHosts::github_com_only")]
        github_hosts: crate::gh::GithubHosts,
        name: Option<String>,
    }

    fn protocols(
        entries: &[(&str, crate::gh::GitProtocol)],
    ) -> std::collections::BTreeMap<String, crate::gh::GitProtocol> {
        entries
            .iter()
            .map(|(host, protocol)| (host.to_string(), *protocol))
            .collect()
    }

    /// The GitHub hosts of the typed-address tests. A web port is part of the
    /// host, as the policy names it.
    fn test_github_hosts() -> crate::gh::GithubHosts {
        crate::gh::GithubHosts::Hosts {
            hosts: vec![
                "ghe.corp.example".to_string(),
                "ghe.corp.example:8443".to_string(),
            ],
        }
    }

    #[test]
    fn a_typed_address_expands_to_the_repository_it_names() {
        use crate::gh::GitProtocol::{Https, Ssh};
        let ssh = protocols(&[("github.com", Ssh), ("ghe.corp.example", Ssh)]);
        let https = protocols(&[("github.com", Https)]);
        let none = protocols(&[]);
        for (text, map, expected) in [
            // owner/repo is on github.com.
            ("o/r", &ssh, "git@github.com:o/r.git"),
            ("o/r.git", &https, "https://github.com/o/r.git"),
            ("o/r#123", &none, "https://github.com/o/r.git"),
            // host/owner/repo, on any host with a dot.
            ("github.com/o/r", &ssh, "git@github.com:o/r.git"),
            ("  GitHub.com/o/r/ ", &ssh, "git@github.com:o/r.git"),
            ("github.com/o/r", &https, "https://github.com/o/r.git"),
            ("github.com/o/r", &none, "https://github.com/o/r.git"),
            ("gitlab.example/o/r", &ssh, "https://gitlab.example/o/r.git"),
            ("v1.2/a/b", &none, "https://v1.2/a/b.git"),
            // A page path on a GitHub host comes off, keeping what was pasted.
            ("github.com/o/r/tree/main", &ssh, "git@github.com:o/r.git"),
            (
                "https://github.com/o/r/issues",
                &ssh,
                "https://github.com/o/r.git",
            ),
            (
                "https://github.com/o/r/pull/7",
                &ssh,
                "https://github.com/o/r.git",
            ),
            (
                "https://github.com/o/r/pull/7/files",
                &ssh,
                "https://github.com/o/r.git",
            ),
            (
                "https://github.com/o/r/security/dependabot",
                &ssh,
                "https://github.com/o/r.git",
            ),
            (
                "https://github.com/o/r/this/is/a/made/up/path",
                &ssh,
                "https://github.com/o/r.git",
            ),
            (
                "http://github.com/o/r/issues",
                &ssh,
                "http://github.com/o/r.git",
            ),
            (
                "https://ghe.corp.example:8443/o/r/tree/main",
                &ssh,
                "https://ghe.corp.example:8443/o/r.git",
            ),
            (
                "ghe.corp.example/o/r/issues",
                &ssh,
                "git@ghe.corp.example:o/r.git",
            ),
            // A query or a fragment on a GitHub host is page too.
            (
                "https://github.com/o/r?tab=readme-ov-file",
                &ssh,
                "https://github.com/o/r.git",
            ),
            (
                "https://github.com/o/r#readme",
                &ssh,
                "https://github.com/o/r.git",
            ),
            (
                "github.com/o/r/issues?q=is:open",
                &ssh,
                "git@github.com:o/r.git",
            ),
            // The scheme's own port is no port.
            (
                "https://github.com:443/o/r/issues",
                &ssh,
                "https://github.com/o/r.git",
            ),
            // What a browser trims, a byte-order mark among it.
            (
                "\u{feff}https://github.com/o/r/issues\u{3000}",
                &ssh,
                "https://github.com/o/r.git",
            ),
        ] {
            assert_eq!(
                expand_typed_address(text, map, &test_github_hosts()).as_deref(),
                Some(expected),
                "{text:?}"
            );
        }
    }

    #[test]
    fn a_typed_address_that_is_not_a_short_form_or_a_github_page_goes_as_typed() {
        let ssh = protocols(&[("github.com", crate::gh::GitProtocol::Ssh)]);
        for text in [
            // A full address with nothing after the repository.
            "https://github.com/o/r",
            "https://github.com/o/r.git",
            "git@github.com:o/r",
            "git@github.com:o/r.git",
            "ssh://git@github.com/o/r.git",
            "git://github.com/o/r.git",
            "file:///srv/o/r",
            // Longer paths elsewhere are the repository: a GitLab subgroup.
            "gitlab.com/group/subgroup/repo",
            "https://gitlab.com/group/subgroup/repo",
            "https://gitlab.com/group/subgroup/repo.git",
            // A user or password in a routed address is not rewritten.
            "https://user:token@github.com/o/r/issues",
            // Local paths.
            "/abs/path",
            "/srv/o/r",
            "/o/r",
            "./rel/path",
            "./owner/repo",
            "./github.com/o/r",
            "../rel/path",
            "~/x/y",
            "localhost/o/r",
            // Nothing that is a repository.
            "#123",
            "123",
            "",
            "a/b/c",
            "github.com:8443/o/r",
            "github.com:123/scriptaculous/pull/7",
            "github.com//r",
            "github.com/o/.git",
            "o/r:tags",
            // Pages and short forms that are not GitHub's.
            "gitlab.com/group/project/-/issues?q=a:b",
            "gitlab.example/o/r#readme",
            "gitlab.example/o/r?x=1",
            "https://gitlab.com/o/r/issues",
            "owner/repo/tree/main",
            // An escape a rebuilt address would have to carry.
            "https://github.com/o/r%23x/issues",
            "https://github.com/o/r/blob/main/a%20b",
            // Spellings a browser reads differently from how they look.
            "https:/github.com/o/r/issues",
            "https:///github.com/o/r/issues",
            "https://github.com/o/r/../x/issues",
            "https://github.com\\o\\r\\issues",
            "https://@github.com/o/r/issues",
            "https://1.2.3.4/o/r/issues",
            "o/r#99999999999999999999999",
        ] {
            assert_eq!(
                expand_typed_address(text, &ssh, &test_github_hosts()),
                None,
                "{text:?}"
            );
        }
    }

    #[test]
    fn a_page_on_a_host_is_read_by_route_only_under_hosts_that_name_it() {
        let ssh = protocols(&[]);
        let page = "https://github.example/o/r/issues";
        for (hosts, expected) in [
            (crate::gh::GithubHosts::github_com_only(), None),
            (
                crate::gh::GithubHosts::NameRule,
                Some("https://github.example/o/r.git"),
            ),
            (
                crate::gh::GithubHosts::Hosts {
                    hosts: vec!["github.example".to_string()],
                },
                Some("https://github.example/o/r.git"),
            ),
        ] {
            assert_eq!(
                expand_typed_address(page, &ssh, &hosts).as_deref(),
                expected,
                "{hosts:?}"
            );
        }
    }

    fn dir_name_fixture() -> DirNameFixture {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/clone_dir_name_cross_language.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// The browser's folder suggestion repeats the small grammar and not the
    /// parser, so the twins agree only while the parser reads every spelling
    /// the grammar accepts as the same repository. Held over every address the
    /// shared fixture carries and a list of spellings a browser or git reads
    /// differently from how they look.
    #[test]
    fn the_parser_reads_every_rewritten_spelling_as_the_small_grammar_does() {
        let mut texts: Vec<String> = dir_name_fixture()
            .cases
            .into_iter()
            .map(|case| case.address)
            .collect();
        for text in [
            "https://github.com:0443/o/r/issues",
            "https://github.com:/o/r/issues",
            "https://GitHub.com/O/R.GIT/issues",
            "https://github.com/o/r/pull/99999999999999999999999",
            "https://github.com/o/r#99999999999999999999999",
            "https://github.com/o/r/a/../../x/y",
            "github.com/o/r#1?x",
            "github.com/o/r?x#1",
            "github.com/o/r/x?y=http://z",
            "o/r?x=a:b",
            "o/r#a:b",
            "o_r/r-x.y",
            "github.com/o/r.git.git/issues",
        ] {
            texts.push(text.to_string());
        }
        for hosts in [
            crate::gh::GithubHosts::github_com_only(),
            crate::gh::GithubHosts::NameRule,
            test_github_hosts(),
        ] {
            for text in &texts {
                let trimmed = trim_typed_address(text);
                assert_eq!(
                    typed_shape(trimmed, &hosts),
                    typed_clone_repository(text, &hosts),
                    "{text:?} under {hosts:?}"
                );
            }
        }
    }

    #[test]
    fn the_shared_clone_folder_name_fixture_holds() {
        for case in dir_name_fixture().cases {
            assert_eq!(
                clone_dir_name(&case.address, &case.github_hosts),
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

    /// A working copy in `root`, named `name`, with one commit on `branch` and
    /// `origin` as its remote.
    fn working_copy(root: &Path, name: &str, origin: &str, branch: &str) -> PathBuf {
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
    }

    /// Make the local branch `name` in the working copy `dir`.
    fn make_branch(dir: &Path, name: &str) {
        let made = crate::git::test_support::git_command()
            .arg("-C")
            .arg(dir)
            .args(["branch", name])
            .output()
            .unwrap();
        assert!(made.status.success());
    }

    #[test]
    fn a_folder_is_judged_by_what_it_holds_against_the_repository_asked_for() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let ours = working_copy(
            &root,
            "ours",
            "https://github.com/Acme/Widget.git",
            "feature",
        );
        let other = working_copy(&root, "other", "https://github.com/acme/other.git", "main");
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
        make_branch(&ours, "fix-docs-review");
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
