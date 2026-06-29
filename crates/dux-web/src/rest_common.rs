//! Shared helpers for the REST action routes (Phase 4 of the REST-first
//! migration): connection-scoped status, id length-bounding, awaiting an
//! asynchronously-created resource's id, and the create idempotency cache.
//!
//! These live in one place so the session/project action modules and the git
//! mutation routes derive `StatusScope` and bound `:id` params identically.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use dux_core::statusline::StatusScope;

use crate::engine_actor::EngineHandle;

/// The request header carrying the originating `/ws` connection id (handed to the
/// client in the `Connected` first frame). Lower-case to match axum's normalized
/// header names.
pub const CONNECTION_ID_HEADER: &str = "x-connection-id";

/// The optional create-idempotency header (`POST /api/v1/sessions`/`/projects`).
pub const IDEMPOTENCY_KEY_HEADER: &str = "idempotency-key";

/// Upper bound on a `:id` path segment before any lookup (matches the
/// length-bounding convention used across the read routes).
pub const MAX_ID_LEN: usize = 128;

/// How long a created resource's id stays addressable under its `Idempotency-Key`,
/// so a client retry after a lost response returns the SAME resource instead of
/// creating a duplicate. Long enough to cover a retry storm; short enough that a
/// reused key from a later, unrelated request does not collide.
pub const IDEMPOTENCY_TTL: Duration = Duration::from_secs(600);

/// How long a create handler waits for the asynchronously-created resource to
/// surface in the spine before giving up and replying `202 Accepted` (the create
/// was dispatched; its completion/failure still rides the status toast stream).
/// Generous because a real create does `git worktree add` + a provider PTY spawn.
pub const CREATE_AWAIT_TIMEOUT: Duration = Duration::from_secs(20);

/// Longer await window for the from-PR create. The from-PR path does a
/// `gh pr view` network round trip BEFORE the worktree+PTY worker even starts, so
/// the default 20s window routinely expires and yields a bodyless `202` for a
/// create that ultimately succeeds. Sixty seconds covers a slow network lookup
/// plus the worktree/PTY work.
///
/// DEFERRED FOLLOW-UP: the fuller fix is to stop blocking on a long poll at all —
/// reply `202` immediately with a body carrying an `op_key` the client correlates
/// against the status stream, plus an in-progress indicator — so neither the
/// from-PR nor a slow ordinary create depends on a fixed timeout. Tracked as a
/// later REST-migration refinement.
pub const FROM_PR_CREATE_AWAIT_TIMEOUT: Duration = Duration::from_secs(60);

/// The class of a live WebSocket connection tracked in the [`ConnectionRegistry`].
/// Used by the liveness reaper (every class is pingable) and by `scope_from_headers`
/// to require that a scoped `x-connection-id` is a live Events-class connection
/// before routing a REST status toast to it; PTY-class ids are never disclosed to
/// clients.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConnClass {
    /// A `/ws/events` change+status socket.
    Events,
    /// A `/ws/sessions/:id/pty` agent-provider PTY socket.
    AgentPty,
    /// A `/ws/sessions/:id/terminals/:tid/pty` companion-terminal PTY socket.
    TerminalPty,
}

/// Thread-safe map of live connection id → its [`ConnClass`]. Every upgraded
/// WebSocket registers its server-minted id on connect and deregisters on
/// disconnect, so the map is the authoritative set of LIVE connection ids.
///
/// [`scope_from_headers`] validates an inbound `X-Connection-Id` against it: an id
/// not present (or not of class [`ConnClass::Events`]) falls back to broadcast, so a
/// forged, stale, or PTY-class id cannot silence a toast by routing it to a
/// non-events connection.
///
/// A plain `Mutex<HashMap<..>>`: every operation takes the lock, does an O(1) map
/// op, and releases it WITHOUT awaiting, so the guard never crosses an `.await`.
#[derive(Default)]
pub struct ConnectionRegistry {
    entries: Mutex<HashMap<String, ConnClass>>,
}

impl ConnectionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a live connection id with its class (called on socket upgrade).
    pub fn insert(&self, id: String, class: ConnClass) {
        self.entries.lock().unwrap().insert(id, class);
    }

    /// Deregister a connection id (called on socket disconnect), freeing its slot.
    pub fn remove(&self, id: &str) {
        self.entries.lock().unwrap().remove(id);
    }

    /// Whether `id` is a currently-live connection.
    #[cfg(test)]
    pub fn contains(&self, id: &str) -> bool {
        self.entries.lock().unwrap().contains_key(id)
    }

    /// The [`ConnClass`] of a currently-live connection, or `None` if absent.
    /// Used by [`scope_from_headers`] to require that a scoped header carries
    /// a live EVENTS-class id rather than any registered id.
    pub fn class_of(&self, id: &str) -> Option<ConnClass> {
        self.entries.lock().unwrap().get(id).copied()
    }
}

