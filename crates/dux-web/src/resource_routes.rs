//! `GET /api/v1/resources`: the REST read behind the web Task Manager, plus the
//! [`ResourceService`] that samples CPU/RSS off both the engine thread and the
//! reactor.
//!
//! ## Why REST and not the event bus
//!
//! [`crate::event_bus`] states its contract: an event names what changed, never
//! the changed value. Resource stats ARE a value, and one that changes on every
//! sample, so a ws topic would push a payload every tick to every connected
//! client whether or not anyone is looking. REST gives natural backpressure: no
//! Task Manager open, no cost.
//!
//! ## Why not `Engine::spawn_resource_stats_worker`
//!
//! That worker is shaped for the TUI event loop: fire-and-forget into a
//! `WorkerEvent`, no reply channel, and its `InFlightKey::ResourceStats` guard
//! SILENTLY DROPS a concurrent request. Dropping a redundant refresh is right for
//! a repainting TUI and wrong for REST, where the second browser would simply get
//! nothing back. It is left untouched for the TUI; the web samples through this
//! service instead.
//!
//! ## Single-flight + TTL
//!
//! Modeled on [`crate::changes::ChangesService`]. N browsers polling at 1s
//! collapse to ONE sysinfo walk: a fresh cache entry (younger than [`CACHE_TTL`])
//! is served directly, and concurrent misses elect one owner while the rest await
//! it and re-read the cache. A drop guard clears the inflight slot on every exit
//! path including future cancellation (an HTTP client disconnecting drops the
//! handler future at its `.await`).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::response::IntoResponse;
use axum::{Json, Router, extract::State, http::StatusCode, response::Response, routing::get};
use serde::Serialize;
use tokio::sync::watch;

use dux_core::resource_stats::ResourceCollector;
use dux_core::viewmodel::ResourceStatsView;

use crate::engine_actor::EngineHandle;
use crate::server::AppState;

/// How long a sample is served before a fresh walk is taken. Both the poll
/// cadence (1s) and this bound are deliberate: several browsers polling in step
/// still cost one process walk per second, and the collector's CPU delta then
/// spans that same natural interval.
const CACHE_TTL: Duration = Duration::from_secs(1);

/// Lock a `Mutex` poison-tolerantly. These are plain caches whose invariants are
/// re-established by the next sample, so recovering the guard beats propagating
/// a panic into every later request.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The 200 body: `{ "rows": [...] }`.
#[derive(Serialize)]
struct ResourcesResponseBody {
    rows: Vec<ResourceStatsView>,
}

struct Cached {
    at: Instant,
    rows: Vec<ResourceStatsView>,
}

pub struct ResourceService {
    engine: EngineHandle,
    /// The sampler. Held across samples because sysinfo derives per-process CPU
    /// from the delta between two refreshes; see [`dux_core::resource_stats`].
    /// This is the web's own collector, independent of the Engine's.
    collector: Arc<Mutex<ResourceCollector>>,
    cache: Mutex<Option<Cached>>,
    /// Single-flight slot: `Some` while a walk is in progress. Late callers clone
    /// the receiver and await it rather than starting a second walk.
    inflight: Mutex<Option<watch::Receiver<bool>>>,
    /// Total sysinfo walks run. Test instrumentation for the single-flight and
    /// TTL tests.
    collect_count: AtomicUsize,
}

impl ResourceService {
    pub fn new(engine: EngineHandle) -> Arc<Self> {
        Arc::new(Self {
            engine,
            collector: Arc::new(Mutex::new(ResourceCollector::new())),
            cache: Mutex::new(None),
            inflight: Mutex::new(None),
            collect_count: AtomicUsize::new(0),
        })
    }

    /// Serve the current sample, taking a fresh one under single-flight when the
    /// cache is cold or stale. `None` means the engine actor is gone (the handler
    /// maps that to 503), which is distinct from a real "nothing is running"
    /// result: the dux and total rows are always present in a successful sample.
    pub async fn get(self: &Arc<Self>) -> Option<Vec<ResourceStatsView>> {
        if let Some(rows) = self.read_fresh() {
            return Some(rows);
        }
        self.sample_cached().await;
        self.read_any()
    }

