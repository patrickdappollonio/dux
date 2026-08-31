//! GitHub CLI (`gh`) integration helpers used by the PR-sync worker
//! (`spawn_pr_sync_worker`, `spawn_initial_pr_refresh`, `spawn_pr_check_for_session`).
//! All helpers shell out to `gh` and parse JSON; no UI deps.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::Path;
use std::process::Stdio;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::git;
use crate::logger;
use crate::model::{PrInfo, PrState, Project};
use crate::storage::StoredPr;
use crate::worker::{
    PrLookupPurpose, PrSyncEntry, PullRequestLookup, ResolvedPullRequest, WorkerEvent,
};

/// Live GraphQL rate-limit snapshot parsed from a batched query's top-level
/// `rateLimit` field. Lets the PR-sync loop back off before it exhausts the
/// GraphQL points budget (typically 5000/hour, higher on GitHub Enterprise Cloud).
#[derive(Clone, Debug)]
pub struct RateLimitInfo {
    pub remaining: i64,
    pub reset_at: Option<chrono::DateTime<chrono::Utc>>,
    /// What this one query cost, in GraphQL points. Already asked for, and what
    /// the cycle summary reports: the widened discovery window is a cost
    /// question, so the log has to be able to answer it.
    pub cost: Option<i64>,
}

/// Per-host outcome of a sync cycle, used to drive the per-host backoff. Only
/// hosts that were actually queried this cycle appear (a host skipped because it
/// is already backed off, or one with only zero-network sessions, is absent — so
/// its backoff window is left untouched rather than spuriously cleared).
pub struct HostSignal {
    pub host: String,
    pub rate: Option<RateLimitInfo>,
    /// A whole-call failure (spawn error / timeout / unparseable-or-null data) on
    /// at least one of this host's chunks.
    pub hard_failed: bool,
    /// The failure looked like GitHub rate-limiting (a 403 / secondary limit, or a
    /// `RATE_LIMITED` GraphQL error), as opposed to a network/`gh` error. Lets the
    /// status message say so instead of a vague "network or gh error".
    pub rate_limited: bool,
}

/// One batched-sync outcome: the per-session PR results plus a per-host signal
/// for every host actually queried this cycle.
type PrSyncOutcome = (Vec<(String, Option<PrInfo>)>, Vec<HostSignal>);

/// One chunk's outcome: per-session results, the chunk's `rateLimit` snapshot,
/// whether the whole call hard-failed, and whether that failure looked like
/// rate-limiting. Aggregated per host by `run_entries`.
type ChunkOutcome = (
    Vec<(String, Option<PrInfo>)>,
    Option<RateLimitInfo>,
    bool,
    bool,
);

/// Snapshot of the active per-host backoff windows (`host -> until`) passed into
/// a sync so backed-off hosts are skipped (their sessions keep last-known PRs)
/// without another `gh` call.
pub type BackoffSnapshot = std::collections::HashMap<String, Instant>;

/// GraphQL `rateLimit.remaining` floor below which the loop pauses polling
/// until the quota resets.
pub const RATE_LIMIT_BACKOFF_FLOOR: i64 = 100;

/// Max GraphQL aliases per `gh api graphql` invocation. Each session emits 1–2
/// aliases; capping the batch keeps every query ~1 GraphQL point AND stays well
/// under Linux's ~128 KiB per-argv-entry limit for the inlined `-f query=` arg.
const MAX_ALIASES_PER_QUERY: usize = 100;

/// How many of a branch's pull requests the discovery alias asks for. The
/// connection answers oldest-first by creation date and the alias walks it from
/// the newest end, so this is "the most recently created N pull requests on this
/// branch name". A branch reused a couple of times needs a handful.
///
/// What the window cannot see: if the newest twenty are all merged or closed
/// while an OLDER one is open, discovery reports a terminal pull request and the
/// open one stays invisible. That takes a branch name used more than twenty
/// times with a long-lived open pull request underneath it; paging the whole
/// history of every branch on every cycle is not worth covering it.
///
/// The cost, measured rather than assumed: the query still bills one GraphQL
/// point, exactly as `first: 1` did, so the quota is unaffected. What grows is
/// the response body, up to about 17x on a hot ref that really does have twenty
/// pull requests, worth roughly 5% of the call's wall time. Most refs have one.
const DISCOVERY_WINDOW: usize = 20;

/// Hard wall-clock cap on a single `gh` invocation. A hung `gh` (stalled TCP,
/// DNS hang, credential-helper prompt) must not park a worker thread: we bail
/// fast and let the next cycle retry rather than block for long.
pub(crate) const GH_CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// After we kill a timed-out/failed `gh`, how long to wait for its output-reader
/// threads to drain before abandoning them (they finish on their own once the
/// pipe closes). Bounds the wait so a pathological grandchild holding the pipe
/// open can never freeze the caller.
const GH_READER_DRAIN: Duration = Duration::from_secs(2);

/// Batched PR sync over the shared session snapshot. Issues one or more
/// `gh api graphql` requests per GitHub host (sessions chunked to at most
/// `MAX_ALIASES_PER_QUERY` aliases each), aliasing every session's lookup into a
/// single query per chunk, and returns one `(session_id, Option<PrInfo>)` per
/// session plus a per-host signal for the backoff. Hosts already backed off in
/// `backoff` are skipped (their sessions keep last-known PRs) with no `gh` call.
pub fn run_pr_sync(
    sessions: &Arc<Mutex<Vec<PrSyncEntry>>>,
    backoff: &BackoffSnapshot,
    policy: &GithubHostPolicy,
    trigger: SyncTrigger,
) -> PrSyncOutcome {
    let snapshot = match sessions.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => return (Vec::new(), Vec::new()),
    };
    run_entries(&snapshot, backoff, policy, trigger)
}

/// What caused this sync, which decides how much a dormant session is worth
/// spending a call on.
///
/// It is a CALL-SITE parameter rather than a field on `PrSyncEntry` on purpose:
/// the blind poll and the initial refresh read the same shared entries vector,
/// so a per-entry field could not tell them apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncTrigger {
    /// The periodic poll loop, running on its own with nobody asking.
    BlindPoll,
    /// An agent was brought to the foreground. Frequent and cheap to provoke:
    /// tabbing down a sidebar fires one per agent passed through, so it buys
    /// nothing for a dormant agent whose pull request is already terminal.
    Focus,
    /// A deliberate event: boot, a refs change, an agent exit, or the user
    /// asking. Rare, and each one is a reason to believe something moved, so it
    /// is the only trigger that spends a call on a terminal row.
    OneShot,
}

/// Single-session PR check (foreground / refs-watcher / exit triggers). Shares
/// the batched machinery with a one-element batch; returns the PR plus the
/// per-host signal so the one-shot caller can arm/clear the shared backoff too.
pub fn check_pr_for_entry(
    entry: &PrSyncEntry,
    backoff: &BackoffSnapshot,
    policy: &GithubHostPolicy,
    trigger: SyncTrigger,
) -> (Option<PrInfo>, Vec<HostSignal>) {
    let (results, signals) = run_entries(std::slice::from_ref(entry), backoff, policy, trigger);
    let pr = results.into_iter().next().and_then(|(_, pr)| pr);
    (pr, signals)
}

/// A session that needs at least one GraphQL alias this cycle.
struct Planned {
    session_id: String,
    host: String,
    owner: String,
    repo: String,
    branch: String,
    known: Option<StoredPr>,
    is_terminal: bool,
    /// Open known PRs also get a by-number alias (robust when the branch was
    /// deleted on merge). Terminal-but-running and undiscovered sessions get
    /// only the head-ref discovery alias.
    emit_num: bool,
    /// Whether the head-ref discovery alias is emitted at all. True for every
    /// remote-derived plan; false for a PINNED session, whose only alias is the
    /// by-number refresh of the pin (discovery would answer for a PR the user
    /// deliberately overrode).
    emit_ref: bool,
    /// A manually attached PR: the plan targets the PIN's repo and number, and
    /// the merge rule reports the pin (or its stored reconstruction), never a
    /// discovery result.
    pinned: bool,
}

/// Single source of truth for "this stored PR is terminal (MERGED/CLOSED)".
/// This is the PLANNING shape: a terminal row gets discovery only and no
/// by-number alias, so a reopen is still noticed through the discovery node at
/// no extra API cost. Whether a terminal row may skip the network entirely is a
/// narrower question, answered by [`exited_entry_needs_no_network`].
fn stored_pr_is_terminal(known: Option<&StoredPr>) -> bool {
    known.is_some_and(|k| k.state == "MERGED" || k.state == "CLOSED")
}

/// A MERGED stored PR: the one state dux treats as final, in the sense that the
/// pull request itself can no longer change. That makes another call pointless
/// only where the call could not learn anything else, which is the PINNED case;
/// an unpinned session's call is a discovery call, and the BRANCH can still
/// grow a successor. See [`pinned_exited_entry_needs_no_network`].
fn stored_pr_is_merged(known: Option<&StoredPr>) -> bool {
    known.is_some_and(|k| k.state == "MERGED")
}

/// Whether an EXITED agent's entry can be answered from SQLite with no `gh`
/// call at all.
///
/// A terminal row (MERGED or CLOSED) on an exited agent is free on the blind
/// poll and on focus, and costs one discovery call on a deliberate trigger.
///
/// It is not free forever, because a terminal row does not mean the BRANCH is
/// finished: a closed pull request can be reopened, and a branch name reused
/// after a merge carries a brand new pull request. Discovery is the only thing
/// that notices either, so a row that never asked again could never heal.
///
/// It is not paid for on every focus either. Focusing an agent is a navigation
/// keystroke, and a sidebar full of finished agents would spawn one `gh` process
/// per agent tabbed past, for a branch nobody has pushed to in weeks. Boot, a
/// refs change, an agent exit and an explicit ask are rare and each one means
/// something plausibly moved, so those pay.
///
/// A terminal row on a RUNNING agent refreshes under every trigger.
fn exited_entry_needs_no_network(known: Option<&StoredPr>, trigger: SyncTrigger) -> bool {
    stored_pr_is_terminal(known) && trigger != SyncTrigger::OneShot
}

/// The same question for a PINNED session, which emits no discovery alias at
/// all: its only call is a by-number refresh of the pull request the user named.
/// So the reused-branch case above cannot arise here and a MERGED pin stays free
/// under every trigger, while a CLOSED one, which can be reopened, follows the
/// same trigger rule as an unpinned terminal row.
fn pinned_exited_entry_needs_no_network(known: &StoredPr, trigger: SyncTrigger) -> bool {
    stored_pr_is_merged(Some(known))
        || (stored_pr_is_terminal(Some(known)) && trigger != SyncTrigger::OneShot)
}

impl Planned {
    /// Build a planned lookup, deriving the terminal/emit_num eligibility from
    /// the known PR so the single rule lives in one place (production and tests
    /// both go through here and can't drift).
    fn new(
        session_id: String,
        host: String,
        owner: String,
        repo: String,
        branch: String,
        known: Option<StoredPr>,
    ) -> Self {
        let is_terminal = stored_pr_is_terminal(known.as_ref());
        // Open known PRs also get a by-number alias; terminal-but-running and
        // undiscovered sessions get only the head-ref discovery alias. The
        // by-number alias fires ONLY when the stored row names the repo being
        // queried: PR numbers are per-repo, so a row from another repository
        // (a detached fork pin, a remote that changed under the session) asked
        // at this target would fetch an unrelated pull request. Such a row
        // falls back to discovery only; a discovery miss keeps the stored
        // badge via the merge rule's fallback.
        let known_matches_target = known.as_ref().is_some_and(|k| {
            k.owner_repo
                .eq_ignore_ascii_case(&format!("{owner}/{repo}"))
                && normalize_github_host(&k.host).eq_ignore_ascii_case(&host)
        });
        let emit_num = known_matches_target && !is_terminal;
        Planned {
            session_id,
            host,
            owner,
            repo,
            branch,
            known,
            is_terminal,
            emit_num,
            emit_ref: true,
            pinned: false,
        }
    }

    /// Build the plan for a PINNED session: exactly one alias, the by-number
    /// refresh of the pin, against the pin's own repo. No head-ref discovery.
    /// `known` is the override row (which carries the pin's cached state), so
    /// the num alias number and every fallback reconstruct the PIN.
    fn new_pinned(
        session_id: String,
        host: String,
        owner: String,
        repo: String,
        branch: String,
        known: StoredPr,
    ) -> Self {
        let is_terminal = stored_pr_is_terminal(Some(&known));
        Planned {
            session_id,
            host,
            owner,
            repo,
            branch,
            known: Some(known),
            is_terminal,
            emit_num: true,
            emit_ref: false,
            pinned: true,
        }
    }
}

