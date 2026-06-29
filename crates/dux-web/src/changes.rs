//! `ChangesService`: the per-session changed-files cache, single-flight git
//! compute, monotonic `rev` chokepoint, and the interest-driven supervised
//! poller behind `GET /api/v1/sessions/:id/changes` and the `session.changes`
//! event.
//!
//! Held in [`crate::server::AppState`] as an `Arc<ChangesService>`. The poller is
//! spawned once at construction (see [`ChangesService::new`]).
//!
//! ## How a change is detected and signalled
//!
//! `compute(id)` resolves the session's worktree (an async actor round-trip) then
//! runs `git status` off the reactor in `spawn_blocking`, sorts both lists by
//! `(path, status)`, and compares them to the cached previous lists. On a real
//! difference (or a recovery from an error state) it bumps the SQLite-persisted
//! per-session `rev` (the single chokepoint, via [`crate::engine_actor::EngineHandle::next_changes_rev`])
//! and emits `session.changes {id, rev}` on the [`EventBus`]. The client then
//! re-GETs and applies the response only if its `rev` is newer.
//!
//! ## Single-flight
//!
//! Many GETs or poll ticks for the same cold session collapse to exactly one git
//! compute. The owner inserts a `watch::Receiver<bool>` under the inflight lock;
//! late callers clone it and `wait_for(done)`, then re-read the cache. A drop
//! guard guarantees the inflight slot is cleared and waiters are woken on EVERY
//! exit path including future cancellation (an HTTP client disconnect drops the
//! compute future at its `.await`); a waiter that wakes to an absent cache (owner
//! cancelled before storing) re-elects a new owner rather than returning a
//! spurious empty-cache error.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use tokio::sync::watch;

use dux_core::model::ChangedFile;
use dux_core::viewmodel::ChangedFileView;
use dux_core::wire::WireStatus;

use crate::engine_actor::EngineHandle;
use crate::event_bus::{Event, EventBus};

/// How long a cached git error is served before a fresh compute is attempted, so
/// repeated GETs during a transient lock/rebase do not each spawn git.
const ERROR_TTL: Duration = Duration::from_secs(2);

/// Poll cadence when an agent PTY is live (changes are likely) versus idle.
const POLL_ACTIVE: Duration = Duration::from_secs(2);
const POLL_IDLE: Duration = Duration::from_secs(10);

/// Per-session timeout for one poll-tick compute, so one slow/locked repo cannot
/// stall the fan-out across the other interested sessions.
const PER_SESSION_TIMEOUT: Duration = Duration::from_secs(15);

/// Bounded fan-out across interested sessions per poll tick.
const POLL_FANOUT: usize = 8;

/// Backoff before restarting the poll loop after a panic.
const POLL_RESTART_BACKOFF: Duration = Duration::from_secs(1);

/// How long a session's cache entry lingers after its interest reaches zero
/// before the poller evicts it (a short grace so a quick unsubscribe/resubscribe,
/// e.g. a reconnect, does not throw away a still-valid entry).
const EVICT_GRACE: Duration = Duration::from_secs(30);

/// Consecutive compute errors for one session before a keyed `Warning` status is
/// raised (cleared by a keyed success on the next good compute).
const ERROR_WARN_THRESHOLD: usize = 3;

/// Lock a `Mutex` poison-tolerantly: a thread that panicked while holding one of
/// these maps poisons it, but the maps are plain caches whose invariants are
/// re-established on the next compute, so recovering the inner guard is safe and
/// far better than propagating the panic across every interested session.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// One cached changed-files result for a session.
enum Cached {
    /// A successful compute. `prev` is the sorted (staged, unstaged) lists used for
    /// change detection and served verbatim to GETs.
    Ok {
        rev: u64,
        /// The invalidation generation observed when this compute STARTED reading
        /// the filesystem. A waiter requiring a newer generation re-elects rather
        /// than accepting this snapshot (see [`ChangesService::compute_cached`]).
        generation: u64,
        prev: (Vec<ChangedFileView>, Vec<ChangedFileView>),
    },
    /// A failed compute, served as `409 + Retry-After` until [`ERROR_TTL`] elapses.
    Err {
        #[allow(dead_code)]
        rev: u64,
        /// The invalidation generation observed when this compute started (mirrors
        /// `Ok::generation` so the waiter freshness check is uniform across variants).
        generation: u64,
        at: Instant,
        message: String,
    },
}

impl Cached {
    /// The invalidation generation this entry's compute observed at start.
    fn generation(&self) -> u64 {
        match self {
            Cached::Ok { generation, .. } => *generation,
            Cached::Err { generation, .. } => *generation,
        }
    }
}

/// The result of [`ChangesService::get`] before HTTP projection.
pub struct ChangesResponse {
    pub rev: u64,
    pub staged: Vec<ChangedFileView>,
    pub unstaged: Vec<ChangedFileView>,
}

/// Why a changed-files read could not be served.
pub enum GitError {
    /// The session id is unknown (no worktree). The handler maps this to 404.
    SessionNotFound,
    /// A git lock/rebase or other git failure. The handler maps this to
    /// `409 + Retry-After`.
    Git(String),
}

