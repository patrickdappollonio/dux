//! GitHub CLI (`gh`) integration helpers used by the PR-sync worker
//! (`spawn_pr_sync_worker`, `spawn_initial_pr_refresh`, `spawn_pr_check_for_session`).
//! All helpers shell out to `gh` and parse JSON; no UI deps.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::git;
use crate::logger;
use crate::model::{PrInfo, PrState, Project};
use crate::status_text::StatusText;
use crate::storage::StoredPr;
use crate::worker::{
    BranchAskRecord, PrLookupPurpose, PrStatusResult, PrSyncAnswer, PrSyncBranch, PrSyncEntry,
    PullRequestLookup, ResolvedPullRequest, WorkerEvent,
};

/// Live GraphQL rate-limit snapshot parsed from a batched query's top-level
/// `rateLimit` field. Lets the PR-sync loop back off before it exhausts the
/// GraphQL points budget (typically 5000/hour, higher on GitHub Enterprise Cloud).
#[derive(Clone, Debug)]
pub(crate) struct RateLimitInfo {
    pub remaining: i64,
    pub(crate) reset_at: Option<chrono::DateTime<chrono::Utc>>,
    /// What this one query cost, in GraphQL points. Already asked for, and what
    /// the cycle summary reports: the widened discovery window is a cost
    /// question, so the log has to be able to answer it.
    pub cost: Option<i64>,
}

/// Per-host outcome of a sync cycle, used to drive the per-host backoff. Only
/// hosts that were actually queried this cycle appear (a host skipped because it
/// is already backed off, or one with only zero-network sessions, is absent, so
/// its backoff window is left untouched rather than spuriously cleared).
pub(crate) struct HostSignal {
    pub host: String,
    pub rate: Option<RateLimitInfo>,
    /// A whole-call failure (spawn error / timeout / unparseable-or-null data) on
    /// at least one of this host's chunks.
    pub(crate) hard_failed: bool,
    /// The failure looked like GitHub rate-limiting (a 403 / secondary limit, or a
    /// `RATE_LIMITED` GraphQL error), as opposed to a network/`gh` error. Lets the
    /// status message say so instead of a vague "network or gh error".
    pub rate_limited: bool,
}

/// One sync's answers, one per agent it covered, plus a per-host signal for
/// every host actually queried.
pub(crate) type PrSyncReport = (Vec<PrStatusResult>, Vec<HostSignal>);

/// One chunk's outcome: the answers it brought back (by question index), the
/// chunk's `rateLimit` snapshot, whether the whole call hard-failed, and
/// whether that failure looked like rate-limiting. Aggregated per host by
/// `run_entries`.
type ChunkOutcome = (
    Vec<(usize, QuestionAnswer)>,
    Option<RateLimitInfo>,
    bool,
    bool,
);

/// Snapshot of the active per-host backoff windows (`host -> until`) passed into
/// a sync so backed-off hosts are skipped (their questions go unasked and every
/// pull request keeps what it last knew) without another `gh` call.
pub(crate) type BackoffSnapshot = std::collections::HashMap<String, Instant>;

/// GraphQL `rateLimit.remaining` floor below which the loop pauses polling
/// until the quota resets.
pub(crate) const RATE_LIMIT_BACKOFF_FLOOR: i64 = 100;

/// Max GraphQL aliases per `gh api graphql` invocation. A question emits one or
/// two aliases; capping the batch keeps every query ~1 GraphQL point AND stays
/// well under Linux's ~128 KiB per-argv-entry limit for the inlined `-f query=`
/// arg.
const MAX_ALIASES_PER_QUERY: usize = 100;

/// How many of a branch's pull requests the discovery alias asks for. The
/// connection answers oldest-first by creation date and the alias walks it from
/// the newest end, so this is "the most recently created N pull requests on this
/// branch name". A branch reused a couple of times needs a handful.
///
/// What the window cannot see is an older pull request behind twenty newer ones
/// on the same name. One the agent already has is still refreshed by its
/// number, so the window only bounds what discovery can add.
///
/// The cost, measured rather than assumed: the query still bills one GraphQL
/// point, exactly as `first: 1` did, so the quota is unaffected. What grows is
/// the response body, up to about 17x on a hot ref that really does have twenty
/// pull requests, worth roughly 5% of the call's wall time. Most refs have one.
const DISCOVERY_WINDOW: usize = 20;

/// How many of the most recently created pull requests with a given head name
/// the head-name lookup asks for. It runs only for a branch whose ref may be
/// gone, and every candidate still has to pass the checks in
/// [`select_head_name_matches`], so a handful covers a name reused a few times.
const HEAD_NAME_WINDOW: usize = 5;

/// How much earlier than a branch's birth a pull request may claim to have been
/// opened and still be counted as the branch's. GitHub stamps `createdAt` with
/// its own clock and dux stamps the branch with this machine's, so an exact
/// comparison would refuse the agent's own pull request on a machine whose
/// clock runs a little fast.
const HEAD_NAME_CLOCK_SLACK: chrono::TimeDelta = chrono::TimeDelta::minutes(5);

/// What every pull-request node in a sync query asks for.
const PR_NODE_FIELDS: &str = "number state isDraft title url createdAt updatedAt closedAt \
mergedAt headRefName headRefOid repository { nameWithOwner }";

/// Hard wall-clock cap on a single `gh` invocation. A hung `gh` (stalled TCP,
/// DNS hang, credential-helper prompt) must not park a worker thread: we bail
/// fast and let the next cycle retry rather than block for long.
pub(crate) const GH_CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// After we kill a timed-out/failed `gh`, how long to wait for its output-reader
/// threads to drain before abandoning them (they finish on their own once the
/// pipe closes). Bounds the wait so a pathological grandchild holding the pipe
/// open can never freeze the caller.
const GH_READER_DRAIN: Duration = Duration::from_secs(2);

/// Batched PR sync over the shared session snapshot. Every agent's questions
/// are packed into one or more `gh api graphql` requests per GitHub host (at
/// most `MAX_ALIASES_PER_QUERY` aliases each), and one [`PrStatusResult`] comes
/// back per agent, plus a per-host signal for the backoff. Hosts already backed
/// off in `backoff` are skipped with no `gh` call.
pub(crate) fn run_pr_sync(
    sessions: &Arc<Mutex<Vec<PrSyncEntry>>>,
    backoff: &BackoffSnapshot,
    policy: &GithubHostPolicy,
    trigger: SyncTrigger,
) -> PrSyncReport {
    run_pr_sync_scoped(sessions, backoff, policy, trigger, SyncScope::Everything)
}

/// Which half of the plan a cycle covers.
///
/// Only the blind poll narrows: an agent in the list's Inactive tail is polled
/// on `ui.pr_poll_inactive_interval_seconds` rather than the active clock, so
/// most of its cycles leave those entries alone. Every deliberate trigger asks
/// about whatever it was pointed at, whichever tail the agent is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SyncScope {
    /// Every entry in the plan.
    Everything,
    /// Only the agents the sidebar shows as active.
    ActiveOnly,
}

/// [`run_pr_sync`] with the cycle's [`SyncScope`] named.
pub(crate) fn run_pr_sync_scoped(
    sessions: &Arc<Mutex<Vec<PrSyncEntry>>>,
    backoff: &BackoffSnapshot,
    policy: &GithubHostPolicy,
    trigger: SyncTrigger,
    scope: SyncScope,
) -> PrSyncReport {
    // The plan is replaced whole, so a writer that panicked left a whole one
    // behind.
    let snapshot = sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let entries = entries_for_scope(snapshot, scope);
    run_entries(&entries, backoff, policy, trigger, chrono::Utc::now())
}

/// The entries a cycle of the given [`SyncScope`] asks GitHub about. Pure, so
/// which agents a narrowed cycle leaves alone can be asserted without a `gh`.
pub(crate) fn entries_for_scope(entries: Vec<PrSyncEntry>, scope: SyncScope) -> Vec<PrSyncEntry> {
    match scope {
        SyncScope::Everything => entries,
        SyncScope::ActiveOnly => entries.into_iter().filter(|e| !e.inactive).collect(),
    }
}

/// Whether the blind poll should include the plan's INACTIVE entries this cycle.
///
/// `interval_secs == 0` is the user turning the slow clock off, and no elapsed
/// time ever brings it back. Otherwise an inactive sweep is due once that many
/// seconds have passed since the last one. Pure so the cadence is testable
/// without a poller, a clock or a `gh`.
pub(crate) fn inactive_sweep_due(since_last: Duration, interval_secs: u64) -> bool {
    interval_secs != 0 && since_last >= Duration::from_secs(interval_secs)
}

/// What caused this sync, which decides how much a dormant agent is worth
/// spending a call on.
///
/// It is a CALL-SITE parameter rather than a field on `PrSyncEntry` on purpose:
/// the blind poll and the initial refresh read the same shared entries vector,
/// so a per-entry field could not tell them apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SyncTrigger {
    /// The periodic poll loop, running on its own with nobody asking.
    BlindPoll,
    /// An agent was brought to the foreground. Frequent and cheap to provoke:
    /// tabbing down a sidebar fires one per agent passed through, so it buys
    /// nothing for a dormant agent whose pull requests are already finished.
    Focus,
    /// A deliberate event: boot, an agent being created, its branch being
    /// renamed or drifting, an agent exit, or the user asking (a resume or an
    /// attach). Rare, and each one is a reason to believe something moved, so it
    /// is the only trigger that spends a call on a dormant agent's closed pull
    /// requests and finished branch.
    OneShot,
}

/// Single-agent PR check (foreground / create / exit triggers). Shares the
/// batched machinery with a one-element batch, and returns the per-host signal
/// so the one-shot caller can arm/clear the shared backoff too.
pub(crate) fn check_pr_for_entry(
    entry: &PrSyncEntry,
    backoff: &BackoffSnapshot,
    policy: &GithubHostPolicy,
    trigger: SyncTrigger,
) -> (PrStatusResult, Vec<HostSignal>) {
    let (mut results, signals) = run_entries(
        std::slice::from_ref(entry),
        backoff,
        policy,
        trigger,
        chrono::Utc::now(),
    );
    let result = results.pop().unwrap_or_else(|| PrStatusResult {
        session_id: entry.session_id.clone(),
        answers: Vec::new(),
        asked: Vec::new(),
        own_repo: None,
    });
    (result, signals)
}

/// One thing a sync asks GitHub about one agent.
struct Question {
    /// Which of the cycle's entries it is for.
    entry: usize,
    host: String,
    owner: String,
    repo: String,
    ask: Ask,
}

/// What a [`Question`] asks.
enum Ask {
    /// The pull requests whose head is `name`, one of the names `branch` has
    /// carried.
    Discovery {
        branch: String,
        name: String,
        /// See [`PrSyncBranch::identity_at`].
        identity_at: Option<chrono::DateTime<chrono::Utc>>,
        /// Also look the pull requests up by head name, which still answers
        /// once a merge has deleted the branch. Used only when the ref is gone.
        head: Option<HeadEvidence>,
    },
    /// One pull request the agent already has, by its number.
    Refresh { number: u64 },
}

/// What a head-name match is checked against: a name is not an identity, so a
/// pull request found by one is taken only when its head commit is provably
/// this branch's.
struct HeadEvidence {
    worktree_path: String,
    /// The local branch whose history the head commit may be in.
    local_branch: String,
    /// The commit the branch last pointed at, for one the agent has left.
    tip: Option<String>,
}

impl Question {
    /// How many aliases the question costs in a query.
    fn aliases(&self) -> usize {
        match &self.ask {
            Ask::Discovery { head, .. } => 1 + usize::from(head.is_some()),
            Ask::Refresh { .. } => 1,
        }
    }

    fn owner_repo(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

/// What came back for one question that GitHub answered.
enum QuestionAnswer {
    Discovery {
        /// The pull requests to record for the agent.
        found: Vec<PrInfo>,
        /// Every pull request of this repository the window returned.
        returned: Vec<crate::model::PrKey>,
        /// The ref came back null: GitHub has no branch by that name.
        ref_gone: bool,
    },
    /// `None` when GitHub answered null for the number.
    Refresh(Option<PrInfo>),
}

/// The pull requests in `known` whose head is one of `branch`'s names.
fn rows_on<'a>(
    known: &'a [crate::model::AgentPr],
    branch: &'a PrSyncBranch,
) -> impl Iterator<Item = &'a crate::model::AgentPr> {
    known.iter().filter(|row| {
        row.head_branch
            .as_deref()
            .is_some_and(|head| branch.names().any(|name| name == head))
    })
}

/// A merged or closed pull request.
fn is_terminal(state: &PrState) -> bool {
    matches!(state, PrState::Merged | PrState::Closed)
}

/// Whether every pull request on `branch` is merged, with at least one there.
fn merged_only(known: &[crate::model::AgentPr], branch: &PrSyncBranch) -> bool {
    let mut rows = rows_on(known, branch).peekable();
    rows.peek().is_some() && rows.all(|row| row.state == PrState::Merged)
}

/// The earlier branches this cycle asks about: the least recently asked first
/// (one never asked before all of them), at most the configured number, and a
/// branch whose pull requests are all merged only once the settled recheck has
/// passed since it was last asked. An exited agent's earlier branches all wait
/// for that recheck, except on a deliberate trigger: a sidebar of dormant
/// agents must not cost a cycle's worth of questions each. Every branch that
/// is eligible is therefore asked within `ceil(eligible / per_cycle)` cycles.
fn due_history_branches(
    entry: &PrSyncEntry,
    trigger: SyncTrigger,
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<&PrSyncBranch> {
    let dormant = entry.agent_exited && trigger != SyncTrigger::OneShot;
    let mut due: Vec<&PrSyncBranch> = entry
        .history
        .iter()
        .filter(|branch| {
            (!dormant && !merged_only(&entry.known, branch))
                || branch
                    .last_ask
                    .as_ref()
                    .is_none_or(|ask| now - ask.at >= entry.settled_recheck)
        })
        .collect();
    due.sort_by_key(|branch| branch.last_ask.as_ref().map(|ask| ask.at));
    due.truncate(entry.history_per_cycle);
    due
}

/// Whether a pull request the agent already has is refreshed by number this
/// cycle, by its state:
///
/// | state | agent running | agent exited |
/// |---|---|---|
/// | never refreshed | yes | yes |
/// | open or draft | yes | yes |
/// | closed | yes | on a deliberate trigger, or once the settled recheck passed |
/// | merged | no, a merge is final | no |
fn refresh_due(
    row: &crate::model::AgentPr,
    entry: &PrSyncEntry,
    trigger: SyncTrigger,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    let Some(refreshed_at) = row.last_refreshed_at else {
        return true;
    };
    match row.state {
        PrState::Open | PrState::Draft => true,
        PrState::Merged => false,
        PrState::Closed => {
            !entry.agent_exited
                || trigger == SyncTrigger::OneShot
                || now - refreshed_at >= entry.settled_recheck
        }
    }
}

/// Core of the batched sync: plan every entry's questions, ask them host by
/// host, and gather the answers back per agent.
fn run_entries(
    entries: &[PrSyncEntry],
    backoff: &BackoffSnapshot,
    policy: &GithubHostPolicy,
    trigger: SyncTrigger,
    now: chrono::DateTime<chrono::Utc>,
) -> PrSyncReport {
    run_entries_with(
        entries,
        backoff,
        policy,
        trigger,
        now,
        &|path| live_remote_resolver(path, policy),
        &|args| run_gh_with_timeout(args, GH_CALL_TIMEOUT),
    )
}

/// [`run_entries`] with how a worktree's remote is read and how `gh` is
/// called handed in, so the grouping by host and the merging of every host's
/// answers can be exercised without git or a `gh`.
fn run_entries_with(
    entries: &[PrSyncEntry],
    backoff: &BackoffSnapshot,
    policy: &GithubHostPolicy,
    trigger: SyncTrigger,
    now: chrono::DateTime<chrono::Utc>,
    resolve_remote: &dyn Fn(&Path) -> git::RemoteResolution,
    call_gh: &dyn Fn(&[&str]) -> GhCallOutcome,
) -> PrSyncReport {
    // The remote each worktree names is the agent's own repository, which
    // the answers carry back whether or not anything was asked there.
    let remotes: std::cell::RefCell<BTreeMap<std::path::PathBuf, git::GitHubRemote>> =
        std::cell::RefCell::default();
    let questions = plan_entries(
        entries,
        &|path| {
            let resolution = resolve_remote(path);
            if let git::RemoteResolution::Allowed(remote) = &resolution {
                remotes.borrow_mut().insert(
                    path.to_path_buf(),
                    git::GitHubRemote {
                        host: normalized_github_host(&remote.host),
                        owner_repo: remote.owner_repo.clone(),
                    },
                );
            }
            resolution
        },
        policy,
        trigger,
        now,
    );
    let remotes = remotes.into_inner();

    // Group by host; for each host either skip it (already backed off: nothing
    // asked, no signal) or chunk its questions by alias budget and emit one
    // per-host signal driving the backoff.
    let instant = Instant::now();
    let mut answers: Vec<(usize, QuestionAnswer)> = Vec::new();
    let mut signals: Vec<HostSignal> = Vec::new();
    // What GitHub billed for the whole cycle, summed over every chunk actually
    // run. The per-host signal keeps only the TIGHTEST snapshot (the backoff
    // cares about what is left, not what was spent), so the total is
    // accumulated here instead.
    let mut points = 0i64;
    let mut by_host: BTreeMap<String, Vec<usize>> = BTreeMap::default();
    for (i, question) in questions.iter().enumerate() {
        by_host.entry(question.host.clone()).or_default().push(i);
    }
    for (host, idxs) in by_host {
        if backoff.get(&host).is_some_and(|until| instant < *until) {
            continue;
        }
        let mut rate: Option<RateLimitInfo> = None;
        let mut hard_failed = false;
        let mut rate_limited = false;
        let mut chunk: Vec<usize> = Vec::new();
        let mut alias_count = 0usize;
        let mut run = |chunk: &[usize]| {
            let (found, rl, failed, limited) =
                run_chunk(&host, &questions, chunk, entries, call_gh);
            answers.extend(found);
            points += chunk_cost(rl.as_ref());
            rate = tighter_rate_limit(rate.take(), rl);
            hard_failed |= failed;
            rate_limited |= limited;
        };
        for i in idxs {
            let cost = questions[i].aliases();
            if !chunk.is_empty() && alias_count + cost > MAX_ALIASES_PER_QUERY {
                run(&chunk);
                chunk.clear();
                alias_count = 0;
            }
            chunk.push(i);
            alias_count += cost;
        }
        if !chunk.is_empty() {
            run(&chunk);
        }
        signals.push(HostSignal {
            host,
            rate,
            hard_failed,
            rate_limited,
        });
    }

    logger::debug(&format_sync_cycle_summary(
        trigger,
        entries.len(),
        questions.len(),
        answers.len(),
        points,
    ));

    let mut results = assemble_results(entries, &questions, answers, now);
    for (result, entry) in results.iter_mut().zip(entries) {
        result.own_repo = remotes.get(Path::new(&entry.worktree_path)).cloned();
    }
    (results, signals)
}

/// Gather a cycle's answers back into one [`PrStatusResult`] per entry.
fn assemble_results(
    entries: &[PrSyncEntry],
    questions: &[Question],
    answers: Vec<(usize, QuestionAnswer)>,
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<PrStatusResult> {
    let mut results: Vec<PrStatusResult> = entries
        .iter()
        .map(|entry| PrStatusResult {
            session_id: entry.session_id.clone(),
            answers: Vec::new(),
            asked: Vec::new(),
            own_repo: None,
        })
        .collect();
    let mut asked: Vec<BTreeMap<String, BranchAskRecord>> = vec![BTreeMap::new(); entries.len()];
    for (index, answer) in answers {
        let question = &questions[index];
        let result = &mut results[question.entry];
        match (&question.ask, answer) {
            (
                Ask::Discovery { branch, .. },
                QuestionAnswer::Discovery {
                    found,
                    returned,
                    ref_gone,
                },
            ) => {
                let record = asked[question.entry]
                    .entry(branch.clone())
                    .or_insert_with(|| BranchAskRecord {
                        at: now,
                        ref_gone: true,
                        returned: Vec::new(),
                    });
                record.ref_gone &= ref_gone;
                for key in returned {
                    if !record.returned.contains(&key) {
                        record.returned.push(key);
                    }
                }
                result
                    .answers
                    .extend(found.into_iter().map(PrSyncAnswer::Found));
            }
            (Ask::Refresh { .. }, QuestionAnswer::Refresh(Some(info))) => {
                result.answers.push(PrSyncAnswer::Refreshed(info));
            }
            (Ask::Refresh { number }, QuestionAnswer::Refresh(None)) => {
                result
                    .answers
                    .push(PrSyncAnswer::Unanswered(crate::model::PrKey::new(
                        question.host.clone(),
                        question.owner_repo(),
                        *number,
                    )));
            }
            _ => {}
        }
    }
    for (result, asked) in results.iter_mut().zip(asked) {
        result.asked = asked.into_iter().collect();
    }
    results
}

fn answer_key(info: &PrInfo) -> crate::model::PrKey {
    crate::model::PrKey::new(info.host.clone(), info.owner_repo.clone(), info.number)
}

/// The remote resolver production uses: the real one, reading the worktree's
/// `origin` through git, with the user's own configuration applied. That is
/// correct here, because the rewritten URL is the one git would really contact.
fn live_remote_resolver(worktree_path: &Path, policy: &GithubHostPolicy) -> git::RemoteResolution {
    git::resolve_remote_github_repo(worktree_path, policy)
}

/// The planning half of [`run_entries`], split out so it can be exercised on
/// its own: it resolves each entry's repository and decides what would be
/// asked, and it makes no network call and spawns no `gh` process.
///
/// The remote resolver is a parameter so the planning can be exercised without
/// git; in production it is always [`live_remote_resolver`]. That keeps a
/// planning test from being decided by the developer's own git configuration:
/// the live resolver shells out with no isolation, deliberately, so that
/// `url.*.insteadOf` is applied for real, and an inherited rewrite would reach
/// straight into any test that used it.
fn plan_entries(
    entries: &[PrSyncEntry],
    resolve_remote: &dyn Fn(&Path) -> git::RemoteResolution,
    policy: &GithubHostPolicy,
    trigger: SyncTrigger,
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<Question> {
    let mut questions = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let remote = resolve_remote(Path::new(&entry.worktree_path));
        plan_entry(index, entry, remote, policy, trigger, now, &mut questions);
    }
    logger::debug(&format!(
        "[gh-integration] PR sync plans {} for {}",
        crate::text::count_of(questions.len(), "question"),
        crate::text::count_of(entries.len(), "agent"),
    ));
    questions
}

/// One entry's questions: discovery of its branches in the repository its
/// remote names, and a refresh by number of each pull request it has that is
/// due, each in its own repository and on its own host.
fn plan_entry(
    index: usize,
    entry: &PrSyncEntry,
    remote: git::RemoteResolution,
    policy: &GithubHostPolicy,
    trigger: SyncTrigger,
    now: chrono::DateTime<chrono::Utc>,
    questions: &mut Vec<Question>,
) {
    // Discovery needs the repository the agent pushes to. A remote dux cannot
    // read, or one on a host dux may not ask about, gets no discovery; the
    // pull requests it already has are still refreshed where they live.
    let target = match remote {
        git::RemoteResolution::Allowed(remote) => {
            // Hostnames are case-insensitive and this value becomes a `gh`
            // `--hostname` argument, so it is lowercased at the boundary.
            let host = normalize_github_host(&remote.host).to_ascii_lowercase();
            remote
                .owner_repo
                .split_once('/')
                .filter(|_| policy.allows(&host))
                .map(|(owner, repo)| (host, owner.to_string(), repo.to_string()))
        }
        git::RemoteResolution::Denied | git::RemoteResolution::Unresolved => None,
    };
    let mut discovered: Vec<&PrSyncBranch> = Vec::new();
    if let Some((host, owner, repo)) = &target {
        let mut ask = |branch: &PrSyncBranch,
                       identity_at: Option<chrono::DateTime<chrono::Utc>>,
                       head: &dyn Fn() -> Option<HeadEvidence>| {
            for name in branch.names() {
                questions.push(Question {
                    entry: index,
                    host: host.clone(),
                    owner: owner.clone(),
                    repo: repo.clone(),
                    ask: Ask::Discovery {
                        branch: branch.name.clone(),
                        name: name.to_string(),
                        identity_at,
                        head: head(),
                    },
                });
            }
        };
        let current = &entry.current;
        let mut on_current = rows_on(&entry.known, current).peekable();
        let has_rows = on_current.peek().is_some();
        let all_terminal = on_current.all(|row| is_terminal(&row.state));
        // A dormant agent whose branch has nothing open left is asked about on
        // the deliberate triggers only: tabbing past a sidebar of finished
        // agents must not spawn a `gh` per agent.
        let settled =
            entry.agent_exited && trigger != SyncTrigger::OneShot && has_rows && all_terminal;
        if !settled {
            // The head-name lookup covers a pull request that was merged and
            // its branch deleted before dux saw it, which matters only while
            // nothing open is known on the branch.
            ask(current, current.identity_at, &|| {
                all_terminal.then(|| HeadEvidence {
                    worktree_path: entry.worktree_path.clone(),
                    local_branch: current.name.clone(),
                    tip: None,
                })
            });
            discovered.push(current);
        }
        for branch in due_history_branches(entry, trigger, now) {
            let gone = branch.last_ask.as_ref().is_some_and(|ask| ask.ref_gone);
            ask(branch, branch.identity_at, &|| {
                gone.then(|| HeadEvidence {
                    worktree_path: entry.worktree_path.clone(),
                    local_branch: branch.name.clone(),
                    tip: branch.tip.clone(),
                })
            });
            discovered.push(branch);
        }
    }

    for row in &entry.known {
        if !refresh_due(row, entry, trigger, now) {
            continue;
        }
        // Discovery of the row's own branch carries every field a refresh
        // would, so a row that branch's last discovery returned is left to it.
        let carried_by_discovery = target.as_ref().is_some_and(|(host, owner, repo)| {
            row.host.eq_ignore_ascii_case(host)
                && row
                    .owner_repo
                    .eq_ignore_ascii_case(&format!("{owner}/{repo}"))
                && discovered.iter().any(|branch| {
                    row.head_branch
                        .as_deref()
                        .is_some_and(|head| branch.names().any(|name| name == head))
                        && branch
                            .last_ask
                            .as_ref()
                            .is_some_and(|ask| !ask.ref_gone && ask.returned.contains(&row.key()))
                })
        });
        if carried_by_discovery {
            continue;
        }
        let host = normalize_github_host(&row.host).to_ascii_lowercase();
        if !policy.allows(&host) {
            continue;
        }
        let Some((owner, repo)) = row.owner_repo.split_once('/') else {
            continue;
        };
        questions.push(Question {
            entry: index,
            host,
            owner: owner.to_string(),
            repo: repo.to_string(),
            ask: Ask::Refresh { number: row.number },
        });
    }
}

/// What one chunk's query billed, or zero when the call failed or the answer
/// carried no `rateLimit` (an unbilled call is not a billed one).
fn chunk_cost(rate: Option<&RateLimitInfo>) -> i64 {
    rate.and_then(|r| r.cost).unwrap_or(0)
}

/// Keep the snapshot with the fewer remaining points (the more urgent backoff
/// signal) across chunked calls.
fn tighter_rate_limit(a: Option<RateLimitInfo>, b: Option<RateLimitInfo>) -> Option<RateLimitInfo> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if b.remaining < a.remaining { b } else { a }),
        (Some(a), None) => Some(a),
        (None, b) => b,
    }
}