/// Derive the [`StatusScope`] for a REST action from the optional
/// `X-Connection-Id` header: present, non-empty, AND still a live connection in
/// `registry` → scope the operation's status toasts to that connection (matching
/// the WS command path); otherwise broadcast to all clients (`All`). The header is
/// OPTIONAL.
///
/// Validating the id against the live-connection registry is the notification-tag
/// guard: an absent, blank, forged, or stale id falls back to `All` rather than
/// being trusted blindly. Routing a status to a connection that does not exist would
/// silence the toast for everyone (nobody is listening on that scope), so the safe
/// fallback is to broadcast. The connection id is an unguessable server UUID, so
/// this is defense in depth for the single-tenant, trusted-access model, not a
/// per-user boundary.
///
/// The absent/unknown → `All` fallback also covers two legitimate windows where the
/// client has no live id to stamp: (1) before the `/ws` `Connected` frame has
/// delivered the first id on a fresh load, and (2) the reconnect window after a
/// socket drop, where the client has cleared the now-dead id (see the web
/// `connection.ts`/`socket.onConn`). Broadcasting in those windows is the safe
/// default for this single-tenant tool: the initiating client still sees its toast
/// (it shares the one workspace), and there is no per-user scoping to leak across.
pub fn scope_from_headers(headers: &HeaderMap, registry: &ConnectionRegistry) -> StatusScope {
    headers
        .get(CONNECTION_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|id| !id.is_empty())
        // Bound length (chars, not bytes) before any registry lookup.
        .filter(|id| id.chars().count() <= MAX_ID_LEN)
        // Only EVENTS-class connections are ever disclosed to clients; a PTY-class
        // id in the header must not scope-and-silence toasts.
        .filter(|id| registry.class_of(id) == Some(ConnClass::Events))
        .map(|id| StatusScope::Connection(id.to_string()))
        .unwrap_or(StatusScope::All)
}

/// 404 response for an unknown or over-length session id. Shared across the REST
/// route modules (`git_routes`, `file_routes`) so the body text and status code
/// stay in one place. The over-length guard calls this BEFORE the engine lookup
/// so an outsized `:id` never reaches the actor.
pub fn unknown_session() -> Response {
    (StatusCode::NOT_FOUND, "unknown session").into_response()
}

/// Whether `provider` is in the engine's configured provider list (the same source
/// as the bootstrap document's `available_providers`). Returns `None` when the
/// engine is unavailable, so a caller can distinguish "not configured" (`Some(false)`)
/// from "can't tell right now" (`None`).
///
/// Used by the session/project PATCH handlers to reject a bad provider UP FRONT,
/// before any sub-command is dispatched. A PATCH applies its fields as independent
/// wire sub-commands with no rollback, so validating the provider (the only field
/// the engine cross-checks against config) before the rename/auto-reopen/etc.
/// sub-commands run keeps a forged or stale provider from partially applying after
/// an earlier field already committed.
pub async fn provider_is_configured(engine: &EngineHandle, provider: &str) -> Option<bool> {
    engine
        .bootstrap()
        .await
        .map(|b| b.available_providers.iter().any(|p| p == provider))
}

