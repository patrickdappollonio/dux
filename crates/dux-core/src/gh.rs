//! GitHub CLI (`gh`) integration helpers used by the PR-sync worker
//! (`spawn_pr_sync_worker`, `spawn_initial_pr_refresh`, `spawn_pr_check_for_session`).
//! All helpers shell out to `gh` and parse JSON; no UI deps.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::git;
use crate::logger;
use crate::model::{PrInfo, PrState, Project};
use crate::storage::StoredPr;
use crate::worker::{PrSyncEntry, PullRequestLookup, ResolvedPullRequest, WorkerEvent};

/// Live GraphQL rate-limit snapshot parsed from a batched query's top-level
/// `rateLimit` field. Lets the PR-sync loop back off before it exhausts the
/// GraphQL points budget (typically 5000/hour, higher on GitHub Enterprise Cloud).
#[derive(Clone, Debug)]
pub struct RateLimitInfo {
    pub remaining: i64,
    pub reset_at: Option<chrono::DateTime<chrono::Utc>>,
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
) -> PrSyncOutcome {
    let snapshot = match sessions.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => return (Vec::new(), Vec::new()),
    };
    run_entries(&snapshot, backoff, policy)
}

/// Single-session PR check (foreground / refs-watcher / exit triggers). Shares
/// the batched machinery with a one-element batch; returns the PR plus the
/// per-host signal so the one-shot caller can arm/clear the shared backoff too.
pub fn check_pr_for_entry(
    entry: &PrSyncEntry,
    backoff: &BackoffSnapshot,
    policy: &GithubHostPolicy,
) -> (Option<PrInfo>, Vec<HostSignal>) {
    let (results, signals) = run_entries(std::slice::from_ref(entry), backoff, policy);
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
}

/// Single source of truth for "this stored PR is terminal (MERGED/CLOSED)".
/// Used by both `run_entries`' zero-network short-circuit and `Planned::new`.
fn stored_pr_is_terminal(known: Option<&StoredPr>) -> bool {
    known.is_some_and(|k| k.state == "MERGED" || k.state == "CLOSED")
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
        // undiscovered sessions get only the head-ref discovery alias.
        let emit_num = known.is_some() && !is_terminal;
        Planned {
            session_id,
            host,
            owner,
            repo,
            branch,
            known,
            is_terminal,
            emit_num,
        }
    }
}

