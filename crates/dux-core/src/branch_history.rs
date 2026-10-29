//! Every branch an agent's worktree has been on, and what a delete does with
//! them.
//!
//! An agent records the branch it was born on and the branch it is on now; an
//! agent that went `a -> b -> c` also passed through `b`, and deleting the
//! agent would orphan it. The session database keeps the whole list
//! (`session_branch_history`): the branch-sync poller adds whatever the
//! worktree's own HEAD reflog shows it was on, and a dux rename renames the
//! entry in place.
//!
//! A delete decides ONCE, as an immutable list of named branches each pinned to
//! the commit the user was shown ([`BranchPlan`]), and the executor deletes
//! exactly those, each only while it still points there. Nothing on the way
//! re-derives a name from what the session says now.
//!
//! Who made a branch is answered from git, never assumed. The birth branch is
//! dux's only while the ref under its name is the very ref dux created (its
//! reflog creation entry is the one recorded at birth); any other branch can
//! at most be proved made AFTER the agent was born, which is weaker, because
//! another worktree may have made it.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::model::BranchProvenance;

/// The reflog subject `git checkout` and `git switch` write on HEAD for every
/// switch. Written untranslated, and stored as written.
const CHECKOUT_SUBJECT: &str = "checkout: moving from ";

/// The subject every way of making a branch writes as the first entry of that
/// branch's own reflog.
const CREATED_SUBJECT: &str = "branch: Created from ";

/// How many HEAD reflog entries one scan reads at most.
pub(crate) const REFLOG_SCAN_CAP: usize = 1000;

/// How many entries an incremental scan reads first, before falling back to
/// the cap when its cursor is not among them.
const REFLOG_SCAN_STEP: usize = 64;

/// One reflog entry as `log --walk-reflogs` reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReflogEntry {
    /// When the entry was written, in whole seconds.
    pub(crate) unix: i64,
    /// The entry's new value.
    pub(crate) oid: String,
    /// The entry's subject, `None` when it was not valid UTF-8.
    pub(crate) subject: Option<String>,
}

/// Where an incremental HEAD reflog scan stopped: the newest entry it read.
pub(crate) type ReflogCursor = ReflogEntry;

/// A `git log --walk-reflogs` over `reference`, newest first, at most `limit`
/// entries, immune to the user's configuration: signatures off, the output
/// encoding forced, colour and abbreviation inert under the explicit format,
/// records NUL-separated and fields US-separated so no subject can split one.
fn read_reflog(dir: &Path, reference: &str, limit: Option<usize>) -> Result<Vec<ReflogEntry>> {
    let mut command = Command::new("git");
    command
        .args([
            "-c",
            "log.showSignature=false",
            "-c",
            "i18n.logOutputEncoding=UTF-8",
            "-C",
        ])
        .arg(dir)
        .args([
            "log",
            "--walk-reflogs",
            "--encoding=UTF-8",
            "--no-color",
            "--no-abbrev-commit",
            "--date=unix",
            "--format=%gd%x1f%H%x1f%gs",
            "-z",
        ]);
    if let Some(limit) = limit {
        command.arg(format!("-n{limit}"));
    }
    let output = command
        .args([reference, "--"])
        .stdin(Stdio::null())
        .output()
        .map_err(|err| anyhow!("git could not be run: {err}"))?;
    if !output.status.success() {
        return Err(anyhow!(
            "git log --walk-reflogs failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(parse_reflog(&output.stdout))
}

/// [`read_reflog`]'s output read back. A record that does not have the three
/// fields, or whose selector carries no time, is skipped.
fn parse_reflog(stdout: &[u8]) -> Vec<ReflogEntry> {
    stdout
        .split(|byte| *byte == 0)
        .filter_map(|record| {
            let record = record.strip_prefix(b"\n").unwrap_or(record);
            let mut fields = record.splitn(3, |byte| *byte == 0x1f);
            let selector = std::str::from_utf8(fields.next()?).ok()?;
            let oid = std::str::from_utf8(fields.next()?).ok()?;
            let subject = fields.next()?;
            let unix = selector
                .rsplit_once("@{")?
                .1
                .strip_suffix('}')?
                .parse()
                .ok()?;
            Some(ReflogEntry {
                unix,
                oid: oid.to_string(),
                subject: std::str::from_utf8(subject)
                    .ok()
                    .map(|subject| subject.trim_end_matches('\n').to_string()),
            })
        })
        .collect()
}

/// The two names a `checkout: moving from A to B` subject names. Refnames hold
/// no space, so splitting on the LAST ` to ` gives both whole.
fn checkout_names(subject: &str) -> Option<(&str, &str)> {
    let rest = subject.strip_prefix(CHECKOUT_SUBJECT)?;
    let (from, to) = rest.rsplit_once(" to ")?;
    (!from.is_empty() && !to.is_empty() && !from.contains(' ') && !to.contains(' '))
        .then_some((from, to))
}

/// Every local branch, by name.
fn local_branches(repo: &Path) -> Result<std::collections::HashSet<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["for-each-ref", "--format=%(refname)", "refs/heads/"])
        .stdin(Stdio::null())
        .output()
        .map_err(|err| anyhow!("git could not be run: {err}"))?;
    if !output.status.success() {
        return Err(anyhow!(
            "git for-each-ref failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter_map(|line| std::str::from_utf8(line).ok())
        .filter_map(|line| line.strip_prefix("refs/heads/"))
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect())
}

/// The whole-second floor of an agent's creation, which is what reflog times
/// are compared with.
fn created_second(created_at: DateTime<Utc>) -> i64 {
    created_at.timestamp()
}

/// The branches a worktree's HEAD reflog shows it on since the agent was
/// created, which still exist as local branches, newest scan first.
///
/// `cursor` is where the previous scan stopped: only newer entries are read,
/// and when the cursor is not among the newest few, the scan reads up to the
/// cap. Answers the names and the new cursor (`None` for an empty reflog).
///
/// # Errors
///
/// Fails when git cannot read the reflog or the repository's branches.
pub(crate) fn scan_head_reflog(
    worktree: &Path,
    created_at: DateTime<Utc>,
    cursor: Option<&ReflogCursor>,
) -> Result<(Vec<String>, Option<ReflogCursor>)> {
    let mut entries = read_reflog(worktree, "HEAD", Some(REFLOG_SCAN_STEP))?;
    let found = cursor.and_then(|cursor| entries.iter().position(|entry| entry == cursor));
    let fresh = match (cursor, found) {
        (Some(_), Some(at)) => {
            entries.truncate(at);
            entries
        }
        _ if entries.len() < REFLOG_SCAN_STEP => entries,
        _ => {
            let mut all = read_reflog(worktree, "HEAD", Some(REFLOG_SCAN_CAP))?;
            if let Some(at) = cursor.and_then(|cursor| all.iter().position(|e| e == cursor)) {
                all.truncate(at);
            }
            all
        }
    };
    let new_cursor = fresh.first().cloned().or_else(|| cursor.cloned());
    if fresh.is_empty() {
        return Ok((Vec::new(), new_cursor));
    }
    let since = created_second(created_at);
    let mut candidates: Vec<&str> = Vec::new();
    for entry in fresh.iter().filter(|entry| entry.unix >= since) {
        if let Some((from, to)) = entry.subject.as_deref().and_then(checkout_names) {
            for name in [to, from] {
                if !candidates.contains(&name) {
                    candidates.push(name);
                }
            }
        }
    }
    if candidates.is_empty() {
        return Ok((Vec::new(), new_cursor));
    }
    let existing = local_branches(worktree)?;
    let names = candidates
        .into_iter()
        .filter(|name| existing.contains(*name))
        .map(str::to_string)
        .collect();
    Ok((names, new_cursor))
}

/// The ref dux made a birth branch as: its own reflog's creation entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchIdentity {
    /// When the creation entry was written, in whole seconds.
    pub unix: i64,
    /// The commit the branch was created at.
    pub oid: String,
}

/// The oldest entry of `refs/heads/<name>`'s reflog, `None` when it has none.
fn oldest_branch_entry(repo: &Path, name: &str) -> Result<Option<ReflogEntry>> {
    Ok(read_reflog(repo, &format!("refs/heads/{name}"), None)?.pop())
}

/// The creation entry of `refs/heads/<name>`, when its oldest reflog entry is
/// one: what [`BranchIdentity`] records and later compares.
///
/// # Errors
///
/// Fails when git cannot read the reflog.
pub(crate) fn branch_creation_entry(repo: &Path, name: &str) -> Result<Option<BranchIdentity>> {
    Ok(oldest_branch_entry(repo, name)?.and_then(|entry| {
        entry
            .subject
            .as_deref()
            .is_some_and(|subject| subject.starts_with(CREATED_SUBJECT))
            .then_some(BranchIdentity {
                unix: entry.unix,
                oid: entry.oid,
            })
    }))
}

/// When a branch was made, as git can prove it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BranchBirth {
    /// Its creation entry is from after the agent's creation second.
    After,
    /// Its creation entry is from before the agent's creation second.
    Before,
    /// Git cannot say: no reflog, an oldest entry that is not a creation (an
    /// expired reflog loses the creation entry first, so a later survivor is
    /// never read as the birth), the very same second, or git failing.
    Unknown,
}

/// When `name` was made, relative to an agent created at `created_at`.
pub(crate) fn branch_birth(repo: &Path, name: &str, created_at: DateTime<Utc>) -> BranchBirth {
    let since = created_second(created_at);
    match branch_creation_entry(repo, name) {
        Ok(Some(entry)) if entry.unix > since => BranchBirth::After,
        Ok(Some(entry)) if entry.unix < since => BranchBirth::Before,
        _ => BranchBirth::Unknown,
    }
}

/// One row of the session database's branch list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchHistoryEntry {
    /// The branch's name.
    pub name: String,
    /// Whether this is the branch the agent was born on, under its current name.
    pub born_here: bool,
    /// Which ref the name held when dux first recorded it (for the birth row,
    /// the ref dux made), `None` when its reflog could not say. A name can be
    /// reused by a different branch later; this is how dux tells them apart.
    pub identity: Option<BranchIdentity>,
    /// The commit the branch pointed at when dux last saw the agent on it,
    /// which a branch whose reflog git expired must still descend from.
    pub tip: Option<String>,
}

/// A branch dux saw the agent's worktree on: its name, which ref it is (its
/// reflog creation entry, when git still has it) and where it pointed then.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeenBranch {
    /// The branch's name.
    pub name: String,
    /// Its creation entry, `None` when its reflog cannot say.
    pub identity: Option<BranchIdentity>,
    /// The commit it pointed at, `None` when git could not say.
    pub tip: Option<String>,
}

/// Who made a branch the delete dialog offers, as far as dux can prove it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchOrigin {
    /// The branch dux created for this agent, and the ref under its name is
    /// still that one.
    Dux,
    /// The agent's birth branch, which existed before it.
    Attached,
    /// The agent's birth branch, which came with the worktree it adopted.
    Adopted,
    /// The agent's birth branch, with a provenance this build does not know.
    NotDux,
    /// git proves it was made after the agent was born. Not proof that this
    /// agent made it: another worktree may have.
    After,
    /// git proves it existed before the agent was born.
    Before,
    /// git cannot say when it was made.
    Unknown,
}

impl BranchOrigin {
    /// Whether dux can vouch the branch is its own: the default of every box
    /// that would delete it.
    pub fn is_duxs(self) -> bool {
        match self {
            Self::Dux => true,
            Self::Attached
            | Self::Adopted
            | Self::NotDux
            | Self::After
            | Self::Before
            | Self::Unknown => false,
        }
    }
}

/// How long before the agent's record a legacy birth branch's own creation
/// entry may be for dux to take it as the branch it made then, in seconds.
/// dux creates the branch first and writes the record once the worktree is
/// checked out, which on a big repository takes minutes; never after.
pub(crate) const LEGACY_BIRTH_WINDOW_SECS: i64 = 600;

/// Whether a creation entry written at `unix` is from the moment an agent
/// created at `created_at` was being made: at or before it, and no more than
/// [`LEGACY_BIRTH_WINDOW_SECS`] before.
pub(crate) fn within_legacy_window(unix: i64, created_at: DateTime<Utc>) -> bool {
    let created = created_second(created_at);
    unix <= created && created - unix <= LEGACY_BIRTH_WINDOW_SECS
}

/// Whether the ref under a recorded name is still the one recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SameBranch {
    /// Its creation entry is the recorded one; or its reflog no longer
    /// reaches back that far and its tip is the tip recorded when dux last
    /// saw it, or a descendant of it.
    Yes,
    /// Its creation entry is a different one: a different branch.
    Replaced,
    /// Its reflog cannot say, and its tip does not descend from the one
    /// recorded: dux cannot confirm it is the same branch.
    Unconfirmed,
}

/// Whether `refs/heads/<name>` is `oid` or a descendant of it.
pub(crate) fn descends_from(repo: &Path, name: &str, oid: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["merge-base", "--is-ancestor", oid])
        .arg(format!("refs/heads/{name}"))
        .stdin(Stdio::null())
        .output()
        .is_ok_and(|out| out.status.success())
}