pub struct ChangesService {
    engine: EngineHandle,
    bus: Arc<EventBus>,
    cache: Mutex<HashMap<String, Cached>>,
    /// Single-flight registry: a session id maps to a receiver that flips `true`
    /// when the owning compute finishes (success or cancellation).
    inflight: Mutex<HashMap<String, watch::Receiver<bool>>>,
    /// Consecutive-error streaks per session, for the keyed `Warning` escalation.
    error_streak: Mutex<HashMap<String, usize>>,
    /// Sessions for which a keyed `Warning` was actually emitted (the streak hit
    /// [`ERROR_WARN_THRESHOLD`]). Only these get a "Changed files are available
    /// again" recovery info on the next good compute, so a short 1-2 error blip
    /// that never showed a warning does not leave an orphaned recovery toast.
    warning_emitted: Mutex<HashSet<String>>,
    /// First time each cache entry was seen with zero interest, for grace eviction.
    uninterested_since: Mutex<HashMap<String, Instant>>,
    /// Monotonic invalidation generation. [`Self::invalidate`] bumps it after a git
    /// mutation; each compute stamps the entry it stores with the value observed
    /// when it STARTED reading the filesystem. A waiter that wakes to an entry
    /// stamped at an older generation than the one it required (a concurrent
    /// compute that read the PRE-mutation filesystem) re-elects as owner and
    /// recomputes, so the post-mutation state always wins.
    invalidation_gen: AtomicU64,
    /// Total git computes run. Test instrumentation for the single-flight test.
    compute_count: AtomicUsize,
}

impl ChangesService {
    /// Build the service and spawn the supervised poll loop. MUST be called from
    /// within a tokio runtime context (the serve paths call it inside their
    /// runtime; the flip wraps its `build_app` in `runtime.enter()`).
    pub fn new(engine: EngineHandle, bus: Arc<EventBus>) -> Arc<Self> {
        let svc = Arc::new(Self {
            engine,
            bus,
            cache: Mutex::new(HashMap::new()),
            inflight: Mutex::new(HashMap::new()),
            error_streak: Mutex::new(HashMap::new()),
            warning_emitted: Mutex::new(HashSet::new()),
            uninterested_since: Mutex::new(HashMap::new()),
            invalidation_gen: AtomicU64::new(0),
            compute_count: AtomicUsize::new(0),
        });
        Self::spawn_poller(Arc::downgrade(&svc));
        svc
    }

    /// Supervisor: run the poll loop, restarting it on panic with backoff. Holds
    /// only a `Weak` so the service (and thus the loop) drops when `AppState` does.
    fn spawn_poller(weak: Weak<Self>) {
        tokio::spawn(async move {
            loop {
                let weak_inner = weak.clone();
                let handle = tokio::spawn(async move { Self::poll_loop(weak_inner).await });
                match handle.await {
                    // Clean exit: the service was dropped, so stop supervising.
                    Ok(()) => break,
                    Err(join_err) if join_err.is_panic() => {
                        dux_core::logger::error(&format!(
                            "changed-files poller panicked; restarting after backoff: {join_err}"
                        ));
                        // Stop if the service is already gone; otherwise surface the
                        // degradation to web clients (not just dux.log) before the
                        // restart so a stalled file list is explainable. Keyed so a
                        // repeated restart replaces rather than stacks the toast.
                        let Some(svc) = weak.upgrade() else {
                            break;
                        };
                        svc.engine.emit_status(WireStatus::keyed(
                            "changes-poller",
                            "warning",
                            "Changed-files updates were interrupted and are restarting; \
                             the file list may briefly lag.",
                        ));
                        tokio::time::sleep(POLL_RESTART_BACKOFF).await;
                    }
                    // Cancelled (runtime shutdown): nothing to restart.
                    Err(_) => break,
                }
            }
        });
    }

    /// The poll loop body. Each tick: pick a cadence from `has_active_processes`,
    /// sleep, then recompute every interested session with bounded fan-out and a
    /// per-session timeout, and grace-evict cache entries that lost all interest.
    /// Exits when the service is dropped (the `Weak` fails to upgrade).
    async fn poll_loop(weak: Weak<Self>) {
        loop {
            let cadence = match weak.upgrade() {
                Some(svc) => {
                    if svc.engine.has_active_processes() {
                        POLL_ACTIVE
                    } else {
                        POLL_IDLE
                    }
                }
                None => return,
            };
            tokio::time::sleep(cadence).await;

            let Some(svc) = weak.upgrade() else {
                return;
            };
            let sessions = svc.bus.interested_sessions();
            if !sessions.is_empty() {
                futures_util::stream::iter(sessions)
                    .for_each_concurrent(POLL_FANOUT, |id| {
                        let svc = Arc::clone(&svc);
                        async move {
                            let id_for_err = id.clone();
                            if tokio::time::timeout(PER_SESSION_TIMEOUT, svc.compute_cached(id))
                                .await
                                .is_err()
                            {
                                // The per-session compute exceeded its budget (a
                                // slow/locked repo). Don't swallow it: log, then
                                // record a cached error so the keyed-Warning streak
                                // escalation fires just like a real git failure.
                                dux_core::logger::warn(&format!(
                                    "changed-files compute for session {id_for_err} timed out \
                                     after {PER_SESSION_TIMEOUT:?}; recording an error"
                                ));
                                let rev = svc.engine.next_changes_rev(id_for_err.clone()).await;
                                let generation = svc.invalidation_gen.load(Ordering::SeqCst);
                                // `clobber_ok = false`: this timeout runs AFTER its
                                // compute was cancelled, racing a freshly-elected
                                // owner. That owner may have just stored a valid
                                // `Cached::Ok` (with a LOWER rev than this later-minted
                                // timeout rev), so the plain `rev >= existing` guard
                                // would let the giving-up error clobber the good result
                                // and surface a spurious 409. Refuse to overwrite an Ok.
                                svc.store_err(
                                    &id_for_err,
                                    rev,
                                    generation,
                                    "changed-files computation timed out".to_string(),
                                    false,
                                );
                            }
                        }
                    })
                    .await;
            }
            svc.evict_uninterested();
        }
    }