/// Read the optional `Idempotency-Key` request header, trimmed and non-empty.
pub fn idempotency_key(headers: &HeaderMap) -> Option<String> {
    headers
        .get(IDEMPOTENCY_KEY_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(|k| k.to_string())
}

/// Whether a `:id` path segment is within the length bound. Counts characters, not
/// bytes, so a multi-byte id is not rejected early by its UTF-8 length.
pub fn id_within_bound(id: &str) -> bool {
    id.chars().count() <= MAX_ID_LEN
}

/// Poll the engine until create op `op_id` resolves to its session id, or the
/// timeout elapses. This is the RACE-FREE create-correlation path: the op id comes
/// back in `WireCommandOutcome.created_op_id` for a synchronous create
/// (`new`/`fork`/`from_worktree`), and the engine records `op_id -> session_id`
/// when the worker-minted session lands, so the handler resolves ITS exact
/// session — never a concurrent create's. Returns `None` on timeout (the create
/// was dispatched but has not completed yet; its completion/failure still rides
/// the status stream).
pub async fn await_session_for_op(
    engine: &EngineHandle,
    op_id: String,
    timeout: Duration,
) -> Option<String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(id) = engine.created_session_for_op(op_id.clone()).await {
            return Some(id);
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Poll the engine's session spine until a session id appears that was not in
/// `pre`, or the timeout elapses. The FALLBACK create-correlation path, used only
/// by the from-PR create (whose create op is minted later, inside the PR-lookup
/// followup, so its id is not in the synchronous outcome). The synchronous
/// `new`/`fork`/`from_worktree` creates instead use [`await_session_for_op`].
///
/// RESIDUAL RACE: this returns the FIRST session not in `pre`, which under truly
/// concurrent creates (another tab, or a TUI create in flip mode) could be a
/// DIFFERENT request's session. The engine serializes the create worker via the
/// `CreateAgent` in-flight guard, which narrows but does not fully close the
/// window. The from-PR path is the only remaining caller and is comparatively
/// rare, so the residual race is accepted here; the op-id path above is race-free
/// and is preferred wherever the op id is available synchronously.
pub async fn await_new_session(
    engine: &EngineHandle,
    pre: &std::collections::HashSet<String>,
    timeout: Duration,
) -> Option<String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(spine) = engine.spine().await
            && let Some(found) = spine.sessions.iter().find(|s| !pre.contains(&s.id))
        {
            return Some(found.id.clone());
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Like [`await_new_session`] but for projects (the `POST /api/v1/projects` add).
/// A direct add resolves synchronously so the first poll usually wins; the
/// checkout-default add goes through a worker, so the poll covers it.
pub async fn await_new_project(
    engine: &EngineHandle,
    pre: &std::collections::HashSet<String>,
    timeout: Duration,
) -> Option<String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(spine) = engine.spine().await
            && let Some(found) = spine.projects.iter().find(|p| !pre.contains(&p.id))
        {
            return Some(found.id.clone());
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Records `Idempotency-Key -> created resource id` for a TTL so a retried create
/// returns the same resource instead of creating a duplicate. Cheap `Arc`-cloned
/// into [`crate::server::AppState`]; entries past the TTL are pruned lazily on read.
#[derive(Default)]
pub struct IdempotencyCache {
    entries: Mutex<HashMap<String, (String, Instant)>>,
}

impl IdempotencyCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// The recorded resource id for `key` if present and still within the TTL.
    /// Prunes expired entries while holding the lock so the map cannot grow
    /// unbounded from one-shot keys.
    pub fn get(&self, key: &str) -> Option<String> {
        let now = Instant::now();
        let mut map = self.entries.lock().unwrap();
        map.retain(|_, (_, at)| now.saturating_duration_since(*at) < IDEMPOTENCY_TTL);
        map.get(key).map(|(id, _)| id.clone())
    }

    /// Record `key -> id` (stamped now). A second create with the same key within
    /// the TTL replays this id.
    pub fn record(&self, key: String, id: String) {
        self.entries
            .lock()
            .unwrap()
            .insert(key, (id, Instant::now()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_map_with(name: &'static str, value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(name, value.parse().unwrap());
        h
    }

    #[test]
    fn scope_from_headers_maps_connection_id() {
        let reg = ConnectionRegistry::default();
        reg.insert("conn-7".into(), ConnClass::Events);
        let mut h = HeaderMap::new();
        assert_eq!(scope_from_headers(&h, &reg), StatusScope::All);
        h.insert(CONNECTION_ID_HEADER, "  ".parse().unwrap());
        assert_eq!(
            scope_from_headers(&h, &reg),
            StatusScope::All,
            "blank → All"
        );
        h.insert(CONNECTION_ID_HEADER, "conn-7".parse().unwrap());
        assert_eq!(
            scope_from_headers(&h, &reg),
            StatusScope::Connection("conn-7".to_string())
        );
    }

    #[test]
    fn unknown_connection_id_falls_back_to_all_scope() {
        let reg = ConnectionRegistry::default();
        let headers = header_map_with(CONNECTION_ID_HEADER, "does-not-exist");
        assert!(matches!(
            scope_from_headers(&headers, &reg),
            StatusScope::All
        ));
    }

    #[test]
    fn live_connection_id_scopes_to_that_connection() {
        let reg = ConnectionRegistry::default();
        reg.insert("conn-1".into(), ConnClass::Events);
        let headers = header_map_with(CONNECTION_ID_HEADER, "conn-1");
        assert!(
            matches!(scope_from_headers(&headers, &reg), StatusScope::Connection(id) if id == "conn-1")
        );
    }

    /// An Events-class id in the header resolves to a Connection scope.
    #[test]
    fn events_class_id_scopes_to_connection() {
        let reg = ConnectionRegistry::default();
        reg.insert("ev-1".into(), ConnClass::Events);
        let headers = header_map_with(CONNECTION_ID_HEADER, "ev-1");
        assert!(
            matches!(scope_from_headers(&headers, &reg), StatusScope::Connection(id) if id == "ev-1"),
            "events-class id must produce a Connection scope"
        );
    }

    /// A PTY-class id (AgentPty or TerminalPty) must fall back to All — PTY
    /// connection ids are never disclosed to clients and must not scope toasts.
    #[test]
    fn pty_class_id_falls_back_to_all_scope() {
        let reg = ConnectionRegistry::default();
        reg.insert("pty-1".into(), ConnClass::AgentPty);
        reg.insert("pty-2".into(), ConnClass::TerminalPty);

        let h1 = header_map_with(CONNECTION_ID_HEADER, "pty-1");
        assert_eq!(
            scope_from_headers(&h1, &reg),
            StatusScope::All,
            "AgentPty id must fall back to All"
        );
        let h2 = header_map_with(CONNECTION_ID_HEADER, "pty-2");
        assert_eq!(
            scope_from_headers(&h2, &reg),
            StatusScope::All,
            "TerminalPty id must fall back to All"
        );
    }

    /// An id longer than MAX_ID_LEN characters must fall back to All without a
    /// registry lookup.
    #[test]
    fn over_long_id_falls_back_to_all_scope() {
        let reg = ConnectionRegistry::default();
        let long_id = "x".repeat(MAX_ID_LEN + 1);
        // Even if registered, the length guard rejects it first.
        reg.insert(long_id.clone(), ConnClass::Events);
        let headers = header_map_with(CONNECTION_ID_HEADER, &long_id);
        assert_eq!(
            scope_from_headers(&headers, &reg),
            StatusScope::All,
            "an id exceeding MAX_ID_LEN must fall back to All"
        );
    }

    #[test]
    fn registry_insert_remove_contains() {
        let reg = ConnectionRegistry::default();
        assert!(!reg.contains("a"));
        assert_eq!(reg.class_of("a"), None);

        reg.insert("a".into(), ConnClass::Events);
        reg.insert("b".into(), ConnClass::AgentPty);
        reg.insert("c".into(), ConnClass::AgentPty);
        assert!(reg.contains("a"));
        assert!(reg.contains("b"));
        assert_eq!(reg.class_of("a"), Some(ConnClass::Events));
        assert_eq!(reg.class_of("b"), Some(ConnClass::AgentPty));
        assert_eq!(reg.class_of("z"), None);

        reg.remove("a");
        assert!(!reg.contains("a"));
        assert_eq!(reg.class_of("a"), None);
        // Removing a never-registered id is a harmless no-op.
        reg.remove("missing");
        assert!(reg.contains("b"));
        assert!(reg.contains("c"));
    }

    #[test]
    fn idempotency_key_is_trimmed_and_nonempty() {
        let mut h = HeaderMap::new();
        assert_eq!(idempotency_key(&h), None);
        h.insert(IDEMPOTENCY_KEY_HEADER, "   ".parse().unwrap());
        assert_eq!(idempotency_key(&h), None);
        h.insert(IDEMPOTENCY_KEY_HEADER, " k1 ".parse().unwrap());
        assert_eq!(idempotency_key(&h), Some("k1".to_string()));
    }

    #[test]
    fn id_bound_counts_chars() {
        assert!(id_within_bound("ok"));
        assert!(id_within_bound(&"x".repeat(MAX_ID_LEN)));
        assert!(!id_within_bound(&"x".repeat(MAX_ID_LEN + 1)));
        // Multi-byte chars count as one each, not by UTF-8 byte length.
        assert!(id_within_bound(&"é".repeat(MAX_ID_LEN)));
    }

    #[test]
    fn idempotency_cache_replays_then_records() {
        let cache = IdempotencyCache::new();
        assert_eq!(cache.get("k"), None);
        cache.record("k".to_string(), "s_1".to_string());
        assert_eq!(cache.get("k"), Some("s_1".to_string()));
        assert_eq!(cache.get("other"), None);
    }
}