/// Core of the batched sync. Classifies each entry, resolves the ones that need
/// no network call (terminal + exited → reconstruct from SQLite), and batches
/// the rest into `gh api graphql` requests grouped by host.
///
/// Per-session strategy:
///
/// | Known PR state | Agent running? | Aliases                                   |
/// |----------------|----------------|-------------------------------------------|
/// | None           | any            | head-ref discovery                        |
/// | OPEN           | any            | head-ref discovery **+** by-number refresh|
/// | MERGED/CLOSED  | yes            | head-ref discovery (catches a follow-up PR)|
/// | MERGED/CLOSED  | no             | zero calls, except discovery on a deliberate trigger |
///
/// A PINNED session is the exception: it emits only the by-number refresh, so a
/// merged pin on an exited agent costs zero calls under every trigger.
fn run_entries(
    entries: &[PrSyncEntry],
    backoff: &BackoffSnapshot,
    policy: &GithubHostPolicy,
    trigger: SyncTrigger,
) -> PrSyncOutcome {
    let (mut results, planned) = plan_entries(
        entries,
        &|path| live_remote_resolver(path, policy),
        policy,
        trigger,
    );

    // Group by host; for each host either skip it (already backed off — keep
    // last-known PRs, no gh call, no signal) or chunk its sessions by alias
    // budget and emit one per-host signal driving the backoff.
    let now = Instant::now();
    let mut signals: Vec<HostSignal> = Vec::new();
    // What GitHub billed for the whole cycle, summed over every chunk actually
    // run. The per-host signal keeps only the TIGHTEST snapshot (the backoff
    // cares about what is left, not what was spent), so the total is
    // accumulated here instead.
    let mut points = 0i64;
    let mut by_host: std::collections::BTreeMap<String, Vec<usize>> = Default::default();
    for (i, p) in planned.iter().enumerate() {
        by_host.entry(p.host.clone()).or_default().push(i);
    }
    for (host, idxs) in by_host {
        // Host is under an active backoff window: preserve last-known PRs and skip
        // the network call entirely (the window will expire on its own).
        if backoff.get(&host).is_some_and(|until| now < *until) {
            for i in idxs {
                let p = &planned[i];
                results.push((
                    p.session_id.clone(),
                    p.known.as_ref().and_then(reconstruct_from_stored),
                ));
            }
            continue;
        }

        let mut rate: Option<RateLimitInfo> = None;
        let mut hard_failed = false;
        let mut rate_limited = false;
        let mut chunk: Vec<usize> = Vec::new();
        let mut alias_count = 0usize;
        for i in idxs {
            let cost = (planned[i].emit_ref as usize) + (planned[i].emit_num as usize);
            if !chunk.is_empty() && alias_count + cost > MAX_ALIASES_PER_QUERY {
                let (r, rl, failed, limited) = run_chunk(&host, &planned, &chunk);
                results.extend(r);
                points += chunk_cost(rl.as_ref());
                rate = tighter_rate_limit(rate, rl);
                hard_failed |= failed;
                rate_limited |= limited;
                chunk.clear();
                alias_count = 0;
            }
            chunk.push(i);
            alias_count += cost;
        }
        if !chunk.is_empty() {
            let (r, rl, failed, limited) = run_chunk(&host, &planned, &chunk);
            results.extend(r);
            points += chunk_cost(rl.as_ref());
            rate = tighter_rate_limit(rate, rl);
            hard_failed |= failed;
            rate_limited |= limited;
        }
        signals.push(HostSignal {
            host,
            rate,
            hard_failed,
            rate_limited,
        });
    }

    log_sync_cycle(entries, planned.len(), points, &results, trigger);

    (results, signals)
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
/// Returns the results that are already settled (an unresolvable entry, or one
/// reconstructed from SQLite) alongside the lookups still to be run.
///
/// The remote resolver is a parameter so the planning can be exercised without
/// git. In production it is always [`live_remote_resolver`], which is the real
/// thing and shells out; a test supplies the answer directly. This is what stops
/// a planning test from being decided by the DEVELOPER's git configuration: the
/// resolver shells out with no isolation (deliberately, since dux wants
/// `url.*.insteadOf` applied for real), so an inherited rewrite reaches
/// straight into any test that uses it (measured, not supposed: a rewrite
/// mapping the fixture's GitLab address onto github.com fails the negative
/// test, and one mapping github.com onto gitlab.com fails the positive one;
/// neither would be testing the remote spelling it names).
fn plan_entries(
    entries: &[PrSyncEntry],
    resolve_remote: &dyn Fn(&Path) -> git::RemoteResolution,
    policy: &GithubHostPolicy,
    trigger: SyncTrigger,
) -> (Vec<(String, Option<PrInfo>)>, Vec<Planned>) {
    let mut results: Vec<(String, Option<PrInfo>)> = Vec::new();
    let mut planned: Vec<Planned> = Vec::new();

    for entry in entries {
        // A PINNED session short-circuits the remote-derived target entirely:
        // the user named the PR, so the query goes to the pin's (host,
        // owner_repo), the policy gates the PINNED host, and no worktree
        // remote is resolved (a pin routinely lives on a fork the remote does
        // not name). Every fallback below answers from the PIN's row, never
        // from `known_pr` raw: a stale `session_prs` latest naming a different
        // PR must not surface as a pinned session's answer.
        if let Some(pin) = &entry.pinned {
            // The override row is the pin's known state. A `known_pr` naming a
            // DIFFERENT number is not the pin (a stale row from before the
            // attach); synthesize an OPEN placeholder from the pin's identity
            // instead so no fallback can report the wrong pull request.
            let known = entry
                .known_pr
                .clone()
                .filter(|k| k.pr_number == pin.number)
                .unwrap_or_else(|| StoredPr {
                    session_id: entry.session_id.clone(),
                    pr_number: pin.number,
                    host: pin.host.clone(),
                    owner_repo: pin.owner_repo.clone(),
                    state: "OPEN".to_string(),
                    title: String::new(),
                    url: pull_request_url(&pin.host, &pin.owner_repo, pin.number),
                });
            let host = normalize_github_host(&pin.host).to_ascii_lowercase();
            // The gate runs on the PINNED host (the host the query would go
            // to), exactly like the remote-derived gate below.
            if !policy.allows(&host) {
                results.push((entry.session_id.clone(), reconstruct_from_stored(&known)));
                continue;
            }
            // Terminal pin + exited agent: zero network, under the pinned rule
            // (no discovery alias exists, so a merged pin stays free).
            if entry.agent_exited && pinned_exited_entry_needs_no_network(&known, trigger) {
                results.push((entry.session_id.clone(), reconstruct_from_stored(&known)));
                continue;
            }
            let Some((owner, repo)) = pin.owner_repo.split_once('/') else {
                results.push((entry.session_id.clone(), reconstruct_from_stored(&known)));
                continue;
            };
            planned.push(Planned::new_pinned(
                entry.session_id.clone(),
                host,
                owner.to_string(),
                repo.to_string(),
                entry.branch_name.clone(),
                known,
            ));
            continue;
        }

        // Resolve (host, owner_repo): live remote first, else the known PR's repo
        // (works even after the branch/remote is gone).
        //
        // The live resolution has THREE outcomes and each wants its own
        // handling. Only an UNRESOLVED address may fall back to a remembered
        // host: nothing is known about where this agent pushes, so the last
        // pull request is the best information there is. A DENIED address is
        // the opposite case, and it used to be indistinguishable from the
        // first: dux knows exactly where this agent pushes and knows it may not
        // ask about it, so falling back sent the query to the stored host, a
        // host this agent's address does not name. The gate below cannot catch
        // that, because by then the live address is gone.
        let (host, owner_repo) = match resolve_remote(Path::new(&entry.worktree_path)) {
            git::RemoteResolution::Allowed(remote) => (remote.host, remote.owner_repo),
            git::RemoteResolution::Denied => {
                let pr = entry.known_pr.as_ref().and_then(reconstruct_from_stored);
                results.push((entry.session_id.clone(), pr));
                continue;
            }
            git::RemoteResolution::Unresolved => match &entry.known_pr {
                Some(known) => (known.host.clone(), known.owner_repo.clone()),
                None => {
                    results.push((entry.session_id.clone(), None));
                    continue;
                }
            },
        };

        // Hostnames are case-insensitive and this value becomes a `gh`
        // `--hostname` argument. The parser lowercases a live remote, but this
        // host can also come straight out of SQLite, where a legacy or
        // externally written row may have kept its capitals, so the lowercasing
        // belongs here, at the boundary, whatever the source.
        let host = normalize_github_host(&host).to_ascii_lowercase();

        // The eligibility test belongs HERE, after the choice between the live
        // address and the stored one, and not inside the parsers alone. A host
        // remembered from a previous pull request never passes through either
        // parser: it is read back from SQLite and handed to `gh`. So a host that
        // was eligible once, or that was written before dux asked `gh` which
        // hosts it can serve, would otherwise reach `gh` unchecked. An entry
        // dux may not ask about makes NO call at all and keeps whatever it last
        // knew, rather than being reported as having no pull request.
        if !policy.allows(&host) {
            let pr = entry.known_pr.as_ref().and_then(reconstruct_from_stored);
            results.push((entry.session_id.clone(), pr));
            continue;
        }

        // Terminal PR (merged or closed) + exited agent: reconstruct from SQLite
        // with zero network calls, unless this is a deliberate trigger. Both
        // states follow the same rule now; see `exited_entry_needs_no_network`
        // for which triggers pay for a discovery call and why.
        if entry.agent_exited && exited_entry_needs_no_network(entry.known_pr.as_ref(), trigger) {
            let pr = entry.known_pr.as_ref().and_then(reconstruct_from_stored);
            results.push((entry.session_id.clone(), pr));
            continue;
        }

        let Some((owner, repo)) = owner_repo.split_once('/') else {
            // Malformed owner/repo — nothing we can query; fall back to stored.
            let pr = entry.known_pr.as_ref().and_then(reconstruct_from_stored);
            results.push((entry.session_id.clone(), pr));
            continue;
        };

        planned.push(Planned::new(
            entry.session_id.clone(),
            host,
            owner.to_string(),
            repo.to_string(),
            entry.branch_name.clone(),
            entry.known_pr.clone(),
        ));
    }

    (results, planned)
}

/// Keep the snapshot with the fewer remaining points (the more urgent backoff
/// signal) across chunked calls.
/// What one chunk's query billed, or zero when the call failed or the answer
/// carried no `rateLimit` (an unbilled call is not a billed one).
fn chunk_cost(rate: Option<&RateLimitInfo>) -> i64 {
    rate.and_then(|r| r.cost).unwrap_or(0)
}

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
// these helpers — a divergence would silently return "no PR" for every session.
fn repo_alias(k: usize) -> String {
    format!("r{k}")
}
fn ref_alias(pos: usize) -> String {
    format!("s{pos}_ref")
}
fn num_alias(pos: usize) -> String {
    format!("s{pos}_num")
}

/// Which hosts dux may name when it calls `gh`.
///
/// This replaces the name-based guess (`github.com` or `github.*`) that decided
/// it before, which rejected a company server at `git.company.example` purely on
/// the strength of its name. The policy is computed by asking `gh` which hosts it
/// can actually serve; it is stored on the engine and passed explicitly to its
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
    /// Whether `host` may be handed to `gh`. Compared lowercased, and compared
    /// EXACTLY otherwise. An empty host never qualifies: callers that mean
    /// github.com say so (see `normalize_github_host`), and treating "" as
    /// github.com here would let an unparsed remote through the gate.
    ///
    /// Lowercasing is a normalisation this function may perform because it
    /// changes no answer: hostnames are case-insensitive, and every caller
    /// lowercases before it gets here anyway. TRIMMING is not, and doing it
    /// here WIDENED THE REMOTE GRAMMAR from the far side. `git@ github.com:o/r`
    /// (a space after the at sign) is a literal address with an interior space,
    /// so the scp-like branch reads the host as `" github.com"` and hands it
    /// here; a trim made this answer for `github.com`, a DIFFERENT host, and
    /// the caller then returned the host it was holding, spaces and all, to be
    /// handed to `gh`. Whitespace in a host is a defect in the caller or in the
    /// address, and the answer to a defect is no. It is refused OUTRIGHT rather
    /// than left to each mode to fail on its own, because one of them would
    /// not: `LegacyNameRule` accepts anything beginning `github.`, so
    /// `"github.com "` matched the prefix on its own merits even with the trim
    /// gone, and `git@github.com :o/r` resolved to a host with a space on it.
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
        available: bool,
        policy: GithubHostPolicy,
    },
}

/// The shape of `gh auth status --active --json hosts`, typed so that anything
/// that is not that shape fails to deserialize instead of being read loosely.
///
/// Only the three fields the decision needs are named; `gh` sends more (login,
/// scopes, gitProtocol, tokenSource) and serde ignores them, so a future field
/// cannot break this.
#[derive(serde::Deserialize)]
struct AuthStatusOutput {
    hosts: std::collections::BTreeMap<String, Vec<AuthStatusAccount>>,
}