    /// Evict cache entries for sessions that have had zero interest for longer than
    /// [`EVICT_GRACE`]. Also covers session deletion: a deleted session loses its
    /// last subscriber, so it ages out here.
    fn evict_uninterested(&self) {
        let interested: HashSet<String> = self.bus.interested_sessions().into_iter().collect();
        let now = Instant::now();
        let mut since = lock(&self.uninterested_since);
        let mut cache = lock(&self.cache);
        let keys: Vec<String> = cache.keys().cloned().collect();
        for key in keys {
            if interested.contains(&key) {
                since.remove(&key);
                continue;
            }
            let first = *since.entry(key.clone()).or_insert(now);
            if now.duration_since(first) >= EVICT_GRACE {
                cache.remove(&key);
                since.remove(&key);
            }
        }
        // Drop grace timers for sessions whose cache entry is already gone.
        since.retain(|k, _| cache.contains_key(k));
        // Prune per-session bookkeeping for sessions no longer cached so an evicted
        // (e.g. deleted) session leaves nothing behind. Bounded to currently-cached
        // sessions, mirroring the grace-timer retain above.
        lock(&self.error_streak).retain(|k, _| cache.contains_key(k));
        lock(&self.warning_emitted).retain(|k| cache.contains_key(k));
    }

    /// Serve the cached changed files, computing under single-flight on a miss.
    ///
    /// When no cache entry exists AFTER a compute, the session either is unknown
    /// (the compute deliberately stores nothing for a vanished session, so the
    /// caller gets [`GitError::SessionNotFound`] → 404 and clears/unsubscribes) or
    /// its entry was evicted by the poller mid-call (a real-session race), in which
    /// case the compute is retried exactly once rather than returning a false 409.
    pub async fn get(self: &Arc<Self>, session_id: &str) -> Result<ChangesResponse, GitError> {
        if let Some(result) = self.read_fresh(session_id) {
            return result;
        }
        self.compute_cached(session_id.to_string()).await;
        if let Some(result) = self.read_cached(session_id) {
            return result;
        }
        // No entry after the compute. Distinguish "session gone" (404) from "real
        // session, entry raced with an evict" (retry once). A gone session has no
        // worktree.
        if self
            .engine
            .session_worktree(session_id.to_string())
            .await
            .is_none()
        {
            return Err(GitError::SessionNotFound);
        }
        // Known session whose entry was evicted between the compute and the read:
        // recompute once and serve that.
        self.compute_cached(session_id.to_string()).await;
        match self.read_cached(session_id) {
            Some(result) => result,
            // Still nothing — it vanished during the retry (or raced again): treat
            // it as gone so the client stops polling it.
            None => Err(GitError::SessionNotFound),
        }
    }

    /// Read the current cache entry (regardless of error TTL), projecting it to the
    /// HTTP result. `None` means there is no entry at all.
    fn read_cached(&self, session_id: &str) -> Option<Result<ChangesResponse, GitError>> {
        let cache = lock(&self.cache);
        match cache.get(session_id) {
            Some(Cached::Ok { rev, prev, .. }) => Some(Ok(ChangesResponse {
                rev: *rev,
                staged: prev.0.clone(),
                unstaged: prev.1.clone(),
            })),
            Some(Cached::Err { message, .. }) => Some(Err(GitError::Git(message.clone()))),
            None => None,
        }
    }

    /// Return a cached result if it is fresh (an `Ok` entry, or an `Err` within
    /// [`ERROR_TTL`]); `None` means a compute is needed.
    fn read_fresh(&self, session_id: &str) -> Option<Result<ChangesResponse, GitError>> {
        let cache = lock(&self.cache);
        match cache.get(session_id) {
            Some(Cached::Ok { rev, prev, .. }) => Some(Ok(ChangesResponse {
                rev: *rev,
                staged: prev.0.clone(),
                unstaged: prev.1.clone(),
            })),
            Some(Cached::Err { at, message, .. }) if at.elapsed() < ERROR_TTL => {
                Some(Err(GitError::Git(message.clone())))
            }
            _ => None,
        }
    }

    /// Drop a session's cached lists and trigger a fresh compute+emit. Called by
    /// the git/file mutation handlers after a successful stage/unstage/discard/
    /// commit/write so the pane refreshes immediately rather than after the poll
    /// interval. Dropping the entry forces the next compute to detect a change
    /// (no `prev`) and emit `session.changes`.
    pub fn invalidate(self: &Arc<Self>, session_id: String) {
        // Bump the invalidation generation BEFORE clearing the entry and spawning
        // the recompute. If a poller compute is already past its `spawn_blocking`
        // (it read the PRE-mutation filesystem) it stays the single-flight owner
        // and will store its now-stale snapshot; the recompute spawned below would
        // otherwise become a waiter, see that populated entry, and exit early —
        // briefly serving pre-mutation files. By requiring an entry stamped at this
        // newer generation, that waiter re-elects as owner and recomputes instead.
        self.invalidation_gen.fetch_add(1, Ordering::SeqCst);
        {
            let mut cache = lock(&self.cache);
            cache.remove(&session_id);
        }
        let svc = Arc::clone(self);
        tokio::spawn(async move {
            svc.compute_cached(session_id).await;
        });
    }