    /// The cached rows if the entry is younger than [`CACHE_TTL`].
    fn read_fresh(&self) -> Option<Vec<ResourceStatsView>> {
        let cache = lock(&self.cache);
        match cache.as_ref() {
            Some(c) if c.at.elapsed() < CACHE_TTL => Some(c.rows.clone()),
            _ => None,
        }
    }

    /// The cached rows regardless of age. Read after a sample: a waiter whose
    /// owner was cancelled, or a sample that failed because the engine is gone,
    /// finds nothing here.
    fn read_any(&self) -> Option<Vec<ResourceStatsView>> {
        lock(&self.cache).as_ref().map(|c| c.rows.clone())
    }

    /// Single-flight wrapper around [`Self::sample`]. Exactly one walk runs at a
    /// time; late callers await the owner and re-read the cache, re-electing a
    /// new owner if the previous one was cancelled before storing.
    async fn sample_cached(self: &Arc<Self>) {
        loop {
            enum Role {
                Owner(watch::Sender<bool>),
                Waiter(watch::Receiver<bool>),
            }
            let role = {
                let mut inflight = lock(&self.inflight);
                match inflight.as_ref() {
                    Some(rx) => Role::Waiter(rx.clone()),
                    None => {
                        let (tx, rx) = watch::channel(false);
                        *inflight = Some(rx);
                        Role::Owner(tx)
                    }
                }
            };
            match role {
                Role::Owner(tx) => {
                    self.run_owned_sample(tx).await;
                    return;
                }
                Role::Waiter(mut rx) => {
                    let _ = rx.wait_for(|done| *done).await;
                    // Accept whatever the owner stored, as long as it is fresh.
                    // An absent/stale entry means the owner was cancelled before
                    // storing, so re-elect rather than serve nothing.
                    if self.read_fresh().is_some() {
                        return;
                    }
                    continue;
                }
            }
        }
    }

    /// Run the walk as the single-flight owner. The drop guard clears the slot and
    /// wakes waiters on EVERY exit path, including cancellation at an `.await`.
    async fn run_owned_sample(self: &Arc<Self>, tx: watch::Sender<bool>) {
        struct InflightGuard<'a> {
            inflight: &'a Mutex<Option<watch::Receiver<bool>>>,
            tx: watch::Sender<bool>,
        }
        impl Drop for InflightGuard<'_> {
            fn drop(&mut self) {
                *self.inflight.lock().unwrap_or_else(|e| e.into_inner()) = None;
                let _ = self.tx.send(true);
            }
        }
        let _guard = InflightGuard {
            inflight: &self.inflight,
            tx,
        };

        self.sample().await;
    }

    /// The two-stage sample: ask the actor for the live targets (map iteration
    /// only), then run the blocking sysinfo walk in `spawn_blocking`. Stores the
    /// projected rows on success; an engine-gone or panicked walk stores nothing.
    async fn sample(self: &Arc<Self>) {
        // Stage 1: the target list, off the engine thread.
        let Some(targets) = self.engine.resource_targets().await else {
            // The actor is gone. Leave the cache alone so `get` reports it.
            return;
        };

        // Stage 2: the sysinfo walk, off the reactor. It is genuinely blocking:
        // a full process-table refresh, and up to `MINIMUM_CPU_UPDATE_INTERVAL`
        // of sleep when the collector has to re-establish its CPU baseline.
        self.collect_count.fetch_add(1, Ordering::SeqCst);
        let collector = Arc::clone(&self.collector);
        let sampled = tokio::task::spawn_blocking(move || lock(&collector).sample(targets)).await;

        match sampled {
            // `was_baseline` (a short-window sample) is a TUI-only concern
            // (the `~` marker in `render_resource_monitor`); the web client
            // has no equivalent indicator, so it is discarded here.
            Ok((rows, _was_baseline)) => {
                let rows = ResourceStatsView::from_stats(rows);
                *lock(&self.cache) = Some(Cached {
                    at: Instant::now(),
                    rows,
                });
            }
            Err(join_err) => {
                dux_core::logger::error(&format!("resource stats sample task failed: {join_err}"));
            }
        }
    }

    /// Total sysinfo walks run (test instrumentation).
    #[cfg(test)]
    pub fn collect_count(&self) -> usize {
        self.collect_count.load(Ordering::SeqCst)
    }
}