/// What to make of a reflog that no longer reaches the creation entry
/// (expired, or never kept), which cannot say which branch it is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ExpiredReflog {
    /// The tip still descending from the recorded tip is enough.
    ByTip,
    /// Nothing short of the creation entry is enough.
    Unproven,
}

/// Whether the ref under `name` is still the branch recorded with `recorded`
/// and last seen at `tip`.
fn same_branch(
    repo: &Path,
    name: &str,
    recorded: &BranchIdentity,
    tip: Option<&str>,
    expired: ExpiredReflog,
) -> SameBranch {
    match branch_creation_entry(repo, name).ok().flatten() {
        Some(now) if now == *recorded => SameBranch::Yes,
        Some(_) => SameBranch::Replaced,
        None if expired == ExpiredReflog::ByTip && tip_still_holds(repo, name, tip) => {
            SameBranch::Yes
        }
        None => SameBranch::Unconfirmed,
    }
}

/// Whether the ref under `name` is the recorded tip or a descendant of it.
/// No tip recorded proves nothing: the commit the branch was created at is
/// in almost every branch, so it is never used in its place.
fn tip_still_holds(repo: &Path, name: &str, tip: Option<&str>) -> bool {
    tip.is_some_and(|tip| descends_from(repo, name, tip))
}

/// Whether the ref under a recorded name is still the branch the agent was
/// on. A row recorded with neither an identity nor a tip is never called
/// different: nothing says it is.
pub(crate) fn sameness(repo: &Path, entry: &BranchHistoryEntry) -> SameBranch {
    let tip = entry.tip.as_deref();
    match (&entry.identity, tip) {
        (Some(recorded), _) => same_branch(repo, &entry.name, recorded, tip, ExpiredReflog::ByTip),
        // No identity, but a tip: it must not have moved off it.
        (None, Some(_)) if tip_still_holds(repo, &entry.name, tip) => SameBranch::Yes,
        (None, Some(_)) => SameBranch::Unconfirmed,
        (None, None) => SameBranch::Yes,
    }
}

/// Whether the birth row's ref is still the one dux made.
///
/// With an identity recorded: its creation entry is the recorded one; or, when
/// git has expired its reflog, the agent's own worktree still has it checked
/// out and its tip descends from the one recorded. A legacy row (no identity)
/// counts while the agent's worktree has it checked out, a renamed checked-out
/// branch moving the worktree's HEAD with it, or while its own creation entry
/// is from the minutes the agent was being made.
fn birth_is_duxs(
    repo: &Path,
    entry: &BranchHistoryEntry,
    checked_out_here: Option<&str>,
    created_at: Option<DateTime<Utc>>,
) -> bool {
    let here = checked_out_here == Some(entry.name.as_str());
    match &entry.identity {
        Some(identity) => {
            let expired = if here {
                ExpiredReflog::ByTip
            } else {
                ExpiredReflog::Unproven
            };
            same_branch(repo, &entry.name, identity, entry.tip.as_deref(), expired)
                == SameBranch::Yes
        }
        None => {
            here || created_at
                .is_some_and(|at| legacy_birth_identity(repo, &entry.name, at).is_some())
        }
    }
}

/// The identity of a legacy birth branch dux can still prove it made: its
/// creation entry is from the minutes the agent was being made.
pub(crate) fn legacy_birth_identity(
    repo: &Path,
    name: &str,
    created_at: DateTime<Utc>,
) -> Option<BranchIdentity> {
    branch_creation_entry(repo, name)
        .ok()
        .flatten()
        .filter(|entry| within_legacy_window(entry.unix, created_at))
}

/// When a recorded branch was made, from its recorded identity, relative to an
/// agent created at `created_at`.
fn recorded_birth(identity: &BranchIdentity, created_at: DateTime<Utc>) -> BranchBirth {
    let since = created_second(created_at);
    match identity.unix.cmp(&since) {
        std::cmp::Ordering::Greater => BranchBirth::After,
        std::cmp::Ordering::Less => BranchBirth::Before,
        std::cmp::Ordering::Equal => BranchBirth::Unknown,
    }
}

/// The origin of one branch of the agent's list.
pub(crate) fn branch_origin(
    repo: &Path,
    entry: &BranchHistoryEntry,
    provenance: BranchProvenance,
    created_at: DateTime<Utc>,
    checked_out_here: Option<&str>,
) -> BranchOrigin {
    if entry.born_here {
        match provenance {
            BranchProvenance::CreatedByDux => {
                if birth_is_duxs(repo, entry, checked_out_here, Some(created_at)) {
                    return BranchOrigin::Dux;
                }
                return match branch_birth(repo, &entry.name, created_at) {
                    BranchBirth::After => BranchOrigin::After,
                    BranchBirth::Before => BranchOrigin::Before,
                    BranchBirth::Unknown => BranchOrigin::Unknown,
                };
            }
            BranchProvenance::AttachedExisting => return BranchOrigin::Attached,
            BranchProvenance::Adopted => return BranchOrigin::Adopted,
            BranchProvenance::Unknown => return BranchOrigin::NotDux,
        }
    }
    let birth = match &entry.identity {
        Some(identity) => recorded_birth(identity, created_at),
        None => branch_birth(repo, &entry.name, created_at),
    };
    match birth {
        BranchBirth::After => BranchOrigin::After,
        BranchBirth::Before => BranchOrigin::Before,
        BranchBirth::Unknown => BranchOrigin::Unknown,
    }
}

/// Which of the delete dialog's two boxes a branch belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchRole {
    /// The branch the worktree is on: the first box.
    Current,
    /// Any other branch the worktree has been on: the second box.
    Other,
}

/// A branch named together with the commit the user was shown it at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinnedBranch {
    /// The branch's name.
    pub name: String,
    /// The full object id it pointed at when the dialog read it.
    pub oid: String,
}

/// One delete-dialog box's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BoxAnswer {
    /// Nobody was asked: the project-removal cascade, the create rollback,
    /// factory reset, a command-line delete with no branch flag. Only the
    /// birth branch, when dux can prove it is its own, is deleted.
    Unasked,
    /// The box was on screen and not ticked.
    Keep,
    /// The box was ticked, over exactly these branches at these commits.
    Delete(Vec<PinnedBranch>),
    /// The dialog had no answer from git to pin anything to (still checking,
    /// or the check failed), so nothing it covers is deleted.
    NotChecked {
        /// Whether the check failed rather than not having finished.
        failed: bool,
    },
    /// The dialog did not show this box at all (it had no branch to offer in
    /// it), so nothing it would have covered is deleted.
    NotOffered,
    /// A caller that predates pins asked for the branch to go
    /// (`delete_branch=true` with no list): the current branch and the birth
    /// branch, each pinned when the removal runs, as such a caller always got.
    Legacy,
}

/// What a delete request says about the agent's branches, one answer per box.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchDeleteRequest {
    /// The first box: the branch the worktree is on.
    pub current: BoxAnswer,
    /// The second box: every other branch the worktree has been on.
    pub others: BoxAnswer,
    /// Every branch a box showed, by that box. A branch keeps the reason of
    /// the box that showed it even when the worktree moved since, so the
    /// branch the user saw in the second box and left unticked is reported
    /// that way after the worktree switched onto it.
    pub offered: Vec<(BranchRole, String)>,
}

impl BranchDeleteRequest {
    /// Nobody was asked about either box.
    pub fn unasked() -> Self {
        Self {
            current: BoxAnswer::Unasked,
            others: BoxAnswer::Unasked,
            offered: Vec::new(),
        }
    }

    /// The answer that decides `name`: the box that showed it, whichever role
    /// it has now, else the box of its current role.
    fn answer_for(&self, role: BranchRole, name: &str) -> &BoxAnswer {
        let shown = self
            .offered
            .iter()
            .find(|(_, offered)| offered == name)
            .map_or(role, |(shown, _)| *shown);
        match shown {
            BranchRole::Current => &self.current,
            BranchRole::Other => &self.others,
        }
    }

    /// The single "also delete the branch" answer of a caller that predates
    /// the second box and the pins.
    pub fn from_legacy(delete_branch: Option<bool>) -> Self {
        match delete_branch {
            None => Self::unasked(),
            Some(false) => Self {
                current: BoxAnswer::Keep,
                others: BoxAnswer::Keep,
                offered: Vec::new(),
            },
            Some(true) => Self {
                current: BoxAnswer::Legacy,
                others: BoxAnswer::Legacy,
                offered: Vec::new(),
            },
        }
    }
}

/// Why a branch the delete could have removed was kept.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum KeptBecause {
    /// The person unticked a box that started ticked.
    UserDeclined,
    /// Its box was not ticked.
    NotTicked,
    /// The birth branch predates the agent, by its provenance.
    NotDuxs {
        /// The provenance, stored spelling.
        provenance: String,
    },
    /// Nobody was asked, and it is not the branch dux made for the agent.
    NobodyAsked,
    /// The birth branch, but dux could not prove the ref under its name is
    /// still the one it made.
    NotProven,
    /// The dialog did not offer it: dux recorded it after the dialog read
    /// the list, the worktree moved onto it since, or the dialog had no box
    /// for it.
    NotOffered,
    /// A different branch has the name now than the one the agent was on.
    Replaced,
    /// git no longer keeps the record that would say which branch it is, and
    /// it does not descend from where the agent left it.
    Unconfirmed,
    /// A dialog pinned it, but dux has no record of this agent on it.
    Unrecorded,
    /// The dialog had not finished checking it, or could not.
    NotChecked {
        /// Whether the check failed.
        failed: bool,
    },
    /// The project's own branch.
    Protected,
    /// git could not say where it points.
    Unverifiable,
}

impl KeptBecause {
    /// Why the branch was kept, as a clause for a one-line report (the
    /// command line's `branch: x kept (…)`).
    pub fn reason(&self) -> String {
        match self {
            Self::UserDeclined => "you chose to keep it".to_string(),
            Self::NotTicked => "its box was not ticked".to_string(),
            Self::NotDuxs { provenance } => {
                format!(
                    "it {}",
                    BranchProvenance::from_str(provenance).kept_reason()
                )
            }
            Self::NobodyAsked => "nobody was asked whether to delete it".to_string(),
            Self::NotProven => {
                "dux could not prove it is still the branch it made for this agent".to_string()
            }
            Self::NotOffered => "the dialog did not offer it".to_string(),
            Self::Replaced => "a different branch now has that name".to_string(),
            Self::Unconfirmed => "dux cannot confirm it is the same branch".to_string(),
            Self::Unrecorded => "dux has no record of this agent on it".to_string(),
            Self::NotChecked { failed: false } => {
                "the dialog had not finished checking it".to_string()
            }
            Self::NotChecked { failed: true } => "the dialog could not check it".to_string(),
            Self::Protected => "it is the project's own branch".to_string(),
            Self::Unverifiable => "git could not say where it points".to_string(),
        }
    }
}

/// A branch the removal deletes, pinned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedDelete {
    /// Which box it came from.
    pub role: BranchRole,
    /// Whether it is the agent's birth branch, which is what a left-over
    /// folder's record carries the agent's provenance for.
    #[serde(default)]
    pub born_here: bool,
    /// The branch and its commit.
    pub branch: PinnedBranch,
}

/// A branch whose fate is decided at the removal, by git, because nobody
/// pinned it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveAtRemoval {
    /// Which box it belongs to.
    pub role: BranchRole,
    /// Whether it is the agent's birth branch.
    #[serde(default)]
    pub born_here: bool,
    /// The branch.
    pub name: String,
    /// The rule.
    pub rule: ResolveRule,
}

/// How a [`ResolveAtRemoval`] is decided.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ResolveRule {
    /// The birth branch: deleted only when it is dux's (its provenance says
    /// dux created it and the ref under its name is still the one it made).
    DuxsBirth {
        /// The agent's provenance, stored spelling.
        provenance: String,
        /// The recorded identity, `None` for a legacy row.
        identity: Option<BranchIdentity>,
        /// When the agent was created, which a legacy row's own creation
        /// entry is compared with.
        #[serde(default)]
        agent_created_at: Option<DateTime<Utc>>,
        /// Where the branch pointed when dux last saw the agent on it.
        #[serde(default)]
        tip: Option<String>,
    },
    /// A caller that predates pins asked for it: deleted at the commit it
    /// points at when the removal runs.
    Legacy,
}

/// A branch the plan keeps, with why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedKeep {
    /// Which box it belongs to.
    pub role: BranchRole,
    /// The branch.
    pub name: String,
    /// Why.
    pub because: KeptBecause,
    /// The identity the agent's list recorded for it, so the removal can say
    /// when a different branch has the name by then.
    #[serde(default)]
    pub identity: Option<BranchIdentity>,
    /// Where it pointed when dux last saw the agent on it.
    #[serde(default)]
    pub tip: Option<String>,
}

/// Everything a removal does with the agent's branches, decided once when the
/// delete is admitted and carried unchanged to the worker that runs it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchPlan {
    /// Branches deleted, each only while it still points at its commit.
    pub delete: Vec<PlannedDelete>,
    /// Branches git decides about when the removal runs.
    pub resolve: Vec<ResolveAtRemoval>,
    /// Branches kept, for the message. One that no longer exists when the
    /// removal runs is left out of it.
    pub keep: Vec<PlannedKeep>,
}

