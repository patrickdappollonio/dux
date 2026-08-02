//! GitHub CLI (`gh`) integration helpers used by the PR-sync worker
//! (`spawn_pr_sync_worker`, `spawn_initial_pr_refresh`, `spawn_pr_check_for_session`).
//! All helpers shell out to `gh` and parse JSON; no UI deps.

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
) -> PrSyncOutcome {
    let snapshot = match sessions.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => return (Vec::new(), Vec::new()),
    };
    run_entries(&snapshot, backoff)
}

/// Single-session PR check (foreground / refs-watcher / exit triggers). Shares
/// the batched machinery with a one-element batch; returns the PR plus the
/// per-host signal so the one-shot caller can arm/clear the shared backoff too.
pub fn check_pr_for_entry(
    entry: &PrSyncEntry,
    backoff: &BackoffSnapshot,
) -> (Option<PrInfo>, Vec<HostSignal>) {
    let (results, signals) = run_entries(std::slice::from_ref(entry), backoff);
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
fn run_entries(entries: &[PrSyncEntry], backoff: &BackoffSnapshot) -> PrSyncOutcome {
    let (mut results, planned) = plan_entries(entries, &live_remote_resolver);

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
fn live_remote_resolver(worktree_path: &Path) -> Option<git::GitHubRemote> {
    git::remote_github_repo(worktree_path)
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
    resolve_remote: &dyn Fn(&Path) -> Option<git::GitHubRemote>,
) -> (Vec<(String, Option<PrInfo>)>, Vec<Planned>) {
    let mut results: Vec<(String, Option<PrInfo>)> = Vec::new();
    let mut planned: Vec<Planned> = Vec::new();

    for entry in entries {
        // Resolve (host, owner_repo): live remote first, else the known PR's repo
        // (works even after the branch/remote is gone).
        let remote = resolve_remote(Path::new(&entry.worktree_path));
        let (host, owner_repo) = if let Some(remote) = remote {
            (remote.host, remote.owner_repo)
        } else if let Some(known) = &entry.known_pr {
            (known.host.clone(), known.owner_repo.clone())
        } else {
            results.push((entry.session_id.clone(), None));
            continue;
        };

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
            // Hostnames are case-insensitive and this value becomes a
            // `gh --hostname` argument. The parser lowercases a live remote,
            // but this host can also come straight out of SQLite, where a
            // legacy or externally written row may have kept its capitals, so
            // the lowercasing belongs here, at the boundary, whatever the
            // source.
            normalize_github_host(&host).to_ascii_lowercase(),
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
    let mut cmd = std::process::Command::new("gh");
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

pub fn gh_repo_arg(host: &str, owner_repo: &str) -> String {
    let host = normalize_github_host(host);
    if host == "github.com" {
        owner_repo.to_string()
    } else {
        format!("{host}/{owner_repo}")
    }
}

fn normalize_github_host(host: &str) -> &str {
    if host.trim().is_empty() {
        "github.com"
    } else {
        host
    }
}

/// Parse a user-typed PR reference into a [`PullRequestLookup`] for the selected
/// project's GitHub remote. Accepts:
///   - a bare PR number (`123`) — assumes the selected project's host/repo,
///   - a `#`-prefixed number (`#123`) — same assumption,
///   - a full GitHub PR URL (`https://github.com/owner/repo/pull/123`,
///     including GitHub Enterprise hosts and trailing `?query`/`#fragment`).
///
/// A URL whose host or owner/repo does not match the selected project's remote
/// is rejected with an actionable error, since fetching another repo's PR head
/// into this project's worktree would silently do the wrong thing.
///
/// This is a pure function shared by the TUI's new-agent-from-pr prompt and the
/// web's `CreateAgentFromPr` wire flow.
pub fn parse_pull_request_lookup(
    raw_input: &str,
    selected_host: &str,
    selected_owner_repo: &str,
) -> Result<PullRequestLookup, String> {
    let input = raw_input.trim();
    if input.is_empty() {
        return Err("Enter a GitHub PR URL or PR number.".to_string());
    }

    if let Ok(number) = input.strip_prefix('#').unwrap_or(input).parse::<u64>() {
        return Ok(PullRequestLookup {
            host: selected_host.to_string(),
            owner_repo: selected_owner_repo.to_string(),
            number,
        });
    }

    let Some((host, rest)) = parse_github_pull_url_parts(input) else {
        return Err("Enter a PR number, #number, or a GitHub PR URL.".to_string());
    };
    let parts: Vec<&str> = rest.split('/').collect();
    if parts.len() < 4 || parts[2] != "pull" {
        return Err(
            "GitHub PR URLs must look like https://github.com/owner/repo/pull/123.".to_string(),
        );
    }
    let owner_repo = format!("{}/{}", parts[0], parts[1]);
    if !host.eq_ignore_ascii_case(selected_host)
        || !owner_repo.eq_ignore_ascii_case(selected_owner_repo)
    {
        return Err(format!(
            "PR belongs to {host}/{owner_repo}, but the selected project uses {selected_host}/{selected_owner_repo}."
        ));
    }
    let number = parts[3]
        .parse::<u64>()
        .map_err(|_| "GitHub PR URL does not contain a valid PR number.".to_string())?;
    Ok(PullRequestLookup {
        host,
        owner_repo,
        number,
    })
}

fn parse_github_pull_url_parts(input: &str) -> Option<(String, String)> {
    let without_scheme = input
        .strip_prefix("https://")
        .or_else(|| input.strip_prefix("http://"))?;
    let (host, rest) = without_scheme.split_once('/')?;
    if host != "github.com" && !host.starts_with("github.") {
        return None;
    }
    let rest = rest
        .split(['?', '#'])
        .next()
        .unwrap_or(rest)
        .trim_end_matches('/')
        .to_string();
    Some((host.to_string(), rest))
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
) {
    let lookup = match git::remote_github_repo(Path::new(&project.path)) {
        Some(remote) => parse_pull_request_lookup(&raw_input, &remote.host, &remote.owner_repo),
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

    let repo = gh_repo_arg(&lookup.host, &lookup.owner_repo);
    let number = lookup.number.to_string();
    // Bounded so a hung `gh pr view` (stalled network, credential prompt) can't
    // strand the web CreateAgentFromPr Busy status forever.
    let result = match run_gh_with_timeout(
        &[
            "pr",
            "view",
            &number,
            "--repo",
            &repo,
            "--json",
            "number,title,state,headRefName",
        ],
        GH_CALL_TIMEOUT,
    ) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn gh_repo_arg_uses_owner_repo_for_github_dot_com() {
        assert_eq!(gh_repo_arg("github.com", "owner/repo"), "owner/repo");
        assert_eq!(gh_repo_arg("", "owner/repo"), "owner/repo");
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
        let plain = parse_pull_request_lookup("123", "github.com", "octocat/Hello-World")
            .expect("plain number");
        assert_eq!(plain.host, "github.com");
        assert_eq!(plain.owner_repo, "octocat/Hello-World");
        assert_eq!(plain.number, 123);

        let hashed = parse_pull_request_lookup("#456", "github.example.com", "octocat/Hello-World")
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
        )
        .expect_err("mismatched repo");
        assert!(err.contains("selected project uses github.com/octocat/Hello-World"));
    }

    #[test]
    fn parse_pull_request_lookup_rejects_empty_input() {
        let err = parse_pull_request_lookup("   ", "github.com", "octocat/Hello-World")
            .expect_err("empty");
        assert!(err.contains("Enter a GitHub PR URL or PR number"));
    }

    #[test]
    fn parse_pull_request_lookup_rejects_garbage() {
        let err = parse_pull_request_lookup("not-a-pr", "github.com", "octocat/Hello-World")
            .expect_err("garbage");
        assert!(err.contains("Enter a PR number, #number, or a GitHub PR URL"));
    }

    #[test]
    fn parse_pull_request_lookup_rejects_non_github_url() {
        let err = parse_pull_request_lookup(
            "https://gitlab.com/octocat/Hello-World/pull/1",
            "github.com",
            "octocat/Hello-World",
        )
        .expect_err("non-github host");
        assert!(err.contains("Enter a PR number, #number, or a GitHub PR URL"));
    }

    #[test]
    fn parse_pull_request_lookup_rejects_malformed_pull_path() {
        let err = parse_pull_request_lookup(
            "https://github.com/octocat/Hello-World/issues/3",
            "github.com",
            "octocat/Hello-World",
        )
        .expect_err("not a pull URL");
        assert!(err.contains("must look like https://github.com/owner/repo/pull/123"));
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

        let lookup = parse_pull_request_lookup("#7", &remote.host, &remote.owner_repo)
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

        let (results, planned) = plan_entries(std::slice::from_ref(&entry), &|_| {
            Some(git::GitHubRemote {
                host: "github.com".to_string(),
                owner_repo: "octocat/Hello-World".to_string(),
            })
        });

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

        let (results, planned) = plan_entries(std::slice::from_ref(&entry), &|_| None);

        assert!(planned.is_empty(), "a non-GitHub remote is never queried");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "s0");
        assert!(results[0].1.is_none());
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
        let (_, planned) = plan_entries(std::slice::from_ref(&entry), &|_| None);

        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].host, "github.com");
    }
}
