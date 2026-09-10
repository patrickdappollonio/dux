//! Shared helpers for the REST action routes: connection-scoped status, id length-bounding, awaiting an
//! asynchronously-created resource's id, and the create idempotency cache.
//!
//! These live in one place so the session/project action modules and the git
//! mutation routes derive `StatusScope` and bound `:id` params identically.
//!
//! A create answers one of three shapes. `201 Created` with the record and a
//! `Location` header when it surfaces inside the await window. `422
//! Unprocessable Entity` with the failure's own words when the operation it
//! dispatched finals with an error first: the request was well formed and
//! dispatched, and the work it asked for could not be done, which is neither the
//! `400` a synchronous guard answers nor the `409` an in-flight guard does.
//! Otherwise `202 Accepted` carrying [`Accepted`], the operation id the eventual
//! final arrives under on the events socket. The deferred reply is never
//! bodyless: a client that stopped being waited on still gets the one thing it
//! needs to correlate the outcome.
//!
//! The await helpers watch the dispatched operation alongside the resource, so a
//! create that fails is answered when it fails rather than at the end of the
//! window.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use dux_core::statusline::StatusScope;
use dux_core::wire::WireCommandOutcome;

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
/// surface in the spine before giving up and replying `202 Accepted` with the
/// operation id. Only a create that is STILL RUNNING reaches the end of this
/// window: one whose operation fails is answered `422` the moment its error
/// final lands. What is left to correlate on the status stream is the eventual
/// success, under that same id.
/// Generous because a real create does `git worktree add` + a provider PTY spawn.
pub const CREATE_AWAIT_TIMEOUT: Duration = Duration::from_secs(20);

/// Longer await window for the from-PR create, which does a `gh pr view` network
/// round trip before the worktree and PTY worker even starts: the ordinary window
/// expires on it and yields the deferred `202` for a create that succeeds. This
/// one covers a slow network lookup plus the worktree and PTY work.
pub const FROM_PR_CREATE_AWAIT_TIMEOUT: Duration = Duration::from_secs(60);

/// The `202 Accepted` body of an operation still running when the handler stops
/// waiting: the keyed status op id, the same key that rides the `status` and
/// `status_cleared` frames on `/ws/events`, so a client correlates what happens
/// next instead of polling for the record.
///
/// A 202 means STILL RUNNING and nothing else. An operation that has already
/// failed is answered `422` with its own message, so this body never stands in
/// for a reason the server already had.
///
/// `op_id` is `null` whenever the dispatch's status carries no key, because there
/// is then nothing to correlate on and the reply says so rather than inventing an
/// id a client would wait forever on. The plain project add is the known case: it
/// resolves on the reactor and mints no keyed op at all.
///
/// WHAT THE ID NAMES DIFFERS BY PATH, and the from-PR create is the one where it
/// does not name the create. A session create on the race-free path names its own
/// create op, and the create's success final arrives under it. A from-PR create
/// names the PR-LOOKUP op, because the create's op is minted later, inside the
/// lookup followup; a lookup that fails never reaches this body at all, since it
/// is answered `422`. A lookup that SUCCEEDS hands off, resolving the named id to
/// a `status_cleared` and nothing more, and the create's own outcome then arrives
/// under an id this reply never named. So a from-PR client can learn that it has
/// stopped being the operation to watch, but not the create's verdict; the
/// hand-off is the engine's design and this type only reports it honestly.
#[derive(serde::Serialize)]
pub struct Accepted {
    pub op_id: Option<String>,
}

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