impl BranchPlan {
    /// The stored form, for the pending-removal row.
    ///
    /// # Errors
    ///
    /// Fails only if serialization fails, which a plan of strings cannot.
    pub(crate) fn encode(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }

    /// [`Self::encode`] read back.
    ///
    /// # Errors
    ///
    /// Fails when the text is not a plan.
    pub(crate) fn decode(text: &str) -> Result<Self> {
        Ok(serde_json::from_str(text)?)
    }
}

/// The state [`plan_branch_removal`] builds the plan in.
struct Planner<'a> {
    provenance: BranchProvenance,
    created_at: Option<DateTime<Utc>>,
    protected: &'a [String],
    /// Every branch either box pinned: a pin is matched by its name, whichever
    /// box it came from, so a branch that changed role mid-dialog is still the
    /// branch the user ticked.
    pins: Vec<PinnedBranch>,
    plan: BranchPlan,
}

impl Planner<'_> {
    fn keep(&mut self, role: BranchRole, entry: &BranchHistoryEntry, because: KeptBecause) {
        self.plan.keep.push(PlannedKeep {
            role,
            name: entry.name.clone(),
            because,
            identity: entry.identity.clone(),
            tip: entry.tip.clone(),
        });
    }

    /// Why a branch on screen in a box nobody ticked was kept.
    fn unticked(&self, entry: &BranchHistoryEntry) -> KeptBecause {
        match self.provenance {
            BranchProvenance::CreatedByDux if entry.born_here => KeptBecause::UserDeclined,
            BranchProvenance::CreatedByDux => KeptBecause::NotTicked,
            _ if entry.born_here => KeptBecause::NotDuxs {
                provenance: self.provenance.as_str().to_string(),
            },
            _ => KeptBecause::NotTicked,
        }
    }

    fn resolve(&mut self, role: BranchRole, entry: &BranchHistoryEntry, how: ResolveRule) {
        self.plan.resolve.push(ResolveAtRemoval {
            role,
            born_here: entry.born_here,
            name: entry.name.clone(),
            rule: how,
        });
    }

    /// One branch under its box's answer.
    fn plan(&mut self, role: BranchRole, entry: &BranchHistoryEntry, answer: &BoxAnswer) {
        let name = entry.name.as_str();
        let pinned = self.pins.iter().position(|pin| pin.name == name);
        let pinned = pinned.map(|at| self.pins.remove(at));
        if self.protected.iter().any(|p| p == name) {
            if pinned.is_some() {
                self.keep(role, entry, KeptBecause::Protected);
            }
            return;
        }
        if let Some(branch) = pinned {
            self.plan.delete.push(PlannedDelete {
                role,
                born_here: entry.born_here,
                branch,
            });
            return;
        }
        match answer {
            // Ticked, but not over this branch: the dialog did not offer it.
            BoxAnswer::Delete(_) | BoxAnswer::NotOffered => {
                self.keep(role, entry, KeptBecause::NotOffered);
            }
            BoxAnswer::Keep => {
                let because = self.unticked(entry);
                self.keep(role, entry, because);
            }
            BoxAnswer::NotChecked { failed } => {
                self.keep(role, entry, KeptBecause::NotChecked { failed: *failed });
            }
            BoxAnswer::Unasked if entry.born_here => {
                let how = ResolveRule::DuxsBirth {
                    provenance: self.provenance.as_str().to_string(),
                    identity: entry.identity.clone(),
                    agent_created_at: self.created_at,
                    tip: entry.tip.clone(),
                };
                self.resolve(role, entry, how);
            }
            BoxAnswer::Legacy if entry.born_here || role == BranchRole::Current => {
                self.resolve(role, entry, ResolveRule::Legacy);
            }
            BoxAnswer::Unasked | BoxAnswer::Legacy => {
                self.keep(role, entry, KeptBecause::NobodyAsked);
            }
        }
    }
}

/// Every branch a box's answer pinned.
fn pins_of(answer: &BoxAnswer) -> &[PinnedBranch] {
    match answer {
        BoxAnswer::Delete(pins) => pins,
        BoxAnswer::Keep
        | BoxAnswer::NotChecked { .. }
        | BoxAnswer::NotOffered
        | BoxAnswer::Unasked
        | BoxAnswer::Legacy => &[],
    }
}

/// Whether the branch an agent was made from is protected from its delete:
/// always, unless it is the agent's own branch, the one it was born on or is
/// on now, or came with the worktree the agent adopted. Having once checked
/// it out does not make it the agent's.
pub fn source_branch_is_protected(managed: &crate::model::ManagedWorkspace) -> bool {
    let own = managed.source_branch == managed.initial_branch
        || managed.source_branch == managed.branch_name;
    let adopted = match managed.branch_provenance {
        BranchProvenance::Adopted => true,
        BranchProvenance::CreatedByDux
        | BranchProvenance::AttachedExisting
        | BranchProvenance::Unknown => false,
    };
    !(own || adopted)
}

/// What a removal nobody was asked about does with an agent's branches: only
/// the branch dux made for it goes, and only while git can prove it is still
/// that branch. `history` is the agent's recorded list; the branch it was made
/// from is kept unless it is one of its own.
pub fn unasked_plan(
    managed: &crate::model::ManagedWorkspace,
    created_at: DateTime<Utc>,
    history: &[BranchHistoryEntry],
) -> BranchPlan {
    let mut history = history.to_vec();
    if !history.iter().any(|entry| entry.born_here) && !managed.initial_branch.is_empty() {
        history.retain(|entry| entry.name != managed.initial_branch);
        history.insert(
            0,
            BranchHistoryEntry {
                name: managed.initial_branch.clone(),
                born_here: true,
                identity: None,
                tip: None,
            },
        );
    }
    let protected: Vec<String> = source_branch_is_protected(managed)
        .then(|| managed.source_branch.clone())
        .into_iter()
        .filter(|name| !name.is_empty())
        .collect();
    plan_branch_removal(
        &managed.branch_name,
        &history,
        managed.branch_provenance,
        Some(created_at),
        &BranchDeleteRequest::unasked(),
        &protected,
    )
}

/// Decide what a delete does with the agent's branches. Pure.
///
/// `current` is the branch the worktree is on; `history` is the agent's list
/// (the current branch may or may not be in it); `protected` are the project's
/// own branches, which nothing deletes; `created_at` is when the agent was
/// created, when known. A pin is matched by name across both boxes, and one naming a
/// branch the agent's list does not have is kept, with that reason.
pub fn plan_branch_removal(
    current: &str,
    history: &[BranchHistoryEntry],
    provenance: BranchProvenance,
    created_at: Option<DateTime<Utc>>,
    request: &BranchDeleteRequest,
    protected: &[String],
) -> BranchPlan {
    let mut pins: Vec<(BranchRole, PinnedBranch)> = Vec::new();
    for (role, answer) in [
        (BranchRole::Current, &request.current),
        (BranchRole::Other, &request.others),
    ] {
        pins.extend(pins_of(answer).iter().map(|pin| (role, pin.clone())));
    }
    let mut planner = Planner {
        provenance,
        created_at,
        protected,
        pins: pins.iter().map(|(_, pin)| pin.clone()).collect(),
        plan: BranchPlan::default(),
    };
    if !current.is_empty() {
        let entry = history
            .iter()
            .find(|entry| entry.name == current)
            .cloned()
            .unwrap_or_else(|| BranchHistoryEntry {
                name: current.to_string(),
                born_here: false,
                identity: None,
                tip: None,
            });
        let answer = request.answer_for(BranchRole::Current, &entry.name);
        planner.plan(BranchRole::Current, &entry, answer);
    }
    let mut seen: Vec<&str> = Vec::new();
    for entry in history {
        let name = entry.name.as_str();
        if name.is_empty() || name == current || seen.contains(&name) {
            continue;
        }
        seen.push(name);
        let answer = request.answer_for(BranchRole::Other, name);
        planner.plan(BranchRole::Other, entry, answer);
    }
    // Pins nothing above matched: a branch the agent's list never had.
    let unmatched = std::mem::take(&mut planner.pins);
    for pin in unmatched {
        let role = pins
            .iter()
            .find(|(_, p)| *p == pin)
            .map_or(BranchRole::Other, |(role, _)| *role);
        let entry = BranchHistoryEntry {
            name: pin.name,
            born_here: false,
            identity: None,
            tip: None,
        };
        planner.keep(role, &entry, KeptBecause::Unrecorded);
    }
    planner.plan
}

/// What became of one branch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BranchFate {
    /// Deleted.
    Deleted,
    /// It did not exist any more.
    AlreadyGone,
    /// It no longer pointed at the commit the user was shown, so it was kept.
    Moved,
    /// git refused, or another worktree has it checked out; it is still there.
    Refused {
        /// Why, in git's words or dux's.
        reason: String,
    },
    /// The plan kept it.
    Kept(KeptBecause),
}

/// One branch's outcome, for the message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchOutcome {
    /// Which box it belonged to.
    pub role: BranchRole,
    /// The branch.
    pub name: String,
    /// What happened.
    pub fate: BranchFate,
}

impl BranchOutcome {
    /// Whether the branch is still there for the user to deal with.
    pub fn still_there(&self) -> bool {
        match self.fate {
            BranchFate::Deleted | BranchFate::AlreadyGone => false,
            BranchFate::Moved | BranchFate::Refused { .. } | BranchFate::Kept(_) => true,
        }
    }
}

/// A plan with every branch pinned, as the worker resolved it right before
/// the worktree is removed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ResolvedPlan {
    pub(crate) delete: Vec<PlannedDelete>,
    pub(crate) keep: Vec<(BranchRole, String, KeptBecause)>,
}

/// Resolve what the plan left to git, against the repository as it is now:
/// pin each branch dux may delete, and drop kept branches that no longer
/// exist. Blocking: the removal worker's call, made while the worktree is
/// still there so its checked-out branch can be read.
pub(crate) fn resolve_plan(repo: &Path, worktree: &Path, plan: &BranchPlan) -> ResolvedPlan {
    let checked_out_here = crate::git::current_branch_opt(worktree).ok().flatten();
    let mut resolved = ResolvedPlan {
        delete: plan.delete.clone(),
        keep: Vec::new(),
    };
    for later in &plan.resolve {
        let duxs = match &later.rule {
            ResolveRule::Legacy => Ok(()),
            ResolveRule::DuxsBirth {
                provenance,
                identity,
                agent_created_at,
                tip,
            } => {
                let provenance = BranchProvenance::from_str(provenance);
                if provenance == BranchProvenance::CreatedByDux {
                    let entry = BranchHistoryEntry {
                        name: later.name.clone(),
                        born_here: true,
                        identity: identity.clone(),
                        tip: tip.clone(),
                    };
                    if birth_is_duxs(repo, &entry, checked_out_here.as_deref(), *agent_created_at) {
                        Ok(())
                    } else {
                        Err(KeptBecause::NotProven)
                    }
                } else {
                    Err(KeptBecause::NotDuxs {
                        provenance: provenance.as_str().to_string(),
                    })
                }
            }
        };
        match (duxs, crate::git::resolve_branch_oid(repo, &later.name)) {
            (_, Ok(None)) => {}
            (Err(because), _) => resolved
                .keep
                .push((later.role, later.name.clone(), because)),
            (Ok(()), Ok(Some(oid))) => resolved.delete.push(PlannedDelete {
                role: later.role,
                born_here: later.born_here,
                branch: PinnedBranch {
                    name: later.name.clone(),
                    oid,
                },
            }),
            (Ok(()), Err(_)) => {
                resolved
                    .keep
                    .push((later.role, later.name.clone(), KeptBecause::Unverifiable));
            }
        }
    }
    for kept in &plan.keep {
        if matches!(crate::git::resolve_branch_oid(repo, &kept.name), Ok(None)) {
            continue;
        }
        let entry = BranchHistoryEntry {
            name: kept.name.clone(),
            born_here: false,
            identity: kept.identity.clone(),
            tip: kept.tip.clone(),
        };
        // Whatever the box said, a different branch under the name is not the
        // one the agent was on, and the message says that instead. The
        // current branch is the one the agent's own worktree has checked
        // out, so there is nothing to ask about it.
        let sameness = if kept.role == BranchRole::Current {
            SameBranch::Yes
        } else {
            sameness(repo, &entry)
        };
        let because = match (&kept.because, sameness) {
            (KeptBecause::Protected | KeptBecause::Unrecorded, _) | (_, SameBranch::Yes) => {
                kept.because.clone()
            }
            (_, SameBranch::Replaced) => KeptBecause::Replaced,
            (_, SameBranch::Unconfirmed) => KeptBecause::Unconfirmed,
        };
        resolved.keep.push((kept.role, kept.name.clone(), because));
    }
    resolved
}