    /// The cached `rev` for a session, if any. Used by the `/ws/events` lag
    /// catch-up and subscribe catch-up to stamp the synthetic `session.changes`
    /// frame. Returns `None` for a cold cache; the caller serialises that as an
    /// absent `rev` field, which the client treats as a force-refetch.
    pub fn peek_rev(&self, session_id: &str) -> Option<u64> {
        let cache = lock(&self.cache);
        match cache.get(session_id) {
            Some(Cached::Ok { rev, .. }) => Some(*rev),
            Some(Cached::Err { rev, .. }) => Some(*rev),
            None => None,
        }
    }

    /// Seed the cache with a known `rev` for `session_id`. Test-only: lets
    /// server-level unit tests assert subscribe catch-up behaviour without
    /// spinning up a git repo or a real poller compute.
    #[cfg(test)]
    pub fn seed_rev_for_test(&self, session_id: &str, rev: u64) {
        let mut cache = lock(&self.cache);
        cache.insert(
            session_id.to_string(),
            Cached::Ok {
                rev,
                generation: 0,
                prev: (vec![], vec![]),
            },
        );
    }

    /// Single-flight wrapper around [`Self::compute`]. Exactly one compute runs per
    /// session at a time; late callers wait for the owner and re-read the cache,
    /// re-electing a new owner if the previous owner was cancelled before storing.
    async fn compute_cached(self: &Arc<Self>, session_id: String) {
        // The minimum invalidation generation this call must see reflected in the
        // cache before accepting another compute's result. Captured once at entry:
        // a recompute spawned by `invalidate()` runs AFTER the bump, so it requires
        // the post-mutation generation and will not accept a stale owner's snapshot.
        let required_gen = self.invalidation_gen.load(Ordering::SeqCst);
        loop {
            enum Role {
                Owner(watch::Sender<bool>),
                Waiter(watch::Receiver<bool>),
            }
            let role = {
                let mut inflight = lock(&self.inflight);
                match inflight.get(&session_id) {
                    Some(rx) => Role::Waiter(rx.clone()),
                    None => {
                        let (tx, rx) = watch::channel(false);
                        inflight.insert(session_id.clone(), rx);
                        Role::Owner(tx)
                    }
                }
            };
            match role {
                Role::Owner(tx) => {
                    self.run_owned_compute(&session_id, tx).await;
                    return;
                }
                Role::Waiter(mut rx) => {
                    // Wait for the owner to finish, then read the cache.
                    let _ = rx.wait_for(|done| *done).await;
                    let fresh_enough = {
                        let cache = lock(&self.cache);
                        match cache.get(&session_id) {
                            // Accept only an entry stamped at our required generation
                            // or newer. An older stamp means the owner read the
                            // pre-mutation filesystem (it started before our
                            // `invalidate()` bump), so re-elect and recompute.
                            Some(entry) => entry.generation() >= required_gen,
                            None => false,
                        }
                    };
                    if fresh_enough {
                        return;
                    }
                    // The owner was cancelled before storing, or stored a snapshot
                    // older than our required generation — re-elect a new owner.
                    continue;
                }
            }
        }
    }

