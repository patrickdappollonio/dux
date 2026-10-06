//! The one registry of operations in flight on a working copy, keyed by its
//! path, and the removals that wait for them.
//!
//! Every operation that writes into a worktree registers the worktree's path
//! here for as long as it runs: a pull, a commit, a push, a branch rename, a
//! recreate, an agent being created there, a startup-command rerun, an editor
//! save or a file upload. Every removal consults it. The two rules:
//!
//! * A removal that finds an operation holding its path WAITS for it, bounded,
//!   on the removal's own worker thread, never on the engine thread. It is never
//!   refused silently: a removal that gives up says what it was waiting for.
//! * Once a removal has been announced for a path, a new operation on that path
//!   is REFUSED out loud ([`HoldRefused`]). The operations already holding it
//!   finish first; nothing new can start writing into a directory that is about
//!   to go, and nothing can recreate it.
//!
//! A second removal of a path that is already being removed JOINS the first
//! ([`RemovalClaim::Join`]) and reports its outcome instead of running git twice.
//!
//! A branch rename that lands while a removal of its worktree is pending is
//! recorded here, so the removal deletes the branch by its CURRENT name rather
//! than the name it captured when the delete began ([`RemovalLease::renamed`]).
//!
//! Shared by handle (`Clone` is an `Arc` clone) between the engine, the workers
//! it spawns and the web routes, because the web's editor and git routes run
//! outside the engine thread and still have to register.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::engine::{InFlightKey, RemovedBranches};

/// What is holding a worktree. Each kind names itself in the sentence a waiting
/// removal shows, so the user is told what dux is waiting for.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
pub enum WorktreeOpKind {
    Pull,
    Commit,
    Push,
    BranchRename,
    RecreateWorkingCopy,
    CreateAgent,
    StartupCommand,
    EditorWrite,
    Upload,
    GitChange,
    AddProject,
    CloneRepository,
}

impl WorktreeOpKind {
    /// The noun phrase a waiting status uses: "waiting for {this} to finish".
    pub fn phrase(self) -> &'static str {
        match self {
            Self::Pull => "a pull",
            Self::Commit => "a commit",
            Self::Push => "a push",
            Self::BranchRename => "a branch rename",
            Self::RecreateWorkingCopy => "the working copy being recreated",
            Self::CreateAgent => "an agent being created in it",
            Self::StartupCommand => "its startup command",
            Self::EditorWrite => "a save from the editor",
            Self::Upload => "a file upload",
            Self::GitChange => "a change to its files from the changes pane",
            Self::AddProject => "a project being added there",
            Self::CloneRepository => "a repository being cloned there",
        }
    }
}

/// "a pull and a push", in a stable order, each kind once.
pub fn describe_holders(kinds: &[WorktreeOpKind]) -> String {
    let mut kinds = kinds.to_vec();
    kinds.sort();
    kinds.dedup();
    let phrases: Vec<&str> = kinds.iter().map(|kind| kind.phrase()).collect();
    match phrases.as_slice() {
        [] => "nothing".to_string(),
        [one] => (*one).to_string(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// Who owns a hold that is not an RAII guard: a hold the engine takes on the
/// engine thread and releases when the operation's completion event lands.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum HoldOwner {
    /// Released by `Engine::clear_in_flight` for this key, so every completion
    /// path that already clears the in-flight key releases the hold too.
    InFlight(InFlightKey),
    /// An agent being created, by the id of its create op. Released when the
    /// create's launch reports back, either way.
    CreateOp(String),
}

/// A new operation was refused because the folder is being removed, or a
/// delete or move of it is running.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HoldRefused {
    pub path: PathBuf,
    /// `None` for a worktree removal; for a destructive file operation, what
    /// it is ("an editor delete or move").
    pub by: Option<&'static str>,
}

impl HoldRefused {
    /// "dux is removing the worktree at X, so it did not {what}.", or, for a
    /// delete or move running there, "{that} of X is running, so dux did not
    /// {what}.". Each caller says what it was asked to do.
    pub fn sentence(&self, what: &str) -> crate::status_text::StatusText {
        match self.by {
            None => crate::status_text![
                "dux is removing the worktree at ",
                n(crate::home_path::shorten_home(&self.path)),
                format!(
                    ", so it did not {what}. The agent that owned it was deleted; nothing \
                     new can start in that folder while it goes."
                )
            ],
            Some(by) => crate::status_text![
                capitalize(by),
                " of ",
                n(crate::home_path::shorten_home(&self.path)),
                format!(" is running, so dux did not {what}. Try again once it has finished.")
            ],
        }
    }
}

impl std::fmt::Display for HoldRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.by {
            None => write!(
                f,
                "dux is removing the worktree at {}, so nothing new can start in it",
                crate::home_path::shorten_home(&self.path)
            ),
            Some(by) => write!(
                f,
                "{by} of {} is running, so nothing new can start in it",
                crate::home_path::shorten_home(&self.path)
            ),
        }
    }
}

/// `text` with its first letter in upper case.
fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

impl std::error::Error for HoldRefused {}

/// What a finished removal reports to the removals that joined it.
pub type RemovalResult = Result<RemovedBranches, String>;

/// The shared slot a leading removal writes its outcome into and every joiner
/// waits on.
#[derive(Default)]
struct OutcomeSlot {
    outcome: Mutex<Option<RemovalResult>>,
    ready: Condvar,
}

struct Removal {
    id: u64,
    slot: Arc<OutcomeSlot>,
    /// Renames recorded while this removal was pending: old name to new name.
    renames: Vec<(String, String)>,
}

/// A path as the registry compares it: both of its spellings (see
/// [`spellings`]), or, for a destructive claim on a symbolic link, the
/// lexical one only (deleting or moving a link leaves its target alone).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Spelled {
    lexical: PathBuf,
    canonical: PathBuf,
    lexical_only: bool,
}