/// Delete what a resolved plan deletes, now that the worktree is gone, each
/// branch only while it still points at its pinned commit, and report every
/// branch's fate, the current one first.
pub(crate) fn execute_plan(repo: &Path, resolved: &ResolvedPlan) -> Vec<BranchOutcome> {
    let mut outcomes: Vec<BranchOutcome> = resolved
        .delete
        .iter()
        .map(|planned| {
            let kept = crate::git::KeptBranch {
                name: planned.branch.name.clone(),
                oid: Some(planned.branch.oid.clone()),
                provenance: None,
            };
            let fate = match crate::git::delete_kept_branch(repo, &kept) {
                Ok(crate::git::KeptBranchDeletion::Deleted) => BranchFate::Deleted,
                Ok(crate::git::KeptBranchDeletion::AlreadyGone) => BranchFate::AlreadyGone,
                Ok(crate::git::KeptBranchDeletion::Moved) => BranchFate::Moved,
                Ok(crate::git::KeptBranchDeletion::Refused { reason }) => {
                    BranchFate::Refused { reason }
                }
                Ok(crate::git::KeptBranchDeletion::Unverifiable) => {
                    BranchFate::Kept(KeptBecause::Unverifiable)
                }
                Err(err) => BranchFate::Refused {
                    reason: format!("{err:#}"),
                },
            };
            BranchOutcome {
                role: planned.role,
                name: planned.branch.name.clone(),
                fate,
            }
        })
        .collect();
    outcomes.extend(
        resolved
            .keep
            .iter()
            .map(|(role, name, because)| BranchOutcome {
                role: *role,
                name: name.clone(),
                fate: BranchFate::Kept(because.clone()),
            }),
    );
    outcomes.sort_by_key(|outcome| outcome.role != BranchRole::Current);
    outcomes
}

/// `[a]`, `[a] and [b]`, `[a], [b] and [c]`, each a name.
fn push_names(text: &mut crate::status_text::StatusText, names: &[&str]) {
    for (index, name) in names.iter().enumerate() {
        if index > 0 {
            text.push(if index + 1 == names.len() {
                " and "
            } else {
                ", "
            });
        }
        text.push_quoted(name);
    }
}

/// Which sentence a fate gets, so branches sharing one are named together.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Group {
    Deleted,
    AlreadyGone,
    Moved,
    Kept(KeptBecause),
}

/// The words after the names, singular and plural.
/// How many branches a sentence names.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Named {
    One,
    Several,
}

impl Named {
    fn of(count: usize) -> Self {
        if count == 1 { Self::One } else { Self::Several }
    }
}

fn predicate(group: &Group, named: Named) -> String {
    let pick =
        |single: &str, plural: &str| if named == Named::One { single } else { plural }.to_string();
    match group {
        Group::Deleted => pick(
            ", the other branch the agent was on.",
            ", the other branches the agent was on.",
        ),
        Group::AlreadyGone => pick(" was already gone.", " were already gone."),
        Group::Moved => pick(
            " was kept because it moved after you were shown it; what is on it now is not \
             what the dialog counted.",
            " were kept because they moved after you were shown them; what is on them now \
             is not what the dialog counted.",
        ),
        Group::Kept(because) => match because {
            KeptBecause::UserDeclined => pick(
                " was kept because you chose to keep it.",
                " were kept because you chose to keep them.",
            ),
            KeptBecause::NotTicked => pick(
                " was kept because its box was not ticked.",
                " were kept because their box was not ticked.",
            ),
            KeptBecause::NotDuxs { provenance } => {
                let reason = BranchProvenance::from_str(provenance).kept_reason();
                pick(
                    &format!(" {reason} and was kept."),
                    &format!(" {reason} and were kept."),
                )
            }
            KeptBecause::NobodyAsked => pick(
                " was kept: this agent was on it, but nobody was asked whether to delete it.",
                " were kept: this agent was on them, but nobody was asked whether to delete \
                 them.",
            ),
            KeptBecause::NotProven => pick(
                " was kept because dux could not prove it is still the branch it made for \
                 this agent.",
                " were kept because dux could not prove they are still the branches it made \
                 for this agent.",
            ),
            KeptBecause::NotOffered => pick(
                " was kept because the dialog did not offer it.",
                " were kept because the dialog did not offer them.",
            ),
            KeptBecause::Replaced => pick(
                " was kept because a different branch now has that name.",
                " were kept because different branches now have those names.",
            ),
            KeptBecause::Unconfirmed => pick(
                " was kept because dux cannot confirm it is the same branch the agent was on.",
                " were kept because dux cannot confirm they are the same branches the agent \
                 was on.",
            ),
            KeptBecause::Unrecorded => pick(
                " was kept because dux has no record of this agent on it.",
                " were kept because dux has no record of this agent on them.",
            ),
            KeptBecause::NotChecked { failed: false } => pick(
                " was kept because the dialog had not finished checking it.",
                " were kept because the dialog had not finished checking them.",
            ),
            KeptBecause::NotChecked { failed: true } => pick(
                " was kept because the dialog could not check it.",
                " were kept because the dialog could not check them.",
            ),
            KeptBecause::Protected => pick(
                " was kept because it is the project's own branch.",
                " were kept because they are the project's own branches.",
            ),
            KeptBecause::Unverifiable => pick(
                " was kept because git could not say where it points.",
                " were kept because git could not say where they point.",
            ),
        },
    }
}

/// The message's account of a removal's branches: one sentence per fate,
/// branches sharing a fate named together, the current branch on its own and
/// first ("Its branch …"), refusals in git's words, and one closing sentence
/// naming the command for every branch still there. Built once for both
/// surfaces. `None` when there is nothing to say.
///
/// The current branch's deletion itself is said by the caller's opening,
/// so a current branch that was deleted adds nothing here.
pub fn branch_outcomes_note(outcomes: &[BranchOutcome]) -> Option<crate::status_text::StatusText> {
    let mut sentences: Vec<crate::status_text::StatusText> = Vec::new();
    let mut leftover: Vec<&str> = Vec::new();
    let group_of = |fate: &BranchFate| match fate {
        BranchFate::Deleted => Some(Group::Deleted),
        BranchFate::AlreadyGone => Some(Group::AlreadyGone),
        BranchFate::Moved => Some(Group::Moved),
        BranchFate::Kept(because) => Some(Group::Kept(because.clone())),
        BranchFate::Refused { .. } => None,
    };
    for outcome in outcomes.iter().filter(|o| o.role == BranchRole::Current) {
        match &outcome.fate {
            BranchFate::Deleted | BranchFate::AlreadyGone | BranchFate::Refused { .. } => {}
            fate => {
                if let Some(group) = group_of(fate) {
                    let mut text = crate::status_text!["Its branch ", q(outcome.name)];
                    text.push(predicate(&group, Named::One));
                    sentences.push(text);
                    leftover.push(&outcome.name);
                }
            }
        }
    }
    let others: Vec<&BranchOutcome> = outcomes
        .iter()
        .filter(|o| o.role == BranchRole::Other)
        .collect();
    let mut groups: Vec<(Group, Vec<&str>)> = Vec::new();
    for outcome in &others {
        let Some(group) = group_of(&outcome.fate) else {
            continue;
        };
        match groups.iter_mut().find(|(g, _)| *g == group) {
            Some((_, names)) => names.push(&outcome.name),
            None => groups.push((group, vec![&outcome.name])),
        }
    }
    for (group, names) in &groups {
        let mut text = crate::status_text::StatusText::new();
        if *group == Group::Deleted {
            text.push("Also deleted ");
        }
        push_names(&mut text, names);
        text.push(predicate(group, Named::of(names.len())));
        sentences.push(text);
        if !matches!(group, Group::Deleted | Group::AlreadyGone) {
            leftover.extend(names.iter().copied());
        }
    }
    for outcome in &others {
        if let BranchFate::Refused { reason } = &outcome.fate {
            sentences.push(crate::git::branch_refusal_note(&outcome.name, reason));
        }
    }
    if !leftover.is_empty() {
        let mut tail = crate::status_text::StatusText::new();
        tail.push(if leftover.len() == 1 {
            "Delete it yourself with "
        } else {
            "Delete any of them yourself with "
        });
        for (index, name) in leftover.iter().enumerate() {
            if index > 0 {
                tail.push(if index + 1 == leftover.len() {
                    " or "
                } else {
                    ", "
                });
            }
            tail.push_name(crate::git::branch_delete_command(name));
        }
        tail.push(if leftover.len() == 1 {
            " if you no longer need it."
        } else {
            " if you no longer need them."
        });
        sentences.push(tail);
    }
    let mut sentences = sentences.into_iter();
    let mut note = sentences.next()?;
    for sentence in sentences {
        note.push(" ");
        note.push(sentence);
    }
    Some(note)
}

impl BranchPlan {
    /// The plan of a removal recorded before plans were: the agent's two known
    /// branches (the birth one, with no identity, and the current one) under
    /// that caller's single answer.
    pub(crate) fn legacy(
        managed: &crate::model::ManagedWorkspace,
        created_at: Option<DateTime<Utc>>,
        delete_branch: Option<bool>,
    ) -> Self {
        let mut history = Vec::new();
        if !managed.initial_branch.is_empty() {
            history.push(BranchHistoryEntry {
                name: managed.initial_branch.clone(),
                born_here: true,
                identity: None,
                tip: None,
            });
        }
        plan_branch_removal(
            &managed.branch_name,
            &history,
            managed.branch_provenance,
            created_at,
            &BranchDeleteRequest::from_legacy(delete_branch),
            &[],
        )
    }

    /// The single "also delete the branch" answer written for an older dux
    /// that finishes this removal: yes only when the plan pins exactly the
    /// current branch and the birth branch, which is what that dux deletes on
    /// a yes; anything else is no, the safe direction.
    pub(crate) fn old_rule_answer(&self, managed: &crate::model::ManagedWorkspace) -> bool {
        let mut old: Vec<&str> = vec![managed.branch_name.as_str()];
        if !managed.initial_branch.is_empty() {
            old.push(managed.initial_branch.as_str());
        }
        old.sort_unstable();
        old.dedup();
        let mut planned: Vec<&str> = self
            .delete
            .iter()
            .map(|planned| planned.branch.name.as_str())
            .collect();
        planned.sort_unstable();
        planned.dedup();
        !planned.is_empty() && planned == old
    }

    /// The plan with every branch name passed through `rename`, which follows
    /// the renames dux itself made while the removal waited. The commits stay
    /// pinned, so a renamed branch is still deleted only at its commit.
    pub(crate) fn renamed(&self, rename: impl Fn(&str) -> String) -> Self {
        let mut plan = self.clone();
        for planned in &mut plan.delete {
            planned.branch.name = rename(&planned.branch.name);
        }
        for later in &mut plan.resolve {
            later.name = rename(&later.name);
        }
        for kept in &mut plan.keep {
            kept.name = rename(&kept.name);
        }
        plan
    }
}

/// Remove the worktree and then do what the plan says with the agent's
/// branches. A worktree git let go of while its folder stayed keeps every
/// branch the plan would have deleted, each at its pinned commit, in the
/// [`crate::git::LeftoverWorktreeFolder`] the error carries.
///
/// # Errors
///
/// Fails when the worktree removal fails.
pub fn remove_worktree_with_plan(
    repo: &Path,
    worktree: &Path,
    plan: &BranchPlan,
    provenance: BranchProvenance,
) -> Result<Vec<BranchOutcome>> {
    let resolved = resolve_plan(repo, worktree, plan);
    if let Err(err) = crate::git::remove_worktree_keep_branch(repo, worktree) {
        return Err(match err.downcast::<crate::git::LeftoverWorktreeFolder>() {
            Ok(mut leftover) => {
                for planned in &resolved.delete {
                    if leftover
                        .kept_branches
                        .iter()
                        .any(|kept| kept.name == planned.branch.name)
                    {
                        continue;
                    }
                    leftover.kept_branches.push(crate::git::KeptBranch {
                        name: planned.branch.name.clone(),
                        oid: Some(planned.branch.oid.clone()),
                        provenance: Some(if planned.born_here {
                            provenance
                        } else {
                            BranchProvenance::Unknown
                        }),
                    });
                }
                anyhow::Error::new(leftover)
            }
            Err(err) => err,
        });
    }
    Ok(execute_plan(repo, &resolved))
}

/// One branch a delete dialog offers, pinned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfferedBranch {
    /// The branch.
    pub name: String,
    /// The full object id it points at, which is what a tick deletes.
    pub oid: String,
    /// Who made it, as far as dux can prove.
    pub origin: BranchOrigin,
}