    /// Run the compute as the single-flight owner. A drop guard clears the inflight
    /// slot and wakes waiters on EVERY exit path (success, error, or future
    /// cancellation at an `.await`).
    async fn run_owned_compute(self: &Arc<Self>, session_id: &str, tx: watch::Sender<bool>) {
        struct InflightGuard<'a> {
            inflight: &'a Mutex<HashMap<String, watch::Receiver<bool>>>,
            id: String,
            tx: watch::Sender<bool>,
        }
        impl Drop for InflightGuard<'_> {
            fn drop(&mut self) {
                self.inflight
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&self.id);
                // Wake any waiters (they re-check the cache).
                let _ = self.tx.send(true);
            }
        }
        let _guard = InflightGuard {
            inflight: &self.inflight,
            id: session_id.to_string(),
            tx,
        };

        self.compute(session_id).await;
    }

    /// The two-stage compute: resolve the worktree on the async thread, then run
    /// `git status` in `spawn_blocking`. Stores the result and, on a detected
    /// change (or recovery from an error), bumps `rev` and emits `session.changes`.
    async fn compute(self: &Arc<Self>, session_id: &str) {
        self.compute_count.fetch_add(1, Ordering::SeqCst);

        // Stamp this compute with the invalidation generation observed at its START
        // (before any filesystem read), so a result based on the pre-mutation tree
        // is recognizably older than a generation a waiting `invalidate()` requires.
        let generation = self.invalidation_gen.load(Ordering::SeqCst);

        // Stage 1: resolve the worktree. An unknown session (no worktree) is NOT a
        // git error: storing one would recreate a phantom SQLite rev row every
        // poll tick, strand a permanent All-scoped warning, and make GETs return
        // 409 instead of 404. So leave no cache entry at all — `get` distinguishes
        // "no entry, session gone" (404) from a real git error (409).
        let worktree = match self.engine.session_worktree(session_id.to_string()).await {
            Some(w) => PathBuf::from(w),
            None => return,
        };

        // Stage 2: the git work, off the reactor.
        let wt = worktree.clone();
        let computed = tokio::task::spawn_blocking(move || dux_core::git::changed_files(&wt)).await;

        match computed {
            Ok(Ok((staged_raw, unstaged_raw))) => {
                let staged = sorted_views(&staged_raw);
                let unstaged = sorted_views(&unstaged_raw);
                self.store_ok_and_maybe_emit(session_id, generation, staged, unstaged)
                    .await;
            }
            Ok(Err(e)) => {
                let rev = self.engine.next_changes_rev(session_id.to_string()).await;
                let message = format!("{e:#}");
                dux_core::logger::error(&format!(
                    "changed-files compute failed for session {session_id}: {message}"
                ));
                // A genuine git failure must replace a now-stale success (`clobber_ok`).
                self.store_err(session_id, rev, generation, message, true);
            }
            Err(join_err) => {
                let rev = self.engine.next_changes_rev(session_id.to_string()).await;
                let message = format!("changed-files git task failed: {join_err}");
                dux_core::logger::error(&format!(
                    "changed-files compute task failed for session {session_id}: {join_err}"
                ));
                self.store_err(session_id, rev, generation, message, true);
            }
        }
    }

    /// Store a successful compute and emit `session.changes` if the lists changed
    /// (or the session is recovering from a cached error). Stores conditionally:
    /// a slow compute never overwrites a newer entry (keep the higher `rev`).
    async fn store_ok_and_maybe_emit(
        self: &Arc<Self>,
        session_id: &str,
        generation: u64,
        staged: Vec<ChangedFileView>,
        unstaged: Vec<ChangedFileView>,
    ) {
        let next = (staged, unstaged);

        // Decide whether anything changed, under the lock.
        let changed = {
            let cache = lock(&self.cache);
            match cache.get(session_id) {
                Some(Cached::Ok { prev, .. }) => prev != &next,
                // Recovering from a cached error always counts as a change.
                Some(Cached::Err { .. }) => true,
                None => true,
            }
        };

        // A clean success resets the error streak and (only if a warning was shown)
        // emits the keyed recovery info.
        self.reset_error_streak(session_id);

        if !changed {
            return;
        }

        let rev = self.engine.next_changes_rev(session_id.to_string()).await;
        // A rev of 0 is the engine-gone fallback (logged in `next_changes_rev`).
        // Emitting `session.changes` with it would only trigger redundant client
        // refetches, so skip both the store and the emit.
        if rev == 0 {
            return;
        }

        // Store conditionally: re-read under the lock and keep the higher rev so a
        // slow compute cannot clobber a newer one that landed while we awaited.
        // Only emit when we actually stored, so a slow compute that lost the race
        // does not signal a `session.changes` the newer compute already announced.
        let stored = {
            let mut cache = lock(&self.cache);
            let should_store = match cache.get(session_id) {
                Some(Cached::Ok { rev: existing, .. }) => rev >= *existing,
                Some(Cached::Err { rev: existing, .. }) => rev >= *existing,
                None => true,
            };
            if should_store {
                cache.insert(
                    session_id.to_string(),
                    Cached::Ok {
                        rev,
                        generation,
                        prev: next,
                    },
                );
            }
            should_store
        };

        if stored {
            self.bus.emit(Event::Resource {
                event: "session.changes".to_string(),
                id: Some(session_id.to_string()),
                rev: Some(rev),
                owner: None,
                epoch: None,
            });
        }
    }

    /// Store a cached error and escalate to a keyed `Warning` after a streak.
    ///
    /// `clobber_ok` controls whether this error may overwrite an existing
    /// `Cached::Ok`. The real-failure paths in [`Self::compute`] pass `true`: git
    /// genuinely failed, so a now-stale success must yield to the error. The
    /// poll-tick timeout path passes `false`: it runs after its compute was
    /// cancelled and races a freshly-elected owner whose successful `Cached::Ok`
    /// (stored with a LOWER rev than this later-minted timeout rev) must not be
    /// clobbered by a giving-up timeout — that would surface a spurious 409. A
    /// timeout that thus declines to overwrite a concurrent success is not a real
    /// error for this session and does not escalate the warning streak.
    fn store_err(
        self: &Arc<Self>,
        session_id: &str,
        rev: u64,
        generation: u64,
        message: String,
        clobber_ok: bool,
    ) {
        let stored = {
            let mut cache = lock(&self.cache);
            // Keep the higher rev (a stale error must not clobber a newer entry),
            // and — unless `clobber_ok` — never overwrite a concurrent success.
            let should_store = match cache.get(session_id) {
                Some(Cached::Ok { rev: existing, .. }) => clobber_ok && rev >= *existing,
                Some(Cached::Err { rev: existing, .. }) => rev >= *existing,
                None => true,
            };
            if should_store {
                cache.insert(
                    session_id.to_string(),
                    Cached::Err {
                        rev,
                        generation,
                        at: Instant::now(),
                        message: message.clone(),
                    },
                );
            }
            should_store
        };

        // A timeout that declined to clobber a concurrent success recorded nothing
        // and must not escalate the streak; a real error always does (its streak
        // tracking is independent of whether a higher-rev entry won the store race).
        if !stored && !clobber_ok {
            return;
        }

        let streak = {
            let mut streaks = lock(&self.error_streak);
            let n = streaks.entry(session_id.to_string()).or_insert(0);
            *n += 1;
            *n
        };
        if streak == ERROR_WARN_THRESHOLD {
            // Record that a warning was actually shown for this session so the
            // recovery info only fires for sessions that saw a warning.
            lock(&self.warning_emitted).insert(session_id.to_string());
            self.engine.emit_status(WireStatus::keyed(
                warn_key(session_id),
                "warning",
                format!(
                    "Changed files for this agent are temporarily unavailable (git busy): {message}"
                ),
            ));
        }
    }

    /// Reset a session's error streak; emit the keyed recovery info ONLY when a
    /// keyed warning was actually emitted for this session (the streak had reached
    /// the threshold). A 1-2 error blip that never showed a warning therefore does
    /// not leave an orphaned "available again" toast.
    fn reset_error_streak(self: &Arc<Self>, session_id: &str) {
        lock(&self.error_streak).remove(session_id);
        let had_warning = lock(&self.warning_emitted).remove(session_id);
        if had_warning {
            // Replace the warning with a success on the same key so the toast
            // resolves and auto-clears instead of lingering.
            self.engine.emit_status(WireStatus::keyed(
                warn_key(session_id),
                "info",
                "Changed files are available again.".to_string(),
            ));
        }
    }

    /// Total git computes run (test instrumentation for the single-flight test).
    #[cfg(test)]
    pub fn compute_count(&self) -> usize {
        self.compute_count.load(Ordering::SeqCst)
    }
}