/// Thread-safe map of live connection id to its [`ConnClass`]. Every upgraded
/// WebSocket registers its server-minted id on connect and deregisters on
/// disconnect, so this is the authoritative set of live connection ids.
///
/// [`scope_from_headers`] validates an inbound `X-Connection-Id` against it: an id
/// that is absent or not [`ConnClass::Events`] falls back to broadcast, so a forged,
/// stale or PTY-class id cannot silence a toast. A plain `Mutex<HashMap<..>>`, and
/// every operation releases the lock without awaiting, so no guard crosses an
/// `.await`.
#[derive(Default)]
pub struct ConnectionRegistry {
    entries: Mutex<HashMap<String, ConnClass>>,
    /// How many [`ConnClass::Events`] connections are live, maintained on every
    /// insert and remove so it can be read without the lock and from a crate that
    /// cannot see this type: the terminal UI's serving chip counts connected
    /// browsers and `dux-tui` never sees `dux-web`. Every registry has one, so the
    /// count is never a special case.
    events_live: Arc<AtomicUsize>,
}

impl ConnectionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// A registry that reports its live Events count into `gauge`, so a reader
    /// outside this crate can see it. One registry per gauge: two sharing one would
    /// each subtract from a total neither owns, and the reset below would zero a
    /// count the other still has entries for, after which its next disconnect wraps
    /// the `usize`.
    pub fn with_events_gauge(gauge: Arc<AtomicUsize>) -> Self {
        debug_assert_eq!(
            gauge.load(Ordering::Relaxed),
            0,
            "a connection registry's events gauge must be its own: this one is already counting \
             somebody else's connections"
        );
        gauge.store(0, Ordering::Relaxed);
        Self {
            entries: Mutex::new(HashMap::new()),
            events_live: gauge,
        }
    }

    /// Register a live connection id with its class (called on socket upgrade).
    pub fn insert(&self, id: String, class: ConnClass) {
        let mut entries = self.entries.lock().unwrap();
        let previous = entries.insert(id, class);
        self.adjust_events_count(previous, Some(class));
    }

    /// Deregister a connection id (called on socket disconnect), freeing its slot.
    pub fn remove(&self, id: &str) {
        let mut entries = self.entries.lock().unwrap();
        let previous = entries.remove(id);
        self.adjust_events_count(previous, None);
    }

    /// How many Events connections are live: one per open browser tab. Events
    /// sockets only, because a tab watching a terminal has a PTY socket open beside
    /// its Events one. Called with the entries lock held, so a concurrent insert and
    /// remove cannot interleave into a count that drifts from the map.
    fn adjust_events_count(&self, previous: Option<ConnClass>, current: Option<ConnClass>) {
        let was = previous == Some(ConnClass::Events);
        let is = current == Some(ConnClass::Events);
        match (was, is) {
            (false, true) => {
                self.events_live.fetch_add(1, Ordering::Relaxed);
            }
            (true, false) => {
                self.events_live.fetch_sub(1, Ordering::Relaxed);
            }
            // Unchanged, including the re-insert of an id that is already there.
            // Ids are server-minted UUIDs so that should never happen, but a
            // double count would be permanent and this costs nothing.
            (false, false) | (true, true) => {}
        }
    }

    /// How many browser tabs are connected right now: the live
    /// [`ConnClass::Events`] count.
    pub fn events_count(&self) -> usize {
        self.events_live.load(Ordering::Relaxed)
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

/// Whether a wire outcome is a soft refusal returned through an error status.
pub(crate) fn outcome_is_error(outcome: &WireCommandOutcome) -> bool {
    outcome
        .status
        .as_ref()
        .is_some_and(|status| status.tone == "error")
}

/// Map a delete command's accepted, soft-refused, and hard-error outcomes.
pub(crate) fn delete_wire_response(result: Result<WireCommandOutcome, String>) -> Response {
    match result {
        Ok(outcome) => match outcome.status {
            Some(status) if status.tone == "error" => {
                (StatusCode::CONFLICT, status.message).into_response()
            }
            _ => StatusCode::NO_CONTENT.into_response(),
        },
        Err(error) => (StatusCode::BAD_REQUEST, error).into_response(),
    }
}

/// A boxed error arm for helpers and extractors whose failure is a ready-made
/// axum [`Response`]. `Response` is a large type, and the stable clippy that
/// newly reached CI fires `result_large_err` on any `Result` carrying it in the
/// `Err` variant, so this newtype boxes it to shrink the `Result` by
/// construction rather than suppressing the lint, following the same choice
/// documented on `acquire_ws_permit` in `server.rs`.
pub struct RouteRejection(Box<Response>);

impl From<Response> for RouteRejection {
    fn from(resp: Response) -> Self {
        Self(Box::new(resp))
    }
}

impl IntoResponse for RouteRejection {
    fn into_response(self) -> Response {
        *self.0
    }
}

pub(crate) async fn require_configured_provider(
    engine: &EngineHandle,
    provider: &str,
) -> Result<(), RouteRejection> {
    match engine
        .bootstrap()
        .await
        .map(|bootstrap| bootstrap.available_providers.iter().any(|p| p == provider))
    {
        Some(true) => Ok(()),
        Some(false) => Err((
            StatusCode::BAD_REQUEST,
            dux_core::provider::provider_not_configured(provider),
        )
            .into_response()
            .into()),
        None => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "the engine is unavailable; retry shortly",
        )
            .into_response()
            .into()),
    }
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