impl Spelled {
    fn of(path: &Path) -> Self {
        Self {
            lexical: lexical_key(path),
            canonical: path_key(path),
            lexical_only: false,
        }
    }

    /// The forms this path is compared under.
    fn forms(&self) -> impl Iterator<Item = &PathBuf> {
        std::iter::once(&self.lexical).chain((!self.lexical_only).then_some(&self.canonical))
    }

    /// THE registry comparison, the same rule as [`folder_contains`]: some
    /// spelling of `inner` is some spelling of `self` or under it.
    fn contains(&self, inner: &Spelled) -> bool {
        inner
            .forms()
            .any(|inner| self.forms().any(|outer| spelled_under(inner, outer)))
    }

    /// Whether the two name the same folder (each contains the other).
    fn same(&self, other: &Spelled) -> bool {
        self.contains(other) && other.contains(self)
    }
}

struct PathEntry {
    /// The path this entry is for, both spellings.
    spelled: Spelled,
    /// RAII holds by id, and owner-keyed holds by owner.
    guarded: HashMap<u64, WorktreeOpKind>,
    owned: HashMap<HoldOwner, WorktreeOpKind>,
    removal: Option<Removal>,
    /// A destructive file operation's claim (an editor or changes-pane delete
    /// or move): its id, what it is, and what it covers (a link's claim
    /// covers the link alone). A different kind from a removal: a removal
    /// never joins one, it waits for it.
    destructive: Option<(u64, &'static str, Spelled)>,
}

impl PathEntry {
    fn new(spelled: Spelled) -> Self {
        Self {
            spelled,
            guarded: HashMap::new(),
            owned: HashMap::new(),
            removal: None,
            destructive: None,
        }
    }
}

impl PathEntry {
    fn kinds(&self) -> Vec<WorktreeOpKind> {
        self.guarded
            .values()
            .chain(self.owned.values())
            .copied()
            .collect()
    }

    fn is_idle(&self) -> bool {
        self.guarded.is_empty()
            && self.owned.is_empty()
            && self.removal.is_none()
            && self.destructive.is_none()
    }
}

#[derive(Default)]
struct State {
    paths: HashMap<PathBuf, PathEntry>,
    /// Owner-keyed holds point back at their path so a release by owner needs
    /// no path.
    owners: HashMap<HoldOwner, PathBuf>,
    next_id: u64,
}

impl State {
    /// A removal, or a destructive file operation, claimed on `path` or on
    /// any folder containing it, under any spelling: the refusal for
    /// anything new there.
    fn removal_covering(&self, path: &Spelled) -> Option<HoldRefused> {
        self.paths.values().find_map(|entry| {
            if entry.removal.is_some() && entry.spelled.contains(path) {
                return Some(HoldRefused {
                    path: entry.spelled.lexical.clone(),
                    by: None,
                });
            }
            entry
                .destructive
                .as_ref()
                .filter(|(_, _, claimed)| claimed.contains(path))
                .map(|(_, by, claimed)| HoldRefused {
                    path: claimed.lexical.clone(),
                    by: Some(*by),
                })
        })
    }

    /// What an older removal or destructive claim overlapping `path` is, in
    /// words, when one is (see [`Self::overlapping_older`]).
    fn overlapping_older_described(&self, path: &Spelled, id: u64) -> Option<String> {
        self.paths.values().find_map(|entry| {
            if entry.removal.as_ref().is_some_and(|removal| {
                removal.id < id
                    && !entry.spelled.same(path)
                    && (entry.spelled.contains(path) || path.contains(&entry.spelled))
            }) {
                return Some(format!(
                    "dux is still removing the worktree at {}",
                    crate::home_path::shorten_home(&entry.spelled.lexical)
                ));
            }
            entry
                .destructive
                .as_ref()
                .filter(|(claim, _, claimed)| {
                    *claim < id && (claimed.contains(path) || path.contains(claimed))
                })
                .map(|(_, by, claimed)| {
                    format!(
                        "{by} of {} is still running",
                        crate::home_path::shorten_home(&claimed.lexical)
                    )
                })
        })
    }

    /// Whether a removal or destructive claim older than `id` is on a folder
    /// that contains `path` or that `path` contains, under any spelling:
    /// two of them on nested folders never run at the same time, the later
    /// one waits. A removal of the SAME folder is not counted (it is joined
    /// instead), but a destructive claim on the same folder is.
    fn overlapping_older(&self, path: &Spelled, id: u64) -> bool {
        self.overlapping_older_described(path, id).is_some()
    }

    /// Every operation holding `path` or any folder inside it, under any
    /// spelling.
    fn kinds_within(&self, path: &Spelled) -> Vec<WorktreeOpKind> {
        self.paths
            .values()
            .filter(|entry| path.contains(&entry.spelled))
            .flat_map(|entry| entry.kinds())
            .collect()
    }

    /// The entry for `path` (keyed by its lexical spelling), made if new.
    fn entry_for(&mut self, path: &Spelled) -> &mut PathEntry {
        self.paths
            .entry(path.lexical.clone())
            .or_insert_with(|| PathEntry::new(path.clone()))
    }