/// One account under one host key.
///
/// `state`, `active` and `host` are REQUIRED, so a record missing one, or
/// carrying null or the wrong type in one, fails to deserialize and takes the
/// whole response down with it. That is deliberate, and it is the difference
/// between "gh says no" and "dux could not read what gh said". Optional fields
/// would instead produce two decisive-looking answers out of records that decide
/// nothing: a missing or null `active` yields an empty but
/// successfully parsed host set, which is a decisive "gh serves nothing" that
/// turns every GitHub feature off and replaces the last known good policy; and a
/// missing or null `host` alongside a successful, active record qualifies the MAP
/// KEY on the strength of a record that never says which host it describes.
///
/// A response containing an unreadable record is therefore transient (see
/// [`decide_gh_probe`]), which preserves the last known good policy. gh 2.95.0
/// emits all three fields on every account.
///
/// `error` is the exception and defaults, because gh tags it `omitempty` and so
/// omits it entirely on a healthy account. Its ABSENCE therefore says nothing
/// and must not be able to fail the parse; it is `state` that says whether the
/// account works, and this only says why when it does not.
#[derive(serde::Deserialize)]
struct AuthStatusAccount {
    state: String,
    active: bool,
    host: String,
    #[serde(default)]
    error: String,
}

impl AuthStatusAccount {
    /// Whether this account makes `host_key` (already trimmed and lowercased) a
    /// host dux may name when it calls `gh`.
    fn qualifies(&self, host_key: &str) -> bool {
        // Every call dux makes names a host and never an account, so `gh` uses
        // that host's ACTIVE account: an account that is not the active one
        // tells us nothing about the call dux is going to make, and a working
        // sibling cannot vouch for a broken active account.
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
/// Returns `None` when the output is not that shape. That is NOT on its own the
/// older-`gh` signal any more: see [`decide_gh_probe`], where an unparseable
/// answer selects the fallback only when `gh`'s own diagnostics say it did not
/// understand the call.
///
/// Measured against gh 2.95.0, whose output is one entry per account:
/// `{"hosts":{"github.com":[{"state":"success","active":true,"host":"github.com",…}]}}`.
/// Neither the map keys nor the exit code can stand in for `state`: `gh` lists
/// every host it merely KNOWS, including one whose login has expired, and in
/// JSON mode it exits zero regardless.
///
/// Test-only: the decision needs the failures behind an empty set too, so it
/// calls [`parse_auth_status`]. This narrower view is what the qualification
/// rules are asserted through.
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
        active_errors: Vec::new(),
    };
    for (key, accounts) in &parsed.hosts {
        let key = key.trim().to_ascii_lowercase();
        if key.is_empty() {
            continue;
        }
        if accounts.iter().any(|account| account.qualifies(&key)) {
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
/// Measured on gh 2.95.0 against a live api.github.com with a token GitHub does
/// not know, in both the shapes `gh` produces:
///
/// ```text
/// GH_TOKEN:   non-200 OK status code: 401 Unauthorized body: "{ … "message": "Bad credentials" … }"
/// hosts.yml:  HTTP 401: Bad credentials (https://api.github.com/)
/// ```
///
/// This is the one account failure that is genuinely decisive: the token is
/// bad, and asking again in five minutes cannot make it good. Checked BEFORE
/// [`AUTH_STATUS_TRANSIENT_ERROR_MARKERS`] so a body that happens to quote a
/// retryable-looking word cannot rescue a rejected credential.
const AUTH_STATUS_CREDENTIAL_ERROR_MARKERS: &[&str] =
    &["bad credentials", "401 unauthorized", "http 401"];

/// Text in an account's `error` that means the call never got a verdict on the
/// login: GitHub rate-limited it, answered a server error, or the request did
/// not arrive at all.
///
/// The connection group is measured on gh 2.95.0 with every outbound connection
/// refused; `gh` surfaces Go's transport error verbatim, which is why the entries
/// read like Go's net stack rather than like `gh`:
///
/// ```text
/// Post "https://api.github.com/graphql": proxyconnect tcp: dial tcp 127.0.0.1:1: connect: connection refused
/// ```
///
/// The status-code group is NOT measured (a rate limit cannot be provoked on
/// demand): it is the measured `HTTP <code>: <message>` shape above carrying the
/// statuses GitHub documents for an exhausted quota (403 and 429) and the ones a
/// proxy or an outage produces (5xx). A record that matches neither table keeps
/// the decisive reading, so this list can only ever RESCUE an answer from being
/// latched, never invent a failure.
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
/// There is exactly one entry, and that is a measurement rather than a
/// shortcut. On gh 2.95.0 a refused token and a dead network produce BYTE FOR
/// BYTE the same diagnostic:
///
/// ```text
/// github.com
///   X Failed to log in to github.com using token (GH_TOKEN)
///   - Active account: true
///   - The token in GH_TOKEN is invalid.
/// ```
///
/// That was measured twice for each of the two credential sources (an env token
/// and a `hosts.yml` login), once against a live api.github.com with a token
/// GitHub rejects and once with every outbound connection refused, and the four
/// runs differed only in naming the source. `gh` never mentions the network, a
/// status code, or a rate limit here, which is why the previous table of
/// HTTP-and-Go phrases matched none of the output this path actually sees.
///
/// Faced with a shape that cannot distinguish them, dux retries: a genuinely bad
/// token keeps producing this same answer, and the sentence the user is shown
/// carries `gh`'s own line, so nothing is hidden by the choice. Deciding the
/// other way is what stranded a working login behind a momentary outage for the
/// rest of the run.
///
/// Everything here is measured on gh 2.95.0. This path only ever RUNS on a `gh`
/// too old to understand `--json`, whose wording could not be measured; an
/// answer this table does not recognise keeps the old decisive reading, so an
/// older `gh` is no worse off than before.
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
            }
        }
    }
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
pub fn gh_reprobe_is_due(
    status: crate::model::GhStatus,
    integration_enabled: bool,
    since_last_probe: Option<Duration>,
    interval: Duration,
) -> bool {
    if !integration_enabled || matches!(status, crate::model::GhStatus::Available) {
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
pub fn probe_github_hosts(program: &OsStr) -> GhProbe {
    // Kept deliberately: without it, "a failure to launch preserves the last
    // known good value" would quietly cover `gh` having been uninstalled, and
    // dux would go on believing in hosts it can no longer reach.
    let on_path = std::process::Command::new("which")
        .arg(program)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !on_path {
        return GhProbe::NotInstalled;
    }
    probe_github_hosts_with(program)
}

/// [`probe_github_hosts`] without the PATH check, against a named program, so
/// the two-mode protocol can be tested against a stand-in `gh`.
pub(crate) fn probe_github_hosts_with(program: &OsStr) -> GhProbe {
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
    }
}

/// Build and run one batched GraphQL query for a chunk of same-host sessions,
/// returning `(session_id, Option<PrInfo>)` for each, the rate-limit snapshot,
/// and whether the whole call hard-failed (spawn error / timeout / unparseable
/// stdout — as opposed to a per-alias error, which is handled inline).
fn run_chunk(host: &str, planned: &[Planned], chunk: &[usize]) -> ChunkOutcome {
    let (query, pos_repo) = build_chunk_query(planned, chunk);

    // On a spawn failure / timeout we still finalize with no data (each session
    // falls back to its stored result) rather than dropping the whole cycle.
    let qarg = format!("query={query}");
    let (data_json, stderr): (Option<serde_json::Value>, String) = match run_gh_with_timeout(
        &["api", "graphql", "--hostname", host, "-f", &qarg],
        GH_CALL_TIMEOUT,
    ) {
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
        GhCallOutcome::Failed(msg) => (None, format!("failed to run gh: {msg}")),
    };
    // Collapse both an absent `data` key and an explicit `data: null` (a real
    // GraphQL execution-failure shape) to "no data", so both drive the
    // hard-failure path and the preserve-last-known-PR fallback below.
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
            .map(|e| e.to_string())
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
            "[gh-integration] gh api graphql failed for host {host} ({} session(s), rate_limited={rate_limited}): {stderr}{}",
            chunk.len(),
            if errors.is_empty() {
                String::new()
            } else {
                format!(" | errors: {errors}")
            },
        ));
    }
    let (out, rate) = parse_chunk_response(planned, chunk, &pos_repo, data);
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

/// Build the batched GraphQL query for a chunk plus the per-position repo-alias
/// index (`pos_repo[pos] == k` means session `s{pos}` lives under `r{k}`), so the
/// response can be reparsed. Pure — no I/O — so it is unit-testable.
fn build_chunk_query(planned: &[Planned], chunk: &[usize]) -> (String, Vec<usize>) {
    // Group the chunk's sessions by repo → one aliased `repository(...)` block
    // each. `pos` is the session's index within the chunk (its `s{pos}` alias).
    let mut groups: std::collections::BTreeMap<(String, String), Vec<usize>> = Default::default();
    for (pos, &i) in chunk.iter().enumerate() {
        groups
            .entry((planned[i].owner.clone(), planned[i].repo.clone()))
            .or_default()
            .push(pos);
    }
    let mut pos_repo: Vec<usize> = vec![0; chunk.len()];
    for (k, (_, positions)) in groups.iter().enumerate() {
        for &pos in positions {
            pos_repo[pos] = k;
        }
    }

    let mut q = String::from("{\n  rateLimit { cost remaining resetAt }\n");
    for (k, ((owner, repo), positions)) in groups.iter().enumerate() {
        q.push_str(&format!(
            "  {}: repository(owner: {}, name: {}) {{\n",
            repo_alias(k),
            graphql_string(owner),
            graphql_string(repo),
        ));
        for &pos in positions {
            let p = &planned[chunk[pos]];
            if p.emit_ref {
                // Discovery asks for a WINDOW of the branch's pull requests and
                // picks among them in Rust (`select_discovered_pr`), rather than
                // for one node in a chosen order.
                //
                // Measured against two real repositories: GitHub IGNORES
                // `orderBy` on `associatedPullRequests`. The connection answers
                // oldest-first by creation date whatever is passed, so `first: 1`
                // with any orderBy returned the OLDEST pull request on the
                // branch, and a branch name reused after a merge reported the
                // old pull request forever. `last` walks from the newest end,
                // which is where the interesting nodes are.
                //
                // Each node names its own repository, because this connection is
                // not confined to the repository the alias was asked under: a
                // fork's pull request whose head is this ref appears here too
                // (measured on golang/go). Those are somebody else's pull
                // requests and `parse_chunk_response` drops them.
                let qname = graphql_string(&format!("refs/heads/{}", p.branch));
                q.push_str(&format!(
                    "    {}: ref(qualifiedName: {qname}) {{ associatedPullRequests(last: {DISCOVERY_WINDOW}) {{ nodes {{ number state title url repository {{ nameWithOwner }} }} }} }}\n",
                    ref_alias(pos),
                ));
            }
            if p.emit_num
                && let Some(known) = &p.known
            {
                q.push_str(&format!(
                    "    {}: pullRequest(number: {}) {{ number state title url }}\n",
                    num_alias(pos),
                    known.pr_number,
                ));
            }
        }
        q.push_str("  }\n");
    }
    q.push_str("}\n");
    (q, pos_repo)
}

/// Map a GraphQL `data` object (or `None` when the call failed) back to each
/// chunk session's `(session_id, Option<PrInfo>)`, applying the per-session merge
/// rule. A `null` repo/node (deleted repo or branch) resolves independently to
/// that session's fallback, so one bad alias never poisons the batch. Pure — so
/// it is unit-testable with a synthetic response.
fn parse_chunk_response(
    planned: &[Planned],
    chunk: &[usize],
    pos_repo: &[usize],
    data: Option<&serde_json::Value>,
) -> (Vec<(String, Option<PrInfo>)>, Option<RateLimitInfo>) {
    let rate = data.and_then(parse_rate_limit);
    let mut out = Vec::with_capacity(chunk.len());
    for (pos, &i) in chunk.iter().enumerate() {
        let p = &planned[i];
        // Whole-call failure (`data` absent, not a per-alias null): preserve each
        // session's last-known PR instead of reporting `None`, which the sidebar
        // would render as "PR gone" and wipe a still-open badge. Mirrors the
        // terminal-state fallback; the next successful cycle re-confirms it.
        if data.is_none() {
            out.push((
                p.session_id.clone(),
                p.known.as_ref().and_then(reconstruct_from_stored),
            ));
            continue;
        }
        let owner_repo = format!("{}/{}", p.owner, p.repo);
        let repo_obj = data
            .and_then(|d| d.get(repo_alias(pos_repo[pos]).as_str()))
            .filter(|v| !v.is_null());
        // Every node in the discovery window is parsed, the ones belonging to
        // another repository are dropped, and which of the rest this session's
        // badge should follow is decided by `select_discovered_pr`. A node dux
        // cannot parse (an unrecognised state, a missing number) drops on its
        // own and leaves its siblings alone.
        let ref_pr = repo_obj
            .and_then(|r| r.get(ref_alias(pos).as_str()))
            .and_then(|rf| rf.get("associatedPullRequests"))
            .and_then(|a| a.get("nodes"))
            .and_then(|n| n.as_array())
            .and_then(|arr| {
                select_discovered_pr(
                    arr.iter()
                        .filter(|node| node_belongs_to(node, &owner_repo))
                        .filter_map(|node| parse_pr_json_value(node, &p.host, &owner_repo))
                        .collect(),
                )
            });
        let num_pr = if p.emit_num {
            repo_obj
                .and_then(|r| r.get(num_alias(pos).as_str()))
                .filter(|v| !v.is_null())
                .and_then(|node| parse_pr_json_value(node, &p.host, &owner_repo))
        } else {
            None
        };
        out.push((p.session_id.clone(), merge_pr_result(p, ref_pr, num_pr)));
    }
    (out, rate)
}