/// GraphQL-escape a string literal for inlining into the query body. Branch
/// names can contain `/` and, rarely, `"`; JSON string encoding is a valid
/// GraphQL string literal.
fn graphql_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

// The GraphQL alias names. `build_chunk_query` writes them and
// `parse_chunk_response` reads them back, so both sides MUST derive them from
// these helpers: a divergence would silently drop every answer.
fn repo_alias(k: usize) -> String {
    format!("r{k}")
}
fn ref_alias(pos: usize) -> String {
    format!("q{pos}_ref")
}
fn num_alias(pos: usize) -> String {
    format!("q{pos}_num")
}
fn head_alias(pos: usize) -> String {
    format!("q{pos}_head")
}

/// Which hosts dux may name when it calls `gh`.
///
/// Computed by asking `gh` which hosts it can actually serve, rather than
/// guessing from the name, which rejects a company server at
/// `git.company.example`. Stored on the engine and passed explicitly to its
/// consumers rather than reached for through a process-global.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum GithubHostPolicy {
    /// No probe has produced a decisive answer yet, so nothing qualifies. This
    /// is the initial value, and it is also what a missing `gh` restores: a
    /// removed `gh` must not leave dux believing in hosts it can no longer reach.
    #[default]
    DenyAll,
    /// `gh` answered in machine-readable form. Exactly these hosts (lowercased)
    /// have an ACTIVE account reporting `success`. Nothing is unioned onto this
    /// set from the name rule: a host `gh` cannot serve must not qualify just
    /// because it is spelled `github.something`.
    Hosts(BTreeSet<String>),
    /// The installed `gh` is too old to report its hosts, so eligibility falls
    /// back to the rule dux shipped before. Nobody on an older `gh` is worse off
    /// than they were, and arbitrary enterprise hostnames need a newer one.
    LegacyNameRule,
}

impl GithubHostPolicy {
    /// Whether `host` may be handed to `gh`. Compared lowercased, and exactly
    /// otherwise. An empty host never qualifies: callers that mean github.com say
    /// so (see `normalize_github_host`), and treating "" as github.com would let
    /// an unparsed remote through the gate.
    ///
    /// Lowercasing is safe here because hostnames are case-insensitive. Trimming
    /// is not, and is deliberately absent: an address like `git@ github.com:o/r`
    /// yields the host `" github.com"`, and answering for `github.com` would let
    /// the caller hand `gh` the spelling it is actually holding. Whitespace or a
    /// control character in a host is refused outright rather than left to each
    /// mode, because `LegacyNameRule` would accept `"github.com "` on its own.
    pub fn allows(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        if host.is_empty() || host.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return false;
        }
        match self {
            Self::DenyAll => false,
            Self::Hosts(hosts) => hosts.contains(&host),
            Self::LegacyNameRule => host == "github.com" || host.starts_with("github."),
        }
    }
}

/// The hosts dux reads an address on as GitHub's, so that a page path pasted
/// from a browser comes off it: `github.com` always, which needs no sign-in to
/// be GitHub, plus the hosts the [`GithubHostPolicy`] allows while the GitHub
/// integration is on. The clone and the destination folder it suggests ask the
/// same value, and the bootstrap document carries it as `github_hosts` so the
/// browser's folder suggestion can ask it too.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GithubHosts {
    /// `github.com` and exactly these hosts, lowercased.
    Hosts {
        /// The hosts besides `github.com`.
        hosts: Vec<String>,
    },
    /// `github.com` and any host whose name starts with `github.`: the rule an
    /// older `gh`, which cannot list its hosts, leaves dux with.
    NameRule,
}

impl GithubHosts {
    /// `github.com` alone, which is what the integration being off leaves.
    pub fn github_com_only() -> Self {
        Self::Hosts { hosts: Vec::new() }
    }

    /// The hosts `policy` lets dux ask `gh` about, plus `github.com`.
    pub fn from_policy(policy: &GithubHostPolicy) -> Self {
        match policy {
            GithubHostPolicy::DenyAll => Self::github_com_only(),
            GithubHostPolicy::Hosts(hosts) => Self::Hosts {
                hosts: hosts.iter().cloned().collect(),
            },
            GithubHostPolicy::LegacyNameRule => Self::NameRule,
        }
    }

    /// Whether `host` (lowercase, with a web port when it has one) is read as
    /// GitHub's.
    pub fn contains(&self, host: &str) -> bool {
        host == "github.com"
            || match self {
                Self::Hosts { hosts } => hosts.iter().any(|known| known == host),
                Self::NameRule => host.starts_with("github."),
            }
    }
}

/// How `gh` clones from a host, as its `gitProtocol` setting says. A missing or
/// unrecognised value is https, which is also what `gh repo clone` falls back to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GitProtocol {
    /// Clone over HTTPS.
    #[default]
    Https,
    /// Clone over SSH.
    Ssh,
}

impl GitProtocol {
    fn from_gh(value: &serde_json::Value) -> Self {
        if value.as_str() == Some("ssh") {
            Self::Ssh
        } else {
            Self::Https
        }
    }
}

/// What one run of the `gh` host probe concluded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GhProbe {
    /// `gh` is not on PATH.
    NotInstalled,
    /// The probe timed out or failed to launch. This is NOT the older-`gh`
    /// case: it decides nothing, must not reach the name-rule fallback, and
    /// leaves the previously computed policy in place.
    Transient(String),
    /// `gh` answered. `available` is whether any GitHub feature can work at all.
    Decided {
        /// Whether any GitHub feature can work at all.
        available: bool,
        /// Which hosts dux may ask about.
        policy: GithubHostPolicy,
        /// Each qualifying host's git protocol (lowercased host keys). Empty
        /// under the legacy policy, which reports none: https for every host.
        protocols: BTreeMap<String, GitProtocol>,
    },
}

/// The shape of `gh auth status --active --json hosts`, typed so that anything
/// that is not that shape fails to deserialize instead of being read loosely.
///
/// Only the fields the decision needs are named (`gitProtocol` is read loosely);
/// `gh` sends more (login, scopes, tokenSource) and serde ignores them, so a
/// future field cannot break this.
#[derive(serde::Deserialize)]
struct AuthStatusOutput {
    hosts: std::collections::BTreeMap<String, Vec<AuthStatusAccount>>,
}

/// One account under one host key.
///
/// `state`, `active` and `host` are required, so a record missing one, or
/// carrying null or the wrong type, fails to deserialize and takes the whole
/// response with it. That is the difference between "gh says no" and "dux could
/// not read what gh said": optional fields would turn records that decide nothing
/// into decisive-looking answers, an empty host set that turns every GitHub
/// feature off, or a map key qualified by a record that never named a host.
///
/// A response containing an unreadable record is therefore transient (see
/// [`decide_gh_probe`]), which preserves the last known good policy. gh 2.95.0
/// emits all three fields on every account.
///
/// `error` is the exception and defaults, because gh tags it `omitempty` and
/// omits it on a healthy account, so its absence must not fail the parse.
#[derive(serde::Deserialize)]
struct AuthStatusAccount {
    state: String,
    active: bool,
    host: String,
    #[serde(default)]
    error: String,
    /// Read loosely: any shape other than the string `ssh` means https.
    #[serde(default, rename = "gitProtocol")]
    git_protocol: serde_json::Value,
}

impl AuthStatusAccount {
    /// Whether this account makes `host_key` (already trimmed and lowercased) a
    /// host dux may name when it calls `gh`.
    fn qualifies(&self, host_key: &str) -> bool {
        // Every call dux makes names a host and never an account, so `gh` uses
        // that host's active account: a working sibling cannot vouch for a
        // broken active one.
        if !self.active {
            return false;
        }
        if self.state != "success" {
            return false;
        }
        // The map key is what would be handed to `gh`, so a record naming a
        // different host is describing something else and cannot vouch for
        // this one.
        self.host.trim().eq_ignore_ascii_case(host_key)
    }
}

/// Parse the stdout of `gh auth status --active --json hosts` into the set of
/// hosts whose active account works.
///
/// Returns `None` when the output is not that shape, which is not on its own the
/// older-`gh` signal: see [`decide_gh_probe`], where an unparseable answer
/// selects the fallback only when `gh`'s own diagnostics say it did not
/// understand the call.
///
/// Measured against gh 2.95.0, whose output is one entry per account:
/// `{"hosts":{"github.com":[{"state":"success","active":true,"host":"github.com",…}]}}`.
/// Neither the map keys nor the exit code can stand in for `state`: `gh` lists
/// every host it merely knows, including one whose login has expired, and in
/// JSON mode it exits zero regardless.
///
/// Test-only: the decision itself calls [`parse_auth_status`], which also carries
/// the failures behind an empty set.
#[cfg(test)]
pub(crate) fn parse_auth_status_hosts(stdout: &str) -> Option<BTreeSet<String>> {
    parse_auth_status(stdout).map(|reading| reading.eligible)
}

/// Everything the decision needs out of one machine-readable answer: which
/// hosts work, and what `gh` said about the ones that do not.
///
/// The failures are kept because an empty `eligible` is not one answer but
/// several. `gh` holding no credential at all, `gh` holding one GitHub rejected,
/// and `gh` being unable to reach GitHub all reduce to "no host qualified", and
/// only the last of the three is worth retrying. See [`decide_gh_probe`].
pub(crate) struct AuthStatusReading {
    /// Hosts whose ACTIVE account reports success.
    eligible: BTreeSet<String>,
    /// The git protocol of each eligible host's qualifying account.
    protocols: BTreeMap<String, GitProtocol>,
    /// `gh`'s own `error` text for every ACTIVE account that did not qualify,
    /// in host order. Inactive accounts are skipped: dux never names an account
    /// when it calls `gh`, so a sibling's failure is not this call's problem.
    active_errors: Vec<String>,
}

/// Parse the stdout of `gh auth status --active --json hosts` into the eligible
/// hosts plus the failures behind the ones that are missing. `None` when the
/// output is not that shape; see [`parse_auth_status_hosts`].
pub(crate) fn parse_auth_status(stdout: &str) -> Option<AuthStatusReading> {
    let parsed: AuthStatusOutput = serde_json::from_str(stdout.trim()).ok()?;
    let mut reading = AuthStatusReading {
        eligible: BTreeSet::new(),
        protocols: BTreeMap::new(),
        active_errors: Vec::new(),
    };
    for (key, accounts) in &parsed.hosts {
        let key = key.trim().to_ascii_lowercase();
        if key.is_empty() {
            continue;
        }
        if let Some(account) = accounts.iter().find(|account| account.qualifies(&key)) {
            reading
                .protocols
                .insert(key.clone(), GitProtocol::from_gh(&account.git_protocol));
            reading.eligible.insert(key);
            continue;
        }
        reading.active_errors.extend(
            accounts
                .iter()
                .filter(|account| account.active && !account.error.trim().is_empty())
                .map(|account| account.error.trim().to_string()),
        );
    }
    Some(reading)
}

/// Whether `gh`'s own diagnostics say it did not UNDERSTAND the call, as opposed
/// to having tried it and failed.
///
/// This is the ONLY thing that selects the permissive older-`gh` fallback. The
/// strings were measured on gh 2.95.0 here:
///
/// ```text
/// $ gh auth status --nope                    stderr: unknown flag: --nope        exit 1
/// $ gh auth status --active --json bogus     stderr: Unknown JSON field: "bogus" exit 1
/// $ gh auth bogus                            stderr: unknown command "bogus" …   exit 1
/// ```
///
/// In each case stdout is EMPTY, so "it did not parse" looks like a good enough
/// signal on its own. It is not: `gh` exits non-zero in JSON
/// mode for ordinary fatal errors too, so a modern failure was indistinguishable
/// from an old CLI and either widened the eligible hosts to the name rule or
/// replaced the last known good policy on what was really a transient fault.
///
/// The first two strings are the ones an older `gh` produces for `--active` and
/// `--json`. They are literals in cobra (`unknown flag: `, `unknown shorthand
/// flag: `, `unknown command %q for %q`) and in `gh`'s own export layer
/// (`Unknown JSON field: `), they have been stable across years of releases, and
/// they are what every script wrapping `gh` already keys off. That is as stable
/// as a diagnostic gets without `gh` offering a machine-readable capability
/// query, which it does not.
fn diagnostic_says_gh_cannot_do_this(output: &std::process::Output) -> bool {
    // Both streams are scanned: `gh` writes these to stderr, but a wrapper or a
    // future build putting one on stdout should still be understood. stdout has
    // already failed to parse by the time this runs, so nothing is lost.
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
    )
    .to_ascii_lowercase();
    text.contains("unknown flag")
        || text.contains("unknown shorthand flag")
        || text.contains("unknown command")
        || text.contains("unknown json field")
}

/// Text in an account's `error` that means GitHub REJECTED the credential.
///
/// The one decisive account failure: a bad token cannot become good on a retry.
/// Checked BEFORE [`AUTH_STATUS_TRANSIENT_ERROR_MARKERS`] so a body quoting a
/// retryable-looking word cannot rescue a rejected credential. Both shapes `gh`
/// produces are measured, as the `MEASURED_JSON_BAD_*_TOKEN` test fixtures.
const AUTH_STATUS_CREDENTIAL_ERROR_MARKERS: &[&str] =
    &["bad credentials", "401 unauthorized", "http 401"];

/// Text in an account's `error` that means the call never got a verdict on the
/// login: GitHub rate-limited it, answered a server error, or the request did
/// not arrive at all.
///
/// The connection entries read like Go's net stack rather than like `gh` because
/// `gh` surfaces Go's transport error verbatim (measured on gh 2.95.0 with every
/// outbound connection refused). The status codes are NOT measured, since a rate
/// limit cannot be provoked on demand: they are the `HTTP <code>: <message>`
/// shape carrying what GitHub documents for an exhausted quota (403, 429) and
/// what a proxy or an outage produces (5xx). A record matching neither table
/// keeps the decisive reading, so this list can only ever RESCUE an answer from
/// being latched, never invent a failure.
const AUTH_STATUS_TRANSIENT_ERROR_MARKERS: &[&str] = &[
    "rate limit",
    "http 403",
    "403 forbidden",
    "http 429",
    "429 too many requests",
    "http 500",
    "http 502",
    "http 503",
    "http 504",
    "internal server error",
    "bad gateway",
    "service unavailable",
    "gateway timeout",
    "connection refused",
    "connection reset",
    "no such host",
    "network is unreachable",
    "temporary failure in name resolution",
    "i/o timeout",
    "context deadline exceeded",
    "timeout",
    "timed out",
    "tls handshake",
    "proxyconnect",
];

/// The first account error that says the probe never got an answer, or `None`
/// when every failure present decides against the user (or there is no failure
/// text at all, which is what `gh` holding no credential looks like).
fn auth_status_transient_reason(errors: &[String]) -> Option<String> {
    errors.iter().find_map(|error| {
        let lowered = error.to_ascii_lowercase();
        let rejected = AUTH_STATUS_CREDENTIAL_ERROR_MARKERS
            .iter()
            .any(|marker| lowered.contains(marker));
        let retryable = AUTH_STATUS_TRANSIENT_ERROR_MARKERS
            .iter()
            .any(|marker| lowered.contains(marker));
        (!rejected && retryable).then(|| error.clone())
    })
}

/// `gh`'s own sentence for "there is no login here at all", which is the one
/// answer from the plain fallback that genuinely decides against the user.
///
/// Measured on gh 2.95.0, where `gh auth status` with an empty config prints
/// exactly `You are not logged into any GitHub hosts. To log in, run: gh auth
/// login` and exits 1. Matched as a substring and case-insensitively rather than
/// exactly, because `gh` wraps it in a banner and has moved the surrounding
/// punctuation between releases while this clause stayed put.
fn plain_status_says_logged_out(output: &std::process::Output) -> bool {
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
    )
    .to_ascii_lowercase();
    text.contains("not logged in")
}

/// Text in a plain `gh auth status` that does not decide whether the user is
/// logged in.
///
/// The single entry is ambiguous by measurement: on gh 2.95.0 a refused token
/// and a dead network print it BYTE FOR BYTE alike, naming neither the network,
/// a status code, nor a rate limit (the `MEASURED_PLAIN_*` test fixtures carry
/// the four runs). dux therefore reads it as transient and retries, and the
/// sentence the user sees carries `gh`'s own line, so nothing is hidden by the
/// choice; deciding the other way strands a working login behind a momentary
/// outage for the rest of the run.
///
/// This path RUNS only on a `gh` too old to understand `--json`, whose wording
/// could not be measured, so an answer this table does not recognise keeps the
/// decisive reading.
const PLAIN_STATUS_TRANSIENT_MARKERS: &[&str] = &["failed to log in to"];

/// The transient phrase this output carries, if any, as the diagnostic line it
/// came from so the log can name the real reason.
fn plain_status_transient_reason(output: &std::process::Output) -> Option<String> {
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
    );
    let carries_marker = |line: &str| {
        let lowered = line.to_ascii_lowercase();
        PLAIN_STATUS_TRANSIENT_MARKERS
            .iter()
            .any(|marker| lowered.contains(marker))
    };
    // The whole diagnostic is several lines of banner; the line carrying the
    // marker is the one worth putting in the log, minus the cross `gh` bullets
    // it with, which reads as a stray letter inside dux's own sentence.
    let line = text
        .lines()
        .find(|line| carries_marker(line))?
        .trim()
        .trim_start_matches(['X', 'x', '✗', '✘'])
        .trim()
        .to_string();
    Some(if line.is_empty() {
        format!("gh auth status {}", output.status)
    } else {
        line
    })
}

/// Decide the probe's outcome from the machine-readable call, running the plain
/// `gh auth status` fallback ONLY when `gh` says it did not understand it.
pub(crate) fn decide_gh_probe(
    json: GhCallOutcome,
    plain: impl FnOnce() -> GhCallOutcome,
) -> GhProbe {
    let output = match json {
        GhCallOutcome::TimedOut => {
            return GhProbe::Transient(format!(
                "gh auth status --json timed out after {}s",
                GH_CALL_TIMEOUT.as_secs()
            ));
        }
        GhCallOutcome::NotFound(_) => return GhProbe::NotInstalled,
        GhCallOutcome::Failed(msg) => {
            return GhProbe::Transient(format!("could not run gh auth status --json: {msg}"));
        }
        GhCallOutcome::Completed(output) => output,
    };

    // Step 1. A parseable answer IS the answer, whatever the exit status, and
    // nothing is unioned onto it. The status carries no information here: in
    // JSON mode `gh` exits zero even when a known host is broken.
    if let Some(reading) = parse_auth_status(&String::from_utf8_lossy(&output.stdout)) {
        // A host that works decides on its own merits. Availability is
        // deliberately not keyed off the exit status: plain `gh auth status`
        // exits non-zero when ANY known host has a problem, which would let one
        // stale token disable every GitHub feature on every host.
        if !reading.eligible.is_empty() {
            return GhProbe::Decided {
                available: true,
                policy: GithubHostPolicy::Hosts(reading.eligible),
                protocols: reading.protocols,
            };
        }
        // Nothing qualified, which is where the three different answers `gh`
        // collapses into one empty host set have to be told apart. An account
        // that could not reach GitHub has decided nothing, and recording it as
        // "not authenticated" would switch every GitHub feature off for the rest
        // of the run. No accounts at all, and an account GitHub rejected, both
        // decide.
        if let Some(reason) = auth_status_transient_reason(&reading.active_errors) {
            return GhProbe::Transient(format!(
                "gh auth status could not reach GitHub ({reason}); \
                 keeping the last known host policy",
            ));
        }
        return GhProbe::Decided {
            available: false,
            policy: GithubHostPolicy::Hosts(reading.eligible),
            protocols: reading.protocols,
        };
    }

    // The answer did not parse. That alone decides nothing: `gh` also exits
    // non-zero in JSON mode for an ordinary fatal error, and it can be killed
    // mid-write. Only `gh` saying it does not UNDERSTAND the call means the
    // installed one is too old, so everything else is transient and the last
    // known good policy stands.
    if !diagnostic_says_gh_cannot_do_this(&output) {
        return GhProbe::Transient(format!(
            "gh auth status --json {} and wrote nothing dux could read; \
             keeping the last known host policy",
            output.status,
        ));
    }

    // Step 2, reached ONLY because the machine-readable form did not exist in
    // older `gh`. Its exit status decides availability and the name rule decides
    // eligibility, which is exactly what dux shipped before.
    match plain() {
        GhCallOutcome::TimedOut => GhProbe::Transient(format!(
            "gh auth status timed out after {}s",
            GH_CALL_TIMEOUT.as_secs()
        )),
        GhCallOutcome::NotFound(_) => GhProbe::NotInstalled,
        GhCallOutcome::Failed(msg) => GhProbe::Transient(format!("could not run gh: {msg}")),
        GhCallOutcome::Completed(output) => {
            // A non-zero exit decides against the user only when `gh` is talking
            // about the login. When it is reporting a rate limit, an API error
            // or a network fault, it has decided nothing: treating that as "not
            // authenticated" would switch every GitHub feature off until the next
            // restart.
            if !output.status.success()
                && !plain_status_says_logged_out(&output)
                && let Some(reason) = plain_status_transient_reason(&output)
            {
                return GhProbe::Transient(format!(
                    "gh auth status could not reach GitHub ({reason}); \
                     keeping the last known host policy",
                ));
            }
            GhProbe::Decided {
                available: output.status.success(),
                policy: GithubHostPolicy::LegacyNameRule,
                protocols: BTreeMap::new(),
            }
        }
    }
}

/// Whether the GitHub integration (`[ui] github_integration`) is switched on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GithubIntegration {
    /// The integration is on.
    Enabled,
    /// The integration is off, so nothing asks `gh` anything.
    Disabled,
}

/// Whether the periodic `gh` re-check is due, as a pure function of the four
/// things that decide it. The engine owns the clock; this owns the rule.
///
/// The rule, in order:
///
/// * The integration being off means dux asks nothing at all. Turning it on is
///   its own immediate probe, so there is nothing to catch up on here.
/// * [`GhStatus::Available`] stops the timer. Everything works, so polling `gh`
///   forever would be a process spawn every few minutes for no answer anybody
///   is waiting for. A later failure moves the status off Available and the
///   timer starts again on its own, which is why this reads the status live
///   rather than latching a "we are done" flag.
/// * A zero interval means the user disabled the periodic re-check. The
///   on-demand re-check on both surfaces is unaffected.
/// * A probe that has never run is not due here: the surfaces run the startup
///   probe themselves, and answering "due" before it lands would double it.
pub(crate) fn gh_reprobe_is_due(
    status: crate::model::GhStatus,
    integration: GithubIntegration,
    since_last_probe: Option<Duration>,
    interval: Duration,
) -> bool {
    if integration == GithubIntegration::Disabled
        || matches!(status, crate::model::GhStatus::Available)
    {
        return false;
    }
    if interval.is_zero() {
        return false;
    }
    match since_last_probe {
        Some(elapsed) => elapsed >= interval,
        None => false,
    }
}