    fn mint(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn tidy(&mut self, key: &Path) {
        if self.paths.get(key).is_some_and(PathEntry::is_idle) {
            self.paths.remove(key);
        }
    }
}

#[derive(Default)]
struct Inner {
    state: Mutex<State>,
    /// Notified whenever a hold is released or a removal finishes.
    changed: Condvar,
}

/// The registry. `Clone` shares it.
#[derive(Clone, Default)]
pub struct WorktreeOps {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for WorktreeOps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorktreeOps").finish_non_exhaustive()
    }
}

/// The key a path is registered under: the nearest ancestor that exists,
/// canonicalized, with the rest of the path appended as spelled. So a key built
/// before a folder exists (an agent create holding the path of a worktree it is
/// about to make) is the key built after it exists and after it is gone, even
/// when the path reaches it through a symlink (a dotfiles-managed config
/// folder, say). The ONE key builder: every occupancy question compares these.
pub fn path_key(path: &Path) -> PathBuf {
    let mut rest: Vec<&std::ffi::OsStr> = Vec::new();
    let mut existing = path;
    loop {
        if let Ok(canonical) = existing.canonicalize() {
            let mut key = canonical;
            for component in rest.iter().rev() {
                key.push(component);
            }
            return key;
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name);
                existing = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// The LEXICAL key of a path: as written, normalized, with its parent
/// resolved (like [`path_key`]) and its own last component kept, never
/// followed. For a path whose last component is not a symbolic link it is
/// the same as [`path_key`]; for a link it names the link itself, not where
/// it leads. The registry keys holds and claims by this, so a delete or move
/// of a link claims the link, never its target.
pub fn lexical_key(path: &Path) -> PathBuf {
    let normalized: PathBuf = path.components().collect();
    match (normalized.parent(), normalized.file_name()) {
        (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => path_key(parent).join(name),
        _ => path_key(&normalized),
    }
}

/// Every spelling a recorded path stands for: LEXICAL (as recorded, its
/// parent resolved, its last component kept) and CANONICAL (fully
/// resolved). They differ only for a symbolic link.
pub fn spellings(path: &Path) -> Vec<PathBuf> {
    let lexical = lexical_key(path);
    let canonical = path_key(path);
    if lexical == canonical {
        vec![lexical]
    } else {
        vec![lexical, canonical]
    }
}

/// Whether `inner` is the folder `outer` or anywhere inside it, under every
/// spelling of both (see [`spellings`]): an occupant recorded at a link
/// inside the folder is inside it however far the link points, and one
/// recorded at a link outside it whose target is inside is inside it too.
/// Compared by path COMPONENTS, never by string prefix, so `/w/agent` does
/// not contain `/w/agent-two`. The ONE containment test: a removal of a
/// folder deletes everything inside it, so every question about what
/// occupies a folder (a hold, a claim, a create, a process, a working
/// directory, the manager's busy state, the last look before git) asks this.
pub fn folder_contains(outer: &Path, inner: &Path) -> bool {
    let outers = spellings(outer);
    spellings(inner)
        .iter()
        .any(|inner| outers.iter().any(|outer| spelled_under(inner, outer)))
}

/// Whether `inner` is `outer` or under it, component by component. On macOS
/// the comparison ignores case, because APFS (and HFS+) are case-insensitive
/// by default: `/Users/me/Work` and `/users/me/work` name one folder there,
/// and a lexical spelling (never resolved by the filesystem) keeps whatever
/// case it was recorded in. On Linux the filesystem is case-sensitive and
/// so is the comparison.
pub fn spelled_under(inner: &Path, outer: &Path) -> bool {
    under_with_case(inner, outer, cfg!(target_os = "macos"))
}

/// Whether two spellings name the same path, under the same comparison as
/// [`spelled_under`] (without case on macOS).
pub fn spelled_same(a: &Path, b: &Path) -> bool {
    spelled_under(a, b) && spelled_under(b, a)
}

/// A key two spellings of one relative path share under the same comparison
/// as [`spelled_same`]: case folded on macOS, as written elsewhere. For
/// comparing many names read from disk against paths git reports.
pub fn case_key(path: &Path) -> PathBuf {
    case_key_with(path, cfg!(target_os = "macos"))
}

/// [`case_key`] with the case rule spelled out, so it can be tested on either
/// platform.
pub fn case_key_with(path: &Path, ignore_case: bool) -> PathBuf {
    if !ignore_case {
        return path.to_path_buf();
    }
    path.components()
        .map(|part| part.as_os_str().to_string_lossy().to_lowercase())
        .collect()
}

fn under_with_case(inner: &Path, outer: &Path, ignore_case: bool) -> bool {
    if !ignore_case {
        return inner.starts_with(outer);
    }
    let mut inner = inner.components();
    for outer in outer.components() {
        match inner.next() {
            Some(inner)
                if inner.as_os_str().to_string_lossy().to_lowercase()
                    == outer.as_os_str().to_string_lossy().to_lowercase() => {}
            _ => return false,
        }
    }
    true
}

impl WorktreeOps {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Register an operation on `path` for as long as the returned guard lives.
    /// Refused once a removal of the path has been announced.
    #[must_use = "a refused hold or claim must be answered, never dropped"]
    pub fn hold(
        &self,
        path: impl AsRef<Path>,
        kind: WorktreeOpKind,
    ) -> Result<WorktreeOpGuard, HoldRefused> {
        let spelled = Spelled::of(path.as_ref());
        let mut state = self.lock();
        if let Some(refused) = state.removal_covering(&spelled) {
            return Err(refused);
        }
        let id = state.mint();
        state.entry_for(&spelled).guarded.insert(id, kind);
        Ok(WorktreeOpGuard {
            ops: self.clone(),
            key: spelled.lexical,
            id,
        })
    }

    /// Register an operation on `path` that `owner` releases later with
    /// [`Self::release_owner`]. Taking a second hold for an owner that already
    /// holds one replaces it.
    #[must_use = "a refused hold or claim must be answered, never dropped"]
    pub fn hold_as(
        &self,
        owner: HoldOwner,
        path: impl AsRef<Path>,
        kind: WorktreeOpKind,
    ) -> Result<(), HoldRefused> {
        let spelled = Spelled::of(path.as_ref());
        let mut state = self.lock();
        if let Some(refused) = state.removal_covering(&spelled) {
            return Err(refused);
        }
        Self::release_owner_locked(&mut state, &owner);
        state.entry_for(&spelled).owned.insert(owner.clone(), kind);
        state.owners.insert(owner, spelled.lexical);
        Ok(())
    }

    fn release_owner_locked(state: &mut State, owner: &HoldOwner) -> bool {
        let Some(key) = state.owners.remove(owner) else {
            return false;
        };
        if let Some(entry) = state.paths.get_mut(&key) {
            entry.owned.remove(owner);
        }
        state.tidy(&key);
        true
    }

    /// Release the hold `owner` took, when it took one. Cheap when it did not.
    pub fn release_owner(&self, owner: &HoldOwner) {
        let released = Self::release_owner_locked(&mut self.lock(), owner);
        if released {
            self.inner.changed.notify_all();
        }
    }

    /// The path `owner` holds, when it holds one.
    pub fn owner_path(&self, owner: &HoldOwner) -> Option<PathBuf> {
        self.lock().owners.get(owner).cloned()
    }

    /// Record that `old` was renamed to `new` in the worktree `owner` holds.
    /// Kept only while a removal of that worktree is pending, because that
    /// removal captured the old name and is the only reader.
    pub fn record_branch_rename(&self, owner: &HoldOwner, old: &str, new: &str) {
        let mut state = self.lock();
        let Some(key) = state.owners.get(owner).cloned() else {
            return;
        };
        if let Some(removal) = state
            .paths
            .get_mut(&key)
            .and_then(|entry| entry.removal.as_mut())
        {
            removal.renames.push((old.to_string(), new.to_string()));
        }
    }

    /// The operations holding `path`, or any folder inside it, right now: a
    /// removal of `path` deletes those folders too.
    pub fn holders(&self, path: impl AsRef<Path>) -> Vec<WorktreeOpKind> {
        self.lock().kinds_within(&Spelled::of(path.as_ref()))
    }

    /// Every path an operation holds that `pick` accepts, with what holds it.
    pub fn holders_where(
        &self,
        pick: &dyn Fn(&Path) -> bool,
    ) -> Vec<(PathBuf, Vec<WorktreeOpKind>)> {
        self.lock()
            .paths
            .values()
            .filter(|entry| pick(&entry.spelled.lexical))
            .map(|entry| (entry.spelled.lexical.clone(), entry.kinds()))
            .filter(|(_, kinds)| !kinds.is_empty())
            .collect()
    }

    /// The refusal for starting something in `path` because a removal of it,
    /// or of a folder containing it, is under way; names the folder being
    /// removed. `None` when nothing covers it.
    pub fn removal_refusal(&self, path: impl AsRef<Path>) -> Option<HoldRefused> {
        self.lock().removal_covering(&Spelled::of(path.as_ref()))
    }

    /// Whether a removal of `path`, or of a folder containing it, has been
    /// announced and not yet finished.
    pub fn is_being_removed(&self, path: impl AsRef<Path>) -> bool {
        self.lock()
            .removal_covering(&Spelled::of(path.as_ref()))
            .is_some()
    }

    /// Announce a removal of `path`. From now on new holds on it are refused.
    /// The first announcement leads; one made while another is unfinished joins
    /// it.
    #[must_use = "a refused hold or claim must be answered, never dropped"]
    pub fn announce_removal(&self, path: impl AsRef<Path>) -> RemovalClaim {
        let spelled = Spelled::of(path.as_ref());
        let mut state = self.lock();
        // A removal of the same folder, under either spelling, is joined.
        if let Some((key, removal)) = state.paths.iter().find_map(|(key, entry)| {
            entry
                .removal
                .as_ref()
                .filter(|_| entry.spelled.same(&spelled))
                .map(|removal| (key.clone(), removal))
        }) {
            return RemovalClaim::Join(RemovalJoin {
                key,
                slot: Arc::clone(&removal.slot),
            });
        }
        let id = state.mint();
        let slot = Arc::new(OutcomeSlot::default());
        state.entry_for(&spelled).removal = Some(Removal {
            id,
            slot: Arc::clone(&slot),
            renames: Vec::new(),
        });
        RemovalClaim::Lead(RemovalLease {
            ops: self.clone(),
            key: spelled.lexical.clone(),
            spelled,
            id,
            slot,
            finished: false,
        })
    }
}

impl WorktreeOps {
    /// Claim `path` for a destructive file operation (an editor delete or
    /// move of a folder), the same claim a removal takes: from now on nothing
    /// new can start in it or anywhere inside it, so nothing lands there
    /// between the occupancy check and the operation. Refused, with the
    /// reason, when a removal already covers it or an operation is already
    /// running inside it. Dropping the lease lets the folder go.
    #[must_use = "a refused hold or claim must be answered, never dropped"]
    pub fn claim_for_destructive(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<DestructiveClaim, String> {
        self.claim_for_destructive_within(path, DESTRUCTIVE_CLAIM_WAIT)
    }

    /// [`Self::claim_for_destructive_within`], saying what the operation is
    /// ("an editor delete or move", "a changes-pane delete"), so anything it
    /// refuses meanwhile names it truthfully.
    #[must_use = "a refused hold or claim must be answered, never dropped"]
    pub fn claim_for_destructive_as(
        &self,
        path: impl AsRef<Path>,
        wait: Duration,
        by: &'static str,
    ) -> Result<DestructiveClaim, String> {
        self.claim_destructive(path.as_ref(), wait, by)
    }

    /// [`Self::claim_for_destructive`], waiting at most `wait` for a removal
    /// running in or under the folder: the same ordering two removals of
    /// nested folders keep, so a delete or move of a folder never runs while
    /// dux is removing a worktree inside it. Refused with a sentence when the
    /// wait runs out. Blocking: never on the engine thread or an async task.
    #[must_use = "a refused hold or claim must be answered, never dropped"]
    pub fn claim_for_destructive_within(
        &self,
        path: impl AsRef<Path>,
        wait: Duration,
    ) -> Result<DestructiveClaim, String> {
        self.claim_destructive(path.as_ref(), wait, "a delete or move")
    }

    fn claim_destructive(
        &self,
        path: &Path,
        wait: Duration,
        by: &'static str,
    ) -> Result<DestructiveClaim, String> {
        // A claim on a link covers the link alone: deleting or moving one
        // leaves its target where it is.
        let mut spelled = Spelled::of(path);
        spelled.lexical_only =
            std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink());
        let claim = {
            let mut state = self.lock();
            if let Some(covering) = state.removal_covering(&spelled) {
                return Err(match covering.by {
                    None => format!(
                        "dux is removing the worktree at {}",
                        crate::home_path::shorten_home(&covering.path)
                    ),
                    Some(by) => format!(
                        "{by} of {} is running",
                        crate::home_path::shorten_home(&covering.path)
                    ),
                });
            }
            let id = state.mint();
            let mut entry_spelling = spelled.clone();
            entry_spelling.lexical_only = false;
            state.entry_for(&entry_spelling).destructive = Some((id, by, spelled.clone()));
            DestructiveClaim {
                ops: self.clone(),
                key: spelled.lexical.clone(),
                spelled,
                id,
            }
        };
        let holders = self.lock().kinds_within(&claim.spelled);
        if !holders.is_empty() {
            return Err(format!("{} is running in it", describe_holders(&holders)));
        }
        if !self.wait_for_older_overlapping(&claim.spelled, claim.id, wait) {
            let what = self
                .lock()
                .overlapping_older_described(&claim.spelled, claim.id)
                .unwrap_or_else(|| "another delete or removal there is still running".to_string());
            return Err(format!(
                "{what} after {} seconds; try again once it has finished",
                wait.as_secs()
            ));
        }
        Ok(claim)
    }

    /// Block until no removal or destructive claim older than `id` overlaps
    /// `key` (see `State::overlapping_older`), or `timeout` passes (`false`).
    fn wait_for_older_overlapping(&self, path: &Spelled, id: u64, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = self.lock();
        loop {
            if !state.overlapping_older(path, id) {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            state = self
                .inner
                .changed
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
    }
}

/// How long a destructive file operation waits for a removal running inside
/// its folder before it is refused.
pub const DESTRUCTIVE_CLAIM_WAIT: Duration = Duration::from_secs(5);

/// A destructive file operation's claim on a folder (see
/// [`WorktreeOps::claim_for_destructive`]). While it lives nothing new starts
/// in the folder; dropping it lets the folder go. Only this registry makes
/// one, and only a claim lets a delete or move be cleared
/// ([`crate::destructive::DestructiveCheck::clear`]).
pub struct DestructiveClaim {
    ops: WorktreeOps,
    key: PathBuf,
    spelled: Spelled,
    id: u64,
}

impl DestructiveClaim {
    /// The folder claimed, as a path key.
    pub fn path(&self) -> &Path {
        &self.key
    }

    /// Whether this claim is on exactly `path`.
    pub fn covers(&self, path: &Path) -> bool {
        spelled_same(&lexical_key(path), &self.key)
    }
}

impl std::fmt::Debug for DestructiveClaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DestructiveClaim")
            .field("path", &self.key)
            .finish()
    }
}

impl Drop for DestructiveClaim {
    fn drop(&mut self) {
        {
            let mut state = self.ops.lock();
            if let Some(entry) = state.paths.get_mut(&self.key)
                && entry
                    .destructive
                    .as_ref()
                    .is_some_and(|(id, _, _)| *id == self.id)
            {
                entry.destructive = None;
            }
            state.tidy(&self.key);
        }
        self.ops.inner.changed.notify_all();
    }
}

/// An RAII hold. Dropping it releases the path, panics included.
pub struct WorktreeOpGuard {
    ops: WorktreeOps,
    key: PathBuf,
    id: u64,
}

impl std::fmt::Debug for WorktreeOpGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorktreeOpGuard")
            .field("path", &self.key)
            .finish()
    }
}

impl Drop for WorktreeOpGuard {
    fn drop(&mut self) {
        {
            let mut state = self.ops.lock();
            if let Some(entry) = state.paths.get_mut(&self.key) {
                entry.guarded.remove(&self.id);
            }
            state.tidy(&self.key);
        }
        self.ops.inner.changed.notify_all();
    }
}

/// What announcing a removal gave the caller.
pub enum RemovalClaim {
    /// This removal runs git.
    Lead(RemovalLease),
    /// Another removal of the same path is running; wait for its outcome.
    Join(RemovalJoin),
}

impl RemovalClaim {
    pub fn path(&self) -> &Path {
        match self {
            Self::Lead(lease) => &lease.key,
            Self::Join(join) => &join.key,
        }
    }
}

impl std::fmt::Debug for RemovalClaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lead(lease) => f.debug_tuple("Lead").field(&lease.key).finish(),
            Self::Join(join) => f.debug_tuple("Join").field(&join.key).finish(),
        }
    }
}