/// Log one sync cycle at debug: what it considered, what it spent, and how much
/// of the answer differs from what it was told. A successful cycle used to say
/// nothing at all, so a wrong badge left no trace of where it came from.
///
/// The per-session INFO line is deliberately not raised here. This function sees
/// what the workers found, which is not the same as what the user's badge ends
/// up saying: a result can still be dropped for a deleted, detached or pinned
/// session. It is raised where the result is applied instead.
fn log_sync_cycle(
    entries: &[PrSyncEntry],
    asked: usize,
    points: i64,
    results: &[(String, Option<PrInfo>)],
    trigger: SyncTrigger,
) {
    let answered = results.iter().filter(|(_, pr)| pr.is_some()).count();
    let changed = results
        .iter()
        .filter(|(session_id, pr)| {
            entries
                .iter()
                .find(|e| &e.session_id == session_id)
                .is_some_and(|entry| {
                    let stored = entry.known_pr.as_ref().and_then(reconstruct_from_stored);
                    pr_badge_changed(stored.as_ref(), pr.as_ref())
                })
        })
        .count();
    logger::debug(&format_sync_cycle_summary(
        trigger,
        entries.len(),
        asked,
        answered,
        changed,
        points,
    ));
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
    match state {
        PrState::Open => "open",
        PrState::Merged => "merged",
        PrState::Closed => "closed",
    }
}

/// The per-cycle debug summary. `sessions` is everything the cycle considered,
/// `asked` the subset that needed a GitHub call at all (the rest were answered
/// from SQLite or skipped by a host backoff), `answered` how many ended the
/// cycle with a pull request, `changed` how many differ from the badge SQLite
/// could reconstruct, and `points` what GitHub billed for the whole cycle.
fn format_sync_cycle_summary(
    trigger: SyncTrigger,
    sessions: usize,
    asked: usize,
    answered: usize,
    changed: usize,
    points: i64,
) -> String {
    let word = match trigger {
        SyncTrigger::BlindPoll => "blind poll",
        SyncTrigger::Focus => "focus",
        SyncTrigger::OneShot => "one-shot",
    };
    format!(
        "PR sync ({word}): {sessions} sessions, {asked} asked, {answered} answered, \
{changed} changed, {points} GraphQL points"
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
    pr.map(|p| format!("#{} {}", p.number, pr_state_word(&p.state)))
        .unwrap_or_else(|| "none".to_string())
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

/// Reconcile the head-ref discovery result and the by-number refresh into the
/// single PR to report, matching the pre-batch behavior:
///   - terminal + running: a strictly-newer follow-up PR wins, then a same-numbered
///     discovery result that says a CLOSED PR is no longer closed, else the stored PR
///   - open known: the newest PR by number wins (a newer PR opened on the same
///     branch), else the by-number refresh (robust when the branch was deleted)
///   - undiscovered: whatever the head-ref discovery found
fn merge_pr_result(p: &Planned, ref_pr: Option<PrInfo>, num_pr: Option<PrInfo>) -> Option<PrInfo> {
    // A PINNED plan reports the pin and nothing else: the by-number refresh
    // when it answered (a CLOSED pin can reopen, an OPEN one can merge), else
    // the stored reconstruction. No discovery result exists to consider
    // (`emit_ref` is false), and a per-alias failure keeps the badge.
    if p.pinned {
        return num_pr.or_else(|| p.known.as_ref().and_then(reconstruct_from_stored));
    }
    let Some(known) = &p.known else {
        return ref_pr;
    };
    if p.is_terminal {
        if let Some(r) = &ref_pr
            && r.number > known.pr_number
        {
            return ref_pr;
        }
        // A CLOSED pull request can be reopened, and discovery already told us:
        // the same number came back in a state that is no longer closed. Accept
        // it, at zero extra API cost, so the badge follows the reopen.
        //
        // This is deliberately not extended to MERGED. A merge cannot practically
        // un-happen, and a stale read replica answering OPEN for a merged pull
        // request must never flip a merged badge back.
        //
        // The comparison is by number alone, in deliberate parity with the
        // strictly-higher-number rule above: discovery answers for the one
        // target repo the planner chose for this session, and the by-number
        // alias is separately guarded by known_matches_target.
        if known.state == "CLOSED"
            && let Some(r) = &ref_pr
            && r.number == known.pr_number
            && r.state != PrState::Closed
        {
            return ref_pr;
        }
        return reconstruct_from_stored(known);
    }
    match (num_pr, ref_pr) {
        (Some(rf), Some(nw)) => {
            if nw.number > rf.number {
                Some(nw)
            } else {
                Some(rf)
            }
        }
        (Some(rf), None) => Some(rf),
        (None, Some(nw)) => Some(nw),
        // Both lookups came back empty for a KNOWN-open PR — this is a per-alias
        // fetch failure (a null repo alias / transient GraphQL error), NOT a real
        // close (GitHub returns the node with state CLOSED/MERGED, never null, for
        // a real terminal PR). Preserve the last-known PR instead of wiping the
        // badge, mirroring the whole-call-failure and terminal fallbacks.
        (None, None) => reconstruct_from_stored(known),
    }
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
        cost: rl.get("cost").and_then(|v| v.as_i64()),
    })
}

/// Reconstruct a PrInfo from stored data without a network call.
/// Used wherever a call is skipped or failed and the stored row is the answer.
/// Reconstruct a live [`PrInfo`] from a stored PR row's decoded state, or `None`
/// when the stored state string is unrecognized. Shared by the PR-sync
/// reconstruction paths and by `Engine::seed_pr_statuses_from_store` (startup PR
/// badge seeding), so the "OPEN"/"MERGED"/"CLOSED" decode lives in exactly one
/// place instead of being re-implemented per surface.
pub fn reconstruct_pr_from_stored(stored: &StoredPr) -> Option<PrInfo> {
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
        title: stored.title.clone(),
        host: stored.host.clone(),
        owner_repo: stored.owner_repo.clone(),
        url: stored.url.clone(),
    })
}

/// Parse a single PR JSON object. Test-only helper.
#[cfg(test)]
fn parse_pr_json_object(json: &str, host: &str, owner_repo: &str) -> Option<PrInfo> {
    let obj: serde_json::Value = serde_json::from_str(json).ok()?;
    parse_pr_json_value(&obj, host, owner_repo)
}

/// Extract PrInfo from a serde_json::Value.
fn parse_pr_json_value(obj: &serde_json::Value, host: &str, owner_repo: &str) -> Option<PrInfo> {
    let number = obj.get("number")?.as_u64()?;
    let state_str = obj.get("state")?.as_str()?;
    let title = obj
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let url = obj
        .get("url")
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| pull_request_url(host, owner_repo, number));
    let state = match state_str {
        "OPEN" => PrState::Open,
        "MERGED" => PrState::Merged,
        "CLOSED" => PrState::Closed,
        _ => return None,
    };

    Some(PrInfo {
        number,
        state,
        title,
        host: normalize_github_host(host).to_string(),
        owner_repo: owner_repo.to_string(),
        url,
    })
}