/// Ask `gh` which hosts it can serve. Runs on a background worker; both calls
/// are bounded so a wedged credential helper cannot park the probe.
///
/// `program` is `gh` in production. It is named rather than hardcoded so the
/// wiring can be tested against a stand-in whose answers are controlled, without
/// a network call, a real login, or mutating the test process's `PATH`.
///
/// A `program` that is not there is [`GhProbe::NotInstalled`], told by the
/// launch itself: without that, "a failure to launch preserves the last known
/// good value" would quietly cover `gh` having been uninstalled, and dux would
/// go on believing in hosts it can no longer reach.
pub(crate) fn probe_github_hosts(program: &OsStr) -> GhProbe {
    let json = run_program_with_timeout(
        program,
        &["auth", "status", "--active", "--json", "hosts"],
        GH_CALL_TIMEOUT,
    );
    decide_gh_probe(json, || {
        run_program_with_timeout(program, &["auth", "status"], GH_CALL_TIMEOUT)
    })
}

/// Outcome of a bounded `gh` invocation. `Failed` carries the failure text (a
/// spawn or wait error) so callers can log the real cause instead of conflating
/// it with a timeout.
#[derive(Debug)]
pub(crate) enum GhCallOutcome {
    Completed(std::process::Output),
    TimedOut,
    Failed(String),
    /// The program is not there; the text is the launch error.
    NotFound(String),
}

/// Run `gh <args>` with piped stdout/stderr drained on threads and a hard
/// wall-clock cap. On every non-`Completed` exit the child is killed and reaped;
/// the reader threads are drained with a bounded wait ([`GH_READER_DRAIN`]) and
/// then abandoned (they self-terminate at EOF) so the caller can never block.
pub(crate) fn run_gh_with_timeout(args: &[&str], timeout: Duration) -> GhCallOutcome {
    run_program_with_timeout(OsStr::new("gh"), args, timeout)
}

/// [`run_gh_with_timeout`] with the program named explicitly, so the host probe
/// can be exercised end to end against a stand-in `gh` without touching the test
/// process's `PATH` (which is shared, and unsafe to mutate under a test runner).
fn run_program_with_timeout(program: &OsStr, args: &[&str], timeout: Duration) -> GhCallOutcome {
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    run_command_with_timeout(cmd, timeout)
}

/// Map the shared bounded runner's outcome onto [`GhCallOutcome`], so every `gh`
/// call site keeps its own vocabulary while the spawn / drain / kill contract
/// lives in one place, [`crate::bounded_command`], shared with the Tailscale
/// probe. (That module is deliberately not `git::wait_child_or_kill`, which
/// pipes only a tiny stderr and never drains stdout, so it cannot be used where
/// the output is the answer.)
fn run_command_with_timeout(cmd: std::process::Command, timeout: Duration) -> GhCallOutcome {
    match crate::bounded_command::run_command_with_timeout(cmd, timeout, GH_READER_DRAIN, "gh") {
        crate::bounded_command::CommandOutcome::Completed(output) => {
            GhCallOutcome::Completed(output)
        }
        crate::bounded_command::CommandOutcome::TimedOut => GhCallOutcome::TimedOut,
        crate::bounded_command::CommandOutcome::Failed(msg) => GhCallOutcome::Failed(msg),
        crate::bounded_command::CommandOutcome::NotFound(msg) => GhCallOutcome::NotFound(msg),
    }
}

/// Build and run one batched GraphQL query for a chunk of same-host questions,
/// returning the answers it got, the rate-limit snapshot, and whether the
/// whole call hard-failed (spawn error / timeout / unparseable stdout, as
/// opposed to a per-alias error, which is handled inline).
fn run_chunk(
    host: &str,
    questions: &[Question],
    chunk: &[usize],
    entries: &[PrSyncEntry],
    call_gh: &dyn Fn(&[&str]) -> GhCallOutcome,
) -> ChunkOutcome {
    let (query, pos_repo) = build_chunk_query(questions, chunk);

    // On a spawn failure / timeout the chunk answers nothing, and every pull
    // request keeps what it last knew.
    let qarg = format!("query={query}");
    let (data_json, stderr): (Option<serde_json::Value>, String) =
        match call_gh(&["api", "graphql", "--hostname", host, "-f", &qarg]) {
            GhCallOutcome::Completed(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                let stdout = String::from_utf8_lossy(&output.stdout);
                // IMPORTANT: `gh api graphql` exits non-zero whenever the response
                // carries any `errors` (e.g. one deleted-repo alias returns
                // `repository: null` + NOT_FOUND), but `data` is still present. Parse
                // stdout regardless of exit status; a single bad alias must not
                // poison the whole batch.
                (
                    serde_json::from_str::<serde_json::Value>(stdout.trim()).ok(),
                    stderr,
                )
            }
            GhCallOutcome::TimedOut => (
                None,
                format!(
                    "gh api graphql timed out after {}s",
                    GH_CALL_TIMEOUT.as_secs()
                ),
            ),
            GhCallOutcome::Failed(msg) | GhCallOutcome::NotFound(msg) => {
                (None, format!("failed to run gh: {msg}"))
            }
        };
    // Collapse both an absent `data` key and an explicit `data: null` (a real
    // GraphQL execution-failure shape) to "no data", so both drive the
    // hard-failure path.
    let data = data_json
        .as_ref()
        .and_then(|j| j.get("data"))
        .filter(|d| !d.is_null());
    let hard_failed = data.is_none();
    let mut rate_limited = false;
    if hard_failed {
        // Always log a total failure (not gated on stderr being non-empty), with
        // the host and how many sessions it affected, so a stuck PR badge is
        // traceable. Include GitHub's own error text when present.
        let errors = data_json
            .as_ref()
            .and_then(|j| j.get("errors"))
            .map(std::string::ToString::to_string)
            .unwrap_or_default();
        // Distinguish rate-limiting (a 403 / secondary limit, or a RATE_LIMITED
        // GraphQL error) from a generic network/gh failure so the status message
        // can say which. Scan gh's stderr and the GraphQL errors payload.
        rate_limited = looks_rate_limited(&stderr) || looks_rate_limited(&errors);
        // Both `stderr` and `errors` go to the log VERBATIM, deliberately.
        //
        // What can appear in them is GitHub's own error text, which for a
        // failure on a private repository names that repository and its owner.
        // That is worth writing down once so the next reader does not have to
        // work out whether it matters.
        //
        // It does not, for three reasons that hold together. The line is
        // `debug`, and the default level is `info`, so it is not even written
        // unless somebody turned debugging on to investigate exactly this. The
        // destination is a local file in the user's own dux directory, not a
        // network sink. And dux never handles a GitHub token: `gh` authenticates
        // itself from its own keyring, so a credential is not among the things
        // that can land here.
        //
        // Against that, the text is the whole value of the line. A stuck PR
        // badge is diagnosed from what GitHub actually said, and a summarised or
        // truncated version would routinely drop the sentence that explains it.
        // So: verbatim, and a reason rather than a redaction.
        logger::debug(&format!(
            "[gh-integration] gh api graphql failed for host {host} ({}, rate_limited={rate_limited}): {stderr}{}",
            crate::text::count_of(chunk.len(), "question"),
            if errors.is_empty() {
                String::new()
            } else {
                format!(" | errors: {errors}")
            },
        ));
    }
    let (out, rate) = parse_chunk_response(
        questions,
        chunk,
        &pos_repo,
        data,
        entries,
        &head_commit_is_the_branchs,
    );
    (out, rate, hard_failed, rate_limited)
}

/// Heuristic: does this `gh`/GraphQL failure text indicate GitHub rate-limiting
/// (a primary/secondary API rate limit) rather than a plain network/`gh` error?
fn looks_rate_limited(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    t.contains("rate limit")
        || t.contains("rate_limited")
        || t.contains("secondary rate")
        || t.contains("abuse detection")
}

/// Build the batched GraphQL query for a chunk of questions plus the
/// per-position repo-alias index (`pos_repo[pos] == k` means question `q{pos}`
/// lives under `r{k}`), so the response can be reparsed. Pure, with no I/O, so
/// it is unit-testable.
fn build_chunk_query(questions: &[Question], chunk: &[usize]) -> (String, Vec<usize>) {
    // Group the chunk's questions by repository → one aliased
    // `repository(...)` block each. GitHub reads owner and name without regard
    // to case, so two spellings of one repository share a block.
    let mut groups: BTreeMap<(String, String), (String, String, Vec<usize>)> = BTreeMap::default();
    for (pos, &i) in chunk.iter().enumerate() {
        let question = &questions[i];
        groups
            .entry((
                question.owner.to_ascii_lowercase(),
                question.repo.to_ascii_lowercase(),
            ))
            .or_insert_with(|| (question.owner.clone(), question.repo.clone(), Vec::new()))
            .2
            .push(pos);
    }
    let mut pos_repo: Vec<usize> = vec![0; chunk.len()];
    for (k, (_, _, positions)) in groups.values().enumerate() {
        for &pos in positions {
            pos_repo[pos] = k;
        }
    }

    let mut q = String::from("{\n  rateLimit { cost remaining resetAt }\n");
    for (k, (owner, repo, positions)) in groups.values().enumerate() {
        let _ = writeln!(
            q,
            "  {}: repository(owner: {}, name: {}) {{",
            repo_alias(k),
            graphql_string(owner),
            graphql_string(repo),
        );
        for &pos in positions {
            match &questions[chunk[pos]].ask {
                Ask::Discovery { name, head, .. } => {
                    // Discovery asks for a WINDOW of the branch's pull
                    // requests and picks among them in Rust, rather than for
                    // one node in a chosen order.
                    //
                    // Measured against two real repositories: GitHub IGNORES
                    // `orderBy` on `associatedPullRequests`. The connection
                    // answers oldest-first by creation date whatever is
                    // passed, so `first: 1` with any orderBy returned the
                    // OLDEST pull request on the branch. `last` walks from the
                    // newest end, which is where the interesting nodes are.
                    //
                    // Each node names its own repository, because this
                    // connection is not confined to the repository the alias
                    // was asked under: a fork's pull request whose head is
                    // this ref appears here too (measured on golang/go). Those
                    // are somebody else's pull requests and
                    // `parse_chunk_response` drops them.
                    let qname = graphql_string(&format!("refs/heads/{name}"));
                    let _ = writeln!(
                        q,
                        "    {}: ref(qualifiedName: {qname}) {{ associatedPullRequests(last: {DISCOVERY_WINDOW}) {{ nodes {{ {PR_NODE_FIELDS} }} }} }}",
                        ref_alias(pos),
                    );
                    if head.is_some() {
                        // The same question asked by head NAME, which still
                        // answers after a merge deleted the branch and the ref
                        // lookup above came back null. A name is not an
                        // identity, so each node carries what
                        // `select_head_name_matches` needs to refuse an older
                        // pull request that merely used the same name.
                        let _ = writeln!(
                            q,
                            "    {}: pullRequests(headRefName: {}, states: [OPEN, MERGED, CLOSED], last: {HEAD_NAME_WINDOW}) {{ nodes {{ {PR_NODE_FIELDS} headRepository {{ nameWithOwner }} }} }}",
                            head_alias(pos),
                            graphql_string(name),
                        );
                    }
                }
                Ask::Refresh { number } => {
                    let _ = writeln!(
                        q,
                        "    {}: pullRequest(number: {number}) {{ {PR_NODE_FIELDS} }}",
                        num_alias(pos),
                    );
                }
            }
        }
        q.push_str("  }\n");
    }
    q.push_str("}\n");
    (q, pos_repo)
}

/// Map a GraphQL `data` object (or `None` when the call failed) back to the
/// answer each of the chunk's questions got. A question whose repository or
/// alias is missing from the answer gets none, so one bad alias never poisons
/// the batch, and a failed call answers nothing at all. Pure apart from the
/// head-commit check passed in, so it is unit-testable with a synthetic
/// response.
fn parse_chunk_response(
    questions: &[Question],
    chunk: &[usize],
    pos_repo: &[usize],
    data: Option<&serde_json::Value>,
    entries: &[PrSyncEntry],
    head_commit_is_the_branchs: &dyn Fn(&HeadEvidence, &str) -> bool,
) -> (Vec<(usize, QuestionAnswer)>, Option<RateLimitInfo>) {
    let Some(data) = data else {
        return (Vec::new(), None);
    };
    let rate = parse_rate_limit(data);
    let mut out = Vec::with_capacity(chunk.len());
    for (pos, &i) in chunk.iter().enumerate() {
        let question = &questions[i];
        let Some(repo_obj) = data
            .get(repo_alias(pos_repo[pos]).as_str())
            .filter(|v| !v.is_null())
        else {
            // A repository GitHub answered null for is gone or out of reach:
            // a refresh there is asked and answered with nothing.
            if matches!(question.ask, Ask::Refresh { .. }) {
                out.push((i, QuestionAnswer::Refresh(None)));
            }
            continue;
        };
        let owner_repo = question.owner_repo();
        let answer = match &question.ask {
            Ask::Discovery {
                name,
                identity_at,
                head,
                ..
            } => {
                let Some(ref_value) = repo_obj.get(ref_alias(pos).as_str()) else {
                    continue;
                };
                if ref_value.is_null() {
                    // A branch a merge deleted has no ref left to ask about,
                    // so its pull requests are looked for by head name.
                    let found = head.as_ref().map_or_else(Vec::new, |evidence| {
                        repo_obj
                            .get(head_alias(pos).as_str())
                            .and_then(|h| h.get("nodes"))
                            .and_then(|n| n.as_array())
                            .map_or_else(Vec::new, |nodes| {
                                select_head_name_matches(
                                    question,
                                    name,
                                    *identity_at,
                                    evidence,
                                    nodes,
                                    head_commit_is_the_branchs,
                                )
                            })
                    });
                    QuestionAnswer::Discovery {
                        returned: found.iter().map(answer_key).collect(),
                        found,
                        ref_gone: true,
                    }
                } else {
                    // A node dux cannot parse (an unrecognised state, a
                    // missing number) drops on its own and leaves its siblings
                    // alone.
                    let nodes: Vec<PrInfo> = ref_value
                        .get("associatedPullRequests")
                        .and_then(|a| a.get("nodes"))
                        .and_then(|n| n.as_array())
                        .map_or_else(Vec::new, |arr| {
                            arr.iter()
                                .filter(|node| node_belongs_to(node, &owner_repo))
                                .filter_map(|node| {
                                    parse_pr_json_value(node, &question.host, &owner_repo)
                                })
                                .collect()
                        });
                    let known = &entries[question.entry].known;
                    QuestionAnswer::Discovery {
                        returned: nodes.iter().map(answer_key).collect(),
                        found: keep_discovered(nodes, *identity_at, known),
                        ref_gone: false,
                    }
                }
            }
            Ask::Refresh { .. } => {
                let Some(node) = repo_obj.get(num_alias(pos).as_str()) else {
                    continue;
                };
                QuestionAnswer::Refresh(
                    Some(node)
                        .filter(|v| !v.is_null())
                        .and_then(|node| parse_pr_json_value(node, &question.host, &owner_repo)),
                )
            }
        };
        out.push((i, answer));
    }
    (out, rate)
}

/// Which of a branch's discovery window to record for the agent. Every one it
/// already has is refreshed. A new one is the branch's when it was opened after
/// the branch was born (give or take [`HEAD_NAME_CLOCK_SLACK`]); when nothing
/// says when the branch was born, only the one [`select_discovered_pr`] picks
/// is taken, so a branch name reused many times never floods the list.
fn keep_discovered(
    nodes: Vec<PrInfo>,
    identity_at: Option<chrono::DateTime<chrono::Utc>>,
    known: &[crate::model::AgentPr],
) -> Vec<PrInfo> {
    let is_known = |info: &PrInfo| known.iter().any(|row| row.is(&answer_key(info)));
    let picked = match identity_at {
        Some(_) => None,
        None => select_discovered_pr(nodes.clone()).map(|pr| pr.number),
    };
    nodes
        .into_iter()
        .filter(|info| {
            is_known(info)
                || match identity_at {
                    Some(born) => info
                        .gh_created_at
                        .is_some_and(|created| created >= born - HEAD_NAME_CLOCK_SLACK),
                    None => picked == Some(info.number),
                }
        })
        .collect()
}

/// Whether a fresh answer differs from what the badge already said. Number and
/// state are the whole badge, so a title or url edit is deliberately not a
/// change worth a line in the log.
pub(crate) fn pr_badge_changed(previous: Option<&PrInfo>, fresh: Option<&PrInfo>) -> bool {
    let old = previous.map(|p| (p.number, pr_state_word(&p.state)));
    let new = fresh.map(|p| (p.number, pr_state_word(&p.state)));
    old != new
}

/// One PR state as it reads in a log line.
fn pr_state_word(state: &PrState) -> &'static str {
    state.word()
}

/// The per-cycle debug summary: how many agents the cycle covered, how many
/// questions it asked, how many of them GitHub answered, and what GitHub
/// billed for the whole cycle.
fn format_sync_cycle_summary(
    trigger: SyncTrigger,
    agents: usize,
    questions: usize,
    answered: usize,
    points: i64,
) -> String {
    let word = match trigger {
        SyncTrigger::BlindPoll => "blind poll",
        SyncTrigger::Focus => "focus",
        SyncTrigger::OneShot => "one-shot",
    };
    format!(
        "PR sync ({word}): {agents} agents, {questions} questions, {answered} answered, \
{points} GraphQL points"
    )
}

/// The per-session info line, naming the branch and both sides of the move.
/// Raised where a result is applied to the badge, so the log says what the user
/// actually sees.
pub(crate) fn format_pr_change(
    branch: &str,
    previous: Option<&PrInfo>,
    fresh: Option<&PrInfo>,
) -> String {
    let old = describe_pr_badge(previous);
    let new = describe_pr_badge(fresh);
    format!("Pull request for branch {branch} changed: {old} -> {new}")
}

fn describe_pr_badge(pr: Option<&PrInfo>) -> String {
    pr.map_or_else(
        || "none".to_string(),
        |p| format!("#{} {}", p.number, pr_state_word(&p.state)),
    )
}

/// Whether a discovery node is a pull request of the repository the alias was
/// asked under.
///
/// It has to be asked, because `associatedPullRequests` is NOT confined to that
/// repository: a fork's pull request whose head is this very ref is associated
/// with it too (measured on golang/go, whose ref carries pull requests from
/// several forks). Such a node is another repository's pull request, and the
/// window plus the newest-and-open-first rule would actively prefer it, storing
/// a foreign number against this session's repo.
///
/// A node that does not name a repository at all is kept: the query asks for the
/// name, so an answer without one is a server that did not say rather than a
/// foreign pull request, and dropping it would wipe a badge on a guess.
fn node_belongs_to(node: &serde_json::Value, owner_repo: &str) -> bool {
    node.get("repository")
        .and_then(|r| r.get("nameWithOwner"))
        .and_then(|v| v.as_str())
        .is_none_or(|name| name.eq_ignore_ascii_case(owner_repo))
}

/// Pick the one pull request a branch's discovery window is about.
///
/// The window is whatever GitHub returned, and nothing about its order can be
/// relied on (the `orderBy` argument on this connection is ignored, measured),
/// so the choice is made here rather than by taking an element:
///   - an OPEN pull request wins over a merged or closed one, whatever the
///     numbers: it is the one somebody is working on right now
///   - among pull requests in the same standing, the highest number wins, which
///     is the most recent one on a branch name that has been reused
fn select_discovered_pr(nodes: Vec<PrInfo>) -> Option<PrInfo> {
    nodes
        .into_iter()
        .max_by_key(|pr| (pr.state == PrState::Open, pr.number))
}

/// The pull requests a head-name lookup found for a branch whose ref is gone.
///
/// A head name is not an identity: branch names are reused, so an older pull
/// request from an earlier branch of the same name answers the lookup just as
/// well. A node is accepted only when every one of these holds:
///   - its head repository is the queried repository, not a fork (a node whose
///     head repository is gone cannot be shown to be this one);
///   - it was opened after the branch was born, give or take
///     [`HEAD_NAME_CLOCK_SLACK`], when dux knows when that was;
///   - its head commit is provably the branch's (`head_commit_is_the_branchs`,
///     which refuses a commit this clone does not have).
fn select_head_name_matches(
    question: &Question,
    name: &str,
    identity_at: Option<chrono::DateTime<chrono::Utc>>,
    evidence: &HeadEvidence,
    nodes: &[serde_json::Value],
    head_commit_is_the_branchs: &dyn Fn(&HeadEvidence, &str) -> bool,
) -> Vec<PrInfo> {
    let owner_repo = question.owner_repo();
    nodes
        .iter()
        .filter_map(|node| {
            let head_repo = node
                .get("headRepository")
                .and_then(|r| r.get("nameWithOwner"))
                .and_then(|v| v.as_str())?;
            if !head_repo.eq_ignore_ascii_case(&owner_repo) {
                return None;
            }
            let mut pr = parse_pr_json_value(node, &question.host, &owner_repo)?;
            let created_at = pr.gh_created_at?;
            if identity_at.is_some_and(|born| created_at < born - HEAD_NAME_CLOCK_SLACK) {
                return None;
            }
            let head_oid = node.get("headRefOid").and_then(|v| v.as_str())?;
            if !head_commit_is_the_branchs(evidence, head_oid) {
                return None;
            }
            if pr.head_branch.is_none() {
                pr.head_branch = Some(name.to_string());
            }
            logger::debug(&format!(
                "[gh-integration] branch {name} is gone on {owner_repo}; adopted PR #{} found \
                 by head name, whose head commit is the branch's",
                pr.number,
            ));
            Some(pr)
        })
        .collect()
}

/// The production check for [`select_head_name_matches`]: whether `head_oid`
/// is the branch's recorded tip, is in the history of its local branch, or is
/// an ancestor of its recorded tip. Runs git, which is fine here because the
/// whole sync runs on a background worker.
fn head_commit_is_the_branchs(evidence: &HeadEvidence, head_oid: &str) -> bool {
    let worktree = Path::new(&evidence.worktree_path);
    evidence.tip.as_deref() == Some(head_oid)
        || git::commit_is_in_branch_history(worktree, head_oid, &evidence.local_branch)
        || evidence
            .tip
            .as_deref()
            .is_some_and(|tip| git::commit_is_ancestor_of(worktree, head_oid, tip))
}

/// Extract the `rateLimit` snapshot from a GraphQL `data` object.
fn parse_rate_limit(data: &serde_json::Value) -> Option<RateLimitInfo> {
    let rl = data.get("rateLimit")?;
    let remaining = rl.get("remaining")?.as_i64()?;
    let reset_at = rl
        .get("resetAt")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&chrono::Utc));
    Some(RateLimitInfo {
        remaining,
        reset_at,
        cost: rl.get("cost").and_then(serde_json::Value::as_i64),
    })
}

/// Reconstruct a `PrInfo` from stored data without a network call.
/// Used wherever a call is skipped or failed and the stored row is the answer.
/// Reconstruct a live [`PrInfo`] from a stored PR row's decoded state, or `None`
/// when the stored state string is unrecognized. Shared by the PR-sync
/// reconstruction paths and by `Engine::seed_pull_requests_from_store` (startup PR
/// badge seeding), so the "OPEN"/"MERGED"/"CLOSED" decode lives in exactly one
/// place instead of being re-implemented per surface.
pub(crate) fn reconstruct_pr_from_stored(stored: &StoredPr) -> Option<PrInfo> {
    reconstruct_from_stored(stored)
}

fn reconstruct_from_stored(stored: &StoredPr) -> Option<PrInfo> {
    let state = match stored.state.as_str() {
        "MERGED" => PrState::Merged,
        "CLOSED" => PrState::Closed,
        "OPEN" => PrState::Open,
        _ => return None,
    };
    Some(PrInfo {
        number: stored.pr_number,
        state,
        // Stored rows may predate the stripping at the gh boundary.
        title: crate::bidi::strip_bidi_controls(&stored.title),
        host: stored.host.clone(),
        owner_repo: stored.owner_repo.clone(),
        url: stored.url.clone(),
        ..PrInfo::default()
    })
}

/// Parse a single PR JSON object. Test-only helper.
#[cfg(test)]
fn parse_pr_json_object(json: &str, host: &str, owner_repo: &str) -> Option<PrInfo> {
    let obj: serde_json::Value = serde_json::from_str(json).ok()?;
    parse_pr_json_value(&obj, host, owner_repo)
}

/// Extract `PrInfo` from a `serde_json::Value`. The repository the node names
/// gives the display case; `owner_repo` is the fallback when it names none.
fn parse_pr_json_value(obj: &serde_json::Value, host: &str, owner_repo: &str) -> Option<PrInfo> {
    let number = obj.get("number")?.as_u64()?;
    let state_str = obj.get("state")?.as_str()?;
    let owner_repo = obj
        .get("repository")
        .and_then(|r| r.get("nameWithOwner"))
        .and_then(|v| v.as_str())
        .filter(|name| name.eq_ignore_ascii_case(owner_repo))
        .unwrap_or(owner_repo);
    // Someone else's text: see `crate::bidi` for why its display-order
    // controls never get past this point.
    let title =
        crate::bidi::strip_bidi_controls(obj.get("title").and_then(|v| v.as_str()).unwrap_or(""));
    let url = obj
        .get("url")
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .map_or_else(
            || pull_request_url(host, owner_repo, number),
            str::to_string,
        );
    let state = match state_str {
        "OPEN" => PrState::Open,
        "MERGED" => PrState::Merged,
        "CLOSED" => PrState::Closed,
        _ => return None,
    };
    let time = |key: &str| {
        obj.get(key)
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|at| at.with_timezone(&chrono::Utc))
    };

    Some(PrInfo {
        number,
        state,
        is_draft: obj
            .get("isDraft")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        title,
        host: normalize_github_host(host).to_string(),
        owner_repo: owner_repo.to_string(),
        url,
        head_branch: obj
            .get("headRefName")
            .and_then(|v| v.as_str())
            .filter(|name| !name.is_empty())
            .map(str::to_string),
        gh_created_at: time("createdAt"),
        gh_updated_at: time("updatedAt"),
        gh_closed_at: time("closedAt"),
        gh_merged_at: time("mergedAt"),
    })
}