/// The resource-monitor read route. Served with no authentication, like every
/// other API route (the single-tenant trusted-access model in CLAUDE.md): any
/// client that can reach the server can read every agent's and terminal's process
/// stats.
pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/resources", get(get_resources))
}

async fn get_resources(State(state): State<AppState>) -> Response {
    match state.resources.get().await {
        Some(rows) => Json(ResourcesResponseBody { rows }).into_response(),
        // Mirrors `workspace_routes::engine_unavailable`.
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            "the engine is unavailable; retry shortly",
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_engine_handle;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    async fn body_json(resp: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn get_resources_returns_dux_and_total_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let router = crate::server::router(test_engine_handle(tmp.path()));

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/resources")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let json = body_json(resp).await;
        let rows = json["rows"].as_array().expect("rows array");
        // No agents or terminals are running, so the sample is exactly the two
        // synthetic rows. An empty Task Manager still reports dux itself.
        let kinds: Vec<&str> = rows.iter().map(|r| r["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, vec!["dux", "total"]);

        let dux = &rows[0];
        assert!(dux["id"].is_null(), "the dux row has no spine id");
        assert!(dux["pid"].as_u64().unwrap() > 0);
        assert!(
            dux["rss_bytes"].as_u64().unwrap() > 0,
            "the dux row must report real memory"
        );
        assert!(rows[1]["id"].is_null(), "the total row has no spine id");
    }

    #[tokio::test]
    async fn get_resources_503_when_engine_gone() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let router = crate::server::router(handle.clone());
        // Stop the actor loop: the handle's round-trip now fails, which must read
        // as "engine unavailable" rather than an empty resource list.
        handle.shutdown().await;

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/resources")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn concurrent_resource_requests_collapse_to_one_collection() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = ResourceService::new(test_engine_handle(tmp.path()));

        // Many browsers hitting a cold cache at once must not each walk the
        // process table.
        let mut tasks = Vec::new();
        for _ in 0..16 {
            let svc = Arc::clone(&svc);
            tasks.push(tokio::spawn(async move { svc.get().await.is_some() }));
        }
        for t in tasks {
            assert!(t.await.unwrap(), "every concurrent GET should be served");
        }
        assert_eq!(
            svc.collect_count(),
            1,
            "concurrent GETs on a cold cache must collapse to exactly one sysinfo walk"
        );
    }

    #[tokio::test]
    async fn resource_cache_serves_within_ttl() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = ResourceService::new(test_engine_handle(tmp.path()));

        assert!(svc.get().await.is_some());
        let after_first = svc.collect_count();
        assert_eq!(after_first, 1);

        // Sequential GETs inside the TTL window are served from the cache.
        assert!(svc.get().await.is_some());
        assert!(svc.get().await.is_some());
        assert_eq!(
            svc.collect_count(),
            after_first,
            "GETs within the TTL must be served from cache, not re-walk the process table"
        );
    }

    #[tokio::test]
    async fn resource_sample_walks_again_once_the_ttl_expires() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = ResourceService::new(test_engine_handle(tmp.path()));

        assert!(svc.get().await.is_some());
        assert_eq!(svc.collect_count(), 1);

        tokio::time::sleep(CACHE_TTL + Duration::from_millis(100)).await;
        assert!(svc.get().await.is_some());
        assert_eq!(
            svc.collect_count(),
            2,
            "a stale cache entry must trigger a fresh walk"
        );
    }
}