/// Everything a delete dialog shows about the agent's branches, read from git
/// off the UI thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchOffer {
    /// The name of the branch the worktree was on when the offer was read,
    /// which decides whether a missing first box is the project's own
    /// branch or a failed check.
    pub current_name: String,
    /// The branch the worktree is on, `None` when git could not read it (the
    /// first box then deletes nothing).
    pub current: Option<OfferedBranch>,
    /// What ticking only the first box loses: the current branch's commits on
    /// no remote and no local branch that stays, or why git could not count.
    pub current_unpushed: Result<crate::git::UnpushedCommits, String>,
    /// Every other branch the worktree has been on that still exists, is
    /// still the branch the agent was on, and is not the project's own.
    pub others: Vec<OfferedBranch>,
    /// What ticking only the second box loses.
    pub others_unpushed: Result<crate::git::UnpushedCommits, String>,
    /// What ticking both boxes loses, which is more than the two counts
    /// apart whenever a commit is on branches in both boxes.
    pub both_unpushed: Result<crate::git::UnpushedCommits, String>,
    /// Branches the agent was on that are the project's own, which no box
    /// offers and the dialog names as kept.
    pub protected: Vec<String>,
    /// Branches the HEAD reflog showed that the agent's list did not have yet,
    /// each with its identity, for the engine to record.
    pub newly_seen: Vec<SeenBranch>,
}

impl BranchOffer {
    /// The first box's default: ticked only for a branch dux can prove is its
    /// own, or, once the agent switched off its birth branch, one git proves
    /// was made after the agent was born.
    pub fn current_default(&self) -> bool {
        self.current.as_ref().is_some_and(|current| {
            matches!(current.origin, BranchOrigin::Dux | BranchOrigin::After)
        })
    }

    /// What deleting the boxes ticked right now loses, and the branches it
    /// deletes; `None` when no box with a branch is ticked.
    pub fn lost(
        &self,
        current: crate::delete_agent_prose::BoxState,
        others: crate::delete_agent_prose::BoxState,
    ) -> Option<(Vec<&str>, &Result<crate::git::UnpushedCommits, String>)> {
        use crate::delete_agent_prose::BoxState;
        let others_ticked = others == BoxState::Ticked;
        let current = self
            .current
            .as_ref()
            .filter(|_| current == BoxState::Ticked);
        let others: Vec<&str> = if others_ticked {
            self.others
                .iter()
                .map(|other| other.name.as_str())
                .collect()
        } else {
            Vec::new()
        };
        let lost = match (current.is_some(), !others.is_empty()) {
            (false, false) => return None,
            (true, false) => &self.current_unpushed,
            (false, true) => &self.others_unpushed,
            (true, true) => &self.both_unpushed,
        };
        let mut names: Vec<&str> = current.iter().map(|c| c.name.as_str()).collect();
        names.extend(others);
        Some((names, lost))
    }

    /// The second box's default: ticked only when dux can prove every branch
    /// in it is its own.
    pub fn others_default(&self) -> bool {
        !self.others.is_empty() && self.others.iter().all(|other| other.origin.is_duxs())
    }
}

/// What deleting `branches` (at `oids`) loses: their commits on no remote and
/// no local branch outside them.
fn count_at(
    repo: &Path,
    oids: &[&str],
    branches: &[&str],
    local: &std::collections::HashSet<String>,
) -> Result<crate::git::UnpushedCommits, String> {
    let outside: Vec<&str> = local
        .iter()
        .map(String::as_str)
        .filter(|name| !branches.contains(name))
        .collect();
    crate::git::unpushed_commits_at(repo, oids, &outside).map_err(|err| {
        crate::logger::warn(&format!(
            "could not count the unpushed commits for the delete dialog: {err:#}"
        ));
        crate::git::unpushed_count_failure_reason(&err)
    })
}

/// Each name as dux sees it now: its identity and its tip, read once.
pub(crate) fn identities_of(repo: &Path, names: Vec<String>) -> Vec<SeenBranch> {
    names
        .into_iter()
        .map(|name| SeenBranch {
            identity: branch_creation_entry(repo, &name).ok().flatten(),
            tip: crate::git::resolve_branch_oid(repo, &name).ok().flatten(),
            name,
        })
        .collect()
}

/// Read what the delete dialog offers. Blocking: a worker's call.
///
/// The HEAD reflog is read once more from the agent's creation, which is what
/// catches a branch visited and left between two branch checks. A name the
/// agent was on that now holds a different branch is not offered. Each
/// offered branch is resolved to its commit ONCE, and each box's count is
/// taken from those commits.
pub fn delete_branch_offer(inputs: &crate::engine::BranchDeleteInputs) -> BranchOffer {
    let repo = Path::new(&inputs.project_path);
    let worktree = Path::new(&inputs.worktree_path);
    let mut history = inputs.history.clone();
    let fresh: Vec<String> = scan_head_reflog(worktree, inputs.created_at, None)
        .map(|(seen, _)| seen)
        .unwrap_or_default()
        .into_iter()
        .filter(|name| !history.iter().any(|entry| entry.name == *name))
        .collect();
    let newly_seen = identities_of(repo, fresh);
    history.extend(newly_seen.iter().map(|seen| BranchHistoryEntry {
        name: seen.name.clone(),
        born_here: false,
        identity: seen.identity.clone(),
        tip: seen.tip.clone(),
    }));
    let local = local_branches(repo).unwrap_or_default();
    let checked_out_here = crate::git::current_branch_opt(worktree).ok().flatten();
    let offer = |entry: &BranchHistoryEntry| -> Option<OfferedBranch> {
        let oid = crate::git::resolve_branch_oid(repo, &entry.name)
            .ok()
            .flatten()?;
        Some(OfferedBranch {
            name: entry.name.clone(),
            oid,
            origin: branch_origin(
                repo,
                entry,
                inputs.branch_provenance,
                inputs.created_at,
                checked_out_here.as_deref(),
            ),
        })
    };
    let is_protected = |name: &str| inputs.protected.iter().any(|p| p == name);
    let mut protected: Vec<String> = Vec::new();
    let mut current = None;
    if !inputs.branch_name.is_empty() {
        if is_protected(&inputs.branch_name) {
            protected.push(inputs.branch_name.clone());
        } else {
            let entry = history
                .iter()
                .find(|entry| entry.name == inputs.branch_name)
                .cloned()
                .unwrap_or(BranchHistoryEntry {
                    name: inputs.branch_name.clone(),
                    born_here: false,
                    identity: None,
                    tip: None,
                });
            current = offer(&entry);
        }
    }
    let mut others: Vec<OfferedBranch> = Vec::new();
    for entry in &history {
        let name = entry.name.as_str();
        if name == inputs.branch_name
            || others.iter().any(|other| other.name == name)
            || protected.iter().any(|p| p == name)
            || !local.contains(name)
        {
            continue;
        }
        if is_protected(name) {
            protected.push(name.to_string());
        } else if sameness(repo, entry) == SameBranch::Yes {
            others.extend(offer(entry));
        }
    }
    let current_unpushed = match &current {
        Some(current) => count_at(
            repo,
            &[current.oid.as_str()],
            &[current.name.as_str()],
            &local,
        ),
        None => count_at(repo, &[], &[], &local),
    };
    let other_oids: Vec<&str> = others.iter().map(|other| other.oid.as_str()).collect();
    let other_names: Vec<&str> = others.iter().map(|other| other.name.as_str()).collect();
    let others_unpushed = count_at(repo, &other_oids, &other_names, &local);
    let mut all_oids = other_oids.clone();
    let mut all_names = other_names.clone();
    if let Some(current) = &current {
        all_oids.push(current.oid.as_str());
        all_names.push(current.name.as_str());
    }
    let both_unpushed = count_at(repo, &all_oids, &all_names, &local);
    BranchOffer {
        current_name: inputs.branch_name.clone(),
        current,
        current_unpushed,
        others,
        others_unpushed,
        both_unpushed,
        protected,
        newly_seen,
    }
}

/// One delete-dialog box as a request carries it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoxWire {
    /// Whether the box was ticked.
    #[serde(default)]
    pub delete: bool,
    /// The branches the ticked box covers, each at the commit the dialog read.
    #[serde(default)]
    pub branches: Vec<PinnedBranch>,
    /// Set when the box pins nothing for a reason other than being unticked:
    /// `pending` while the dialog was still checking, `failed` when the check
    /// failed, `not_offered` when the dialog had no branch to show in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unchecked: Option<Unchecked>,
    /// Every branch the box showed, ticked or not.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub offered: Vec<String>,
}

/// Why a dialog box had nothing to pin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unchecked {
    /// The check had not finished.
    Pending,
    /// The check failed.
    Failed,
    /// The dialog did not show the box: it had no branch to offer in it.
    NotOffered,
}

/// Both boxes as a delete request carries them. A box left out was not asked.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchAnswersWire {
    /// The first box: the branch the worktree is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<BoxWire>,
    /// The second box: every other branch the worktree has been on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub others: Option<BoxWire>,
}

/// Whether `oid` is a full object id as git prints it: 40 (SHA-1) or 64
/// (SHA-256) lowercase hex digits.
fn is_full_oid(oid: &str) -> bool {
    matches!(oid.len(), 40 | 64) && oid.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Whether `name` could be a branch name at all. Membership in the agent's
/// own list is what really admits one; this only refuses what no refname is.
fn is_plausible_branch_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains("..")
        && !name.contains("@{")
        && !name
            .chars()
            .any(|c| c.is_control() || matches!(c, ' ' | ':' | '~' | '^' | '?' | '*' | '[' | '\\'))
}

impl BranchAnswersWire {
    /// The request these answers make, refusing anything malformed rather than
    /// reading it as something it does not say: a malformed pin never becomes
    /// a delete without one. `legacy` is the older single `delete_branch`
    /// answer, which cannot be combined with the boxes.
    ///
    /// # Errors
    ///
    /// The sentence saying what is wrong with the request.
    pub fn to_request(
        answers: Option<&Self>,
        legacy: Option<bool>,
    ) -> Result<BranchDeleteRequest, String> {
        let Some(answers) = answers else {
            return Ok(BranchDeleteRequest::from_legacy(legacy));
        };
        if legacy.is_some() {
            return Err(
                "delete_branch cannot be combined with the branch answers; send one or the other"
                    .to_string(),
            );
        }
        let mut named: Vec<String> = Vec::new();
        let mut parse = |which: &str, answer: Option<&BoxWire>, single: bool| {
            let Some(answer) = answer else {
                return Ok(BoxAnswer::Unasked);
            };
            for pin in &answer.branches {
                if !is_plausible_branch_name(&pin.name) {
                    return Err(format!("the {which} box names an invalid branch"));
                }
                if !is_full_oid(&pin.oid) {
                    return Err(format!(
                        "the {which} box pins branch {} to {}, which is not a full commit id",
                        pin.name, pin.oid
                    ));
                }
                if named.contains(&pin.name) {
                    return Err(format!("branch {} is named twice", pin.name));
                }
                named.push(pin.name.clone());
            }
            match (answer.unchecked, answer.delete, answer.branches.len()) {
                (Some(_), _, n) if n > 0 => Err(format!(
                    "the {which} box says it was not checked but pins branches"
                )),
                (Some(Unchecked::NotOffered), _, _) => Ok(BoxAnswer::NotOffered),
                (Some(unchecked), _, _) => Ok(BoxAnswer::NotChecked {
                    failed: unchecked == Unchecked::Failed,
                }),
                (None, false, 0) => Ok(BoxAnswer::Keep),
                (None, false, _) => Err(format!("the {which} box is not ticked but pins branches")),
                (None, true, 0) => Err(format!(
                    "the {which} box is ticked but pins no branch; nothing was deleted"
                )),
                (None, true, n) if single && n > 1 => {
                    Err(format!("the {which} box pins more than one branch"))
                }
                (None, true, _) => Ok(BoxAnswer::Delete(answer.branches.clone())),
            }
        };
        let current = parse("first", answers.current.as_ref(), true)?;
        let others = parse("second", answers.others.as_ref(), false)?;
        let mut offered = Vec::new();
        for (role, answer) in [
            (BranchRole::Current, &answers.current),
            (BranchRole::Other, &answers.others),
        ] {
            for name in answer.iter().flat_map(|answer| answer.offered.iter()) {
                if !is_plausible_branch_name(name) {
                    return Err("a box names an invalid branch it showed".to_string());
                }
                offered.push((role, name.clone()));
            }
        }
        Ok(BranchDeleteRequest {
            current,
            others,
            offered,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn git(dir: &Path, args: &[&str]) -> String {
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
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A repository on `main` with one commit, and a worktree on `agent`
    /// (made the way dux makes one) at `<tmp>/wt`.
    fn repo_and_worktree(tmp: &Path) -> (PathBuf, PathBuf) {
        let repo = tmp.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "--initial-branch=main"]);
        git(&repo, &["config", "user.email", "t@example.com"]);
        git(&repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("f"), "x").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "init"]);
        let worktree = tmp.join("wt");
        git(
            &repo,
            &["worktree", "add", "-b", "agent", worktree.to_str().unwrap()],
        );
        (repo, worktree)
    }

    fn long_ago() -> DateTime<Utc> {
        Utc::now() - chrono::Duration::hours(1)
    }

    fn oid(repo: &Path, name: &str) -> String {
        git(repo, &["rev-parse", &format!("refs/heads/{name}")])
    }

    fn entry(name: &str) -> BranchHistoryEntry {
        BranchHistoryEntry {
            name: name.to_string(),
            born_here: false,
            identity: None,
            tip: None,
        }
    }

    fn birth(name: &str) -> BranchHistoryEntry {
        BranchHistoryEntry {
            born_here: true,
            ..entry(name)
        }
    }