pub(crate) fn pull_request_url(host: &str, owner_repo: &str, number: u64) -> String {
    let host = normalize_github_host(host);
    format!("https://{host}/{owner_repo}/pull/{number}")
}

/// The `--repo` argument for a `gh` call: ALWAYS `host/owner/repo`, including
/// for github.com.
///
/// A bare `owner/repo` makes `gh` resolve the host from its own default, and the
/// `GH_HOST` environment variable overrides that resolution, so a user who
/// exports `GH_HOST` pointing at their company server had dux's github.com
/// lookups quietly sent there instead. Naming the host is also what makes the
/// per-host authentication check meaningful: the host dux qualified is then the
/// host `gh` actually contacts. Verified against gh 2.95.0: the host-qualified
/// form is accepted and ignores a conflicting `GH_HOST`; the bare form does not.
pub(crate) fn gh_repo_arg(host: &str, owner_repo: &str) -> String {
    let host = normalize_github_host(host);
    format!("{host}/{owner_repo}")
}

/// Argument list for the bounded `gh pr view` lookup. Split out of
/// [`run_pull_request_lookup_job`] so the exact argv dux builds can be asserted
/// against a stand-in `gh`, without a network call or a real login.
pub(crate) fn pr_view_args(host: &str, owner_repo: &str, number: u64) -> Vec<String> {
    vec![
        "pr".to_string(),
        "view".to_string(),
        number.to_string(),
        "--repo".to_string(),
        gh_repo_arg(host, owner_repo),
        "--json".to_string(),
        "number,title,state,headRefName,headRefOid".to_string(),
    ]
}

/// The exact host spelling the sync planner produces: empty means github.com,
/// and hostnames compare (and are stored) lowercased. `apply_pr_attach` stores
/// pins through this so a stored pin is byte-identical to what the planner
/// derives; an unnormalized host would never match and the pin would never
/// refresh.
pub(crate) fn normalized_github_host(host: &str) -> String {
    normalize_github_host(host).to_ascii_lowercase()
}

fn normalize_github_host(host: &str) -> &str {
    if host.trim().is_empty() {
        "github.com"
    } else {
        host
    }
}

/// Parse a user-typed PR reference into a [`PullRequestLookup`] for the selected
/// project's GitHub remote.
///
/// The grammar lives in [`crate::pr_reference`], which is deliberately separate
/// from the parser for a project's CONFIGURED address (see that module's docs
/// for why the two rules differ). This function is what turns a parsed
/// reference into a lookup against a project the caller has already chosen, and
/// it owns the three things that need the project to answer:
///
/// * **The host gate.** A host the user WROTE is the SECOND place a host can
///   enter dux, qualified separately from a project's configured address so
///   that fixing only the other gate would leave an enterprise user able to
///   have their project recognised but not their pasted URL. Both ask the same
///   policy, and both compare lowercased. A reference naming no host (`#123`,
///   `owner/repo#123`) has nothing to gate; it inherits the selected project's
///   host, which was itself qualified when it was read.
/// * **The mismatch refusal.** A reference naming another repository is
///   rejected with an actionable error, since fetching another repository's PR
///   head into this project's worktree would silently do the wrong thing.
/// * **The missing number.** A repository address on its own names no pull
///   request, so it is refused rather than guessed at.
///
/// A pure function, shared by the TUI's new-agent-from-pr prompt and the web's
/// `CreateAgentFromPr` wire flow.
pub(crate) fn parse_pull_request_lookup(
    raw_input: &str,
    selected_host: &str,
    selected_owner_repo: &str,
    policy: &GithubHostPolicy,
) -> Result<PullRequestLookup, StatusText> {
    let reference = crate::pr_reference::parse_typed_reference(raw_input)?;

    if let Some(host) = reference.host.as_deref()
        && !policy.allows(host)
    {
        return Err(host_not_signed_in(host));
    }

    if reference.owner_repo.is_some() && !reference.matches(selected_host, selected_owner_repo) {
        let named = reference.repository_label().unwrap_or_default();
        return Err(format!(
            "PR belongs to {named}, but the selected project uses \
             {selected_host}/{selected_owner_repo}."
        )
        .into());
    }

    let Some(number) = reference.number else {
        let named = reference
            .repository_label()
            .unwrap_or_else(|| selected_owner_repo.to_string());
        return Err(format!(
            "That address names {named} but no pull request. Add the number, for example \
             {selected_owner_repo}#123."
        )
        .into());
    };

    Ok(PullRequestLookup {
        host: selected_host.to_string(),
        owner_repo: selected_owner_repo.to_string(),
        number,
    })
}

/// Resolve a PR reference for a project against the GitHub remote and `gh` CLI,
/// posting [`WorkerEvent::PullRequestResolved`] with the outcome. Runs on a
/// background thread (it parses the project remote, then shells out to
/// `gh pr view`). Shared by the TUI's `dispatch_pull_request_lookup` and the
/// web's `CreateAgentFromPr` flow so both surfaces resolve PRs identically.
///
/// `custom_name` carries a caller-supplied display name through to the resolved
/// PR (`None` for the TUI, which prompts for a name after resolution; `Some` for
/// the web, which sends the name upfront).
#[expect(
    clippy::too_many_arguments,
    reason = "every input of the lookup job, handed over once by the surface that starts it"
)]
pub fn run_pull_request_lookup_job(
    project: Project,
    raw_input: &str,
    custom_name: Option<String>,
    review_copy: crate::worker::ReviewCopyConsent,
    ask_id: Option<String>,
    worker_tx: &Sender<WorkerEvent>,
    status_op_id: Option<String>,
    policy: &GithubHostPolicy,
) {
    let lookup = match git::remote_github_repo(Path::new(&project.path), policy) {
        Some(remote) => {
            parse_pull_request_lookup(raw_input, &remote.host, &remote.owner_repo, policy)
        }
        None => Err(format!(
            "Project \"{}\" does not have a GitHub origin remote.",
            project.name
        )
        .into()),
    };
    let lookup = match lookup {
        Ok(lookup) => lookup,
        Err(message) => {
            let _ = worker_tx.send(WorkerEvent::PullRequestResolved {
                result: Err(message),
                purpose: PrLookupPurpose::CreateAgent,
                status_op_id,
            });
            return;
        }
    };

    let result = look_up_pull_request(OsStr::new("gh"), &lookup)
        .map_err(StatusText::from)
        .map(|metadata| {
            resolved_pull_request(metadata, project, &lookup, custom_name, review_copy, ask_id)
        });
    let _ = worker_tx.send(WorkerEvent::PullRequestResolved {
        result,
        purpose: PrLookupPurpose::CreateAgent,
        status_op_id,
    });
}

/// Parse a user-typed PR reference into a lookup for a MANUAL ATTACH to an
/// existing session. Deliberately NOT [`parse_pull_request_lookup`], whose
/// project-match refusal is correct for the create flow (fetching another
/// repository's head into this project's worktree would be wrong) and exactly
/// backwards here: the whole point of a manual attach is that the PR may live
/// under any head ref or another repository (a fork). Shares the same
/// [`crate::pr_reference::parse_typed_reference`] grammar.
///
/// * A reference NAMING a repository is taken at its word: the host it typed
///   (or the session's project remote host when it typed none, else
///   github.com) is gated through the policy, and the lookup targets the typed
///   `(host, owner_repo, number)`. No project-match check.
/// * A BARE number resolves against the session's project remote, and the
///   three real failure modes are surfaced by name: no GitHub origin (or an
///   unreadable remote), and a policy-denied host. Bare numbers work exactly
///   when that remote resolves.
///
/// Pure over the pre-resolved `resolution`, so every input form is testable
/// without git.
pub(crate) fn parse_attach_pull_request_lookup(
    raw_input: &str,
    resolution: &git::RemoteResolution,
    project_name: &str,
    policy: &GithubHostPolicy,
) -> Result<PullRequestLookup, StatusText> {
    let reference = crate::pr_reference::parse_typed_reference(raw_input)?;

    if let Some(owner_repo) = reference.owner_repo.clone() {
        let host = match reference.host.as_deref() {
            Some(host) => host.to_string(),
            // `owner/repo#123` names no host; it inherits the project's
            // (already-qualified) remote host, else github.com.
            None => match resolution {
                git::RemoteResolution::Allowed(remote) => remote.host.clone(),
                git::RemoteResolution::Denied | git::RemoteResolution::Unresolved => {
                    "github.com".to_string()
                }
            },
        };
        let host = host.to_ascii_lowercase();
        if !policy.allows(&host) {
            return Err(host_not_signed_in(&host));
        }
        let Some(number) = reference.number else {
            return Err(format!(
                "That address names {owner_repo} but no pull request. Add the number, \
                 for example {owner_repo}#123."
            )
            .into());
        };
        return Ok(PullRequestLookup {
            host,
            owner_repo,
            number,
        });
    }

    let Some(number) = reference.number else {
        // Unreachable through the parser (a reference without a repository is
        // always a bare number), kept as a refusal rather than a panic.
        return Err(
            "Enter a pull request URL, owner/repo#123, or a PR number. A repository \
                    address works too."
                .into(),
        );
    };
    match resolution {
        git::RemoteResolution::Allowed(remote) => Ok(PullRequestLookup {
            host: remote.host.clone(),
            owner_repo: remote.owner_repo.clone(),
            number,
        }),
        git::RemoteResolution::Denied => Err(crate::status_text![
            "Project ",
            q(project_name),
            "'s remote is on a host dux is not signed in to, so a bare PR number cannot be \
             resolved. Sign in with ",
            q("gh auth login --hostname <host>"),
            ", or paste the full PR URL instead."
        ]),
        git::RemoteResolution::Unresolved => Err(format!(
            "Project \"{project_name}\" does not have a GitHub origin remote, so a bare PR \
             number cannot be resolved. Paste the full PR URL (or owner/repo#{number}) \
             instead."
        )
        .into()),
    }
}

/// The refusal for a pull request on a host `gh` is not signed in to, naming
/// the command that signs in as a chip.
pub fn host_not_signed_in(host: &str) -> StatusText {
    crate::status_text![
        format!("dux cannot look up pull requests on {host}. Sign in to that host with "),
        q(format!("gh auth login --hostname {host}")),
        ", or paste a reference from a host you are already signed in to."
    ]
}

/// Resolve a PR reference for a MANUAL ATTACH and post
/// [`WorkerEvent::PullRequestResolved`] with `purpose: Attach`. Runs on a
/// background thread (it reads the project's remote, then shells out to
/// `gh pr view`). Shared by the TUI's attach modal and the web's
/// `PUT /sessions/:id/pull-request` handler through
/// [`crate::engine::Engine::dispatch_attach_pull_request`], so both surfaces
/// resolve attaches identically. `project` is the SESSION's project: its
/// remote is what a bare number (or a host-less `owner/repo#123`) resolves
/// against.
pub(crate) fn run_attach_pull_request_lookup_job(
    project: Project,
    session_id: String,
    raw_input: &str,
    worker_tx: &Sender<WorkerEvent>,
    status_op_id: Option<String>,
    policy: &GithubHostPolicy,
) {
    let resolution = git::resolve_remote_github_repo(Path::new(&project.path), policy);
    let purpose = PrLookupPurpose::Attach { session_id };
    let lookup =
        match parse_attach_pull_request_lookup(raw_input, &resolution, &project.name, policy) {
            Ok(lookup) => lookup,
            Err(message) => {
                let _ = worker_tx.send(WorkerEvent::PullRequestResolved {
                    result: Err(message),
                    purpose,
                    status_op_id,
                });
                return;
            }
        };

    let project_remote = match resolution {
        git::RemoteResolution::Allowed(remote) => Some(git::GitHubRemote {
            host: normalized_github_host(&remote.host),
            owner_repo: remote.owner_repo,
        }),
        git::RemoteResolution::Denied | git::RemoteResolution::Unresolved => None,
    };
    let result = look_up_pull_request(OsStr::new("gh"), &lookup)
        .map_err(StatusText::from)
        .map(|metadata| crate::worker::ResolvedPullRequest {
            project_remote,
            ..resolved_pull_request(
                metadata,
                project,
                &lookup,
                None,
                crate::worker::ReviewCopyConsent::default(),
                None,
            )
        });
    let _ = worker_tx.send(WorkerEvent::PullRequestResolved {
        result,
        purpose,
        status_op_id,
    });
}

/// A pull request as `gh pr view` reports it, before any project is attached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PullRequestMetadata {
    pub number: u64,
    /// With the bidirectional-text controls stripped.
    pub title: String,
    pub state: String,
    pub head_ref_name: String,
    /// The head commit, when `gh` reported one.
    pub(crate) head_ref_oid: Option<String>,
}