/// The right to remove a path. Dropping it unfinished (a panic, a removal that
/// decided to keep the worktree) withdraws the announcement, so the path is
/// usable again and any joiner is told nothing was removed.
pub struct RemovalLease {
    ops: WorktreeOps,
    key: PathBuf,
    spelled: Spelled,
    id: u64,
    slot: Arc<OutcomeSlot>,
    finished: bool,
}

impl RemovalLease {
    pub fn path(&self) -> &Path {
        &self.key
    }

    /// The registry this lease was taken in.
    pub fn ops(&self) -> &WorktreeOps {
        &self.ops
    }

    /// The operations still holding the path.
    pub fn holders(&self) -> Vec<WorktreeOpKind> {
        self.ops.lock().kinds_within(&self.spelled)
    }

    /// Block until nothing holds the path, or `timeout` passes. On a timeout,
    /// answers with what was still holding it. Run it on a worker thread only.
    pub fn wait_for_holders(&self, timeout: Duration) -> Result<(), Vec<WorktreeOpKind>> {
        let deadline = Instant::now() + timeout;
        let mut state = self.ops.lock();
        loop {
            let kinds = state.kinds_within(&self.spelled);
            if kinds.is_empty() {
                return Ok(());
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(kinds);
            }
            state = self
                .ops
                .inner
                .changed
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
    }

    /// Block until no OTHER removal of a folder that contains this one, or
    /// that this one contains, is running, or `timeout` passes (`false`). Two
    /// such removals must never run git at the same time: the outer one
    /// deletes the inner folder too. The later one waits; afterwards its own
    /// folder may already be gone, which its git step answers by forgetting
    /// the registration and nothing more. Run it on a worker thread only.
    pub fn wait_for_overlapping_removals(&self, timeout: Duration) -> bool {
        self.ops
            .wait_for_older_overlapping(&self.spelled, self.id, timeout)
    }

    /// `branch` as it is called now, following every rename recorded while
    /// this removal was pending.
    pub fn renamed(&self, branch: &str) -> String {
        let state = self.ops.lock();
        let renames = state
            .paths
            .get(&self.key)
            .and_then(|entry| entry.removal.as_ref())
            .filter(|removal| removal.id == self.id)
            .map(|removal| removal.renames.clone())
            .unwrap_or_default();
        drop(state);
        let mut current = branch.to_string();
        for (old, new) in renames {
            if current == old {
                current = new;
            }
        }
        current
    }

    /// Finish the removal, handing its outcome to every joiner.
    pub fn finish(mut self, outcome: RemovalResult) {
        self.finished = true;
        self.close(outcome);
    }

    fn close(&mut self, outcome: RemovalResult) {
        {
            let mut state = self.ops.lock();
            if let Some(entry) = state.paths.get_mut(&self.key)
                && entry.removal.as_ref().is_some_and(|r| r.id == self.id)
            {
                entry.removal = None;
            }
            state.tidy(&self.key);
        }
        *self
            .slot
            .outcome
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(outcome);
        self.slot.ready.notify_all();
        self.ops.inner.changed.notify_all();
    }
}

impl Drop for RemovalLease {
    fn drop(&mut self) {
        if !self.finished {
            self.finished = true;
            self.close(Err(format!(
                "the removal of {} stopped before it ran",
                crate::home_path::shorten_home(&self.key)
            )));
        }
    }
}

/// A removal that joined one already running for the same path.
pub struct RemovalJoin {
    key: PathBuf,
    slot: Arc<OutcomeSlot>,
}

impl RemovalJoin {
    pub fn path(&self) -> &Path {
        &self.key
    }