    fn pin(name: &str, oid: &str) -> PinnedBranch {
        PinnedBranch {
            name: name.to_string(),
            oid: oid.to_string(),
        }
    }

    // ── the HEAD reflog ─────────────────────────────────────────────────────

    /// `a -> b -> a` between two branch checks leaves the branch name exactly
    /// where it was; only the HEAD reflog shows `b`.
    #[test]
    fn the_head_reflog_shows_a_branch_the_worktree_passed_through() {
        let tmp = tempfile::tempdir().unwrap();
        let (_repo, wt) = repo_and_worktree(tmp.path());
        git(&wt, &["switch", "-c", "visited"]);
        git(&wt, &["switch", "agent"]);
        let (seen, cursor) = scan_head_reflog(&wt, long_ago(), None).unwrap();
        assert!(seen.contains(&"visited".to_string()), "{seen:?}");

        // From the cursor on, only what happened since is read.
        let (again, _) = scan_head_reflog(&wt, long_ago(), cursor.as_ref()).unwrap();
        assert!(again.is_empty(), "nothing new since the cursor: {again:?}");
        git(&wt, &["switch", "-c", "later"]);
        let (next, _) = scan_head_reflog(&wt, long_ago(), cursor.as_ref()).unwrap();
        assert_eq!(next, vec!["later".to_string(), "agent".to_string()]);
    }