/// Run `gh pr view` through `program` for `lookup` and read its answer. Needs no
/// project: callers that already hold one attach it with
/// [`resolved_pull_request`]. Bounded by [`GH_CALL_TIMEOUT`] so a hung call
/// (stalled network, credential prompt) cannot strand a busy status forever.
pub(crate) fn look_up_pull_request(
    program: &OsStr,
    lookup: &PullRequestLookup,
) -> Result<PullRequestMetadata, String> {
    let args = pr_view_args(&lookup.host, &lookup.owner_repo, lookup.number);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    match run_program_with_timeout(program, &arg_refs, GH_CALL_TIMEOUT) {
        GhCallOutcome::Completed(output) if output.status.success() => {
            parse_pull_request_metadata_json(&String::from_utf8_lossy(&output.stdout))
        }
        GhCallOutcome::Completed(output) => Err(format!(
            "Failed to resolve PR #{} from {}: {}",
            lookup.number,
            lookup.owner_repo,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        GhCallOutcome::TimedOut => Err(format!(
            "gh pr view timed out after {}s resolving PR #{} from {}.",
            GH_CALL_TIMEOUT.as_secs(),
            lookup.number,
            lookup.owner_repo,
        )),
        GhCallOutcome::Failed(msg) | GhCallOutcome::NotFound(msg) => {
            Err(format!("Failed to run gh pr view: {msg}"))
        }
    }
}

fn resolved_pull_request(
    metadata: PullRequestMetadata,
    project: Project,
    lookup: &PullRequestLookup,
    custom_name: Option<String>,
    review_copy: crate::worker::ReviewCopyConsent,
    ask_id: Option<String>,
) -> ResolvedPullRequest {
    ResolvedPullRequest {
        project,
        host: lookup.host.clone(),
        owner_repo: lookup.owner_repo.clone(),
        number: metadata.number,
        title: metadata.title,
        state: metadata.state,
        head_ref_name: metadata.head_ref_name,
        custom_name,
        review_copy,
        ask_id,
        project_remote: None,
    }
}

fn parse_pull_request_metadata_json(json: &str) -> Result<PullRequestMetadata, String> {
    let obj: serde_json::Value = serde_json::from_str(json.trim())
        .map_err(|err| format!("gh returned invalid PR JSON: {err}"))?;
    let number = obj
        .get("number")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "gh PR response did not include a PR number.".to_string())?;
    let title =
        crate::bidi::strip_bidi_controls(obj.get("title").and_then(|v| v.as_str()).unwrap_or(""));
    let state = obj
        .get("state")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let head_ref_name = obj
        .get("headRefName")
        .and_then(|v| v.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "gh PR response did not include a head branch.".to_string())?
        .to_string();
    let head_ref_oid = obj
        .get("headRefOid")
        .and_then(|v| v.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);
    Ok(PullRequestMetadata {
        number,
        title,
        state,
        head_ref_name,
        head_ref_oid,
    })
}

/// The default branch of `host/owner_repo`, asked of `gh repo view` through
/// `program` under the same bound as the pull request lookup. Needs no project.
pub(crate) fn look_up_default_branch(
    program: &OsStr,
    host: &str,
    owner_repo: &str,
) -> Result<String, String> {
    let repo = gh_repo_arg(host, owner_repo);
    let args = ["repo", "view", repo.as_str(), "--json", "defaultBranchRef"];
    match run_program_with_timeout(program, &args, GH_CALL_TIMEOUT) {
        GhCallOutcome::Completed(output) if output.status.success() => {
            let json: serde_json::Value =
                serde_json::from_str(String::from_utf8_lossy(&output.stdout).trim())
                    .map_err(|err| format!("gh returned invalid repository JSON: {err}"))?;
            json.get("defaultBranchRef")
                .and_then(|branch| branch.get("name"))
                .and_then(|name| name.as_str())
                .filter(|name| !name.trim().is_empty())
                .map(str::to_string)
                .ok_or_else(|| {
                    format!(
                        "gh repository response did not name a default branch for {owner_repo}."
                    )
                })
        }
        GhCallOutcome::Completed(output) => Err(format!(
            "Failed to look up the default branch of {owner_repo}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        GhCallOutcome::TimedOut => Err(format!(
            "gh repo view timed out after {}s looking up the default branch of {owner_repo}.",
            GH_CALL_TIMEOUT.as_secs()
        )),
        GhCallOutcome::Failed(msg) | GhCallOutcome::NotFound(msg) => {
            Err(format!("Failed to run gh repo view: {msg}"))
        }
    }
}

/// Stand-in `gh` builders, shared by the probe's own tests and by the wiring
/// tests on both surfaces. A stand-in is preferred over mocking dux's internals:
/// it exercises the real two-call protocol, and it needs neither the network,
/// nor a real login, nor a mutation of the test process's `PATH`.
///
/// This is the ONE copy. It is reachable from another crate's integration tests
/// (which are external crates, so `pub(crate)` and a plain `#[cfg(test)]` would
/// hide it) through the `test-support` cargo feature, which `dux-tui` and
/// `dux-web` turn on as a dev-dependency only. A normal `cargo build` of either
/// surface leaves the feature off, so none of this reaches a shipped binary.
#[cfg(any(test, feature = "test-support"))]
pub mod probe_test_support {
    use std::path::{Path, PathBuf};

    /// The first argument the stand-in answers to, before its body runs. See
    /// [`stand_in_gh`] for why it exists.
    const WARMUP_ARG: &str = "--dux-stand-in-warmup";

    /// Write an executable `/bin/sh` stand-in for `gh` into `dir`.
    ///
    /// The script is exec'd once here before it is returned, because a freshly
    /// written executable can be refused with `ETXTBSY` ("Text file busy") in a
    /// multi-threaded process: any other test forking while this write's file
    /// descriptor is open inherits it, and until that child execs, the kernel
    /// sees an open write handle on this file and refuses to run it. That is a
    /// real, observed flake, not a theoretical one. The warmup swallows the
    /// window, and it answers [`WARMUP_ARG`] by exiting before the body so a
    /// stand-in that records its calls does not record this one.
    ///
    /// # Panics
    ///
    /// Panics when the stand-in cannot be written or made executable, or stays busy for a second.
    pub fn stand_in_gh(dir: &Path, body: &str) -> PathBuf {
        let script = dir.join("fake-gh");
        std::fs::write(
            &script,
            format!("#!/bin/sh\ncase \"$1\" in {WARMUP_ARG}) exit 0 ;; esac\n{body}\n"),
        )
        .expect("write stand-in gh");
        std::fs::set_permissions(
            &script,
            <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
        )
        .expect("chmod stand-in gh");
        for _ in 0..200 {
            match std::process::Command::new(&script).arg(WARMUP_ARG).status() {
                Ok(_) => return script,
                Err(err) if err.raw_os_error() == Some(26) => {
                    // ETXTBSY. The inheriting child has not exec'd yet.
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(err) => panic!("stand-in gh is not runnable: {err}"),
            }
        }
        panic!("stand-in gh stayed busy for a second; something is holding it open");
    }

    /// A stand-in whose machine-readable answer reports each of `hosts` with a
    /// successful active account, and which fails the plain call (so a test can
    /// tell the two modes apart).
    pub fn stand_in_gh_serving(dir: &Path, hosts: &[&str]) -> PathBuf {
        let entries: Vec<String> = hosts
            .iter()
            .map(|h| format!(r#""{h}":[{{"state":"success","active":true,"host":"{h}"}}]"#))
            .collect();
        let json = format!(r#"{{"hosts":{{{}}}}}"#, entries.join(","));
        stand_in_gh(
            dir,
            &format!("case \"$*\" in\n  *--json*) printf '%s' '{json}' ;;\n  *) exit 1 ;;\nesac\n"),
        )
    }

    /// A stand-in that tells the two calls dux makes apart by their first
    /// word, which `stand_in_gh_serving` cannot: both carry `--json`.
    /// `auth status ...` answers each of `hosts` (host, `gitProtocol`) with a
    /// successful active account; `pr view ...` prints `pull_request` when it
    /// is `Ok`, and exits 1 with the message on stderr when it is `Err`.
    pub fn stand_in_gh_with_pull_request(
        dir: &Path,
        hosts: &[(&str, &str)],
        pull_request: Result<&str, &str>,
    ) -> PathBuf {
        stand_in_gh_with_pull_request_and_repository(
            dir,
            hosts,
            pull_request,
            Err("no repository answers here"),
        )
    }

    /// A stand-in that answers only `repo view ...`: `repository` is printed
    /// when it is `Ok`, and is a message on stderr with a failing exit when it
    /// is `Err`.
    pub fn stand_in_gh_with_repository(dir: &Path, repository: Result<&str, &str>) -> PathBuf {
        stand_in_gh_with_pull_request_and_repository(dir, &[], Err("not found"), repository)
    }

    /// [`stand_in_gh_with_pull_request`] that also answers `repo view ...`.
    pub fn stand_in_gh_with_pull_request_and_repository(
        dir: &Path,
        hosts: &[(&str, &str)],
        pull_request: Result<&str, &str>,
        repository: Result<&str, &str>,
    ) -> PathBuf {
        let entries: Vec<String> = hosts
            .iter()
            .map(|(h, protocol)| {
                format!(
                    r#""{h}":[{{"state":"success","active":true,"host":"{h}","gitProtocol":"{protocol}"}}]"#
                )
            })
            .collect();
        let auth = format!(r#"{{"hosts":{{{}}}}}"#, entries.join(","));
        let answer = |outcome: Result<&str, &str>| match outcome {
            Ok(json) => format!("printf '%s' '{json}'"),
            Err(message) => format!("echo '{message}' >&2; exit 1"),
        };
        let (pr, repo) = (answer(pull_request), answer(repository));
        stand_in_gh(
            dir,
            &format!(
                "case \"$1\" in\n  auth) printf '%s' '{auth}' ;;\n  pr) {pr} ;;\n  repo) {repo} ;;\n  *) exit 1 ;;\nesac\n"
            ),
        )
    }
}

#[cfg(test)]
mod host_policy_tests {
    use super::probe_test_support::stand_in_gh;
    use super::*;

    /// The real shape, copied from `gh auth status --active --json hosts` run
    /// against gh 2.95.0 on a logged-in machine (token fields are absent from
    /// this output; nothing secret is reproduced here).
    const MEASURED_GH_2_95_OUTPUT: &str = r#"{"hosts":{"github.com":[{"active":true,"gitProtocol":"ssh","host":"github.com","login":"someone","scopes":["gist","read:org","repo"],"state":"success","tokenSource":"keyring"}]}}"#;

    fn hosts(policy: &GhProbe) -> Vec<String> {
        match policy {
            GhProbe::Decided {
                policy: GithubHostPolicy::Hosts(h),
                ..
            } => h.iter().cloned().collect(),
            other => panic!("expected a machine-readable host set, got {other:?}"),
        }
    }

    #[test]
    fn a_host_qualifies_only_when_its_active_account_reports_success() {
        let parsed = parse_auth_status_hosts(MEASURED_GH_2_95_OUTPUT).expect("expected shape");
        assert_eq!(
            parsed.iter().cloned().collect::<Vec<_>>(),
            vec!["github.com".to_string()],
        );

        let errored =
            r#"{"hosts":{"github.com":[{"state":"error","active":true,"host":"github.com"}]}}"#;
        assert!(
            parse_auth_status_hosts(errored)
                .expect("expected shape")
                .is_empty(),
            "a host whose active account errors must not qualify",
        );

        let protocols = parse_auth_status(MEASURED_GH_2_95_OUTPUT)
            .expect("expected shape")
            .protocols;
        assert_eq!(
            protocols,
            BTreeMap::from([("github.com".to_string(), GitProtocol::Ssh)]),
        );
        let no_field =
            r#"{"hosts":{"GHE.example":[{"state":"success","active":true,"host":"ghe.example"}]}}"#;
        assert_eq!(
            parse_auth_status(no_field)
                .expect("expected shape")
                .protocols,
            BTreeMap::from([("ghe.example".to_string(), GitProtocol::Https)]),
        );
        let other = r#"{"hosts":{"github.com":[{"state":"success","active":true,"host":"github.com","gitProtocol":"https"}]}}"#;
        assert_eq!(
            parse_auth_status(other).expect("expected shape").protocols,
            BTreeMap::from([("github.com".to_string(), GitProtocol::Https)]),
        );
    }

    #[test]
    fn a_host_never_qualifies_on_its_name_alone() {
        // `gh` lists every host it merely KNOWS. A `github.`-prefixed name in
        // the map is not evidence the host works.
        let known_but_broken = r#"{"hosts":{"github.enterprise.example":[{"state":"error","active":true,"host":"github.enterprise.example"}]}}"#;
        let parsed = parse_auth_status_hosts(known_but_broken).expect("expected shape");
        assert!(
            parsed.is_empty(),
            "a `github.`-named host that never succeeded must not qualify, got {parsed:?}",
        );
    }

    #[test]
    fn a_broken_active_account_is_not_rescued_by_a_working_sibling() {
        // Every call dux makes names a host and never an account, so `gh` uses
        // that host's ACTIVE account. A host whose active login is broken must
        // not qualify because some other account on it happens to work.
        let mixed = r#"{"hosts":{"git.company.example":[
            {"state":"error","active":true,"host":"git.company.example","login":"work"},
            {"state":"success","active":false,"host":"git.company.example","login":"personal"}
        ]}}"#;
        let parsed = parse_auth_status_hosts(mixed).expect("expected shape");
        assert!(
            parsed.is_empty(),
            "only the active account's state counts, got {parsed:?}",
        );
    }

    /// A record dux cannot read IN FULL is not evidence of anything, so the
    /// answer containing it is not an answer.
    ///
    /// This used to be asserted with `unwrap_or_default()`, which masked the
    /// outcome it was supposed to be about: a missing or null `active` DID
    /// parse, into an empty host set, and an empty host set is a DECISIVE "gh
    /// serves nothing" that replaces the last known good policy and turns every
    /// GitHub feature off. A wrongly typed one failed to parse and was
    /// transient. The test could not tell those apart because it defaulted the
    /// one into the other. They are now the same thing, and it asserts which.
    ///
    /// A missing or null `host` was worse: the record said nothing about which
    /// host it describes, and the map KEY was taken as qualified on the
    /// strength of it.
    #[test]
    fn a_record_dux_cannot_read_in_full_is_not_an_answer() {
        for malformed in [
            // `active` absent, null, or not a boolean.
            r#"{"hosts":{"github.com":[{"state":"success","host":"github.com"}]}}"#,
            r#"{"hosts":{"github.com":[{"state":"success","active":null,"host":"github.com"}]}}"#,
            r#"{"hosts":{"github.com":[{"state":"success","active":"true","host":"github.com"}]}}"#,
            r#"{"hosts":{"github.com":[{"state":"success","active":1,"host":"github.com"}]}}"#,
            // `host` absent or null: the record does not say what it describes.
            r#"{"hosts":{"github.com":[{"state":"success","active":true}]}}"#,
            r#"{"hosts":{"github.com":[{"state":"success","active":true,"host":null}]}}"#,
            // `state` absent or null: the record does not say whether it works.
            r#"{"hosts":{"github.com":[{"active":true,"host":"github.com"}]}}"#,
            r#"{"hosts":{"github.com":[{"state":null,"active":true,"host":"github.com"}]}}"#,
            // One unreadable record spoils the answer even alongside a good
            // one, because dux cannot know what it was going to say.
            r#"{"hosts":{"github.com":[{"state":"success","active":true,"host":"github.com"}],"git.company.example":[{"state":"success"}]}}"#,
        ] {
            assert_eq!(
                parse_auth_status_hosts(malformed),
                None,
                "a record dux cannot read must make the answer unreadable: {malformed}",
            );
        }
    }

    /// And what the caller does with that: an unreadable record is TRANSIENT,
    /// so the last known good policy stands. It must not be mistaken for a `gh`
    /// too old to answer, and it must not be mistaken for a decisive "gh serves
    /// no hosts", which is what an empty parsed set means.
    #[test]
    fn a_response_carrying_an_unreadable_record_is_transient() {
        let mut retried = false;
        let probe = decide_gh_probe(
            GhCallOutcome::Completed(completed(
                0,
                r#"{"hosts":{"github.com":[{"state":"success","host":"github.com"}]}}"#,
            )),
            || {
                retried = true;
                GhCallOutcome::Completed(completed(0, ""))
            },
        );
        assert!(!retried, "an unreadable record is not an older gh");
        assert!(
            matches!(probe, GhProbe::Transient(_)),
            "an unreadable record decides nothing, got {probe:?}",
        );

        // The contrast, so the assertion above is not just "anything odd is
        // transient": a WELL-FORMED record saying the account is not active, or
        // that its state is not success, is a decisive no for that host, and an
        // answer made only of those is a decisive empty set.
        for decisive in [
            r#"{"hosts":{"github.com":[{"state":"success","active":false,"host":"github.com"}]}}"#,
            r#"{"hosts":{"github.com":[{"state":"error","active":true,"host":"github.com"}]}}"#,
        ] {
            assert_eq!(
                decide_gh_probe(GhCallOutcome::Completed(completed(0, decisive)), || {
                    panic!("a parseable answer needs no fallback")
                }),
                GhProbe::Decided {
                    available: false,
                    policy: GithubHostPolicy::Hosts(BTreeSet::new()),
                    protocols: BTreeMap::default(),
                },
                "{decisive}",
            );
        }
    }

    /// With several accounts on one host, a broken active entry must not be
    /// rescued by a sibling, whether that sibling is well-formed and inactive
    /// or unreadable.
    #[test]
    fn several_accounts_on_one_host_still_need_one_exactly_active_success() {
        let broken_plus_inactive_sibling = r#"{"hosts":{"git.company.example":[
            {"state":"error","active":true,"host":"git.company.example","login":"work"},
            {"state":"success","active":false,"host":"git.company.example","login":"personal"}
        ]}}"#;
        assert_eq!(
            parse_auth_status_hosts(broken_plus_inactive_sibling)
                .expect("every record is well-formed, so this is an answer")
                .len(),
            0,
            "the active account errored and the sibling is not the active one",
        );

        // The same host with an UNREADABLE sibling is not an answer at all,
        // rather than an answer of "no". See
        // `a_record_dux_cannot_read_in_full_is_not_an_answer`.
        let broken_plus_unreadable_sibling = r#"{"hosts":{"git.company.example":[
            {"state":"error","active":true,"host":"git.company.example","login":"work"},
            {"state":"success","host":"git.company.example","login":"personal"}
        ]}}"#;
        assert_eq!(
            parse_auth_status_hosts(broken_plus_unreadable_sibling),
            None
        );

        // The same host DOES qualify when one of its accounts really is the
        // active one and really succeeded.
        let broken_plus_active_success = r#"{"hosts":{"git.company.example":[
            {"state":"error","active":false,"host":"git.company.example","login":"old"},
            {"state":"success","active":true,"host":"git.company.example","login":"work"}
        ]}}"#;
        assert_eq!(
            parse_auth_status_hosts(broken_plus_active_success)
                .expect("shape")
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["git.company.example".to_string()],
        );
    }

    /// The key is what dux would hand `gh`, so an entry describing a DIFFERENT
    /// host describes something else and cannot vouch for this one.
    #[test]
    fn an_entry_whose_host_disagrees_with_its_key_does_not_qualify() {
        let disagreeing = r#"{"hosts":{"git.company.example":[{"state":"success","active":true,"host":"github.com"}]}}"#;
        let parsed = parse_auth_status_hosts(disagreeing).expect("shape");
        assert!(
            parsed.is_empty(),
            "the entry vouches for github.com, not for the key it sits under, got {parsed:?}",
        );

        // Agreement is case-insensitive, like every other host comparison.
        let capitalised =
            r#"{"hosts":{"GitHub.com":[{"state":"success","active":true,"host":"github.com"}]}}"#;
        assert_eq!(
            parse_auth_status_hosts(capitalised)
                .expect("shape")
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["github.com".to_string()],
        );
    }

    /// The mode switch. Only `gh` saying it does not UNDERSTAND the call selects
    /// the permissive older-`gh` rule; a modern fatal failure, a truncated
    /// answer and valid JSON of the wrong shape are all transient, so the last
    /// known good policy stands rather than being replaced or widened.
    #[test]
    fn only_a_capability_diagnostic_selects_the_older_gh_mode() {
        // Measured on gh 2.95.0: an unsupported flag writes exactly this to
        // stderr, leaves stdout empty, and exits 1. That is how an older `gh`
        // rejecting `--json`/`--active` presents itself.
        let mut retried = false;
        let probe = decide_gh_probe(
            GhCallOutcome::Completed(completed_with_stderr(
                1,
                "",
                "unknown flag: --json\n\nUsage:  gh auth status [flags]\n",
            )),
            || {
                retried = true;
                GhCallOutcome::Completed(completed(0, ""))
            },
        );
        assert!(retried, "an unsupported flag is the older-gh case");
        assert_eq!(
            probe,
            GhProbe::Decided {
                available: true,
                policy: GithubHostPolicy::LegacyNameRule,
                protocols: BTreeMap::default(),
            },
        );

        // gh's own capability diagnostic for a field it cannot export, also
        // measured (`gh auth status --active --json bogusfield`).
        let mut retried = false;
        let probe = decide_gh_probe(
            GhCallOutcome::Completed(completed_with_stderr(
                1,
                "",
                "Unknown JSON field: \"hosts\"\nAvailable fields:\n",
            )),
            || {
                retried = true;
                GhCallOutcome::Completed(completed(0, ""))
            },
        );
        assert!(retried, "an unsupported JSON field is a capability answer");
        assert!(matches!(probe, GhProbe::Decided { .. }), "got {probe:?}");

        // Everything else that fails is TRANSIENT: it decides nothing, must not
        // reach the permissive name rule, and must not replace the last known
        // good policy.
        for (label, output) in [
            (
                "a modern fatal error in JSON mode",
                completed_with_stderr(
                    1,
                    "",
                    "error: could not read the config: permission denied\n",
                ),
            ),
            (
                "JSON written to stderr instead of stdout",
                completed_with_stderr(1, "", r#"{"hosts":{"github.com":[]}}"#),
            ),
            (
                "a truncated answer",
                completed_with_stderr(0, r#"{"hosts":{"github.com":["#, ""),
            ),
            (
                "valid JSON of the wrong shape",
                completed_with_stderr(0, r#"{"other":1}"#, ""),
            ),
            (
                "nothing at all, with a zero exit",
                completed_with_stderr(0, "", ""),
            ),
        ] {
            let mut retried = false;
            let probe = decide_gh_probe(GhCallOutcome::Completed(output), || {
                retried = true;
                GhCallOutcome::Completed(completed(0, ""))
            });
            assert!(!retried, "{label} must not reach the older-gh fallback");
            assert!(
                matches!(probe, GhProbe::Transient(_)),
                "{label} must be transient, got {probe:?}",
            );
        }
    }

    /// The journey the old name rule got wrong in both directions: a company
    /// server that works is rejected on the strength of its name, and a
    /// github.com whose login is broken is accepted on the strength of its.
    ///
    /// Availability and eligibility are separate answers here, which is the
    /// whole point: the interface says GitHub is available (one host works)
    /// while only that host's projects actually resolve.
    #[test]
    fn a_working_company_host_serves_its_projects_while_a_broken_github_com_does_not() {
        let answer = r#"{"hosts":{
            "git.company.example":[{"state":"success","active":true,"host":"git.company.example"}],
            "github.com":[{"state":"error","active":true,"host":"github.com"}]
        }}"#;
        let probe = decide_gh_probe(GhCallOutcome::Completed(completed(0, answer)), || {
            panic!("a parseable answer needs no fallback")
        });
        let GhProbe::Decided {
            available, policy, ..
        } = probe
        else {
            panic!("expected a decided probe, got {probe:?}");
        };
        assert!(available, "one working host means GitHub is available");

        assert_eq!(
            git::github_remote_from_git_output_with(
                b"git@git.company.example:acme/widget.git\n",
                &policy,
            ),
            Some(git::GitHubRemote {
                host: "git.company.example".to_string(),
                owner_repo: "acme/widget".to_string(),
            }),
            "a project on the working host resolves",
        );
        assert_eq!(
            git::github_remote_from_git_output_with(
                b"git@github.com:octocat/Hello-World.git\n",
                &policy,
            ),
            None,
            "and the broken host does not, even though it is literally github.com",
        );
        assert_eq!(
            git::github_remote_from_git_output_with(b"git@gitlab.com:acme/widget.git\n", &policy),
            None,
            "GitLab is still rejected: gh has never heard of it",
        );
    }

    /// An enterprise host has to clear BOTH gates. They are separate checks on
    /// separate inputs (a project's configured address, and a reference the user
    /// types), so fixing only one leaves a user whose project is recognised but
    /// whose pasted URL is not.
    #[test]
    fn an_enterprise_host_works_as_a_project_address_and_in_a_typed_reference() {
        let policy =
            GithubHostPolicy::Hosts(["git.company.example".to_string()].into_iter().collect());

        let remote = git::github_remote_from_git_output_with(
            b"git@GIT.Company.Example:acme/widget.git\n",
            &policy,
        )
        .expect("gate 1: the project's configured address");
        assert_eq!(remote.host, "git.company.example", "compared lowercased");

        let lookup = parse_pull_request_lookup(
            "https://git.company.example/acme/widget/pull/42",
            &remote.host,
            &remote.owner_repo,
            &policy,
        )
        .expect("gate 2: the reference the user types");
        assert_eq!(lookup.host, "git.company.example");
        assert_eq!(lookup.owner_repo, "acme/widget");
        assert_eq!(lookup.number, 42);

        // And a host the policy does not name is refused at gate 2 as well,
        // whatever its spelling.
        assert!(
            parse_pull_request_lookup(
                "https://github.enterprise.example/acme/widget/pull/42",
                "git.company.example",
                "acme/widget",
                &policy,
            )
            .is_err(),
            "a `github.`-named host gh cannot serve is not a way in",
        );
    }

    #[test]
    fn output_that_is_not_the_expected_shape_does_not_parse() {
        // How an older `gh` presents itself: it rejects the flag and writes
        // nothing usable to stdout.
        assert_eq!(parse_auth_status_hosts(""), None);
        assert_eq!(parse_auth_status_hosts("unknown flag: --json"), None);
        // Valid JSON of the wrong shape is still not an answer.
        assert_eq!(parse_auth_status_hosts(r#"{"other":1}"#), None);
    }

    #[test]
    fn policy_allows_exactly_the_hosts_gh_named() {
        let policy =
            GithubHostPolicy::Hosts(["git.company.example".to_string()].into_iter().collect());
        assert!(policy.allows("git.company.example"));
        assert!(policy.allows("GIT.Company.Example"), "compared lowercased");
        assert!(
            !policy.allows("github.com"),
            "nothing is unioned onto the answered set",
        );
        assert!(!policy.allows("gitlab.com"));
        assert!(!policy.allows(""));
    }

    /// The policy compares WHAT IT IS GIVEN. It used to trim first, which made
    /// it answer for a host other than the one it was asked about: with the
    /// parser preserving an interior space, `git@ git.company.example:o/r` was
    /// checked as `git.company.example`, allowed, and then handed onwards with
    /// the space still on it. Whitespace in a host is a defect in the caller or
    /// in the address, and the answer to a defect is no.
    #[test]
    fn the_policy_never_trims_the_host_it_is_asked_about() {
        let named =
            GithubHostPolicy::Hosts(["git.company.example".to_string()].into_iter().collect());
        for host in [
            " git.company.example",
            "git.company.example ",
            "\tgit.company.example",
            "git.company.example\n",
            " ",
        ] {
            assert!(
                !named.allows(host),
                "an answered host set must not match {host:?} with whitespace on it",
            );
        }

        for host in [
            " github.com",
            "github.com ",
            "\tgithub.com",
            " github.enterprise",
        ] {
            assert!(
                !GithubHostPolicy::LegacyNameRule.allows(host),
                "the name rule must not match {host:?} either",
            );
        }
    }

    #[test]
    fn the_legacy_policy_is_the_rule_dux_shipped_before() {
        let policy = GithubHostPolicy::LegacyNameRule;
        assert!(policy.allows("github.com"));
        assert!(policy.allows("github.company.example"));
        assert!(!policy.allows("git.company.example"));
        assert!(!policy.allows("gitlab.com"), "GitLab is rejected");
        assert!(!GithubHostPolicy::DenyAll.allows("github.com"));
        assert!(
            !GithubHostPolicy::DenyAll.allows("gitlab.com"),
            "GitLab is rejected under every mode",
        );
    }

    #[test]
    fn one_working_host_alongside_one_broken_host_reports_github_available() {
        // The behaviour change. Plain `gh auth status` exits non-zero when ANY
        // known host has a problem, so one stale token used to disable every
        // GitHub feature on every host. The stand-in reproduces exactly that:
        // it answers the machine-readable call and exits non-zero on the plain
        // one, as a real `gh` in this state does.
        let dir = tempfile::tempdir().expect("tempdir");
        let json = r#"{"hosts":{"git.company.example":[{"state":"success","active":true,"host":"git.company.example"}],"github.com":[{"state":"error","active":true,"host":"github.com"}]}}"#;
        let gh = stand_in_gh(
            dir.path(),
            &format!(
                "case \"$*\" in\n  *--json*) printf '%s' '{json}' ;;\n  *) echo 'error: could not authenticate to github.com' >&2; exit 1 ;;\nesac\n"
            ),
        );

        let probe = probe_github_hosts(gh.as_os_str());
        assert!(
            matches!(
                probe,
                GhProbe::Decided {
                    available: true,
                    ..
                }
            ),
            "one working host means GitHub is available, got {probe:?}",
        );
        assert_eq!(
            hosts(&probe),
            vec!["git.company.example".to_string()],
            "only the working host qualifies",
        );
    }

    #[test]
    fn a_gh_logged_out_of_everything_is_decisive_and_unavailable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let gh = stand_in_gh(dir.path(), "printf '%s' '{\"hosts\":{}}'");
        assert_eq!(
            probe_github_hosts(gh.as_os_str()),
            GhProbe::Decided {
                available: false,
                policy: GithubHostPolicy::Hosts(BTreeSet::new()),
                protocols: BTreeMap::default(),
            },
        );
    }

    #[test]
    fn an_older_gh_falls_back_to_the_name_rule_after_exactly_one_retry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("calls.log");
        let gh = stand_in_gh(
            dir.path(),
            &format!(
                "printf '%s\\n' \"$*\" >> '{}'\ncase \"$*\" in\n  *--json*) echo 'unknown flag: --json' >&2; exit 1 ;;\n  *) exit 0 ;;\nesac\n",
                log.display()
            ),
        );

        let probe = probe_github_hosts(gh.as_os_str());
        assert_eq!(
            probe,
            GhProbe::Decided {
                available: true,
                policy: GithubHostPolicy::LegacyNameRule,
                protocols: BTreeMap::default(),
            },
        );
        let calls: Vec<String> = std::fs::read_to_string(&log)
            .expect("stand-in logged its calls")
            .lines()
            .map(str::to_string)
            .collect();
        assert_eq!(
            calls,
            vec![
                "auth status --active --json hosts".to_string(),
                "auth status".to_string(),
            ],
            "exactly one plain retry, after the machine-readable call",
        );
    }

    /// The retry schedule, as the pure rule the engine consults every tick.
    mod reprobe_schedule {
        use super::super::gh_reprobe_is_due;
        use crate::model::GhStatus;
        use std::time::Duration;

        const INTERVAL: Duration = Duration::from_secs(300);

        #[test]
        fn an_unavailable_gh_is_re_checked_once_the_interval_has_passed() {
            for status in [
                GhStatus::Unknown,
                GhStatus::NotInstalled,
                GhStatus::NotAuthenticated,
                GhStatus::Unreachable,
            ] {
                assert!(
                    gh_reprobe_is_due(
                        status,
                        crate::gh::GithubIntegration::Enabled,
                        Some(Duration::from_secs(300)),
                        INTERVAL
                    ),
                    "{status:?} is not a working gh, so it is re-checked",
                );
                assert!(
                    !gh_reprobe_is_due(
                        status,
                        crate::gh::GithubIntegration::Enabled,
                        Some(Duration::from_secs(299)),
                        INTERVAL
                    ),
                    "{status:?} waits out the interval",
                );
            }
        }

        #[test]
        fn a_working_gh_stops_the_timer_and_a_later_failure_restarts_it() {
            assert!(
                !gh_reprobe_is_due(
                    GhStatus::Available,
                    crate::gh::GithubIntegration::Enabled,
                    Some(Duration::from_mins(150)),
                    INTERVAL
                ),
                "nothing is waiting on an answer, so dux stops spawning gh",
            );
            // The rule reads the status live rather than latching, so the same
            // clock with a failed status is due again immediately.
            assert!(gh_reprobe_is_due(
                GhStatus::Unreachable,
                crate::gh::GithubIntegration::Enabled,
                Some(Duration::from_mins(150)),
                INTERVAL
            ));
        }

        #[test]
        fn the_integration_being_off_asks_nothing() {
            assert!(!gh_reprobe_is_due(
                GhStatus::Unreachable,
                crate::gh::GithubIntegration::Disabled,
                Some(Duration::from_mins(150)),
                INTERVAL
            ));
        }

        #[test]
        fn a_zero_interval_disables_the_timer() {
            assert!(!gh_reprobe_is_due(
                GhStatus::Unreachable,
                crate::gh::GithubIntegration::Enabled,
                Some(Duration::from_mins(150)),
                Duration::ZERO
            ));
        }

        #[test]
        fn a_probe_that_has_never_run_is_not_due_here() {
            // The surfaces run the startup probe themselves; answering "due"
            // before it lands would spawn a second one alongside it.
            assert!(!gh_reprobe_is_due(
                GhStatus::Unknown,
                crate::gh::GithubIntegration::Enabled,
                None,
                INTERVAL
            ));
        }
    }

    #[test]
    fn a_missing_gh_is_reported_as_not_installed() {
        // Told by the launch itself, so no other program has to be on PATH to
        // ask where `gh` is.
        assert_eq!(
            probe_github_hosts(OsStr::new("dux-no-such-gh-binary")),
            GhProbe::NotInstalled,
        );
    }

    /// `gh auth status` (no `--json`) on gh 2.95.0 with no login anywhere.
    /// stdout is empty, this is stderr, and the exit code is 1.
    const MEASURED_PLAIN_NO_LOGIN: &str =
        "You are not logged into any GitHub hosts. To log in, run: gh auth login\n";

    /// `gh auth status` on gh 2.95.0 with a token in `GH_TOKEN` that GitHub
    /// rejects. Measured twice, once against a live api.github.com and once
    /// with every outbound connection refused, and the two runs were byte for
    /// byte identical: this shape cannot tell a bad token from a dead network.
    const MEASURED_PLAIN_TOKEN_REFUSED: &str = "github.com\n  \
         X Failed to log in to github.com using token (GH_TOKEN)\n  \
         - Active account: true\n  \
         - The token in GH_TOKEN is invalid.\n";

    /// The same failure for a login stored in `hosts.yml` rather than an
    /// environment token, measured the same two ways with the same result.
    const MEASURED_PLAIN_ACCOUNT_REFUSED: &str = "github.com\n  \
         X Failed to log in to github.com account octocat (/home/u/.config/gh/hosts.yml)\n  \
         - Active account: true\n  \
         - The token in /home/u/.config/gh/hosts.yml is invalid.\n  \
         - To re-authenticate, run: gh auth login -h github.com\n  \
         - To forget about this account, run: gh auth logout -h github.com -u octocat\n";

    #[test]
    fn a_plain_status_saying_not_logged_in_is_decisive() {
        // gh's own sentence for a machine with no login at all. It decides, and
        // it must keep deciding: retrying it forever would be noise.
        let probe = decide_gh_probe(
            GhCallOutcome::Completed(completed_with_stderr(1, "", "unknown flag: --json")),
            || GhCallOutcome::Completed(completed_with_stderr(1, "", MEASURED_PLAIN_NO_LOGIN)),
        );
        assert_eq!(
            probe,
            GhProbe::Decided {
                available: false,
                policy: GithubHostPolicy::LegacyNameRule,
                protocols: BTreeMap::default(),
            },
        );
    }

    #[test]
    fn a_plain_status_that_could_not_log_in_is_transient() {
        // The measured ambiguity: this is what a refused token AND a dead
        // network both print. Retrying is the safe reading of a shape that
        // cannot distinguish them.
        for measured in [MEASURED_PLAIN_TOKEN_REFUSED, MEASURED_PLAIN_ACCOUNT_REFUSED] {
            let probe = decide_gh_probe(
                GhCallOutcome::Completed(completed_with_stderr(1, "", "unknown flag: --json")),
                || GhCallOutcome::Completed(completed_with_stderr(1, "", measured)),
            );
            let GhProbe::Transient(reason) = probe else {
                panic!("an ambiguous login failure decides nothing, got {probe:?}");
            };
            assert!(
                reason.contains("Failed to log in to github.com"),
                "the real diagnostic line is kept for the log, got {reason}",
            );
            assert!(
                !reason.contains("X Failed"),
                "gh's cross glyph is stripped from the sentence dux shows, got {reason}",
            );
        }
    }

    #[test]
    fn a_plain_status_timing_out_is_transient() {
        let probe = decide_gh_probe(
            GhCallOutcome::Completed(completed_with_stderr(1, "", "unknown flag: --json")),
            || GhCallOutcome::TimedOut,
        );
        assert!(matches!(probe, GhProbe::Transient(_)), "got {probe:?}");
    }

    #[test]
    fn a_plain_status_that_succeeds_is_available() {
        let probe = decide_gh_probe(
            GhCallOutcome::Completed(completed_with_stderr(1, "", "unknown flag: --json")),
            || GhCallOutcome::Completed(completed(0, "")),
        );
        assert_eq!(
            probe,
            GhProbe::Decided {
                available: true,
                policy: GithubHostPolicy::LegacyNameRule,
                protocols: BTreeMap::default(),
            },
        );
    }

    /// `gh auth status --active --json hosts` on gh 2.95.0 with no login
    /// anywhere. Note the exit code: ZERO, with the not-logged-in sentence on
    /// stderr and this on stdout.
    const MEASURED_JSON_NO_ACCOUNTS: &str = r#"{"hosts":{}}"#;

    /// The same call with a `GH_TOKEN` set and every outbound connection
    /// refused (measured through a proxy pointed at a closed port).
    const MEASURED_JSON_NETWORK_DOWN: &str = r#"{"hosts":{"github.com":[{"state":"error","error":"Post \"https://api.github.com/graphql\": proxyconnect tcp: dial tcp 127.0.0.1:1: connect: connection refused","active":true,"host":"github.com","login":"","tokenSource":"GH_TOKEN","gitProtocol":"https"}]}}"#;

    /// The same call with a `GH_TOKEN` GitHub rejects, reached over a live
    /// network. gh reports the raw non-200 body for an environment token.
    const MEASURED_JSON_BAD_ENV_TOKEN: &str = r#"{"hosts":{"github.com":[{"state":"error","error":"non-200 OK status code: 401 Unauthorized body: \"{\r\n  \"message\": \"Bad credentials\",\r\n  \"documentation_url\": \"https://docs.github.com/rest\",\r\n  \"status\": \"401\"\r\n}\"","active":true,"host":"github.com","login":"","tokenSource":"GH_TOKEN","gitProtocol":"https"}]}}"#;

    /// The same rejection for a login stored in `hosts.yml`, which gh reports
    /// in its own shorter `HTTP <code>: <message>` form.
    const MEASURED_JSON_BAD_STORED_TOKEN: &str = r#"{"hosts":{"github.com":[{"state":"error","error":"HTTP 401: Bad credentials (https://api.github.com/)","active":true,"host":"github.com","login":"octocat","tokenSource":"/home/u/.config/gh/hosts.yml","gitProtocol":"https"}]}}"#;

    /// NOT measured: a rate limit could not be provoked on demand here. It is
    /// [`MEASURED_JSON_BAD_STORED_TOKEN`]'s shape, which was measured, carrying
    /// the status and message GitHub's REST API documents for an exhausted
    /// quota.
    const MIRRORED_JSON_RATE_LIMITED: &str = r#"{"hosts":{"github.com":[{"state":"error","error":"HTTP 403: API rate limit exceeded for user ID 583231. (https://api.github.com/)","active":true,"host":"github.com","login":"octocat","tokenSource":"keyring","gitProtocol":"https"}]}}"#;

    fn json_probe(stdout: &str) -> GhProbe {
        decide_gh_probe(GhCallOutcome::Completed(completed(0, stdout)), || {
            panic!("a machine-readable answer must never reach the plain fallback")
        })
    }

    #[test]
    fn a_json_answer_with_no_accounts_at_all_is_decisively_logged_out() {
        // Nothing to retry: gh holds no credential, so waiting changes nothing
        // until the user runs `gh auth login`.
        assert_eq!(
            json_probe(MEASURED_JSON_NO_ACCOUNTS),
            GhProbe::Decided {
                available: false,
                policy: GithubHostPolicy::Hosts(BTreeSet::new()),
                protocols: BTreeMap::default(),
            },
        );
    }

    #[test]
    fn a_json_account_that_could_not_reach_github_is_transient() {
        let probe = json_probe(MEASURED_JSON_NETWORK_DOWN);
        let GhProbe::Transient(reason) = probe else {
            panic!("an unreachable API decides nothing, got {probe:?}");
        };
        assert!(
            reason.contains("connection refused"),
            "gh's own error text is kept for the log, got {reason}",
        );
    }

    #[test]
    fn a_json_account_that_is_rate_limited_is_transient() {
        let probe = json_probe(MIRRORED_JSON_RATE_LIMITED);
        let GhProbe::Transient(reason) = probe else {
            panic!("a rate limit decides nothing, got {probe:?}");
        };
        assert!(
            reason.contains("rate limit"),
            "gh's own error text is kept for the log, got {reason}",
        );
    }

    #[test]
    fn a_json_account_whose_credentials_are_rejected_is_decisive() {
        // A 401 is GitHub saying the token itself is no good. Retrying it every
        // few minutes would never produce a different answer.
        for measured in [MEASURED_JSON_BAD_ENV_TOKEN, MEASURED_JSON_BAD_STORED_TOKEN] {
            assert_eq!(
                json_probe(measured),
                GhProbe::Decided {
                    available: false,
                    policy: GithubHostPolicy::Hosts(BTreeSet::new()),
                    protocols: BTreeMap::default(),
                },
                "a rejected credential decides",
            );
        }
    }

    #[test]
    fn one_working_host_is_not_undone_by_an_unreachable_sibling() {
        // Transience is only consulted when NOTHING qualified: a host that
        // works keeps the GitHub features on whatever its siblings report.
        let mixed = r#"{"hosts":{
            "git.company.example":[{"state":"error","error":"Post \"https://git.company.example/api/graphql\": dial tcp: connect: connection refused","active":true,"host":"git.company.example"}],
            "github.com":[{"state":"success","active":true,"host":"github.com"}]
        }}"#;
        assert_eq!(
            hosts(&json_probe(mixed)),
            vec!["github.com".to_string()],
            "a reachable host still qualifies",
        );
    }

    #[test]
    fn an_unclassifiable_account_error_still_decides() {
        // The classification only ever RESCUES an answer from being decisive.
        // Text that matches neither table keeps the old reading rather than
        // retrying something dux cannot recognise forever.
        let odd = r#"{"hosts":{"github.com":[{"state":"error","error":"something dux has never seen","active":true,"host":"github.com"}]}}"#;
        assert_eq!(
            json_probe(odd),
            GhProbe::Decided {
                available: false,
                policy: GithubHostPolicy::Hosts(BTreeSet::new()),
                protocols: BTreeMap::default(),
            },
        );
    }

    #[test]
    fn a_timeout_is_transient_and_triggers_no_retry() {
        // A timeout is not the older-`gh` case: it must not reach the
        // permissive name rule, and it must leave the previous value alone.
        let mut retried = false;
        let probe = decide_gh_probe(GhCallOutcome::TimedOut, || {
            retried = true;
            GhCallOutcome::Completed(completed(0, ""))
        });
        assert!(matches!(probe, GhProbe::Transient(_)), "got {probe:?}");
        assert!(
            !retried,
            "a timeout must not trigger the plain-status retry"
        );

        let mut retried = false;
        let probe = decide_gh_probe(GhCallOutcome::Failed("no such file".to_string()), || {
            retried = true;
            GhCallOutcome::Completed(completed(0, ""))
        });
        assert!(matches!(probe, GhProbe::Transient(_)), "got {probe:?}");
        assert!(
            !retried,
            "a failure to launch must not trigger the plain-status retry",
        );
    }

    #[test]
    fn a_parsed_answer_stands_regardless_of_the_exit_status() {
        // In JSON mode `gh` exits zero even when a known host is broken, so the
        // exit code carries no information; only the parse does.
        let mut retried = false;
        let probe = decide_gh_probe(
            GhCallOutcome::Completed(completed(1, MEASURED_GH_2_95_OUTPUT)),
            || {
                retried = true;
                GhCallOutcome::Completed(completed(0, ""))
            },
        );
        assert!(!retried, "a parseable answer needs no retry");
        assert_eq!(hosts(&probe), vec!["github.com".to_string()]);
    }

    /// Build a `std::process::Output` with a real exit status, by running
    /// `sh -c 'exit N'`. `ExitStatus` cannot be constructed portably by hand.
    fn completed(code: i32, stdout: &str) -> std::process::Output {
        completed_with_stderr(code, stdout, "")
    }

    /// [`completed`] with `gh`'s diagnostics too, which is where the mode switch
    /// reads from: `gh` writes its unsupported-option message to stderr and
    /// leaves stdout empty (measured on 2.95.0).
    fn completed_with_stderr(code: i32, stdout: &str, stderr: &str) -> std::process::Output {
        let status = std::process::Command::new("sh")
            .args(["-c", &format!("exit {code}")])
            .status()
            .expect("sh");
        std::process::Output {
            status,
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule dux applied before it asked `gh` which hosts it can serve:
    /// `github.com` or `github.*`. The tests below are about parsing, planning
    /// and argument building rather than about which hosts qualify, so they run
    /// under the policy those behaviours were written against and keep meaning
    /// exactly what they meant. The eligibility gates have their own tests,
    /// which pass a real answered host set.
    fn legacy_policy() -> GithubHostPolicy {
        GithubHostPolicy::LegacyNameRule
    }

    fn pr_node(number: u64, state: &str) -> serde_json::Value {
        pr_node_from(number, state, "octocat/Hello-World")
    }

    /// A discovery node naming the repository it belongs to, which is not
    /// necessarily the repository the alias was asked under (a fork's pull
    /// request whose head is this ref shows up in the same connection).
    fn pr_node_from(number: u64, state: &str, name_with_owner: &str) -> serde_json::Value {
        serde_json::json!({
            "number": number,
            "state": state,
            "title": format!("PR {number}"),
            "url": format!("https://github.com/{name_with_owner}/pull/{number}"),
            "repository": { "nameWithOwner": name_with_owner },
        })
    }

    #[test]
    fn looks_rate_limited_detects_github_limit_messages() {
        assert!(looks_rate_limited(
            "HTTP 403: API rate limit exceeded for user"
        ));
        assert!(looks_rate_limited(
            "You have exceeded a secondary rate limit"
        ));
        assert!(looks_rate_limited(
            r#"[{"type":"RATE_LIMITED","message":"..."}]"#
        ));
        // Not rate-limiting: a plain network / not-found error.
        assert!(!looks_rate_limited(
            "dial tcp: lookup api.github.com: no such host"
        ));
        assert!(!looks_rate_limited(
            "Could not resolve to a Repository with the name"
        ));
    }

    #[test]
    fn tighter_rate_limit_keeps_the_lower_remaining() {
        let a = RateLimitInfo {
            remaining: 500,
            reset_at: None,
            cost: Some(1),
        };
        let b = RateLimitInfo {
            remaining: 50,
            reset_at: None,
            cost: Some(1),
        };
        // Whichever ordering, the tighter (fewer-remaining) snapshot wins.
        assert_eq!(
            tighter_rate_limit(Some(a.clone()), Some(b.clone()))
                .unwrap()
                .remaining,
            50
        );
        assert_eq!(
            tighter_rate_limit(Some(b), Some(a.clone()))
                .unwrap()
                .remaining,
            50
        );
        // None cases pass through the present snapshot.
        assert_eq!(
            tighter_rate_limit(None, Some(a.clone())).unwrap().remaining,
            500
        );
        assert_eq!(tighter_rate_limit(Some(a), None).unwrap().remaining, 500);
        assert!(tighter_rate_limit(None, None).is_none());
    }

    fn selected(nodes: &[(u64, &str)]) -> PrInfo {
        let parsed: Vec<PrInfo> = nodes
            .iter()
            .map(|(n, st)| {
                parse_pr_json_value(&pr_node(*n, st), "github.com", "octocat/Hello-World")
                    .expect("node parses")
            })
            .collect();
        select_discovered_pr(parsed).expect("a node was selected")
    }

    #[test]
    fn select_discovered_pr_takes_the_newest_of_two_terminal_pull_requests() {
        // The reused-branch case: an old merged pull request and a newer one on
        // the same branch name. The newer number is the live one.
        let pr = selected(&[(7, "MERGED"), (47, "MERGED")]);
        assert_eq!(pr.number, 47);
    }

    #[test]
    fn select_discovered_pr_takes_the_newest_when_it_is_open() {
        let pr = selected(&[(7, "MERGED"), (47, "OPEN")]);
        assert_eq!(pr.number, 47);
        assert_eq!(pr.state, PrState::Open);
    }

    #[test]
    fn select_discovered_pr_prefers_an_open_pull_request_over_a_newer_closed_one() {
        // Somebody opened #7, closed #47 without merging: the open one is the
        // pull request this branch is actually about, older number or not.
        let pr = selected(&[(7, "OPEN"), (47, "CLOSED")]);
        assert_eq!(pr.number, 7);
        assert_eq!(pr.state, PrState::Open);
    }

    #[test]
    fn select_discovered_pr_is_empty_for_no_nodes() {
        assert!(select_discovered_pr(Vec::new()).is_none());
    }

    /// A live [`PrInfo`] the way a parsed discovery node produces one.
    fn info(number: u64, state: &str) -> PrInfo {
        parse_pr_json_value(&pr_node(number, state), "github.com", "octocat/Hello-World")
            .expect("node parses")
    }

    #[test]
    fn format_pr_change_reads_as_old_to_new() {
        let old = info(7, "MERGED");
        let fresh = info(47, "OPEN");
        assert_eq!(
            format_pr_change("feat/x", Some(&old), Some(&fresh)),
            "Pull request for branch feat/x changed: #7 merged -> #47 open"
        );
        assert_eq!(
            format_pr_change("feat/x", None, Some(&fresh)),
            "Pull request for branch feat/x changed: none -> #47 open"
        );
        assert_eq!(
            format_pr_change("feat/x", Some(&old), None),
            "Pull request for branch feat/x changed: #7 merged -> none"
        );
    }

    #[test]
    fn pr_badge_changed_ignores_a_repeat_of_the_same_pull_request() {
        let open = info(47, "OPEN");
        let same = info(47, "OPEN");
        let merged = info(47, "MERGED");
        let newer = info(48, "OPEN");
        assert!(!pr_badge_changed(Some(&open), Some(&same)));
        assert!(pr_badge_changed(Some(&open), Some(&merged)));
        assert!(pr_badge_changed(Some(&open), Some(&newer)));
        assert!(!pr_badge_changed(None, None));
        assert!(pr_badge_changed(None, Some(&same)));
        assert!(pr_badge_changed(Some(&open), None));
    }

    #[test]
    fn parse_rate_limit_carries_the_query_cost() {
        let data = serde_json::json!({
            "rateLimit": { "cost": 1, "remaining": 4999, "resetAt": "2030-06-01T12:00:00Z" },
        });
        assert_eq!(parse_rate_limit(&data).expect("rate limit").cost, Some(1));
        // An answer without a cost is not a free call, just an unmeasured one.
        let bare = serde_json::json!({ "rateLimit": { "remaining": 4999 } });
        assert_eq!(parse_rate_limit(&bare).expect("rate limit").cost, None);
    }

    #[test]
    fn parse_rate_limit_extracts_remaining_and_reset() {
        let data = serde_json::json!({
            "rateLimit": { "remaining": 4321, "resetAt": "2030-06-01T12:00:00Z" },
        });
        let rl = parse_rate_limit(&data).expect("rate limit");
        assert_eq!(rl.remaining, 4321);
        assert!(rl.reset_at.is_some());
    }

    #[test]
    fn the_inactive_sweep_is_due_only_after_its_own_interval() {
        assert!(
            !inactive_sweep_due(Duration::from_secs(0), 43_200),
            "a cycle that has just run is not due again"
        );
        assert!(!inactive_sweep_due(Duration::from_secs(43_199), 43_200));
        assert!(inactive_sweep_due(Duration::from_hours(12), 43_200));
        assert!(inactive_sweep_due(Duration::from_secs(999_999), 43_200));
    }

    #[test]
    fn a_zero_inactive_interval_never_sweeps_however_long_it_has_been() {
        // 0 is the user turning the slow clock off; no amount of elapsed time
        // is allowed to turn it back on.
        assert!(!inactive_sweep_due(Duration::from_secs(0), 0));
        assert!(!inactive_sweep_due(Duration::from_secs(u32::MAX.into()), 0));
    }

    #[test]
    fn run_command_with_timeout_kills_a_wedged_child() {
        // A command that never exits must be killed at the timeout and reported as
        // TimedOut, not hang the caller.
        let mut cmd = std::process::Command::new("sleep");
        cmd.arg("30");
        let started = Instant::now();
        let outcome = run_command_with_timeout(cmd, Duration::from_millis(200));
        assert!(matches!(outcome, GhCallOutcome::TimedOut));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "must return promptly after the timeout, not block",
        );
    }

    #[test]
    fn run_command_with_timeout_captures_output_and_reports_spawn_failure() {
        let out = run_command_with_timeout(
            {
                let mut c = std::process::Command::new("sh");
                c.args(["-c", "printf hello"]);
                c
            },
            Duration::from_secs(5),
        );
        match out {
            GhCallOutcome::Completed(o) => assert_eq!(&o.stdout, b"hello"),
            _ => panic!("expected Completed with captured stdout"),
        }
        // A binary that doesn't exist → NotFound, not TimedOut.
        let missing = run_command_with_timeout(
            std::process::Command::new("dux-nonexistent-binary-xyz"),
            Duration::from_secs(5),
        );
        assert!(matches!(missing, GhCallOutcome::NotFound(_)));
    }

    #[test]
    fn gh_repo_arg_carries_the_host_for_github_dot_com() {
        // A bare `owner/repo` lets `gh` resolve the host itself, and `GH_HOST`
        // overrides that resolution, so a user pointing `GH_HOST` at their
        // company server had github.com lookups quietly sent there. Every
        // repository argument names its host.
        assert_eq!(
            gh_repo_arg("github.com", "owner/repo"),
            "github.com/owner/repo"
        );
        assert_eq!(gh_repo_arg("", "owner/repo"), "github.com/owner/repo");
    }

    #[test]
    fn pr_view_targets_the_intended_host_even_with_gh_host_set() {
        // Hermetic regression test for the `GH_HOST` override: a stand-in `gh`
        // records its own argv, so this asserts what dux ASKED for rather than
        // depending on the network or on this machine's login. `GH_HOST` is set
        // on the child command only, never on the test process.
        //
        // Built through the SHARED stand-in helper rather than hand-rolled. This
        // test used to write and chmod its own script, which is the pattern
        // `stand_in_gh` exists to replace: a freshly written executable can be
        // refused with ETXTBSY in a multi-threaded process, because another test
        // forking while the write handle is open inherits it and the kernel then
        // sees an open write handle on the file. That is exactly how this test
        // failed, once, in a full run, with an assertion that did not say why.
        // The helper warms the script up until it runs, and the assertion below
        // now names the outcome so a recurrence cannot be anonymous again.
        let dir = tempfile::tempdir().expect("tempdir");
        let recorded = dir.path().join("argv.txt");
        let script = probe_test_support::stand_in_gh(
            dir.path(),
            &format!(
                "for a in \"$@\"; do printf '%s\\n' \"$a\"; done > '{}'\nprintf '{{}}'\n",
                recorded.display()
            ),
        );

        let args = pr_view_args("github.com", "owner/repo", 42);
        let mut cmd = std::process::Command::new(&script);
        cmd.args(&args);
        cmd.env("GH_HOST", "git.company.example");
        let outcome = run_command_with_timeout(cmd, Duration::from_secs(10));
        assert!(
            matches!(outcome, GhCallOutcome::Completed(ref o) if o.status.success()),
            "stand-in gh should have run, got {outcome:?}",
        );

        let recorded_args: Vec<String> = std::fs::read_to_string(&recorded)
            .expect("stand-in recorded its argv")
            .lines()
            .map(str::to_string)
            .collect();
        let repo = recorded_args
            .iter()
            .position(|a| a == "--repo")
            .and_then(|i| recorded_args.get(i + 1))
            .expect("--repo argument");
        assert_eq!(
            repo, "github.com/owner/repo",
            "the lookup must name github.com itself, not leave it to GH_HOST",
        );
    }

    #[test]
    fn gh_repo_arg_includes_host_for_enterprise() {
        assert_eq!(
            gh_repo_arg("github.example.com", "owner/repo"),
            "github.example.com/owner/repo"
        );
    }

    #[test]
    fn pull_request_url_defaults_empty_host_to_github_dot_com() {
        assert_eq!(
            pull_request_url("", "owner/repo", 12),
            "https://github.com/owner/repo/pull/12"
        );
    }

    #[test]
    fn parse_pr_json_object_uses_gh_url_when_present() {
        let pr = parse_pr_json_object(
            r#"{"number":42,"state":"OPEN","title":"Demo","url":"https://github.com/owner/repo/pull/42"}"#,
            "github.com",
            "owner/repo",
        )
        .expect("pr");

        assert_eq!(pr.number, 42);
        assert_eq!(pr.state, PrState::Open);
        assert_eq!(pr.url, "https://github.com/owner/repo/pull/42");
    }

    /// A pull request title is someone else's text: the controls that would
    /// reorder the dialog around it are gone before anything shows it.
    #[test]
    fn a_pull_request_title_loses_its_bidi_controls_wherever_it_enters() {
        let title = "Fix login \u{202E}exe.txt\u{202C}";
        let json = format!(r#"{{"number":42,"state":"OPEN","title":"{title}","headRefName":"f"}}"#);
        let polled = parse_pr_json_object(&json, "github.com", "owner/repo").expect("pr");
        assert_eq!(polled.title, "Fix login exe.txt");

        let resolved = parse_pull_request_metadata_json(&json).expect("resolved");
        assert_eq!(resolved.title, "Fix login exe.txt");

        // A row stored before the stripping existed is cleaned on the way out.
        let stored = StoredPr {
            session_id: "s1".to_string(),
            pr_number: 42,
            host: "github.com".to_string(),
            owner_repo: "owner/repo".to_string(),
            state: "OPEN".to_string(),
            title: title.to_string(),
            url: "https://github.com/owner/repo/pull/42".to_string(),
        };
        assert_eq!(
            reconstruct_pr_from_stored(&stored).expect("pr").title,
            "Fix login exe.txt"
        );
    }

    #[test]
    fn parse_pr_json_object_falls_back_to_host_url() {
        let pr = parse_pr_json_object(
            r#"{"number":42,"state":"MERGED","title":"Demo"}"#,
            "github.example.com",
            "owner/repo",
        )
        .expect("pr");

        assert_eq!(pr.state, PrState::Merged);
        assert_eq!(pr.url, "https://github.example.com/owner/repo/pull/42");
    }

    #[test]
    fn parse_pull_request_lookup_accepts_number_and_hash_number() {
        let plain =
            parse_pull_request_lookup("123", "github.com", "octocat/Hello-World", &legacy_policy())
                .expect("plain number");
        assert_eq!(plain.host, "github.com");
        assert_eq!(plain.owner_repo, "octocat/Hello-World");
        assert_eq!(plain.number, 123);

        let hashed = parse_pull_request_lookup(
            "#456",
            "github.example.com",
            "octocat/Hello-World",
            &legacy_policy(),
        )
        .expect("hash number");
        assert_eq!(hashed.host, "github.example.com");
        assert_eq!(hashed.owner_repo, "octocat/Hello-World");
        assert_eq!(hashed.number, 456);
    }

    #[test]
    fn parse_pull_request_lookup_accepts_matching_github_url() {
        let lookup = parse_pull_request_lookup(
            "https://github.com/octocat/Hello-World/pull/789?foo=bar",
            "github.com",
            "octocat/Hello-World",
            &legacy_policy(),
        )
        .expect("matching URL");
        assert_eq!(lookup.host, "github.com");
        assert_eq!(lookup.owner_repo, "octocat/Hello-World");
        assert_eq!(lookup.number, 789);
    }

    #[test]
    fn parse_pull_request_lookup_accepts_matching_enterprise_url() {
        let lookup = parse_pull_request_lookup(
            "https://github.example.com/octocat/Hello-World/pull/789",
            "github.example.com",
            "octocat/Hello-World",
            &legacy_policy(),
        )
        .expect("matching enterprise URL");
        assert_eq!(lookup.host, "github.example.com");
        assert_eq!(lookup.owner_repo, "octocat/Hello-World");
        assert_eq!(lookup.number, 789);
    }

    #[test]
    fn parse_pull_request_lookup_strips_trailing_slash_and_fragment() {
        let lookup = parse_pull_request_lookup(
            "https://github.com/octocat/Hello-World/pull/5/#discussion",
            "github.com",
            "octocat/Hello-World",
            &legacy_policy(),
        )
        .expect("trailing slash + fragment");
        assert_eq!(lookup.number, 5);
    }

    #[test]
    fn parse_pull_request_lookup_rejects_mismatched_github_url() {
        let err = parse_pull_request_lookup(
            "https://github.com/other/repo/pull/12",
            "github.com",
            "octocat/Hello-World",
            &legacy_policy(),
        )
        .expect_err("mismatched repo");
        assert!(err.contains("selected project uses github.com/octocat/Hello-World"));
    }

    #[test]
    fn parse_pull_request_lookup_rejects_empty_input() {
        let err =
            parse_pull_request_lookup("   ", "github.com", "octocat/Hello-World", &legacy_policy())
                .expect_err("empty");
        assert!(err.contains("Enter a pull request URL"), "{err}");
    }

    #[test]
    fn parse_pull_request_lookup_rejects_garbage() {
        let err = parse_pull_request_lookup(
            "not-a-pr",
            "github.com",
            "octocat/Hello-World",
            &legacy_policy(),
        )
        .expect_err("garbage");
        assert!(err.contains("Enter a pull request URL"), "{err}");
    }

    #[test]
    fn parse_pull_request_lookup_rejects_non_github_url() {
        let err = parse_pull_request_lookup(
            "https://gitlab.com/octocat/Hello-World/pull/1",
            "github.com",
            "octocat/Hello-World",
            &legacy_policy(),
        )
        .expect_err("non-github host");
        // The host is now READ rather than left unparsed, so the refusal can say
        // which host it is and what would make it work.
        assert!(
            err.contains("cannot look up pull requests on gitlab.com"),
            "{err}"
        );
    }

    #[test]
    fn parse_pull_request_lookup_reads_a_browser_route_as_the_repository_it_names() {
        // `/issues/3` used to be refused as a malformed pull URL. A trailing
        // browser route is now discarded, so the address names the repository
        // and the only thing missing is the pull request number. Saying so is
        // more use than calling the whole address malformed.
        let err = parse_pull_request_lookup(
            "https://github.com/octocat/Hello-World/issues/3",
            "github.com",
            "octocat/Hello-World",
            &legacy_policy(),
        )
        .expect_err("names no pull request");
        assert!(
            err.contains("names github.com/octocat/Hello-World but no pull request"),
            "{err}"
        );
        assert!(err.contains("octocat/Hello-World#123"), "{err}");
    }

    #[test]
    fn parse_pull_request_lookup_accepts_a_pull_url_under_a_browser_route() {
        for input in [
            "https://github.com/octocat/Hello-World/pull/7/files",
            "https://github.com/octocat/Hello-World/pull/7/commits/deadbee",
        ] {
            let lookup = parse_pull_request_lookup(
                input,
                "github.com",
                "octocat/Hello-World",
                &legacy_policy(),
            )
            .unwrap_or_else(|err| panic!("{input}: {err}"));
            assert_eq!(lookup.number, 7, "{input}");
        }
    }

    #[test]
    fn parse_pull_request_lookup_accepts_owner_repo_hash_number_for_the_selected_project() {
        // It names no host, so it must not be assumed to mean github.com: it
        // takes the selected project's host, whatever that is.
        let lookup = parse_pull_request_lookup(
            "octocat/Hello-World#42",
            "git.company.example",
            "octocat/Hello-World",
            &legacy_policy(),
        )
        .expect("owner/repo#number");
        assert_eq!(lookup.host, "git.company.example");
        assert_eq!(lookup.owner_repo, "octocat/Hello-World");
        assert_eq!(lookup.number, 42);
    }

    #[test]
    fn parse_pull_request_lookup_rejects_owner_repo_hash_number_for_another_repository() {
        let err = parse_pull_request_lookup(
            "other/repo#42",
            "github.com",
            "octocat/Hello-World",
            &legacy_policy(),
        )
        .expect_err("another repository");
        assert!(err.contains("PR belongs to other/repo"), "{err}");
        assert!(
            err.contains("selected project uses github.com/octocat/Hello-World"),
            "{err}"
        );
    }

    #[test]
    fn a_pull_request_is_looked_up_without_a_project_and_failures_keep_their_sentences() {
        let dir = tempfile::tempdir().unwrap();
        let lookup = PullRequestLookup {
            host: "github.com".to_string(),
            owner_repo: "acme/widget".to_string(),
            number: 7,
        };

        let found = probe_test_support::stand_in_gh_with_pull_request(
            dir.path(),
            &[],
            Ok(
                r#"{"number":7,"title":"Add a thing","state":"OPEN","headRefName":"add-thing","headRefOid":"0123456789abcdef0123456789abcdef01234567"}"#,
            ),
        );
        assert_eq!(
            look_up_pull_request(found.as_os_str(), &lookup).unwrap(),
            PullRequestMetadata {
                number: 7,
                title: "Add a thing".to_string(),
                state: "OPEN".to_string(),
                head_ref_name: "add-thing".to_string(),
                head_ref_oid: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
            }
        );

        let missing =
            probe_test_support::stand_in_gh_with_pull_request(dir.path(), &[], Err("not found"));
        assert_eq!(
            look_up_pull_request(missing.as_os_str(), &lookup).unwrap_err(),
            "Failed to resolve PR #7 from acme/widget: not found"
        );
    }

    #[test]
    fn a_repository_s_default_branch_is_read_from_gh_and_failures_keep_their_sentences() {
        let dir = tempfile::tempdir().unwrap();
        let found = probe_test_support::stand_in_gh_with_repository(
            dir.path(),
            Ok(r#"{"defaultBranchRef":{"name":"trunk"}}"#),
        );
        assert_eq!(
            look_up_default_branch(found.as_os_str(), "github.com", "acme/widget").unwrap(),
            "trunk"
        );

        let missing = probe_test_support::stand_in_gh_with_repository(dir.path(), Err("boom"));
        assert_eq!(
            look_up_default_branch(missing.as_os_str(), "github.com", "acme/widget").unwrap_err(),
            "Failed to look up the default branch of acme/widget: boom"
        );

        // A repository with no default branch (an empty one) is not an answer.
        let empty = probe_test_support::stand_in_gh_with_repository(
            dir.path(),
            Ok(r#"{"defaultBranchRef":null}"#),
        );
        assert!(
            look_up_default_branch(empty.as_os_str(), "github.com", "acme/widget")
                .unwrap_err()
                .contains("did not name a default branch")
        );
    }

    #[test]
    fn parse_pull_request_metadata_json_extracts_fields() {
        let resolved = parse_pull_request_metadata_json(
            r#"{"number":42,"title":"Fix bug","state":"OPEN","headRefName":"feature/fix"}"#,
        )
        .expect("resolved");
        assert_eq!(resolved.number, 42);
        assert_eq!(resolved.title, "Fix bug");
        assert_eq!(resolved.state, "OPEN");
        assert_eq!(resolved.head_ref_name, "feature/fix");
    }

    #[test]
    fn parse_pull_request_metadata_json_rejects_missing_head_branch() {
        let err =
            parse_pull_request_metadata_json(r#"{"number":42,"title":"Fix bug","state":"OPEN"}"#)
                .expect_err("missing head");
        assert!(err.contains("head branch"));
    }

    /// A real repository whose `origin` remote is spelled with an explicit
    /// `ssh://` scheme, which is what git reports whenever an `url.*.insteadOf`
    /// rewrite maps onto an ssh base.
    fn ssh_origin_repo(url: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for args in [
            vec!["init", "--quiet"],
            vec!["remote", "add", "origin", url],
        ] {
            let out = crate::git::test_support::git_command()
                .args(&args)
                .current_dir(dir.path())
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        }
        dir
    }

    /// The hazard these fixtures have to be protected from, demonstrated rather
    /// than asserted from memory: `git remote get-url` APPLIES the user's
    /// `url.*.insteadOf` rewrites, so a developer with one configured would see
    /// these tests resolve a host nobody wrote down. The rewrite is supplied
    /// per-command here, so this reproduces on every machine and reads nobody's
    /// real configuration.
    #[test]
    fn git_remote_get_url_applies_insteadof_rewrites() {
        let dir = ssh_origin_repo("ssh://git@github.com/octocat/Hello-World.git");
        let hostile = dir.path().join("hostile.gitconfig");
        std::fs::write(
            &hostile,
            "[url \"ssh://git@evil.example/\"]\n\tinsteadOf = ssh://git@github.com/\n",
        )
        .unwrap();
        let out = crate::git::test_support::git_command()
            .args([
                "-C",
                dir.path().to_str().unwrap(),
                "remote",
                "get-url",
                "origin",
            ])
            .env("GIT_CONFIG_GLOBAL", &hostile)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "ssh://git@evil.example/octocat/Hello-World.git",
            "if this stops rewriting, the isolation below is no longer load-bearing"
        );
    }

    /// So the fixtures isolate git's system and global configuration on every
    /// command they run. Whatever the developer has configured, git sees an
    /// empty global config here, and these tests pass or fail for reasons that
    /// belong to the code.
    #[test]
    fn git_fixtures_run_against_an_empty_global_configuration() {
        let dir = ssh_origin_repo("ssh://git@github.com/octocat/Hello-World.git");
        let out = crate::git::test_support::git_command()
            .args([
                "-C",
                dir.path().to_str().unwrap(),
                "config",
                "--global",
                "--list",
            ])
            .output()
            .unwrap();
        // The exit status is asserted FIRST, and it is what makes the emptiness
        // below mean anything. A git command that fails to run at all prints
        // nothing on stdout, so an emptiness assertion on its own passes for the
        // one reason it must never pass for.
        assert!(
            out.status.success(),
            "the fixture's git command must succeed, got status {} and stderr:\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            String::from_utf8_lossy(&out.stdout).trim().is_empty(),
            "a git command built by the fixture helper must see no global git \
             configuration, got:\n{}",
            String::from_utf8_lossy(&out.stdout)
        );
    }

    /// Configuration reaches git through the ENVIRONMENT as well as through
    /// files, and the file variables say nothing about it. `GIT_CONFIG_COUNT`
    /// with its numbered key and value installs an `insteadOf` rewrite just as
    /// a global config file would, and `GIT_CONFIG_PARAMETERS` is a second,
    /// independent channel that a zero count does not touch. Each is shown
    /// rewriting first, so the assertion that follows cannot be vacuous, and
    /// then shown neutralised. The variables are set on the command BEFORE the
    /// isolation is applied, which is what an inherited variable looks like
    /// from git's side.
    #[test]
    fn git_fixtures_are_isolated_from_configuration_passed_through_the_environment() {
        let dir = ssh_origin_repo("ssh://git@github.com/octocat/Hello-World.git");
        let get_url = [
            "-C",
            dir.path().to_str().unwrap(),
            "remote",
            "get-url",
            "origin",
        ];
        let channels: [Vec<(&str, &str)>; 2] = [
            vec![
                ("GIT_CONFIG_COUNT", "1"),
                ("GIT_CONFIG_KEY_0", "url.ssh://git@evil.example/.insteadOf"),
                ("GIT_CONFIG_VALUE_0", "ssh://git@github.com/"),
            ],
            vec![(
                "GIT_CONFIG_PARAMETERS",
                "'url.ssh://git@evil.example/.insteadOf=ssh://git@github.com/'",
            )],
        ];
        for channel in channels {
            // The demonstration half is isolated FIRST and given the hostile
            // channel afterwards, so it shows this channel rewriting and not
            // whatever the developer happens to have inherited. (Without that,
            // an inherited rewrite of its own competes with it and this test
            // fails for a reason that belongs to nobody's code, which is the
            // exact defect it exists to close.)
            let mut demonstration = crate::test_git::fixture_git();
            demonstration.args(get_url);
            crate::git::test_support::isolate_git_config(&mut demonstration);
            demonstration.envs(channel.iter().copied());
            let rewritten = demonstration.output().unwrap();
            assert_eq!(
                String::from_utf8_lossy(&rewritten.stdout).trim(),
                "ssh://git@evil.example/octocat/Hello-World.git",
                "{channel:?} must really rewrite, or the assertion below proves nothing",
            );

            let mut isolated = crate::test_git::fixture_git();
            isolated.args(get_url).envs(channel.iter().copied());
            crate::git::test_support::isolate_git_config(&mut isolated);
            let out = isolated.output().unwrap();
            assert_eq!(
                String::from_utf8_lossy(&out.stdout).trim(),
                "ssh://git@github.com/octocat/Hello-World.git",
                "{channel:?} must not reach an isolated command",
            );
        }
    }

    /// The `origin` remote of a fixture repository, resolved the way production
    /// resolves it but with the git half isolated from the developer's
    /// configuration.
    ///
    /// `git::remote_github_repo` is deliberately NOT used: it spawns git itself
    /// and inherits the test process's environment, so it applies whatever
    /// `url.*.insteadOf` the developer has configured (which is correct in
    /// production, since the rewritten URL is the one git would really contact,
    /// and is exactly what must not decide a test's outcome). This runs the same
    /// command through the isolating helper and hands the output to the same
    /// pure parser, which is the composition production performs and the half
    /// where the behaviour under test lives.
    fn isolated_origin_remote(dir: &std::path::Path) -> Option<git::GitHubRemote> {
        let out = crate::git::test_support::git_command()
            .args(["-C", dir.to_str().unwrap(), "remote", "get-url", "origin"])
            .output()
            .unwrap();
        assert!(out.status.success(), "git remote get-url failed");
        git::github_remote_from_git_output(&out.stdout)
    }

    #[test]
    fn pull_request_lookup_resolves_an_ssh_scheme_origin_remote() {
        // The from-PR flow resolves the project's remote before it can look
        // anything up; an unparsed spelling made it fail with "does not have a
        // GitHub origin remote". This covers everything the job does before it
        // shells out to `gh`, which is the part that was broken.
        let dir = ssh_origin_repo("ssh://git@github.com/octocat/Hello-World.git");
        let remote = isolated_origin_remote(dir.path()).expect("ssh:// origin resolves");
        assert_eq!(remote.host, "github.com");
        assert_eq!(remote.owner_repo, "octocat/Hello-World");

        let lookup =
            parse_pull_request_lookup("#7", &remote.host, &remote.owner_repo, &legacy_policy())
                .expect("a resolved remote makes the lookup reachable");
        assert_eq!(lookup.host, "github.com");
        assert_eq!(lookup.owner_repo, "octocat/Hello-World");
        assert_eq!(lookup.number, 7);
    }

    /// The attach lookup constructor, per input form. Unlike
    /// `parse_pull_request_lookup` there is NO project-match refusal: a
    /// cross-repo URL is the point of the flow.
    #[test]
    fn parse_attach_lookup_accepts_a_cross_repo_url() {
        let resolution = git::RemoteResolution::Allowed(git::GitHubRemote {
            host: "github.com".to_string(),
            owner_repo: "octocat/Hello-World".to_string(),
        });
        let lookup = parse_attach_pull_request_lookup(
            "https://github.com/forker/Hello-World/pull/12",
            &resolution,
            "proj",
            &legacy_policy(),
        )
        .expect("cross-repo URLs are accepted");
        assert_eq!(lookup.owner_repo, "forker/Hello-World");
        assert_eq!(lookup.number, 12);
        assert_eq!(lookup.host, "github.com");

        // `owner/repo#123` names no host: it inherits the project remote's.
        let resolution = git::RemoteResolution::Allowed(git::GitHubRemote {
            host: "github.example.com".to_string(),
            owner_repo: "octocat/Hello-World".to_string(),
        });
        let lookup = parse_attach_pull_request_lookup(
            "forker/Hello-World#7",
            &resolution,
            "proj",
            &legacy_policy(),
        )
        .expect("host-less repo references are accepted");
        assert_eq!(lookup.host, "github.example.com");
        assert_eq!(lookup.owner_repo, "forker/Hello-World");
        assert_eq!(lookup.number, 7);
    }

    #[test]
    fn parse_attach_lookup_gates_the_typed_host_and_requires_a_number() {
        let resolution = git::RemoteResolution::Allowed(git::GitHubRemote {
            host: "github.com".to_string(),
            owner_repo: "octocat/Hello-World".to_string(),
        });
        let policy = GithubHostPolicy::Hosts(std::iter::once("github.com".to_string()).collect());
        let err = parse_attach_pull_request_lookup(
            "https://git.company.example/acme/widget/pull/3",
            &resolution,
            "proj",
            &policy,
        )
        .expect_err("a typed host the policy denies is refused");
        assert!(err.contains("git.company.example"), "got {err}");
        // The sign-in command is a chip, quoted in the plain spelling, never
        // between backticks, which no status surface renders.
        assert!(!err.contains('`'), "got {err}");
        assert!(
            err.segments()
                .is_some_and(|parts| parts.contains(&crate::prose::ProseSegment::Name {
                    name: "gh auth login --hostname git.company.example".to_string(),
                    quoted: true,
                })),
            "got {err:?}"
        );

        let err =
            parse_attach_pull_request_lookup("42", &git::RemoteResolution::Denied, "proj", &policy)
                .expect_err("a denied remote host cannot resolve a bare number");
        assert!(!err.contains('`'), "got {err}");
        assert!(
            err.contains("\"gh auth login --hostname <host>\""),
            "got {err}"
        );

        let err = parse_attach_pull_request_lookup(
            "https://github.com/forker/Hello-World",
            &resolution,
            "proj",
            &policy,
        )
        .expect_err("a repository address without a number names no PR");
        assert!(err.contains("forker/Hello-World"), "got {err}");
    }

    /// A bare number resolves against the session's project remote, and each of
    /// the three real failure modes is surfaced by name rather than collapsed.
    #[test]
    fn parse_attach_lookup_bare_number_follows_the_project_remote() {
        let allowed = git::RemoteResolution::Allowed(git::GitHubRemote {
            host: "github.com".to_string(),
            owner_repo: "octocat/Hello-World".to_string(),
        });
        let lookup = parse_attach_pull_request_lookup("#42", &allowed, "proj", &legacy_policy())
            .expect("bare numbers resolve against the remote");
        assert_eq!(lookup.owner_repo, "octocat/Hello-World");
        assert_eq!(lookup.number, 42);

        let err = parse_attach_pull_request_lookup(
            "42",
            &git::RemoteResolution::Unresolved,
            "proj",
            &legacy_policy(),
        )
        .expect_err("no GitHub origin means no bare-number resolution");
        assert!(err.contains("GitHub origin remote"), "got {err}");
        assert!(err.contains("proj"), "got {err}");

        let err = parse_attach_pull_request_lookup(
            "42",
            &git::RemoteResolution::Denied,
            "proj",
            &legacy_policy(),
        )
        .expect_err("a policy-denied remote host is named, not collapsed");
        assert!(err.contains("not signed in"), "got {err}");
    }

    // ── The pull-request sync: questions, rotation, refresh, answers ────────

    fn at(text: &str) -> chrono::DateTime<chrono::Utc> {
        text.parse().expect("an RFC 3339 time")
    }

    fn sync_branch(name: &str) -> PrSyncBranch {
        PrSyncBranch {
            name: name.to_string(),
            former_names: Vec::new(),
            identity_at: None,
            tip: None,
            last_ask: None,
        }
    }

    fn asked_at(when: &str) -> BranchAskRecord {
        BranchAskRecord {
            at: at(when),
            ref_gone: false,
            returned: Vec::new(),
        }
    }

    /// A pull request the agent already has, last refreshed at noon.
    fn known_row(
        number: u64,
        state: PrState,
        owner_repo: &str,
        head: Option<&str>,
    ) -> crate::model::AgentPr {
        let mut row = crate::model::AgentPr::found(
            &PrInfo {
                number,
                state: PrState::Open,
                title: format!("PR {number}"),
                host: "github.com".to_string(),
                owner_repo: owner_repo.to_string(),
                url: format!("https://github.com/{owner_repo}/pull/{number}"),
                head_branch: head.map(str::to_string),
                ..PrInfo::default()
            },
            at("2026-11-01T00:00:00Z"),
        );
        row.state = state;
        row.last_refreshed_at = Some(at("2026-11-06T12:00:00Z"));
        row
    }

    fn sync_entry() -> PrSyncEntry {
        PrSyncEntry {
            session_id: "s0".to_string(),
            worktree_path: "/nonexistent/worktree".to_string(),
            current: sync_branch("feat/x"),
            history: Vec::new(),
            known: Vec::new(),
            agent_exited: false,
            inactive: false,
            history_per_cycle: 8,
            settled_recheck: chrono::TimeDelta::hours(12),
        }
    }

    fn ours(_: &Path) -> git::RemoteResolution {
        git::RemoteResolution::Allowed(git::GitHubRemote {
            host: "github.com".to_string(),
            owner_repo: "octocat/Hello-World".to_string(),
        })
    }

    const NOON: &str = "2026-11-06T12:00:00Z";

    /// Each question one line: where it goes and what it asks.
    fn described(questions: &[Question]) -> Vec<String> {
        questions
            .iter()
            .map(|q| {
                let what = match &q.ask {
                    Ask::Discovery {
                        branch, name, head, ..
                    } => format!(
                        "{branch} as {name}{}",
                        if head.is_some() { " +head" } else { "" }
                    ),
                    Ask::Refresh { number } => format!("#{number}"),
                };
                format!("{} {}/{} {what}", q.host, q.owner, q.repo)
            })
            .collect()
    }

    fn plan_one(
        entry: &PrSyncEntry,
        resolve: &dyn Fn(&Path) -> git::RemoteResolution,
        policy: &GithubHostPolicy,
        trigger: SyncTrigger,
        now: &str,
    ) -> Vec<String> {
        described(&plan_entries(
            std::slice::from_ref(entry),
            resolve,
            policy,
            trigger,
            at(now),
        ))
    }

    #[test]
    fn a_cycle_asks_about_the_current_branch_by_every_name_a_turn_of_earlier_branches_and_each_due_pull_request()
     {
        let mut entry = sync_entry();
        entry.current.former_names = vec!["feat/old".to_string()];
        entry.history = vec![
            PrSyncBranch {
                last_ask: Some(asked_at("2026-11-06T09:00:00Z")),
                ..sync_branch("spike-a")
            },
            sync_branch("spike-b"),
            PrSyncBranch {
                last_ask: Some(asked_at("2026-11-06T11:00:00Z")),
                ..sync_branch("spike-c")
            },
        ];
        entry.history_per_cycle = 2;
        let mut attachment = known_row(7, PrState::Closed, "other/Elsewhere", None);
        attachment.host = "git.example.com".to_string();
        attachment.found = false;
        attachment.attached = true;
        entry.known = vec![attachment];
        let policy = GithubHostPolicy::Hosts(
            ["github.com".to_string(), "git.example.com".to_string()]
                .into_iter()
                .collect(),
        );

        let questions = plan_entries(
            std::slice::from_ref(&entry),
            &ours,
            &policy,
            SyncTrigger::BlindPoll,
            at(NOON),
        );

        assert_eq!(
            described(&questions),
            vec![
                "github.com octocat/Hello-World feat/x as feat/x +head",
                "github.com octocat/Hello-World feat/x as feat/old +head",
                "github.com octocat/Hello-World spike-b as spike-b",
                "github.com octocat/Hello-World spike-a as spike-a",
                "git.example.com other/Elsewhere #7",
            ]
        );
        let (q, pos_repo) = build_chunk_query(&questions, &[0, 1, 2, 3]);
        assert_eq!(pos_repo, vec![0, 0, 0, 0]);
        assert!(q.contains("r0: repository(owner: \"octocat\", name: \"Hello-World\")"));
        assert!(q.contains(
            "q1_ref: ref(qualifiedName: \"refs/heads/feat/old\") { associatedPullRequests(last: 20) \
{ nodes { number state isDraft title url createdAt updatedAt closedAt mergedAt headRefName \
headRefOid repository { nameWithOwner } } } }"
        ));
        assert!(q.contains(
            "q1_head: pullRequests(headRefName: \"feat/old\", states: [OPEN, MERGED, CLOSED], last: 5) \
{ nodes { number state isDraft title url createdAt updatedAt closedAt mergedAt headRefName \
headRefOid repository { nameWithOwner } headRepository { nameWithOwner } } }"
        ));
        assert!(q.contains("q2_ref: ref(qualifiedName: \"refs/heads/spike-b\")"));
        assert!(
            !q.contains("q2_head"),
            "a branch whose ref answered is not looked up by name"
        );
        assert!(
            !q.contains("orderBy"),
            "GitHub ignores orderBy on this connection"
        );
        let (q, _) = build_chunk_query(&questions, &[4]);
        assert!(q.contains("r0: repository(owner: \"other\", name: \"Elsewhere\")"));
        assert!(q.contains("q0_num: pullRequest(number: 7) { number state isDraft"));
    }

    #[test]
    fn every_earlier_branch_is_asked_within_a_bounded_number_of_cycles() {
        let mut entry = sync_entry();
        entry.history = (1..=9).map(|n| sync_branch(&format!("b{n}"))).collect();
        entry.history_per_cycle = 2;
        let mut now = at(NOON);
        let mut asked: Vec<String> = Vec::new();
        for cycle in 1..=5 {
            let due: Vec<String> = due_history_branches(&entry, SyncTrigger::BlindPoll, now)
                .into_iter()
                .map(|b| b.name.clone())
                .collect();
            assert_eq!(due.len(), 2, "cycle {cycle} asks its share");
            for branch in &mut entry.history {
                if due.contains(&branch.name) {
                    branch.last_ask = Some(BranchAskRecord {
                        at: now,
                        ref_gone: false,
                        returned: Vec::new(),
                    });
                }
            }
            asked.extend(due);
            if cycle == 4 {
                let mut seen = asked.clone();
                seen.sort();
                seen.dedup();
                assert_eq!(seen.len(), 8, "four cycles of two cannot cover nine");
            }
            now += chrono::TimeDelta::minutes(3);
        }
        asked.sort();
        asked.dedup();
        assert_eq!(
            asked,
            vec!["b1", "b2", "b3", "b4", "b5", "b6", "b7", "b8", "b9"]
        );
    }

    #[test]
    fn a_branch_with_only_merged_pull_requests_waits_for_the_settled_recheck() {
        let mut entry = sync_entry();
        entry.history = vec![
            PrSyncBranch {
                last_ask: Some(asked_at("2026-11-06T11:00:00Z")),
                ..sync_branch("done")
            },
            PrSyncBranch {
                last_ask: Some(asked_at("2026-11-06T11:59:00Z")),
                ..sync_branch("busy")
            },
        ];
        entry.known = vec![
            known_row(3, PrState::Merged, "octocat/Hello-World", Some("done")),
            known_row(4, PrState::Merged, "octocat/Hello-World", Some("busy")),
            known_row(5, PrState::Closed, "octocat/Hello-World", Some("busy")),
        ];
        let due = |now: &str| -> Vec<String> {
            due_history_branches(&entry, SyncTrigger::BlindPoll, at(now))
                .into_iter()
                .map(|b| b.name.clone())
                .collect()
        };

        assert_eq!(due(NOON), vec!["busy"], "asked an hour ago and settled");
        assert_eq!(due("2026-11-06T22:59:00Z"), vec!["busy"]);
        assert_eq!(due("2026-11-06T23:00:00Z"), vec!["done", "busy"]);
    }

    #[test]
    fn a_pull_request_the_window_did_not_return_is_refreshed_by_number() {
        let mut entry = sync_entry();
        entry.known = vec![known_row(
            5,
            PrState::Open,
            "octocat/Hello-World",
            Some("feat/x"),
        )];
        let returned_it = BranchAskRecord {
            at: at("2026-11-06T11:57:00Z"),
            ref_gone: false,
            returned: vec![crate::model::PrKey::new(
                "github.com",
                "octocat/hello-world",
                5,
            )],
        };
        entry.current.last_ask = Some(returned_it.clone());
        assert_eq!(
            plan_one(
                &entry,
                &ours,
                &legacy_policy(),
                SyncTrigger::BlindPoll,
                NOON
            ),
            vec!["github.com octocat/Hello-World feat/x as feat/x"],
            "the branch's discovery carries it"
        );

        entry.current.last_ask = Some(BranchAskRecord {
            returned: Vec::new(),
            ..returned_it
        });
        let questions = plan_entries(
            std::slice::from_ref(&entry),
            &ours,
            &legacy_policy(),
            SyncTrigger::BlindPoll,
            at(NOON),
        );
        assert_eq!(
            described(&questions),
            vec![
                "github.com octocat/Hello-World feat/x as feat/x",
                "github.com octocat/Hello-World #5",
            ]
        );
        let (_, pos_repo) = build_chunk_query(&questions, &[0, 1]);
        let data = serde_json::json!({
            "r0": {
                "q0_ref": { "associatedPullRequests": { "nodes": [ pr_node(30, "OPEN") ] } },
                "q1_num": pr_node(5, "OPEN"),
            },
        });
        let (answers, _) = parse_chunk_response(
            &questions,
            &[0, 1],
            &pos_repo,
            Some(&data),
            std::slice::from_ref(&entry),
            &|_, _| false,
        );
        let results = assemble_results(std::slice::from_ref(&entry), &questions, answers, at(NOON));
        let got: Vec<String> = results[0]
            .answers
            .iter()
            .map(|answer| match answer {
                PrSyncAnswer::Found(pr) => format!("found #{}", pr.number),
                PrSyncAnswer::Refreshed(pr) => format!("refreshed #{}", pr.number),
                PrSyncAnswer::Unanswered(key) => format!("unanswered #{}", key.number),
            })
            .collect();
        assert_eq!(got, vec!["found #30", "refreshed #5"]);
        assert_eq!(results[0].asked.len(), 1);
        assert_eq!(results[0].asked[0].0, "feat/x");
        assert_eq!(
            results[0].asked[0].1.returned,
            vec![crate::model::PrKey::new(
                "github.com",
                "octocat/Hello-World",
                30
            )]
        );
    }

    #[test]
    fn each_pull_request_is_refreshed_on_the_cadence_its_state_sets() {
        let mut entry = sync_entry();
        let mut migrated_open = known_row(10, PrState::Open, "octocat/Hello-World", None);
        migrated_open.last_refreshed_at = None;
        let mut migrated_merged = known_row(12, PrState::Merged, "octocat/Hello-World", None);
        migrated_merged.last_refreshed_at = None;
        let mut elsewhere = known_row(14, PrState::Merged, "acme/widget", None);
        elsewhere.host = "git.example.com".to_string();
        elsewhere.last_refreshed_at = None;
        entry.known = vec![
            known_row(1, PrState::Open, "octocat/Hello-World", Some("feat/x")),
            known_row(2, PrState::Draft, "octocat/Hello-World", Some("feat/x")),
            known_row(3, PrState::Merged, "octocat/Hello-World", Some("feat/x")),
            known_row(4, PrState::Closed, "octocat/Hello-World", Some("feat/x")),
            migrated_open,
            migrated_merged,
            elsewhere,
        ];
        let refreshes = |entry: &PrSyncEntry, trigger, now| -> Vec<String> {
            plan_one(
                entry,
                &ours,
                &GithubHostPolicy::LegacyNameRule,
                trigger,
                now,
            )
            .into_iter()
            .filter(|q| q.contains('#'))
            .collect()
        };
        let policy = GithubHostPolicy::Hosts(
            ["github.com".to_string(), "git.example.com".to_string()]
                .into_iter()
                .collect(),
        );
        assert_eq!(
            plan_one(&entry, &ours, &policy, SyncTrigger::BlindPoll, NOON)
                .into_iter()
                .filter(|q| q.contains('#'))
                .collect::<Vec<_>>(),
            vec![
                "github.com octocat/Hello-World #1",
                "github.com octocat/Hello-World #2",
                "github.com octocat/Hello-World #4",
                "github.com octocat/Hello-World #10",
                "github.com octocat/Hello-World #12",
                "git.example.com acme/widget #14",
            ],
            "a running agent: everything but a merge, and every row never refreshed, wherever it lives"
        );

        entry.agent_exited = true;
        entry.known.retain(|row| row.last_refreshed_at.is_some());
        assert_eq!(
            refreshes(&entry, SyncTrigger::BlindPoll, NOON),
            vec![
                "github.com octocat/Hello-World #1",
                "github.com octocat/Hello-World #2",
            ],
            "an exited agent's closed pull request waits"
        );
        assert_eq!(
            refreshes(&entry, SyncTrigger::OneShot, NOON),
            vec![
                "github.com octocat/Hello-World #1",
                "github.com octocat/Hello-World #2",
                "github.com octocat/Hello-World #4",
            ],
            "a deliberate trigger asks about it"
        );
        assert_eq!(
            refreshes(&entry, SyncTrigger::BlindPoll, "2026-11-07T00:00:00Z"),
            vec![
                "github.com octocat/Hello-World #1",
                "github.com octocat/Hello-World #2",
                "github.com octocat/Hello-World #4",
            ],
            "and so does the settled recheck"
        );
    }

    #[test]
    fn an_exited_agent_whose_branch_is_finished_is_asked_about_only_on_a_deliberate_trigger() {
        let mut entry = sync_entry();
        entry.agent_exited = true;
        entry.known = vec![known_row(
            42,
            PrState::Merged,
            "octocat/Hello-World",
            Some("feat/x"),
        )];
        // An earlier branch asked an hour ago: an exited agent's earlier
        // branches wait for the settled recheck like its finished one does.
        let mut spike = sync_branch("spike");
        spike.last_ask = Some(asked_at("2026-11-06T11:00:00Z"));
        entry.history = vec![spike];
        for trigger in [SyncTrigger::BlindPoll, SyncTrigger::Focus] {
            assert_eq!(
                plan_one(&entry, &ours, &legacy_policy(), trigger, NOON),
                Vec::<String>::new(),
                "{trigger:?} costs nothing"
            );
            assert_eq!(
                plan_one(
                    &entry,
                    &ours,
                    &legacy_policy(),
                    trigger,
                    "2026-11-07T00:00:00Z"
                ),
                vec!["github.com octocat/Hello-World spike as spike"],
                "{trigger:?} asks again once the settled recheck has passed"
            );
        }
        assert_eq!(
            plan_one(&entry, &ours, &legacy_policy(), SyncTrigger::OneShot, NOON),
            vec![
                "github.com octocat/Hello-World feat/x as feat/x +head",
                "github.com octocat/Hello-World spike as spike",
            ],
            "a reused branch name can carry a new pull request"
        );
        entry.agent_exited = false;
        assert_eq!(
            plan_one(
                &entry,
                &ours,
                &legacy_policy(),
                SyncTrigger::BlindPoll,
                NOON
            ),
            vec![
                "github.com octocat/Hello-World feat/x as feat/x +head",
                "github.com octocat/Hello-World spike as spike",
            ]
        );
    }

    #[test]
    fn discovery_needs_a_remote_dux_may_ask_and_a_refresh_goes_where_its_pull_request_lives() {
        let mut entry = sync_entry();
        let mut capitalised = known_row(7, PrState::Open, "octocat/Hello-World", None);
        capitalised.host = "GitHub.COM".to_string();
        let mut ineligible = known_row(8, PrState::Open, "acme/widget", None);
        ineligible.host = "git.company.example".to_string();
        entry.known = vec![capitalised, ineligible];
        let github = GithubHostPolicy::Hosts(["github.com".to_string()].into_iter().collect());
        let company = |_: &Path| {
            git::resolve_remote_from_git_output(
                b"git@git.company.example:acme/widget.git\n",
                &github,
            )
        };
        assert_eq!(company(Path::new("/")), git::RemoteResolution::Denied);

        let unresolved = |_: &Path| git::RemoteResolution::Unresolved;
        let resolvers: [&dyn Fn(&Path) -> git::RemoteResolution; 2] = [&company, &unresolved];
        for resolve in resolvers {
            assert_eq!(
                plan_one(&entry, resolve, &github, SyncTrigger::BlindPoll, NOON),
                vec!["github.com octocat/Hello-World #7"],
                "no discovery, and nothing at all for the host gh does not serve"
            );
        }
        assert_eq!(
            plan_one(&entry, &ours, &github, SyncTrigger::BlindPoll, NOON),
            vec![
                "github.com octocat/Hello-World feat/x as feat/x +head",
                "github.com octocat/Hello-World #7",
            ]
        );
    }

    #[test]
    fn discovery_keeps_what_the_agent_has_and_what_was_opened_after_the_branch_was_born() {
        let node = |number: u64, state: &str, created: &str| {
            let mut node = pr_node(number, state);
            node["createdAt"] = serde_json::json!(created);
            node["headRefName"] = serde_json::json!("feat/x");
            node
        };
        let nodes = vec![
            node(1, "MERGED", "2026-01-01T00:00:00Z"),
            node(2, "CLOSED", "2026-10-01T00:00:00Z"),
            node(3, "MERGED", "2026-11-01T00:00:00Z"),
            node(4, "CLOSED", "2026-11-02T00:00:00Z"),
        ];
        let mut entry = sync_entry();
        entry.known = vec![known_row(
            1,
            PrState::Merged,
            "octocat/Hello-World",
            Some("feat/x"),
        )];
        let found = |identity_at: Option<&str>, entry: &PrSyncEntry| -> Vec<u64> {
            let mut entry = entry.clone();
            entry.current.identity_at = identity_at.map(at);
            let questions = plan_entries(
                std::slice::from_ref(&entry),
                &ours,
                &legacy_policy(),
                SyncTrigger::OneShot,
                at(NOON),
            );
            let data = serde_json::json!({
                "r0": { "q0_ref": { "associatedPullRequests": { "nodes": nodes } } },
            });
            let (answers, _) = parse_chunk_response(
                &questions,
                &[0],
                &[0],
                Some(&data),
                std::slice::from_ref(&entry),
                &|_, _| false,
            );
            match &answers[0].1 {
                QuestionAnswer::Discovery { found, .. } => {
                    found.iter().map(|pr| pr.number).collect()
                }
                QuestionAnswer::Refresh(_) => panic!("a discovery answer"),
            }
        };

        assert_eq!(
            found(Some("2026-10-31T23:58:00Z"), &entry),
            vec![1, 3, 4],
            "born at the end of October: its own two, and the one it already has"
        );
        assert_eq!(
            found(None, &entry),
            vec![1, 4],
            "nothing says when it was born: only the newest, beside the one it has"
        );
    }

    #[test]
    fn a_discovery_window_drops_other_repositories_and_nodes_it_cannot_read() {
        let mut fork = pr_node_from(4, "OPEN", "contributor/Hello-World");
        fork["createdAt"] = serde_json::json!("2026-11-02T00:00:00Z");
        let mut unreadable = pr_node(5, "OPEN");
        unreadable["state"] = serde_json::json!("QUEUED");
        let mut draft = pr_node(3, "OPEN");
        draft["isDraft"] = serde_json::json!(true);
        draft["headRefName"] = serde_json::json!("feat/x");
        draft["createdAt"] = serde_json::json!("2026-11-01T10:00:00Z");
        draft["mergedAt"] = serde_json::Value::Null;
        let entry = sync_entry();
        let questions = plan_entries(
            std::slice::from_ref(&entry),
            &ours,
            &legacy_policy(),
            SyncTrigger::OneShot,
            at(NOON),
        );
        let data = serde_json::json!({
            "r0": { "q0_ref": { "associatedPullRequests": { "nodes": [draft, fork, unreadable] } } },
        });
        let (answers, _) = parse_chunk_response(
            &questions,
            &[0],
            &[0],
            Some(&data),
            std::slice::from_ref(&entry),
            &|_, _| false,
        );
        let QuestionAnswer::Discovery {
            found,
            returned,
            ref_gone,
        } = &answers[0].1
        else {
            panic!("a discovery answer");
        };
        assert!(!ref_gone);
        assert_eq!(
            returned,
            &vec![crate::model::PrKey::new(
                "github.com",
                "octocat/Hello-World",
                3
            )]
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].number, 3);
        assert!(found[0].is_draft, "isDraft decodes");
        assert_eq!(found[0].head_branch.as_deref(), Some("feat/x"));
        assert_eq!(found[0].gh_created_at, Some(at("2026-11-01T10:00:00Z")));
        assert_eq!(found[0].gh_merged_at, None);
    }

    #[test]
    fn a_host_whose_call_failed_answers_nothing_and_leaves_the_others_answers() {
        use std::os::unix::process::ExitStatusExt;
        let mut entry = sync_entry();
        let mut elsewhere = known_row(7, PrState::Closed, "acme/widget", None);
        elsewhere.host = "git.example.com".to_string();
        entry.known = vec![
            known_row(5, PrState::Open, "octocat/Hello-World", None),
            elsewhere,
        ];
        let policy = GithubHostPolicy::Hosts(
            ["github.com".to_string(), "git.example.com".to_string()]
                .into_iter()
                .collect(),
        );
        let unresolved = |_: &Path| git::RemoteResolution::Unresolved;
        assert_eq!(
            described(&plan_entries(
                std::slice::from_ref(&entry),
                &unresolved,
                &policy,
                SyncTrigger::BlindPoll,
                at(NOON),
            )),
            vec![
                "github.com octocat/Hello-World #5",
                "git.example.com acme/widget #7"
            ]
        );
        // github.com cannot be reached; git.example.com answers with `example`.
        let sync = |example: serde_json::Value,
                    resolve: &dyn Fn(&Path) -> git::RemoteResolution| {
            let answer = serde_json::json!({ "data": example }).to_string();
            run_entries_with(
                std::slice::from_ref(&entry),
                &BackoffSnapshot::new(),
                &policy,
                SyncTrigger::BlindPoll,
                at(NOON),
                resolve,
                &|args: &[&str]| {
                    if args.contains(&"github.com") {
                        GhCallOutcome::Failed("connection refused".to_string())
                    } else {
                        GhCallOutcome::Completed(std::process::Output {
                            status: std::process::ExitStatus::from_raw(0),
                            stdout: answer.clone().into_bytes(),
                            stderr: Vec::new(),
                        })
                    }
                },
            )
        };

        let (results, signals) = sync(
            serde_json::json!({ "r0": { "q0_num": pr_node_from(7, "OPEN", "acme/widget") } }),
            &unresolved,
        );
        let [PrSyncAnswer::Refreshed(pr)] = results[0].answers.as_slice() else {
            panic!("one refresh, got {:?}", results[0].answers);
        };
        assert_eq!((pr.number, pr.state.clone()), (7, PrState::Open));
        assert_eq!(pr.host, "git.example.com");
        let failed: Vec<(&str, bool)> = signals
            .iter()
            .map(|signal| (signal.host.as_str(), signal.hard_failed))
            .collect();
        assert_eq!(
            failed,
            vec![("git.example.com", false), ("github.com", true)]
        );

        // The worktree's remote is the agent's own repository, reported even
        // though the host it is on answered nothing.
        let (results, _) = sync(serde_json::json!({ "r0": null }), &|_: &Path| {
            git::RemoteResolution::Allowed(git::GitHubRemote {
                host: "GitHub.com".to_string(),
                owner_repo: "octocat/Hello-World".to_string(),
            })
        });
        assert_eq!(
            results[0].own_repo,
            Some(git::GitHubRemote {
                host: "github.com".to_string(),
                owner_repo: "octocat/Hello-World".to_string(),
            })
        );
        let [PrSyncAnswer::Unanswered(key)] = results[0].answers.as_slice() else {
            panic!(
                "a repository that answered null says only that it was asked, got {:?}",
                results[0].answers
            );
        };
        assert_eq!(
            key,
            &crate::model::PrKey::new("git.example.com", "acme/widget", 7)
        );
    }

    /// The agent's worktree with a branch `feat/x` two commits ahead of `main`,
    /// plus an unrelated commit on `other`: the repository, the branch's tip,
    /// its first commit and the stray commit.
    fn repo_with_feature_branch() -> (tempfile::TempDir, String, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| -> String {
            let out = crate::git::test_support::git_command()
                .args(args)
                .current_dir(dir.path())
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?} failed");
            String::from_utf8(out.stdout).unwrap().trim().to_string()
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.name", "test"]);
        git(&["config", "user.email", "t@t"]);
        git(&["commit", "-q", "--allow-empty", "-m", "init"]);
        git(&["switch", "-q", "-c", "other"]);
        git(&["commit", "-q", "--allow-empty", "-m", "stray"]);
        let stray = git(&["rev-parse", "HEAD"]);
        git(&["switch", "-q", "main"]);
        git(&["switch", "-q", "-c", "feat/x"]);
        git(&["commit", "-q", "--allow-empty", "-m", "one"]);
        let first = git(&["rev-parse", "HEAD"]);
        git(&["commit", "-q", "--allow-empty", "-m", "two"]);
        let tip = git(&["rev-parse", "HEAD"]);
        (dir, tip, first, stray)
    }

    fn head_node(
        number: u64,
        created_at: &str,
        head_oid: &str,
        head_repo: Option<&str>,
    ) -> serde_json::Value {
        let mut node = pr_node(number, "MERGED");
        node["createdAt"] = serde_json::json!(created_at);
        node["headRefOid"] = serde_json::json!(head_oid);
        node["headRepository"] =
            serde_json::json!(head_repo.map(|name| serde_json::json!({ "nameWithOwner": name })));
        node
    }

    /// What one cycle finds for `entry` with `branch` as its only earlier
    /// branch, whose ref answers null and whose head-name lookup answers
    /// `nodes`.
    fn gone_branch_lookup(
        entry: &PrSyncEntry,
        branch: PrSyncBranch,
        nodes: &[serde_json::Value],
    ) -> Vec<u64> {
        let mut entry = entry.clone();
        entry.history = vec![branch];
        let questions = plan_entries(
            std::slice::from_ref(&entry),
            &ours,
            &legacy_policy(),
            SyncTrigger::BlindPoll,
            at(NOON),
        );
        assert_eq!(
            described(&questions)[1],
            format!(
                "github.com octocat/Hello-World {} as {} +head",
                entry.history[0].name, entry.history[0].name
            )
        );
        let data = serde_json::json!({
            "r0": {
                "q0_ref": { "associatedPullRequests": { "nodes": [] } },
                "q1_ref": null,
                "q1_head": { "nodes": nodes },
            },
        });
        let (answers, _) = parse_chunk_response(
            &questions,
            &[0, 1],
            &[0, 0],
            Some(&data),
            std::slice::from_ref(&entry),
            &head_commit_is_the_branchs,
        );
        let results = assemble_results(std::slice::from_ref(&entry), &questions, answers, at(NOON));
        assert!(
            results[0]
                .asked
                .iter()
                .any(|(name, ask)| name == &entry.history[0].name && ask.ref_gone)
        );
        results[0]
            .answers
            .iter()
            .map(|answer| match answer {
                PrSyncAnswer::Found(pr) | PrSyncAnswer::Refreshed(pr) => pr.number,
                PrSyncAnswer::Unanswered(key) => key.number,
            })
            .collect()
    }

    /// An earlier branch whose ref a merge deleted is looked up by head name
    /// the next time it is due, and a pull request found that way is taken only
    /// when its head commit is the branch's tip, in the history of its local
    /// branch, or behind its recorded tip.
    #[test]
    fn a_gone_branchs_pull_request_is_found_by_head_name_only_on_the_branchs_own_commits() {
        let (dir, tip, first, stray) = repo_with_feature_branch();
        let mut entry = sync_entry();
        entry.worktree_path = dir.path().to_string_lossy().into_owned();
        entry.current = sync_branch("trunk");
        entry.known = vec![known_row(
            9,
            PrState::Open,
            "octocat/Hello-World",
            Some("trunk"),
        )];
        let gone = |tip: &str, local: &str| PrSyncBranch {
            identity_at: Some(at("2026-10-09T09:00:00Z")),
            tip: Some(tip.to_string()),
            last_ask: Some(BranchAskRecord {
                at: at("2026-11-06T11:00:00Z"),
                ref_gone: true,
                returned: Vec::new(),
            }),
            ..sync_branch(local)
        };
        let lookup =
            |branch, nodes: Vec<serde_json::Value>| gone_branch_lookup(&entry, branch, &nodes);
        let ours_repo = Some("octocat/Hello-World");

        assert_eq!(
            lookup(
                gone(&tip, "gone"),
                vec![head_node(72, "2026-10-09T10:00:00Z", &tip, ours_repo)]
            ),
            vec![72],
            "its recorded tip"
        );
        assert_eq!(
            lookup(
                gone(&tip, "gone"),
                vec![head_node(72, "2026-10-09T10:00:00Z", &first, ours_repo)]
            ),
            vec![72],
            "behind its recorded tip"
        );
        assert_eq!(
            lookup(
                gone("0123456789abcdef0123456789abcdef01234567", "feat/x"),
                vec![head_node(72, "2026-10-09T10:00:00Z", &first, ours_repo)]
            ),
            vec![72],
            "in the history of the local branch that is still there"
        );
        assert_eq!(
            lookup(
                gone(&tip, "gone"),
                vec![head_node(72, "2026-10-09T10:00:00Z", &stray, ours_repo)]
            ),
            Vec::<u64>::new(),
            "a reused name whose commit is not the branch's"
        );
        assert_eq!(
            lookup(
                gone(&tip, "gone"),
                vec![head_node(
                    72,
                    "2026-10-09T10:00:00Z",
                    &tip,
                    Some("contributor/Hello-World")
                )]
            ),
            Vec::<u64>::new(),
            "a fork's pull request is somebody else's"
        );
        assert_eq!(
            lookup(
                gone(&tip, "gone"),
                vec![head_node(72, "2026-10-01T10:00:00Z", &tip, ours_repo)]
            ),
            Vec::<u64>::new(),
            "opened before the branch was born"
        );
    }

    #[test]
    fn format_sync_cycle_summary_names_the_trigger_the_counts_and_the_cost() {
        assert_eq!(
            format_sync_cycle_summary(SyncTrigger::BlindPoll, 5, 12, 9, 2),
            "PR sync (blind poll): 5 agents, 12 questions, 9 answered, 2 GraphQL points"
        );
        assert_eq!(
            format_sync_cycle_summary(SyncTrigger::Focus, 1, 0, 0, 0),
            "PR sync (focus): 1 agents, 0 questions, 0 answered, 0 GraphQL points"
        );
        assert_eq!(
            format_sync_cycle_summary(SyncTrigger::OneShot, 1, 1, 0, 1),
            "PR sync (one-shot): 1 agents, 1 questions, 0 answered, 1 GraphQL points"
        );
    }

    /// A thread that panicked holding the plan leaves a plan that is still
    /// whole, so the sync keeps reading it rather than freezing every PR.
    #[test]
    fn a_poisoned_plan_is_still_synced() {
        let plan = Arc::new(Mutex::new(vec![sync_entry()]));
        let poisoner = Arc::clone(&plan);
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.lock().expect("lock");
            panic!("poison the plan");
        })
        .join();

        let (results, _) = run_pr_sync(
            &plan,
            &std::collections::HashMap::new(),
            &legacy_policy(),
            SyncTrigger::BlindPoll,
        );

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].session_id, "s0");
    }

    #[test]
    fn an_active_only_cycle_leaves_the_inactive_entries_alone() {
        let mut working = sync_entry();
        working.session_id = "working".to_string();
        let mut dormant = sync_entry();
        dormant.session_id = "dormant".to_string();
        dormant.inactive = true;
        let plan = vec![working, dormant];

        let narrowed: Vec<String> = entries_for_scope(plan.clone(), SyncScope::ActiveOnly)
            .into_iter()
            .map(|e| e.session_id)
            .collect();
        assert_eq!(narrowed, vec!["working".to_string()]);

        let whole: Vec<String> = entries_for_scope(plan, SyncScope::Everything)
            .into_iter()
            .map(|e| e.session_id)
            .collect();
        assert_eq!(
            whole,
            vec!["working".to_string(), "dormant".to_string()],
            "every other cycle still asks about both"
        );
    }
}