/// What a create's await settled on, in the order the handler checks them.
///
/// Three outcomes rather than an `Option`, because "the resource never appeared"
/// covers two different answers: an operation still running, which is the `202`
/// the client correlates on, and one that already failed, which is a reason the
/// client can be told now.
pub(crate) enum AwaitedCreate {
    /// The resource surfaced; its id.
    Resolved(String),
    /// The dispatched operation finaled with an error before the resource
    /// appeared, carrying the final's message.
    Failed(String),
    /// Neither happened inside the window.
    Pending,
}

/// The error final `op_id` has already landed, if any.
///
/// The status snapshot carries every open status, keyed ones included, and the
/// read is a synchronous `watch` borrow, so the hundred-millisecond poll sees a
/// final within a tick of it landing. That is what makes the short retention the
/// emitting controller gives a final irrelevant here: the wait never has to
/// outlast it. A `busy` under the same key means the operation is still running,
/// which is not an answer.
fn failed_op_message(engine: &EngineHandle, op_id: Option<&str>) -> Option<String> {
    let op_id = op_id?;
    engine
        .status_snapshot()
        .into_iter()
        .find(|status| status.key.as_deref() == Some(op_id) && status.tone == "error")
        .map(|status| status.message)
}

/// The `422 Unprocessable Entity` reply for a create whose operation failed: the
/// request was well formed and dispatched, and the work it asked for could not be
/// done. The body is the failure's own sentence, the same one the events socket
/// carries, so a client showing either says the same thing.
pub(crate) fn create_failed(message: String) -> Response {
    (StatusCode::UNPROCESSABLE_ENTITY, message).into_response()
}