    /// A detached hop, a branch deleted since, and a dash-leading name: only
    /// local branches that exist are recorded, read as names, never options.
    #[test]
    fn the_head_reflog_keeps_only_branches_that_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, wt) = repo_and_worktree(tmp.path());
        git(&wt, &["switch", "--detach"]);
        git(&wt, &["switch", "-c", "gone"]);
        git(&repo, &["update-ref", "refs/heads/-dash", "HEAD"]);
        git(
            &wt,
            &[
                "symbolic-ref",
                "-m",
                "checkout: moving from gone to -dash",
                "HEAD",
                "refs/heads/-dash",
            ],
        );
        git(&wt, &["switch", "agent"]);
        git(&repo, &["branch", "-D", "gone"]);
        let (seen, _) = scan_head_reflog(&wt, long_ago(), None).unwrap();
        assert!(seen.contains(&"-dash".to_string()), "{seen:?}");
        assert!(seen.contains(&"agent".to_string()), "{seen:?}");
        assert!(
            !seen.iter().any(|name| name == "gone" || name == "HEAD"),
            "{seen:?}"
        );
    }

    /// The user's output encoding is overridden, so a non-ASCII branch name
    /// reads back byte for byte.
    #[test]
    fn the_reflog_reads_unicode_names_whatever_the_output_encoding() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, wt) = repo_and_worktree(tmp.path());
        git(&repo, &["config", "i18n.logOutputEncoding", "UTF-16"]);
        git(&wt, &["switch", "-c", "ünïcødé-分支"]);
        let (seen, _) = scan_head_reflog(&wt, long_ago(), None).unwrap();
        assert!(seen.contains(&"ünïcødé-分支".to_string()), "{seen:?}");
    }

    /// Entries from before the agent existed (an adopted worktree's history)
    /// are not the agent's.
    #[test]
    fn the_head_reflog_ignores_switches_before_the_agent_was_born() {
        let tmp = tempfile::tempdir().unwrap();
        let (_repo, wt) = repo_and_worktree(tmp.path());
        git(&wt, &["switch", "-c", "older"]);
        git(&wt, &["switch", "agent"]);
        let born = Utc::now() + chrono::Duration::hours(1);
        let (seen, _) = scan_head_reflog(&wt, born, None).unwrap();
        assert!(seen.is_empty(), "{seen:?}");
    }

    // ── who made a branch ───────────────────────────────────────────────────

    #[test]
    fn a_branch_made_after_the_agent_is_after_and_one_made_before_is_before() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, _wt) = repo_and_worktree(tmp.path());
        git(&repo, &["branch", "fresh"]);
        assert_eq!(branch_birth(&repo, "fresh", long_ago()), BranchBirth::After);
        let later = Utc::now() + chrono::Duration::hours(1);
        assert_eq!(branch_birth(&repo, "fresh", later), BranchBirth::Before);
        // The very same second proves nothing either way.
        let created = branch_creation_entry(&repo, "fresh").unwrap().unwrap();
        let same = DateTime::from_timestamp(created.unix, 500_000_000).unwrap();
        assert_eq!(branch_birth(&repo, "fresh", same), BranchBirth::Unknown);
    }

    /// No reflog, or one whose creation entry expired, is never read as the
    /// branch's birth.
    #[test]
    fn a_branch_with_no_creation_entry_is_unknown() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, _wt) = repo_and_worktree(tmp.path());
        // Plumbing with no message writes an entry that is not a creation.
        git(&repo, &["update-ref", "refs/heads/silent", "HEAD"]);
        assert_eq!(
            branch_birth(&repo, "silent", long_ago()),
            BranchBirth::Unknown
        );

        git(&repo, &["branch", "aged"]);
        git(&repo, &["commit", "--allow-empty", "-m", "next"]);
        git(
            &repo,
            &[
                "update-ref",
                "-m",
                "later move",
                "refs/heads/aged",
                "refs/heads/main",
            ],
        );
        git(
            &repo,
            &["reflog", "delete", "--rewrite", "refs/heads/aged@{1}"],
        );
        assert_eq!(
            branch_birth(&repo, "aged", long_ago()),
            BranchBirth::Unknown
        );
    }

    /// The birth row names a name; the ref under it must be the one dux made.
    /// A dux rename keeps the reflog and so the identity; a different branch
    /// made later under the same name does not.
    #[test]
    fn the_birth_branch_is_duxs_only_while_its_ref_is_the_one_dux_made() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, _wt) = repo_and_worktree(tmp.path());
        git(&repo, &["branch", "minted"]);
        let identity = branch_creation_entry(&repo, "minted").unwrap().unwrap();
        let born = |name: &str| BranchHistoryEntry {
            name: name.to_string(),
            born_here: true,
            identity: Some(identity.clone()),
            tip: None,
        };
        let origin = |entry: &BranchHistoryEntry| {
            branch_origin(
                &repo,
                entry,
                BranchProvenance::CreatedByDux,
                long_ago(),
                None,
            )
        };
        assert_eq!(origin(&born("minted")), BranchOrigin::Dux);
        git(&repo, &["branch", "-m", "minted", "renamed"]);
        assert_eq!(
            origin(&born("renamed")),
            BranchOrigin::Dux,
            "renamed by dux"
        );
        // Same name, different ref (made a second later, so a new entry).
        std::thread::sleep(std::time::Duration::from_millis(1100));
        git(&repo, &["branch", "minted"]);
        assert_ne!(origin(&born("minted")), BranchOrigin::Dux);
    }

    /// A legacy birth row (no identity) made well before the agent's record
    /// is dux's only while the agent's worktree is on it.
    #[test]
    fn a_legacy_birth_branch_is_duxs_only_while_checked_out_here() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, _wt) = repo_and_worktree(tmp.path());
        let legacy = birth("agent");
        let origin = |here: Option<&str>| {
            branch_origin(
                &repo,
                &legacy,
                BranchProvenance::CreatedByDux,
                long_ago(),
                here,
            )
        };
        assert_eq!(origin(Some("agent")), BranchOrigin::Dux);
        assert_ne!(origin(Some("elsewhere")), BranchOrigin::Dux);
        assert_ne!(origin(None), BranchOrigin::Dux);
    }

    // ── the plan ────────────────────────────────────────────────────────────

    const A: &str = "1111111111111111111111111111111111111111";
    const B: &str = "2222222222222222222222222222222222222222";

    fn planned(plan: &BranchPlan) -> Vec<(&str, &str)> {
        plan.delete
            .iter()
            .map(|d| (d.branch.name.as_str(), d.branch.oid.as_str()))
            .collect()
    }

    fn kept<'a>(plan: &'a BranchPlan, name: &str) -> Option<&'a KeptBecause> {
        plan.keep
            .iter()
            .find(|keep| keep.name == name)
            .map(|keep| &keep.because)
    }

    fn history() -> Vec<BranchHistoryEntry> {
        vec![birth("born"), entry("cur"), entry("visited")]
    }

    #[test]
    fn both_boxes_ticked_delete_exactly_what_they_pinned() {
        let request = BranchDeleteRequest {
            current: BoxAnswer::Delete(vec![pin("cur", A)]),
            others: BoxAnswer::Delete(vec![pin("born", B)]),
            offered: Vec::new(),
        };
        let plan = plan_branch_removal(
            "cur",
            &history(),
            BranchProvenance::CreatedByDux,
            None,
            &request,
            &[],
        );
        assert_eq!(planned(&plan), vec![("cur", A), ("born", B)]);
        assert_eq!(
            kept(&plan, "visited"),
            Some(&KeptBecause::NotOffered),
            "recorded after the dialog read the list"
        );
        assert_eq!(plan.resolve, Vec::new());
    }

    #[test]
    fn an_unticked_box_keeps_its_branches_and_says_why() {
        let request = BranchDeleteRequest {
            current: BoxAnswer::Delete(vec![pin("cur", A)]),
            others: BoxAnswer::Keep,
            offered: Vec::new(),
        };
        let plan = plan_branch_removal(
            "cur",
            &history(),
            BranchProvenance::CreatedByDux,
            None,
            &request,
            &[],
        );
        assert_eq!(planned(&plan), vec![("cur", A)]);
        assert_eq!(kept(&plan, "born"), Some(&KeptBecause::UserDeclined));
        assert_eq!(kept(&plan, "visited"), Some(&KeptBecause::NotTicked));

        let request = BranchDeleteRequest {
            current: BoxAnswer::Keep,
            others: BoxAnswer::Delete(vec![pin("born", B), pin("visited", A)]),
            offered: Vec::new(),
        };
        let plan = plan_branch_removal(
            "cur",
            &history(),
            BranchProvenance::AttachedExisting,
            None,
            &request,
            &[],
        );
        assert_eq!(planned(&plan), vec![("born", B), ("visited", A)]);
        assert_eq!(kept(&plan, "cur"), Some(&KeptBecause::NotTicked));
    }

    /// Nobody asked: only the birth branch is left for git to vouch for at the
    /// removal; a switched-to current branch and a visited one are kept.
    #[test]
    fn an_unasked_delete_leaves_only_the_birth_branch_to_git() {
        let plan = plan_branch_removal(
            "cur",
            &history(),
            BranchProvenance::CreatedByDux,
            None,
            &BranchDeleteRequest::unasked(),
            &[],
        );
        assert_eq!(plan.delete, Vec::new());
        assert_eq!(plan.resolve.len(), 1);
        assert_eq!(plan.resolve[0].name, "born");
        assert_eq!(kept(&plan, "cur"), Some(&KeptBecause::NobodyAsked));
        assert_eq!(kept(&plan, "visited"), Some(&KeptBecause::NobodyAsked));
    }

    #[test]
    fn a_dialog_that_had_not_checked_deletes_nothing() {
        let request = BranchDeleteRequest {
            current: BoxAnswer::NotChecked { failed: false },
            others: BoxAnswer::NotChecked { failed: true },
            offered: Vec::new(),
        };
        let plan = plan_branch_removal(
            "cur",
            &history(),
            BranchProvenance::CreatedByDux,
            None,
            &request,
            &[],
        );
        assert!(plan.delete.is_empty() && plan.resolve.is_empty());
        assert_eq!(
            kept(&plan, "cur"),
            Some(&KeptBecause::NotChecked { failed: false })
        );
        assert_eq!(
            kept(&plan, "born"),
            Some(&KeptBecause::NotChecked { failed: true })
        );
    }

    /// The project's own branch is never deleted, whatever a request pins,
    /// and a pin for a branch the agent was never on is kept and named.
    #[test]
    fn protected_and_unknown_branches_are_never_planned() {
        let mut list = history();
        list.push(entry("main"));
        let request = BranchDeleteRequest {
            current: BoxAnswer::Delete(vec![pin("cur", A)]),
            others: BoxAnswer::Delete(vec![pin("main", A), pin("stranger", B)]),
            offered: Vec::new(),
        };
        let plan = plan_branch_removal(
            "cur",
            &list,
            BranchProvenance::CreatedByDux,
            None,
            &request,
            &["main".to_string()],
        );
        assert_eq!(planned(&plan), vec![("cur", A)]);
        assert_eq!(kept(&plan, "main"), Some(&KeptBecause::Protected));
        assert_eq!(kept(&plan, "stranger"), Some(&KeptBecause::Unrecorded));
    }

    // ── the request ─────────────────────────────────────────────────────────

    fn ticked(branches: Vec<PinnedBranch>) -> BoxWire {
        BoxWire {
            delete: true,
            branches,
            unchecked: None,
            offered: Vec::new(),
        }
    }

    #[test]
    fn a_malformed_request_is_refused_and_never_read_as_an_unpinned_delete() {
        let refused = |answers: BranchAnswersWire| {
            BranchAnswersWire::to_request(Some(&answers), None).unwrap_err()
        };
        refused(BranchAnswersWire {
            current: Some(ticked(vec![pin("cur", "1234abc")])),
            others: None,
        });
        refused(BranchAnswersWire {
            current: Some(ticked(Vec::new())),
            others: None,
        });
        refused(BranchAnswersWire {
            current: Some(ticked(vec![pin("cur", A)])),
            others: Some(ticked(vec![pin("cur", B)])),
        });
        refused(BranchAnswersWire {
            current: None,
            others: Some(ticked(vec![pin("a b", A)])),
        });
        assert!(
            BranchAnswersWire::to_request(Some(&BranchAnswersWire::default()), Some(true)).is_err(),
            "the older answer cannot be combined with the boxes"
        );
        assert_eq!(
            BranchAnswersWire::to_request(
                Some(&BranchAnswersWire {
                    current: Some(ticked(vec![pin("cur", A)])),
                    others: Some(BoxWire::default()),
                }),
                None
            )
            .unwrap(),
            BranchDeleteRequest {
                current: BoxAnswer::Delete(vec![pin("cur", A)]),
                others: BoxAnswer::Keep,
                offered: Vec::new(),
            }
        );
    }

    // ── execution ───────────────────────────────────────────────────────────

    fn delete(name: &str, oid: &str) -> PlannedDelete {
        PlannedDelete {
            role: BranchRole::Other,
            born_here: false,
            branch: pin(name, oid),
        }
    }

    fn fate_of<'a>(outcomes: &'a [BranchOutcome], name: &str) -> &'a BranchFate {
        &outcomes.iter().find(|o| o.name == name).unwrap().fate
    }

    /// Compare-and-delete: a branch that moved since the dialog is kept, one
    /// already gone is said to be gone, one checked out in another worktree is
    /// refused.
    #[test]
    fn a_branch_is_deleted_only_while_it_points_where_the_dialog_saw_it() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, wt) = repo_and_worktree(tmp.path());
        git(&repo, &["branch", "still"]);
        git(&repo, &["branch", "moving"]);
        let shown = oid(&repo, "moving");
        git(&repo, &["branch", "gone"]);
        let gone = oid(&repo, "gone");
        git(&repo, &["branch", "-D", "gone"]);
        git(&wt, &["commit", "--allow-empty", "-m", "on agent"]);
        git(
            &repo,
            &["update-ref", "refs/heads/moving", "refs/heads/agent"],
        );
        let resolved = ResolvedPlan {
            delete: vec![
                delete("still", &oid(&repo, "still")),
                delete("moving", &shown),
                delete("gone", &gone),
                delete("agent", &oid(&repo, "agent")),
            ],
            keep: Vec::new(),
        };
        let outcomes = execute_plan(&repo, &resolved);
        assert_eq!(fate_of(&outcomes, "still"), &BranchFate::Deleted);
        assert_eq!(fate_of(&outcomes, "moving"), &BranchFate::Moved);
        assert_eq!(fate_of(&outcomes, "gone"), &BranchFate::AlreadyGone);
        assert!(
            matches!(fate_of(&outcomes, "agent"), BranchFate::Refused { reason } if reason.contains("checked out")),
            "{outcomes:?}"
        );
        assert!(!local_branches(&repo).unwrap().contains("still"));
        assert!(local_branches(&repo).unwrap().contains("moving"));
    }

    /// A name that became a symbolic ref is removed as itself; the branch it
    /// points at survives.
    #[test]
    fn a_symbolic_ref_is_removed_as_itself_and_its_target_survives() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, _wt) = repo_and_worktree(tmp.path());
        git(&repo, &["branch", "target"]);
        let at = oid(&repo, "target");
        git(
            &repo,
            &["symbolic-ref", "refs/heads/alias", "refs/heads/target"],
        );
        let resolved = ResolvedPlan {
            delete: vec![delete("alias", &at)],
            keep: Vec::new(),
        };
        let _ = execute_plan(&repo, &resolved);
        assert_eq!(oid(&repo, "target"), at, "the target is untouched");
    }

    /// The count is about the commits pinned, not wherever the name points by
    /// the time it is asked.
    #[test]
    fn unpushed_commits_are_counted_from_the_pinned_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, wt) = repo_and_worktree(tmp.path());
        let pinned = oid(&repo, "agent");
        git(&wt, &["commit", "--allow-empty", "-m", "after the dialog"]);
        let at_pin = crate::git::unpushed_commits_at(&repo, &[pinned.as_str()], &[]).unwrap();
        let by_name = crate::git::unpushed_commit_count(&repo, &["agent"]).unwrap();
        assert_eq!(at_pin.count + 1, by_name.count);
    }

    /// A worktree git lets go of while its folder stays keeps every branch the
    /// plan would have deleted, each at its pinned commit.
    #[test]
    fn a_left_over_folder_keeps_every_planned_branch_at_its_pin() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, wt) = repo_and_worktree(tmp.path());
        git(&repo, &["branch", "visited"]);
        let plan = BranchPlan {
            delete: vec![
                PlannedDelete {
                    role: BranchRole::Current,
                    born_here: true,
                    branch: pin("agent", &oid(&repo, "agent")),
                },
                delete("visited", A),
            ],
            ..BranchPlan::default()
        };
        // Something the removal cannot delete: a read-only directory inside.
        let stuck = wt.join("stuck");
        std::fs::create_dir_all(stuck.join("inner")).unwrap();
        std::fs::write(stuck.join("inner/f"), "x").unwrap();
        let mut perms = std::fs::metadata(&stuck).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o500);
        std::fs::set_permissions(&stuck, perms.clone()).unwrap();
        let result = remove_worktree_with_plan(&repo, &wt, &plan, BranchProvenance::CreatedByDux);
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o700);
        let _ = std::fs::set_permissions(&stuck, perms);
        let err = result.expect_err("the folder stays");
        let Some(leftover) = err.downcast_ref::<crate::git::LeftoverWorktreeFolder>() else {
            // Running as root: nothing is read-only, so there is no left-over.
            return;
        };
        let names: Vec<(&str, Option<&str>)> = leftover
            .kept_branches
            .iter()
            .map(|kept| (kept.name.as_str(), kept.oid.as_deref()))
            .collect();
        assert!(names.contains(&("visited", Some(A))), "{names:?}");
        assert!(names.iter().any(|(name, _)| *name == "agent"), "{names:?}");
        assert!(local_branches(&repo).unwrap().contains("visited"));
    }

    // ── the offer ───────────────────────────────────────────────────────────

    fn inputs(
        repo: &Path,
        wt: &Path,
        current: &str,
        provenance: BranchProvenance,
        history: Vec<BranchHistoryEntry>,
        protected: &[&str],
    ) -> crate::engine::BranchDeleteInputs {
        crate::engine::BranchDeleteInputs {
            project_path: repo.to_string_lossy().into_owned(),
            branch_name: current.to_string(),
            initial_branch: current.to_string(),
            branch_provenance: provenance,
            worktree_path: wt.to_string_lossy().into_owned(),
            created_at: long_ago(),
            history,
            protected: protected.iter().map(|p| (*p).to_string()).collect(),
            session_id: "s1".to_string(),
        }
    }

    /// An adopted worktree's branch is its own: offered in the first box,
    /// unticked, as the branch that came with the worktree.
    #[test]
    fn an_adopted_agents_own_branch_is_offered_as_adopted() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, wt) = repo_and_worktree(tmp.path());
        let offer = delete_branch_offer(&inputs(
            &repo,
            &wt,
            "agent",
            BranchProvenance::Adopted,
            vec![birth("agent")],
            &[],
        ));
        let current = offer.current.as_ref().expect("the first box");
        assert_eq!(current.origin, BranchOrigin::Adopted);
        assert!(!offer.current_default());
        assert_eq!(offer.protected, Vec::<String>::new());
    }

    /// The project's own branch the agent was on is named as protected and
    /// offered in no box, never reported as a failed check.
    #[test]
    fn the_projects_own_branch_is_named_as_protected() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, wt) = repo_and_worktree(tmp.path());
        let offer = delete_branch_offer(&inputs(
            &repo,
            &wt,
            "agent",
            BranchProvenance::CreatedByDux,
            vec![birth("agent"), entry("main")],
            &["main"],
        ));
        assert_eq!(offer.protected, vec!["main".to_string()]);
        assert!(offer.others.is_empty(), "{:?}", offer.others);
        assert!(offer.current.is_some());
    }

    /// A name the agent was on that now holds a different branch is not
    /// offered, and a delete that kept it says why.
    #[test]
    fn a_different_branch_under_a_recorded_name_is_not_offered() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, wt) = repo_and_worktree(tmp.path());
        git(&repo, &["branch", "spike"]);
        let recorded = branch_creation_entry(&repo, "spike").unwrap().unwrap();
        git(&repo, &["branch", "-D", "spike"]);
        std::thread::sleep(std::time::Duration::from_millis(1100));
        git(&wt, &["commit", "--allow-empty", "-m", "elsewhere"]);
        git(&repo, &["branch", "spike", "refs/heads/agent"]);
        let spike = BranchHistoryEntry {
            identity: Some(recorded.clone()),
            ..entry("spike")
        };
        let offer = delete_branch_offer(&inputs(
            &repo,
            &wt,
            "agent",
            BranchProvenance::CreatedByDux,
            vec![birth("agent"), spike],
            &[],
        ));
        assert!(offer.others.is_empty(), "{:?}", offer.others);

        let plan = BranchPlan {
            keep: vec![PlannedKeep {
                role: BranchRole::Other,
                name: "spike".to_string(),
                because: KeptBecause::NotOffered,
                identity: Some(recorded),
                tip: None,
            }],
            ..BranchPlan::default()
        };
        let resolved = resolve_plan(&repo, &wt, &plan);
        assert_eq!(
            resolved.keep,
            vec![(
                BranchRole::Other,
                "spike".to_string(),
                KeptBecause::Replaced
            )]
        );
    }

    /// A dux-made birth branch whose reflog git expired is still dux's while
    /// the agent's worktree has it checked out and still holds the commit dux
    /// made it at; anywhere else it cannot be proven.
    #[test]
    fn an_expired_reflog_does_not_unmake_the_birth_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, wt) = repo_and_worktree(tmp.path());
        let identity = branch_creation_entry(&repo, "agent").unwrap().unwrap();
        git(&wt, &["commit", "--allow-empty", "-m", "work"]);
        git(
            &repo,
            &["reflog", "expire", "--expire=all", "refs/heads/agent"],
        );
        assert_eq!(branch_creation_entry(&repo, "agent").unwrap(), None);
        let born = BranchHistoryEntry {
            tip: Some(identity.oid.clone()),
            identity: Some(identity),
            ..birth("agent")
        };
        assert!(birth_is_duxs(&repo, &born, Some("agent"), None));
        assert!(!birth_is_duxs(&repo, &born, Some("other"), None));
        // With no tip recorded, the commit the branch was made at proves
        // nothing: it is in almost every branch.
        let untipped = BranchHistoryEntry { tip: None, ..born };
        assert!(!birth_is_duxs(&repo, &untipped, Some("agent"), None));
        assert_eq!(sameness(&repo, &untipped), SameBranch::Unconfirmed);
    }

    /// A birth row's tip is the commit the branch was made at, which a
    /// different branch of the same name made from main also descends from:
    /// once the reflog is gone and the agent has switched away, nothing proves
    /// the name is still dux's, so it is neither offered as dux's nor deleted
    /// unasked.
    #[test]
    fn a_recreated_birth_name_off_the_agents_worktree_is_not_duxs() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, wt) = repo_and_worktree(tmp.path());
        let identity = branch_creation_entry(&repo, "agent").unwrap().unwrap();
        git(&wt, &["switch", "-c", "elsewhere"]);
        git(&repo, &["branch", "-D", "agent"]);
        git(
            &repo,
            &["update-ref", "refs/heads/agent", "refs/heads/main"],
        );
        git(
            &repo,
            &["reflog", "expire", "--expire=all", "refs/heads/agent"],
        );
        assert_eq!(branch_creation_entry(&repo, "agent").unwrap(), None);
        let born = BranchHistoryEntry {
            tip: Some(identity.oid.clone()),
            identity: Some(identity),
            ..birth("agent")
        };
        assert!(!birth_is_duxs(
            &repo,
            &born,
            Some("elsewhere"),
            Some(long_ago())
        ));
        assert_ne!(
            branch_origin(
                &repo,
                &born,
                BranchProvenance::CreatedByDux,
                long_ago(),
                Some("elsewhere"),
            ),
            BranchOrigin::Dux
        );
        let plan = plan_branch_removal(
            "elsewhere",
            &[born, entry("elsewhere")],
            BranchProvenance::CreatedByDux,
            Some(long_ago()),
            &BranchDeleteRequest::unasked(),
            &[],
        );
        let resolved = resolve_plan(&repo, &wt, &plan);
        assert!(
            !resolved.delete.iter().any(|d| d.branch.name == "agent"),
            "{resolved:?}"
        );
        assert_eq!(oid(&repo, "agent"), oid(&repo, "main"));
    }

    /// A row with no identity but a recorded tip is the same branch only
    /// while the ref has not moved off that tip; with neither, nothing says
    /// otherwise.
    #[test]
    fn a_row_without_an_identity_is_held_to_its_recorded_tip() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, wt) = repo_and_worktree(tmp.path());
        let tip = oid(&repo, "agent");
        git(&wt, &["commit", "--allow-empty", "-m", "on top"]);
        let tipped = BranchHistoryEntry {
            tip: Some(tip),
            ..entry("agent")
        };
        assert_eq!(sameness(&repo, &tipped), SameBranch::Yes);
        git(
            &repo,
            &["update-ref", "refs/heads/elsewhere", "refs/heads/agent~1"],
        );
        git(&wt, &["commit", "--allow-empty", "-m", "more"]);
        let moved = BranchHistoryEntry {
            tip: Some(oid(&repo, "agent")),
            ..entry("elsewhere")
        };
        assert_eq!(sameness(&repo, &moved), SameBranch::Unconfirmed);
        assert_eq!(sameness(&repo, &entry("elsewhere")), SameBranch::Yes);
    }

    /// The current branch is checked out in the agent's own worktree: an
    /// unticked one keeps its box's reason, whatever its recorded identity.
    #[test]
    fn the_current_branch_keeps_its_box_reason() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, wt) = repo_and_worktree(tmp.path());
        let plan = BranchPlan {
            keep: vec![PlannedKeep {
                role: BranchRole::Current,
                name: "agent".to_string(),
                because: KeptBecause::NotTicked,
                identity: Some(BranchIdentity {
                    unix: 1,
                    oid: "c".repeat(40),
                }),
                tip: None,
            }],
            ..BranchPlan::default()
        };
        assert_eq!(
            resolve_plan(&repo, &wt, &plan).keep,
            vec![(
                BranchRole::Current,
                "agent".to_string(),
                KeptBecause::NotTicked
            )]
        );
    }

    /// A legacy birth row (no identity) whose own creation entry is from the
    /// moment the agent was created is dux's, wherever the worktree is now.
    #[test]
    fn a_legacy_birth_branch_made_with_the_agent_is_duxs() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, _wt) = repo_and_worktree(tmp.path());
        let made = branch_creation_entry(&repo, "agent").unwrap().unwrap();
        let born_with = DateTime::from_timestamp(made.unix + 1, 0).unwrap();
        assert!(birth_is_duxs(&repo, &birth("agent"), None, Some(born_with)));
        let long_after = DateTime::from_timestamp(made.unix + 3600, 0).unwrap();
        assert!(!birth_is_duxs(
            &repo,
            &birth("agent"),
            None,
            Some(long_after)
        ));
    }

    /// Each box counts what its own deletion loses: a commit another local
    /// branch still holds is not lost.
    #[test]
    fn a_box_does_not_count_commits_another_branch_keeps() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, wt) = repo_and_worktree(tmp.path());
        git(&wt, &["commit", "--allow-empty", "-m", "shared"]);
        git(&repo, &["branch", "keeper", "refs/heads/agent"]);
        git(&wt, &["commit", "--allow-empty", "-m", "only here"]);
        let tip = oid(&repo, "agent");
        let alone = crate::git::unpushed_commits_at(&repo, &[tip.as_str()], &[]).unwrap();
        let kept = crate::git::unpushed_commits_at(&repo, &[tip.as_str()], &["keeper"]).unwrap();
        assert_eq!((alone.count, kept.count), (3, 1));
    }

    /// A pin is matched by name whichever box carried it, so a branch the
    /// worktree switched off mid-dialog is still deleted as ticked, and a pin
    /// for a branch the agent never had is kept and named.
    #[test]
    fn pins_are_matched_by_name_and_an_unrecorded_one_is_reported() {
        let request = BranchDeleteRequest {
            current: BoxAnswer::Delete(vec![pin("visited", A)]),
            others: BoxAnswer::Delete(vec![pin("stranger", B)]),
            offered: Vec::new(),
        };
        let plan = plan_branch_removal(
            "cur",
            &history(),
            BranchProvenance::CreatedByDux,
            None,
            &request,
            &[],
        );
        assert_eq!(planned(&plan), vec![("visited", A)]);
        assert_eq!(kept(&plan, "stranger"), Some(&KeptBecause::Unrecorded));
        assert_eq!(kept(&plan, "cur"), Some(&KeptBecause::NotOffered));
    }

    /// A box the dialog did not show deletes nothing it would have covered.
    #[test]
    fn a_box_the_dialog_did_not_show_deletes_nothing() {
        let request = BranchDeleteRequest {
            current: BoxAnswer::Keep,
            others: BoxAnswer::NotOffered,
            offered: Vec::new(),
        };
        let plan = plan_branch_removal(
            "cur",
            &history(),
            BranchProvenance::CreatedByDux,
            None,
            &request,
            &[],
        );
        assert_eq!(plan.delete, Vec::new());
        assert_eq!(plan.resolve, Vec::new());
        assert_eq!(kept(&plan, "born"), Some(&KeptBecause::NotOffered));
    }

    /// An older dux reading the pending row deletes no more than the plan:
    /// it is told yes only when the plan deletes exactly the two branches its
    /// own yes would.
    #[test]
    fn the_older_answer_is_yes_only_for_exactly_its_own_two_branches() {
        let managed = crate::model::ManagedWorkspace {
            project_id: "p".to_string(),
            project_path: None,
            source_branch: "main".to_string(),
            branch_name: "cur".to_string(),
            initial_branch: "born".to_string(),
            branch_provenance: BranchProvenance::CreatedByDux,
            worktree_path: "/wt".to_string(),
        };
        let plan = |names: &[&str]| BranchPlan {
            delete: names.iter().map(|name| delete(name, A)).collect(),
            ..BranchPlan::default()
        };
        assert!(plan(&["born", "cur"]).old_rule_answer(&managed));
        assert!(!plan(&["cur"]).old_rule_answer(&managed));
        assert!(!plan(&["born", "cur", "visited"]).old_rule_answer(&managed));
        assert!(!BranchPlan::default().old_rule_answer(&managed));
    }

    /// A dux-made `born` with three commits of its own, and `drifted` made on
    /// top of it with one more: ticking only one box loses only what no
    /// branch that stays holds, and ticking both loses all four, which the two
    /// counts apart would hide.
    #[test]
    fn each_tick_state_counts_exactly_what_its_delete_loses() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, wt) = repo_and_worktree(tmp.path());
        for n in 0..3 {
            git(
                &wt,
                &["commit", "--allow-empty", "-m", &format!("born {n}")],
            );
        }
        git(&wt, &["switch", "-c", "drifted"]);
        git(&wt, &["commit", "--allow-empty", "-m", "drifted"]);
        let identity = branch_creation_entry(&repo, "agent").unwrap();
        let born = BranchHistoryEntry {
            identity,
            ..birth("agent")
        };
        let offer = delete_branch_offer(&inputs(
            &repo,
            &wt,
            "drifted",
            BranchProvenance::CreatedByDux,
            vec![born, entry("drifted")],
            &[],
        ));
        let count =
            |answer: &Result<crate::git::UnpushedCommits, String>| answer.as_ref().unwrap().count;
        assert_eq!(
            (
                count(&offer.current_unpushed),
                count(&offer.others_unpushed),
                count(&offer.both_unpushed)
            ),
            (1, 0, 4)
        );
        let (deleted, lost) = offer
            .lost(
                crate::delete_agent_prose::BoxState::Ticked,
                crate::delete_agent_prose::BoxState::Ticked,
            )
            .unwrap();
        assert_eq!(deleted, vec!["drifted", "agent"]);
        assert_eq!(count(lost), 4);
        assert_eq!(
            offer.lost(
                crate::delete_agent_prose::BoxState::Unticked,
                crate::delete_agent_prose::BoxState::Unticked
            ),
            None
        );
    }

    /// A different branch put under a recorded name by plumbing has no
    /// creation entry, and the base commit every branch holds is no proof it
    /// is the same branch: it is not offered, and a delete says dux cannot
    /// confirm it.
    #[test]
    fn a_branch_whose_reflog_cannot_say_is_not_taken_on_a_shared_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, wt) = repo_and_worktree(tmp.path());
        git(&wt, &["commit", "--allow-empty", "-m", "on agent"]);
        git(&repo, &["branch", "spike", "refs/heads/agent"]);
        let recorded = branch_creation_entry(&repo, "spike").unwrap().unwrap();
        let tip = oid(&repo, "spike");
        git(&repo, &["branch", "-D", "spike"]);
        // Plumbing with no message: no creation entry, and a tip that does not
        // descend from the recorded one, though both hold the base commit.
        git(
            &repo,
            &["update-ref", "refs/heads/spike", "refs/heads/main"],
        );
        let spike = BranchHistoryEntry {
            identity: Some(recorded),
            tip: Some(tip),
            ..entry("spike")
        };
        assert_eq!(sameness(&repo, &spike), SameBranch::Unconfirmed);
        let offer = delete_branch_offer(&inputs(
            &repo,
            &wt,
            "agent",
            BranchProvenance::CreatedByDux,
            vec![birth("agent"), spike.clone()],
            &[],
        ));
        assert!(offer.others.is_empty(), "{:?}", offer.others);
        let plan = BranchPlan {
            keep: vec![PlannedKeep {
                role: BranchRole::Other,
                name: "spike".to_string(),
                because: KeptBecause::NotOffered,
                identity: spike.identity,
                tip: spike.tip,
            }],
            ..BranchPlan::default()
        };
        assert_eq!(
            resolve_plan(&repo, &wt, &plan).keep,
            vec![(
                BranchRole::Other,
                "spike".to_string(),
                KeptBecause::Unconfirmed
            )]
        );
    }

    /// A legacy birth branch made minutes before the agent's record (a big
    /// checkout in between) is dux's; one made after the record never is.
    #[test]
    fn the_legacy_window_reaches_back_minutes_and_never_forward() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, _wt) = repo_and_worktree(tmp.path());
        let made = branch_creation_entry(&repo, "agent").unwrap().unwrap();
        let minutes_later = DateTime::from_timestamp(made.unix + 300, 0).unwrap();
        assert!(birth_is_duxs(
            &repo,
            &birth("agent"),
            None,
            Some(minutes_later)
        ));
        let at_the_edge = DateTime::from_timestamp(made.unix + 600, 0).unwrap();
        assert!(birth_is_duxs(
            &repo,
            &birth("agent"),
            None,
            Some(at_the_edge)
        ));
        let past_the_edge = DateTime::from_timestamp(made.unix + 601, 0).unwrap();
        assert!(!birth_is_duxs(
            &repo,
            &birth("agent"),
            None,
            Some(past_the_edge)
        ));
        let before_it = DateTime::from_timestamp(made.unix - 60, 0).unwrap();
        assert!(!birth_is_duxs(
            &repo,
            &birth("agent"),
            None,
            Some(before_it)
        ));
    }

    /// A branch the second box showed and the user left unticked, which the
    /// worktree switched onto before the delete, is reported as unticked,
    /// not as one the dialog did not offer.
    #[test]
    fn a_branch_keeps_the_reason_of_the_box_that_showed_it() {
        let request = BranchDeleteRequest {
            current: BoxAnswer::Delete(vec![pin("born", A)]),
            others: BoxAnswer::Keep,
            offered: vec![
                (BranchRole::Current, "born".to_string()),
                (BranchRole::Other, "cur".to_string()),
            ],
        };
        let plan = plan_branch_removal(
            "cur",
            &history(),
            BranchProvenance::CreatedByDux,
            None,
            &request,
            &[],
        );
        assert_eq!(planned(&plan), vec![("born", A)]);
        assert_eq!(kept(&plan, "cur"), Some(&KeptBecause::NotTicked));
    }

    // ── the message ─────────────────────────────────────────────────────────

    #[test]
    fn the_message_names_each_branch_with_its_own_fate() {
        let outcome = |role, name: &str, fate| BranchOutcome {
            role,
            name: name.to_string(),
            fate,
        };
        let note = branch_outcomes_note(&[
            outcome(
                BranchRole::Current,
                "cur",
                BranchFate::Kept(KeptBecause::NobodyAsked),
            ),
            outcome(BranchRole::Other, "a", BranchFate::Deleted),
            outcome(BranchRole::Other, "b", BranchFate::Deleted),
            outcome(BranchRole::Other, "c", BranchFate::Moved),
            outcome(
                BranchRole::Other,
                "d",
                BranchFate::Kept(KeptBecause::NotDuxs {
                    provenance: "attached".to_string(),
                }),
            ),
        ])
        .unwrap()
        .to_string();
        assert_eq!(
            note,
            "Its branch \"cur\" was kept: this agent was on it, but nobody was asked whether to \
             delete it. Also deleted \"a\" and \"b\", the other branches the agent was on. \"c\" \
             was kept because it moved after you were shown it; what is on it now is not what \
             the dialog counted. \"d\" existed before this agent and was kept. Delete any of \
             them yourself with git branch -D 'cur', git branch -D 'c' or git branch -D 'd' if \
             you no longer need them."
        );
    }
}