/// The keyed-status key for a session's changed-files error escalation.
fn warn_key(session_id: &str) -> String {
    format!("changes-error:{session_id}")
}

/// Project a `ChangedFile` to its wire view (the `from_file` projection in
/// dux-core is private, but every field of [`ChangedFileView`] is public).
fn view_from(f: &ChangedFile) -> ChangedFileView {
    ChangedFileView {
        status: f.status.clone(),
        path: f.path.clone(),
        additions: f.additions,
        deletions: f.deletions,
        binary: f.binary,
    }
}

/// Project and sort a changed-files list by `(path, status)` so change detection
/// is stable regardless of git's output order.
fn sorted_views(files: &[ChangedFile]) -> Vec<ChangedFileView> {
    let mut views: Vec<ChangedFileView> = files.iter().map(view_from).collect();
    views.sort_by(|a, b| {
        (a.path.as_str(), a.status.as_str()).cmp(&(b.path.as_str(), b.status.as_str()))
    });
    views
}

#[cfg(test)]
mod tests {
    use super::*;
    use dux_core::config::{DuxPaths, ProjectConfig};
    use dux_core::storage::SessionStore;

    fn now() -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now()
    }

    fn sample_session(id: &str, worktree: &str) -> dux_core::model::AgentSession {
        let n = now();
        dux_core::model::AgentSession {
            id: id.to_string(),
            project_id: "p1".to_string(),
            project_path: None,
            provider: dux_core::model::ProviderKind::new("claude"),
            source_branch: "main".to_string(),
            branch_name: "feat".to_string(),
            worktree_path: worktree.to_string(),
            title: None,
            started_providers: Vec::new(),
            desired_running: true,
            auto_reopen_enabled: false,
            status: dux_core::model::SessionStatus::Detached,
            created_at: n,
            updated_at: n,
        }
    }

    fn run_git(cwd: &std::path::Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .status()
            .expect("spawn git")
            .success();
        assert!(ok, "git {args:?} failed");
    }

    /// Build an engine handle whose session `s1` points at a real git repo with an
    /// uncommitted edit, plus the `EventBus`. Returns the worktree root too.
    fn boot() -> (
        EngineHandle,
        Arc<EventBus>,
        tempfile::TempDir,
        std::path::PathBuf,
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        // The git repo lives in its OWN subdir, separate from the dux runtime files
        // (sessions.sqlite3 + WAL, config.toml, dux.lock) at `root`, so those never
        // show up as untracked changes and make `changed_files` nondeterministic.
        let wt = root.join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        run_git(&wt, &["init", "-q"]);
        run_git(&wt, &["config", "user.email", "t@example.com"]);
        run_git(&wt, &["config", "user.name", "t"]);
        std::fs::write(wt.join("f.txt"), "line1\nline2\n").unwrap();
        run_git(&wt, &["add", "f.txt"]);
        run_git(&wt, &["commit", "-q", "-m", "init"]);
        // Uncommitted edit so there is an unstaged change.
        std::fs::write(wt.join("f.txt"), "line1\nCHANGED\n").unwrap();

        // A second worktree dir that is NOT a git repo, so its `changed_files`
        // compute fails — this deterministically exercises the cached-error path
        // (an unknown session no longer caches an error; it 404s).
        let wt_err = root.join("wt_err");
        std::fs::create_dir_all(&wt_err).unwrap();

        let paths = DuxPaths {
            root: root.clone(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
        };
        std::fs::create_dir_all(&paths.worktrees_root).unwrap();
        {
            let store = SessionStore::open(&paths.sessions_db_path).unwrap();
            store
                .upsert_project(&ProjectConfig {
                    id: "p1".to_string(),
                    path: root.to_string_lossy().into_owned(),
                    name: Some("p1".to_string()),
                    default_provider: None,
                    leading_branch: None,
                    auto_reopen_agents: None,
                    startup_command: None,
                    env: Default::default(),
                })
                .unwrap();
            store
                .upsert_session(&sample_session("s1", wt.to_string_lossy().as_ref()))
                .unwrap();
            store
                .upsert_session(&sample_session("s_err", wt_err.to_string_lossy().as_ref()))
                .unwrap();
        }
        let engine = crate::bootstrap::bootstrap_engine(&paths).unwrap();
        let (handle, _join) = crate::engine_actor::spawn_engine_thread(engine);
        (handle, Arc::new(EventBus::new()), tmp, wt)
    }

    #[tokio::test]
    async fn get_returns_unstaged_change_and_increments_rev() {
        let (engine, bus, _tmp, _root) = boot();
        let svc = ChangesService::new(engine, bus);

        let resp = svc
            .get("s1")
            .await
            .unwrap_or_else(|_| panic!("expected Ok"));
        assert!(
            resp.unstaged.iter().any(|f| f.path == "f.txt"),
            "expected f.txt unstaged change"
        );
        assert!(
            resp.rev >= 1,
            "rev should advance on the first detected change"
        );

        // A second get with no underlying change serves the cache: same rev, no
        // new compute beyond the cached read.
        let again = svc
            .get("s1")
            .await
            .unwrap_or_else(|_| panic!("expected Ok"));
        assert_eq!(again.rev, resp.rev, "rev must not advance without a change");
    }

    #[tokio::test]
    async fn unknown_session_is_session_not_found() {
        let (engine, bus, _tmp, _root) = boot();
        let svc = ChangesService::new(engine, bus);
        // An unknown session caches NOTHING (no phantom rev row, no warning); the
        // service reports it as gone so the route maps it to 404 (not 409).
        match svc.get("nope").await {
            Err(GitError::SessionNotFound) => {}
            _ => panic!("expected SessionNotFound for an unknown session"),
        }
    }

    #[tokio::test]
    async fn real_session_git_error_is_git_error() {
        let (engine, bus, _tmp, _root) = boot();
        let svc = ChangesService::new(engine, bus);
        // A real session whose worktree is not a git repo yields a git error (409),
        // distinct from the unknown-session 404 above.
        match svc.get("s_err").await {
            Err(GitError::Git(_)) => {}
            _ => panic!("expected a git error for a non-repo worktree"),
        }
    }

    #[tokio::test]
    async fn single_flight_collapses_concurrent_gets_to_one_compute() {
        let (engine, bus, _tmp, _root) = boot();
        let svc = ChangesService::new(engine, bus);

        // Fire many concurrent GETs on the cold session.
        let mut tasks = Vec::new();
        for _ in 0..16 {
            let svc = Arc::clone(&svc);
            tasks.push(tokio::spawn(async move { svc.get("s1").await.is_ok() }));
        }
        for t in tasks {
            assert!(t.await.unwrap(), "every concurrent GET should succeed");
        }
        assert_eq!(
            svc.compute_count(),
            1,
            "concurrent GETs on a cold session must collapse to exactly one git compute"
        );
    }

    #[tokio::test]
    async fn change_in_only_stat_emits_with_prev_retained() {
        let (engine, bus, _tmp, root) = boot();
        let svc = ChangesService::new(engine, Arc::clone(&bus));

        // Prime the cache (stores prev so the next compute can COMPARE rather than
        // unconditionally emit). f.txt is an `M` unstaged change with one edited line.
        let first = svc.get("s1").await.unwrap_or_else(|_| panic!("ok"));
        // Subscribe AFTER priming so we only observe the recompute's emit, not the
        // priming get's first-compute emit.
        let mut events = bus.subscribe();

        // Add a line: same path, same `M` status, different additions stat. The
        // compare includes additions/deletions, so a recompute (keeping prev) emits.
        std::fs::write(root.join("f.txt"), "line1\nCHANGED\nEXTRA\n").unwrap();
        svc.compute_cached("s1".to_string()).await;

        let ev = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("event timeout")
            .expect("event");
        let Event::Resource { event, id, rev, .. } = ev;
        assert_eq!(event, "session.changes");
        assert_eq!(id.as_deref(), Some("s1"));
        assert!(
            rev.unwrap() > first.rev,
            "rev must advance on a stat-only change (lists are compared incl. stats)"
        );
    }

    #[tokio::test]
    async fn identical_lists_do_not_emit() {
        let (engine, bus, _tmp, _root) = boot();
        let svc = ChangesService::new(engine, Arc::clone(&bus));

        // Prime, then recompute with no underlying change: no event, same rev.
        let first = svc.get("s1").await.unwrap_or_else(|_| panic!("ok"));
        // Subscribe after priming so the priming emit is not mistaken for a recompute.
        let mut events = bus.subscribe();
        svc.compute_cached("s1".to_string()).await;
        let again = svc.get("s1").await.unwrap_or_else(|_| panic!("ok"));
        assert_eq!(again.rev, first.rev, "rev must not advance with no change");
        assert!(
            tokio::time::timeout(Duration::from_millis(400), events.recv())
                .await
                .is_err(),
            "an unchanged recompute must not emit session.changes"
        );
    }

    #[tokio::test]
    async fn timeout_error_does_not_clobber_a_fresh_ok() {
        let (engine, bus, _tmp, _root) = boot();
        let svc = ChangesService::new(engine, bus);

        // Seed a successful entry (as if a concurrent compute just stored it).
        {
            lock(&svc.cache).insert(
                "s1".to_string(),
                Cached::Ok {
                    rev: 5,
                    generation: 0,
                    prev: (Vec::new(), Vec::new()),
                },
            );
        }

        // A poll-tick timeout error mints a HIGHER rev (it ran later) but must NOT
        // overwrite the concurrent success — `clobber_ok = false`. Otherwise a GET
        // would see a spurious 409 over a perfectly good result.
        svc.store_err("s1", 6, 0, "timed out".to_string(), false);
        match lock(&svc.cache).get("s1") {
            Some(Cached::Ok { rev, .. }) => assert_eq!(*rev, 5, "the fresh Ok must survive"),
            _ => panic!("a timeout error clobbered a concurrent success"),
        }
        // It also must not escalate the error streak (nothing was actually wrong).
        assert!(
            lock(&svc.error_streak).get("s1").copied().unwrap_or(0) == 0,
            "a declined timeout must not bump the error streak"
        );

        // A GENUINE git error (`clobber_ok = true`) with a higher rev DOES replace
        // the stale success, so a real failure still surfaces as 409.
        svc.store_err("s1", 7, 0, "git failed".to_string(), true);
        match lock(&svc.cache).get("s1") {
            Some(Cached::Err { rev, .. }) => assert_eq!(*rev, 7, "a real error must win"),
            _ => panic!("a real git error should replace the stale Ok"),
        }
    }

    #[tokio::test]
    async fn waiter_re_elects_when_entry_predates_required_generation() {
        let (engine, bus, _tmp, _root) = boot();
        let svc = ChangesService::new(engine, bus);

        // Install an inflight slot we control, so the next `compute_cached` parks as
        // a waiter behind this synthetic "owner".
        let (tx, rx) = watch::channel(false);
        {
            lock(&svc.inflight).insert("s1".to_string(), rx);
        }
        // A git mutation bumped the invalidation generation: a compute entering now
        // requires an entry stamped at the new generation.
        svc.invalidation_gen.fetch_add(1, Ordering::SeqCst);

        let svc_w = Arc::clone(&svc);
        let waiter = tokio::spawn(async move { svc_w.compute_cached("s1".to_string()).await });
        // Let the waiter reach `wait_for` on the inflight receiver.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Act as a STALE owner: store a pre-mutation snapshot (generation 0), clear
        // the inflight slot, then wake the waiter.
        {
            lock(&svc.cache).insert(
                "s1".to_string(),
                Cached::Ok {
                    rev: 0,
                    generation: 0,
                    prev: (Vec::new(), Vec::new()),
                },
            );
            lock(&svc.inflight).remove("s1");
        }
        let _ = tx.send(true);

        waiter.await.unwrap();

        // The waiter saw a generation-0 entry while requiring generation 1, so it
        // re-elected and recomputed (the poller never runs for an uninterested
        // session). The fresh entry is stamped at the required generation.
        assert_eq!(
            svc.compute_count(),
            1,
            "a stale-generation entry must trigger exactly one recompute"
        );
        assert_eq!(
            lock(&svc.cache).get("s1").map(|c| c.generation()),
            Some(1),
            "the recompute must stamp the entry at the required generation"
        );
    }

    #[tokio::test]
    async fn waiter_accepts_entry_at_required_generation() {
        let (engine, bus, _tmp, _root) = boot();
        let svc = ChangesService::new(engine, bus);

        let (tx, rx) = watch::channel(false);
        {
            lock(&svc.inflight).insert("s1".to_string(), rx);
        }
        // No invalidation: the waiter requires generation 0.
        let svc_w = Arc::clone(&svc);
        let waiter = tokio::spawn(async move { svc_w.compute_cached("s1".to_string()).await });
        tokio::time::sleep(Duration::from_millis(50)).await;

        // The owner stores an entry at generation 0 (== required) and wakes the
        // waiter, which must accept it without recomputing.
        {
            lock(&svc.cache).insert(
                "s1".to_string(),
                Cached::Ok {
                    rev: 1,
                    generation: 0,
                    prev: (Vec::new(), Vec::new()),
                },
            );
            lock(&svc.inflight).remove("s1");
        }
        let _ = tx.send(true);
        waiter.await.unwrap();

        assert_eq!(
            svc.compute_count(),
            0,
            "a fresh-enough entry must NOT trigger a recompute"
        );
    }

    #[tokio::test]
    async fn error_is_cached_within_ttl() {
        let (engine, bus, _tmp, _root) = boot();
        let svc = ChangesService::new(engine, bus);

        // A real session whose worktree is not a git repo deterministically caches
        // an error (no git lock flake). An unknown session would NOT cache (404).
        let _ = svc.get("s_err").await;
        let before = svc.compute_count();
        // Repeated GETs during the cached-error window must NOT each spawn git.
        let _ = svc.get("s_err").await;
        let _ = svc.get("s_err").await;
        assert_eq!(
            svc.compute_count(),
            before,
            "GETs during a cached error window must not each spawn git"
        );
    }
}