    /// Block until the leading removal finishes and take its outcome, or
    /// `None` when `timeout` passes first. Run it on a worker thread only.
    pub fn wait(self, timeout: Duration) -> Option<RemovalResult> {
        let deadline = Instant::now() + timeout;
        let mut outcome = self
            .slot
            .outcome
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            if let Some(result) = outcome.as_ref() {
                return Some(result.clone());
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            outcome = self
                .slot
                .ready
                .wait_timeout(outcome, deadline - now)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// macOS compares spellings without case (APFS is case-insensitive by
    /// default); Linux compares them exactly. Both rules are pinned here, on
    /// every platform, through the helper the containment rule uses.
    #[test]
    fn spellings_compare_without_case_where_the_filesystem_does() {
        let outer = Path::new("/Users/Me/Work");
        let inner = Path::new("/users/me/work/agent");
        assert!(under_with_case(inner, outer, true));
        assert!(!under_with_case(inner, outer, false));
        assert!(!under_with_case(
            Path::new("/users/me/workshop"),
            outer,
            true
        ));
        assert!(under_with_case(
            Path::new("/Users/Me/Work/agent"),
            outer,
            false
        ));
        assert_eq!(spelled_under(inner, outer), cfg!(target_os = "macos"));
    }

    /// A link inside a folder that points outside it is inside the folder
    /// (its lexical spelling), and a link outside it that points inside is
    /// inside too (its canonical spelling).
    #[test]
    fn containment_holds_under_either_spelling_of_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("wt");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(folder.join("inner")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let link_in = folder.join("link-out");
        std::os::unix::fs::symlink(&outside, &link_in).unwrap();
        let link_out = dir.path().join("link-in");
        std::os::unix::fs::symlink(folder.join("inner"), &link_out).unwrap();
        assert!(
            folder_contains(&folder, &link_in),
            "recorded at a link inside"
        );
        assert!(
            folder_contains(&folder, &link_out),
            "recorded at a link to inside"
        );
        assert!(
            !folder_contains(&folder, &outside),
            "the outside folder itself"
        );
    }

    /// A delete or move's claim is keyed on a link's own path, and what it
    /// refuses meanwhile names it truthfully.
    #[test]
    fn a_destructive_claim_names_itself_in_what_it_refuses() {
        let ops = WorktreeOps::new();
        let _claim = ops
            .claim_for_destructive_as("/work/folder", Duration::ZERO, "an editor delete or move")
            .unwrap();
        let refused = ops
            .hold("/work/folder/inner", WorktreeOpKind::EditorWrite)
            .unwrap_err();
        let sentence = refused.sentence("save the file").to_string();
        assert!(
            sentence.starts_with("An editor delete or move of "),
            "{sentence}"
        );
        assert!(!sentence.contains("removing the worktree"), "{sentence}");
        let claimed = ops
            .claim_for_destructive_within("/work/folder/inner", Duration::ZERO)
            .unwrap_err();
        assert!(claimed.contains("an editor delete or move of"), "{claimed}");
    }

    /// A removal never joins a destructive claim: it leads, and waits for
    /// the claim (bounded) under the nested-ordering rule.
    #[test]
    fn a_removal_waits_for_a_destructive_claim_instead_of_joining_it() {
        let ops = WorktreeOps::new();
        let claim = ops.claim_for_destructive("/work/agent").unwrap();
        let RemovalClaim::Lead(lease) = ops.announce_removal("/work/agent") else {
            panic!("a removal leads; it never joins a destructive claim");
        };
        assert!(!lease.wait_for_overlapping_removals(Duration::from_millis(20)));
        drop(claim);
        assert!(lease.wait_for_overlapping_removals(Duration::from_millis(20)));
    }

    #[test]
    fn a_destructive_claim_keeps_everything_out_of_the_folder_until_it_ends() {
        let ops = WorktreeOps::new();
        let lease = ops.claim_for_destructive("/work/folder").unwrap();
        assert!(
            ops.hold("/work/folder/inner", WorktreeOpKind::EditorWrite)
                .is_err()
        );
        assert!(ops.claim_for_destructive("/work/folder").is_err());
        drop(lease);
        let hold = ops
            .hold("/work/folder/inner", WorktreeOpKind::EditorWrite)
            .unwrap();
        let Err(refused) = ops.claim_for_destructive("/work/folder") else {
            panic!("a folder something is running in cannot be claimed");
        };
        assert!(refused.contains("running in it"), "{refused}");
        drop(hold);
    }
    use crate::model::BranchKeptReason;

    fn kept() -> RemovalResult {
        Ok(RemovedBranches::Kept(BranchKeptReason::UserDeclined))
    }

    #[test]
    fn containment_is_by_components_not_by_text() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path().join("agent");
        assert!(folder_contains(&wt, &wt));
        assert!(folder_contains(&wt, &wt.join("frontend").join("src")));
        assert!(!folder_contains(&wt, &dir.path().join("agent-two")));
        assert!(!folder_contains(&wt.join("frontend"), &wt));
    }

    #[test]
    fn a_key_built_before_the_folder_exists_matches_one_built_after() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let through_link = link.join("project").join("fresh");
        let before = path_key(&through_link);
        std::fs::create_dir_all(real.join("project").join("fresh")).unwrap();
        assert_eq!(before, path_key(&through_link));
        assert_eq!(before, path_key(&real.join("project").join("fresh")));
    }