/// Poll the engine until create op `op_id` resolves to its session id, that op
/// fails, or the timeout elapses. This is the RACE-FREE create-correlation path:
/// the op id comes back in `WireCommandOutcome.created_op_id` for a synchronous
/// create (`new`/`fork`/`from_worktree`), and the engine records
/// `op_id -> session_id` when the worker-minted session lands, so the handler
/// resolves ITS exact session, never a concurrent create's. The same id is what
/// makes the failure observable, so this path watches its own op.
pub(crate) async fn await_session_for_op(
    engine: &EngineHandle,
    op_id: String,
    timeout: Duration,
) -> AwaitedCreate {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(id) = engine.created_session_for_op(op_id.clone()).await {
            return AwaitedCreate::Resolved(id);
        }
        if let Some(message) = failed_op_message(engine, Some(&op_id)) {
            return AwaitedCreate::Failed(message);
        }
        if Instant::now() >= deadline {
            return AwaitedCreate::Pending;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Poll the engine's session spine until a session id appears that was not in
/// `pre`, `watch_op` fails, or the timeout elapses. The FALLBACK
/// create-correlation path, used only by the from-PR create (whose create op is
/// minted later, inside the PR-lookup followup, so its id is not in the
/// synchronous outcome). The synchronous `new`/`fork`/`from_worktree` creates
/// instead use [`await_session_for_op`].
///
/// `watch_op` is therefore the PR-LOOKUP op here, and a lookup that fails is a
/// create that will never happen, so its final is the failure to report. A lookup
/// that succeeds hands off and clears that key instead, leaving this wait exactly
/// as it was.
///
/// RESIDUAL RACE: this returns the FIRST session not in `pre`, which under truly
/// concurrent creates (another tab, or a TUI create in flip mode) could be a
/// DIFFERENT request's session. The engine serializes the create worker via the
/// `CreateAgent` in-flight guard, which narrows but does not fully close the
/// window. The from-PR path is the only remaining caller and is comparatively
/// rare, so the residual race is accepted here; the op-id path above is race-free
/// and is preferred wherever the op id is available synchronously.
pub(crate) async fn await_new_session(
    engine: &EngineHandle,
    pre: &std::collections::HashSet<String>,
    watch_op: Option<&str>,
    timeout: Duration,
) -> AwaitedCreate {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(spine) = engine.spine().await
            && let Some(found) = spine.sessions.iter().find(|s| !pre.contains(&s.id))
        {
            return AwaitedCreate::Resolved(found.id.clone());
        }
        if let Some(message) = failed_op_message(engine, watch_op) {
            return AwaitedCreate::Failed(message);
        }
        if Instant::now() >= deadline {
            return AwaitedCreate::Pending;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Like [`await_new_session`] but for projects (the `POST /api/v1/projects` add).
/// A direct add resolves synchronously so the first poll usually wins; the
/// worker-backed adds (checkout-default, initial commit, init-repo) go through a
/// worker, so the poll covers them and `watch_op` is their own add op.
pub(crate) async fn await_new_project(
    engine: &EngineHandle,
    pre: &std::collections::HashSet<String>,
    watch_op: Option<&str>,
    timeout: Duration,
) -> AwaitedCreate {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(spine) = engine.spine().await
            && let Some(found) = spine.projects.iter().find(|p| !pre.contains(&p.id))
        {
            return AwaitedCreate::Resolved(found.id.clone());
        }
        if let Some(message) = failed_op_message(engine, watch_op) {
            return AwaitedCreate::Failed(message);
        }
        if Instant::now() >= deadline {
            return AwaitedCreate::Pending;
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

    /// Dispatch a project add whose worker refuses, and hand back the engine and
    /// the key its error final landed under.
    ///
    /// The refusal is `create_initial_commit`'s staged-changes stop, which needs
    /// no permission games and so behaves the same under root. The wait for the
    /// final to reach the snapshot is what makes the assertions below about a
    /// PRESENT final rather than an absent one.
    async fn engine_with_a_failed_add() -> (tempfile::TempDir, EngineHandle, String) {
        let tmp = tempfile::tempdir().unwrap();
        let engine = crate::test_support::test_engine_handle(tmp.path());

        let repo = tmp.path().join("staged-repo");
        std::fs::create_dir_all(&repo).unwrap();
        let run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?} failed");
        };
        run(&["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("staged.txt"), "x").unwrap();
        run(&["add", "staged.txt"]);

        let outcome = engine
            .apply_wire_scoped(
                dux_core::wire::WireCommand::AddProjectCreateInitialCommit {
                    path: repo.to_string_lossy().into_owned(),
                    name: String::new(),
                },
                StatusScope::All,
            )
            .await
            .expect("the add dispatches; the worker is what refuses");
        let key = outcome
            .status
            .and_then(|s| s.key)
            .expect("a worker-backed add mints a keyed op");

        let deadline = Instant::now() + Duration::from_secs(10);
        while failed_op_message(&engine, Some(&key)).is_none() {
            assert!(
                Instant::now() < deadline,
                "the add's error final never landed"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        (tmp, engine, key)
    }

    #[tokio::test]
    async fn await_new_project_fails_on_its_own_ops_error_final() {
        let (_tmp, engine, key) = engine_with_a_failed_add().await;
        let pre = std::collections::HashSet::new();

        let waited = await_new_project(&engine, &pre, Some(&key), Duration::from_secs(5)).await;

        match waited {
            AwaitedCreate::Failed(message) => assert!(
                message.contains("staged changes"),
                "the failure must carry the worker's own words, got {message:?}"
            ),
            _ => panic!("an error final under the watched key must end the wait"),
        }
    }

    /// The discriminating half: the same engine, the same error final in the
    /// snapshot, watched under a key that is not it. Nothing about somebody
    /// else's failure may end this wait, so the route answers 202.
    #[tokio::test]
    async fn await_new_project_ignores_an_error_final_under_another_key() {
        let (_tmp, engine, key) = engine_with_a_failed_add().await;
        let pre = std::collections::HashSet::new();

        let waited = await_new_project(
            &engine,
            &pre,
            Some(&format!("{key}-not-this-one")),
            Duration::from_millis(300),
        )
        .await;

        assert!(
            matches!(waited, AwaitedCreate::Pending),
            "another operation's failure is not this create's answer"
        );
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

    /// A PTY-class id (AgentPty or TerminalPty) must fall back to All: PTY
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

    /// The gauge counts browser tabs, which means Events sockets and nothing
    /// else: a PTY socket is a second connection from a tab that is already
    /// counted, so counting those would report three connections for one browser.
    #[test]
    fn the_events_gauge_counts_only_events_connections() {
        let gauge = Arc::new(AtomicUsize::new(0));
        let reg = ConnectionRegistry::with_events_gauge(Arc::clone(&gauge));
        assert_eq!(reg.events_count(), 0, "nothing connected yet");

        reg.insert("tab-1".into(), ConnClass::Events);
        reg.insert("tab-1-pty".into(), ConnClass::AgentPty);
        reg.insert("tab-1-term".into(), ConnClass::TerminalPty);
        assert_eq!(reg.events_count(), 1, "one browser tab, three sockets");
        assert_eq!(
            gauge.load(Ordering::Relaxed),
            1,
            "the shared gauge is what the terminal UI reads, so it must agree"
        );

        reg.insert("tab-2".into(), ConnClass::Events);
        assert_eq!(reg.events_count(), 2);

        reg.remove("tab-1-pty");
        assert_eq!(
            reg.events_count(),
            2,
            "a PTY socket closing changes nothing"
        );
        reg.remove("tab-1");
        assert_eq!(reg.events_count(), 1, "the tab left");
        reg.remove("tab-1");
        assert_eq!(
            reg.events_count(),
            1,
            "removing an id twice must not underflow the count"
        );
        reg.remove("tab-2");
        assert_eq!(reg.events_count(), 0);
        assert_eq!(gauge.load(Ordering::Relaxed), 0);
    }

    /// The two arms that exist because a double count would be PERMANENT, unlike a
    /// missed one: re-registering an id must not add a second time, and an id
    /// changing class must move the count with it.
    ///
    /// Server-minted UUIDs mean neither should ever happen, which is exactly why
    /// neither would be noticed without a test.
    #[test]
    fn re_registering_a_connection_id_neither_double_counts_nor_strands_a_count() {
        let reg = ConnectionRegistry::default();
        reg.insert("tab-1".into(), ConnClass::Events);
        reg.insert("tab-1".into(), ConnClass::Events);
        assert_eq!(
            reg.events_count(),
            1,
            "the same id registered twice is still one connection"
        );

        reg.insert("tab-1".into(), ConnClass::AgentPty);
        assert_eq!(
            reg.events_count(),
            0,
            "an id that stopped being an Events connection must stop being counted"
        );

        reg.insert("tab-1".into(), ConnClass::Events);
        assert_eq!(
            reg.events_count(),
            1,
            "and counted again when it comes back"
        );
        reg.remove("tab-1");
        assert_eq!(reg.events_count(), 0);
    }

    /// A registry nobody handed a gauge to still counts, so every existing serve
    /// path keeps working and the count is never a special case.
    #[test]
    fn a_registry_without_a_shared_gauge_still_counts() {
        let reg = ConnectionRegistry::default();
        reg.insert("tab-1".into(), ConnClass::Events);
        assert_eq!(reg.events_count(), 1);
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