/// Core of the batched sync. Classifies each entry, resolves the ones that need
/// no network call (terminal + exited → reconstruct from SQLite), and batches
/// the rest into `gh api graphql` requests grouped by host.
///
/// Per-session strategy (preserves the pre-batch semantics exactly):
///
/// | Known PR state | Agent running? | Aliases                                   |
/// |----------------|----------------|-------------------------------------------|
/// | None           | any            | head-ref discovery                        |
/// | OPEN           | any            | head-ref discovery **+** by-number refresh|
/// | MERGED/CLOSED  | yes            | head-ref discovery (catches a follow-up PR)|
/// | MERGED/CLOSED  | no             | **zero calls** — reconstruct from SQLite  |
fn run_entries(
    entries: &[PrSyncEntry],
    backoff: &BackoffSnapshot,
    policy: &GithubHostPolicy,
) -> PrSyncOutcome {
    let (mut results, planned) =
        plan_entries(entries, &|path| live_remote_resolver(path, policy), policy);

    // Group by host; for each host either skip it (already backed off — keep
    // last-known PRs, no gh call, no signal) or chunk its sessions by alias
    // budget and emit one per-host signal driving the backoff.
    let now = Instant::now();
    let mut signals: Vec<HostSignal> = Vec::new();
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
            let cost = if planned[i].emit_num { 2 } else { 1 };
            if !chunk.is_empty() && alias_count + cost > MAX_ALIASES_PER_QUERY {
                let (r, rl, failed, limited) = run_chunk(&host, &planned, &chunk);
                results.extend(r);
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
/// `url.*.insteadOf` applied for real), so an inherited rewrite used to reach
/// straight into these tests. It was measured, not supposed: with a rewrite
/// mapping the fixture's GitLab address onto github.com the negative test
/// FAILED, and with one mapping github.com onto gitlab.com the positive test
/// failed, so neither was testing the remote spelling it named.
fn plan_entries(
    entries: &[PrSyncEntry],
    resolve_remote: &dyn Fn(&Path) -> git::RemoteResolution,
    policy: &GithubHostPolicy,
) -> (Vec<(String, Option<PrInfo>)>, Vec<Planned>) {
    let mut results: Vec<(String, Option<PrInfo>)> = Vec::new();
    let mut planned: Vec<Planned> = Vec::new();

    for entry in entries {
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

        // Terminal PR + exited agent: nobody is pushing to that branch anymore,
        // so reconstruct from SQLite with zero network calls.
        if stored_pr_is_terminal(entry.known_pr.as_ref()) && entry.agent_exited {
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
/// All three fields are REQUIRED, so a record missing one, or carrying null or
/// the wrong type in one, fails to deserialize and takes the whole response
/// down with it. That is deliberate, and it is the difference between "gh says
/// no" and "dux could not read what gh said". They were previously optional,
/// which produced two decisive-looking answers out of records that decided
/// nothing: a missing or null `active` yielded an empty but successfully parsed
/// host set, which is a decisive "gh serves nothing" that turns every GitHub
/// feature off and replaces the last known good policy; and a missing or null
/// `host` alongside a successful, active record qualified the MAP KEY on the
/// strength of a record that never said which host it describes.
///
/// A response containing an unreadable record is therefore transient (see
/// [`decide_gh_probe`]), which preserves the last known good policy. gh 2.95.0
/// emits all three fields on every account.
#[derive(serde::Deserialize)]
struct AuthStatusAccount {
    state: String,
    active: bool,
    host: String,
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
pub(crate) fn parse_auth_status_hosts(stdout: &str) -> Option<BTreeSet<String>> {
    let parsed: AuthStatusOutput = serde_json::from_str(stdout.trim()).ok()?;
    let mut eligible = BTreeSet::new();
    for (key, accounts) in &parsed.hosts {
        let key = key.trim().to_ascii_lowercase();
        if key.is_empty() {
            continue;
        }
        if accounts.iter().any(|account| account.qualifies(&key)) {
            eligible.insert(key);
        }
    }
    Some(eligible)
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
/// In each case stdout is EMPTY, which is why "it did not parse" used to look
/// like a good enough signal on its own. It is not: `gh` exits non-zero in JSON
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
    if let Some(eligible) = parse_auth_status_hosts(&String::from_utf8_lossy(&output.stdout)) {
        return GhProbe::Decided {
            // GitHub is available when at least one host reports success. This
            // is the behaviour change: plain `gh auth status` exits non-zero
            // when ANY known host has a problem, so one stale token used to
            // disable every GitHub feature on every host.
            available: !eligible.is_empty(),
            policy: GithubHostPolicy::Hosts(eligible),
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
        GhCallOutcome::Completed(output) => GhProbe::Decided {
            available: output.status.success(),
            policy: GithubHostPolicy::LegacyNameRule,
        },
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

/// Binary-agnostic core of [`run_gh_with_timeout`], split out so the
/// timeout/kill/drain contract is unit-testable with `sleep`/`sh` instead of a
/// live `gh`. (Distinct from `git::wait_child_or_kill`, which deliberately does
/// NOT drain stdout/stderr — it pipes only a tiny stderr — so it can't be reused
/// for the larger GraphQL responses this helper captures.)
fn run_command_with_timeout(mut cmd: std::process::Command, timeout: Duration) -> GhCallOutcome {
    let mut child = match cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => return GhCallOutcome::Failed(err.to_string()),
    };

    // Drain the pipes on their own threads (so a full pipe buffer can't wedge the
    // child) and hand each buffer back over a channel, so we can wait for them
    // with a deadline and abandon them if a grandchild keeps the pipe open.
    let (out_tx, out_rx) = std::sync::mpsc::channel();
    let (err_tx, err_rx) = std::sync::mpsc::channel();
    match (child.stdout.take(), child.stderr.take()) {
        (Some(mut out), Some(mut err)) => {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = out.read_to_end(&mut buf);
                let _ = out_tx.send(buf);
            });
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = err.read_to_end(&mut buf);
                let _ = err_tx.send(buf);
            });
        }
        _ => {
            // Should be unreachable (we just set piped stdio), but never leak the
            // spawned child if a pipe handle is somehow missing.
            let _ = child.kill();
            let _ = child.wait();
            return GhCallOutcome::Failed("gh stdout/stderr pipe unavailable".to_string());
        }
    }

    // Read the readers with a deadline, then abandon them.
    let drain = |rx: &std::sync::mpsc::Receiver<Vec<u8>>| {
        rx.recv_timeout(GH_READER_DRAIN).unwrap_or_default()
    };

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return GhCallOutcome::Completed(std::process::Output {
                    status,
                    stdout: drain(&out_rx),
                    stderr: drain(&err_rx),
                });
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    // Best-effort drain; the readers unblock once the killed
                    // child's pipes close.
                    let _ = drain(&out_rx);
                    let _ = drain(&err_rx);
                    return GhCallOutcome::TimedOut;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = drain(&out_rx);
                let _ = drain(&err_rx);
                return GhCallOutcome::Failed(format!("waiting for gh failed: {err}"));
            }
        }
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
            let qname = graphql_string(&format!("refs/heads/{}", p.branch));
            q.push_str(&format!(
                "    {}: ref(qualifiedName: {qname}) {{ associatedPullRequests(first: 1, orderBy: {{field: CREATED_AT, direction: DESC}}) {{ nodes {{ number state title url }} }} }}\n",
                ref_alias(pos),
            ));
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
        let ref_pr = repo_obj
            .and_then(|r| r.get(ref_alias(pos).as_str()))
            .and_then(|rf| rf.get("associatedPullRequests"))
            .and_then(|a| a.get("nodes"))
            .and_then(|n| n.as_array())
            .and_then(|arr| arr.first())
            .and_then(|node| parse_pr_json_value(node, &p.host, &owner_repo));
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

/// Reconcile the head-ref discovery result and the by-number refresh into the
/// single PR to report, matching the pre-batch behavior:
///   - terminal + running: a strictly-newer follow-up PR wins, else the stored PR
///   - open known: the newest PR by number wins (a newer PR opened on the same
///     branch), else the by-number refresh (robust when the branch was deleted)
///   - undiscovered: whatever the head-ref discovery found
fn merge_pr_result(p: &Planned, ref_pr: Option<PrInfo>, num_pr: Option<PrInfo>) -> Option<PrInfo> {
    let Some(known) = &p.known else {
        return ref_pr;
    };
    if p.is_terminal {
        if let Some(r) = &ref_pr
            && r.number > known.pr_number
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
    })
}

/// Reconstruct a PrInfo from stored data without a network call.
/// Used for terminal states (merged/closed) that don't need refreshing.
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
#[cfg(test)]
pub(crate) mod probe_test_support {
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
    pub(crate) fn stand_in_gh(dir: &Path, body: &str) -> PathBuf {
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
    pub(crate) fn stand_in_gh_serving(dir: &Path, hosts: &[&str]) -> PathBuf {
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
        serde_json::json!({
            "number": number,
            "state": state,
            "title": format!("PR {number}"),
            "url": format!("https://github.com/octocat/Hello-World/pull/{number}"),
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
        assert!(q.contains("CREATED_AT"));
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
        };
        let b = RateLimitInfo {
            remaining: 50,
            reset_at: None,
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
    fn parse_chunk_response_terminal_running_keeps_stored_unless_newer() {
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
            "r1": { "s1_ref": ref_node(9, "OPEN") },
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
    fn run_entries_terminal_exited_reconstructs_without_network() {
        // A terminal + exited session is reconstructed from SQLite with no gh call
        // (the worktree path is bogus, so any git/gh access would fail).
        let entry = PrSyncEntry {
            session_id: "s0".to_string(),
            branch_name: "feat/done".to_string(),
            worktree_path: "/nonexistent/dux-test-path".to_string(),
            known_pr: Some(stored(42, "MERGED")),
            agent_exited: true,
        };
        let (results, signals) = run_entries(
            std::slice::from_ref(&entry),
            &std::collections::HashMap::new(),
            &legacy_policy(),
        );
        // Zero-network (terminal+exited) → no host was queried, so no signal.
        assert!(signals.is_empty(), "no network call means no host signal");
        assert_eq!(results.len(), 1);
        let pr = results[0].1.as_ref().expect("reconstructed");
        assert_eq!(pr.number, 42);
        assert_eq!(pr.state, PrState::Merged);
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
        let dir = tempfile::tempdir().expect("tempdir");
        let recorded = dir.path().join("argv.txt");
        let script = dir.path().join("fake-gh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > '{}'\nprintf '{{}}'\n",
                recorded.display()
            ),
        )
        .expect("write stand-in gh");
        std::fs::set_permissions(
            &script,
            <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
        )
        .expect("chmod");

        let args = pr_view_args("github.com", "owner/repo", 42);
        let mut cmd = std::process::Command::new(&script);
        cmd.args(&args);
        cmd.env("GH_HOST", "git.company.example");
        let outcome = run_command_with_timeout(cmd, Duration::from_secs(10));
        assert!(
            matches!(outcome, GhCallOutcome::Completed(ref o) if o.status.success()),
            "stand-in gh should have run",
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
        );

        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].host, "github.com");
    }
}