    #[test]
    fn a_removal_covers_everything_inside_its_folder() {
        let ops = WorktreeOps::new();
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path().join("wt");
        let inside = wt.join("frontend");
        let _editor = ops.hold(&inside, WorktreeOpKind::EditorWrite).unwrap();
        assert_eq!(ops.holders(&wt), vec![WorktreeOpKind::EditorWrite]);
        let _lease = ops.announce_removal(&wt);
        assert!(ops.is_being_removed(&inside));
        let refused = ops.hold(&inside, WorktreeOpKind::Upload).unwrap_err();
        assert_eq!(
            refused.path,
            path_key(&wt),
            "names the folder being removed"
        );
        assert!(
            ops.hold(dir.path().join("wt-two"), WorktreeOpKind::Upload)
                .is_ok()
        );
    }

    /// Removals of `/a` and `/a/c` never run git at the same time: the later
    /// one waits for the earlier one to finish, whichever is the outer.
    #[test]
    fn a_removal_waits_for_an_earlier_removal_of_a_folder_around_or_inside_it() {
        let ops = WorktreeOps::new();
        let dir = tempfile::tempdir().unwrap();
        let outer = dir.path().join("a");
        let inner = outer.join("c");
        let RemovalClaim::Lead(first) = ops.announce_removal(&outer) else {
            panic!("leads");
        };
        let RemovalClaim::Lead(second) = ops.announce_removal(&inner) else {
            panic!("a different folder leads its own removal");
        };
        assert!(
            first.wait_for_overlapping_removals(Duration::ZERO),
            "the earlier one goes first"
        );
        assert!(
            !second.wait_for_overlapping_removals(Duration::from_millis(50)),
            "the later one waits while the earlier one runs"
        );
        let waiter = std::thread::spawn(move || {
            second.wait_for_overlapping_removals(Duration::from_secs(10))
        });
        std::thread::sleep(Duration::from_millis(50));
        first.finish(kept());
        assert!(waiter.join().unwrap(), "and goes once it has finished");
        // An unrelated sibling never waits.
        let RemovalClaim::Lead(_a) = ops.announce_removal(dir.path().join("x")) else {
            panic!()
        };
        let RemovalClaim::Lead(b) = ops.announce_removal(dir.path().join("y")) else {
            panic!()
        };
        assert!(b.wait_for_overlapping_removals(Duration::ZERO));
    }