pub fn pull_request_url(host: &str, owner_repo: &str, number: u64) -> String {
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
pub fn gh_repo_arg(host: &str, owner_repo: &str) -> String {
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
        "number,title,state,headRefName".to_string(),
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
pub fn parse_pull_request_lookup(
    raw_input: &str,
    selected_host: &str,
    selected_owner_repo: &str,
    policy: &GithubHostPolicy,
) -> Result<PullRequestLookup, String> {
    let reference = crate::pr_reference::parse_typed_reference(raw_input)?;

    if let Some(host) = reference.host.as_deref()
        && !policy.allows(host)
    {
        return Err(format!(
            "dux cannot look up pull requests on {host}. Sign in to that host with \
             `gh auth login --hostname {host}`, or paste a reference from a host you \
             are already signed in to."
        ));
    }

    if reference.owner_repo.is_some() && !reference.matches(selected_host, selected_owner_repo) {
        let named = reference.repository_label().unwrap_or_default();
        return Err(format!(
            "PR belongs to {named}, but the selected project uses \
             {selected_host}/{selected_owner_repo}."
        ));
    }

    let Some(number) = reference.number else {
        let named = reference
            .repository_label()
            .unwrap_or_else(|| selected_owner_repo.to_string());
        return Err(format!(
            "That address names {named} but no pull request. Add the number, for example \
             {selected_owner_repo}#123."
        ));
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
pub fn run_pull_request_lookup_job(
    project: Project,
    raw_input: String,
    custom_name: Option<String>,
    worker_tx: Sender<WorkerEvent>,
    status_op_id: Option<String>,
    policy: GithubHostPolicy,
) {
    let lookup = match git::remote_github_repo(Path::new(&project.path), &policy) {
        Some(remote) => {
            parse_pull_request_lookup(&raw_input, &remote.host, &remote.owner_repo, &policy)
        }
        None => Err(format!(
            "Project \"{}\" does not have a GitHub origin remote.",
            project.name
        )),
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

    let args = pr_view_args(&lookup.host, &lookup.owner_repo, lookup.number);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    // Bounded so a hung `gh pr view` (stalled network, credential prompt) can't
    // strand the web CreateAgentFromPr Busy status forever.
    let result = match run_gh_with_timeout(&arg_refs, GH_CALL_TIMEOUT) {
        GhCallOutcome::Completed(output) if output.status.success() => {
            parse_resolved_pull_request_json(
                &String::from_utf8_lossy(&output.stdout),
                project,
                &lookup.host,
                &lookup.owner_repo,
                custom_name,
            )
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
        GhCallOutcome::Failed(msg) => Err(format!("Failed to run gh pr view: {msg}")),
    };
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
pub fn parse_attach_pull_request_lookup(
    raw_input: &str,
    resolution: &git::RemoteResolution,
    project_name: &str,
    policy: &GithubHostPolicy,
) -> Result<PullRequestLookup, String> {
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
            return Err(format!(
                "dux cannot look up pull requests on {host}. Sign in to that host with \
                 `gh auth login --hostname {host}`, or paste a reference from a host you \
                 are already signed in to."
            ));
        }
        let Some(number) = reference.number else {
            return Err(format!(
                "That address names {owner_repo} but no pull request. Add the number, \
                 for example {owner_repo}#123."
            ));
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
                .to_string(),
        );
    };
    match resolution {
        git::RemoteResolution::Allowed(remote) => Ok(PullRequestLookup {
            host: remote.host.clone(),
            owner_repo: remote.owner_repo.clone(),
            number,
        }),
        git::RemoteResolution::Denied => Err(format!(
            "Project \"{project_name}\"'s remote is on a host dux is not signed in to, so a \
             bare PR number cannot be resolved. Sign in with `gh auth login --hostname \
             <host>`, or paste the full PR URL instead."
        )),
        git::RemoteResolution::Unresolved => Err(format!(
            "Project \"{project_name}\" does not have a GitHub origin remote, so a bare PR \
             number cannot be resolved. Paste the full PR URL (or owner/repo#{number}) \
             instead."
        )),
    }
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
pub fn run_attach_pull_request_lookup_job(
    project: Project,
    session_id: String,
    raw_input: String,
    worker_tx: Sender<WorkerEvent>,
    status_op_id: Option<String>,
    policy: GithubHostPolicy,
) {
    let resolution = git::resolve_remote_github_repo(Path::new(&project.path), &policy);
    let purpose = PrLookupPurpose::Attach { session_id };
    let lookup =
        match parse_attach_pull_request_lookup(&raw_input, &resolution, &project.name, &policy) {
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

    let args = pr_view_args(&lookup.host, &lookup.owner_repo, lookup.number);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    // Bounded so a hung `gh pr view` cannot strand the attach busy forever.
    let result = match run_gh_with_timeout(&arg_refs, GH_CALL_TIMEOUT) {
        GhCallOutcome::Completed(output) if output.status.success() => {
            parse_resolved_pull_request_json(
                &String::from_utf8_lossy(&output.stdout),
                project,
                &lookup.host,
                &lookup.owner_repo,
                None,
            )
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
        GhCallOutcome::Failed(msg) => Err(format!("Failed to run gh pr view: {msg}")),
    };
    let _ = worker_tx.send(WorkerEvent::PullRequestResolved {
        result,
        purpose,
        status_op_id,
    });
}

fn parse_resolved_pull_request_json(
    json: &str,
    project: Project,
    host: &str,
    owner_repo: &str,
    custom_name: Option<String>,
) -> Result<ResolvedPullRequest, String> {
    let obj: serde_json::Value = serde_json::from_str(json.trim())
        .map_err(|err| format!("gh returned invalid PR JSON: {err}"))?;
    let number = obj
        .get("number")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "gh PR response did not include a PR number.".to_string())?;
    let title = obj
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
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
    Ok(ResolvedPullRequest {
        project,
        host: host.to_string(),
        owner_repo: owner_repo.to_string(),
        number,
        title,
        state,
        head_ref_name,
        custom_name,
    })
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
        let GhProbe::Decided { available, policy } = probe else {
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

        let probe = probe_github_hosts_with(gh.as_os_str());
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
            probe_github_hosts_with(gh.as_os_str()),
            GhProbe::Decided {
                available: false,
                policy: GithubHostPolicy::Hosts(BTreeSet::new()),
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

        let probe = probe_github_hosts_with(gh.as_os_str());
        assert_eq!(
            probe,
            GhProbe::Decided {
                available: true,
                policy: GithubHostPolicy::LegacyNameRule,
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
                    gh_reprobe_is_due(status, true, Some(Duration::from_secs(300)), INTERVAL),
                    "{status:?} is not a working gh, so it is re-checked",
                );
                assert!(
                    !gh_reprobe_is_due(status, true, Some(Duration::from_secs(299)), INTERVAL),
                    "{status:?} waits out the interval",
                );
            }
        }

        #[test]
        fn a_working_gh_stops_the_timer_and_a_later_failure_restarts_it() {
            assert!(
                !gh_reprobe_is_due(
                    GhStatus::Available,
                    true,
                    Some(Duration::from_secs(9_000)),
                    INTERVAL,
                ),
                "nothing is waiting on an answer, so dux stops spawning gh",
            );
            // The rule reads the status live rather than latching, so the same
            // clock with a failed status is due again immediately.
            assert!(gh_reprobe_is_due(
                GhStatus::Unreachable,
                true,
                Some(Duration::from_secs(9_000)),
                INTERVAL,
            ));
        }

        #[test]
        fn the_integration_being_off_asks_nothing() {
            assert!(!gh_reprobe_is_due(
                GhStatus::Unreachable,
                false,
                Some(Duration::from_secs(9_000)),
                INTERVAL,
            ));
        }

        #[test]
        fn a_zero_interval_disables_the_timer() {
            assert!(!gh_reprobe_is_due(
                GhStatus::Unreachable,
                true,
                Some(Duration::from_secs(9_000)),
                Duration::ZERO,
            ));
        }

        #[test]
        fn a_probe_that_has_never_run_is_not_due_here() {
            // The surfaces run the startup probe themselves; answering "due"
            // before it lands would spawn a second one alongside it.
            assert!(!gh_reprobe_is_due(GhStatus::Unknown, true, None, INTERVAL));
        }
    }

    #[test]
    fn a_missing_gh_is_reported_as_not_installed() {
        // `probe_github_hosts` (unlike the `_with` variant) is the one that
        // consults PATH, and a name nothing on PATH resolves is the case.
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

    fn stored(number: u64, state: &str) -> StoredPr {
        StoredPr {
            session_id: "s".to_string(),
            pr_number: number,
            host: "github.com".to_string(),
            owner_repo: "octocat/Hello-World".to_string(),
            state: state.to_string(),
            title: "Stored".to_string(),
            url: format!("https://github.com/octocat/Hello-World/pull/{number}"),
        }
    }

    fn planned(
        session_id: &str,
        owner: &str,
        repo: &str,
        branch: &str,
        known: Option<StoredPr>,
    ) -> Planned {
        // Go through the real constructor so the is_terminal/emit_num rule is
        // never duplicated between production and tests.
        Planned::new(
            session_id.to_string(),
            "github.com".to_string(),
            owner.to_string(),
            repo.to_string(),
            branch.to_string(),
            known,
        )
    }

    /// A ref-discovery node wrapped as the GraphQL shape `s{pos}_ref` resolves to.
    fn ref_node(number: u64, state: &str) -> serde_json::Value {
        serde_json::json!({
            "associatedPullRequests": {
                "nodes": [ pr_node(number, state) ]
            }
        })
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
    fn build_chunk_query_discovers_undiscovered_session() {
        let ps = vec![planned("s0", "octocat", "Hello-World", "feat/x", None)];
        let (q, pos_repo) = build_chunk_query(&ps, &[0]);
        assert_eq!(pos_repo, vec![0]);
        assert!(q.contains("rateLimit { cost remaining resetAt }"));
        assert!(q.contains("r0: repository(owner: \"octocat\", name: \"Hello-World\")"));
        // Branch with a slash must be JSON-escaped into a valid GraphQL string.
        assert!(q.contains("s0_ref: ref(qualifiedName: \"refs/heads/feat/x\")"));
        // A window taken from the NEWEST end, and no orderBy: GitHub ignores
        // orderBy on this connection (measured), so asking for one ordered node
        // returned the oldest pull request on the branch.
        assert!(q.contains("associatedPullRequests(last: 20)"));
        // Every node names its own repository, so a fork's pull request on this
        // very ref can be told apart from one of this repository's own.
        assert!(q.contains("nodes { number state title url repository { nameWithOwner } }"));
        assert!(!q.contains("orderBy"));
        assert!(!q.contains("CREATED_AT"));
        // No known PR → no by-number alias.
        assert!(!q.contains("s0_num"));
    }

    #[test]
    fn build_chunk_query_open_known_emits_both_aliases() {
        let ps = vec![planned(
            "s0",
            "octocat",
            "Hello-World",
            "feat/x",
            Some(stored(42, "OPEN")),
        )];
        let (q, _) = build_chunk_query(&ps, &[0]);
        assert!(q.contains("s0_ref: ref(qualifiedName:"));
        assert!(q.contains("s0_num: pullRequest(number: 42)"));
    }

    #[test]
    fn build_chunk_query_groups_by_repo() {
        let ps = vec![
            planned("s0", "octocat", "repo-a", "feat/a", None),
            planned("s1", "octocat", "repo-b", "feat/b", None),
        ];
        let (q, pos_repo) = build_chunk_query(&ps, &[0, 1]);
        // Two distinct repos → two aliased repository blocks.
        assert!(q.contains("repository(owner: \"octocat\", name: \"repo-a\")"));
        assert!(q.contains("repository(owner: \"octocat\", name: \"repo-b\")"));
        assert_eq!(pos_repo.len(), 2);
        assert_ne!(pos_repo[0], pos_repo[1]);
    }

    #[test]
    fn build_and_parse_two_sessions_on_same_repo() {
        // The core batching case: two agents on the SAME repo share one
        // repository(...) block, each with its own aliased sub-fields, and
        // parse_chunk_response must demultiplex both back to the right session.
        let ps = vec![
            planned("s0", "octocat", "Hello-World", "feat/a", None),
            planned("s1", "octocat", "Hello-World", "feat/b", None),
        ];
        let chunk = [0usize, 1usize];
        let (q, pos_repo) = build_chunk_query(&ps, &chunk);
        // One shared repo block; both sessions' ref aliases live under it.
        assert_eq!(q.matches("repository(owner:").count(), 1);
        assert_eq!(pos_repo, vec![0, 0]);
        assert!(q.contains("s0_ref: ref(qualifiedName: \"refs/heads/feat/a\")"));
        assert!(q.contains("s1_ref: ref(qualifiedName: \"refs/heads/feat/b\")"));

        let data = serde_json::json!({
            "r0": { "s0_ref": ref_node(10, "OPEN"), "s1_ref": ref_node(20, "OPEN") },
        });
        let (results, _) = parse_chunk_response(&ps, &chunk, &pos_repo, Some(&data));
        let by_id: std::collections::HashMap<_, _> = results.into_iter().collect();
        assert_eq!(by_id[&"s0".to_string()].as_ref().unwrap().number, 10);
        assert_eq!(by_id[&"s1".to_string()].as_ref().unwrap().number, 20);
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

    #[test]
    fn parse_chunk_response_preserves_known_pr_on_whole_call_failure() {
        // data=None means the whole gh call failed; an OPEN known PR must NOT be
        // wiped to None (which the UI reads as "PR gone") — keep last-known state.
        let ps = vec![planned(
            "s0",
            "octocat",
            "Hello-World",
            "feat/x",
            Some(stored(42, "OPEN")),
        )];
        let chunk = [0usize];
        let (_, pos_repo) = build_chunk_query(&ps, &chunk);
        let (results, rate) = parse_chunk_response(&ps, &chunk, &pos_repo, None);
        let pr = results[0].1.as_ref().expect("kept last-known PR");
        assert_eq!(pr.number, 42);
        assert_eq!(pr.state, PrState::Open);
        assert!(rate.is_none());
    }

    #[test]
    fn parse_chunk_response_discovers_pr_and_rate_limit() {
        let ps = vec![planned("s0", "octocat", "Hello-World", "feat/x", None)];
        let chunk = [0usize];
        let (_, pos_repo) = build_chunk_query(&ps, &chunk);
        let data = serde_json::json!({
            "rateLimit": { "remaining": 42, "resetAt": "2030-01-01T00:00:00Z" },
            "r0": { "s0_ref": ref_node(7, "OPEN") },
        });
        let (results, rate) = parse_chunk_response(&ps, &chunk, &pos_repo, Some(&data));
        assert_eq!(results.len(), 1);
        let pr = results[0].1.as_ref().expect("discovered pr");
        assert_eq!(pr.number, 7);
        assert_eq!(pr.state, PrState::Open);
        assert_eq!(rate.expect("rate").remaining, 42);
    }

    #[test]
    fn parse_chunk_response_open_prefers_newer_ref_pr() {
        let ps = vec![planned(
            "s0",
            "octocat",
            "Hello-World",
            "feat/x",
            Some(stored(42, "OPEN")),
        )];
        let chunk = [0usize];
        let (_, pos_repo) = build_chunk_query(&ps, &chunk);
        // A newer PR (#43) exists on the branch; the by-number refresh still shows #42.
        let data = serde_json::json!({
            "r0": { "s0_ref": ref_node(43, "OPEN"), "s0_num": pr_node(42, "OPEN") },
        });
        let (results, _) = parse_chunk_response(&ps, &chunk, &pos_repo, Some(&data));
        assert_eq!(results[0].1.as_ref().unwrap().number, 43);
    }

    #[test]
    fn parse_chunk_response_open_survives_branch_deletion_on_merge() {
        let ps = vec![planned(
            "s0",
            "octocat",
            "Hello-World",
            "feat/x",
            Some(stored(42, "OPEN")),
        )];
        let chunk = [0usize];
        let (_, pos_repo) = build_chunk_query(&ps, &chunk);
        // Branch deleted on squash-merge → ref is null, but by-number still resolves
        // the now-MERGED PR (the case the pre-batch `gh pr view` handled).
        let data = serde_json::json!({
            "r0": { "s0_ref": serde_json::Value::Null, "s0_num": pr_node(42, "MERGED") },
        });
        let (results, _) = parse_chunk_response(&ps, &chunk, &pos_repo, Some(&data));
        let pr = results[0].1.as_ref().unwrap();
        assert_eq!(pr.number, 42);
        assert_eq!(pr.state, PrState::Merged);
    }

    #[test]
    fn parse_chunk_response_merged_running_keeps_stored_unless_newer() {
        let ps = vec![planned(
            "s0",
            "octocat",
            "Hello-World",
            "feat/x",
            Some(stored(42, "MERGED")),
        )];
        let chunk = [0usize];
        let (_, pos_repo) = build_chunk_query(&ps, &chunk);
        // Same PR still on the branch → keep the stored terminal PR, not a re-fetch.
        let same = serde_json::json!({ "r0": { "s0_ref": ref_node(42, "MERGED") } });
        let (results, _) = parse_chunk_response(&ps, &chunk, &pos_repo, Some(&same));
        assert_eq!(results[0].1.as_ref().unwrap().number, 42);
        // A strictly-newer follow-up PR (#50) replaces it.
        let newer = serde_json::json!({ "r0": { "s0_ref": ref_node(50, "OPEN") } });
        let (results, _) = parse_chunk_response(&ps, &chunk, &pos_repo, Some(&newer));
        let pr = results[0].1.as_ref().unwrap();
        assert_eq!(pr.number, 50);
        assert_eq!(pr.state, PrState::Open);
        // A merge is final: a same-numbered answer claiming OPEN (a stale read
        // replica) must NOT flip the merged badge back. This is the one place
        // the closed-reopen rule deliberately does not reach.
        let stale = serde_json::json!({ "r0": { "s0_ref": ref_node(42, "OPEN") } });
        let (results, _) = parse_chunk_response(&ps, &chunk, &pos_repo, Some(&stale));
        let pr = results[0].1.as_ref().unwrap();
        assert_eq!(pr.number, 42);
        assert_eq!(pr.state, PrState::Merged);
    }

    /// The discovery alias asks for a WINDOW of nodes, in the ascending order
    /// GitHub actually returns them in. This wraps a list of them.
    fn ref_nodes(nodes: &[(u64, &str)]) -> serde_json::Value {
        serde_json::json!({
            "associatedPullRequests": {
                "nodes": nodes
                    .iter()
                    .map(|(n, st)| pr_node(*n, st))
                    .collect::<Vec<_>>(),
            }
        })
    }

    /// The same, for nodes that name their own repositories.
    fn ref_nodes_from(nodes: &[(u64, &str, &str)]) -> serde_json::Value {
        serde_json::json!({
            "associatedPullRequests": {
                "nodes": nodes
                    .iter()
                    .map(|(n, st, repo)| pr_node_from(*n, st, repo))
                    .collect::<Vec<_>>(),
            }
        })
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

    #[test]
    fn parse_chunk_response_follows_a_reused_branch_to_the_newest_pull_request() {
        // Measured against real repositories: the connection comes back
        // ascending by number, so the stored MERGED #7 sits FIRST and the live
        // #47 last. Reading element zero reported the merged one forever.
        let ps = vec![planned(
            "s0",
            "octocat",
            "Hello-World",
            "feat/x",
            Some(stored(7, "MERGED")),
        )];
        let chunk = [0usize];
        let (_, pos_repo) = build_chunk_query(&ps, &chunk);
        let data = serde_json::json!({
            "r0": { "s0_ref": ref_nodes(&[(7, "MERGED"), (47, "OPEN")]) },
        });
        let (results, _) = parse_chunk_response(&ps, &chunk, &pos_repo, Some(&data));
        let pr = results[0].1.as_ref().expect("a pull request");
        assert_eq!(pr.number, 47);
        assert_eq!(pr.state, PrState::Open);
    }

    #[test]
    fn parse_chunk_response_ignores_a_pull_request_from_another_repository() {
        // Measured on golang/go: a fork's pull request whose head is this very
        // ref is associated with it, so the window can hold pull requests dux
        // must not report. Left in, the newest-and-open-first rule would prefer
        // the foreign #99 and store it against this session's repository.
        let ps = vec![planned("s0", "octocat", "Hello-World", "feat/x", None)];
        let chunk = [0usize];
        let (_, pos_repo) = build_chunk_query(&ps, &chunk);
        let data = serde_json::json!({
            "r0": {
                "s0_ref": ref_nodes_from(&[
                    (12, "OPEN", "octocat/Hello-World"),
                    (99, "OPEN", "contributor/Hello-World"),
                ]),
            },
        });
        let (results, _) = parse_chunk_response(&ps, &chunk, &pos_repo, Some(&data));
        let pr = results[0].1.as_ref().expect("a pull request");
        assert_eq!(pr.number, 12);
        assert_eq!(pr.owner_repo, "octocat/Hello-World");
    }

    #[test]
    fn parse_chunk_response_drops_only_the_node_it_cannot_parse() {
        // An unrecognised state (a value this build has never heard of) drops
        // that node alone; its siblings still decide the badge.
        let ps = vec![planned("s0", "octocat", "Hello-World", "feat/x", None)];
        let chunk = [0usize];
        let (_, pos_repo) = build_chunk_query(&ps, &chunk);
        let data = serde_json::json!({
            "r0": { "s0_ref": ref_nodes(&[(12, "OPEN"), (99, "DRAFTED_SOMEHOW")]) },
        });
        let (results, _) = parse_chunk_response(&ps, &chunk, &pos_repo, Some(&data));
        let pr = results[0].1.as_ref().expect("a pull request");
        assert_eq!(pr.number, 12);
    }

    #[test]
    fn parse_chunk_response_merged_row_keeps_its_number_over_an_older_open_one() {
        // The select rule and the merge rule meeting: discovery prefers the OPEN
        // #7, and the merge rule then refuses it, because within one repository
        // a genuine successor to #47 always has a higher number. The stored
        // merged badge stands.
        let ps = vec![planned(
            "s0",
            "octocat",
            "Hello-World",
            "feat/x",
            Some(stored(47, "MERGED")),
        )];
        let chunk = [0usize];
        let (_, pos_repo) = build_chunk_query(&ps, &chunk);
        let data = serde_json::json!({
            "r0": { "s0_ref": ref_nodes(&[(7, "OPEN"), (47, "MERGED")]) },
        });
        let (results, _) = parse_chunk_response(&ps, &chunk, &pos_repo, Some(&data));
        let pr = results[0].1.as_ref().expect("a pull request");
        assert_eq!(pr.number, 47);
        assert_eq!(pr.state, PrState::Merged);
    }

    #[test]
    fn exited_entry_needs_no_network_re_queries_a_terminal_row_only_on_a_deliberate_trigger() {
        for state in ["MERGED", "CLOSED"] {
            let known = stored(7, state);
            assert!(
                exited_entry_needs_no_network(Some(&known), SyncTrigger::BlindPoll),
                "the blind poll stays free for {state}"
            );
            assert!(
                exited_entry_needs_no_network(Some(&known), SyncTrigger::Focus),
                "focusing a finished agent must not spawn a gh process for {state}"
            );
            assert!(
                !exited_entry_needs_no_network(Some(&known), SyncTrigger::OneShot),
                "boot, a refs change or an exit spends one discovery call on {state}, \
                 so a reused branch heals"
            );
        }
    }

    #[test]
    fn exited_entry_needs_no_network_never_skips_an_open_or_undiscovered_row() {
        for trigger in [
            SyncTrigger::BlindPoll,
            SyncTrigger::Focus,
            SyncTrigger::OneShot,
        ] {
            assert!(!exited_entry_needs_no_network(
                Some(&stored(7, "OPEN")),
                trigger
            ));
            assert!(!exited_entry_needs_no_network(None, trigger));
        }
    }

    #[test]
    fn pinned_exited_entry_needs_no_network_keeps_a_merged_pin_free() {
        // A pin emits no discovery alias, so there is nothing a merged pin's
        // call could ever find, under any trigger. A closed pin can reopen, so
        // the deliberate triggers ask.
        let merged = stored(7, "MERGED");
        let closed = stored(7, "CLOSED");
        for trigger in [
            SyncTrigger::BlindPoll,
            SyncTrigger::Focus,
            SyncTrigger::OneShot,
        ] {
            assert!(pinned_exited_entry_needs_no_network(&merged, trigger));
        }
        assert!(pinned_exited_entry_needs_no_network(
            &closed,
            SyncTrigger::BlindPoll
        ));
        assert!(pinned_exited_entry_needs_no_network(
            &closed,
            SyncTrigger::Focus
        ));
        assert!(!pinned_exited_entry_needs_no_network(
            &closed,
            SyncTrigger::OneShot
        ));
    }

    #[test]
    fn format_sync_cycle_summary_names_the_trigger_the_counts_and_the_cost() {
        assert_eq!(
            format_sync_cycle_summary(SyncTrigger::BlindPoll, 5, 3, 4, 1, 2),
            "PR sync (blind poll): 5 sessions, 3 asked, 4 answered, 1 changed, 2 GraphQL points"
        );
        assert_eq!(
            format_sync_cycle_summary(SyncTrigger::Focus, 1, 0, 1, 0, 0),
            "PR sync (focus): 1 sessions, 0 asked, 1 answered, 0 changed, 0 GraphQL points"
        );
        assert_eq!(
            format_sync_cycle_summary(SyncTrigger::OneShot, 1, 1, 0, 0, 1),
            "PR sync (one-shot): 1 sessions, 1 asked, 0 answered, 0 changed, 1 GraphQL points"
        );
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
    fn parse_chunk_response_closed_still_closed_keeps_stored() {
        // The counterpart to the reopen case: discovery confirming the pull
        // request is still closed changes nothing.
        let ps = vec![planned(
            "s0",
            "octocat",
            "Hello-World",
            "feat/x",
            Some(stored(12, "CLOSED")),
        )];
        let chunk = [0usize];
        let (_, pos_repo) = build_chunk_query(&ps, &chunk);
        let same = serde_json::json!({ "r0": { "s0_ref": ref_node(12, "CLOSED") } });
        let (results, _) = parse_chunk_response(&ps, &chunk, &pos_repo, Some(&same));
        let pr = results[0].1.as_ref().unwrap();
        assert_eq!(pr.number, 12);
        assert_eq!(pr.state, PrState::Closed);
    }

    #[test]
    fn parse_chunk_response_closed_ignores_a_different_lower_number() {
        // The reopen rule matches the SAME number only: discovery answering
        // with some other, lower-numbered pull request must not flip the
        // stored closed badge onto it.
        let ps = vec![planned(
            "s0",
            "octocat",
            "Hello-World",
            "feat/x",
            Some(stored(12, "CLOSED")),
        )];
        let chunk = [0usize];
        let (_, pos_repo) = build_chunk_query(&ps, &chunk);
        let other = serde_json::json!({ "r0": { "s0_ref": ref_node(7, "OPEN") } });
        let (results, _) = parse_chunk_response(&ps, &chunk, &pos_repo, Some(&other));
        let pr = results[0].1.as_ref().unwrap();
        assert_eq!(pr.number, 12);
        assert_eq!(pr.state, PrState::Closed);
    }

    /// A CLOSED pull request can be reopened, and discovery already carries the
    /// answer: the same number comes back with state OPEN. The stored CLOSED row
    /// must yield to it, at no extra API cost.
    #[test]
    fn parse_chunk_response_closed_accepts_the_same_number_reopened() {
        let ps = vec![planned(
            "s0",
            "octocat",
            "Hello-World",
            "feat/x",
            Some(stored(12, "CLOSED")),
        )];
        let chunk = [0usize];
        let (_, pos_repo) = build_chunk_query(&ps, &chunk);
        let reopened = serde_json::json!({ "r0": { "s0_ref": ref_node(12, "OPEN") } });
        let (results, _) = parse_chunk_response(&ps, &chunk, &pos_repo, Some(&reopened));
        let pr = results[0].1.as_ref().expect("reopened pr");
        assert_eq!(pr.number, 12);
        assert_eq!(pr.state, PrState::Open);
    }

    #[test]
    fn parse_chunk_response_one_bad_repo_does_not_poison_the_batch() {
        // s0's repo was deleted (r0: null + NOT_FOUND); s1 is fine in r1.
        let ps = vec![
            planned("s0", "octocat", "gone", "feat/a", None),
            planned("s1", "octocat", "repo-b", "feat/b", None),
        ];
        let chunk = [0usize, 1usize];
        let (_, pos_repo) = build_chunk_query(&ps, &chunk);
        let data = serde_json::json!({
            "r0": serde_json::Value::Null,
            // The node names s1's OWN repository, which is what makes it s1's
            // pull request rather than some fork's.
            "r1": { "s1_ref": ref_nodes_from(&[(9, "OPEN", "octocat/repo-b")]) },
        });
        let (results, _) = parse_chunk_response(&ps, &chunk, &pos_repo, Some(&data));
        let by_id: std::collections::HashMap<_, _> = results.into_iter().collect();
        assert!(by_id[&"s0".to_string()].is_none());
        assert_eq!(by_id[&"s1".to_string()].as_ref().unwrap().number, 9);
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
    fn run_entries_merged_exited_reconstructs_without_network_on_the_blind_poll() {
        // A merged + exited session is reconstructed from SQLite with no gh call
        // on the poll (the worktree path is bogus, so any git/gh access would
        // fail). A one-shot still spends one discovery call on it, because the
        // branch name may have been reused for a brand new pull request.
        let entry = PrSyncEntry {
            session_id: "s0".to_string(),
            branch_name: "feat/done".to_string(),
            worktree_path: "/nonexistent/dux-test-path".to_string(),
            known_pr: Some(stored(42, "MERGED")),
            agent_exited: true,
            pinned: None,
        };
        let trigger = SyncTrigger::BlindPoll;
        let (results, signals) = run_entries(
            std::slice::from_ref(&entry),
            &std::collections::HashMap::new(),
            &legacy_policy(),
            trigger,
        );
        // Zero-network on the poll → no host was queried, so no signal.
        assert!(signals.is_empty(), "no network call means no host signal");
        assert_eq!(results.len(), 1);
        let pr = results[0].1.as_ref().expect("reconstructed");
        assert_eq!(pr.number, 42);
        assert_eq!(pr.state, PrState::Merged);
    }

    #[test]
    fn run_entries_closed_exited_makes_no_call_on_the_blind_poll() {
        // A wall of dormant sessions with closed pull requests must not tick the
        // API every interval, so the blind poll still reconstructs from SQLite.
        let entry = PrSyncEntry {
            session_id: "s0".to_string(),
            branch_name: "feat/done".to_string(),
            worktree_path: "/nonexistent/dux-test-path".to_string(),
            known_pr: Some(stored(42, "CLOSED")),
            agent_exited: true,
            pinned: None,
        };
        let (results, signals) = run_entries(
            std::slice::from_ref(&entry),
            &std::collections::HashMap::new(),
            &legacy_policy(),
            SyncTrigger::BlindPoll,
        );
        assert!(signals.is_empty(), "no network call means no host signal");
        let pr = results[0].1.as_ref().expect("reconstructed");
        assert_eq!(pr.number, 42);
        assert_eq!(pr.state, PrState::Closed);
    }

    #[test]
    fn merge_pr_result_open_known_preserves_stored_on_double_null() {
        // A per-alias null (transient error) yields (None, None) for a KNOWN-open
        // PR: preserve the stored PR rather than wiping the badge.
        let p = planned(
            "s0",
            "octocat",
            "Hello-World",
            "feat/x",
            Some(stored(42, "OPEN")),
        );
        let merged = merge_pr_result(&p, None, None).expect("preserved");
        assert_eq!(merged.number, 42);
        assert_eq!(merged.state, PrState::Open);
    }

    #[test]
    fn parse_chunk_response_preserves_terminal_known_pr_on_whole_call_failure() {
        // The whole-call-failure fallback must also cover a CLOSED/terminal known
        // PR (agent still running, so it was queried), not just OPEN.
        let ps = vec![planned(
            "s0",
            "octocat",
            "Hello-World",
            "feat/x",
            Some(stored(42, "CLOSED")),
        )];
        let chunk = [0usize];
        let (_, pos_repo) = build_chunk_query(&ps, &chunk);
        let (results, _) = parse_chunk_response(&ps, &chunk, &pos_repo, None);
        let pr = results[0].1.as_ref().expect("kept last-known terminal PR");
        assert_eq!(pr.number, 42);
        assert_eq!(pr.state, PrState::Closed);
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
        // A binary that doesn't exist → Failed (spawn error), not TimedOut.
        let missing = run_command_with_timeout(
            std::process::Command::new("dux-nonexistent-binary-xyz"),
            Duration::from_secs(5),
        );
        assert!(matches!(missing, GhCallOutcome::Failed(_)));
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

        let argv: Vec<String> = std::fs::read_to_string(&recorded)
            .expect("stand-in recorded its argv")
            .lines()
            .map(str::to_string)
            .collect();
        let repo = argv
            .iter()
            .position(|a| a == "--repo")
            .and_then(|i| argv.get(i + 1))
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

    fn lookup_test_project() -> Project {
        Project {
            id: "p1".to_string(),
            name: "demo".to_string(),
            path: "/tmp/demo".to_string(),
            explicit_default_provider: None,
            default_provider: crate::model::ProviderKind::new("claude"),
            leading_branch: Some("main".to_string()),
            auto_reopen_agents: None,
            startup_command: None,
            env: std::collections::BTreeMap::new(),
            current_branch: "main".to_string(),
            branch_status: crate::model::ProjectBranchStatus::Leading,
            path_missing: false,
            created_at: None,
        }
    }

    #[test]
    fn parse_resolved_pull_request_json_extracts_fields() {
        let resolved = parse_resolved_pull_request_json(
            r#"{"number":42,"title":"Fix bug","state":"OPEN","headRefName":"feature/fix"}"#,
            lookup_test_project(),
            "github.com",
            "octocat/Hello-World",
            Some("my-name".to_string()),
        )
        .expect("resolved");
        assert_eq!(resolved.number, 42);
        assert_eq!(resolved.title, "Fix bug");
        assert_eq!(resolved.state, "OPEN");
        assert_eq!(resolved.head_ref_name, "feature/fix");
        assert_eq!(resolved.host, "github.com");
        assert_eq!(resolved.owner_repo, "octocat/Hello-World");
        assert_eq!(resolved.custom_name.as_deref(), Some("my-name"));
    }

    #[test]
    fn parse_resolved_pull_request_json_rejects_missing_head_branch() {
        let err = parse_resolved_pull_request_json(
            r#"{"number":42,"title":"Fix bug","state":"OPEN"}"#,
            lookup_test_project(),
            "github.com",
            "octocat/Hello-World",
            None,
        )
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
            let mut demonstration = std::process::Command::new("git");
            demonstration.args(get_url);
            crate::git::test_support::isolate_git_config(&mut demonstration);
            demonstration.envs(channel.iter().copied());
            let rewritten = demonstration.output().unwrap();
            assert_eq!(
                String::from_utf8_lossy(&rewritten.stdout).trim(),
                "ssh://git@evil.example/octocat/Hello-World.git",
                "{channel:?} must really rewrite, or the assertion below proves nothing",
            );

            let mut isolated = std::process::Command::new("git");
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

    /// An entry whose worktree path is irrelevant, because these tests supply
    /// the resolved remote themselves.
    fn planning_entry() -> PrSyncEntry {
        PrSyncEntry {
            session_id: "s0".to_string(),
            branch_name: "feat/x".to_string(),
            worktree_path: "/nonexistent/worktree".to_string(),
            known_pr: None,
            agent_exited: false,
            pinned: None,
        }
    }

    /// The silent bug lives in the `known_pr: None` branch: an agent that has
    /// never had a pull request recorded has no stored host to fall back on, so
    /// a remote the parser did not understand was written down as an empty
    /// result and the session was skipped, every cycle, with nothing saying
    /// why.
    ///
    /// This asserts the PLAN rather than the run, which is what makes it
    /// honest: it names the exact host, owner and repo the poller would query,
    /// and it cannot spawn `gh` under any outcome, passing or failing.
    ///
    /// The remote is INJECTED rather than read out of a fixture repository.
    /// This test used to build one and let planning shell out through
    /// `git remote get-url`, with no isolation, so the developer's own
    /// `url.*.insteadOf` decided the outcome: an inherited rewrite mapping
    /// github.com onto gitlab.com made it fail. What it is actually about is
    /// the planning logic, and that is now all it touches. The spellings a
    /// remote can arrive in are covered by the parser's own tests in `git.rs`
    /// and by `pull_request_lookup_resolves_an_ssh_scheme_origin_remote`, which
    /// runs git through the isolating helper.
    #[test]
    fn plan_entries_queries_a_resolved_remote_for_a_session_with_no_known_pr() {
        let entry = planning_entry();

        let (results, planned) = plan_entries(
            std::slice::from_ref(&entry),
            &|_| {
                git::RemoteResolution::Allowed(git::GitHubRemote {
                    host: "github.com".to_string(),
                    owner_repo: "octocat/Hello-World".to_string(),
                })
            },
            &legacy_policy(),
            SyncTrigger::BlindPoll,
        );

        assert!(
            results.is_empty(),
            "a resolvable remote must be queried, not recorded as an empty result"
        );
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].session_id, "s0");
        assert_eq!(planned[0].host, "github.com");
        assert_eq!(planned[0].owner, "octocat");
        assert_eq!(planned[0].repo, "Hello-World");
        assert_eq!(planned[0].branch, "feat/x");
    }

    /// A stored override row for the pinned tests: PR #12 on a FORK, while the
    /// session's own remote (when a test consults it at all) is
    /// `octocat/Hello-World`.
    fn pinned_stored(state: &str) -> StoredPr {
        StoredPr {
            session_id: "s0".to_string(),
            pr_number: 12,
            host: "github.com".to_string(),
            owner_repo: "forker/Hello-World".to_string(),
            state: state.to_string(),
            title: "Pinned".to_string(),
            url: "https://github.com/forker/Hello-World/pull/12".to_string(),
        }
    }

    fn pin_of(stored: &StoredPr) -> crate::worker::PinnedPr {
        crate::worker::PinnedPr {
            host: stored.host.clone(),
            owner_repo: stored.owner_repo.clone(),
            number: stored.pr_number,
        }
    }

    /// A pinned session queries the PINNED repo (here a fork), never the
    /// worktree's remote: the resolver panics to prove planning does not even
    /// look at it, and the built query carries exactly ONE alias, the by-number
    /// one for the pinned PR. Head-ref discovery is not planned for pins.
    #[test]
    fn plan_entries_pinned_session_queries_only_the_pinned_repo_by_number() {
        let stored = pinned_stored("OPEN");
        let entry = PrSyncEntry {
            known_pr: Some(stored.clone()),
            pinned: Some(pin_of(&stored)),
            ..planning_entry()
        };

        let (results, planned) = plan_entries(
            std::slice::from_ref(&entry),
            &|_| panic!("a pinned session must not resolve the worktree remote"),
            &legacy_policy(),
            SyncTrigger::BlindPoll,
        );

        assert!(results.is_empty(), "a pinned OPEN session is queried");
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].host, "github.com");
        assert_eq!(planned[0].owner, "forker");
        assert_eq!(planned[0].repo, "Hello-World");

        let (q, _) = build_chunk_query(&planned, &[0]);
        assert!(
            q.contains("repository(owner: \"forker\", name: \"Hello-World\")"),
            "the query targets the fork: {q}"
        );
        assert!(
            q.contains("s0_num: pullRequest(number: 12)"),
            "the pinned PR is refreshed by number: {q}"
        );
        assert!(
            !q.contains("s0_ref"),
            "no head-ref discovery for a pinned session: {q}"
        );
    }

    /// The host gate runs on the PINNED host. A denied pin answers from the
    /// pin's own stored row, and even a stale `known_pr` naming a DIFFERENT PR
    /// cannot leak through as the answer for a pinned session.
    #[test]
    fn plan_entries_pinned_gate_runs_on_the_pinned_host_and_answers_only_the_pin() {
        let stored = StoredPr {
            host: "git.company.example".to_string(),
            ..pinned_stored("OPEN")
        };
        // A stale non-pin row (the autodetected latest) that must NOT surface.
        let entry = PrSyncEntry {
            known_pr: Some(stored_pr_named(50, "OPEN", "octocat/Hello-World")),
            pinned: Some(pin_of(&stored)),
            ..planning_entry()
        };
        let policy = GithubHostPolicy::Hosts(std::iter::once("github.com".to_string()).collect());

        let (results, planned) = plan_entries(
            std::slice::from_ref(&entry),
            &|_| panic!("a pinned session must not resolve the worktree remote"),
            &policy,
            SyncTrigger::BlindPoll,
        );

        assert!(planned.is_empty(), "a denied pinned host is never queried");
        assert_eq!(results.len(), 1);
        let pr = results[0].1.as_ref().expect("the pin is kept");
        assert_eq!(
            pr.number, 12,
            "the answer is the PIN, not the stale stored latest"
        );
        assert_eq!(pr.owner_repo, "forker/Hello-World");
    }

    /// Merged pin + exited agent: zero network under either trigger,
    /// reconstructed from the pin.
    #[test]
    fn plan_entries_pinned_merged_exited_reconstructs_the_pin_without_network() {
        let stored = pinned_stored("MERGED");
        let entry = PrSyncEntry {
            known_pr: Some(stored.clone()),
            pinned: Some(pin_of(&stored)),
            agent_exited: true,
            ..planning_entry()
        };

        for trigger in [SyncTrigger::BlindPoll, SyncTrigger::OneShot] {
            let (results, planned) = plan_entries(
                std::slice::from_ref(&entry),
                &|_| panic!("a pinned session must not resolve the worktree remote"),
                &legacy_policy(),
                trigger,
            );

            assert!(planned.is_empty(), "merged pins are never queried");
            let pr = results[0].1.as_ref().expect("reconstructed pin");
            assert_eq!(pr.number, 12);
            assert_eq!(pr.state, PrState::Merged);
        }
    }

    /// Closed pin + exited agent: skipped by the blind poll, refreshed by a
    /// one-shot, because a closed pull request can be reopened.
    #[test]
    fn plan_entries_pinned_closed_exited_is_skipped_by_the_poll_and_refreshed_one_shot() {
        let stored = pinned_stored("CLOSED");
        let entry = PrSyncEntry {
            known_pr: Some(stored.clone()),
            pinned: Some(pin_of(&stored)),
            agent_exited: true,
            ..planning_entry()
        };

        let (results, planned) = plan_entries(
            std::slice::from_ref(&entry),
            &|_| panic!("a pinned session must not resolve the worktree remote"),
            &legacy_policy(),
            SyncTrigger::BlindPoll,
        );
        assert!(planned.is_empty(), "the blind poll spends no call here");
        let pr = results[0].1.as_ref().expect("reconstructed pin");
        assert_eq!(pr.number, 12);
        assert_eq!(pr.state, PrState::Closed);

        let (results, planned) = plan_entries(
            std::slice::from_ref(&entry),
            &|_| panic!("a pinned session must not resolve the worktree remote"),
            &legacy_policy(),
            SyncTrigger::OneShot,
        );
        assert!(results.is_empty(), "a one-shot asks about the closed pin");
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].owner, "forker");
    }

    /// The unpinned half of the same rule, at the other short-circuit site.
    #[test]
    fn plan_entries_closed_exited_is_skipped_by_the_poll_and_refreshed_one_shot() {
        let entry = PrSyncEntry {
            known_pr: Some(stored(12, "CLOSED")),
            agent_exited: true,
            ..planning_entry()
        };
        let resolve = |_: &Path| {
            git::RemoteResolution::Allowed(git::GitHubRemote {
                host: "github.com".to_string(),
                owner_repo: "octocat/Hello-World".to_string(),
            })
        };

        let (results, planned) = plan_entries(
            std::slice::from_ref(&entry),
            &resolve,
            &legacy_policy(),
            SyncTrigger::BlindPoll,
        );
        assert!(planned.is_empty(), "the blind poll spends no call here");
        let pr = results[0].1.as_ref().expect("reconstructed");
        assert_eq!(pr.state, PrState::Closed);

        let (results, planned) = plan_entries(
            std::slice::from_ref(&entry),
            &resolve,
            &legacy_policy(),
            SyncTrigger::OneShot,
        );
        assert!(
            results.is_empty(),
            "a one-shot asks whether the closed pull request came back"
        );
        assert_eq!(planned.len(), 1);
        // Discovery only: a terminal row gets no by-number alias, so noticing a
        // reopen costs nothing extra.
        let (q, _) = build_chunk_query(&planned, &[0]);
        assert!(q.contains("s0_ref"), "discovery is planned: {q}");
        assert!(!q.contains("s0_num"), "no extra by-number alias: {q}");
    }

    /// A per-alias fetch failure (the pinned repo alias came back null) keeps
    /// the pin rather than wiping the badge, mirroring the open-known fallback.
    #[test]
    fn pinned_per_alias_failure_preserves_the_pin() {
        let stored = pinned_stored("OPEN");
        let entry = PrSyncEntry {
            known_pr: Some(stored.clone()),
            pinned: Some(pin_of(&stored)),
            ..planning_entry()
        };
        let (_, planned) = plan_entries(
            std::slice::from_ref(&entry),
            &|_| panic!("no remote resolution for pins"),
            &legacy_policy(),
            SyncTrigger::BlindPoll,
        );
        let chunk = [0usize];
        let (_, pos_repo) = build_chunk_query(&planned, &chunk);
        let data = serde_json::json!({ "r0": serde_json::Value::Null });
        let (results, _) = parse_chunk_response(&planned, &chunk, &pos_repo, Some(&data));
        let pr = results[0].1.as_ref().expect("pin preserved");
        assert_eq!(pr.number, 12);

        // And a real answer for the pin refreshes it (a CLOSED pin can reopen).
        let data = serde_json::json!({ "r0": { "s0_num": pr_node(12, "MERGED") } });
        let (results, _) = parse_chunk_response(&planned, &chunk, &pos_repo, Some(&data));
        let pr = results[0].1.as_ref().expect("refreshed pin");
        assert_eq!(pr.number, 12);
        assert_eq!(pr.state, PrState::Merged);
    }

    /// A stored PR naming a DIFFERENT repo than the resolved query target must
    /// not put its number into the target repo's query: PR numbers are
    /// per-repo, so `other/Repo#12` asked of `octocat/Hello-World` is an
    /// unrelated pull request that would then be surfaced and persisted. The
    /// two ways this happens for real: a detached fork pin whose row survived
    /// somewhere, and a remote that changed under a session. Such an entry
    /// falls back to discovery only, and a discovery miss keeps the stored
    /// badge rather than inventing anything.
    #[test]
    fn a_known_pr_from_another_repo_never_emits_its_number_at_the_query_target() {
        let entry = PrSyncEntry {
            known_pr: Some(stored_pr_named(12, "OPEN", "forker/Hello-World")),
            ..planning_entry()
        };
        let (results, planned) = plan_entries(
            std::slice::from_ref(&entry),
            &|_| {
                git::RemoteResolution::Allowed(git::GitHubRemote {
                    host: "github.com".to_string(),
                    owner_repo: "octocat/Hello-World".to_string(),
                })
            },
            &legacy_policy(),
            SyncTrigger::BlindPoll,
        );
        assert!(results.is_empty());
        assert_eq!(planned.len(), 1);

        let chunk = [0usize];
        let (q, pos_repo) = build_chunk_query(&planned, &chunk);
        assert!(
            !q.contains("s0_num"),
            "the foreign row's number must not be asked of the session repo: {q}"
        );
        assert!(q.contains("s0_ref"), "discovery still runs: {q}");

        // Discovery finding nothing keeps the stored badge (per-alias-failure
        // semantics), and cannot surface an unrelated same-number PR because
        // no by-number alias exists to fetch one.
        let data = serde_json::json!({ "r0": { "s0_ref": serde_json::Value::Null } });
        let (results, _) = parse_chunk_response(&planned, &chunk, &pos_repo, Some(&data));
        let pr = results[0].1.as_ref().expect("stored badge kept");
        assert_eq!(pr.owner_repo, "forker/Hello-World");
        assert_eq!(pr.number, 12);

        // And a same-repo known row still gets its by-number refresh, so the
        // gate did not disable the branch-deleted-on-merge robustness.
        let entry = PrSyncEntry {
            known_pr: Some(stored_pr_named(12, "OPEN", "octocat/Hello-World")),
            ..planning_entry()
        };
        let (_, planned) = plan_entries(
            std::slice::from_ref(&entry),
            &|_| {
                git::RemoteResolution::Allowed(git::GitHubRemote {
                    host: "github.com".to_string(),
                    owner_repo: "octocat/Hello-World".to_string(),
                })
            },
            &legacy_policy(),
            SyncTrigger::BlindPoll,
        );
        let (q, _) = build_chunk_query(&planned, &[0]);
        assert!(q.contains("s0_num: pullRequest(number: 12)"), "{q}");
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
        assert!(err.contains("gh auth login"), "got {err}");

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

    fn stored_pr_named(number: u64, state: &str, owner_repo: &str) -> StoredPr {
        StoredPr {
            session_id: "s0".to_string(),
            pr_number: number,
            host: "github.com".to_string(),
            owner_repo: owner_repo.to_string(),
            state: state.to_string(),
            title: "t".to_string(),
            url: format!("https://github.com/{owner_repo}/pull/{number}"),
        }
    }

    /// The counterpart, so the assertion above is not vacuous: with nothing to
    /// resolve and nothing stored, the entry really does record an empty result
    /// and is never queried.
    ///
    /// The unresolvable remote is injected for the same reason as above. This
    /// test used to build a GitLab fixture repository and let planning read it
    /// through an unisolated `git remote get-url`, so an inherited rewrite
    /// pointing that address at github.com made it fail outright, and one
    /// pointing it at some other non-GitHub host let it pass while testing
    /// nothing about the spelling it named.
    #[test]
    fn plan_entries_records_nothing_for_an_unresolvable_remote_with_no_known_pr() {
        let entry = planning_entry();

        let (results, planned) = plan_entries(
            std::slice::from_ref(&entry),
            &|_| git::RemoteResolution::Unresolved,
            &legacy_policy(),
            SyncTrigger::BlindPoll,
        );

        assert!(planned.is_empty(), "a non-GitHub remote is never queried");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "s0");
        assert!(results[0].1.is_none());
    }

    /// A live address dux may not ask about is not the same thing as no live
    /// address, and only the second may fall back to a remembered host.
    ///
    /// The agent's own remote is readable and names `git.company.example`,
    /// which the policy does not allow. Its last known pull request is on
    /// github.com, which the policy DOES allow. Collapsing the denied address
    /// into "nothing resolved" made planning fall back to that stored host and
    /// query github.com about an agent whose remote is somewhere else entirely.
    /// The gate after the selection cannot catch it, because by then the live
    /// address is gone.
    ///
    /// The address is classified by the real code rather than asserted about in
    /// the abstract, so the test cannot pass on a hand-made verdict.
    #[test]
    fn a_readable_address_on_a_denied_host_never_falls_back_to_a_stored_one() {
        let entry = PrSyncEntry {
            known_pr: Some(StoredPr {
                session_id: "s0".to_string(),
                pr_number: 7,
                host: "github.com".to_string(),
                owner_repo: "octocat/Hello-World".to_string(),
                state: "OPEN".to_string(),
                title: "Hello".to_string(),
                url: "https://github.com/octocat/Hello-World/pull/7".to_string(),
            }),
            ..planning_entry()
        };
        // `gh` serves github.com and nothing else.
        let policy = GithubHostPolicy::Hosts(["github.com".to_string()].into_iter().collect());

        // The address really is READABLE and really is denied, so what the
        // planning does below is not the unreadable case under another name.
        assert_eq!(
            git::resolve_remote_from_git_output(
                b"git@git.company.example:acme/widget.git\n",
                &policy,
            ),
            git::RemoteResolution::Denied,
        );

        let (results, planned) = plan_entries(
            std::slice::from_ref(&entry),
            &|_| {
                git::resolve_remote_from_git_output(
                    b"git@git.company.example:acme/widget.git\n",
                    &policy,
                )
            },
            &policy,
            SyncTrigger::BlindPoll,
        );

        assert!(
            planned.is_empty(),
            "a denied live address must produce no gh call, {} were planned to {:?}",
            planned.len(),
            planned.iter().map(|p| p.host.clone()).collect::<Vec<_>>(),
        );
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].1.as_ref().map(|pr| pr.number),
            Some(7),
            "the agent keeps the pull request it last knew about",
        );
    }

    /// The THIRD place a host can enter, and the one that is easy to miss: a
    /// host remembered from a previous pull request, read back from SQLite and
    /// handed straight to `gh` without passing through either parser.
    ///
    /// An agent whose address cannot be read and whose stored host is not
    /// eligible must produce NO `gh` invocation at all. It keeps what it last
    /// knew rather than being reported as having no pull request, because dux
    /// did not ask and therefore did not find out.
    #[test]
    fn an_unreadable_address_with_an_ineligible_stored_host_plans_no_gh_call() {
        let entry = PrSyncEntry {
            known_pr: Some(StoredPr {
                session_id: "s0".to_string(),
                pr_number: 7,
                host: "git.company.example".to_string(),
                owner_repo: "acme/widget".to_string(),
                state: "OPEN".to_string(),
                title: "Widget".to_string(),
                url: "https://git.company.example/acme/widget/pull/7".to_string(),
            }),
            ..planning_entry()
        };
        // `gh` serves github.com and nothing else, so the stored host does not
        // qualify. It once might have, or it may predate the policy entirely.
        let policy = GithubHostPolicy::Hosts(["github.com".to_string()].into_iter().collect());

        let (results, planned) = plan_entries(
            std::slice::from_ref(&entry),
            &|_| git::RemoteResolution::Unresolved,
            &policy,
            SyncTrigger::BlindPoll,
        );

        assert!(
            planned.is_empty(),
            "an ineligible stored host must reach gh through no call at all, {} were planned",
            planned.len(),
        );
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].1.as_ref().map(|pr| pr.number),
            Some(7),
            "and the agent keeps the pull request it last knew about",
        );

        // The same entry under a policy that DOES name the host is queried
        // normally, so the gate is the policy and not the fallback itself.
        let serving =
            GithubHostPolicy::Hosts(["git.company.example".to_string()].into_iter().collect());
        let (_, planned) = plan_entries(
            std::slice::from_ref(&entry),
            &|_| git::RemoteResolution::Unresolved,
            &serving,
            SyncTrigger::BlindPoll,
        );
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].host, "git.company.example");
    }

    /// The live remote is lowercased by the parser, but a stored pull request
    /// comes straight out of SQLite, and a legacy or externally written row can
    /// carry a capitalised host. That host is handed to `gh --hostname`, so it
    /// is lowercased at the planning boundary whatever its source.
    #[test]
    fn plan_entries_lowercases_a_host_taken_from_a_stored_pull_request() {
        let entry = PrSyncEntry {
            known_pr: Some(StoredPr {
                session_id: "s0".to_string(),
                pr_number: 7,
                host: "GitHub.COM".to_string(),
                owner_repo: "octocat/Hello-World".to_string(),
                state: "OPEN".to_string(),
                title: "t".to_string(),
                url: "https://github.com/octocat/Hello-World/pull/7".to_string(),
            }),
            ..planning_entry()
        };

        // No remote at all, so planning must fall back to the stored PR.
        let (_, planned) = plan_entries(
            std::slice::from_ref(&entry),
            &|_| git::RemoteResolution::Unresolved,
            &legacy_policy(),
            SyncTrigger::BlindPoll,
        );

        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].host, "github.com");
    }
}