    #[test]
    fn a_hold_is_refused_once_a_removal_is_announced() {
        let ops = WorktreeOps::new();
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path().join("wt");
        let _lease = ops.announce_removal(&wt);
        let refused = ops.hold(&wt, WorktreeOpKind::EditorWrite).unwrap_err();
        assert_eq!(refused.path, path_key(&wt));
        assert!(
            ops.hold_as(
                HoldOwner::CreateOp("op-1".into()),
                &wt,
                WorktreeOpKind::CreateAgent
            )
            .is_err()
        );
    }

    #[test]
    fn the_key_is_the_same_before_and_after_the_directory_goes() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path().join("wt");
        std::fs::create_dir(&wt).unwrap();
        std::fs::create_dir(dir.path().join("x")).unwrap();
        let before = path_key(&wt);
        std::fs::remove_dir(&wt).unwrap();
        assert_eq!(before, path_key(&wt));
        assert_eq!(
            before,
            path_key(&dir.path().join("x").join("..").join("wt"))
        );
    }

    #[test]
    fn a_removal_waits_for_a_hold_and_wakes_when_it_is_released() {
        let ops = WorktreeOps::new();
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path().join("wt");
        let guard = ops.hold(&wt, WorktreeOpKind::Pull).unwrap();
        let RemovalClaim::Lead(lease) = ops.announce_removal(&wt) else {
            panic!("the first removal leads");
        };
        assert_eq!(lease.holders(), vec![WorktreeOpKind::Pull]);
        let (tx, rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let waited = lease.wait_for_holders(Duration::from_secs(20));
            tx.send(waited.is_ok()).unwrap();
            lease
        });
        assert!(
            rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "the removal must still be waiting while the pull holds the path"
        );
        drop(guard);
        assert!(rx.recv_timeout(Duration::from_secs(20)).unwrap());
        waiter.join().unwrap().finish(kept());
        assert!(!ops.is_being_removed(&wt));
        assert!(ops.hold(&wt, WorktreeOpKind::Pull).is_ok());
    }

    #[test]
    fn a_wait_that_runs_out_names_what_still_holds_the_path() {
        let ops = WorktreeOps::new();
        let wt = PathBuf::from("/nonexistent-dux-test/wt");
        let _guard = ops.hold(&wt, WorktreeOpKind::Push).unwrap();
        let _owned = ops.hold_as(
            HoldOwner::InFlight(InFlightKey::Pull("x".into())),
            &wt,
            WorktreeOpKind::Pull,
        );
        let RemovalClaim::Lead(lease) = ops.announce_removal(&wt) else {
            panic!("leads");
        };
        let still = lease
            .wait_for_holders(Duration::from_millis(20))
            .unwrap_err();
        assert_eq!(describe_holders(&still), "a pull and a push");
    }

    #[test]
    fn an_owned_hold_is_released_by_its_owner() {
        let ops = WorktreeOps::new();
        let wt = PathBuf::from("/nonexistent-dux-test/wt");
        let owner = HoldOwner::InFlight(InFlightKey::BranchRename("s1".into()));
        ops.hold_as(owner.clone(), &wt, WorktreeOpKind::BranchRename)
            .unwrap();
        assert_eq!(ops.holders(&wt), vec![WorktreeOpKind::BranchRename]);
        ops.release_owner(&owner);
        assert!(ops.holders(&wt).is_empty());
    }

    #[test]
    fn a_second_removal_joins_the_first_and_takes_its_outcome() {
        let ops = WorktreeOps::new();
        let wt = PathBuf::from("/nonexistent-dux-test/wt");
        let RemovalClaim::Lead(lease) = ops.announce_removal(&wt) else {
            panic!("leads");
        };
        let RemovalClaim::Join(join) = ops.announce_removal(&wt) else {
            panic!("the second removal of the same path joins");
        };
        let joined = std::thread::spawn(move || join.wait(Duration::from_secs(20)));
        lease.finish(Err("boom".into()));
        assert_eq!(joined.join().unwrap(), Some(Err("boom".into())));
    }

    #[test]
    fn a_dropped_lease_withdraws_the_announcement_and_tells_joiners() {
        let ops = WorktreeOps::new();
        let wt = PathBuf::from("/nonexistent-dux-test/wt");
        let lease = ops.announce_removal(&wt);
        let RemovalClaim::Join(join) = ops.announce_removal(&wt) else {
            panic!("joins");
        };
        drop(lease);
        assert!(!ops.is_being_removed(&wt));
        assert!(matches!(join.wait(Duration::from_secs(1)), Some(Err(_))));
    }

    #[test]
    fn a_rename_recorded_while_a_removal_is_pending_is_followed() {
        let ops = WorktreeOps::new();
        let wt = PathBuf::from("/nonexistent-dux-test/wt");
        let owner = HoldOwner::InFlight(InFlightKey::BranchRename("s1".into()));
        ops.hold_as(owner.clone(), &wt, WorktreeOpKind::BranchRename)
            .unwrap();
        let RemovalClaim::Lead(lease) = ops.announce_removal(&wt) else {
            panic!("leads");
        };
        ops.record_branch_rename(&owner, "old", "new");
        ops.release_owner(&owner);
        assert_eq!(lease.renamed("old"), "new");
        assert_eq!(lease.renamed("other"), "other");
    }

    #[test]
    fn a_rename_with_no_removal_pending_is_not_kept() {
        let ops = WorktreeOps::new();
        let wt = PathBuf::from("/nonexistent-dux-test/wt");
        let owner = HoldOwner::InFlight(InFlightKey::BranchRename("s1".into()));
        ops.hold_as(owner.clone(), &wt, WorktreeOpKind::BranchRename)
            .unwrap();
        ops.record_branch_rename(&owner, "old", "new");
        ops.release_owner(&owner);
        let RemovalClaim::Lead(lease) = ops.announce_removal(&wt) else {
            panic!("leads");
        };
        assert_eq!(lease.renamed("old"), "old");
    }

    /// review9: nested ordering for a destructive claim. A worktree removal of
    /// `/p/x` is running (its agent's processes are gone; git is about to run
    /// or running). An editor delete or move of `/p`, which deletes or moves
    /// `/p/x` with it, must not be granted while that removal runs: two
    /// removals of nested folders never run at the same time.
    #[test]
    fn review9_a_destructive_claim_waits_for_or_refuses_a_removal_inside_it() {
        let ops = WorktreeOps::new();
        let dir = tempfile::tempdir().unwrap();
        let outer = dir.path().join("p");
        let inner = outer.join("x");
        std::fs::create_dir_all(&inner).unwrap();
        let RemovalClaim::Lead(_running) = ops.announce_removal(&inner) else {
            panic!("leads");
        };
        assert!(
            ops.claim_for_destructive(&outer).is_err(),
            "an editor delete/move of a folder around a worktree being removed was cleared to run \
             at the same time as that removal"
        );
    }
}
