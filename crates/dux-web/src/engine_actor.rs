//! The engine runs on its own thread (the `Engine` is `!Send`). Async code talks to it
//! through `EngineHandle`: requests over a BOUNDED tokio mpsc (so the handle is
//! `Send + Sync` for use as axum state, and a misbehaving/flooding client cannot grow
//! the queue without limit — see [`REQ_CHANNEL_CAPACITY`]), the engine thread polling it
//! with `try_recv` on a tick (so it also drains worker events and fires the
//! coarse spine-change/status/commit signals); replies over tokio oneshots.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use dux_core::config::{server_bind_settings_changed, server_console_settings_changed};
use dux_core::engine::{
    Command, Engine, EventReaction, InFlightKey, ProjectPersistenceView, PrunedPtyKind,
};
use dux_core::ids::{TabId, TabIdRef};
use dux_core::model::TerminalOwner;
use dux_core::pty::{PtyClient, PtyViewerGuard};
use dux_core::statusline::{
    Generation, KeyedStatusController, KeyedWireStatus, StatusScope, StatusTone,
};
use dux_core::wire::{WireCommand, WireCommandOutcome, WireStatus};
use dux_core::worker::{AgentLaunchKind, AgentLaunchRequest};
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use crate::pty_owners::PtySizeOwners;

/// A PTY subscription: an RAII unsubscribe guard, an initial repaint snapshot,
/// and the live byte stream the caller forwards. Drop the guard to detach
/// immediately without waiting for the next PTY output.
/// (PTY bytes never travel through the request channel.)
pub type PtySubscription = (PtyViewerGuard, Vec<u8>, std::sync::mpsc::Receiver<Vec<u8>>);

/// Which half of the projects/sessions spine changed since the last tick. The
/// engine loop fingerprints the projected spine each tick and fires the matching
/// variant; the web layer's forwarder turns it into a coarse `projects.changed` /
/// `sessions.changed` event. Since the whole document is also pushed on the same
/// socket (see [`WorkspaceDoc`]), that event is a nudge for a client too old to
/// read the push, and a pointer at the thin per-resource reads.
///
/// A single coarse signal per side is intentional: the sessions side
/// also covers session lifecycle/status, the `working` hysteresis flag, and the
/// per-session terminal list (they all live in the sessions/sidebar projection).
/// The spec's finer `session.status` / `session.working` / `terminals.changed`
/// split is an optional later optimization and is deliberately NOT implemented here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpineChange {
    Projects,
    Sessions,
}

/// One unit of work for the engine thread.
pub enum EngineRequest {
    ApplyWire(
        WireCommand,
        oneshot::Sender<Result<WireCommandOutcome, String>>,
        /// Audience for any statuses this command mints. The actor sets
        /// `engine.current_origin` to this for the duration of `apply_wire` and
        /// resets it to [`StatusScope::All`] after, so a web operation's toasts
        /// reach only the originating connection. `All` is the broadcast default.
        StatusScope,
    ),
    /// A status from a non-engine producer (the changed-files `ChangesService`)
    /// to broadcast through the shared status controller so it auto-clears and
    /// reaches every client, exactly like engine-originated statuses.
    EmitStatus(WireStatus),
    /// Dismiss a keyed status without replacing it: the pending→final contract
    /// allows a CLEAR as the final, for operations whose success is already on
    /// the user's screen (the release-notes route: the rendered notes are the
    /// success signal, so a "loaded" toast would narrate the visible). Errors
    /// still post a real final on the same key.
    ClearStatus(String),
    SubscribePty(String, oneshot::Sender<Result<PtySubscription, String>>),
    WritePty(String, Vec<u8>),
    /// Resize a PTY: id, rows, cols, and the apply-order seq the owners lock
    /// stamped for it.
    ///
    /// The seq travels with the request because this resize is applied LATER,
    /// whenever this queue is drained, while another surface (the terminal UI,
    /// when it is serving in the background) applies its own resizes the moment
    /// it claims them. The apply site offers the seq to
    /// `PtySizeOwners::accept_grid_apply`, which drops a resize that a newer
    /// claim's geometry has already overtaken.
    ResizePty(String, u16, u16, u64),
    /// Read a live PTY's current grid as `(rows, cols)`, replying `None` when the
    /// id names nothing running. The PTY socket asks once per attach so the
    /// `connected` handshake can tell an arriving viewer what geometry the child
    /// is actually drawing for.
    PtyGridSize(String, oneshot::Sender<Option<(u16, u16)>>),
    /// A "user is looking at this tab" ping from a foregrounded, input-owning
    /// browser terminal. Fire-and-forget; routed to
    /// [`dux_core::engine::Engine::note_agent_viewed_if_known`] so continuous
    /// viewing keeps a tab's attention flag down even without typing, mirroring
    /// the TUI's per-tick focus stamp. The engine self-gates on the id being a
    /// real tab, so a stale/bogus id is a harmless no-op.
    NoteViewed(String),
    /// Subscribe to an existing companion terminal (no launch; replies immediately).
    SubscribeTerminal(String, oneshot::Sender<Result<PtySubscription, String>>),
    /// Create a companion terminal for a session, replying `(terminal_id, label)`.
    CreateTerminal(String, oneshot::Sender<Result<(String, String), String>>),
    /// Create a project terminal for a project (a plain shell at the project's
    /// repo root with no agent attached), replying `(terminal_id, label)`.
    CreateProjectTerminal(String, oneshot::Sender<Result<(String, String), String>>),
    /// Create a standalone terminal (a plain shell in the user's home directory,
    /// owned by neither an agent nor a project), replying `(terminal_id, label)`.
    /// Carries no owner id, because there is no owner to name.
    CreateStandaloneTerminal(oneshot::Sender<Result<(String, String), String>>),
    /// Resolve the owner of a companion terminal (instant lookup), or `None`
    /// when the terminal id is unknown. Lets the nested PTY sockets and the
    /// terminal REST routes enforce that a `:tid` belongs to its path owner
    /// (session or project) before subscribing to or deleting it (the legacy
    /// `SubscribeTerminal`/`DeleteTerminal` path looks terminals up by id alone
    /// and does not check ownership).
    TerminalOwnerOf(String, oneshot::Sender<Option<TerminalOwner>>),
    /// A terminal's owner paired with the ABSOLUTE directory it was spawned in:
    /// the root a terminal-rooted editor serves files from. Both halves in one
    /// call because the caller needs both to answer a single question, whether
    /// this address may serve this terminal and from where; asking twice would
    /// let the pair come from two different moments. Only the ~-collapsed
    /// display label travels the wire elsewhere, which is why the real path
    /// needs a query of its own.
    TerminalRoot(
        String,
        oneshot::Sender<Option<(TerminalOwner, std::path::PathBuf)>>,
    ),
    /// Create an extra tab for a session running the given provider (or the
    /// session's project default when `None`), replying `(tab_id, provider)`. The
    /// launch is fire-and-forget (async worker); the id returns synchronously so
    /// the REST handler can echo it and the client can attach.
    CreateAgentTab(
        String,
        Option<String>,
        oneshot::Sender<Result<(String, String), String>>,
    ),
    /// Start a DORMANT tab explicitly: the press on its "Start session" card.
    /// It is the one launch path that gets past a recorded failure, because a
    /// press is the user saying "try it again anyway"; dispatching the launch is
    /// itself what clears the verdict, so the pane that mounts behind the card
    /// then attaches to a launch already in flight rather than starting a second
    /// one. A tab that is already running is an idempotent `Ok`.
    StartAgentTab(String, oneshot::Sender<Result<(), String>>),
    /// Resolve the owning session id of an EXTRA tab (instant lookup), or
    /// `None` when the tab id is unknown or names a session's first tab, whose
    /// owner is resolved from that session's slot pointer rather than looked up.
    /// Lets the tab PTY socket and tab REST routes enforce that a `:tab` belongs
    /// to its path `:id`.
    TabSession(String, oneshot::Sender<Option<String>>),
    /// Resolve an agent's session-slot tab id (the id its first tab's PTY is
    /// keyed by), or `None` when the session id names no agent. The HTTP and
    /// socket layers are handed bare path segments, so this is how they ask the
    /// one resolver instead of assuming the slot tab id is the session id.
    SlotTabId(String, oneshot::Sender<Option<String>>),
    /// Run the create-agent branch preflight for `(project_id, name)`: does a new
    /// agent of that name start fresh or attach to an existing branch? Runs a git
    /// subprocess on the engine thread. `None` means the project is unknown.
    CreateAgentBranchPlan(
        String,
        String,
        oneshot::Sender<Option<dux_core::git::CreateAgentBranchPlan>>,
    ),
    /// Resolve a session's worktree path (instant lookup; diff I/O happens
    /// off-thread in the server handler).
    SessionWorktree(String, oneshot::Sender<Option<String>>),
    /// What the delete dialog needs in order to ask git about the branch it is
    /// offering to remove (instant lookup; the `rev-list` runs off-thread in
    /// the handler). `None` for a standalone agent, which has no branch, and
    /// for an orphaned session, whose project record is gone.
    SessionBranchDeleteInputs(
        String,
        oneshot::Sender<Option<dux_core::engine::BranchDeleteInputs>>,
    ),
    /// What git dux may do for one agent, in ONE round trip: the directory, and
    /// whether the changes panel works and the branch features exist there.
    /// Asking those separately would let a caller act on half an answer.
    ///
    /// Asking also REFRESHES a standalone folder's verdict off-thread, so a
    /// folder that became a repository since the last look starts working the
    /// next time the panel asks. `None` means the session is unknown.
    SessionGitAccess(
        String,
        oneshot::Sender<Option<dux_core::engine::SessionGitAccess>>,
    ),
    /// The runtime PTY key a pane's addressed id names, resolving the bare
    /// per-agent spelling of a slot tab the way the agent PTY socket does.
    /// `None` when nothing answers to the id.
    PtyKeyForPaneId(String, oneshot::Sender<Option<String>>),
    /// Where a file dropped onto the pane showing this pty id should be saved.
    /// The pty id may be a terminal, an agent's first tab, or an extra tab; the
    /// engine resolves all three. A terminal answers with a PLAN rather
    /// than a path, so the live-directory probe (a `/proc` read, or `lsof` on
    /// macOS) happens on a blocking pool and never on this thread.
    FileDropDestination(
        String,
        oneshot::Sender<Option<dux_core::file_drop::FileDropDestination>>,
    ),
    /// Where a file dropped onto the EDITOR'S FILE TREE should be saved: the
    /// root-relative directory the user dropped on, inside that editor's root.
    /// That root is an agent's worktree, or, for a terminal-rooted editor, the
    /// directory the terminal was spawned in. The directory travels
    /// UNVALIDATED; its guards live beside the walk that opens it, on the
    /// blocking pool.
    FileDropTreeDestination(
        String,
        String,
        oneshot::Sender<Option<dux_core::file_drop::FileDropDestination>>,
    ),
    /// The agent pane a drop on this pty id could affect: its session id and its
    /// worktree. `None` when there is no agent behind the pane (a terminal owned
    /// by a project or by nothing). Ownership only: whether the file actually
    /// landed inside that worktree is checked on the FINAL path, off this
    /// thread.
    FileDropRefreshTarget(
        String,
        oneshot::Sender<Option<(String, std::path::PathBuf)>>,
    ),
    /// Resolve a project's repo-root path (instant lookup). Lets the project
    /// terminal routes and the project-nested PTY socket 404 an unknown project
    /// before acting, mirroring how `SessionWorktree` gates the session routes.
    ProjectPath(String, oneshot::Sender<Option<String>>),
    /// Snapshot the live process trees the resource monitor should sample: every
    /// live agent tab and companion terminal, each with its spine id and root
    /// pid. Instant map iteration only: the blocking sysinfo walk runs off both
    /// the engine thread and the reactor, in [`crate::resource_routes::ResourceService`].
    ResourceTargets(oneshot::Sender<Vec<dux_core::worker::ResourceTarget>>),
    /// Snapshot the build-/config-static bootstrap projection (providers, macros,
    /// palette commands, welcome tips, version, `ui.*` flags, gh availability,
    /// global env) served by `GET /api/v1/bootstrap`. Instant clone off engine
    /// state; refetched by the client on a `config.changed` event.
    Bootstrap(oneshot::Sender<dux_core::viewmodel::BootstrapView>),
    /// Snapshot the projects/sessions/sidebar spine served by the thin
    /// per-resource reads (`/api/v1/projects`, `/api/v1/sessions`). Instant clone
    /// off engine state; refetched by the client on a `projects.changed` /
    /// `sessions.changed` event. The hot whole-spine read (`GET /api/v1/workspace`)
    /// instead uses [`EngineRequest::SpineJson`] (the loop's cached serialization).
    Spine(oneshot::Sender<dux_core::viewmodel::SpineView>),
    /// The pre-serialized whole-spine JSON for `GET /api/v1/workspace`, served
    /// from the loop's cache (rebuilt only when the spine actually changes)
    /// instead of re-projecting + re-serializing on every client request. It
    /// carries the document's `rev`, because the cache is serialized WITH it (see
    /// [`WorkspaceDoc`]), so a fetched body and a pushed frame are the same bytes.
    /// Handled inline in the loop because the cache is loop-local state.
    SpineJson(oneshot::Sender<String>),
    /// Project ONLY the requested session for `GET /api/v1/sessions/:id` instead of
    /// building the whole spine to find one session, together with THAT session's
    /// terminals: the thin read still nests them (see
    /// [`crate::workspace_routes::SessionWithTerminals`]), and fetching them here
    /// keeps it to one round trip and one consistent snapshot. `None` when the id
    /// is unknown (the handler returns 404).
    Session(
        String,
        oneshot::Sender<
            Option<(
                dux_core::viewmodel::SessionView,
                Vec<dux_core::viewmodel::TerminalView>,
            )>,
        >,
    ),
    /// Resolve the session id produced by a create op (the opaque id returned in
    /// `WireCommandOutcome.created_op_id`). Lets the REST create handler poll for
    /// ITS exact new session instead of a racy set-difference. `None` while the
    /// create is still in flight or the entry has expired.
    CreatedSessionForOp(String, oneshot::Sender<Option<String>>),
    /// Manually attach (pin) a pull request to a session from a raw typed
    /// reference (`PUT /api/v1/sessions/:id/pull-request`). Dispatches
    /// [`dux_core::engine::Engine::dispatch_attach_pull_request`], which mints
    /// the ONE keyed op spanning resolve and attach and spawns the gh lookup
    /// worker; the reply carries the op id. The pending busy is broadcast on
    /// the status stream by the handler arm (scoped like the `ApplyWire` arm's
    /// statuses), and the final is resolved engine-side in
    /// `process_worker_event`, so it rides the normal status stream.
    AttachPullRequest(
        String,
        String,
        StatusScope,
        oneshot::Sender<Result<String, String>>,
    ),
    /// Bump and return the next monotonic changed-files revision for a session
    /// (one actor round-trip over the engine's single SQLite connection). The
    /// `ChangesService` calls this at each detected change; the counter is
    /// persisted, so it never resets across restarts.
    NextChangesRev(String, oneshot::Sender<u64>),
    /// The configured preferred editor name (`config.editor.default`, e.g.
    /// "cursor"/"vscode"/"zed"). Instant clone; the detect + launch I/O for the
    /// "open in editor" action runs off-thread in the server handler.
    EditorDefault(oneshot::Sender<String>),
    /// Resolve the directory the add-project picker should open at from the LIVE
    /// config (`defaults.start_directory`, with the shared fallback chain). Read
    /// through the engine so it reflects the currently-applied config — a reload
    /// that swaps `engine.config` changes the answer; a not-yet-reloaded raw save
    /// does not. Instant clone of a resolved path; the filesystem listing runs
    /// off-thread in the browse handler.
    BrowseStartDir(oneshot::Sender<String>),
    /// Ask the engine to recompute the changed-files lists for a worktree (after
    /// an HTTP git mutation ran the git op off-thread). Fire-and-forget: the
    /// engine spawns its off-thread refresh worker, whose result flows back
    /// through the normal `ChangedFilesReady` path; the refreshed lists are then
    /// served by the REST changed-files read.
    RefreshChangedFiles(String),
    /// Snapshot the inputs needed to classify a project's managed worktrees:
    /// the project, the dux paths, and the current sessions. Instant clones off
    /// engine state; the git work (`list_worktrees` + classification) runs
    /// off-thread in the server handler (it shells to git), mirroring how
    /// `SessionWorktree` feeds the off-thread diff. `None` when the project id is
    /// unknown.
    ProjectWorktreeInputs(
        String,
        oneshot::Sender<
            Option<(
                dux_core::model::Project,
                dux_core::config::DuxPaths,
                Vec<dux_core::model::AgentSession>,
            )>,
        >,
    ),
    /// Everything the pull-request reference resolver needs: the live project
    /// list and the GitHub host policy. Both are instant clones off engine
    /// state; the git call per project runs off-thread in the caller (the
    /// [`EngineRequest::ProjectWorktreeInputs`] precedent), because reading a
    /// project's configured address shells out to git and must not run on the
    /// engine loop or the async reactor.
    ///
    /// Deliberately fetched per request, never cached: the answer changes when
    /// an address is edited, when git's rewrite configuration changes, and when
    /// a project's path moves under the same id.
    PullRequestResolutionInputs(
        oneshot::Sender<(
            Vec<dux_core::model::Project>,
            dux_core::gh::GithubHostPolicy,
            bool,
        )>,
    ),
    /// Resolve a session's startup-command-log context: `(dux paths, project_id)`
    /// for the session, so a GET handler can list/read its startup-command logs
    /// off-thread. `None` when the session id is unknown. Instant clone off engine
    /// state, mirroring [`EngineRequest::ProjectWorktreeInputs`].
    SessionStartupLogContext(
        String,
        oneshot::Sender<Option<(dux_core::config::DuxPaths, String)>>,
    ),
    /// Resolve a PROJECT's startup-command-log context: the dux paths, once the
    /// project id is confirmed to exist. The project id is the log directory key
    /// itself, so nothing else needs resolving. `None` when the project id is
    /// unknown, which is what makes the GET 404 instead of reporting an empty
    /// listing for a project that was never registered.
    ProjectStartupLogContext(String, oneshot::Sender<Option<dux_core::config::DuxPaths>>),
    /// Read the raw `config.toml` text off the engine thread for the Monaco
    /// config editor. Replies with the file's contents verbatim, or the canonical
    /// plain render of the running config when the file does not exist yet. A
    /// non-`NotFound` read error (permission denied, I/O failure) is an `Err` so
    /// the editor refuses to open with wrong content rather than silently showing
    /// (and letting the user save) a blank/default over their real config.
    ReadRawConfig(oneshot::Sender<Result<String, String>>),
    /// Validate and write raw `config.toml` text from the Monaco editor. Parses
    /// the text as a `Config` first (rejecting invalid TOML), flushes any pending
    /// managed writes so they cannot clobber it, then atomically writes the file
    /// verbatim. The caller adopts the change via the existing config reload.
    /// `Ok(())` on success; `Err(message)` for a parse or IO failure.
    WriteRawConfig(String, oneshot::Sender<Result<(), String>>),
    /// Read everything `dux_core::first_load::plan` needs, off the engine thread:
    /// the last-seen version from SQLite, the running display version, the two
    /// `[ui]` suppression flags, and the state root the release-notes cache lives
    /// under. One round-trip so the resolver never touches the store directly
    /// (the engine is the single writer/reader of `sessions.sqlite3`).
    FirstLoadInputs(oneshot::Sender<FirstLoadInputs>),
    /// Record the running version as seen (`SessionStore::set_last_seen_version`).
    ///
    /// Routed through the engine because it owns the ONE `SessionStore` handle,
    /// which is also what makes dismissal shared: the TUI reads the same row, so
    /// dismissing in a browser settles the screen for both surfaces.
    MarkVersionSeen(String, oneshot::Sender<Result<(), String>>),
    /// Gracefully wind down every running PTY (SIGTERM the children so CLIs can
    /// save state for a later resume), then stop the engine thread. Replies once
    /// the wind-down completes so the server can finish exiting.
    Shutdown(oneshot::Sender<()>),
}

/// Resolve the live PTY for an id, which may name either an agent provider
/// (keyed by `session_id`) or a companion terminal (keyed by `terminal_id`).
/// This unifies the write/resize path so the same input/resize routing serves
/// both agents and terminals via whichever id the connection is subscribed to.
fn pty_for<'a>(engine: &'a Engine, id: &str) -> Option<&'a PtyClient> {
    // The id's kind is exactly what this lookup decides, so it arrives as a plain
    // string and is named per probe rather than up front.
    engine
        .providers
        .get(TabIdRef::new(id))
        .or_else(|| engine.companion_terminals.get(id).map(|t| &t.client))
}

const TICK: Duration = Duration::from_millis(50);

/// Consider running the spine fingerprint/cache check every Nth tick rather than
/// every tick (one decision per ~250ms instead of per 50ms). Whether the check
/// actually serializes the spine on a given interval is then gated further by
/// the change signals below ([`SpineCheck::maybe_check`]): an idle interval with
/// no mutation, no streaming transition, and no backstop does ZERO work.
const SPINE_CHECK_TICK_INTERVAL: u64 = 5;

/// Every ~40 ticks (~2s), run an unconditional spine fingerprint comparison so
/// mutations that do not bump `mutation_version` are still published.
const SPINE_BACKSTOP_TICK_INTERVAL: u32 = 40;

/// Per-iteration control for [`run_engine_loop`]. Checked once at the top of
/// every outer loop iteration: `Continue` runs another tick, `Exit` stops the
/// loop and returns the engine to the caller. The in-process flip's status
/// screen drives this (via [`crate::serve_with_engine`]); the dedicated-thread
/// path always returns `Continue` (it exits only on the `Shutdown` request).
pub enum LoopControl {
    Continue,
    Exit,
}

/// The loop-side ends of the actor channels, owned by [`run_engine_loop`], with
/// [`EngineHandle`] holding the caller-facing ends. The dedicated-thread path
/// and the in-process flip build the channels once and run the same loop body.
pub(crate) struct ActorLoopEnds {
    req_rx: mpsc::Receiver<EngineRequest>,
    status_tx: broadcast::Sender<WireStatus>,
    status_clear_tx: broadcast::Sender<Option<String>>,
    status_snapshot_tx: watch::Sender<Vec<KeyedWireStatus>>,
    /// Fires `()` once per successful config reload so the web layer can emit a
    /// `config.changed` event on its event bus (clients then refetch
    /// `/api/v1/bootstrap`). Broadcast — the web forwarder is the only listener,
    /// but a broadcast keeps the send a cheap fire-and-forget with no receiver.
    config_reload_tx: broadcast::Sender<()>,
    /// Fires a [`SpineChange`] whenever the projected projects-portion or
    /// sessions+sidebar-portion of the spine changes, so the web layer emits a
    /// coarse `projects.changed` / `sessions.changed` event. The document itself
    /// travels on `workspace_tx` below; this stays a value-less signal.
    /// Broadcast, though the web forwarder is the only listener: a broadcast keeps
    /// the send a cheap fire-and-forget with no receiver.
    spine_change_tx: broadcast::Sender<SpineChange>,
    /// Publishes the whole workspace document each time the loop rebuilds its
    /// cached serialization, so `/ws/events` connections can be PUSHED the new
    /// document instead of each refetching it. A `watch` rather than a
    /// broadcast: it coalesces by construction (a slow connection sees only the
    /// latest document, never a queue of superseded ones), it has no `Lagged`
    /// variant to recover from, and its current value IS the replay a
    /// newly-subscribing connection needs. `None` only before the loop has
    /// built its first document; a replay never sends that.
    workspace_tx: watch::Sender<Option<Arc<WorkspaceDoc>>>,
    /// Shared with the caller-facing [`EngineHandle`] and every PTY forwarder.
    /// The inline `Shutdown` request trips this so forwarders exit promptly even
    /// before the engine drop disconnects their channels.
    shutdown_flag: Arc<AtomicBool>,
    /// The same limits as [`EngineHandle::live_limits`]: each successful reload
    /// stores its `[server]` values here so the routes read them next request.
    live_limits: Arc<LiveServerLimits>,
    /// The same registry as [`EngineHandle::pty_input_owners`]: the loop reads
    /// it every spine check to overlay the current input owners onto the
    /// projected spine, so an ownership flip moves the sessions fingerprint and
    /// fires `sessions.changed` like any other spine mutation.
    pty_input_owners: Arc<PtySizeOwners>,
    /// The serve's live Tailscale-mode handle, filled by the serve path once its
    /// loop exists. The reload arm uses it to apply a changed
    /// `[server] tailscale` to the running listener; empty means nothing is
    /// serving and the reload only saved the value.
    tailscale_mode_control: Arc<std::sync::OnceLock<crate::serve_legs::TailscaleModeControl>>,
}

/// Extract the reloaded `Config` from a reload follow-up reaction, consuming it.
///
/// The engine returns `ApplyReloadedConfig` bare in the common case, but folds it
/// into a `Multi` (alongside the deferred saves' status reactions) when
/// config-mutating commands were deferred during the reload. The actor must
/// handle BOTH so the config-reload and server-restart warning always fire.
/// Returns `None` for any reaction that is not (and does not wrap) an
/// `ApplyReloadedConfig`.
fn take_apply_reloaded_config(reaction: EventReaction) -> Option<Box<dux_core::config::Config>> {
    match reaction {
        EventReaction::ApplyReloadedConfig(config) => Some(config),
        EventReaction::Multi(reactions) => {
            reactions.into_iter().find_map(take_apply_reloaded_config)
        }
        _ => None,
    }
}

/// The warning a reload owes the user about `[server]` settings it could not
/// make live, or `None` when nothing startup-bound moved.
///
/// Two independent sentences because the two sets are read at different moments.
/// `background` picks the bind sentence's remedy: with the background server the
/// restart is stopping and starting it. The console sentence never takes that
/// remedy, because only `dux server` builds a console at all.
fn server_restart_warning_copy(
    prev: &dux_core::config::ServerConfig,
    next: &dux_core::config::ServerConfig,
    background: bool,
) -> Option<String> {
    let mut sentences: Vec<&str> = Vec::new();
    if server_bind_settings_changed(prev, next) {
        sentences.push(
            "Server settings changed in config that are read only when a listener binds; \
             restart the server to apply them.",
        );
        if background {
            sentences
                .push("With the background server, stopping and starting it again is the restart.");
        }
    }
    if server_console_settings_changed(prev, next) {
        sentences.push(
            "The [server] color setting changed; the console that reads it is built by \
             `dux server`, so it applies the next time you start `dux server`.",
        );
    }
    match sentences.is_empty() {
        true => None,
        false => Some(sentences.join(" ")),
    }
}

/// Bound on the engine request channel. A burst buffer, not a steady-state
/// queue: the engine drains the WHOLE channel every `TICK` (50ms), so under
/// normal use it holds only a handful of in-flight requests. The cap exists so a
/// flooding or buggy client cannot grow the queue without limit. Reply-bearing
/// sends apply backpressure when full (`.send().await`
/// waits for the next drain); fire-and-forget sends (`write_pty`, `resize_pty`,
/// `refresh_changed_files`, `emit_status`) use `try_send` and drop on a full
/// channel — acceptable overload shedding, since reaching this depth means the
/// producer is far outrunning a 20-drains-per-second consumer. Kept a const,
/// like the broadcast capacities above, rather than user config: it is an
/// internal safety ceiling, not a preference.
///
/// WITH THE BACKGROUND SERVER the consumer is the terminal UI's run loop rather
/// than this 50ms tick, so the drain cadence is that loop's poll interval (capped
/// at 33ms while serving, so if anything faster). The shedding above is accepted
/// unchanged, and it is worth saying why rather than leaving it to be rediscovered:
/// 1024 in-flight fire-and-forget requests means a producer far outrunning a
/// ~30-drains-per-second consumer, and the two kinds of request that can be shed
/// there already heal. A dropped keystroke is a keystroke the user watches not
/// appear and retypes; a dropped resize is recovered by the grid handshake, which
/// re-reads the authoritative geometry on the next attach or bounce rather than
/// trusting that every resize frame landed.
const REQ_CHANNEL_CAPACITY: usize = 1024;

/// Build the actor channels and split them into the caller-facing
/// [`EngineHandle`] and the loop-side [`ActorLoopEnds`]. Both server entry
/// points (the dedicated engine thread and the in-process flip) call this so
/// the channel topology is defined in exactly one place.
/// The `[server]` limits a running listener can adopt from a config reload.
///
/// Everything else under `[server]` is frozen when the listener binds (a
/// semaphore, a body-limit layer) or when `dux server` builds its console, and
/// is reported by [`server_restart_warning_copy`] instead. These are plain
/// scalars the routes and socket handlers read per request, so the actor stores
/// the reloaded values here and the next one honors them.
///
/// Seeded from the router's own bind-time values in `build_app`, because a test
/// or a serve path may pass something other than the engine's config.
#[derive(Debug, Default)]
pub struct LiveServerLimits {
    search_index_max_files: AtomicUsize,
    access_log: AtomicBool,
    pty_send_timeout_seconds: AtomicUsize,
    heartbeat_deadline_seconds: AtomicUsize,
}

impl LiveServerLimits {
    /// Cap on the editor's file-search flat walk. `0` disables the cap.
    pub fn search_index_max_files(&self) -> usize {
        self.search_index_max_files.load(Ordering::Relaxed)
    }

    pub fn set_search_index_max_files(&self, value: usize) {
        self.search_index_max_files.store(value, Ordering::Relaxed);
    }

    /// Whether the per-request access log is on. The middleware still requires an
    /// active console, so the flip and the noop-console paths emit nothing.
    pub fn access_log(&self) -> bool {
        self.access_log.load(Ordering::Relaxed)
    }

    pub fn set_access_log(&self, value: bool) {
        self.access_log.store(value, Ordering::Relaxed);
    }

    /// Deadline on one of a PTY socket's OPENING sends, in seconds. Read when a
    /// socket opens, so a reload applies to the next connection rather than to
    /// the ones already attached. `0` means "not seeded yet" and the caller
    /// falls back to the compiled default; the config renderer never writes 0
    /// and a user who does is asking for no bound at all, which is the one
    /// answer this must not give.
    pub fn pty_send_timeout_seconds(&self) -> usize {
        self.pty_send_timeout_seconds.load(Ordering::Relaxed)
    }

    pub fn set_pty_send_timeout_seconds(&self, value: usize) {
        self.pty_send_timeout_seconds
            .store(value, Ordering::Relaxed);
    }

    /// How long a browser waits for the server's answer to one beat before it
    /// treats the socket as half-open and reconnects
    /// (`[server] heartbeat_deadline_seconds`). Nothing on the server times
    /// itself by this; it is read so a send that ANSWERS a beat cannot outlive
    /// the window the client is waiting in. `0` means "not seeded yet" and the
    /// caller falls back to the compiled default.
    pub fn heartbeat_deadline_seconds(&self) -> usize {
        self.heartbeat_deadline_seconds.load(Ordering::Relaxed)
    }

    pub fn set_heartbeat_deadline_seconds(&self, value: usize) {
        self.heartbeat_deadline_seconds
            .store(value, Ordering::Relaxed);
    }

    /// Adopt every value from a reloaded `[server]` section.
    pub fn store_from(&self, server: &dux_core::config::ServerConfig) {
        self.set_search_index_max_files(server.search_index_max_files);
        self.set_access_log(server.access_log);
        self.set_pty_send_timeout_seconds(server.pty_send_timeout_seconds as usize);
        self.set_heartbeat_deadline_seconds(server.heartbeat_deadline_seconds as usize);
    }
}

pub(crate) fn build_actor_channels(engine: &Engine) -> (EngineHandle, ActorLoopEnds) {
    let (req_tx, req_rx) = mpsc::channel::<EngineRequest>(REQ_CHANNEL_CAPACITY);
    // Status uses THREE channels driven from one place (the StatusEmitter):
    //  - `status_tx` (broadcast) delivers every status LIVE, so a transient
    //    pending flash ("Pulling…", "Launching…") is never coalesced away.
    //  - `status_clear_tx` (broadcast) delivers each key that was cleared so
    //    clients can dismiss the matching toast without waiting for a replacement.
    //    `None` = the anonymous slot; `Some(key)` = a named keyed op.
    //  - `status_snapshot_tx` (watch) always holds ALL OPEN statuses so a client
    //    connecting mid-status reads the full set once on connect (see
    //    `status_snapshot`), rather than waiting blank for the next update.
    let (status_tx, _status_rx) = broadcast::channel::<WireStatus>(256);
    let (status_clear_tx, _status_clear_rx) = broadcast::channel::<Option<String>>(256);
    let (status_snapshot_tx, status_snapshot_rx) = watch::channel::<Vec<KeyedWireStatus>>(vec![]);
    // Config-reload notifier: the loop fires `()` on each successful reload and the
    // web layer's forwarder turns it into a `config.changed` event. A small buffer
    // is plenty — reloads are rare and the forwarder drains promptly.
    let (config_reload_tx, _config_reload_rx) = broadcast::channel::<()>(8);
    // Spine-change notifier: the loop fingerprints the spine each tick and fires a
    // `SpineChange` per changed side; the web forwarder turns it into a coarse
    // `projects.changed` / `sessions.changed` event. A small buffer is plenty — the
    // forwarder drains promptly and a `Lagged` recovery just re-emits both coarse
    // signals (idempotent refetches).
    let (spine_change_tx, _spine_change_rx) = broadcast::channel::<SpineChange>(64);
    // Workspace-document publication: the loop replaces this value each time it
    // rebuilds the cached serialization, and every `/ws/events` connection
    // subscribed to a coarse topic is handed the new document. `None` is the
    // pre-first-build value; the loop replaces it before it serves anything.
    let (workspace_tx, workspace_rx) = watch::channel::<Option<Arc<WorkspaceDoc>>>(None);
    let shutdown_flag = Arc::new(AtomicBool::new(false));
    // The input-ownership registry is built alongside the channels because it,
    // too, is a bridge between the loop and the web layer: the PTY socket
    // handlers write claims into it and the loop's spine check reads them back
    // out to publish the owner per agent tab.
    let pty_input_owners = Arc::new(PtySizeOwners::default());
    // Built here for the same reason as `pty_input_owners`: the loop starts
    // before the router exists, so both sides have to be handed the same Arc.
    let live_limits = Arc::new(LiveServerLimits::default());
    // Filled by the serve path once its loop exists, which is after the actor is
    // already running on `dux server`. A `OnceLock` rather than a constructor
    // argument for exactly that reason; empty means nothing is serving, which is
    // what a reload on a TUI-only run sees.
    let tailscale_mode_control: Arc<std::sync::OnceLock<crate::serve_legs::TailscaleModeControl>> =
        Arc::new(std::sync::OnceLock::new());
    (
        EngineHandle {
            req_tx,
            status_tx: status_tx.clone(),
            status_clear_tx: status_clear_tx.clone(),
            status_snapshot_rx,
            config_reload_tx: config_reload_tx.clone(),
            spine_change_tx: spine_change_tx.clone(),
            workspace_rx,
            shutdown_flag: Arc::clone(&shutdown_flag),
            has_active_processes: Arc::clone(&engine.has_active_processes),
            pty_input_owners: Arc::clone(&pty_input_owners),
            live_limits: Arc::clone(&live_limits),
            tailscale_mode_control: Arc::clone(&tailscale_mode_control),
            #[cfg(test)]
            refresh_requests: Arc::new(std::sync::Mutex::new(Vec::new())),
        },
        ActorLoopEnds {
            req_rx,
            status_tx,
            status_clear_tx,
            status_snapshot_tx,
            config_reload_tx,
            spine_change_tx,
            workspace_tx,
            shutdown_flag,
            pty_input_owners,
            live_limits,
            tailscale_mode_control: Arc::clone(&tailscale_mode_control),
        },
    )
}

/// Spawn the four global background workers on `engine`. Both `App::run` (the
/// TUI) and every server entry point spawn these, and the in-process flip hands
/// the SAME engine — with these workers already running — to the other surface,
/// which calls this again. The spawn helpers are individually idempotent for
/// the long-lived pollers (see `dux_core::engine`), so a redundant call here is
/// safe: it will not start a second poller.
pub(crate) fn spawn_global_workers(engine: &mut Engine) {
    engine.spawn_changed_files_poller();
    engine.spawn_branch_sync_worker();
    engine.spawn_project_branch_status_checks();
    engine.spawn_gh_status_check();
}

/// How long to wait for an agent provider to come up before failing a subscribe,
/// and the threshold after which a stale `Busy` entry is upgraded to `Warning`.
/// Shared with the TUI via `dux_core::statusline::BUSY_TIMEOUT`.
const LAUNCH_TIMEOUT: Duration = dux_core::statusline::BUSY_TIMEOUT;

/// A subscribe that is waiting for its provider to be launched/resumed. The reply
/// is held until `engine.providers` contains the session (success) or the
/// deadline passes (timeout).
struct PendingSubscribe {
    /// The TAB whose provider this subscribe is waiting on, never a session id:
    /// a subscribe to an extra tab parks its own tab id here, and both the
    /// `providers` probe and the `AgentLaunch` guard below are tab-keyed.
    tab_id: TabId,
    reply: Option<oneshot::Sender<Result<PtySubscription, String>>>,
    deadline: Instant,
}

#[derive(Clone)]
pub struct EngineHandle {
    req_tx: mpsc::Sender<EngineRequest>,
    status_tx: broadcast::Sender<WireStatus>,
    status_clear_tx: broadcast::Sender<Option<String>>,
    status_snapshot_rx: watch::Receiver<Vec<KeyedWireStatus>>,
    /// Notifies on each successful config reload (see [`ActorLoopEnds`]). The web
    /// layer subscribes via [`EngineHandle::subscribe_config_reloads`] and re-emits
    /// a `config.changed` event so clients refetch `/api/v1/bootstrap`.
    config_reload_tx: broadcast::Sender<()>,
    /// Notifies on each projects/sessions spine change (see [`ActorLoopEnds`]). The
    /// web layer subscribes via [`EngineHandle::subscribe_spine_changes`] and
    /// re-emits a coarse `projects.changed` / `sessions.changed` event. A client
    /// that reads the pushed document ignores it; an older one refetches
    /// `/api/v1/workspace` on it.
    spine_change_tx: broadcast::Sender<SpineChange>,
    /// The receiving end of the workspace-document publication (see
    /// [`ActorLoopEnds::workspace_tx`]). Held here for the whole life of the
    /// handle, mirroring `status_snapshot_rx`: a `watch` channel with no live
    /// receiver would make every publish a no-op, and connections come and go.
    workspace_rx: watch::Receiver<Option<Arc<WorkspaceDoc>>>,
    /// Tripped when the server is tearing down (ReturnToTui, QuitProcess, or a
    /// `Shutdown` request). PTY forwarders poll it so their blocking
    /// `recv_timeout` loop exits promptly even when the engine — and therefore
    /// the std-mpsc `Sender` in the `PtyClient` reader thread — stays alive
    /// across the flip. Without this, a forwarder parked on a never-disconnecting
    /// channel would wedge the tokio blocking pool and hang the runtime teardown.
    shutdown_flag: Arc<AtomicBool>,
    /// Shared clone of the engine's `has_active_processes` flag, so the
    /// changed-files poller can read whether any agent PTY is live with a local
    /// atomic load (deciding its 2s-vs-10s cadence) instead of an actor
    /// round-trip. The engine writes it; the handle only reads it.
    has_active_processes: Arc<AtomicBool>,
    /// The per-PTY input-ownership registry, shared three ways: the per-PTY
    /// socket handlers write claims/releases into it (via
    /// [`crate::server::AppState`], which clones this Arc out of the handle),
    /// the file-drop route reads its courtesy check from it, and the engine
    /// actor loop overlays it onto the spine so ownership is published to every
    /// client. Built here (in [`build_actor_channels`]) because the loop starts
    /// before the router exists, so the router cannot be the one to create it.
    pty_input_owners: Arc<PtySizeOwners>,
    /// The `[server]` limits a reload can move on a bound listener, shared with
    /// the router the same way and for the same reason as `pty_input_owners`.
    live_limits: Arc<LiveServerLimits>,
    /// Test-only tally of the worktrees [`Self::refresh_changed_files`] was asked
    /// to recompute, newest last. That call is fire-and-forget into the actor
    /// channel, so a route test has no other way to prove the request was made,
    /// and "asked the engine to refresh" is exactly half of what a refresh-now
    /// route must get right.
    /// The same slot as [`ActorLoopEnds::tailscale_mode_control`], so the serve
    /// path can hand the actor its live Tailscale-mode handle after the actor is
    /// already running (which it always is on `dux server`).
    tailscale_mode_control: Arc<std::sync::OnceLock<crate::serve_legs::TailscaleModeControl>>,
    #[cfg(test)]
    refresh_requests: Arc<std::sync::Mutex<Vec<String>>>,
}

// Axum state must be `Send + Sync`; prove the handle satisfies that here so a future
// regression (e.g. swapping a channel type) fails at compile time, not at the axum
// router boundary.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<EngineHandle>();
};

impl EngineHandle {
    /// Hand the actor this serve's live Tailscale-mode handle. Called once per
    /// serve, before the first reload can happen; a second call is ignored.
    pub(crate) fn set_tailscale_mode_control(
        &self,
        control: crate::serve_legs::TailscaleModeControl,
    ) {
        let _ = self.tailscale_mode_control.set(control);
    }

    /// The teardown flag PTY forwarders poll. Cloned into each forwarder so a
    /// blocking `recv_timeout` loop can break within one timeout window once the
    /// server starts winding down, even though the underlying `PtyClient`'s
    /// `Sender` outlives the flip (ReturnToTui keeps PTYs alive). The same flag
    /// is held loop-side ([`ActorLoopEnds`]) and by `serve_with_engine`, which
    /// trips it the instant the engine loop returns.
    pub(crate) fn shutdown_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.shutdown_flag)
    }

    pub fn subscribe_status(&self) -> broadcast::Receiver<WireStatus> {
        self.status_tx.subscribe()
    }

    /// Subscribe to the clear broadcast. Each item is the key that was removed:
    /// `None` = the anonymous slot cleared, `Some(key)` = a named keyed op. The
    /// `/ws/events` socket converts each into a `status_cleared` event.
    pub fn subscribe_status_clears(&self) -> broadcast::Receiver<Option<String>> {
        self.status_clear_tx.subscribe()
    }

    /// All currently open statuses (anonymous + keyed), from the snapshot watch.
    /// A client connecting mid-operation reads this once and sends each entry as
    /// a `Status` frame so it sees all active toasts immediately — e.g. a
    /// "Launching agent…" Busy that hasn't resolved yet — instead of a blank
    /// line until the next live update. An empty `Vec` means nothing is showing.
    pub fn status_snapshot(&self) -> Vec<KeyedWireStatus> {
        self.status_snapshot_rx.borrow().clone()
    }

    /// A receiver for the pushed workspace document. Each `/ws/events`
    /// connection clones one and forwards every new document to the client that
    /// asked for the coarse topics, which is what keeps N clients from each
    /// pulling the same document on every coarse ping. The current
    /// value doubles as the replay a newly-subscribed connection is handed.
    pub(crate) fn workspace_docs(&self) -> watch::Receiver<Option<Arc<WorkspaceDoc>>> {
        self.workspace_rx.clone()
    }

    /// Like [`emit_status`] but attaches a correlation key so a later success,
    /// error, or clear on the same key replaces or dismisses the same toast.
    /// Prefer this over `emit_status` for any operation that has a keyed lifecycle
    /// (a "Working…" busy that should be replaced by an info on success and
    /// dismissed by `StatusCleared`).
    pub fn emit_keyed_status(&self, key: impl Into<String>, status: WireStatus) {
        self.emit_status(status.with_key(key));
    }

    /// Publish a status from a non-engine producer (the changed-files
    /// `ChangesService`) THROUGH the shared status controller — not directly onto
    /// the broadcast — so it auto-clears on the same tone-aware policy as every
    /// other status and can never linger. The engine loop drains this and emits it
    /// via its `StatusEmitter`. A no-op if the engine loop has already exited.
    pub fn emit_status(&self, status: WireStatus) {
        // `try_send` (not `send().await`): this is sync fire-and-forget, called
        // from non-engine producers (the changed-files `ChangesService`). On a
        // full channel the status is dropped
        // — only under extreme overload — but a dropped status is worth a
        // breadcrumb, so log the Full case with the status's tone/key so the
        // operator can tell WHICH producer's update went missing. A Closed channel
        // means the engine is already gone (normal shutdown), so it stays silent.
        let tone = status.tone.clone();
        let key = status.key.clone();
        if let Err(mpsc::error::TrySendError::Full(_)) =
            self.req_tx.try_send(EngineRequest::EmitStatus(status))
        {
            dux_core::logger::warn(&format!(
                "engine request channel full: dropped a non-engine status update \
                 (tone={tone}, key={key:?})"
            ));
        }
    }

    /// Dismiss a keyed status without replacing it. The valid FINAL for an
    /// operation whose success is already visible on screen (see
    /// [`EngineRequest::ClearStatus`]); error paths must still post a real
    /// final. Same fire-and-forget semantics as [`Self::emit_status`].
    pub fn clear_status(&self, key: &str) {
        if let Err(mpsc::error::TrySendError::Full(_)) = self
            .req_tx
            .try_send(EngineRequest::ClearStatus(key.to_string()))
        {
            dux_core::logger::warn(&format!(
                "engine request channel full: dropped a status clear (key={key})"
            ));
        }
    }

    pub async fn apply_wire(&self, command: WireCommand) -> Result<WireCommandOutcome, String> {
        self.apply_wire_scoped(command, StatusScope::All).await
    }

    /// Like [`apply_wire`](Self::apply_wire) but tags the command with the
    /// originating connection's [`StatusScope`], so any statuses it mints (the
    /// synchronous outcome, deferred busies/finals, worker busies) are delivered
    /// only to that connection. `apply_wire` delegates here with
    /// [`StatusScope::All`] (broadcast), so existing callers are unchanged.
    pub async fn apply_wire_scoped(
        &self,
        command: WireCommand,
        origin: StatusScope,
    ) -> Result<WireCommandOutcome, String> {
        let (tx, rx) = oneshot::channel();
        self.req_tx
            .send(EngineRequest::ApplyWire(command, tx, origin))
            .await
            .map_err(|_| "engine thread gone".to_string())?;
        rx.await.map_err(|_| "engine reply dropped".to_string())?
    }

    /// Bump and return the next monotonic changed-files revision for `session_id`
    /// (one actor round-trip). The counter is persisted in SQLite, so it never
    /// resets across restarts. Returns `0` only if the engine thread is gone.
    pub async fn next_changes_rev(&self, session_id: String) -> u64 {
        let (tx, rx) = oneshot::channel();
        let sid = session_id.clone();
        if self
            .req_tx
            .send(EngineRequest::NextChangesRev(session_id, tx))
            .await
            .is_err()
        {
            // The engine thread is gone (shutdown). Returning the 0 fallback is
            // safe (the client's `rev >=` apply guard treats it as redundant), but
            // log it so a spurious non-advancing rev is explainable.
            dux_core::logger::warn(&format!(
                "next_changes_rev for session {sid}: engine thread gone; using rev 0 fallback"
            ));
            return 0;
        }
        match rx.await {
            Ok(rev) => rev,
            Err(_) => {
                dux_core::logger::warn(&format!(
                    "next_changes_rev for session {sid}: engine reply dropped; using rev 0 fallback"
                ));
                0
            }
        }
    }

    /// Whether any agent PTY is currently live, read as a local atomic load (no
    /// actor round-trip). The changed-files poller uses this to pick its cadence
    /// (2s when an agent is active, else 10s).
    pub fn has_active_processes(&self) -> bool {
        self.has_active_processes
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub async fn subscribe_pty(&self, session_id: String) -> Result<PtySubscription, String> {
        let (tx, rx) = oneshot::channel();
        self.req_tx
            .send(EngineRequest::SubscribePty(session_id, tx))
            .await
            .map_err(|_| "engine thread gone".to_string())?;
        rx.await.map_err(|_| "engine reply dropped".to_string())?
    }

    pub fn write_pty(&self, session_id: String, bytes: Vec<u8>) {
        // `try_send`: keystrokes are fire-and-forget from a sync caller. A full
        // channel means the producer is flooding far past the engine's drain
        // rate; shedding the write then is the intended overload behaviour (the
        // bounded channel + WS frame-size limit together cap memory).
        let _ = self
            .req_tx
            .try_send(EngineRequest::WritePty(session_id, bytes));
    }

    /// Enqueue a resize the owners lock has already stamped with `seq`, which
    /// travels along so the apply site can drop it if a later claim's geometry
    /// beat it to the child.
    pub fn resize_pty(&self, session_id: String, rows: u16, cols: u16, seq: u64) {
        // `try_send`: a resize dropped under overload is self-correcting (the next
        // resize re-establishes the size); no need to backpressure a sync caller.
        let _ = self
            .req_tx
            .try_send(EngineRequest::ResizePty(session_id, rows, cols, seq));
    }

    /// The PTY's current grid as `(rows, cols)`, or `None` when the id names no
    /// running PTY (or the engine is gone). Asked once per PTY-socket attach so
    /// the `connected` handshake can carry the geometry the child is drawing for.
    pub async fn pty_grid_size(&self, pty_id: String) -> Option<(u16, u16)> {
        let (tx, rx) = oneshot::channel();
        self.req_tx
            .send(EngineRequest::PtyGridSize(pty_id, tx))
            .await
            .ok()?;
        rx.await.ok().flatten()
    }

    pub fn note_viewed(&self, pty_id: String) {
        // `try_send`: a viewed ping is a periodic best-effort hint. Dropping one
        // under overload is harmless: the next ping (every ~2s while the tab is
        // foregrounded) re-stamps the engagement window.
        let _ = self.req_tx.try_send(EngineRequest::NoteViewed(pty_id));
    }

    pub async fn subscribe_terminal(&self, terminal_id: String) -> Result<PtySubscription, String> {
        let (tx, rx) = oneshot::channel();
        self.req_tx
            .send(EngineRequest::SubscribeTerminal(terminal_id, tx))
            .await
            .map_err(|_| "engine thread gone".to_string())?;
        rx.await.map_err(|_| "engine reply dropped".to_string())?
    }

    pub async fn create_terminal(&self, session_id: String) -> Result<(String, String), String> {
        let (tx, rx) = oneshot::channel();
        self.req_tx
            .send(EngineRequest::CreateTerminal(session_id, tx))
            .await
            .map_err(|_| "engine thread gone".to_string())?;
        rx.await.map_err(|_| "engine reply dropped".to_string())?
    }

    /// Create a project terminal (a plain shell at the project's repo root with
    /// no agent attached), replying `(terminal_id, label)`. Mirrors
    /// `create_terminal`.
    pub async fn create_project_terminal(
        &self,
        project_id: String,
    ) -> Result<(String, String), String> {
        let (tx, rx) = oneshot::channel();
        self.req_tx
            .send(EngineRequest::CreateProjectTerminal(project_id, tx))
            .await
            .map_err(|_| "engine thread gone".to_string())?;
        rx.await.map_err(|_| "engine reply dropped".to_string())?
    }

    /// Create a standalone terminal (a plain shell in the user's home directory
    /// owned by nothing), replying `(terminal_id, label)`. Takes no owner id,
    /// which is the whole point of the kind.
    pub async fn create_standalone_terminal(&self) -> Result<(String, String), String> {
        let (tx, rx) = oneshot::channel();
        self.req_tx
            .send(EngineRequest::CreateStandaloneTerminal(tx))
            .await
            .map_err(|_| "engine thread gone".to_string())?;
        rx.await.map_err(|_| "engine reply dropped".to_string())?
    }

    /// Create an extra tab for `session_id` (provider `None` → project default),
    /// replying `(tab_id, provider)`. Direct-return, mirroring `create_terminal`.
    pub async fn create_agent_tab(
        &self,
        session_id: String,
        provider: Option<String>,
    ) -> Result<(String, String), String> {
        let (tx, rx) = oneshot::channel();
        self.req_tx
            .send(EngineRequest::CreateAgentTab(session_id, provider, tx))
            .await
            .map_err(|_| "engine thread gone".to_string())?;
        rx.await.map_err(|_| "engine reply dropped".to_string())?
    }

    /// Start dormant tab `tab_id` explicitly (the "Start session" press). See
    /// [`EngineRequest::StartAgentTab`] for why this exists alongside the
    /// subscribe-launch path.
    pub async fn start_agent_tab(&self, tab_id: String) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.req_tx
            .send(EngineRequest::StartAgentTab(tab_id, tx))
            .await
            .map_err(|_| "engine thread gone".to_string())?;
        rx.await.map_err(|_| "engine reply dropped".to_string())?
    }

    /// The session id that owns EXTRA tab `tab_id`, or `None` when the tab is
    /// unknown or is a session-slot tab. Used by the tab PTY socket and the tab REST routes
    /// to enforce that the tab belongs to the path's session before acting.
    pub async fn tab_session(&self, tab_id: String) -> Option<String> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::TabSession(tab_id, tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.unwrap_or(None)
    }

    /// The id of `session_id`'s session-slot tab (its first tab), or `None` when
    /// the session is unknown. The routing key for that tab's PTY, and the one
    /// way the HTTP and socket layers learn it.
    pub async fn slot_tab_id(&self, session_id: String) -> Option<String> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::SlotTabId(session_id, tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.unwrap_or(None)
    }

    /// Whether `tab_id` names `session_id`'s session-slot tab. An unknown session
    /// answers `false`: it has no slot tab to name.
    pub async fn is_slot_tab(&self, session_id: String, tab_id: &str) -> bool {
        self.slot_tab_id(session_id).await.as_deref() == Some(tab_id)
    }

    /// The create-agent branch preflight for `(project_id, name)`: whether a new
    /// agent of that name would start fresh or attach to an existing branch.
    /// `None` when the project is unknown or the engine thread is gone. The create
    /// route uses this to refuse an unconfirmed existing-branch attach with a
    /// confirmable 409 (the web's half of the "no silent attach" tenet).
    pub async fn create_agent_branch_plan(
        &self,
        project_id: String,
        name: String,
    ) -> Option<dux_core::git::CreateAgentBranchPlan> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::CreateAgentBranchPlan(project_id, name, tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.unwrap_or(None)
    }

    /// The owner (session or project) of companion terminal `terminal_id`, or
    /// `None` when the terminal is unknown or the engine thread is gone. The
    /// nested terminal PTY sockets and the `DELETE .../terminals/:tid` routes use
    /// this to enforce that the terminal belongs to the path's owner before
    /// acting on it.
    pub async fn terminal_owner_of(&self, terminal_id: String) -> Option<TerminalOwner> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::TerminalOwnerOf(terminal_id, tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.unwrap_or(None)
    }

    /// A terminal's owner and the absolute directory it was spawned in: the root
    /// a terminal-rooted editor serves from. `None` when the terminal is unknown
    /// or the engine thread is gone. The owner rides along so the caller can put
    /// the terminal's real owner and the address it was asked at through
    /// `TerminalOwner::is_at_route` before serving anything.
    pub async fn terminal_root(
        &self,
        terminal_id: String,
    ) -> Option<(TerminalOwner, std::path::PathBuf)> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::TerminalRoot(terminal_id, tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.unwrap_or(None)
    }

    /// Gracefully wind down the engine: SIGTERM the agent/terminal children so
    /// CLIs can save state for a later resume, then stop the engine thread.
    /// Errors are ignored — if the thread is already gone, shutdown has already
    /// happened.
    pub async fn shutdown(&self) {
        let (tx, rx) = oneshot::channel();
        if self.req_tx.send(EngineRequest::Shutdown(tx)).await.is_ok() {
            let _ = rx.await;
        }
    }

    /// The runtime PTY key a pane's addressed `pane_id` names: a terminal id or
    /// a tab id, with the bare per-agent spelling of a slot tab resolved to that
    /// tab. `None` when the id names nothing or the engine thread is gone.
    ///
    /// A route holding a pane id resolves it HERE, once, before asking anything
    /// else about it, so a lookup against a runtime map keyed by the real pty
    /// (input ownership, say) and a lookup that resolves the id itself cannot
    /// answer about two different things.
    pub async fn pty_key_for_pane_id(&self, pane_id: String) -> Option<String> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::PtyKeyForPaneId(pane_id, tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.unwrap_or(None)
    }

    /// Resolve `pty_id` to a file-drop destination. `None` when the pty id is
    /// unknown or the engine thread is gone.
    pub async fn file_drop_destination(
        &self,
        pty_id: String,
    ) -> Option<dux_core::file_drop::FileDropDestination> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::FileDropDestination(pty_id, tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.unwrap_or(None)
    }

    /// Resolve `pty_id` plus a root-relative directory to an editor file-tree
    /// drop destination: an agent's worktree, or a terminal's pinned spawn
    /// directory. `None` when the pty id is unknown or the engine thread is
    /// gone.
    pub async fn file_drop_tree_destination(
        &self,
        pty_id: String,
        dir: String,
    ) -> Option<dux_core::file_drop::FileDropDestination> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::FileDropTreeDestination(pty_id, dir, tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.unwrap_or(None)
    }

    /// The agent pane whose changed files a drop on `pty_id` could affect, as
    /// `(session id, worktree)`. `None` when no agent is behind the pane or the
    /// engine thread is gone.
    pub async fn file_drop_refresh_target(
        &self,
        pty_id: String,
    ) -> Option<(String, std::path::PathBuf)> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::FileDropRefreshTarget(pty_id, tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.unwrap_or(None)
    }

    pub async fn session_git_access(
        &self,
        session_id: String,
    ) -> Option<dux_core::engine::SessionGitAccess> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::SessionGitAccess(session_id, tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.unwrap_or(None)
    }

    /// See [`EngineRequest::SessionBranchDeleteInputs`].
    pub async fn session_branch_delete_inputs(
        &self,
        session_id: String,
    ) -> Option<dux_core::engine::BranchDeleteInputs> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::SessionBranchDeleteInputs(session_id, tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.unwrap_or(None)
    }

    pub async fn session_worktree(&self, session_id: String) -> Option<String> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::SessionWorktree(session_id, tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.unwrap_or(None)
    }

    /// The repo-root path of project `project_id`, or `None` when the project is
    /// unknown or the engine thread is gone.
    pub async fn project_path(&self, project_id: String) -> Option<String> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::ProjectPath(project_id, tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.unwrap_or(None)
    }

    /// Snapshot the process trees the resource monitor should sample. `None` if
    /// the engine is gone (the handler then returns 503), distinguishing a dead
    /// engine from a real "nothing is running" empty list.
    pub async fn resource_targets(&self) -> Option<Vec<dux_core::worker::ResourceTarget>> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::ResourceTargets(tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.ok()
    }

    /// Snapshot the build-/config-static bootstrap projection for
    /// `GET /api/v1/bootstrap`. `None` if the engine is gone (the handler then
    /// returns 503), distinguishing a dead engine from a real empty payload.
    pub async fn bootstrap(&self) -> Option<dux_core::viewmodel::BootstrapView> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::Bootstrap(tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.ok()
    }

    /// Snapshot the projects/sessions/sidebar spine for the thin per-resource
    /// reads (`/api/v1/projects`, `/api/v1/sessions`). `None` if the engine is gone (the handler then
    /// returns 503), distinguishing a dead engine from a real empty payload.
    pub async fn spine(&self) -> Option<dux_core::viewmodel::SpineView> {
        let (tx, rx) = oneshot::channel();
        if self.req_tx.send(EngineRequest::Spine(tx)).await.is_err() {
            return None;
        }
        rx.await.ok()
    }

    /// The pre-serialized workspace document for `GET /api/v1/workspace`, served
    /// from the loop's cache with its `rev` already inside it. The same bytes the
    /// push frame carries. `None` if the engine is gone (the handler then returns
    /// 503), distinguishing a dead engine from a real payload.
    pub async fn spine_json(&self) -> Option<String> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::SpineJson(tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.ok()
    }

    /// Project ONLY the session with `id` for `GET /api/v1/sessions/:id`, with the
    /// terminals that session owns. The outer `Option` is `None` if the engine is
    /// gone (503); the inner `None` means the session id is unknown (404).
    pub async fn session(
        &self,
        id: String,
    ) -> Option<
        Option<(
            dux_core::viewmodel::SessionView,
            Vec<dux_core::viewmodel::TerminalView>,
        )>,
    > {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::Session(id, tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.ok()
    }

    /// Manually attach (pin) a pull request to a session from the raw typed
    /// reference. Returns the keyed status op id the REST handler echoes in its
    /// `202` body; the outcome rides the status toast stream. A synchronous
    /// refusal (gh unavailable, empty reference, unknown session) is `Err` with
    /// the engine's message.
    pub async fn attach_pull_request(
        &self,
        session_id: String,
        raw: String,
        scope: StatusScope,
    ) -> Result<String, String> {
        let (tx, rx) = oneshot::channel();
        self.req_tx
            .send(EngineRequest::AttachPullRequest(session_id, raw, scope, tx))
            .await
            .map_err(|_| "engine thread gone".to_string())?;
        rx.await.map_err(|_| "engine reply dropped".to_string())?
    }

    /// Resolve the session id produced by create op `op_id` (returned in
    /// `WireCommandOutcome.created_op_id`). `None` while the create is still in
    /// flight, the entry has expired, or the engine thread is gone.
    pub async fn created_session_for_op(&self, op_id: String) -> Option<String> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::CreatedSessionForOp(op_id, tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.ok().flatten()
    }

    /// Subscribe to config-reload notifications. The web layer forwards each into a
    /// `config.changed` event on its event bus so subscribed clients refetch
    /// `/api/v1/bootstrap`.
    pub fn subscribe_config_reloads(&self) -> broadcast::Receiver<()> {
        self.config_reload_tx.subscribe()
    }

    /// Subscribe to projects/sessions spine-change notifications. The web layer
    /// forwards each into a coarse `projects.changed` / `sessions.changed` event
    /// on its event bus. The document those events describe is pushed separately
    /// (see [`EngineHandle::workspace_docs`]); a client that cannot read the push
    /// refetches `/api/v1/workspace` on the event instead.
    pub fn subscribe_spine_changes(&self) -> broadcast::Receiver<SpineChange> {
        self.spine_change_tx.subscribe()
    }

    /// The shared per-PTY input-ownership registry (see the field doc). The
    /// router clones this into `AppState` so the PTY socket handlers, the
    /// file-drop courtesy check, and the loop's spine overlay all operate on
    /// the one map.
    pub(crate) fn pty_input_owners(&self) -> Arc<PtySizeOwners> {
        Arc::clone(&self.pty_input_owners)
    }

    /// The `[server]` limits a config reload can move without a restart. The
    /// router clones this into `AppState`; the actor stores each reload into it.
    pub fn live_limits(&self) -> Arc<LiveServerLimits> {
        Arc::clone(&self.live_limits)
    }

    /// The configured preferred editor name for the "open in editor" action
    /// (`config.editor.default`). Empty if the engine is gone — the handler then
    /// falls back to the first detected editor.
    pub async fn editor_default(&self) -> String {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::EditorDefault(tx))
            .await
            .is_err()
        {
            return String::new();
        }
        rx.await.unwrap_or_default()
    }

    /// The directory the add-project picker should open at, resolved from the
    /// live config. `None` if the engine is gone — the browse handler then falls
    /// back to `$HOME` on its own.
    pub async fn browse_start_dir(&self) -> Option<String> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::BrowseStartDir(tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.ok()
    }

    /// Fire-and-forget: ask the engine to recompute changed-files for `worktree`
    /// (after an HTTP git mutation). The refreshed lists are served by the REST
    /// changed-files read; nothing to await here.
    pub fn refresh_changed_files(&self, worktree: String) {
        #[cfg(test)]
        self.refresh_requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(worktree.clone());
        // `try_send`: a dropped refresh under overload self-heals — the periodic
        // changed-files poller recomputes the lists on its next pass regardless.
        let _ = self
            .req_tx
            .try_send(EngineRequest::RefreshChangedFiles(worktree));
    }

    /// The worktrees this handle was asked to recompute, in call order.
    /// Test-only (see the field).
    #[cfg(test)]
    pub(crate) fn refresh_requests(&self) -> Vec<String> {
        self.refresh_requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Snapshot the inputs to classify a project's managed worktrees (project,
    /// paths, sessions). Instant — the git classification runs off-thread in the
    /// caller. `None` when the project id is unknown.
    #[allow(clippy::type_complexity)]
    pub async fn project_worktree_inputs(
        &self,
        project_id: String,
    ) -> Option<(
        dux_core::model::Project,
        dux_core::config::DuxPaths,
        Vec<dux_core::model::AgentSession>,
    )> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::ProjectWorktreeInputs(project_id, tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.unwrap_or(None)
    }

    /// The project list, host policy and gh-availability flag the pull-request
    /// reference resolver needs. See
    /// [`EngineRequest::PullRequestResolutionInputs`].
    pub async fn pull_request_resolution_inputs(
        &self,
    ) -> Option<(
        Vec<dux_core::model::Project>,
        dux_core::gh::GithubHostPolicy,
        bool,
    )> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::PullRequestResolutionInputs(tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.ok()
    }

    /// Resolve a session's startup-command-log context: the dux paths and the
    /// session's owning project id. Instant lookup — the log directory listing /
    /// file read runs off-thread in the caller (the `project_worktree_inputs`
    /// precedent). `None` when the session id is unknown.
    pub async fn session_startup_log_context(
        &self,
        session_id: String,
    ) -> Option<(dux_core::config::DuxPaths, String)> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::SessionStartupLogContext(session_id, tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.unwrap_or(None)
    }

    /// Resolve a project's startup-command-log context: the dux paths, present
    /// only when the project id is known. Instant lookup, mirroring
    /// [`EngineHandle::session_startup_log_context`]; the directory listing / file
    /// read runs off-thread in the caller. `None` when the project id is unknown.
    pub async fn project_startup_log_context(
        &self,
        project_id: String,
    ) -> Option<dux_core::config::DuxPaths> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::ProjectStartupLogContext(project_id, tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.unwrap_or(None)
    }

    /// Read the raw `config.toml` text for the Monaco config editor (or the plain
    /// render of the running config if the file is missing). `Err` on a read
    /// failure or a dead engine thread, so the editor never opens on blank
    /// content the user could save over their real config.
    pub async fn read_raw_config(&self) -> Result<String, String> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::ReadRawConfig(tx))
            .await
            .is_err()
        {
            return Err("the engine is not available".to_string());
        }
        rx.await
            .unwrap_or_else(|_| Err("the engine did not reply".to_string()))
    }

    /// Validate and write raw `config.toml` text from the Monaco editor. Returns
    /// `Err(message)` for invalid TOML, an IO failure, or a dead engine thread.
    pub async fn write_raw_config(&self, content: String) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::WriteRawConfig(content, tx))
            .await
            .is_err()
        {
            return Err("the engine is not available".to_string());
        }
        rx.await
            .unwrap_or_else(|_| Err("the engine did not reply".to_string()))
    }

    /// Read the inputs `dux_core::first_load::plan` needs. `None` when the engine
    /// thread is gone, in which case the caller shows no screen at all rather
    /// than guessing (and, critically, stamps nothing).
    pub async fn first_load_inputs(&self) -> Option<FirstLoadInputs> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::FirstLoadInputs(tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.ok()
    }

    /// Record `version` as seen. This is the write that makes a dismissal shared
    /// between the web UI and the TUI: one SQLite row, read by both.
    pub async fn mark_version_seen(&self, version: String) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::MarkVersionSeen(version, tx))
            .await
            .is_err()
        {
            return Err("the engine is not available".to_string());
        }
        rx.await
            .unwrap_or_else(|_| Err("the engine did not reply".to_string()))
    }
}

/// Everything the first-load gate needs, read off the engine thread in one
/// round-trip. See [`EngineRequest::FirstLoadInputs`].
#[derive(Clone, Debug)]
pub struct FirstLoadInputs {
    /// The version recorded in SQLite; `None` on a first ever launch.
    pub last_seen: Option<String>,
    /// This build's display version (`"development"` for a non-release build).
    pub running: String,
    pub disable_welcome: bool,
    pub disable_release_notes: bool,
    /// The dux state directory, where the release-notes cache file lives.
    pub state_root: std::path::PathBuf,
}

/// Spawn the engine thread. Returns a handle and the thread's join handle.
///
/// This is the dedicated-thread server path (`dux server`): the engine lives on
/// its own std thread for its whole life. The channel setup, worker spawns, and
/// loop body are shared with the in-process flip ([`crate::serve_with_engine`])
/// via [`build_actor_channels`], [`spawn_global_workers`], and
/// [`run_engine_loop`] — this path simply runs the loop on a spawned thread and
/// drops the returned engine. The control closure always returns `Continue`, so
/// the loop's only exit is the inline `Shutdown` request, exactly as before.
/// Relaunch agents with auto-reopen intent, once, at server startup. The
/// headless counterpart of the TUI's `auto_reopen_eligible_sessions`: the
/// ELIGIBILITY rule is core-owned (`Engine::auto_reopen_candidates`, the same
/// matrix the TUI applies), and every candidate launches through the shared
/// chokepoint (`build_agent_launch_request` with
/// `AgentLaunchKind::StartupAutoReopen`, dispatched via
/// `Command::DispatchAgentLaunch`), never a hand-rolled spawn. The wire drain
/// already understands the kind: success is silent (mirroring the TUI) and a
/// failure surfaces through the shared launch-failed path.
///
/// Runs only on the fresh-server path (`spawn_engine_thread`, i.e. `dux
/// serve`): the TUI-flip path hands over a LIVE engine whose agents are
/// already in their intended state, so re-running the pass there would be
/// wrong. Must run AFTER `bootstrap_engine`, whose
/// `normalize_restored_sessions` settles restored statuses first, mirroring
/// the TUI's restore-then-reopen ordering. No client is connected this early,
/// so the user-facing signal is a log line rather than a keyed status; the
/// per-launch statuses ride the normal worker-event drain once the loop runs.
/// Returns the number of launches actually dispatched.
pub(crate) fn auto_reopen_agents_on_startup(engine: &mut Engine) -> usize {
    let candidates = engine.auto_reopen_candidates();
    if candidates.is_empty() {
        return 0;
    }
    dux_core::logger::info(&format!(
        "Auto-reopening {} agent(s) that were running when dux last exited...",
        candidates.len()
    ));
    let mut launched = 0;
    for session in candidates {
        let id = session.id.clone();
        let request = engine.build_agent_launch_request(
            session,
            true,
            (24, 80),
            AgentLaunchKind::StartupAutoReopen,
        );
        match engine.apply(Command::DispatchAgentLaunch {
            request: Box::new(request),
        }) {
            // The chokepoint refuses (closing session, in-flight collision) by
            // returning `launched: false`, not an `Err`; log it like the
            // subscribe path does rather than counting it as a launch.
            Ok(EventReaction::DispatchAgentLaunchView(view)) if view.launched => launched += 1,
            Ok(EventReaction::DispatchAgentLaunchView(view)) => {
                let message = view
                    .status
                    .as_ref()
                    .map(|s| s.message.clone())
                    .unwrap_or_else(|| "launch refused".to_string());
                dux_core::logger::info(&format!("Auto-reopen skipped for {id}: {message}"));
            }
            Ok(_) => launched += 1,
            Err(err) => {
                dux_core::logger::info(&format!("Auto-reopen dispatch failed for {id}: {err}"));
            }
        }
    }
    launched
}

pub fn spawn_engine_thread(mut engine: Engine) -> (EngineHandle, JoinHandle<()>) {
    let (handle, ends) = build_actor_channels(&engine);
    spawn_global_workers(&mut engine);
    // Startup auto-reopen: dispatched before the loop starts so the loop's very
    // first worker-event drain picks up the launches' results.
    auto_reopen_agents_on_startup(&mut engine);

    let join = thread::spawn(move || {
        // The dedicated thread never asks the loop to exit; the loop stops only
        // on the `Shutdown` request handled inline. The returned engine is
        // dropped here (thread end), exactly as the previous implementation did.
        let _engine = run_engine_loop(engine, ends, ShutdownEcho::Stderr, || LoopControl::Continue);
    });

    (handle, join)
}

/// Whether handling `req` can change the projected spine (the projects /
/// sessions / sidebar snapshot that [`fingerprint_halves`] serializes), and so
/// must bump `mutation_version` to open the change-gated spine check.
///
/// EXHAUSTIVE ON PURPOSE, with no wildcard arm. A wildcard arm would compile no
/// matter what is added to [`EngineRequest`] and silently answer "changed
/// nothing" for everything new, so a new kind's change would reach the browser
/// only when the ~2s self-healing backstop noticed. With no wildcard, a new
/// variant does not build until somebody says which answer it deserves.
///
/// The two errors are not symmetric, so lean one way. Answering `false` for a
/// real mutator is the BUG this shape exists to prevent (the browser is told
/// late, or the fingerprint compare never runs at all in that window).
/// Answering `true` for something that turns out to be a read costs one extra
/// fingerprint serialize per ~250ms interval and nothing else. Where the answer
/// is not obvious from the handler, answer `true` and say so on the arm.
fn request_mutates_spine(req: &EngineRequest) -> bool {
    match req {
        // Writers. `ApplyWire` is the named loop chokepoint (every wire command
        // the web can issue). The three terminal creates and the tab create each
        // add a row the sidebar renders. `SubscribePty` is a writer too, and was
        // the omission that proved the point: opening an agent's socket launches
        // the provider when it is not already up (`handle_subscribe` ->
        // `launch_agent`), which flips the session live, and it also spawns a PR
        // check and stamps the viewed/attention state.
        EngineRequest::ApplyWire(..)
        | EngineRequest::SubscribePty(..)
        | EngineRequest::CreateTerminal(..)
        | EngineRequest::CreateProjectTerminal(..)
        | EngineRequest::CreateStandaloneTerminal(..)
        | EngineRequest::CreateAgentTab(..)
        // Starting a dormant tab dispatches a launch and clears its recorded
        // failure, both of which the spine publishes.
        | EngineRequest::StartAgentTab(..) => true,

        // The attach dispatch itself only mints a pending status op and spawns
        // the lookup worker, but the worker's result lands as an engine event
        // that pins the PR onto the session view (`SessionView.pr`), so lean
        // `true` per the asymmetry note above.
        EngineRequest::AttachPullRequest(..) => true,

        // Writers of the per-tab activity/attention state that the spine
        // projects. `WritePty` calls `note_pty_input`, which is what lights the
        // `typing` flag on `SessionView` / `TerminalView` / the tab views;
        // `NoteViewed` calls `note_agent_viewed_if_known`, which clears
        // `needs_attention`. Both are also observed by the per-tick
        // `poll_attention_transitions` / streaming polls, so answering `true`
        // here mostly makes the same-tick case prompt rather than next-tick.
        EngineRequest::WritePty(..) | EngineRequest::NoteViewed(..) => true,

        // Tears down every PTY and marks the agent sessions Detached, so it is a
        // mutator by any reading. Handled inline in the loop, which then breaks
        // out, so this answer is never actually consulted; it is stated
        // truthfully rather than left as a convenient `false`.
        EngineRequest::Shutdown(..) => true,

        // Pure projections and instant clones off engine state: `Spine`,
        // `Session`, `Bootstrap`, `ResourceTargets` and `CreatedSessionForOp`
        // all call `&self` methods on `Engine`; `SpineJson` serves the loop's
        // cached string. Nothing here can write.
        EngineRequest::Spine(..)
        | EngineRequest::SpineJson(..)
        | EngineRequest::Session(..)
        | EngineRequest::Bootstrap(..)
        | EngineRequest::ResourceTargets(..)
        | EngineRequest::CreatedSessionForOp(..) => false,

        // Lookups: each arm is an `iter().find()` or a `HashMap::get` followed by
        // a clone of what it found (worktree path, repo root, terminal owner, tab
        // owner, project/session log context, worktree inputs). Read-only by
        // construction.
        EngineRequest::TerminalOwnerOf(..)
        | EngineRequest::TerminalRoot(..)
        | EngineRequest::TabSession(..)
        | EngineRequest::SlotTabId(..)
        | EngineRequest::SessionWorktree(..)
        | EngineRequest::SessionBranchDeleteInputs(..)
        | EngineRequest::SessionGitAccess(..)
        | EngineRequest::PtyKeyForPaneId(..)
        | EngineRequest::FileDropDestination(..)
        | EngineRequest::FileDropTreeDestination(..)
        | EngineRequest::FileDropRefreshTarget(..)
        | EngineRequest::ProjectPath(..)
        | EngineRequest::ProjectWorktreeInputs(..)
        | EngineRequest::SessionStartupLogContext(..)
        | EngineRequest::ProjectStartupLogContext(..)
        | EngineRequest::EditorDefault(..)
        | EngineRequest::BrowseStartDir(..) => false,

        // Attach to an ALREADY-RUNNING PTY, or push bytes at one. Deliberately
        // distinct from `SubscribePty`: `SubscribeTerminal` never launches
        // anything (an unknown terminal id is an error, not a spawn), and
        // `ResizePty` resolves the client through `pty_for`, which borrows the
        // engine immutably. Neither adds, removes, or restates a spine row.
        EngineRequest::SubscribeTerminal(..) | EngineRequest::ResizePty(..) => false,

        // A read of a live PTY's grid through the same immutable `pty_for`
        // borrow `ResizePty` uses. It clones two integers out from under the
        // terminal lock and writes nothing anywhere.
        EngineRequest::PtyGridSize(..) => false,

        // Read-only git. `create_agent_branch_preflight` is a `branch_exists`
        // probe answering "fresh or existing branch?"; it writes nothing.
        EngineRequest::CreateAgentBranchPlan(..) => false,

        // A pure read of what the caller needs to resolve a typed pull request
        // reference: the project list, the GitHub host policy, and whether the
        // from-PR command is available. It clones all three and sends them back.
        // The per-project git calls happen on the CALLER's worker, not here.
        EngineRequest::PullRequestResolutionInputs(..) => false,

        // Writes that land somewhere the spine does not project. The changed-files
        // revision counter and the last-seen version are SQLite rows that no
        // `SpineView` field reads; the raw config save is persist-only by design
        // (`write_raw_config_on_engine` writes the file and leaves `engine.config`
        // untouched until an explicit reload). `RefreshChangedFiles` only spawns a
        // worker (`&self`), and the refreshed lists arrive through the worker-event
        // drain, which bumps the version itself.
        EngineRequest::NextChangesRev(..)
        | EngineRequest::MarkVersionSeen(..)
        | EngineRequest::ReadRawConfig(..)
        | EngineRequest::WriteRawConfig(..)
        | EngineRequest::FirstLoadInputs(..)
        | EngineRequest::RefreshChangedFiles(..) => false,

        // Broadcast on the status channels only. Statuses are their own transport
        // (toasts on the web); no spine field carries them.
        EngineRequest::EmitStatus(..) | EngineRequest::ClearStatus(..) => false,
    }
}

/// The browser-facing notice for one reaped PTY, or `None` where the reaping is
/// its own announcement.
///
/// Shared by the web layer's own maintenance sweep and by
/// [`EngineService::note_drained_maintenance`], which stands in for it when the
/// terminal UI is the surface that swept. One builder so the two paths cannot
/// drift into telling a browser different things about the same exit.
///
/// `None` is the answer whenever the row itself LEAVES the screen in the same
/// sweep: the strip and the sidebar are what the user is looking at, and a toast
/// restating a disappearance they just watched is noise. What survives is the
/// case where something REMAINS and nothing else says why: a dormant tab whose
/// row is still sitting there, and the workspace-level warning that the whole
/// agent detached.
fn prune_wire_status(pruned: &dux_core::engine::PrunedPty) -> Option<WireStatus> {
    match pruned.kind {
        // A last-tab exit detaches the whole agent, which is a workspace-level
        // event worth a warning. A tab exit that leaves siblings running is
        // routine and scoped: never the loud "Agent exited" warning (which would
        // falsely imply the agent died).
        PrunedPtyKind::Agent if pruned.agent_detached => Some(WireStatus::new(
            "warning",
            format!("Agent \"{}\" exited.", pruned.label),
        )),
        // The tab closed itself on a clean exit, taking its pill out of the
        // strip. The strip is the announcement.
        PrunedPtyKind::Agent if pruned.tab_closed => None,
        // The tab stays, dormant. Nothing else on screen distinguishes a pill
        // whose process just ended from one that was never launched, so this
        // sentence is the only word the user gets.
        PrunedPtyKind::Agent => Some(WireStatus::new(
            "info",
            format!("Tab ({}) exited.", pruned.label),
        )),
        // The terminal's row is gone from the sidebar in the same breath.
        PrunedPtyKind::Terminal => None,
    }
}

/// How a drained reaction's web-side follow-ups are routed.
///
/// The follow-up BODIES are identical either way; this only decides which of
/// them are allowed to run. It exists because a process can now have two
/// surfaces reading one worker-event stream, and roughly four reactions carry a
/// follow-up that DOES something rather than merely reporting something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FollowupRouting {
    /// Run every follow-up that matches the reaction. What a process with one
    /// surface wants: `dux server` and the flip are the only drainer, so every
    /// reaction they see is theirs and there is nobody else to defer to.
    RunEverything,
    /// Run only the follow-ups [`Engine::followup_owner`] assigns to the web.
    /// The terminal UI is the drainer in this mode and holds the other half of
    /// those reactions, so an unrouted follow-up would run on both surfaces.
    ///
    /// Measured, so the difference is not mysterious: the two modes diverge only
    /// for a routed reaction whose op id is absent from the web pending-op maps,
    /// and every web dispatch site forwards a real op id into its worker, so
    /// `dux server` and the flip cannot tell the two apart. `RunEverything` is
    /// nonetheless what they pass, because their equivalence is a property of
    /// today's call sites rather than of this function.
    ByOrigin,
}

/// What one call to [`EngineService::drain_requests`] did.
struct DrainOutcome {
    /// At least one drained request could move the spine, per the exhaustive
    /// [`request_mutates_spine`].
    mutated: bool,
    /// The request channel closed, or an inline `Shutdown` request asked the loop
    /// to stop.
    stopped: bool,
}

/// Whether the request drain may echo shutdown progress to stderr.
///
/// `dux server` owns its terminal and an operator running it in the foreground
/// should see the agents winding down. Every other serve path shares the terminal
/// with a themed dux-tui screen, where a raw line lands wherever the cursor
/// happens to sit; those paths stay silent and rely on `dux.log`.
///
/// This flag is about PRINTING, not about the `Shutdown` request itself, which
/// SIGTERMs every agent wherever it arrives. Only `EngineHandle::shutdown` sends
/// one and only `run_plain_http` calls it, so the flip and the background server
/// cannot receive one today. Worth knowing if that ever changes: on the
/// background-server path a `Shutdown` would wind down the agents of a terminal
/// UI that never asked, so a new sender needs a reason and a gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShutdownEcho {
    Stderr,
    Silent,
}

/// Everything the engine loop keeps between iterations, so one iteration's worth
/// of web-side work can also be driven from the terminal UI's own run loop.
///
/// The split follows one rule: the shared maintenance sweeps have exactly ONE
/// runner per process, and that runner is whoever drains `worker_rx`. In
/// `dux server` and the flip that is [`run_engine_loop`], which therefore calls
/// [`Self::run_maintenance`]. While the background server runs behind a live TUI,
/// the TUI drains and sweeps, and the only thing left for the web layer is
/// [`Self::service_engine_once`]: resolve pending subscribes, check the spine,
/// drain requests, retire timed-out statuses.
pub(crate) struct EngineService {
    req_rx: mpsc::Receiver<EngineRequest>,
    /// Route every status through the shared `KeyedStatusController` so the web
    /// gets the SAME auto-clear + pending→final behaviour as the TUI from one
    /// place. The emitter upserts by key, broadcasts each status LIVE, and keeps
    /// the Vec snapshot watch in sync so connecting clients see all open toasts
    /// at once. [`Self::tick_statuses`] upgrades a stale Busy→Warning and drops
    /// finals that have aged past `FINAL_REPLAY_WINDOW`. It takes no
    /// `status_clear_seconds`: under `StatusRetention::Emit` how long a toast
    /// stays on SCREEN belongs to the browser, and the only server-side lifetime
    /// left is the fixed replay window.
    status: StatusEmitter,
    config_reload_tx: broadcast::Sender<()>,
    spine_change_tx: broadcast::Sender<SpineChange>,
    workspace_tx: watch::Sender<Option<Arc<WorkspaceDoc>>>,
    shutdown_flag: Arc<AtomicBool>,
    pty_input_owners: Arc<PtySizeOwners>,
    /// The limits every route reads per request; each successful reload stores
    /// its `[server]` values here.
    live_limits: Arc<LiveServerLimits>,
    /// Subscribes waiting for their provider to come up via the worker-event
    /// drain.
    pending: Vec<PendingSubscribe>,
    /// The session most recently brought to the foreground via a PTY subscribe.
    /// Workspace-global (single-tenant server), last-subscribe-wins across
    /// browser clients — deliberately NOT the engine's `watched_session_id` (that
    /// is the TUI's changed-files watch). Gates the tight foreground PR check so a
    /// socket reconnect for an already-focused agent doesn't re-fire it (only a
    /// real focus change does).
    last_pr_foregrounded: Option<String>,
    /// Change-gated spine check (fingerprints, cached `/spine` JSON, backstop
    /// accumulator). Seeded from the current state so the first tick does not emit
    /// a spurious change for an unchanged spine and a `/spine` read before the
    /// first change still serves a valid body.
    spine_check: SpineCheck,
    /// In-memory spine mutation version: bumped after each spine mutator (a wire
    /// apply or a `CreateTerminal` request, a worker-event drain, a changed
    /// terminal-foreground refresh, a non-empty PTY prune, and, while the
    /// background server runs, anything the terminal UI applied). The spine check
    /// runs the serialize only when this (or `streaming_version`) moved since its
    /// last pass, so idle ticks cost nothing.
    mutation_version: u64,
    /// In-memory streaming-transition version: bumped whenever any agent's
    /// time-derived `is_agent_streaming()` flag flips (see
    /// `poll_streaming_transitions`), which a mutation counter cannot observe.
    streaming_version: u64,
    /// Per-agent last-seen streaming flag, carried across ticks so transitions can
    /// be detected O(1) without re-deriving history each tick.
    prev_streaming: std::collections::HashMap<String, bool>,
    /// Per-tick snapshot of the engine's needs-attention set, carried across ticks
    /// so a set/clear can bump the spine-change gate promptly instead of waiting
    /// for the ~2s backstop (see `poll_attention_transitions`).
    prev_attention: std::collections::HashSet<TabId>,
    /// Tick counter for throttling the spine fingerprint/cache check (see
    /// `SPINE_CHECK_TICK_INTERVAL`) so it is evaluated ~every 250ms rather than
    /// every tick.
    tick_count: u64,
    /// True between a raw `config.toml` "Save" (disk written, not yet adopted) and
    /// the next reconcile (an explicit reload, or the reconcile a config-static
    /// mutation performs). Drives the clobber-safe reconcile in `handle_request`.
    config_disk_ahead: bool,
    shutdown_echo: ShutdownEcho,
    /// The serve's live Tailscale-mode handle, empty only when nothing is
    /// serving. Every serve fills it, background mode included; background mode
    /// simply does not run this loop, so its reload is the terminal UI's.
    tailscale_mode_control: Arc<std::sync::OnceLock<crate::serve_legs::TailscaleModeControl>>,
}

impl EngineService {
    pub(crate) fn new(engine: &Engine, ends: ActorLoopEnds, shutdown_echo: ShutdownEcho) -> Self {
        let ActorLoopEnds {
            req_rx,
            status_tx,
            status_clear_tx,
            status_snapshot_tx,
            config_reload_tx,
            spine_change_tx,
            workspace_tx,
            shutdown_flag,
            pty_input_owners,
            live_limits,
            tailscale_mode_control,
        } = ends;
        let spine_check = SpineCheck::new(engine, &pty_input_owners, &workspace_tx);
        Self {
            req_rx,
            status: StatusEmitter::new(
                status_tx,
                status_clear_tx,
                status_snapshot_tx,
                engine.live_status_keys.clone(),
            ),
            config_reload_tx,
            spine_change_tx,
            workspace_tx,
            shutdown_flag,
            pty_input_owners,
            live_limits,
            tailscale_mode_control,
            pending: Vec::new(),
            last_pr_foregrounded: None,
            spine_check,
            mutation_version: 0,
            streaming_version: 0,
            prev_streaming: std::collections::HashMap::new(),
            prev_attention: std::collections::HashSet::new(),
            tick_count: 0,
            config_disk_ahead: false,
            shutdown_echo,
        }
    }

    /// Open the spine-change gate for this iteration. The fingerprint compare in
    /// [`SpineCheck::maybe_check`] stays the precise emit gate, so an over-broad
    /// bump only costs a serialize.
    pub(crate) fn note_mutation(&mut self) {
        self.mutation_version = self.mutation_version.wrapping_add(1);
    }

    /// The web-side follow-ups for ONE drained reaction, in the order the actor
    /// loop has always run them.
    ///
    /// Deliberately takes the reaction by reference and does NOT consume it: the
    /// terminal UI applies the same reaction by value straight afterwards, so the
    /// seam has to be pre-consume. The one follow-up that genuinely needs
    /// ownership (adopting a reloaded config) is therefore not here; see
    /// [`Self::announce_config_reload`] for the half the web needs.
    pub(crate) fn fanout_reaction(
        &mut self,
        engine: &mut Engine,
        reaction: &EventReaction,
        routing: FollowupRouting,
    ) {
        for status in dux_core::wire::wire_statuses_from_reaction(reaction) {
            let _ = self.status.send(status);
        }
        // `gh_available` rides the bootstrap document, which a browser fetches
        // at connect and then only on `config.changed`, so a flip in whether
        // GitHub features work has to ride that same signal: without it a
        // browser keeps hiding (or offering) the pull-request entries, and a
        // momentary rate limit stands until the page is reloaded.
        if peek_gh_availability_changed(reaction).is_some() {
            let _ = self.config_reload_tx.send(());
        }
        // NOT origin-routed, deliberately, and for the same reason as the launch
        // follow-up further down: it looks its own session up in a web pending map
        // first and does nothing when the delete was not web-started. It starts no
        // work either way, it only resolves a keyed op the web itself opened,
        // which is what makes self-guarding enough here where it is not enough for
        // the routed arms below.
        for status in engine.drive_delete_followup(reaction) {
            let _ = self.status.send(status);
        }
        // The checkout-default-branch inspection (worker 1) just produced a
        // Known-default reaction: spawn the switch (worker 2). Its completion
        // posts NonDefaultBranchCheckoutCompleted, whose status flows through
        // the wire_statuses_from_reaction drain above on the next iteration.
        if self.web_owns(engine, reaction, routing) {
            for status in engine.drive_checkout_followup(reaction) {
                let _ = self.status.send(status);
            }
            // The add-project "Check Out & Add" switch (worker 2) just succeeded:
            // AddProjectAfterBranchCheckout drives the actual project add here
            // (the TUI does this in workers.rs). A switch FAILURE instead produced
            // an error Status, already surfaced by the wire_statuses drain above.
            for status in engine.drive_add_project_followup(reaction) {
                let _ = self.status.send(status);
            }
            // A new-agent-from-PR lookup (gh pr view) just resolved: the TUI would
            // open a name prompt here, but the web already sent the name, so
            // OpenNewAgentPromptForPr drives the actual CreateAgentRequest::PullRequest
            // dispatch. A lookup FAILURE instead produced a keyed error Status
            // (resolving the PR-lookup op), already surfaced by the wire_statuses
            // drain above. On SUCCESS the followup hands off to the create busy and
            // returns the PR-lookup op's clear key so its spinner is dismissed.
            let pr_followup = engine.drive_pr_lookup_followup(reaction);
            for status in pr_followup.statuses {
                let _ = self.status.send(status);
            }
            for key in pr_followup.clear_keys {
                self.status.clear(key);
            }
        }

        // A reconnect / force-restart launch reported back: resolve the web
        // launch op (Engine::pending_web_launch_ops) so its "Launching…" /
        // "Starting fresh…" busy is replaced by the same-key final (or cleared
        // when the session vanished). Create-kind launch finals are resolved
        // engine-side and ride the wire_statuses drain above.
        //
        // Not origin-routed: it looks its own session up in a web pending map
        // first and does nothing when the launch was not web-started, which is
        // the same guard `followup_owner` would apply.
        let launch_followup = engine.drive_web_launch_followup(reaction);
        for status in launch_followup.statuses {
            let _ = self.status.send(status);
        }
        for key in launch_followup.clear_keys {
            self.status.clear(key);
        }
        // A StatusOp resolved to `Final::Clear`: dismiss the keyed toast.
        if let dux_core::engine::EventReaction::ClearStatus(key) = reaction {
            self.status.clear(key.clone());
        }

        // A project mutation just updated SQLite + in-memory projects; mirror
        // it into the portable config.toml so a later TUI start doesn't clobber it.
        // Skip for `Added` — that arm already wrote config inline in command.rs,
        // and Skip for `PersistenceFailed` — nothing was saved.
        if let EventReaction::ProjectPersistenceOutcome(outcome) = reaction
            && !matches!(
                outcome.view,
                ProjectPersistenceView::PersistenceFailed { .. }
                    | ProjectPersistenceView::Added { .. }
            )
            && let Err(e) = engine.persist_projects_to_config()
        {
            // STICKY: SQLite and the portable config now disagree about
            // the same project. That is textbook half-done, and dux treats
            // config/DB divergence as a first-class hazard elsewhere (a
            // later TUI start reads the config, not the database, so the
            // change silently reverts). Fixing it means editing config.toml,
            // outside anything the toast can offer.
            let _ = self.status.send(
                WireStatus::keyed(
                    "config-write",
                    "error",
                    format!("Saved to the database, but config.toml could not be updated: {e:#}"),
                )
                .sticky(),
            );
        }
    }

    /// Whether the web owns the routable follow-ups for `reaction`.
    fn web_owns(
        &self,
        engine: &Engine,
        reaction: &EventReaction,
        routing: FollowupRouting,
    ) -> bool {
        match routing {
            FollowupRouting::RunEverything => true,
            FollowupRouting::ByOrigin => {
                matches!(
                    engine.followup_owner(reaction),
                    dux_core::engine::FollowupOwner::Web
                )
            }
        }
    }

    /// Tell web clients that config-static state changed, WITHOUT adopting the
    /// config: the reloaded config is applied by whoever drains the event, and
    /// while the background server runs that is the terminal UI, whose own reload
    /// arm has never had a reason to fire this.
    ///
    /// Called BEFORE the drainer applies the config, because the seam is
    /// pre-consume, so nothing here may change what a route answers: the two
    /// live limits move in [`Self::note_config_applied`] instead. The cost,
    /// accepted and stated: if the drainer's apply then FAILS, a browser has been
    /// told to refetch a config that did not take. It refetches
    /// `/api/v1/bootstrap` and reads the config still in force, so the browser
    /// ends up correct either way; what it loses is the news that the reload
    /// failed, which the surface that failed it is the one showing.
    pub(crate) fn announce_config_reload(&mut self, engine: &Engine, reaction: &EventReaction) {
        let Some(config) = peek_apply_reloaded_config(reaction) else {
            return;
        };
        let restart_warning =
            server_restart_warning_copy(&engine.config.server, &config.server, true);
        let _ = self.config_reload_tx.send(());
        // Says what has actually happened at this point, and no more. The
        // drainer has not adopted the config yet (the seam is pre-consume), so
        // "new settings are active" would be a claim about a step that has not
        // run and may still fail. What is true here is that the file was read and
        // that browsers are being told to refetch.
        let _ = self.status.send(WireStatus::new(
            "info",
            "Configuration reloaded; connected browsers are refreshing.",
        ));
        if let Some(warning) = restart_warning {
            let _ = self.status.send(WireStatus::new("warning", warning));
        }
    }

    /// Adopt the two live `[server]` limits from a config the drainer has already
    /// applied. The companion seam's post-apply half; see
    /// [`dux_core::background_serve::BackgroundServeCompanion::note_config_applied`].
    pub(crate) fn note_config_applied(&self, server: &dux_core::config::ServerConfig) {
        self.live_limits.store_from(server);
    }

    /// The shared maintenance sweeps, run by whoever drains `worker_rx`.
    ///
    /// EXACTLY ONE runner per process. The terminal UI runs its own equivalents
    /// inside `drain_events`, so while the background server is on this function
    /// is not called at all: calling it there would reap PTYs twice, sweep the
    /// resume fallbacks twice, and dispatch a deferred worktree removal twice.
    pub(crate) fn run_maintenance(&mut self, engine: &mut Engine) {
        // Consume each provider's received-data flag once per tick and stamp
        // the engine's activity map, so bytes that arrived this tick count
        // toward the `working` projection in the spine read below. This is the
        // single poll site for the web surface (the TUI run loop is the single
        // poll site for the other surface).
        engine.poll_pty_activity();

        // Drain attention/progress signals right after activity so the progress
        // report and any output it also produced land in the same tick. Keeps the
        // "working" override truthful and maintains the per-tab attention flag.
        engine.poll_agent_signals();

        // Re-ask `gh` when the periodic re-check is due. The terminal UI runs
        // this from its own run loop, so while the background server is on this
        // sweep does not run at all and the probe is not doubled.
        engine.poll_gh_probe_schedule();

        // The two change-detection polls also run inside `check_spine`, which is
        // where they matter for a surface that does no sweeping. Keeping them here
        // as well preserves this loop's original ordering. Precisely: they are
        // pure compares against snapshots this struct carries, so calling them
        // twice costs nothing, and the FIRST of the two calls is the one that can
        // see a transition, which means a change is noticed one call earlier than
        // it would be otherwise. That is consequence-free, because the fingerprint
        // compare downstream is the emit gate and an early bump only buys it a
        // serialize it was going to do anyway.
        self.poll_change_signals(engine);

        // Refresh companion-terminal foreground commands so the spine's
        // `foreground_cmd` tracks what's running. The engine throttles this by
        // wall-clock (~2s), so calling it every tick is cheap.
        //
        // Bump #3: only when the refresh actually changed a `foreground_cmd` (a
        // throttled no-op or an unchanged probe returns false), so a quiet terminal
        // does not reopen the gate every interval.
        if engine.refresh_terminal_foregrounds() {
            self.note_mutation();
        }

        // Reap PTYs that an individual delete/close SIGTERMed and that have now
        // exited or passed their grace deadline (force-killed + dropped) — the
        // non-blocking background half of graceful close. For a reaped agent whose
        // delete also removes its worktree, dispatch that removal now, only after
        // the agent's process is actually gone (the existing
        // `WorktreeRemoveCompleted` path then drives its status).
        for removal in engine.reap_terminating_ptys() {
            let _busy = engine.dispatch_deferred_worktree_removal(removal);
        }

        // Resume-fallback sweep (both detection windows), BEFORE the exit prune:
        // a `--continue` that came up empty or a resume that hung past its
        // timeout is relaunched fresh here, so `dux serve` gets the same
        // continue-then-fresh behavior the TUI has instead of showing "Agent
        // exited" (the prune below would otherwise reap the exited resume
        // candidate and mark the agent Detached). Each retry's launch reaction
        // is surfaced through the same web launch-followup path the drained
        // reactions use, and a retry mutates the spine.
        // The web launches at a fixed (24, 80) seed size (the real size arrives
        // on the first client subscribe/resize), matching the other web launch
        // sites in this file.
        for reaction in engine.sweep_resume_fallbacks((24, 80)) {
            self.note_mutation();
            let followup = engine.drive_web_launch_followup(&reaction);
            for status in followup.statuses {
                let _ = self.status.send(status);
            }
            for key in followup.clear_keys {
                self.status.clear(key);
            }
        }

        // Reap agent/terminal PTYs whose child process exited so they stop
        // lingering in `providers`/`companion_terminals` and disappear from the
        // spine, broadcasting a status for each so web clients learn.
        //
        // Bump #4: only when something was actually pruned (the returned Vec is
        // non-empty), since a prune that found nothing left the spine untouched.
        let pruned = engine.prune_exited_ptys();
        if !pruned.is_empty() {
            self.note_mutation();
        }
        for pruned in pruned {
            if let Some(status) = prune_wire_status(&pruned) {
                let _ = self.status.send(status);
            }
        }
    }

    /// Emit what [`Self::run_maintenance`] would have emitted for sweeps ANOTHER
    /// surface ran, and open the change gate for them.
    ///
    /// The concurrent path's drainer is the terminal UI, so this function is the
    /// only way a browser hears about an agent that exited or a terminal that
    /// closed while the TUI is up. It runs the same status builder and the same
    /// two bump rules as the sweep it stands in for, and does no sweeping of its
    /// own: everything here has already happened.
    pub(crate) fn note_drained_maintenance(
        &mut self,
        maintenance: &dux_core::background_serve::DrainedMaintenance,
    ) {
        if maintenance.foregrounds_changed {
            self.note_mutation();
        }
        if maintenance.pruned.is_empty() {
            return;
        }
        self.note_mutation();
        for pruned in &maintenance.pruned {
            if let Some(status) = prune_wire_status(pruned) {
                let _ = self.status.send(status);
            }
        }
    }

    /// One iteration of the web-only servicing: the whole of what the web layer
    /// needs when it is NOT the surface draining worker events.
    ///
    /// `dux server` and the flip compose the same pieces individually (with the
    /// has-active-processes sync in its original slot between the request drain
    /// and the status tick), so the two paths run identical code in an identical
    /// order.
    pub(crate) fn service_engine_once(
        &mut self,
        engine: &mut Engine,
    ) -> dux_core::background_serve::ServiceOutcome {
        self.resolve_pending_subscribes(engine);
        self.check_spine(engine);
        let drained = self.drain_requests(engine);
        self.tick_statuses();
        dux_core::background_serve::ServiceOutcome {
            mutated: drained.mutated,
            stopped: drained.stopped,
            // Retirement is the companion's own decision, made one level up in
            // the `dux` binary: this type only reports what one iteration did.
            retirement: None,
        }
    }

    /// Resolve or expire pending subscribes now that providers may have appeared.
    fn resolve_pending_subscribes(&mut self, engine: &mut Engine) {
        let now = Instant::now();
        self.pending.retain_mut(|p| {
            if let Some(client) = engine.providers.get(p.tab_id.as_ref_id()) {
                if let Some(reply) = p.reply.take() {
                    let _ = reply.send(Ok(client.subscribe_with_repaint()));
                }
                false
            } else if !engine.is_in_flight(&InFlightKey::AgentLaunch(p.tab_id.clone())) {
                // The launch worker finished but no provider came up: it failed.
                // Fail fast with a clear message instead of waiting for the timeout;
                // the specific error was already broadcast on the status stream.
                if let Some(reply) = p.reply.take() {
                    let _ = reply.send(Err(format!(
                        "Agent failed to launch for session {}. Check dux.log for details.",
                        p.tab_id
                    )));
                }
                false
            } else if now > p.deadline {
                if let Some(reply) = p.reply.take() {
                    let _ = reply.send(Err("timed out launching agent".to_string()));
                }
                false
            } else {
                true
            }
        });
    }

    /// The two change-detection polls that open the spine-change gate for state a
    /// mutation counter cannot see. Pure reads: they compare the engine against
    /// snapshots this struct carries and touch nothing else, which is why calling
    /// them twice in one iteration is free.
    fn poll_change_signals(&mut self, engine: &Engine) {
        // Push attention set/clear promptly: the `needs_attention` projection is
        // event-derived (a mutation counter can't see it flip), so this O(1)
        // compare bumps `mutation_version` on any change to open the fingerprint
        // gate. The fingerprint (which includes `needs_attention`) stays the
        // precise emit gate.
        poll_attention_transitions(engine, &mut self.prev_attention, &mut self.mutation_version);

        // Track per-agent streaming transitions. The `working` flag is time-derived
        // (it flips off once AGENT_STREAMING_WINDOW lapses), so a mutation counter
        // cannot see it; this O(1) poll bumps `streaming_version` on any flip so the
        // spine check opens on idle->working / working->idle.
        poll_streaming_transitions(
            engine,
            &mut self.prev_streaming,
            &mut self.streaming_version,
        );
    }

    /// The projects/sessions/sidebar spine is signaled via coarse events.
    /// Evaluated every Nth tick (see `SPINE_CHECK_TICK_INTERVAL`); the actual
    /// fingerprint serialize runs only when a change signal moved or the
    /// backstop fired (see `SpineCheck::maybe_check`), so idle ticks cost nothing.
    /// A failed send means no web forwarder is listening (e.g. the TUI flip),
    /// which is fine.
    fn check_spine(&mut self, engine: &Engine) {
        self.poll_change_signals(engine);
        self.tick_count = self.tick_count.wrapping_add(1);
        if self.tick_count.is_multiple_of(SPINE_CHECK_TICK_INTERVAL) {
            self.spine_check.maybe_check(
                engine,
                self.mutation_version,
                self.streaming_version,
                &self.pty_input_owners,
                &self.spine_change_tx,
                &self.workspace_tx,
            );
        }
    }

    /// Drain every queued engine request.
    fn drain_requests(&mut self, engine: &mut Engine) -> DrainOutcome {
        let mut mutated = false;
        let mut stopped = false;
        loop {
            let req = match self.req_rx.try_recv() {
                Ok(req) => req,
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    stopped = true;
                    break;
                }
            };
            // Bump #1: every request that can move the spine, decided ONCE for
            // the whole drain by the exhaustive `request_mutates_spine` (which
            // covers the arms handled inline below as well as the ones
            // `handle_request` takes). Decided before `req` is moved; the
            // fingerprint compare downstream stays the precise emit gate, so an
            // over-broad `true` only costs a serialize.
            let mutates = request_mutates_spine(&req);
            match req {
                EngineRequest::SubscribePty(tab_id, reply) => {
                    handle_subscribe(
                        engine,
                        &mut self.pending,
                        &mut self.last_pr_foregrounded,
                        &mut self.status,
                        tab_id,
                        reply,
                    );
                }
                EngineRequest::SpineJson(reply) => {
                    // Serve the loop-local cache (handled here, not in
                    // `handle_request`, which has no access to it).
                    let _ = reply.send(self.spine_check.doc.json.to_string());
                }
                EngineRequest::Shutdown(reply) => {
                    // Trip the teardown flag first so any PTY forwarders exit
                    // promptly (symmetry with the flip; harmless here since the
                    // engine drop will also disconnect their channels).
                    self.shutdown_flag
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    // SIGTERM children, wait up to the configured web grace for
                    // them to flush state, then mark agent sessions Detached.
                    // Handled here (not in `handle_request`) because it must stop
                    // the loop. `shutdown_ptys` logs the start/result to dux.log;
                    // on the `dux server` path we also echo to the console
                    // (stderr) so an operator running it in the foreground sees
                    // the shutdown progress, mirroring what the TUI prints on
                    // quit. Every other path shares the terminal with a themed
                    // dux-tui screen and stays silent.
                    let echo = self.shutdown_echo == ShutdownEcho::Stderr;
                    let agents = engine.providers.len();
                    let terminals = engine.companion_terminals.len();
                    let grace = dux_core::config::shutdown_grace(
                        engine.config.server.shutdown_timeout_seconds,
                    );
                    if echo && agents + terminals > 0 {
                        eprintln!(
                            "{}",
                            dux_core::engine::format_shutdown_start(agents, terminals, grace)
                        );
                    }
                    let report = engine.shutdown_ptys(grace);
                    if echo && report.agents_total + report.terminals_total > 0 {
                        eprintln!("{}", dux_core::engine::format_shutdown_result(&report));
                    }
                    let _ = reply.send(());
                    stopped = true;
                    break;
                }
                other => {
                    handle_request(
                        engine,
                        other,
                        &mut self.status,
                        &self.config_reload_tx,
                        &mut self.config_disk_ahead,
                        &self.pty_input_owners,
                    );
                }
            }
            if mutates {
                mutated = true;
                self.note_mutation();
            }
        }
        DrainOutcome { mutated, stopped }
    }

    /// Expire a timed-out transient status and broadcast the cleared state to
    /// every connected client. Busy/warning/error are left untouched.
    fn tick_statuses(&mut self) {
        self.status.tick(Instant::now());
    }
}

/// The shared engine request/drain loop. Runs on the CALLER's thread (a spawned
/// std thread for `dux server`, the main thread for the in-process flip) and
/// owns `engine` for the loop's duration, returning it on exit so the flip can
/// resume the TUI around the same live engine (PTYs intact).
///
/// A thin composition of [`EngineService`]'s pieces, in the order this loop has
/// always run them. It is also the process's ONE worker-event drainer while it
/// runs, which is why it is the surface that calls
/// [`EngineService::run_maintenance`].
///
/// `control` is consulted once at the top of each outer iteration: `Exit` stops
/// the loop and returns the engine WITHOUT shutting down any PTYs (the flip's
/// ReturnToTui path relies on this). The inline `Shutdown` request still stops
/// the loop too — it SIGTERMs the children (the CLI/quit teardown).
pub(crate) fn run_engine_loop(
    mut engine: Engine,
    ends: ActorLoopEnds,
    shutdown_echo: ShutdownEcho,
    mut control: impl FnMut() -> LoopControl,
) -> Engine {
    let mut svc = EngineService::new(&engine, ends, shutdown_echo);
    loop {
        // Caller-driven exit (the flip's status screen asked to stop). Checked
        // before any work so an exit takes effect on the next tick. PTYs are
        // left untouched — teardown, if any, is the caller's responsibility.
        if matches!(control(), LoopControl::Exit) {
            break;
        }

        // Draining worker events may insert a launched provider (AgentLaunchReady)
        // into `engine.providers`, which resolves pending subscribes below.
        while let Ok(event) = engine.worker_rx.try_recv() {
            let reaction = engine.process_worker_event(event);
            // Bump #2: a worker event can insert/remove a provider, flip a session
            // status, or apply a project mutation — all spine state. Bump
            // unconditionally; the fingerprint compare stays the precise emit gate.
            svc.note_mutation();
            svc.fanout_reaction(&mut engine, &reaction, FollowupRouting::RunEverything);

            // A reload worker re-read config.toml; apply the new config to the
            // running engine. This consumes `reaction`, so it MUST be the last
            // use of it in the loop body (all `&reaction` borrows above end
            // first). `ApplyReloadedConfig` and `ProjectPersistenceOutcome` are
            // distinct variants, so consuming here never skips the project sync.
            //
            // The reload follow-up reaction may arrive WRAPPED in a `Multi` when
            // config-mutating commands were deferred during the reload (the
            // engine folds the `ApplyReloadedConfig` in with the deferred saves'
            // status reactions). Pull the `ApplyReloadedConfig` out of either the
            // bare or the wrapped form so the server-restart warning always
            // runs. The deferred saves' own status reactions were already
            // surfaced by the `wire_statuses_from_reaction` drain above (it
            // flattens `Multi`).
            if let Some(config) = take_apply_reloaded_config(reaction) {
                // Capture the rebind-relevant [server] settings
                // BEFORE the swap so we can tell whether the reload touched
                // anything that only takes effect at startup (listeners are
                // bound once; reload-config never rebinds). Comparing here — the
                // arm already holds both the running config (pre-swap) and the
                // incoming one — keeps the detection next to the config-reload handler.
                let restart_warning =
                    server_restart_warning_copy(&engine.config.server, &config.server, false);
                // The parsed MODE, never the raw string: the value is trimmed
                // and case-insensitive, so a user who retyped "Auto" must not
                // have their listener stopped and started for nothing.
                let previous_tailscale = engine.config.server.tailscale_mode();
                let next_tailscale = config.server.tailscale_mode();
                match engine.apply_reloaded_config(*config) {
                    Ok(()) => {
                        // Memory now matches disk: any pending raw "Save" has been
                        // adopted, so disk is no longer ahead.
                        svc.config_disk_ahead = false;
                        svc.live_limits.store_from(&engine.config.server);
                        // Signal the web layer that config-static state changed so
                        // it emits a `config.changed` event and clients refetch
                        // `/api/v1/bootstrap`. Fire-and-forget: an `Err` only means
                        // no forwarder is listening (e.g. the TUI flip), which is
                        // fine.
                        let _ = svc.config_reload_tx.send(());
                        let _ = svc.status.send(WireStatus::new(
                            "info",
                            "Configuration reloaded. New settings are active.",
                        ));

                        // The new config WAS applied to the engine, but the
                        // `[server]` bind section only takes effect at startup; a
                        // reload cannot rebind listeners. Warn so the user knows a
                        // restart is needed for those specific changes to take
                        // effect.
                        if let Some(warning) = restart_warning {
                            let _ = svc.status.send(WireStatus::new("warning", warning));
                        }

                        // `[server] tailscale` IS live, so a reload that changed
                        // it acts rather than warning. This is the reload owner
                        // for `dux server` and for the flip. Background mode
                        // fills the same slot but never runs this loop, so its
                        // reload is the terminal UI's and there is no double
                        // apply.
                        if previous_tailscale != next_tailscale
                            && let Some(control) = svc.tailscale_mode_control.get()
                        {
                            let status = svc.status.tx.clone();
                            control.set_mode_detached(next_tailscale, move |outcome| {
                                let report = outcome.report(next_tailscale);
                                let tone = if report.warning { "warning" } else { "info" };
                                let _ = status.send(WireStatus::new(tone, report.message));
                            });
                        }
                    }
                    Err(e) => {
                        let _ = svc.status.send(WireStatus::new(
                            "error",
                            format!("Config reload failed to apply: {e:#}"),
                        ));
                    }
                }
            }
        }

        svc.run_maintenance(&mut engine);
        svc.resolve_pending_subscribes(&mut engine);
        svc.check_spine(&engine);
        let drained = svc.drain_requests(&mut engine);
        if drained.stopped {
            break;
        }
        // Keep the changed-files poller's cadence flag in step with the live PTY
        // count. Placed at the END of the iteration deliberately: both the halves
        // that move the count run above it, the exit prune that drops PTYs whose
        // child died and the request drain that launches new ones, so one call
        // here sees both directions. The TUI does the same thing once per tick
        // from its own run loop.
        engine.sync_has_active_processes();

        svc.tick_statuses();
        thread::sleep(TICK);
    }
    engine
}

/// Whether this reaction (or one nested inside a `Multi`) says GitHub
/// availability flipped, and which way.
fn peek_gh_availability_changed(reaction: &EventReaction) -> Option<bool> {
    match reaction {
        EventReaction::GhAvailabilityChanged { available } => Some(*available),
        EventReaction::Multi(reactions) => reactions.iter().find_map(peek_gh_availability_changed),
        _ => None,
    }
}

/// Borrow the reloaded `Config` out of a reload follow-up reaction WITHOUT
/// consuming it, the pre-consume twin of [`take_apply_reloaded_config`].
///
/// Needed because the background-server seam sees each reaction by reference,
/// before the terminal UI applies it: the web half of a reload (announcing
/// `config.changed`) has to read the incoming config to compare bind settings,
/// while the drainer keeps the value to adopt.
fn peek_apply_reloaded_config(reaction: &EventReaction) -> Option<&dux_core::config::Config> {
    match reaction {
        EventReaction::ApplyReloadedConfig(config) => Some(config),
        EventReaction::Multi(reactions) => reactions.iter().find_map(peek_apply_reloaded_config),
        _ => None,
    }
}

/// Wraps the WS status channels so every status flows through one shared
/// [`KeyedStatusController`]: [`send`](Self::send) upserts the status by key,
/// refreshes the Vec snapshot watch (all open statuses, for clients connecting
/// mid-operation), and broadcasts it LIVE so a transient pending flash is never
/// coalesced away; [`tick`](Self::tick) — called once per loop iteration —
/// expires timed-out entries, pushes removed keys onto `clear_tx` (so the WS
/// forwarder sends `StatusCleared` frames), and upgrades stale Busy entries
/// to Warning (busy-timeout). The `send` method name and broadcast return type
/// let the loop's existing call sites stay unchanged.
struct StatusEmitter {
    tx: broadcast::Sender<WireStatus>,
    clear_tx: broadcast::Sender<Option<String>>,
    snapshot_tx: watch::Sender<Vec<KeyedWireStatus>>,
    controller: KeyedStatusController,
    /// Most recent generation for each keyed status so `clear` can guard
    /// against dismissing a newer status placed on the same key by a
    /// concurrent operation (e.g. a rapid retry during commit-msg generation).
    generations: std::collections::HashMap<String, Generation>,
}

impl StatusEmitter {
    fn new(
        tx: broadcast::Sender<WireStatus>,
        clear_tx: broadcast::Sender<Option<String>>,
        snapshot_tx: watch::Sender<Vec<KeyedWireStatus>>,
        live: dux_core::statusline::LiveStatusKeys,
    ) -> Self {
        Self {
            tx,
            clear_tx,
            snapshot_tx,
            // The web REPLAYS this snapshot to every `/ws/events` connection at
            // connect and again after a broadcast lag, so a retained final would
            // be re-raised as a fresh toast on every page load, every new tab and
            // every reconnect, forever. A `Busy` is live state a late joiner must
            // learn about; a final is an event it legitimately missed. See
            // [`dux_core::statusline::StatusRetention`].
            // Sharing the engine's live-key set is what tells a slow operation
            // from an abandoned one; without it a spinner is timed out on
            // twenty seconds of silence however long the work really takes.
            controller: KeyedStatusController::emitting_finals().with_live_keys(live),
            generations: std::collections::HashMap::new(),
        }
    }

    /// Upsert the status in the controller (keyed or anonymous), refresh the
    /// Vec snapshot, then broadcast it live. Returns the broadcast `send` result
    /// so the call sites keep discarding it with `let _ =` exactly as before.
    fn send(
        &mut self,
        status: WireStatus,
    ) -> Result<usize, broadcast::error::SendError<WireStatus>> {
        // A quiet status is the command's answer and not a notification: it
        // already rode back to its caller in the outcome, so it must not enter
        // the controller (which would replay it to every joining tab) and must
        // not be broadcast. This is the ONE gate, so a site marking a status
        // quiet cannot be undone by whichever path it happens to travel.
        if status.quiet {
            return Ok(0);
        }
        let tone = StatusTone::from_wire(&status.tone);
        let generation = self.controller.set_scoped(
            Instant::now(),
            status.key.clone(),
            tone,
            status.message.as_str(),
            status.scope.clone(),
            status.sticky,
        );
        if let Some(ref k) = status.key {
            self.generations.insert(k.clone(), generation);
        }
        let _ = self.snapshot_tx.send(self.controller.snapshot());
        self.tx.send(status)
    }

    /// Explicitly clear a keyed entry (remove from the controller, push the
    /// key onto `clear_tx` so the WS forwarder sends a `StatusCleared` frame,
    /// refresh the snapshot). Guards with the generation stored when the busy
    /// was emitted so a concurrent in-flight cannot be prematurely dismissed.
    fn clear(&mut self, key: String) {
        let generation = self.generations.get(&key).copied();
        if self.controller.clear(&key, generation) {
            self.generations.remove(&key);
            let _ = self.snapshot_tx.send(self.controller.snapshot());
            let _ = self.clear_tx.send(Some(key));
        }
    }

    /// Expire timed-out entries: upgrade a stale Busy→Warning and drop finals
    /// that have aged past `FINAL_REPLAY_WINDOW`. Pushes cleared keys onto
    /// `clear_tx` (for the WS forwarder's `StatusCleared` frames) and broadcasts
    /// upgraded entries as live `WireStatus` updates. Short-circuits when
    /// nothing changed so idle ticks cost nothing.
    ///
    /// `changes.purged` MUST be part of that short-circuit test even though it
    /// produces no frame: it is the count of finals that left the replay
    /// snapshot, so skipping on it alone would leave `snapshot_tx` publishing
    /// entries the controller has already dropped, and the stale-replay bug
    /// would survive in the copy the sockets actually read.
    ///
    /// A busy whose operation the engine still holds is re-broadcast rather than
    /// upgraded to a false "timed out" (the controller answers that from the
    /// live-key set it shares with the engine), and that re-broadcast is also
    /// what re-arms the browser's own leak guard on the spinner.
    fn tick(&mut self, now: Instant) {
        let changes = self.controller.tick(now, LAUNCH_TIMEOUT);
        if changes.cleared_keys.is_empty()
            && changes.upgraded.is_empty()
            && changes.refreshed.is_empty()
            && changes.purged == 0
        {
            return;
        }
        let _ = self.snapshot_tx.send(self.controller.snapshot());
        for key in changes.cleared_keys {
            let _ = self.clear_tx.send(key);
        }
        for up in changes.upgraded.into_iter().chain(changes.refreshed) {
            let _ = self.tx.send(WireStatus {
                key: up.key,
                tone: up.tone,
                message: up.message,
                scope: up.scope,
                sticky: up.sticky,
                // A quiet status never reaches the controller, so nothing it
                // hands back here can be one.
                quiet: false,
            });
        }
    }
}

/// Fingerprint the two halves of the projected spine as `(projects, sessions)`
/// JSON strings, for the loop's coarse change detection.
///
/// The sidebar is deliberately EXCLUDED from both fingerprints: it is fully
/// DERIVED from projects + sessions, so every sidebar input change is already
/// captured by one of the two halves — a project name/`path_missing` change moves
/// the projects fingerprint, and a session `project_id`/order/orphan transition
/// moves the sessions fingerprint. Folding the sidebar (which embeds project
/// fields) into the sessions half instead made a PROJECT-only change spuriously
/// fire `sessions.changed`. Since either event means the whole document was
/// rebuilt and republished, the sidebar still reaches the client on whichever
/// side fired.
///
/// Terminals ARE included, folded into the half that matches their owner: left
/// out of both halves, a terminal's label, foreground command, working flag or
/// drag order could change WITHOUT any coarse event firing, and no client would
/// ever refetch. A session-owned terminal moves the sessions half, a project
/// terminal the projects half.
///
/// A STANDALONE terminal belongs to neither naturally, so one has to be
/// chosen. It moves the SESSIONS half, because that
/// is the event the flat sidebar list's churn already fires on and because
/// either event carries the whole document regardless. What matters
/// is that it fires on ONE of them: firing on neither is the silent-omission bug
/// this whole partition exists to prevent.
///
/// The check builds the spine through [`owned_spine`] (the engine projection
/// plus the web-layer input-owner overlay) and hands it here, so ownership
/// flips fingerprint like any other sessions-half change. The projection is
/// already done by then, so this is only the split-and-serialize, which is
/// what makes the owner partitioning testable without spawning a real PTY.
fn fingerprint_halves(spine: &dux_core::viewmodel::SpineView) -> (String, String) {
    // Exhaustive over the owner kinds: a new kind must decide which coarse event
    // its churn belongs to instead of silently signalling nothing.
    let (session_terminals, project_terminals): (Vec<_>, Vec<_>) =
        spine.terminals.iter().partition(|t| match &t.owner {
            dux_core::viewmodel::TerminalOwnerView::Session { .. } => true,
            dux_core::viewmodel::TerminalOwnerView::Project { .. } => false,
            // Owned by nothing, so it belongs to neither half naturally and one
            // has to be chosen. It fires the sessions event, which is
            // where the flat sidebar list's churn already goes; see the note on
            // `fingerprint_halves`.
            dux_core::viewmodel::TerminalOwnerView::Standalone { .. } => true,
        });
    let projects = serde_json::to_string(&(&spine.projects, &project_terminals))
        .unwrap_or_else(|_| "[]".to_string());
    let sessions = serde_json::to_string(&(&spine.sessions, &session_terminals))
        .unwrap_or_else(|_| "[]".to_string());
    (projects, sessions)
}

/// Build the spine the web layer actually serves: the engine's projection with
/// the current PTY input owners stamped onto each owned agent tab. The check
/// builds this ONCE per pass and derives both the fingerprint compare and the
/// cached `/spine` JSON from the same built value, so the served document
/// always matches what was fingerprinted (two separate builds could straddle a
/// concurrent claim and disagree). An ownership flip therefore moves the
/// sessions fingerprint and fires `sessions.changed` exactly like any
/// engine-side spine mutation. See
/// [`dux_core::viewmodel::AgentTabView::input_owner`] for why the engine
/// cannot fill this itself.
fn owned_spine(engine: &Engine, owners: &PtySizeOwners) -> dux_core::viewmodel::SpineView {
    let mut spine = engine.spine();
    let map = owners.input_owners_snapshot();
    if !map.is_empty() {
        for session in &mut spine.sessions {
            overlay_session_input_owners(session, &map);
        }
        // A companion terminal's pty id IS its terminal id and the spine carries
        // terminals in one flat collection, so publishing their ownership is one
        // more loop over the same snapshot. See
        // [`overlay_session_input_owners`] for the cost this accepts.
        overlay_terminal_input_owners(&mut spine.terminals, &map);
    }
    spine
}

/// The pure half of the overlay: stamp `input_owner` onto every tab of ONE
/// session whose PTY id appears in the owner map. A tab's own id is its PTY id,
/// the first tab included, so this is a direct per-tab lookup.
/// Companion terminal ownership IS published now, by
/// [`overlay_terminal_input_owners`]: a terminal pane's take-over card consumes
/// it to tell a stale driver name from a fresh one. Shared by [`owned_spine`] and the `Spine`/`Session` request arms in
/// [`handle_request`], so every web-served read of a session agrees about who
/// owns its tabs.
///
/// Companion terminals are overlaid separately, by
/// [`overlay_terminal_input_owners`], because they live in the spine's own flat
/// collection rather than under a session.
fn overlay_session_input_owners(
    session: &mut dux_core::viewmodel::SessionView,
    owners: &std::collections::HashMap<String, u64>,
) {
    for tab in &mut session.tabs {
        if let Some(conn_id) = owners.get(&tab.id) {
            // Stringified so the wire shape matches the `pty.owner` handover
            // frames' `owner` field, which is what the client compares its own
            // PTY-socket ids against.
            tab.input_owner = Some(conn_id.to_string());
        }
    }
}

/// The terminal half of the overlay: stamp `input_owner` onto every terminal
/// whose PTY id appears in the owner map. A companion terminal's PTY id is its
/// terminal id, so this is the same direct lookup the tab loop does, over the
/// spine's one flat terminal collection.
///
/// THE COST, accepted deliberately: an ownership flip on a terminal now moves
/// the spine fingerprint and fires `sessions.changed`, exactly as it already
/// does for an agent tab. It buys the browser the one fact it cannot derive,
/// which device is driving a terminal, so a terminal pane's take-over card can
/// tell a stale driver name from a fresh one.
fn overlay_terminal_input_owners(
    terminals: &mut [dux_core::viewmodel::TerminalView],
    owners: &std::collections::HashMap<String, u64>,
) {
    for terminal in terminals {
        if let Some(conn_id) = owners.get(&terminal.id) {
            terminal.input_owner = Some(conn_id.to_string());
        }
    }
}

/// The revision of the seed document built at loop start. The watch channel
/// holds `None` until then, so no revision value is reserved as a sentinel.
const FIRST_WORKSPACE_REV: u64 = 1;

/// The workspace document as BOTH of its consumers read it: one cached
/// serialization with its revision already inside the JSON, plus that revision
/// broken out so the push frame can carry it without parsing the body.
///
/// The revision is minted at the single place that rewrites the cache
/// ([`SpineCheck::maybe_check`]), which is also the only place in the process
/// that knows the document actually changed. Embedding it at serialization time
/// rather than splicing it per consumer is what makes a fetched body and a
/// pushed frame orderable against each other: they are the same bytes, carrying
/// the same number, however the client came by them.
pub struct WorkspaceDoc {
    /// Monotonic within one run of the server, starting at 1. It says nothing
    /// across restarts, which is exactly why the client resets what it has
    /// applied whenever its events socket reopens.
    pub rev: u64,
    /// The serialized document, `rev` field included. `Arc<str>` because every
    /// connected client is handed the same bytes on every change; re-serializing
    /// per connection is the cost this whole change exists to remove.
    pub json: Arc<str>,
}

/// Serialize one workspace document with `rev` as a top-level field alongside
/// the spine's own fields. Flattening keeps every existing field exactly where
/// it was, so `rev` is purely additive to the REST body.
///
/// A serialization failure cannot happen for this type, but if it ever did, the
/// fallback still carries the rev: a client that applies an empty document is
/// merely stale, while a client that applies a document whose rev it cannot
/// order would be wrong.
fn serialize_workspace(rev: u64, spine: &dux_core::viewmodel::SpineView) -> Arc<str> {
    #[derive(serde::Serialize)]
    struct Revisioned<'a> {
        rev: u64,
        #[serde(flatten)]
        spine: &'a dux_core::viewmodel::SpineView,
    }
    serde_json::to_string(&Revisioned { rev, spine })
        .unwrap_or_else(|_| format!("{{\"rev\":{rev}}}"))
        .into()
}

/// Loop-local state for the change-gated spine check and its self-healing
/// backstop. Holds the last-seen fingerprints of the two spine halves, the cached
/// whole-spine JSON for `GET /api/v1/workspace`, the version values last compared
/// against, and the backstop tick accumulator.
///
/// The gate's job is to skip the (relatively expensive) project + serialize on
/// idle intervals: it runs the [`owned_spine`] build + [`fingerprint_halves`] only when a change signal moved
/// since the last check, or the backstop fired. The fingerprint compare remains
/// the PRECISE emit gate — it never emits a coarse event for an unchanged half —
/// so the version signals only need to be a conservative "something might have
/// changed" hint, never a false negative for a covered mutator.
struct SpineCheck {
    prev_projects_fp: String,
    prev_sessions_fp: String,
    /// The cached workspace document: the `GET /api/v1/workspace` body and the
    /// pushed frame's payload, one serialization, rebuilt only when a half
    /// actually changes.
    doc: Arc<WorkspaceDoc>,
    /// The `mutation_version` value at the last fingerprint compare.
    last_checked_mutation: u64,
    /// The `streaming_version` value at the last fingerprint compare.
    last_checked_streaming: u64,
    /// The input-ownership generation ([`PtySizeOwners::ownership_generation`])
    /// at the last fingerprint compare. Ownership lives outside the engine, so
    /// neither `mutation_version` nor `streaming_version` can observe a claim
    /// or a disconnect release; this third signal is what opens the gate for
    /// them. It moves ONLY on take-over/first-claim/release — never on
    /// ordinary keystrokes — so publishing ownership does not churn the spine
    /// per write.
    last_checked_ownership: u64,
    /// Ticks accumulated toward the next backstop fire. Counted in real ticks
    /// (incremented by [`SPINE_CHECK_TICK_INTERVAL`] per call, since
    /// [`SpineCheck::maybe_check`] runs once per interval) and reset when the
    /// backstop fires.
    ticks_since_backstop: u32,
    /// Test-only count of how many times the gate actually ran the serialize.
    /// This is the seam that lets a test assert "idle intervals serialized zero
    /// times" as a positive fact rather than inferring it from "no event fired".
    #[cfg(test)]
    fp_call_count: u64,
}

impl SpineCheck {
    /// Build the seed state and publish the seed document. Publishing here (not
    /// on the first change) is what lets a client that connects before anything
    /// has happened be handed a real document instead of waiting for the first
    /// mutation.
    fn new(
        engine: &Engine,
        owners: &PtySizeOwners,
        workspace_tx: &watch::Sender<Option<Arc<WorkspaceDoc>>>,
    ) -> Self {
        // Read the generation BEFORE building the spine: a claim landing in
        // between then reads as newer than what was fingerprinted, so the next
        // check re-runs rather than missing it until the backstop.
        let last_checked_ownership = owners.ownership_generation();
        // ONE build feeds both the fingerprints and the cache (see `owned_spine`).
        let spine = owned_spine(engine, owners);
        let (prev_projects_fp, prev_sessions_fp) = fingerprint_halves(&spine);
        let doc = Arc::new(WorkspaceDoc {
            rev: FIRST_WORKSPACE_REV,
            json: serialize_workspace(FIRST_WORKSPACE_REV, &spine),
        });
        // `send_replace`, not `send`: `watch::Sender::send` fails and DROPS the
        // value when no receiver is alive, and the only long-lived receiver is
        // the one the handle holds, which may not exist yet in a test.
        workspace_tx.send_replace(Some(Arc::clone(&doc)));
        Self {
            prev_projects_fp,
            prev_sessions_fp,
            doc,
            last_checked_mutation: 0,
            last_checked_streaming: 0,
            last_checked_ownership,
            ticks_since_backstop: 0,
            #[cfg(test)]
            fp_call_count: 0,
        }
    }

    /// Called once per [`SPINE_CHECK_TICK_INTERVAL`] ticks. Runs the fingerprint
    /// compare (the serialize) only when `mutation_version` or `streaming_version`
    /// moved since the last check, OR the slow backstop fired. On a real change to
    /// either half, sends the matching coarse [`SpineChange`] and rebuilds the
    /// cached spine JSON. Idle intervals return immediately, doing zero work.
    fn maybe_check(
        &mut self,
        engine: &Engine,
        mutation_version: u64,
        streaming_version: u64,
        owners: &PtySizeOwners,
        spine_change_tx: &broadcast::Sender<SpineChange>,
        workspace_tx: &watch::Sender<Option<Arc<WorkspaceDoc>>>,
    ) {
        self.ticks_since_backstop = self
            .ticks_since_backstop
            .saturating_add(SPINE_CHECK_TICK_INTERVAL as u32);
        // Same before-the-build ordering as `new`: a claim racing this check
        // reads as a still-newer generation next interval.
        let ownership_generation = owners.ownership_generation();
        let signalled = mutation_version != self.last_checked_mutation
            || streaming_version != self.last_checked_streaming
            || ownership_generation != self.last_checked_ownership;
        let backstop = self.ticks_since_backstop >= SPINE_BACKSTOP_TICK_INTERVAL;
        if !signalled && !backstop {
            return;
        }
        self.last_checked_mutation = mutation_version;
        self.last_checked_streaming = streaming_version;
        self.last_checked_ownership = ownership_generation;
        if backstop {
            self.ticks_since_backstop = 0;
        }
        #[cfg(test)]
        {
            self.fp_call_count += 1;
        }

        // ONE build feeds both the fingerprints and (below) the cache, so the
        // served JSON can never describe a different ownership snapshot than
        // the fingerprint that gated its `sessions.changed`.
        let spine = owned_spine(engine, owners);
        let (projects_fp, sessions_fp) = fingerprint_halves(&spine);
        let mut spine_changed = false;
        if projects_fp != self.prev_projects_fp {
            self.prev_projects_fp = projects_fp;
            let _ = spine_change_tx.send(SpineChange::Projects);
            spine_changed = true;
        }
        if sessions_fp != self.prev_sessions_fp {
            self.prev_sessions_fp = sessions_fp;
            let _ = spine_change_tx.send(SpineChange::Sessions);
            spine_changed = true;
        }
        // Rebuild the cached workspace document only when a half actually
        // changed, so the common case (no change) skips the full serialization.
        // This is the one chokepoint that rewrites the cache, so it is also
        // where the revision is minted and where the document is published to
        // every connected client. An idle interval publishes nothing at all: a
        // client's applied revision only ever moves because the document did.
        if spine_changed {
            let rev = self.doc.rev.saturating_add(1);
            self.doc = Arc::new(WorkspaceDoc {
                rev,
                json: serialize_workspace(rev, &spine),
            });
            workspace_tx.send_replace(Some(Arc::clone(&self.doc)));
        }
    }
}

/// Track each agent's `is_agent_streaming()` value and bump `*streaming_version`
/// on any transition. The streaming flag is time-derived (it flips to `false`
/// once [`dux_core::engine::AGENT_STREAMING_WINDOW`] elapses with no new output),
/// so a mutation counter cannot observe it — this poll is the only way the gate
/// learns the `working` projection changed.
///
/// O(1)-per-agent and allocation-free on the hot path: it walks the existing
/// `pty_activity` map (the complete set of possibly-streaming sessions — an agent
/// with no recent activity is never streaming), compares against the carried
/// `prev_streaming` map, and bumps on a differing or first-seen value. Entries
/// for agents that left `pty_activity` (session teardown, prune) are dropped via
/// `retain` so the map cannot grow without bound. No sort, no per-tick `Vec`.
fn poll_streaming_transitions(
    engine: &Engine,
    prev_streaming: &mut std::collections::HashMap<String, bool>,
    streaming_version: &mut u64,
) {
    for session_id in engine.pty_activity.keys() {
        let now = engine.is_agent_streaming(session_id);
        match prev_streaming.get(session_id) {
            Some(&was) if was == now => {}
            _ => {
                *streaming_version = streaming_version.wrapping_add(1);
                prev_streaming.insert(session_id.clone(), now);
            }
        }
    }
    if prev_streaming.len() > engine.pty_activity.len() {
        prev_streaming.retain(|id, _| engine.pty_activity.contains_key(id));
    }
}

/// Bump `*version` whenever the engine's needs-attention set changed since the
/// last tick, so a set or clear opens the change-gated spine check promptly (the
/// `needs_attention` projection is event-derived and a coarse mutation counter
/// otherwise wouldn't see it flip). The serialized-spine fingerprint remains the
/// precise emit gate; this only opens the door. The set is small (usually empty),
/// so the comparison and occasional clone are cheap.
fn poll_attention_transitions(
    engine: &Engine,
    prev_attention: &mut std::collections::HashSet<TabId>,
    version: &mut u64,
) {
    if engine.needs_attention != *prev_attention {
        *version = version.wrapping_add(1);
        *prev_attention = engine.needs_attention.clone();
    }
}

fn handle_pty_write(engine: &mut Engine, id: String, bytes: Vec<u8>) {
    let wrote = pty_for(engine, &id).is_some_and(|client| client.write_bytes(&bytes).is_ok());
    if wrote
        && (engine.providers.contains_key(TabIdRef::new(&id))
            || engine.companion_terminals.contains_key(&id))
    {
        engine.note_pty_write(&id, &bytes);
    }
}

fn handle_pty_resize(
    engine: &Engine,
    input_owners: &PtySizeOwners,
    id: String,
    rows: u16,
    cols: u16,
    seq: u64,
) {
    let client = pty_for(engine, &id);
    let had_client = client.is_some();
    let outcome = input_owners.apply_grid_in_order(
        &id,
        seq,
        rows,
        cols,
        |rows, cols| {
            if let Some(client) = client {
                let _ = client.resize(rows, cols);
            }
        },
        |rows, cols| {
            if had_client {
                dux_core::logger::debug(&format!(
                    "PTY resize seq {seq} for pty {id} was overtaken mid-apply, so the \
                     newer {rows}x{cols} is being re-applied"
                ));
            }
        },
    );
    if matches!(outcome, dux_core::pty_owners::GridApplyOutcome::Dropped) {
        dux_core::logger::debug(&format!(
            "PTY resize seq {seq} for pty {id} dropped: a newer claim's geometry has \
             already reached the child"
        ));
    }
}

fn handle_apply_wire_request(
    engine: &mut Engine,
    cmd: WireCommand,
    reply: oneshot::Sender<Result<WireCommandOutcome, String>>,
    origin: StatusScope,
    status_tx: &mut StatusEmitter,
    config_reload_tx: &broadcast::Sender<()>,
    config_disk_ahead: &mut bool,
) {
    let mutates_config = cmd.mutates_config_static();
    if mutates_config && *config_disk_ahead {
        let reloaded = dux_core::config::load_config(&engine.paths);
        let _ = engine.apply_reloaded_config(reloaded);
        *config_disk_ahead = false;
    }

    engine.current_origin = origin;
    let result = engine.apply_wire(cmd).map_err(|e| e.to_string());
    engine.current_origin = StatusScope::All;

    if result.is_ok() && mutates_config {
        let _ = config_reload_tx.send(());
    }
    if let Ok(outcome) = &result
        && let Some(status) = &outcome.status
    {
        let _ = status_tx.send(status.clone());
    }
    let _ = reply.send(result);
}

fn handle_attach_pull_request(
    engine: &mut Engine,
    session_id: String,
    raw: String,
    origin: StatusScope,
    reply: oneshot::Sender<Result<String, String>>,
    status_tx: &mut StatusEmitter,
) {
    engine.current_origin = origin;
    let result = engine.dispatch_attach_pull_request(&session_id, &raw);
    engine.current_origin = StatusScope::All;
    let result = match result {
        Ok((op_id, pending)) => {
            for status in
                dux_core::wire::wire_statuses_from_reaction(&EventReaction::Status(pending))
            {
                let _ = status_tx.send(status);
            }
            Ok(op_id)
        }
        Err(error) => Err(error.to_string()),
    };
    let _ = reply.send(result);
}

fn handle_create_agent_branch_plan(
    engine: &Engine,
    project_id: String,
    name: String,
    reply: oneshot::Sender<Option<dux_core::git::CreateAgentBranchPlan>>,
) {
    let repo_path = engine
        .projects
        .iter()
        .find(|project| project.id == project_id)
        .map(|project| project.path.clone());
    if let Some(repo_path) = repo_path {
        std::thread::spawn(move || {
            let plan = dux_core::git::create_agent_branch_preflight(
                std::path::Path::new(&repo_path),
                &name,
            );
            let _ = reply.send(Some(plan));
        });
    } else {
        let _ = reply.send(None);
    }
}

fn first_load_inputs(engine: &Engine) -> FirstLoadInputs {
    let last_seen = match engine.session_store.last_seen_version() {
        Ok(version) => version,
        Err(error) => {
            dux_core::logger::warn(&format!(
                "[server] could not read the last-seen version; treating this \
                 as a first launch: {error:#}"
            ));
            None
        }
    };
    FirstLoadInputs {
        last_seen,
        running: dux_core::display_version().to_string(),
        disable_welcome: engine.config.ui.disable_automated_welcome_screen,
        disable_release_notes: engine.config.ui.disable_release_notes,
        state_root: engine.paths.root.clone(),
    }
}

fn read_raw_config(engine: &Engine) -> Result<String, String> {
    match std::fs::read_to_string(&engine.paths.config_path) {
        Ok(raw) => Ok(raw),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(dux_core::config_write::render_config_plain(&engine.config))
        }
        Err(error) => Err(format!("Could not read config.toml: {error}")),
    }
}

fn handle_request(
    engine: &mut Engine,
    req: EngineRequest,
    status_tx: &mut StatusEmitter,
    config_reload_tx: &broadcast::Sender<()>,
    // `true` when a raw `config.toml` write (Monaco "Save") has landed on disk but
    // has NOT been adopted into `engine.config` yet — i.e. disk is ahead of memory.
    // Set by the WriteRawConfig path; cleared whenever memory is reconciled with
    // disk (an explicit reload, or the reconcile below before a config-static
    // mutation). Loop-local because only the web surface writes raw config.
    config_disk_ahead: &mut bool,
    // The shared input-ownership registry, so the `Spine` and `Session` reads
    // carry the same `input_owner` overlay the cached `/spine` document does —
    // without it the REST list/single reads would permanently answer "unowned"
    // while `/spine` names an owner.
    input_owners: &PtySizeOwners,
) {
    match req {
        EngineRequest::ApplyWire(cmd, reply, origin) => {
            handle_apply_wire_request(
                engine,
                cmd,
                reply,
                origin,
                status_tx,
                config_reload_tx,
                config_disk_ahead,
            );
        }
        EngineRequest::EmitStatus(status) => {
            let _ = status_tx.send(status);
        }
        EngineRequest::ClearStatus(key) => {
            status_tx.clear(key);
        }
        EngineRequest::AttachPullRequest(session_id, raw, origin, reply) => {
            handle_attach_pull_request(engine, session_id, raw, origin, reply, status_tx);
        }
        // SubscribePty is handled inline in the loop (it needs `&mut pending`).
        EngineRequest::SubscribePty(_, _) => unreachable!("SubscribePty handled in the loop"),
        // Shutdown is handled inline in the loop (it must stop the thread).
        EngineRequest::Shutdown(_) => unreachable!("Shutdown handled in the loop"),
        EngineRequest::WritePty(id, bytes) => {
            handle_pty_write(engine, id, bytes);
        }
        EngineRequest::ResizePty(id, rows, cols, seq) => {
            handle_pty_resize(engine, input_owners, id, rows, cols, seq);
        }
        EngineRequest::PtyGridSize(id, reply) => {
            let _ = reply.send(pty_for(engine, &id).and_then(|client| client.grid_size()));
        }
        EngineRequest::NoteViewed(id) => {
            // The engine gates on the id resolving to a real agent tab, so a
            // companion-terminal id or a stale/bogus id is a harmless no-op and
            // `agent_viewed` never accumulates junk entries.
            engine.note_agent_viewed_if_known(&id);
        }
        EngineRequest::SubscribeTerminal(terminal_id, reply) => {
            let res = match engine.companion_terminals.get(&terminal_id) {
                Some(terminal) => Ok(terminal.client.subscribe_with_repaint()),
                None => Err("unknown terminal".to_string()),
            };
            let _ = reply.send(res);
        }
        EngineRequest::CreateTerminal(session_id, reply) => {
            // Headless spawn: seed a default 24x80 and let the first attaching
            // client resize the PTY to its real viewport.
            let res = engine
                .create_companion_terminal(&session_id, 24, 80)
                .map_err(|e| e.to_string());
            let _ = reply.send(res);
        }
        EngineRequest::CreateProjectTerminal(project_id, reply) => {
            let res = engine
                .create_project_terminal(&project_id, 24, 80)
                .map_err(|e| e.to_string());
            let _ = reply.send(res);
        }
        EngineRequest::CreateStandaloneTerminal(reply) => {
            let res = engine
                .create_standalone_terminal(24, 80)
                .map_err(|e| e.to_string());
            let _ = reply.send(res);
        }
        EngineRequest::TerminalOwnerOf(terminal_id, reply) => {
            let owner = engine
                .companion_terminals
                .get(&terminal_id)
                .map(|t| t.owner.clone());
            let _ = reply.send(owner);
        }
        EngineRequest::TerminalRoot(terminal_id, reply) => {
            let root = engine
                .companion_terminals
                .get(&terminal_id)
                .map(|t| (t.owner.clone(), t.client.spawn_dir().to_path_buf()));
            let _ = reply.send(root);
        }
        EngineRequest::CreateAgentTab(session_id, provider, reply) => {
            let res = create_agent_tab_inner(engine, &session_id, provider);
            let _ = reply.send(res);
        }
        EngineRequest::StartAgentTab(tab_id, reply) => {
            // Already running is success, not a second launch: the card can be
            // pressed from a page whose spine has not caught up yet.
            let res = if engine.providers.contains_key(TabIdRef::new(&tab_id)) {
                Ok(())
            } else {
                launch_agent(engine, &tab_id)
            };
            let _ = reply.send(res);
        }
        EngineRequest::SlotTabId(session_id, reply) => {
            let slot = engine
                .session_by_id(&session_id)
                .map(|s| s.slot_tab_id().to_string());
            let _ = reply.send(slot);
        }
        EngineRequest::TabSession(tab_id, reply) => {
            // This answers "which session owns this EXTRA tab". Slot-ness is
            // asked of the resolver rather than inferred from "the slot tab has
            // no `agent_tabs` row", which is a fact about today's storage shape.
            //
            // The per-tab route accepts the slot tab and reaches its PTY; the
            // exclusion here is only about the stored-owner lookup, since the
            // slot tab's owner is not stored, it is resolved. Callers that must
            // accept the slot tab ask `is_slot_tab` first (the tab PTY socket
            // and the REST tab verbs all do).
            let owner = match engine.session_for_slot_tab(TabIdRef::new(&tab_id)) {
                Some(_) => None,
                None => engine
                    .agent_tabs
                    .get(TabIdRef::new(&tab_id))
                    .map(|t| t.session_id.clone()),
            };
            let _ = reply.send(owner);
        }
        EngineRequest::CreateAgentBranchPlan(project_id, name, reply) => {
            handle_create_agent_branch_plan(engine, project_id, name, reply);
        }
        EngineRequest::PtyKeyForPaneId(pane_id, reply) => {
            let _ = reply.send(engine.pty_key_for_pane_id(&pane_id));
        }
        EngineRequest::FileDropDestination(pty_id, reply) => {
            let _ = reply.send(engine.file_drop_destination(&pty_id));
        }
        EngineRequest::FileDropTreeDestination(pty_id, dir, reply) => {
            let _ = reply.send(engine.file_drop_tree_destination(&pty_id, &dir));
        }
        EngineRequest::FileDropRefreshTarget(pty_id, reply) => {
            let _ = reply.send(engine.file_drop_refresh_target(&pty_id));
        }
        EngineRequest::SessionWorktree(session_id, reply) => {
            let worktree = engine
                .sessions
                .iter()
                .find(|s| s.id == session_id)
                .map(|s| s.directory().to_string());
            let _ = reply.send(worktree);
        }
        EngineRequest::SessionBranchDeleteInputs(session_id, reply) => {
            let _ = reply.send(engine.branch_delete_inputs(&session_id));
        }
        EngineRequest::SessionGitAccess(session_id, reply) => {
            // Refresh first: the probe is off-thread, so this call answers with
            // the previous verdict and the next one sees the new answer. That
            // is how a folder that became a repository is noticed when the
            // changes panel opens, with no git subprocess on the engine thread.
            engine.spawn_folder_repo_probe(&session_id);
            let _ = reply.send(engine.session_git_access(&session_id));
        }
        EngineRequest::ProjectPath(project_id, reply) => {
            let path = engine
                .projects
                .iter()
                .find(|p| p.id == project_id)
                .map(|p| p.path.clone());
            let _ = reply.send(path);
        }
        EngineRequest::ResourceTargets(reply) => {
            let _ = reply.send(engine.resource_monitor_targets());
        }
        EngineRequest::Bootstrap(reply) => {
            let _ = reply.send(engine.bootstrap());
        }
        EngineRequest::Spine(reply) => {
            let _ = reply.send(owned_spine(engine, input_owners));
        }
        // SpineJson is handled inline in the loop (it serves the loop-local cache).
        EngineRequest::SpineJson(_) => unreachable!("SpineJson handled in the loop"),
        EngineRequest::Session(id, reply) => {
            let view = engine.session_view(&id).map(|mut session| {
                // Same input-owner overlay as the spine (see `owned_spine`), so
                // the single-session read agrees with `/spine` about who is
                // driving each tab.
                overlay_session_input_owners(&mut session, &input_owners.input_owners_snapshot());
                let terminals = engine
                    .terminal_views_for_owner(dux_core::model::TerminalOwnerRef::Session(&id));
                (session, terminals)
            });
            let _ = reply.send(view);
        }
        EngineRequest::CreatedSessionForOp(op_id, reply) => {
            let _ = reply.send(engine.created_session_for_op(&op_id));
        }
        EngineRequest::NextChangesRev(session_id, reply) => {
            // Single chokepoint over the engine's SQLite connection. On a DB
            // error fall back to 0 (a non-advancing rev), which the client's
            // `rev >=` apply guard treats as a redundant refetch rather than a
            // crash; the error is logged for diagnosis.
            let rev = engine
                .session_store
                .next_changes_rev(&session_id)
                .unwrap_or_else(|e| {
                    dux_core::logger::error(&format!(
                        "next_changes_rev failed for session {session_id}: {e}"
                    ));
                    0
                });
            let _ = reply.send(rev);
        }
        EngineRequest::EditorDefault(reply) => {
            let _ = reply.send(engine.config.editor.default.clone());
        }
        EngineRequest::BrowseStartDir(reply) => {
            let dir = dux_core::project_browser::resolve_start_dir(&engine.config)
                .to_string_lossy()
                .to_string();
            let _ = reply.send(dir);
        }
        EngineRequest::RefreshChangedFiles(worktree) => {
            // Spawn the off-thread refresh unconditionally. If this worktree is
            // not the currently-watched one, the resulting `ChangedFilesReady`
            // (worktree-tagged) is dropped by `events.rs`. In practice the git
            // HTTP handler only refreshes the worktree it just mutated, which is
            // normally the watched one.
            engine.spawn_changed_files_refresh(std::path::PathBuf::from(worktree));
        }
        EngineRequest::PullRequestResolutionInputs(reply) => {
            let _ = reply.send((
                engine.projects.clone(),
                engine.github_host_policy(),
                engine.pr_agent_command_available(),
            ));
        }
        EngineRequest::ProjectWorktreeInputs(project_id, reply) => {
            let inputs = engine
                .projects
                .iter()
                .find(|p| p.id == project_id)
                .cloned()
                .map(|project| (project, engine.paths.clone(), engine.sessions.clone()));
            let _ = reply.send(inputs);
        }
        EngineRequest::SessionStartupLogContext(session_id, reply) => {
            let context = engine
                .sessions
                .iter()
                .find(|s| s.id == session_id)
                // A standalone agent has no project, so it has no startup-command
                // log directory either: startup commands are a project's
                // worktree provisioning step.
                .and_then(|session| {
                    session
                        .project_id()
                        .map(|project_id| (engine.paths.clone(), project_id.to_string()))
                });
            let _ = reply.send(context);
        }
        EngineRequest::ProjectStartupLogContext(project_id, reply) => {
            let context = engine
                .projects
                .iter()
                .find(|p| p.id == project_id)
                .map(|_| engine.paths.clone());
            let _ = reply.send(context);
        }
        EngineRequest::ReadRawConfig(reply) => {
            let _ = reply.send(read_raw_config(engine));
        }
        EngineRequest::WriteRawConfig(content, reply) => {
            let _ = reply.send(write_raw_config_on_engine(
                engine,
                &content,
                config_disk_ahead,
            ));
        }
        EngineRequest::FirstLoadInputs(reply) => {
            let _ = reply.send(first_load_inputs(engine));
        }
        EngineRequest::MarkVersionSeen(version, reply) => {
            let result = engine
                .session_store
                .set_last_seen_version(&version)
                .map_err(|err| format!("could not record the version as seen: {err:#}"));
            let _ = reply.send(result);
        }
    }
}

/// Validate and persist a raw `config.toml` edit from the web Monaco editor, on
/// the engine thread. Runs as a free function (not an inline closure) so the `?`
/// short-circuits read cleanly.
///
/// "Save" PERSISTS but does NOT apply: the new file is written verbatim and left
/// on disk, but `engine.config` is intentionally NOT reloaded and no
/// `config.changed` is emitted, so the running app keeps its current settings
/// until the user explicitly runs "Reload config". This is deliberate — some
/// settings (the `[server]` perimeter, the port) only take effect at restart, so
/// silently adopting an edit on save would be surprising and, for those, a no-op
/// that hides the need to restart. Reload is the single apply point.
///
/// Because memory is now behind disk, `*config_disk_ahead` is set so the next
/// config-static mutation reconciles first and cannot clobber the saved edits
/// (see the `ApplyWire` handler). On any failure nothing is written and the flag
/// is left untouched.
fn write_raw_config_on_engine(
    engine: &mut Engine,
    content: &str,
    config_disk_ahead: &mut bool,
) -> Result<(), String> {
    let parsed = dux_core::config::validate_config_str(content)
        .map_err(|e| format!("config.toml is not valid: {e}"))?;
    // The web editor must not silently weaken the server perimeter: [server]
    // host/allowed_hosts only take effect at restart, so a change here would
    // persist unreviewed. Reject perimeter edits.
    if parsed.server.host != engine.config.server.host
        || parsed.server.allowed_hosts != engine.config.server.allowed_hosts
    {
        return Err(
            "Server host/allowed_hosts cannot be changed from the web editor; \
             edit config.toml directly and restart."
                .to_string(),
        );
    }
    // Flush pending managed writes so a coalesced lazy save cannot clobber the
    // raw write, then persist the user's text verbatim.
    engine.config_writer.flush();
    dux_core::config_write::write_config_atomic(
        &engine.paths.config_path,
        content,
        dux_core::config_write::Durability::Fsync,
    )
    // Don't leak the absolute config-dir path to the client: return the
    // underlying OS error without the path-annotated context.
    .map_err(|e| format!("Could not write config.toml: {}", e.root_cause()))?;
    // Persist-only: the file is on disk, but the running config is left as-is so
    // nothing applies until an explicit reload. Mark disk as ahead of memory so a
    // later config-static mutation reconciles before its wholesale patch (which
    // would otherwise serialize the stale in-memory config over these edits).
    *config_disk_ahead = true;
    Ok(())
}

/// Why a PTY subscribe onto a tab whose last run failed is refused, in the words
/// the user gets. The socket's own close carries a fixed code and a fixed reason
/// (the client keys its do-not-retry rule off the code), so this sentence reaches
/// the user as a keyed status instead, which the web renders as a toast.
fn last_run_failed_refusal() -> String {
    "This tab's last run failed, so it will not be started by opening it. \
     Start it explicitly to try again."
        .to_string()
}

/// Handle a `SubscribePty` request. If the provider already exists, reply
/// immediately. Otherwise launch/resume the real agent provider and defer the
/// reply via a `PendingSubscribe` until the provider comes up (or times out).
fn handle_subscribe(
    engine: &mut Engine,
    pending: &mut Vec<PendingSubscribe>,
    last_pr_foregrounded: &mut Option<String>,
    status_tx: &mut StatusEmitter,
    tab_id: String,
    reply: oneshot::Sender<Result<PtySubscription, String>>,
) {
    // Opening an agent's PTY foregrounds it — refresh its PR status. What is
    // subscribed is always a TAB id (a slot tab's or an extra tab's), never a
    // session id, so resolve the owning session first. Only a GENUINE focus
    // change gets the tight foreground refresh; a reconnect/remount of the
    // already-focused agent falls back to the normal background cadence, so
    // socket blips don't re-poll `gh` (mirrors the TUI's `previously_watched`
    // gate).
    if let Some(owner) = engine.owning_session_for_tab(&tab_id) {
        if last_pr_foregrounded.as_deref() != Some(owner.as_str()) {
            engine.spawn_foreground_pr_check(&owner);
        } else {
            engine.spawn_pr_check_for_session(&owner, dux_core::engine::PR_CHECK_MIN_INTERVAL);
        }
        *last_pr_foregrounded = Some(owner);
    }
    // Opening a tab's live view is the web's "looking at it" signal: clear and
    // briefly suppress its attention flag. Stamp the subscribed TAB id, not the
    // owning session. Gated on the id resolving to a real tab so a stale deep
    // link, a race, or a retry can never leak an `agent_viewed` entry that is
    // never cleaned up. Typing then keeps it cleared via `note_pty_input` on
    // `WritePty`, and the client's periodic viewed ping keeps it down while
    // foregrounded.
    engine.note_agent_viewed_if_known(&tab_id);
    if let Some(client) = engine.providers.get(TabIdRef::new(&tab_id)) {
        let _ = reply.send(Ok(client.subscribe_with_repaint()));
        return;
    }
    // Subscribing force-launches a dormant tab deliberately: it is how selecting
    // an agent starts it in one click. It must not start a tab whose last run
    // FAILED, though, or one that cannot come up relaunches on every passive
    // attach (a retrying socket, a second browser with the pane open) with
    // nothing the user can do. Only the passive path is refused: the explicit
    // start route clears the verdict first, so a press still works.
    //
    // The socket close cannot carry the why (its code is the client's
    // do-not-retry rule and its reason is fixed), so the sentence rides the keyed
    // status controller instead, keyed on the tab in the same `tab-launch-<id>`
    // family a launch failure uses, so a second refused attach replaces the toast
    // rather than stacking another one.
    if engine.tab_last_run_failed(&tab_id) {
        let _ = status_tx.send(
            WireStatus::new("warning", last_run_failed_refusal())
                .with_key(format!("tab-launch-{tab_id}")),
        );
        let _ = reply.send(Err(last_run_failed_refusal()));
        return;
    }
    match launch_agent(engine, &tab_id) {
        Ok(()) => pending.push(PendingSubscribe {
            tab_id: TabId::new(tab_id),
            reply: Some(reply),
            deadline: Instant::now() + LAUNCH_TIMEOUT,
        }),
        Err(e) => {
            let _ = reply.send(Err(e));
        }
    }
}

/// Resolve `provider` (or the session's project default) and create a Support
/// tab, replying `(tab_id, provider)`. Mirrors the `create_terminal` direct
/// return: the launch is dispatched fire-and-forget inside `Engine::create_tab`.
fn create_agent_tab_inner(
    engine: &mut Engine,
    session_id: &str,
    provider: Option<String>,
) -> Result<(String, String), String> {
    let session = engine
        .sessions
        .iter()
        .find(|s| s.id == session_id)
        .cloned()
        .ok_or_else(|| format!("unknown session {session_id}"))?;
    let provider = match provider {
        Some(p) => {
            if !engine.config.providers.commands.contains_key(&p) {
                return Err(format!(
                    "Provider \"{p}\" is not configured. Pick one of the configured providers."
                ));
            }
            dux_core::model::ProviderKind::new(p)
        }
        // The single-source new-tab default: the owning project's provider,
        // else the global config default (`Engine::default_provider_for_new_tab`,
        // shared with the TUI).
        None => engine.default_provider_for_new_tab(session.project_id()),
    };
    let provider_str = provider.as_str().to_string();
    let tab_id = engine
        .create_tab(session_id, provider, (24, 80))
        .map_err(|e| e.to_string())?;
    Ok((tab_id, provider_str))
}

/// Launch (or resume) the real provider for a subscribed id. The id names either
/// a session's slot tab or one of its extra tabs; either is resume-eligible
/// per-provider (see `tab_resume_decision`).
/// The provider is NOT inserted here: the dispatched launch runs in a background
/// worker and the provider appears later via the worker-event drain
/// (`process_agent_launch_ready`). The caller's `PendingSubscribe` waits for it.
/// This subscribe-launches path is the web "start a dormant extra tab" action.
fn launch_agent(engine: &mut Engine, subscribed_id: &str) -> Result<(), String> {
    // A launch is already running for THIS id (session or tab): just wait for it.
    // The guard keys by the subscribed id, matching the tab-keyed in-flight lock.
    // Transport-facing: the subscribed id is a tab id (companion terminals ride
    // a different request), named here at the door.
    let subscribed_tab = TabIdRef::new(subscribed_id);
    if engine.is_in_flight(&InFlightKey::AgentLaunch(subscribed_tab.to_owned())) {
        return Ok(());
    }
    // session-slot tab: the subscribed id names some agent's first tab ->
    // resume-eligible reconnect.
    if let Some(session) = engine.session_for_slot_tab(subscribed_tab).cloned() {
        // Derive the message from the ACTUAL resume decision, not just
        // `should_resume_session`: a live same-provider extra tab downgrades the
        // session-slot launch to fresh (per-provider collision), and the toast
        // must not claim "Resumed" when the dispatch actually starts fresh.
        // The launch is for the agent's session-slot tab.
        let resume =
            engine.tab_resume_decision(&session, session.slot_tab_id(), &session.provider, true);
        // Use the SAME completion message the TUI shows on reconnect-ready rather
        // than a static "attaching…" placeholder (which left the status line stuck).
        let status_message = engine.agent_reconnect_status_message(&session, resume);
        let request = engine.build_agent_launch_request(
            session,
            resume,
            (24, 80),
            AgentLaunchKind::Reconnect { status_message },
        );
        return dispatch_launch(engine, request);
    }
    // Extra tab: resolve the owning session + the tab's own provider and launch.
    // Resume is per-provider — reopening a dormant tab resumes that provider's
    // conversation when it is the sole live tab of that provider (see
    // `tab_resume_decision`). A tab-aware status message avoids the Main-only
    // `agent_reconnect_status_message` (which would name the wrong provider).
    let tab = engine
        .agent_tabs
        .get(subscribed_tab)
        .cloned()
        .ok_or_else(|| format!("unknown session {subscribed_id}"))?;
    // Refuse to (re)launch an extra tab into a session that is mid-deletion: its
    // worktree is about to be removed, so spawning a fresh provider there would
    // race `git::remove_worktree`. Mirrors the `closing_sessions` guard in
    // `Engine::create_tab`.
    if engine.closing_sessions.contains(&tab.session_id) {
        return Err(format!(
            "session {} is being deleted; not launching its tab",
            tab.session_id
        ));
    }
    // Resolution, the per-provider resume decision, the fresh/resumed wording,
    // and the request build are the single-source `dormant_tab_launch_request`
    // (shared with the TUI's `launch_focused_extra_tab`). The tab was just
    // resolved above, so `None` is unreachable, but map it to the same
    // unknown-tab error for safety.
    let request = engine
        .dormant_tab_launch_request(subscribed_id, (24, 80))
        .ok_or_else(|| format!("unknown session {subscribed_id}"))?;
    dispatch_launch(engine, request)
}

/// Run a built launch request through the dispatch chokepoint and turn a refusal
/// into an `Err`.
///
/// The chokepoint (`Command::DispatchAgentLaunch`) refuses a launch for a closing
/// session (or an in-flight collision) by returning `Ok(view { launched: false,
/// .. })`, not an `Err`. Ignoring `view.launched` swallows that refusal silently:
/// never logged, never surfaced, leaving `PendingSubscribe` to fail-fast later
/// with a generic "check dux.log" message and nothing in the log to check. Both
/// launch arms answer through here so neither can drift back into swallowing it.
fn dispatch_launch(engine: &mut Engine, request: AgentLaunchRequest) -> Result<(), String> {
    let reaction = engine
        .apply(Command::DispatchAgentLaunch {
            request: Box::new(request),
        })
        .map_err(|e| e.to_string())?;
    if let EventReaction::DispatchAgentLaunchView(view) = &reaction
        && !view.launched
    {
        let message = view
            .status
            .as_ref()
            .map(|s| s.message.clone())
            .unwrap_or_else(|| "Agent launch was refused.".to_string());
        return Err(message);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::bootstrap_engine;
    use dux_core::config::{DuxPaths, server_restart_settings_changed};
    use dux_core::statusline::FINAL_REPLAY_WINDOW;

    fn temp_paths() -> (tempfile::TempDir, DuxPaths) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let paths = DuxPaths {
            root: root.clone(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
        };
        std::fs::create_dir_all(&paths.worktrees_root).unwrap();
        (tmp, paths)
    }

    /// The pre-consume seam announces a reload the drainer has not applied yet, so
    /// it must not move the caps the routes answer on: an apply that fails would
    /// leave every route running the incoming limits against the old config.
    #[test]
    fn announcing_a_reload_leaves_the_live_limits_where_they_were() {
        let (_tmp, paths) = temp_paths();
        let engine = bootstrap_engine(&paths).expect("engine");
        let (handle, ends) = build_actor_channels(&engine);
        let limits = handle.live_limits();
        limits.set_search_index_max_files(9);
        limits.set_access_log(false);
        let mut svc = EngineService::new(&engine, ends, ShutdownEcho::Silent);

        let mut config = engine.config.clone();
        config.server.search_index_max_files = 4321;
        config.server.access_log = true;
        svc.announce_config_reload(
            &engine,
            &EventReaction::ApplyReloadedConfig(Box::new(config)),
        );

        assert_eq!(limits.search_index_max_files(), 9);
        assert!(!limits.access_log());
    }

    /// And the post-apply half is what moves them, with the section the drainer
    /// adopted.
    #[test]
    fn noting_an_applied_config_moves_the_live_limits() {
        let (_tmp, paths) = temp_paths();
        let engine = bootstrap_engine(&paths).expect("engine");
        let (handle, ends) = build_actor_channels(&engine);
        let limits = handle.live_limits();
        limits.set_search_index_max_files(9);
        limits.set_access_log(false);
        let svc = EngineService::new(&engine, ends, ShutdownEcho::Silent);

        let mut server = engine.config.server.clone();
        server.search_index_max_files = 4321;
        server.access_log = true;
        svc.note_config_applied(&server);

        assert_eq!(limits.search_index_max_files(), 4321);
        assert!(limits.access_log());
    }

    /// A change made by the OTHER surface reaches a browser inside one spine-check
    /// period, and it is the mutation bump that carries it.
    ///
    /// The terminal UI applies commands straight to the engine, over channels this
    /// crate never observes, so the request drain's own `request_mutates_spine`
    /// answers cannot see them. Without the bump the change waits for the slow
    /// self-healing backstop, which at the TUI's stretched cadence is seconds. Both
    /// halves are asserted here, because "the bump helps" is only interesting next
    /// to "without it, nothing happens".
    #[test]
    fn an_engine_change_the_web_cannot_see_needs_the_mutation_bump_to_reach_clients() {
        let (_tmp, paths) = temp_paths();
        let mut engine = bootstrap_engine(&paths).expect("engine");
        let (handle, ends) = build_actor_channels(&engine);
        let mut spine_changes = handle.subscribe_spine_changes();
        let mut svc = EngineService::new(&engine, ends, ShutdownEcho::Silent);

        // Stand in for the terminal UI mutating shared state behind the web
        // layer's back: no request was drained, so nothing here knows.
        engine.sessions.push(sample_session(
            "from-the-terminal",
            "project-1",
            "a-new-agent",
            "/tmp/a-new-agent",
        ));

        // One whole check period with no bump: the gate stays shut.
        for _ in 0..SPINE_CHECK_TICK_INTERVAL {
            svc.check_spine(&engine);
        }
        assert!(
            spine_changes.try_recv().is_err(),
            "without the bump the web layer has no signal, so nothing is emitted yet"
        );

        // The bump is what the terminal UI's apply count buys.
        svc.note_mutation();
        for _ in 0..SPINE_CHECK_TICK_INTERVAL {
            svc.check_spine(&engine);
        }
        assert!(
            spine_changes.try_recv().is_ok(),
            "the bump must carry a terminal-side change to clients within one check period"
        );
        drop(handle);
    }

    /// THE STATUS CONTRACT, in concurrent mode.
    ///
    /// A worker-completing final reaches browsers, because it rides the drain
    /// seam. A status the terminal UI sets by hand does not, because it never
    /// becomes a reaction the drain sees, so it never reaches this fanout at all.
    /// Only the first half can regress silently (a fanout that stopped emitting
    /// looks like nothing happening), so that is the half asserted; the second is
    /// asserted negatively, by fanning out a reaction that carries no status and
    /// checking the channel stays quiet.
    ///
    /// Web-originated statuses are a third thing entirely and are unaffected here:
    /// they are scoped per connection inside `handle_request`, before any of this.
    #[test]
    fn the_drain_seam_carries_worker_finals_to_clients_and_nothing_else() {
        let (_tmp, paths) = temp_paths();
        let mut engine = bootstrap_engine(&paths).expect("engine");
        let (handle, ends) = build_actor_channels(&engine);
        let mut statuses = handle.subscribe_status();
        let mut svc = EngineService::new(&engine, ends, ShutdownEcho::Silent);

        // A reaction with no status of its own: the fanout must stay quiet, which
        // is what makes the positive case below mean something.
        svc.fanout_reaction(
            &mut engine,
            &EventReaction::RebuildLeftItems,
            FollowupRouting::ByOrigin,
        );
        assert!(
            statuses.try_recv().is_err(),
            "a view-refresh reaction is not news for a browser"
        );

        // A worker's final. This is the one a browser has to see: the operation it
        // reports may well have been started from the terminal, and "it finished"
        // is not terminal-only chatter.
        svc.fanout_reaction(
            &mut engine,
            &EventReaction::Status(dux_core::engine::StatusUpdate::info(
                "Pulled main.".to_string(),
            )),
            FollowupRouting::ByOrigin,
        );
        let emitted = statuses
            .try_recv()
            .expect("a worker-completing final must reach clients through the seam");
        assert_eq!(emitted.message, "Pulled main.");
        drop(handle);
    }

    /// A pruned agent whose exit detached it, as `prune_exited_ptys` would report.
    fn pruned_agent(label: &str) -> dux_core::engine::PrunedPty {
        dux_core::engine::PrunedPty {
            kind: PrunedPtyKind::Agent,
            id: "s1".to_string(),
            owner: Some(dux_core::model::TerminalOwner::Session("s1".to_string())),
            agent_detached: true,
            label: label.to_string(),
            tab_closed: false,
            exit_success: Some(false),
            is_minimal: false,
            output_excerpt: String::new(),
            read_error: None,
        }
    }

    /// An agent that exits while the TERMINAL UI is the sweeper still reaches
    /// browsers, with the same notice and the same change gate the web layer's own
    /// sweep would have produced.
    ///
    /// The sweeps have one runner per process, so in concurrent mode
    /// `run_maintenance` does not run at all. Without this lane the "Agent exited."
    /// status is emitted by nobody, and the row's disappearance waits for the slow
    /// fingerprint backstop.
    #[test]
    fn a_prune_the_terminal_ui_swept_still_reaches_browsers() {
        let (_tmp, paths) = temp_paths();
        let engine = bootstrap_engine(&paths).expect("engine");
        let (handle, ends) = build_actor_channels(&engine);
        let mut statuses = handle.subscribe_status();
        let mut svc = EngineService::new(&engine, ends, ShutdownEcho::Silent);

        let before = svc.mutation_version;
        svc.note_drained_maintenance(&dux_core::background_serve::DrainedMaintenance {
            pruned: vec![pruned_agent("my-agent")],
            foregrounds_changed: false,
        });

        let emitted = statuses
            .try_recv()
            .expect("a browser must be told the agent exited");
        assert_eq!(emitted.message, "Agent \"my-agent\" exited.");
        assert_eq!(emitted.tone, "warning");
        // And the change gate is open, so the row's disappearance rides the NEXT
        // check (the fingerprint compare, ~250ms) instead of the ~2s backstop.
        assert_ne!(
            svc.mutation_version, before,
            "a prune must open the spine gate, or the vanished row waits for the backstop"
        );
        drop(handle);
    }

    /// A terminal-foreground change the terminal UI observed opens the gate too,
    /// and an unchanged refresh does not: the flag is the throttled sweep's own
    /// answer, not "the sweep ran".
    #[test]
    fn a_foreground_change_the_terminal_ui_observed_opens_the_gate() {
        let (_tmp, paths) = temp_paths();
        let engine = bootstrap_engine(&paths).expect("engine");
        let (handle, ends) = build_actor_channels(&engine);
        let mut svc = EngineService::new(&engine, ends, ShutdownEcho::Silent);

        let before = svc.mutation_version;
        svc.note_drained_maintenance(&dux_core::background_serve::DrainedMaintenance {
            pruned: Vec::new(),
            foregrounds_changed: false,
        });
        assert_eq!(
            svc.mutation_version, before,
            "a throttled or unchanged refresh must not reopen the gate every interval"
        );

        svc.note_drained_maintenance(&dux_core::background_serve::DrainedMaintenance {
            pruned: Vec::new(),
            foregrounds_changed: true,
        });
        assert_ne!(
            svc.mutation_version, before,
            "a changed foreground_cmd is spine state and must open the gate"
        );
        drop(handle);
    }

    fn sample_session(
        id: &str,
        project_id: &str,
        branch: &str,
        worktree: &str,
    ) -> dux_core::model::AgentSession {
        let now = chrono::Utc::now();
        dux_core::model::AgentSession {
            id: id.to_string(),
            slot_tab_id: format!("{id}-slot"),
            provider: dux_core::model::ProviderKind::new("claude"),
            title: Some(format!("{id}-title")),
            started_providers: Vec::new(),
            desired_running: true,
            auto_reopen_enabled: false,
            status: dux_core::model::SessionStatus::Detached,
            created_at: now,
            updated_at: now,
            last_focused_tab: None,
            workspace: dux_core::model::AgentWorkspace::Managed(
                dux_core::model::ManagedWorkspace {
                    project_id: project_id.to_string(),
                    project_path: None,
                    source_branch: "main".to_string(),
                    branch_name: branch.to_string(),
                    initial_branch: branch.to_string(),
                    branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
                    worktree_path: worktree.to_string(),
                },
            ),
        }
    }

    /// The server's startup auto-reopen pass: after `bootstrap_engine` (which
    /// settles restored statuses), every core-eligible candidate gets a real
    /// launch dispatched through the shared chokepoint, and nothing else does.
    /// The in-flight marker is the observable seam: `DispatchAgentLaunch`
    /// registers `InFlightKey::AgentLaunch(session id)` synchronously, while
    /// the actual provider spawn runs in a background worker this test never
    /// drains.
    #[test]
    fn startup_auto_reopen_dispatches_launches_for_eligible_sessions_only() {
        let (_tmp, paths) = temp_paths();
        // The global switch defaults OFF; the pass must honor the config file
        // the read-only bootstrap load reads.
        std::fs::write(&paths.config_path, "[ui]\nauto_reopen_agents = true\n").unwrap();
        {
            let store = dux_core::storage::SessionStore::open(&paths.sessions_db_path).unwrap();
            // Eligible: reopen intent + per-agent opt-in + existing worktree.
            let mut eligible = sample_session(
                "s-reopen",
                "p1",
                "feat",
                paths.root.to_string_lossy().as_ref(),
            );
            eligible.auto_reopen_enabled = true;
            store.create_session(&eligible).unwrap();
            // Same intent but the per-agent opt-in is off: must be skipped.
            let opted_out = sample_session(
                "s-optout",
                "p1",
                "other",
                paths.root.to_string_lossy().as_ref(),
            );
            store.create_session(&opted_out).unwrap();
        }
        let mut engine = bootstrap_engine(&paths).expect("bootstrap");

        let launched = auto_reopen_agents_on_startup(&mut engine);

        assert_eq!(launched, 1, "exactly the eligible session launches");
        assert!(
            engine.is_in_flight(&dux_core::engine::InFlightKey::AgentLaunch(
                engine
                    .slot_tab_id_of(dux_core::ids::SessionIdRef::new("s-reopen"))
                    .to_owned()
            )),
            "the eligible session's launch must be dispatched (in-flight)"
        );
        assert!(
            !engine.is_in_flight(&dux_core::engine::InFlightKey::AgentLaunch(TabId::new(
                "s-optout"
            ))),
            "the opted-out session must not launch"
        );
    }

    #[test]
    fn startup_auto_reopen_is_a_noop_with_the_global_switch_off() {
        let (_tmp, paths) = temp_paths();
        {
            let store = dux_core::storage::SessionStore::open(&paths.sessions_db_path).unwrap();
            let mut eligible = sample_session(
                "s-reopen",
                "p1",
                "feat",
                paths.root.to_string_lossy().as_ref(),
            );
            eligible.auto_reopen_enabled = true;
            store.upsert_session(&eligible).unwrap();
        }
        // No config file: `ui.auto_reopen_agents` stays at its false default.
        let mut engine = bootstrap_engine(&paths).expect("bootstrap");
        assert_eq!(auto_reopen_agents_on_startup(&mut engine), 0);
    }

    #[test]
    fn launch_agent_surfaces_closing_session_refusal_as_err() {
        // The chokepoint's closing-session refusal comes back as
        // `Ok(view { launched: false, .. })`, not an `Err`; the session-slot
        // branch of `launch_agent` must surface it as an `Err` carrying the
        // refusal's status message rather than reading only the dispatch
        // result.
        let (_tmp, paths) = temp_paths();
        {
            let store = dux_core::storage::SessionStore::open(&paths.sessions_db_path).unwrap();
            store
                .create_session(&sample_session(
                    "s1",
                    "p1",
                    "feat",
                    paths.root.to_string_lossy().as_ref(),
                ))
                .unwrap();
        }
        let mut engine = bootstrap_engine(&paths).expect("bootstrap");
        engine.closing_sessions.insert("s1".to_string());

        let slot = engine
            .slot_tab_id_of(dux_core::ids::SessionIdRef::new("s1"))
            .to_string();
        let err = launch_agent(&mut engine, &slot).expect_err("closing session must refuse");
        assert!(
            err.contains("being deleted"),
            "refusal message should be surfaced verbatim: {err}"
        );
    }

    /// Opening a tab's PTY socket is what starts a dormant tab in one click, and
    /// that is exactly why it must refuse a tab whose last run FAILED: otherwise
    /// a tab that cannot come up relaunches on every passive attach (a retrying
    /// socket, a second browser that still had the pane open) forever. The
    /// explicit start route is the way past it, and it clears the verdict first.
    /// The refusal is also SAID: the socket's close code is the client's
    /// do-not-retry rule and carries no room for a reason, so the sentence rides
    /// the keyed status controller and reaches the browser as a toast.
    #[test]
    fn subscribing_does_not_relaunch_a_tab_whose_last_run_failed() {
        let (_tmp, paths) = temp_paths();
        {
            let store = dux_core::storage::SessionStore::open(&paths.sessions_db_path).unwrap();
            store
                .create_session(&sample_session(
                    "s1",
                    "p1",
                    "feat",
                    paths.root.to_string_lossy().as_ref(),
                ))
                .unwrap();
        }
        let mut engine = bootstrap_engine(&paths).expect("bootstrap");
        let slot = engine
            .slot_tab_id_of(dux_core::ids::SessionIdRef::new("s1"))
            .to_owned();
        engine.mark_tab_run_failed(&slot);

        let mut pending = Vec::new();
        let mut last_pr = None;
        let (status_tx, mut status_rx) = broadcast::channel(8);
        let (clear_tx, _clear_rx) = broadcast::channel(8);
        let (snapshot_tx, _snapshot_rx) = watch::channel(Vec::new());
        let mut status = StatusEmitter::new(status_tx, clear_tx, snapshot_tx, Default::default());
        let (tx, rx) = oneshot::channel();
        handle_subscribe(
            &mut engine,
            &mut pending,
            &mut last_pr,
            &mut status,
            slot.as_str().to_string(),
            tx,
        );

        let emitted = status_rx.try_recv().expect("the refusal is said out loud");
        assert_eq!(emitted.tone, "warning");
        assert!(
            emitted.message.contains("Start it explicitly"),
            "the toast must name the way forward: {}",
            emitted.message
        );
        assert_eq!(
            emitted.key.as_deref(),
            Some(format!("tab-launch-{}", slot.as_str()).as_str()),
            "keyed on the tab, so a second refused attach replaces the toast"
        );
        assert!(pending.is_empty(), "no launch may be waited on");
        assert!(
            !engine.is_in_flight(&dux_core::engine::InFlightKey::AgentLaunch(slot.clone())),
            "no launch may be dispatched by a passive attach onto a failed tab"
        );
        match rx.blocking_recv().expect("a reply") {
            Ok(_) => panic!("subscribing a failed tab must be refused"),
            Err(err) => assert!(
                err.contains("Start it explicitly"),
                "the refusal must name the way forward: {err}"
            ),
        }
    }

    /// An engine whose agent has ALREADY had its slot promoted: the claude tab
    /// that held the slot is closed over a codex extra, so the slot is a
    /// row-backed tab called `t2` whose id is not the session id. This is the
    /// shape the web's launch and subscribe paths have to route correctly.
    fn engine_with_a_promoted_codex_slot(paths: &DuxPaths) -> Engine {
        {
            let store = dux_core::storage::SessionStore::open(&paths.sessions_db_path).unwrap();
            let mut session =
                sample_session("s1", "p1", "feat", paths.root.to_string_lossy().as_ref());
            session.started_providers = vec!["claude".into(), "codex".into()];
            store.create_session(&session).unwrap();
            store
                .insert_agent_tab(&dux_core::model::AgentTab {
                    id: "t2".to_string(),
                    session_id: "s1".to_string(),
                    provider: dux_core::model::ProviderKind::new("codex"),
                    sort_order: 1,
                    created_at: chrono::Utc::now(),
                })
                .unwrap();
        }
        let mut engine = bootstrap_engine(paths).expect("bootstrap");
        let outcome = engine.close_tab("s1", "s1-slot").expect("promotion");
        assert_eq!(
            outcome.promoted.as_deref(),
            Some(dux_core::ids::TabIdRef::new("t2")),
            "fixture precondition: the codex tab is in the slot now"
        );
        engine
    }

    /// The web launches a subscribed id through one of two arms, and the arm is
    /// chosen by the slot resolver rather than by the id's shape. A promoted
    /// slot must take the SESSION-SLOT arm: it is the agent's own tab now, so its
    /// launch is the agent's reconnect, not a tab-scoped relaunch.
    ///
    /// The agent is mid-deletion on purpose. Both arms refuse that, in different
    /// words, and the wording is what makes the arm observable without spawning
    /// a provider.
    #[test]
    fn launch_agent_after_a_promotion_takes_the_session_slot_arm() {
        let (_tmp, paths) = temp_paths();
        let mut engine = engine_with_a_promoted_codex_slot(&paths);
        engine.closing_sessions.insert("s1".to_string());

        let err = launch_agent(&mut engine, "t2").expect_err("closing session must refuse");

        assert!(
            err.contains("cannot be launched"),
            "the session-slot arm's refusal (the dispatch chokepoint's) is expected: {err}"
        );
        assert!(
            !err.contains("not launching its tab"),
            "that is the EXTRA-tab arm's refusal, so the promoted slot took the wrong arm: {err}"
        );
    }

    /// The passive-attach refusal is keyed by tab, and a promoted slot is just
    /// another tab id there: subscribing to one whose last run failed must
    /// refuse rather than relaunch it on every retry.
    #[test]
    fn subscribing_a_promoted_slot_whose_last_run_failed_is_refused() {
        let (_tmp, paths) = temp_paths();
        let mut engine = engine_with_a_promoted_codex_slot(&paths);
        engine.mark_tab_run_failed(dux_core::ids::TabIdRef::new("t2"));

        let mut pending = Vec::new();
        let mut last_pr = None;
        let (status_tx, mut status_rx) = broadcast::channel(8);
        let (clear_tx, _clear_rx) = broadcast::channel(8);
        let (snapshot_tx, _snapshot_rx) = watch::channel(Vec::new());
        let mut status = StatusEmitter::new(status_tx, clear_tx, snapshot_tx, Default::default());
        let (tx, rx) = oneshot::channel();
        handle_subscribe(
            &mut engine,
            &mut pending,
            &mut last_pr,
            &mut status,
            "t2".to_string(),
            tx,
        );

        let emitted = status_rx.try_recv().expect("the refusal is said out loud");
        assert_eq!(
            emitted.key.as_deref(),
            Some("tab-launch-t2"),
            "keyed on the promoted tab's own id"
        );
        assert!(pending.is_empty(), "no launch may be waited on");
        assert!(
            !engine.is_in_flight(&dux_core::engine::InFlightKey::AgentLaunch(TabId::new(
                "t2"
            ))),
            "a passive attach onto a failed promoted slot must not relaunch it"
        );
        assert!(rx.blocking_recv().expect("a reply").is_err());
    }

    fn pruned(
        kind: PrunedPtyKind,
        detached: bool,
        tab_closed: bool,
    ) -> dux_core::engine::PrunedPty {
        dux_core::engine::PrunedPty {
            kind,
            id: "t1".to_string(),
            owner: None,
            agent_detached: detached,
            label: "Claude".to_string(),
            tab_closed,
            exit_success: Some(tab_closed),
            is_minimal: false,
            output_excerpt: String::new(),
            read_error: None,
        }
    }

    /// A reaping whose row leaves the screen says nothing; a reaping that leaves
    /// something behind still has to explain itself.
    #[test]
    fn a_reaped_pty_is_announced_only_when_something_is_left_on_screen() {
        assert_eq!(
            prune_wire_status(&pruned(PrunedPtyKind::Agent, false, true)),
            None,
            "the pill left the strip in the same sweep"
        );
        assert_eq!(
            prune_wire_status(&pruned(PrunedPtyKind::Terminal, false, false)),
            None,
            "the terminal's row left the sidebar in the same sweep"
        );

        let dormant = prune_wire_status(&pruned(PrunedPtyKind::Agent, false, false))
            .expect("a dormant pill is indistinguishable from a never-launched one");
        assert_eq!(dormant.tone, "info");
        assert!(dormant.message.contains("Tab (Claude) exited."));
        assert!(!dormant.quiet, "there is nothing else to say it");

        let detached = prune_wire_status(&pruned(PrunedPtyKind::Agent, true, false))
            .expect("losing the whole agent stays a warning");
        assert_eq!(detached.tone, "warning");
        assert!(!detached.quiet);
    }

    /// The quiet flag is honored at the one gate, so a status marked quiet
    /// reaches neither the live broadcast nor the replay snapshot a joining tab
    /// reads.
    #[test]
    fn a_quiet_status_is_neither_broadcast_nor_replayed() {
        let (status_tx, mut status_rx) = broadcast::channel(8);
        let (clear_tx, _clear_rx) = broadcast::channel(8);
        let (snapshot_tx, snapshot_rx) = watch::channel(Vec::new());
        let mut status = StatusEmitter::new(status_tx, clear_tx, snapshot_tx, Default::default());

        let _ = status.send(WireStatus::new("info", "the pane you are looking at moved").quiet());
        assert!(
            status_rx.try_recv().is_err(),
            "a quiet status raises no toast"
        );
        assert!(
            snapshot_rx.borrow().is_empty(),
            "and never joins the snapshot replayed to a new tab"
        );

        let _ = status.send(WireStatus::new("info", "loud"));
        assert_eq!(
            status_rx
                .try_recv()
                .expect("an unmarked status still goes out")
                .message,
            "loud"
        );
    }

    /// The per-tab REST verbs (`DELETE`, `PATCH`, `POST .../start`) accept a tab
    /// either because it IS the session's slot or because its stored row belongs
    /// to that session. A promoted slot passes on the first test and, by design,
    /// fails the second: its row is the slot row, so it is no longer an extra.
    /// Both halves are asserted, because a route that only asked the second
    /// question would 404 the agent's own tab.
    #[tokio::test]
    async fn the_per_tab_routes_still_recognise_a_promoted_slot() {
        let (_tmp, paths) = temp_paths();
        let engine = engine_with_a_promoted_codex_slot(&paths);
        let (handle, _join) = spawn_engine_thread(engine);

        assert_eq!(
            handle.slot_tab_id("s1".to_string()).await.as_deref(),
            Some("t2")
        );
        assert!(handle.is_slot_tab("s1".to_string(), "t2").await);
        assert!(!handle.is_slot_tab("s1".to_string(), "s1").await);
        assert_eq!(
            handle.tab_session("t2".to_string()).await,
            None,
            "the promoted slot is not an extra tab any more"
        );
    }

    #[tokio::test]
    async fn apply_wire_toggle_reflects_in_spine_and_emits_sessions_change() {
        let (_tmp, paths) = temp_paths();
        {
            let store = dux_core::storage::SessionStore::open(&paths.sessions_db_path).unwrap();
            store
                .create_session(&sample_session(
                    "s1",
                    "p1",
                    "feat",
                    paths.root.to_string_lossy().as_ref(),
                ))
                .unwrap();
        }
        let engine = bootstrap_engine(&paths).expect("bootstrap");
        let (handle, _join) = spawn_engine_thread(engine);

        // Subscribe to spine changes BEFORE the mutation so we observe the loop's
        // fingerprint detection fire `SpineChange::Sessions`.
        let mut spine_rx = handle.subscribe_spine_changes();

        let outcome = handle
            .apply_wire(WireCommand::ToggleAgentAutoReopen {
                session_id: "s1".to_string(),
                enabled: true,
            })
            .await
            .expect("apply");
        assert!(outcome.status.is_some());

        // The session toggle changes the sessions half of the spine, so the loop
        // emits `SpineChange::Sessions`.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let saw_sessions = loop {
            match tokio::time::timeout(std::time::Duration::from_millis(200), spine_rx.recv()).await
            {
                Ok(Ok(SpineChange::Sessions)) => break true,
                Ok(Ok(SpineChange::Projects)) => {}
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => break false,
                Ok(Err(_)) | Err(_) => {}
            }
            if std::time::Instant::now() >= deadline {
                break false;
            }
        };
        assert!(saw_sessions, "toggle must fire SpineChange::Sessions");

        // The spine read now reflects the toggle.
        let spine = handle.spine().await.expect("spine");
        let session = spine
            .sessions
            .iter()
            .find(|s| s.id == "s1")
            .expect("session s1 present");
        assert!(
            session.auto_reopen_enabled,
            "the spine must reflect the auto-reopen toggle"
        );
    }

    #[tokio::test]
    async fn apply_wire_status_is_broadcast_live_and_briefly_replayable() {
        // A synchronous command result is ALSO published to the live broadcast,
        // not just returned to the requester, so it reaches every client that is
        // currently attached, AND it enters the snapshot so a socket that
        // dropped across the command still learns the outcome when it comes
        // back. It stops being replayable on `FINAL_REPLAY_WINDOW`, which the
        // controller and emitter tests pin with a controlled clock rather than
        // by making this one wait thirty seconds.
        let (_tmp, paths) = temp_paths();
        {
            let store = dux_core::storage::SessionStore::open(&paths.sessions_db_path).unwrap();
            store
                .create_session(&sample_session(
                    "s1",
                    "p1",
                    "feat",
                    paths.root.to_string_lossy().as_ref(),
                ))
                .unwrap();
        }
        let engine = bootstrap_engine(&paths).expect("bootstrap");
        let (handle, _join) = spawn_engine_thread(engine);

        // Subscribe to the LIVE broadcast before issuing the command: the
        // broadcast is now the ONLY delivery path for a final, so a regression
        // that dropped it would leave the status reaching nobody at all.
        let mut status_rx = handle.subscribe_status();

        let outcome = handle
            .apply_wire(WireCommand::ToggleAgentAutoReopen {
                session_id: "s1".to_string(),
                enabled: true,
            })
            .await
            .expect("apply");
        // The reply still carries the status (the requester's instant ack)…
        let want = outcome.status.expect("command produced a status").message;

        // …and the snapshot a reconnecting client would read holds it too.
        let snap = handle.status_snapshot();
        assert!(
            snap.iter().any(|s| s.message == want),
            "a just-finished command's status must be replayable: {snap:?}"
        );

        // …AND it is delivered live on the broadcast to every connected client.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let delivered = loop {
            match tokio::time::timeout(std::time::Duration::from_millis(200), status_rx.recv())
                .await
            {
                Ok(Ok(s)) if s.message == want => break true,
                Ok(Ok(_)) => {}
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => break false,
                Ok(Err(_)) | Err(_) => {}
            }
            if std::time::Instant::now() >= deadline {
                break false;
            }
        };
        assert!(
            delivered,
            "command status was not broadcast to live clients"
        );
    }

    #[tokio::test]
    async fn shutdown_acks_and_stops_the_engine_thread() {
        let (_tmp, paths) = temp_paths();
        {
            let store = dux_core::storage::SessionStore::open(&paths.sessions_db_path).unwrap();
            store
                .create_session(&sample_session(
                    "s1",
                    "p1",
                    "feat",
                    paths.root.to_string_lossy().as_ref(),
                ))
                .unwrap();
        }
        let mut engine = bootstrap_engine(&paths).expect("bootstrap");
        // A `cat`-backed companion terminal that must be SIGTERMed on shutdown.
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];
        engine
            .create_companion_terminal("s1", 24, 80)
            .expect("create companion terminal");
        let (handle, join) = spawn_engine_thread(engine);

        // Shutdown should ack quickly: cat exits on SIGTERM well inside the grace
        // window, so the configured shutdown_timeout_seconds is never reached.
        tokio::time::timeout(std::time::Duration::from_secs(5), handle.shutdown())
            .await
            .expect("shutdown acked");

        // The engine thread has stopped, so further requests fail.
        let res = handle
            .apply_wire(WireCommand::ToggleAgentAutoReopen {
                session_id: "s1".to_string(),
                enabled: true,
            })
            .await;
        assert!(res.is_err(), "requests should fail after shutdown");

        // The thread should have exited; join in a blocking task to avoid blocking
        // the async runtime.
        tokio::task::spawn_blocking(move || join.join())
            .await
            .expect("join task")
            .expect("engine thread joined");
    }

    // -----------------------------------------------------------------------
    // StatusEmitter unit tests (no Engine, no I/O — channels only)
    // -----------------------------------------------------------------------

    /// Build a `StatusEmitter` directly from inline channels (no engine needed)
    /// so the shape of the struct and its snapshot behaviour can be tested
    /// without spawning a thread. Mirrors the channel setup in
    /// `build_actor_channels`.
    fn make_emitter() -> (StatusEmitter, watch::Receiver<Vec<KeyedWireStatus>>) {
        let (tx, _rx) = broadcast::channel::<WireStatus>(16);
        let (clear_tx, _crx) = broadcast::channel::<Option<String>>(16);
        let (snap_tx, snap_rx) = watch::channel::<Vec<KeyedWireStatus>>(vec![]);
        let emitter = StatusEmitter {
            tx,
            clear_tx,
            snapshot_tx: snap_tx,
            controller: KeyedStatusController::emitting_finals(),
            generations: std::collections::HashMap::new(),
        };
        (emitter, snap_rx)
    }

    #[test]
    fn emitter_snapshot_holds_all_open_keyed_statuses() {
        // Both a keyed "pull" busy and a keyed "launch" busy must appear in the
        // snapshot together so a reconnecting client receives every active toast,
        // not just the latest one.
        let (mut e, snap_rx) = make_emitter();
        let _ = e.send(WireStatus::keyed("pull", "busy", "Pulling\u{2026}"));
        let _ = e.send(WireStatus::keyed("launch", "busy", "Launching\u{2026}"));
        let snap = snap_rx.borrow().clone();
        assert_eq!(snap.len(), 2, "snapshot must list both open busys");
        assert!(
            snap.iter().any(|s| s.key.as_deref() == Some("pull")),
            "pull must appear in snapshot"
        );
        assert!(
            snap.iter().any(|s| s.key.as_deref() == Some("launch")),
            "launch must appear in snapshot"
        );
    }

    #[test]
    fn emitter_snapshot_carries_busys_and_recent_finals_then_drops_the_finals() {
        // The replay contract in one test. The snapshot is what a NEW or a
        // RECONNECTING `/ws/events` connection is handed, so it must contain
        // work still in flight plus outcomes recent enough that the tab which
        // missed them is the same session, and nothing older.
        let (mut e, snap_rx) = make_emitter();
        let _ = e.send(WireStatus::keyed("pull", "busy", "Pulling\u{2026}"));
        let _ = e.send(WireStatus::keyed("push", "error", "Push to remote failed."));
        let _ = e.send(WireStatus::keyed("commit", "info", "Committed."));
        let _ = e.send(WireStatus::new("warning", "Heads up."));

        assert_eq!(
            snap_rx.borrow().len(),
            4,
            "everything is replayable at first: {:?}",
            snap_rx.borrow()
        );

        // A tick inside the window changes nothing: a reconnect landing here
        // still learns how the two finished operations ended, and the in-flight
        // pull is still a spinner.
        e.tick(Instant::now() + Duration::from_secs(5));
        let snap = snap_rx.borrow().clone();
        assert_eq!(
            snap.len(),
            4,
            "a reconnect inside the window is owed the finals"
        );
        assert!(
            snap.iter()
                .any(|s| s.key.as_deref() == Some("pull") && s.tone == "busy"),
            "the in-flight operation is still in flight: {snap:?}"
        );

        // Past the window the finals are gone. The pull is a WARNING rather than
        // a busy, and that is not incidental: `LAUNCH_TIMEOUT` (20s) is shorter
        // than `FINAL_REPLAY_WINDOW` (30s), so a busy that reaches the window has
        // already been upgraded by the stranded-busy rule. A busy outliving the
        // window on its own is pinned in core, where the two timeouts can be
        // driven apart.
        assert!(
            LAUNCH_TIMEOUT < FINAL_REPLAY_WINDOW,
            "the note above assumes this"
        );
        e.tick(Instant::now() + FINAL_REPLAY_WINDOW + Duration::from_secs(1));
        let snap = snap_rx.borrow().clone();
        assert_eq!(
            snap.len(),
            1,
            "only the live operation's slot is left: {snap:?}"
        );
        assert_eq!(snap[0].key.as_deref(), Some("pull"));
        assert!(
            !snap.iter().any(|s| s.key.as_deref() == Some("push")),
            "the aged error must not be replayable: {snap:?}"
        );

        // And a final ends the operation on its slot: no orphan spinner remains.
        let _ = e.send(WireStatus::keyed(
            "pull",
            "error",
            "Pull from remote failed.",
        ));
        let snap = snap_rx.borrow().clone();
        assert_eq!(snap.len(), 1, "the final replaced what was there: {snap:?}");
        assert_eq!(snap[0].tone, "error");
    }

    #[test]
    fn emitter_clear_dismisses_a_keyed_busy_and_broadcasts_its_key() {
        // The path behind `EngineRequest::ClearStatus` (the release-notes
        // route's success final): an explicit clear must drop the keyed entry
        // from the snapshot AND broadcast the key so the WS forwarder sends
        // `StatusCleared`, dismissing the busy toast without a success toast.
        let (tx, _rx) = broadcast::channel::<WireStatus>(16);
        let (clear_tx, mut crx) = broadcast::channel::<Option<String>>(16);
        let (snap_tx, snap_rx) = watch::channel::<Vec<KeyedWireStatus>>(vec![]);
        let mut e = StatusEmitter {
            tx,
            clear_tx,
            snapshot_tx: snap_tx,
            controller: KeyedStatusController::emitting_finals(),
            generations: std::collections::HashMap::new(),
        };
        let _ = e.send(WireStatus::keyed("notes", "busy", "Fetching\u{2026}"));
        assert_eq!(snap_rx.borrow().len(), 1, "busy must be in the snapshot");

        e.clear("notes".to_string());

        assert!(
            snap_rx.borrow().is_empty(),
            "an explicit clear must drop the keyed entry"
        );
        assert_eq!(
            crx.try_recv().ok(),
            Some(Some("notes".to_string())),
            "the cleared key must be broadcast for the StatusCleared frame"
        );
    }

    #[test]
    fn emitter_expires_a_final_silently_so_no_one_s_toast_is_dismissed() {
        // Under `StatusRetention::Emit` a final leaves the snapshot on the fixed
        // replay window, and it leaves QUIETLY. No `StatusCleared` key is pushed,
        // because the frame that would produce dismisses the toast on every
        // screen showing it, including a `sticky` one whose entire purpose is to
        // wait for the user. On-screen lifetime is the browser's job
        // (`lib/statusToast.ts`); the server only decides what is replayable.
        let (tx, _rx) = broadcast::channel::<WireStatus>(16);
        let (clear_tx, mut crx) = broadcast::channel::<Option<String>>(16);
        let (snap_tx, snap_rx) = watch::channel::<Vec<KeyedWireStatus>>(vec![]);
        let mut e = StatusEmitter {
            tx,
            clear_tx,
            snapshot_tx: snap_tx,
            controller: KeyedStatusController::emitting_finals(),
            generations: std::collections::HashMap::new(),
        };
        // One keyed Info, one anonymous Info, and a sticky error that must not
        // be dismissed by anything the server does here.
        let _ = e.send(WireStatus::keyed("commit", "info", "Committed."));
        let _ = e.send(WireStatus::new("info", "Saved."));
        let _ = e.send(WireStatus::keyed("del", "error", "Worktree delete failed.").sticky());
        assert_eq!(snap_rx.borrow().len(), 3, "all replayable at first");

        e.tick(Instant::now() + FINAL_REPLAY_WINDOW + Duration::from_secs(1));

        assert!(
            snap_rx.borrow().is_empty(),
            "every final must stop being replayable, got {:?}",
            snap_rx.borrow()
        );
        assert!(
            crx.try_recv().is_err(),
            "leaving the replay snapshot must dismiss nobody's toast"
        );
    }

    #[test]
    fn the_published_snapshot_always_matches_the_controller_and_is_never_spuriously_empty() {
        // Answers, deterministically: can a connection's connect-time replay
        // ever observe an EMPTY snapshot while an operation's status is open?
        //
        // No. The watch has no intermediate state (a `send` swaps the value
        // atomically and `borrow` returns a whole value), and the only empty
        // value ever published is the channel's initial one. So after every
        // operation the published vec must equal `controller.snapshot()`
        // exactly, and must be non-empty whenever the controller holds anything.
        //
        // The ORDERING that makes this matter is in `send`: the snapshot is
        // written BEFORE the live broadcast. So any observer that has seen a
        // status frame is guaranteed that a replay taken afterwards contains it,
        // and an observer keyed off something even later (a REST response) is
        // guaranteed it all the more.
        let (mut e, snap_rx) = make_emitter();
        let check =
            |snap_rx: &watch::Receiver<Vec<KeyedWireStatus>>, e: &StatusEmitter, at: &str| {
                let published = snap_rx.borrow().clone();
                let truth = e.controller.snapshot();
                assert_eq!(
                    published, truth,
                    "published snapshot diverged from the controller at {at}"
                );
                if !truth.is_empty() {
                    assert!(
                        !published.is_empty(),
                        "published an EMPTY snapshot while {} status(es) were open at {at}",
                        truth.len()
                    );
                }
            };

        let _ = e.send(WireStatus::keyed("pull", "busy", "Pulling\u{2026}"));
        check(&snap_rx, &e, "after a busy");
        let _ = e.send(WireStatus::keyed("pull", "info", "Pulled."));
        check(&snap_rx, &e, "after the final that replaces it");
        let _ = e.send(WireStatus::keyed("push", "busy", "Pushing\u{2026}"));
        check(&snap_rx, &e, "after a second operation opens");
        let _ = e.send(WireStatus::new("info", "Saved."));
        check(&snap_rx, &e, "after an unkeyed final");
        e.clear("push".to_string());
        check(&snap_rx, &e, "after an explicit clear");
        e.tick(Instant::now());
        check(&snap_rx, &e, "after an idle tick");
        e.tick(Instant::now() + FINAL_REPLAY_WINDOW + Duration::from_secs(1));
        check(&snap_rx, &e, "after the purge tick");
        assert!(
            snap_rx.borrow().is_empty(),
            "and once everything has aged out, empty is the CORRECT answer"
        );
    }

    #[test]
    fn emitter_clear_cannot_dismiss_a_sticky_toast() {
        // The web half of the sticky guard, at the layer that actually emits the
        // frame. A `status_cleared` is what dismisses a toast in the browser, so
        // if a clear could fire on a sticky key the whole feature would be
        // decorative: a clear names nothing but a key, and every engine-raised
        // final is keyed.
        let (tx, _rx) = broadcast::channel::<WireStatus>(16);
        let (clear_tx, mut crx) = broadcast::channel::<Option<String>>(16);
        let (snap_tx, snap_rx) = watch::channel::<Vec<KeyedWireStatus>>(vec![]);
        let mut e = StatusEmitter {
            tx,
            clear_tx,
            snapshot_tx: snap_tx,
            controller: KeyedStatusController::emitting_finals(),
            generations: std::collections::HashMap::new(),
        };
        let _ = e.send(WireStatus::keyed("del", "error", "Worktree delete failed.").sticky());
        assert_eq!(snap_rx.borrow().len(), 1);

        e.clear("del".to_string());

        assert_eq!(
            snap_rx.borrow().len(),
            1,
            "a sticky status must survive a clear"
        );
        assert!(
            crx.try_recv().is_err(),
            "and NO status_cleared frame may go out, or the browser dismisses it"
        );

        // The control: an ordinary final on another key still clears normally.
        let _ = e.send(WireStatus::keyed("push", "error", "Push failed."));
        e.clear("push".to_string());
        assert_eq!(
            crx.try_recv().ok(),
            Some(Some("push".to_string())),
            "a non-sticky final must still be dismissible"
        );
    }

    #[test]
    fn emitter_clear_is_a_no_op_when_a_newer_status_replaced_the_key() {
        // LOW 5: a commit-msg clear must not dismiss a concurrent same-key
        // generate's busy. The emitter stores the generation from the busy it
        // emitted and guards the clear with it; once a newer status (a fresh
        // generate) replaces the key, the stale clear becomes a no-op.
        let (mut e, snap_rx) = make_emitter();
        let key = "commit-msg:s1";

        // First generate sets the busy and remembers its generation.
        let _ = e.send(WireStatus::keyed(key, "busy", "Generating\u{2026}"));
        let stale_generation = *e.generations.get(key).expect("busy stored a generation");

        // A concurrent second generate replaces the key with a newer generation.
        let _ = e.send(WireStatus::keyed(key, "busy", "Generating again\u{2026}"));
        assert_ne!(
            *e.generations.get(key).unwrap(),
            stale_generation,
            "the replacement must bump the generation"
        );

        // Simulate the FIRST generate's clear arriving late by restoring the
        // stale generation it captured, then clearing.
        e.generations.insert(key.to_string(), stale_generation);
        e.clear(key.to_string());

        // The newer busy must still be present — the stale clear was a no-op.
        let snap = snap_rx.borrow().clone();
        assert_eq!(
            snap.len(),
            1,
            "the concurrent generate's busy must survive a stale clear"
        );
        assert_eq!(snap[0].key.as_deref(), Some(key));
        assert_eq!(snap[0].tone, "busy");
    }

    #[test]
    fn emitter_tick_rebroadcasts_a_busy_whose_operation_is_still_running() {
        // The spinner survives past the busy timeout AND is re-sent on the wire.
        // The re-send is the load-bearing half: the browser holds its own leak
        // guard on every spinner, and another frame on the key is the only thing
        // that re-arms it. Keeping the entry alive here while saying nothing
        // would just move the silent disappearance into the browser.
        let (tx, mut rx) = broadcast::channel::<WireStatus>(16);
        let (clear_tx, _crx) = broadcast::channel::<Option<String>>(16);
        let (snap_tx, snap_rx) = watch::channel::<Vec<KeyedWireStatus>>(vec![]);
        // The engine registered the create op when it started the clone.
        let live = dux_core::statusline::LiveStatusKeys::default();
        live.register("create-1");
        let mut e = StatusEmitter {
            tx,
            clear_tx,
            snapshot_tx: snap_tx,
            controller: KeyedStatusController::emitting_finals().with_live_keys(live.clone()),
            generations: std::collections::HashMap::new(),
        };
        let _ = e.send(WireStatus::keyed("create-1", "busy", "Pulling\u{2026}"));
        let _ = rx.try_recv();

        let past_timeout = Instant::now() + LAUNCH_TIMEOUT + Duration::from_secs(1);
        e.tick(past_timeout);

        let snap = snap_rx.borrow().clone();
        assert_eq!(snap.len(), 1, "{snap:?}");
        assert_eq!(
            snap[0].tone, "busy",
            "a running operation keeps its spinner: {snap:?}"
        );
        let refreshed = rx.try_recv().expect("the busy must be re-broadcast");
        assert_eq!(refreshed.tone, "busy");
        assert_eq!(refreshed.key.as_deref(), Some("create-1"));
        assert_eq!(refreshed.message, "Pulling\u{2026}");

        // And once the operation is gone, the leak guard still fires.
        live.retire("create-1");
        e.tick(past_timeout + LAUNCH_TIMEOUT * 2);
        assert_eq!(snap_rx.borrow()[0].tone, "warning");
    }

    #[test]
    fn emitter_tick_upgrades_stale_busy_and_broadcasts_upgraded_wire_status() {
        // A Busy that outlives LAUNCH_TIMEOUT is upgraded to Warning and
        // broadcast live so the client sees the spinner stop. Both slots are
        // covered: a KEYED busy and the ANONYMOUS one, whose upgrade path is a
        // separate branch in `tick`.
        let (tx, mut rx) = broadcast::channel::<WireStatus>(16);
        let (clear_tx, _crx) = broadcast::channel::<Option<String>>(16);
        let (snap_tx, snap_rx) = watch::channel::<Vec<KeyedWireStatus>>(vec![]);
        let mut e = StatusEmitter {
            tx,
            clear_tx,
            snapshot_tx: snap_tx,
            controller: KeyedStatusController::emitting_finals(),
            generations: std::collections::HashMap::new(),
        };
        // Drain the initial sends so `rx` only sees the upgrades.
        let _ = e.send(WireStatus::keyed("launch", "busy", "Launching\u{2026}"));
        let _ = e.send(WireStatus::new("busy", "Loading\u{2026}"));
        let _ = rx.try_recv();
        let _ = rx.try_recv();

        // Advance past LAUNCH_TIMEOUT.
        let timed_out = Instant::now() + LAUNCH_TIMEOUT + Duration::from_secs(1);
        e.tick(timed_out);

        // Both entries are now warnings, and both stay replayable: a tab that
        // dropped while the operation hung must reconnect to the timed-out
        // warning, not to an empty snapshot and a spinner nothing will stop.
        let snap = snap_rx.borrow().clone();
        assert_eq!(snap.len(), 2, "both upgrades remain replayable: {snap:?}");
        assert!(
            snap.iter().all(|s| s.tone == "warning"),
            "both must be warnings now: {snap:?}"
        );
        assert!(
            snap.iter().any(|s| s.key.is_none()),
            "the ANONYMOUS slot's upgrade must be there too: {snap:?}"
        );
        assert!(
            snap.iter().any(|s| s.key.as_deref() == Some("launch")),
            "and the keyed one: {snap:?}"
        );

        // Both must have been broadcast live, keyed and anonymous alike.
        let mut broadcast_keys = Vec::new();
        while let Ok(up) = rx.try_recv() {
            assert_eq!(up.tone, "warning");
            broadcast_keys.push(up.key);
        }
        assert!(
            broadcast_keys.contains(&Some("launch".to_string())),
            "keyed upgrade must be broadcast, got {broadcast_keys:?}"
        );
        assert!(
            broadcast_keys.contains(&None),
            "anonymous upgrade must be broadcast, got {broadcast_keys:?}"
        );

        // The window runs from the upgrade, so a later tick retires both.
        e.tick(timed_out + FINAL_REPLAY_WINDOW);
        assert!(
            snap_rx.borrow().is_empty(),
            "the timed-out warnings age out like any other final, got {:?}",
            snap_rx.borrow()
        );
    }

    #[test]
    fn restart_drift_is_false_for_identical_server_config() {
        let cfg = dux_core::config::ServerConfig::default();
        assert!(!server_restart_settings_changed(&cfg, &cfg.clone()));
    }

    #[test]
    fn restart_drift_detects_a_file_drop_cap_change() {
        let prev = dux_core::config::ServerConfig::default();
        let mut next = prev.clone();
        next.file_drop_max_bytes = prev.file_drop_max_bytes + 1;
        assert!(server_restart_settings_changed(&prev, &next));
        let mut next2 = prev.clone();
        next2.file_drop_max_concurrency = prev.file_drop_max_concurrency + 1;
        assert!(server_restart_settings_changed(&prev, &next2));
    }

    #[test]
    fn restart_drift_detects_port_change() {
        let prev = dux_core::config::ServerConfig::default();
        let mut next = prev.clone();
        next.port += 1;
        assert!(server_restart_settings_changed(&prev, &next));
    }

    #[test]
    fn restart_drift_never_names_the_tailscale_mode() {
        // The mode is a LIVE switch: the reload arm hands a changed value to the
        // serve's mode control, which moves the watcher, the leg and the Host
        // guard. Telling that user to restart would be false.
        let prev = dux_core::config::ServerConfig::default();
        for mode in ["yes", "no", " AUTO ", "Auto"] {
            let mut next = prev.clone();
            next.tailscale = mode.to_string();
            assert!(
                !server_restart_settings_changed(&prev, &next),
                "changing tailscale from {} to {mode} is live and must not warn",
                prev.tailscale
            );
        }
    }

    #[test]
    fn restart_drift_ignores_a_tailscale_value_that_means_the_same_mode() {
        // The value is trimmed and matched case-insensitively, so " AUTO " is the
        // mode the server is already running under. Telling that user to restart
        // would be a warning about nothing.
        let prev = dux_core::config::ServerConfig::default();
        for same in [" auto", "Auto", "AUTO ", "\tauto\t"] {
            let mut next = prev.clone();
            next.tailscale = same.to_string();
            assert!(
                !server_restart_settings_changed(&prev, &next),
                "{same:?} is the same mode as {:?} and must not warn",
                prev.tailscale
            );
        }
    }

    #[test]
    fn restart_drift_detects_host_change() {
        let prev = dux_core::config::ServerConfig::default();
        let mut next = prev.clone();
        next.host = "0.0.0.0".to_string();
        assert!(server_restart_settings_changed(&prev, &next));
    }

    #[test]
    fn restart_drift_detects_allowed_hosts_change() {
        let prev = dux_core::config::ServerConfig::default();
        let mut next = prev.clone();
        next.allowed_hosts.push("box.tailnet.ts.net".to_string());
        assert!(server_restart_settings_changed(&prev, &next));
    }

    #[test]
    fn restart_drift_detects_max_websocket_events_connections_change() {
        // Each per-class connection-cap semaphore is built once at startup, so
        // changing a cap must surface as a restart-needed warning like the other
        // startup-bound settings, not be silently swallowed by a live reload.
        let prev = dux_core::config::ServerConfig::default();
        let mut next = prev.clone();
        next.max_websocket_events_connections += 1;
        assert!(server_restart_settings_changed(&prev, &next));
    }

    #[test]
    fn restart_drift_detects_max_websocket_agent_connections_change() {
        let prev = dux_core::config::ServerConfig::default();
        let mut next = prev.clone();
        next.max_websocket_agent_connections += 1;
        assert!(server_restart_settings_changed(&prev, &next));
    }

    #[test]
    fn restart_drift_detects_max_websocket_terminal_connections_change() {
        let prev = dux_core::config::ServerConfig::default();
        let mut next = prev.clone();
        next.max_websocket_terminal_connections += 1;
        assert!(server_restart_settings_changed(&prev, &next));
    }

    #[test]
    fn restart_drift_detects_max_websocket_tab_connections_change() {
        let prev = dux_core::config::ServerConfig::default();
        let mut next = prev.clone();
        next.max_websocket_tab_connections += 1;
        assert!(server_restart_settings_changed(&prev, &next));
    }

    #[test]
    fn restart_drift_detects_max_websocket_tabs_per_agent_change() {
        let prev = dux_core::config::ServerConfig::default();
        let mut next = prev.clone();
        next.max_websocket_tabs_per_agent += 1;
        assert!(server_restart_settings_changed(&prev, &next));
    }

    #[test]
    fn restart_drift_detects_tree_list_max_concurrency_change() {
        let prev = dux_core::config::ServerConfig::default();
        let mut next = prev.clone();
        next.tree_list_max_concurrency += 1;
        assert!(server_restart_settings_changed(&prev, &next));
    }

    #[test]
    fn restart_drift_detects_release_notes_max_concurrency_change() {
        let prev = dux_core::config::ServerConfig::default();
        let mut next = prev.clone();
        next.release_notes_max_concurrency += 1;
        assert!(server_restart_settings_changed(&prev, &next));
    }

    #[test]
    fn restart_drift_detects_color_change() {
        // The console is built once, before the engine moves into the actor
        // thread, so a reloaded `color` reaches nothing until a restart.
        let prev = dux_core::config::ServerConfig::default();
        let mut next = prev.clone();
        next.color = "never".to_string();
        assert!(server_restart_settings_changed(&prev, &next));
    }

    #[test]
    fn the_restart_warning_names_dux_server_for_a_console_only_change() {
        let prev = dux_core::config::ServerConfig::default();
        let mut next = prev.clone();
        next.color = "never".to_string();

        let copy = server_restart_warning_copy(&prev, &next, false).expect("a warning");
        assert!(
            copy.contains("dux server"),
            "the console is built by that process alone: {copy}"
        );
        assert!(
            !copy.contains("listener binds"),
            "nothing bound moved, so the bind sentence must stay away: {copy}"
        );
    }

    #[test]
    fn the_restart_warning_carries_both_sentences_when_both_sets_moved() {
        let prev = dux_core::config::ServerConfig::default();
        let mut next = prev.clone();
        next.color = "never".to_string();
        next.port += 1;

        let copy = server_restart_warning_copy(&prev, &next, false).expect("a warning");
        assert!(copy.contains("listener binds"), "{copy}");
        assert!(copy.contains("dux server"), "{copy}");
    }

    #[test]
    fn the_restart_warning_adds_the_background_remedy_only_for_a_bind_change() {
        let prev = dux_core::config::ServerConfig::default();
        let mut bind = prev.clone();
        bind.port += 1;
        let mut console = prev.clone();
        console.color = "never".to_string();

        let bind_copy = server_restart_warning_copy(&prev, &bind, true).expect("a warning");
        assert!(bind_copy.contains("stopping and starting"), "{bind_copy}");
        let console_copy = server_restart_warning_copy(&prev, &console, true).expect("a warning");
        assert!(
            !console_copy.contains("stopping and starting"),
            "the background server has no console to rebuild: {console_copy}"
        );
    }

    #[test]
    fn the_restart_warning_is_absent_when_nothing_startup_bound_moved() {
        let prev = dux_core::config::ServerConfig::default();
        let mut next = prev.clone();
        next.access_log = !prev.access_log;
        assert!(server_restart_warning_copy(&prev, &next, true).is_none());
    }

    #[test]
    fn restart_drift_ignores_the_two_settings_a_reload_applies() {
        // `access_log` and `search_index_max_files` are read live off the shared
        // limits, so warning about them would tell the user to restart for a
        // change that has already taken effect.
        let prev = dux_core::config::ServerConfig::default();
        let mut next = prev.clone();
        next.access_log = !prev.access_log;
        next.search_index_max_files = prev.search_index_max_files + 1;
        assert!(!server_restart_settings_changed(&prev, &next));
    }

    // -----------------------------------------------------------------------
    // Change-gated spine check + self-healing backstop
    //
    // These drive the gating logic (`SpineCheck`, `poll_streaming_transitions`)
    // directly rather than through the async actor thread, so they are
    // deterministic, allocation-free of real sleeps where it matters, and
    // immune to the parallel-test races a shared global call counter would have
    // suffered. `SpineCheck::fp_call_count` (a cfg(test) field) is the seam: it
    // counts how many times the gate actually ran the fingerprint serialize (the
    // serialize), so "the serialize was skipped" is a positive assertion, not an
    // inference from "no event fired".
    // -----------------------------------------------------------------------

    fn seed_session(paths: &DuxPaths, id: &str) {
        let store = dux_core::storage::SessionStore::open(&paths.sessions_db_path).unwrap();
        store
            .create_session(&sample_session(
                id,
                "p1",
                "feat",
                paths.root.to_string_lossy().as_ref(),
            ))
            .unwrap();
    }

    // -----------------------------------------------------------------------
    // Terminals are one flat, owner-tagged collection; these pin that a
    // terminal change moves the coarse fingerprint of its OWNER's half. Without
    // that, a terminal's label, foreground command, working flag or drag order
    // could change with no coarse event firing at all, and no client would ever
    // refetch.
    // -----------------------------------------------------------------------

    fn empty_spine() -> dux_core::viewmodel::SpineView {
        dux_core::viewmodel::SpineView {
            projects: Vec::new(),
            sessions: Vec::new(),
            terminals: Vec::new(),
            sidebar: dux_core::sidebar::build_sidebar(
                &[],
                &[],
                &std::collections::HashSet::new(),
                0,
            ),
        }
    }

    fn sample_terminal_view(
        id: &str,
        owner: dux_core::viewmodel::TerminalOwnerView,
    ) -> dux_core::viewmodel::TerminalView {
        dux_core::viewmodel::TerminalView {
            id: id.to_string(),
            owner,
            input_owner: None,
            label: "Terminal 1".to_string(),
            has_output: false,
            working: false,
            typing: false,
            foreground_cmd: None,
            sort_order: 1,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn a_session_terminal_moves_only_the_sessions_fingerprint() {
        let before = empty_spine();
        let mut after = empty_spine();
        after.terminals.push(sample_terminal_view(
            "term-1",
            dux_core::viewmodel::TerminalOwnerView::Session {
                session_id: "s1".to_string(),
            },
        ));

        let (projects_before, sessions_before) = fingerprint_halves(&before);
        let (projects_after, sessions_after) = fingerprint_halves(&after);
        assert_ne!(
            sessions_before, sessions_after,
            "a session-owned terminal must still fire sessions.changed"
        );
        assert_eq!(
            projects_before, projects_after,
            "and must not spuriously fire projects.changed"
        );
    }

    #[test]
    fn a_project_terminal_moves_only_the_projects_fingerprint() {
        let before = empty_spine();
        let mut after = empty_spine();
        after.terminals.push(sample_terminal_view(
            "term-1",
            dux_core::viewmodel::TerminalOwnerView::Project {
                project_id: "p1".to_string(),
            },
        ));

        let (projects_before, sessions_before) = fingerprint_halves(&before);
        let (projects_after, sessions_after) = fingerprint_halves(&after);
        assert_ne!(
            projects_before, projects_after,
            "a project terminal must still fire projects.changed"
        );
        assert_eq!(
            sessions_before, sessions_after,
            "and must not spuriously fire sessions.changed"
        );
    }

    #[test]
    fn a_standalone_terminal_fires_a_coarse_change_rather_than_none() {
        // The silent half of a new owner kind: it rides in NEITHER of the two
        // nested containers, so leaving it out of both halves would mean a
        // standalone terminal appearing, changing its command, or being dragged
        // fires no event at all and no browser ever refetches. What it fires
        // matters less than THAT it fires; it goes with the sessions half.
        let before = empty_spine();
        let mut after = empty_spine();
        after.terminals.push(sample_terminal_view(
            "term-1",
            dux_core::viewmodel::TerminalOwnerView::Standalone {
                cwd_label: "~/code".to_string(),
            },
        ));

        let (projects_before, sessions_before) = fingerprint_halves(&before);
        let (projects_after, sessions_after) = fingerprint_halves(&after);
        assert_ne!(
            sessions_before, sessions_after,
            "a standalone terminal must fire a coarse change, not silence"
        );
        assert_eq!(projects_before, projects_after);

        // And a FIELD change on a live one, which is the case a mere "does it
        // appear" test would pass while the browser showed stale information.
        let mut changed = after.clone();
        changed.terminals[0].foreground_cmd = Some("vim".to_string());
        let (_, sessions_changed) = fingerprint_halves(&changed);
        assert_ne!(sessions_after, sessions_changed);
    }

    #[test]
    fn a_terminal_field_change_still_moves_its_owners_fingerprint() {
        // The subtler half: not just appearing, but CHANGING. A foreground
        // command arriving on a live terminal has to reach the client.
        let mut before = empty_spine();
        before.terminals.push(sample_terminal_view(
            "term-1",
            dux_core::viewmodel::TerminalOwnerView::Session {
                session_id: "s1".to_string(),
            },
        ));
        let mut after = before.clone();
        after.terminals[0].foreground_cmd = Some("vim".to_string());

        let (projects_before, sessions_before) = fingerprint_halves(&before);
        let (projects_after, sessions_after) = fingerprint_halves(&after);
        assert_ne!(sessions_before, sessions_after);
        assert_eq!(projects_before, projects_after);
    }

    #[test]
    fn idle_ticks_do_not_serialize_the_spine() {
        // With no command, worker event, or streaming transition bumping the
        // versions, and before the backstop interval, the gate must NEVER call
        // the fingerprint serialize — proving idle ticks cost zero serialization.
        let (_tmp, paths) = temp_paths();
        let engine = bootstrap_engine(&paths).expect("bootstrap");
        let (tx, _rx) = broadcast::channel::<SpineChange>(64);
        let owners = crate::pty_owners::PtySizeOwners::default();
        let (workspace_tx, _workspace_rx) = watch::channel(None);
        let mut check = SpineCheck::new(&engine, &owners, &workspace_tx);

        // Run every spine-check interval that fits before the backstop would fire.
        let intervals_before_backstop =
            (SPINE_BACKSTOP_TICK_INTERVAL / SPINE_CHECK_TICK_INTERVAL as u32) - 1;
        for _ in 0..intervals_before_backstop {
            check.maybe_check(&engine, 0, 0, &owners, &tx, &workspace_tx);
        }
        assert_eq!(
            check.fp_call_count, 0,
            "idle ticks must not serialize the spine"
        );
    }

    #[test]
    fn backstop_emits_a_change_that_bypassed_the_version() {
        // A spine mutation that did NOT bump the version (the seam for any future
        // loop mutator added without a bump) must still be detected and emitted
        // once the slow self-healing backstop fires.
        let (_tmp, paths) = temp_paths();
        seed_session(&paths, "s1");
        let mut engine = bootstrap_engine(&paths).expect("bootstrap");
        let (tx, mut rx) = broadcast::channel::<SpineChange>(64);
        let owners = crate::pty_owners::PtySizeOwners::default();
        let (workspace_tx, _workspace_rx) = watch::channel(None);
        let mut check = SpineCheck::new(&engine, &owners, &workspace_tx);

        // Mutate the sessions spine WITHOUT touching the version counters.
        for s in engine.sessions.iter_mut() {
            if s.id == "s1" {
                s.title = Some("renamed-out-of-band".to_string());
            }
        }

        // Drive exactly up to the backstop interval. The version never changed,
        // so the only thing that can run the compare is the backstop.
        let intervals_to_backstop = SPINE_BACKSTOP_TICK_INTERVAL / SPINE_CHECK_TICK_INTERVAL as u32;
        for _ in 0..intervals_to_backstop {
            check.maybe_check(&engine, 0, 0, &owners, &tx, &workspace_tx);
        }
        assert!(
            check.fp_call_count >= 1,
            "the backstop must run the fingerprint compare even with no version bump"
        );

        let mut saw_sessions = false;
        while let Ok(c) = rx.try_recv() {
            if c == SpineChange::Sessions {
                saw_sessions = true;
            }
        }
        assert!(
            saw_sessions,
            "the backstop must emit the change that bypassed the version"
        );
    }

    /// The published-input-ownership chain, end to end at the loop level: a
    /// claim on an agent PTY moves the sessions fingerprint WITHOUT any engine
    /// version bump (ownership lives outside the engine, so the generation is
    /// the only signal), fires `sessions.changed`, and stamps the owning
    /// connection id into the served `/spine` JSON; the owner's disconnect
    /// release clears the field the same way. This is what lets a client that
    /// never attached to the PTY disable its agent-menu mutations while
    /// another device drives the agent.
    #[test]
    fn an_ownership_flip_publishes_the_owner_and_fires_sessions_changed() {
        let (_tmp, paths) = temp_paths();
        seed_session(&paths, "s1");
        let engine = bootstrap_engine(&paths).expect("bootstrap");
        let owners = crate::pty_owners::PtySizeOwners::default();
        let (tx, mut rx) = broadcast::channel::<SpineChange>(64);
        let (workspace_tx, _workspace_rx) = watch::channel(None);
        let mut check = SpineCheck::new(&engine, &owners, &workspace_tx);
        assert!(
            !check.doc.json.contains("input_owner"),
            "an unowned PTY must publish no owner at all (absent, not null)"
        );

        // Take-over: a connection claims the session-slot tab's PTY.
        owners.claim(
            engine
                .slot_tab_id_of(dux_core::ids::SessionIdRef::new("s1"))
                .as_str(),
            42,
        );
        check.maybe_check(&engine, 0, 0, &owners, &tx, &workspace_tx);
        assert_eq!(
            rx.try_recv(),
            Ok(SpineChange::Sessions),
            "an ownership claim must fire sessions.changed with no engine version bump"
        );
        assert!(
            check.doc.json.contains(r#""input_owner":"42""#),
            "the served spine must carry the owning connection id: {}",
            check.doc.json
        );

        // Steady state: the owner typing away bumps nothing, so further checks
        // must not churn (no event, no re-serialize).
        let serializes_after_claim = check.fp_call_count;
        assert!(
            owners
                .may_write(
                    engine
                        .slot_tab_id_of(dux_core::ids::SessionIdRef::new("s1"))
                        .as_str(),
                    42,
                    None
                )
                .allowed
        );
        check.maybe_check(&engine, 0, 0, &owners, &tx, &workspace_tx);
        assert_eq!(
            check.fp_call_count, serializes_after_claim,
            "unchanged ownership must not even re-run the fingerprint compare"
        );
        assert!(rx.try_recv().is_err(), "and must emit no event");

        // The owner disconnects: the release must clear the published field so
        // a crashed device never leaves the agent permanently owned.
        owners.release(
            engine
                .slot_tab_id_of(dux_core::ids::SessionIdRef::new("s1"))
                .as_str(),
            42,
        );
        check.maybe_check(&engine, 0, 0, &owners, &tx, &workspace_tx);
        assert_eq!(
            rx.try_recv(),
            Ok(SpineChange::Sessions),
            "a disconnect release must fire sessions.changed"
        );
        assert!(
            !check.doc.json.contains("input_owner"),
            "and the served spine must drop the owner field"
        );
    }

    /// The revision moves when the document does, and only then. A client
    /// discards a pushed document whose revision it has already applied, so a
    /// revision that failed to move on a real change would strand every tab on
    /// stale data, and one that moved without a change would make the dedup
    /// worthless.
    #[test]
    fn the_workspace_revision_moves_once_per_real_change() {
        let (_tmp, paths) = temp_paths();
        seed_session(&paths, "s1");
        let engine = bootstrap_engine(&paths).expect("bootstrap");
        let owners = crate::pty_owners::PtySizeOwners::default();
        let (tx, _rx) = broadcast::channel::<SpineChange>(64);
        let (workspace_tx, mut workspace_rx) = watch::channel(None);
        let mut check = SpineCheck::new(&engine, &owners, &workspace_tx);

        let seed = workspace_rx
            .borrow_and_update()
            .clone()
            .expect("the seed document must be published at construction");
        assert_eq!(seed.rev, 1, "revisions start at 1, leaving 0 for `unset`");
        assert!(
            seed.json.contains(r#""rev":1"#),
            "the revision must be embedded in the serialization itself: {}",
            seed.json
        );

        // A real change: an input-ownership claim moves the sessions half.
        owners.claim(
            engine
                .slot_tab_id_of(dux_core::ids::SessionIdRef::new("s1"))
                .as_str(),
            42,
        );
        check.maybe_check(&engine, 0, 0, &owners, &tx, &workspace_tx);
        assert!(
            workspace_rx.has_changed().unwrap(),
            "a changed document must be published"
        );
        let next = workspace_rx.borrow_and_update().clone().unwrap();
        assert_eq!(next.rev, seed.rev + 1, "one change, one revision");
        assert!(
            next.json.contains(r#""rev":2"#),
            "and the body agrees with the frame: {}",
            next.json
        );

        // Nothing changed since: the gate does not even re-run, so there is
        // nothing to publish and the revision must stand still.
        check.maybe_check(&engine, 0, 0, &owners, &tx, &workspace_tx);
        assert!(
            !workspace_rx.has_changed().unwrap(),
            "an unchanged document must not be republished"
        );
        assert_eq!(check.doc.rev, next.rev);
    }

    /// The self-healing backstop runs the compare on a schedule, whether or not
    /// anything changed. When it finds nothing, it must publish nothing: every
    /// connected client would otherwise be handed the whole document twice a
    /// minute for no reason, which is the traffic the push exists to remove.
    #[test]
    fn an_unchanged_backstop_pass_publishes_nothing() {
        let (_tmp, paths) = temp_paths();
        seed_session(&paths, "s1");
        let engine = bootstrap_engine(&paths).expect("bootstrap");
        let owners = crate::pty_owners::PtySizeOwners::default();
        let (tx, _rx) = broadcast::channel::<SpineChange>(64);
        let (workspace_tx, mut workspace_rx) = watch::channel(None);
        let mut check = SpineCheck::new(&engine, &owners, &workspace_tx);
        let seed_rev = workspace_rx.borrow_and_update().clone().unwrap().rev;

        let intervals_to_backstop = SPINE_BACKSTOP_TICK_INTERVAL / SPINE_CHECK_TICK_INTERVAL as u32;
        for _ in 0..intervals_to_backstop {
            check.maybe_check(&engine, 0, 0, &owners, &tx, &workspace_tx);
        }
        assert!(
            check.fp_call_count >= 1,
            "the backstop must have run the compare, or this proves nothing"
        );
        assert!(
            !workspace_rx.has_changed().unwrap(),
            "a backstop pass that found no change must publish nothing"
        );
        assert_eq!(check.doc.rev, seed_rev, "and must not move the revision");
    }

    /// Ownership of a pty id that is not an agent tab (a companion terminal's,
    /// or a stale id) is deliberately NOT published: the overlay only stamps
    /// agent tabs. The generation still opens the gate, but the fingerprints
    /// come out identical, so no coarse event fires and the cache stays put.
    #[test]
    fn a_non_tab_claim_publishes_nothing_and_fires_no_event() {
        let (_tmp, paths) = temp_paths();
        seed_session(&paths, "s1");
        let engine = bootstrap_engine(&paths).expect("bootstrap");
        let owners = crate::pty_owners::PtySizeOwners::default();
        let (tx, mut rx) = broadcast::channel::<SpineChange>(64);
        let (workspace_tx, _workspace_rx) = watch::channel(None);
        let mut check = SpineCheck::new(&engine, &owners, &workspace_tx);

        owners.claim("term-9", 7);
        check.maybe_check(&engine, 0, 0, &owners, &tx, &workspace_tx);
        assert!(
            rx.try_recv().is_err(),
            "a claim on a non-tab pty must not fire any coarse event"
        );
        assert!(!check.doc.json.contains("input_owner"));
    }

    /// The overlay must iterate EVERY session's tabs, not merely find a first
    /// match: with two agents, claiming the second one's PTY must stamp that
    /// tab and only that tab. A regression narrowing the loop to the first
    /// session would pass the single-session tests above while letting a
    /// second agent driven elsewhere read as unowned.
    #[test]
    fn the_overlay_stamps_the_right_tab_across_multiple_sessions() {
        let (_tmp, paths) = temp_paths();
        seed_session(&paths, "s1");
        seed_session(&paths, "s2");
        let engine = bootstrap_engine(&paths).expect("bootstrap");
        let owners = crate::pty_owners::PtySizeOwners::default();
        let s2_slot = engine
            .slot_tab_id_of(dux_core::ids::SessionIdRef::new("s2"))
            .to_string();
        owners.claim(&s2_slot, 9);

        let spine = owned_spine(&engine, &owners);
        let owner_of = |sid: &str| {
            spine
                .sessions
                .iter()
                .find(|s| s.id == sid)
                .expect("session present")
                .tabs
                .iter()
                .find(|t| {
                    t.id == engine
                        .slot_tab_id_of(dux_core::ids::SessionIdRef::new(sid))
                        .as_str()
                })
                .expect("session-slot tab present")
                .input_owner
                .clone()
        };
        assert_eq!(
            owner_of("s2"),
            Some("9".to_string()),
            "the claimed session's slot tab must carry the owning connection id"
        );
        assert_eq!(
            owner_of("s1"),
            None,
            "and the unclaimed session must not be stamped"
        );
    }

    #[test]
    fn streaming_transition_triggers_a_check() {
        // The time-derived `working` flag cannot be observed by a mutation
        // counter, so a dedicated O(1) streaming counter tracks each agent's
        // `is_agent_streaming()` value and bumps on every transition. Back-date
        // pty_activity past AGENT_STREAMING_WINDOW (mirroring the engine's
        // hysteresis tests) to flip it deterministically with no real sleep.
        use dux_core::engine::AGENT_STREAMING_WINDOW;

        let (_tmp, paths) = temp_paths();
        seed_session(&paths, "s1");
        let mut engine = bootstrap_engine(&paths).expect("bootstrap");

        let mut prev_streaming: std::collections::HashMap<String, bool> =
            std::collections::HashMap::new();
        let mut streaming_version = 0u64;

        // Fresh activity → streaming. First observation is a transition.
        engine
            .pty_activity
            .insert("s1".to_string(), std::time::Instant::now());
        poll_streaming_transitions(&engine, &mut prev_streaming, &mut streaming_version);
        let after_first = streaming_version;
        assert_eq!(
            after_first, 1,
            "first streaming observation bumps the counter"
        );

        // Still streaming → no transition, no bump.
        poll_streaming_transitions(&engine, &mut prev_streaming, &mut streaming_version);
        assert_eq!(
            streaming_version, after_first,
            "a steady streaming agent must not bump the counter every tick"
        );

        // Back-date past the window → streaming flips to idle: a transition.
        engine.pty_activity.insert(
            "s1".to_string(),
            std::time::Instant::now()
                - (AGENT_STREAMING_WINDOW + std::time::Duration::from_millis(50)),
        );
        poll_streaming_transitions(&engine, &mut prev_streaming, &mut streaming_version);
        assert_eq!(
            streaming_version,
            after_first + 1,
            "a streaming->idle transition must bump the counter"
        );

        // And the gate opens: the changed streaming_version makes the next
        // interval run the fingerprint compare.
        let (tx, _rx) = broadcast::channel::<SpineChange>(64);
        let owners = crate::pty_owners::PtySizeOwners::default();
        let (workspace_tx, _workspace_rx) = watch::channel(None);
        let mut check = SpineCheck::new(&engine, &owners, &workspace_tx);
        check.maybe_check(&engine, 0, streaming_version, &owners, &tx, &workspace_tx);
        assert_eq!(
            check.fp_call_count, 1,
            "a streaming_version change must open the gate"
        );
    }

    #[test]
    fn attention_transition_bumps_version() {
        // Setting and clearing the needs-attention flag must bump the version so
        // the spine change is pushed promptly; a steady state must not.
        let (_tmp, paths) = temp_paths();
        seed_session(&paths, "s1");
        let engine_seed = bootstrap_engine(&paths).expect("bootstrap");
        // We mutate the set directly to drive transitions deterministically.
        let mut engine = engine_seed;

        let mut prev: std::collections::HashSet<TabId> = std::collections::HashSet::new();
        let mut version = 0u64;

        // No change yet.
        poll_attention_transitions(&engine, &mut prev, &mut version);
        assert_eq!(version, 0, "an empty, unchanged set must not bump");

        // Set → transition.
        engine
            .needs_attention
            .insert(engine.sessions[0].slot_tab_id().to_owned());
        poll_attention_transitions(&engine, &mut prev, &mut version);
        assert_eq!(version, 1, "raising the flag must bump the version");

        // Steady → no bump.
        poll_attention_transitions(&engine, &mut prev, &mut version);
        assert_eq!(version, 1, "a steady flag must not bump every tick");

        // Clear → transition.
        engine.needs_attention.clear();
        poll_attention_transitions(&engine, &mut prev, &mut version);
        assert_eq!(version, 2, "clearing the flag must bump the version");
    }

    #[tokio::test]
    async fn note_viewed_request_clears_attention_for_a_known_tab() {
        // The web "user is looking at it" ping must route through the actor to the
        // core `note_agent_viewed_if_known`, clearing a flagged tab's attention.
        let (_tmp, paths) = temp_paths();
        seed_session(&paths, "s1");
        let mut engine = bootstrap_engine(&paths).expect("bootstrap");
        // The agent is flagged for attention before anyone opens it.
        let slot = engine.sessions[0].slot_tab_id().to_string();
        engine.needs_attention.insert(TabId::new(slot.clone()));
        let (handle, _join) = spawn_engine_thread(engine);

        // It starts flagged in the spine projection.
        let spine = handle.spine().await.expect("spine");
        assert_eq!(
            spine
                .sessions
                .iter()
                .find(|s| s.id == "s1")
                .map(|s| s.needs_attention),
            Some(true),
            "the seeded flag must show in the spine"
        );

        // A viewed ping for the real tab clears it.
        handle.note_viewed(slot);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let spine = handle.spine().await.expect("spine");
            let flagged = spine
                .sessions
                .iter()
                .find(|s| s.id == "s1")
                .map(|s| s.needs_attention);
            if flagged == Some(false) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "note_viewed did not clear the attention flag in time"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    /// The SAME published-ownership chain, for a companion terminal.
    ///
    /// Terminal PTY ids are terminal ids and the spine carries terminals in one
    /// flat collection, so the overlay is one more loop over the same snapshot.
    /// The browser consumes it so a terminal pane's take-over card can name the
    /// device driving it rather than guessing from a stale value. The accepted
    /// cost, asserted here: an ownership flip on a terminal now moves the spine
    /// fingerprint and fires `sessions.changed`, exactly as it already did for
    /// an agent tab.
    #[test]
    fn a_terminal_ownership_flip_publishes_the_owner_and_fires_sessions_changed() {
        let (_tmp, paths) = temp_paths();
        seed_session(&paths, "s1");
        let mut engine = bootstrap_engine(&paths).expect("bootstrap");
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];
        let (terminal_id, _) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("create companion terminal");

        let owners = crate::pty_owners::PtySizeOwners::default();
        let (tx, mut rx) = broadcast::channel::<SpineChange>(64);
        let (workspace_tx, _workspace_rx) = watch::channel(None);
        let mut check = SpineCheck::new(&engine, &owners, &workspace_tx);
        assert!(
            !check.doc.json.contains("input_owner"),
            "an undriven terminal must publish no owner at all"
        );

        owners.claim(&terminal_id, 42);
        check.maybe_check(&engine, 0, 0, &owners, &tx, &workspace_tx);
        assert_eq!(
            rx.try_recv(),
            Ok(SpineChange::Sessions),
            "a terminal ownership claim must fire sessions.changed, the accepted \
             cost of publishing the field"
        );
        assert!(
            check.doc.json.contains(r#""input_owner":"42""#),
            "the served spine must carry the terminal's owning connection id: {}",
            check.doc.json
        );

        owners.release(&terminal_id, 42);
        check.maybe_check(&engine, 0, 0, &owners, &tx, &workspace_tx);
        assert!(
            !check.doc.json.contains("input_owner"),
            "a disconnect must clear the terminal's published owner"
        );
    }

    #[test]
    fn prune_exit_triggers_a_check_within_one_interval() {
        // A quiet agent/terminal exit flows through prune_exited_ptys, which
        // returns the pruned entry -> the loop bumps the mutation version -> the
        // very next spine-check interval emits the change, far before the 2s
        // backstop.
        let (_tmp, paths) = temp_paths();
        seed_session(&paths, "s1");
        let mut engine = bootstrap_engine(&paths).expect("bootstrap");
        // A companion terminal backed by a command that exits immediately, so
        // prune_exited_ptys reaps it deterministically.
        engine.config.terminal.command = "true".to_string();
        engine.config.terminal.args = vec![];
        engine
            .create_companion_terminal("s1", 24, 80)
            .expect("create companion terminal");

        let (tx, mut rx) = broadcast::channel::<SpineChange>(64);
        // Seed the fingerprint WHILE the terminal is present so its removal is a
        // real diff.
        let owners = crate::pty_owners::PtySizeOwners::default();
        let (workspace_tx, _workspace_rx) = watch::channel(None);
        let mut check = SpineCheck::new(&engine, &owners, &workspace_tx);

        // Wait for the child to exit, then prune (the loop's #4 mutator).
        let mut mutation_version = 0u64;
        let mut bumped = false;
        for _ in 0..300 {
            if !engine.prune_exited_ptys().is_empty() {
                mutation_version += 1;
                bumped = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(bumped, "the quiet terminal must exit and be pruned");

        // A SINGLE spine-check interval later (one maybe_check), the bump opens
        // the gate. The backstop needs many more intervals, so this proves the
        // bump path, not the backstop.
        check.maybe_check(&engine, mutation_version, 0, &owners, &tx, &workspace_tx);
        assert_eq!(
            check.fp_call_count, 1,
            "the prune bump must open the gate on the next interval"
        );

        let mut saw_sessions = false;
        while let Ok(c) = rx.try_recv() {
            if c == SpineChange::Sessions {
                saw_sessions = true;
            }
        }
        assert!(
            saw_sessions,
            "pruning the exited terminal must emit SpineChange::Sessions"
        );
    }

    // ── Bug 1: "Save" persists but does not apply ────────────────────────────

    /// Write a config.toml that sets only `defaults.start_directory`, leaving
    /// everything else at defaults so a later raw write that also defaults the
    /// `[server]` section is accepted (the raw-save guard rejects host changes).
    fn write_start_dir_config(paths: &DuxPaths, dir: &std::path::Path) {
        std::fs::write(
            &paths.config_path,
            format!(
                "[defaults]\nstart_directory = \"{}\"\n",
                dir.to_string_lossy()
            ),
        )
        .unwrap();
    }

    /// Poll `browse_start_dir` until it equals `want` or the deadline passes.
    /// Reload adoption happens asynchronously (a barrier + worker), so the flip is
    /// observed by polling rather than assumed immediate.
    async fn await_start_dir(handle: &EngineHandle, want: &str) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if handle.browse_start_dir().await.as_deref() == Some(want) {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }

    #[tokio::test]
    async fn raw_save_persists_to_disk_but_does_not_apply_until_reload() {
        let (_tmp, paths) = temp_paths();
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        write_start_dir_config(&paths, dir_a.path());

        let engine = bootstrap_engine(&paths).expect("bootstrap");
        let (handle, _join) = spawn_engine_thread(engine);

        // Baseline: the live config resolves to dir A.
        assert_eq!(
            handle.browse_start_dir().await.as_deref(),
            Some(dir_a.path().to_string_lossy().as_ref())
        );

        // Save a new config pointing at dir B.
        let new_body = format!(
            "[defaults]\nstart_directory = \"{}\"\n",
            dir_b.path().to_string_lossy()
        );
        handle
            .write_raw_config(new_body.clone())
            .await
            .expect("write");

        // PERSISTED: the file on disk now carries dir B.
        let on_disk = handle.read_raw_config().await.expect("read");
        assert!(
            on_disk.contains(dir_b.path().to_string_lossy().as_ref()),
            "disk must hold the saved edit"
        );

        // NOT APPLIED: the running config still resolves to dir A — saving did not
        // adopt. Give any erroneous async adopt a moment to (not) happen.
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert_eq!(
            handle.browse_start_dir().await.as_deref(),
            Some(dir_a.path().to_string_lossy().as_ref()),
            "save must not apply the config"
        );

        // Explicit reload is the single apply point: now it flips to dir B.
        handle
            .apply_wire(WireCommand::ReloadConfig {})
            .await
            .expect("reload");
        assert!(
            await_start_dir(&handle, dir_b.path().to_string_lossy().as_ref()).await,
            "reload must apply the saved config"
        );
    }

    #[tokio::test]
    async fn config_static_mutation_after_a_raw_save_does_not_clobber_it() {
        let (_tmp, paths) = temp_paths();
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        write_start_dir_config(&paths, dir_a.path());

        let engine = bootstrap_engine(&paths).expect("bootstrap");
        let (handle, _join) = spawn_engine_thread(engine);

        // Save dir B (persist-only; memory still on dir A).
        let new_body = format!(
            "[defaults]\nstart_directory = \"{}\"\n",
            dir_b.path().to_string_lossy()
        );
        handle.write_raw_config(new_body).await.expect("write");

        // Now toggle a config-static setting. Its wholesale toml_edit patch would
        // serialize the (stale) in-memory config over the file — reverting the
        // saved dir B back to dir A — UNLESS the handler reconciles with disk first.
        handle
            .apply_wire(WireCommand::SetChangesPaneVisible { visible: false })
            .await
            .expect("toggle");

        // The saved edit survived on disk: the toggle reconciled instead of clobbering.
        let on_disk = handle.read_raw_config().await.expect("read");
        assert!(
            on_disk.contains(dir_b.path().to_string_lossy().as_ref()),
            "the config-static mutation must not clobber the saved start_directory"
        );
        assert!(
            !on_disk.contains(dir_a.path().to_string_lossy().as_ref()),
            "the stale dir A must not have been written back"
        );

        // And the reconcile adopted dir B, so the live config now resolves to it.
        assert!(
            await_start_dir(&handle, dir_b.path().to_string_lossy().as_ref()).await,
            "reconcile must adopt the saved config"
        );
    }

    // -----------------------------------------------------------------------
    // WritePty input classification
    // -----------------------------------------------------------------------

    /// Drive one `EngineRequest` straight through `handle_request`, the way the
    /// actor loop does, with throwaway channels. Nothing here reads the status
    /// or reload plumbing; it exists so the request handler can be exercised
    /// without spawning the actor thread.
    fn run_request(engine: &mut Engine, req: EngineRequest) {
        let (tx, _rx) = broadcast::channel(8);
        let (clear_tx, _clear_rx) = broadcast::channel(8);
        let (snapshot_tx, _snapshot_rx) = watch::channel(Vec::new());
        let mut status = StatusEmitter::new(tx, clear_tx, snapshot_tx, Default::default());
        let (config_reload_tx, _config_rx) = broadcast::channel(8);
        let mut disk_ahead = false;
        let owners = crate::pty_owners::PtySizeOwners::default();
        handle_request(
            engine,
            req,
            &mut status,
            &config_reload_tx,
            &mut disk_ahead,
            &owners,
        );
    }

    /// A `cat`-backed companion terminal on a bootstrapped engine, so a
    /// `WritePty` actually reaches a live PTY.
    fn engine_with_terminal() -> (tempfile::TempDir, Engine, String) {
        let (tmp, paths) = temp_paths();
        {
            let store = dux_core::storage::SessionStore::open(&paths.sessions_db_path).unwrap();
            store
                .create_session(&sample_session(
                    "s1",
                    "p1",
                    "feat",
                    paths.root.to_string_lossy().as_ref(),
                ))
                .unwrap();
        }
        let mut engine = bootstrap_engine(&paths).expect("bootstrap");
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];
        let (id, _) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("create companion terminal");
        (tmp, engine, id)
    }

    /// A forwarded wheel notch must stamp the pointer window: unstamped, the
    /// repaint the child answers with reads as the agent working, and an idle
    /// agent shows Working for as long as you scroll.
    #[test]
    fn a_forwarded_wheel_stamps_the_pointer_window() {
        let (_tmp, mut engine, id) = engine_with_terminal();

        run_request(
            &mut engine,
            EngineRequest::WritePty(id.clone(), b"\x1b[<64;10;5M".to_vec()),
        );

        assert!(
            engine.recent_pointer_input(&id),
            "a forwarded wheel must stamp the pointer window so the repaint it \
             causes is not read as working"
        );
        assert!(!engine.is_typing(&id), "and it must never read as typing");
    }

    #[test]
    fn a_real_keystroke_still_stamps_only_the_typing_window() {
        let (_tmp, mut engine, id) = engine_with_terminal();

        run_request(
            &mut engine,
            EngineRequest::WritePty(id.clone(), b"x".to_vec()),
        );

        assert!(engine.is_typing(&id), "a keystroke still lights Typing");
        assert!(
            !engine.recent_pointer_input(&id),
            "and it must not stamp the pointer window"
        );
    }

    #[test]
    fn a_focus_report_stamps_neither_window() {
        let (_tmp, mut engine, id) = engine_with_terminal();

        run_request(
            &mut engine,
            EngineRequest::WritePty(id.clone(), b"\x1b[I".to_vec()),
        );

        assert!(!engine.is_typing(&id));
        assert!(
            !engine.recent_pointer_input(&id),
            "merely focusing a terminal is not pointer input either"
        );
    }

    #[test]
    fn a_write_to_an_unknown_id_stamps_nothing() {
        let (_tmp, mut engine, _id) = engine_with_terminal();

        run_request(
            &mut engine,
            EngineRequest::WritePty("nobody".to_string(), b"\x1b[<64;10;5M".to_vec()),
        );

        assert!(
            !engine.recent_pointer_input("nobody"),
            "a write that reached no PTY must not accumulate a stamp"
        );
    }

    // -----------------------------------------------------------------------
    // Spine-mutation gate (`request_mutates_spine`)
    // -----------------------------------------------------------------------

    /// A reply channel whose receiver is dropped immediately. The gate only ever
    /// inspects the discriminant, so nothing is ever sent on these.
    fn dead_reply<T>() -> oneshot::Sender<T> {
        oneshot::channel().0
    }

    /// Every `EngineRequest` kind paired with the answer `request_mutates_spine`
    /// owes it. Building the values needs only channels, so this stays a pure
    /// unit test: no engine, no PTY, no I/O.
    ///
    /// This list is NOT what makes the gate exhaustive (the wildcard-free `match`
    /// in `request_mutates_spine` is, and adding a variant breaks the build until
    /// somebody answers for it). What it pins is the ANSWERS: an arm that
    /// silently answers `false` for a mutator below fails here.
    fn request_kind_answers() -> Vec<(&'static str, EngineRequest, bool)> {
        vec![
            (
                "ApplyWire",
                EngineRequest::ApplyWire(
                    WireCommand::SetChangesPaneVisible { visible: true },
                    dead_reply(),
                    StatusScope::All,
                ),
                true,
            ),
            (
                "EmitStatus",
                EngineRequest::EmitStatus(WireStatus::new("info", "hello")),
                false,
            ),
            (
                "ClearStatus",
                EngineRequest::ClearStatus("op-example".to_string()),
                false,
            ),
            (
                "SubscribePty",
                EngineRequest::SubscribePty("s1".into(), dead_reply()),
                true,
            ),
            (
                "WritePty",
                EngineRequest::WritePty("s1".into(), b"hello".to_vec()),
                true,
            ),
            (
                "ResizePty",
                EngineRequest::ResizePty("s1".into(), 24, 80, 1),
                false,
            ),
            (
                "PtyGridSize",
                EngineRequest::PtyGridSize("s1".into(), dead_reply()),
                false,
            ),
            ("NoteViewed", EngineRequest::NoteViewed("s1".into()), true),
            (
                "SubscribeTerminal",
                EngineRequest::SubscribeTerminal("t1".into(), dead_reply()),
                false,
            ),
            (
                "CreateTerminal",
                EngineRequest::CreateTerminal("s1".into(), dead_reply()),
                true,
            ),
            (
                "CreateProjectTerminal",
                EngineRequest::CreateProjectTerminal("p1".into(), dead_reply()),
                true,
            ),
            (
                "CreateStandaloneTerminal",
                EngineRequest::CreateStandaloneTerminal(dead_reply()),
                true,
            ),
            (
                "TerminalOwnerOf",
                EngineRequest::TerminalOwnerOf("t1".into(), dead_reply()),
                false,
            ),
            (
                "TerminalRoot",
                EngineRequest::TerminalRoot("t1".into(), dead_reply()),
                false,
            ),
            (
                "CreateAgentTab",
                EngineRequest::CreateAgentTab("s1".into(), None, dead_reply()),
                true,
            ),
            (
                "TabSession",
                EngineRequest::TabSession("t1".into(), dead_reply()),
                false,
            ),
            (
                "CreateAgentBranchPlan",
                EngineRequest::CreateAgentBranchPlan("p1".into(), "name".into(), dead_reply()),
                false,
            ),
            (
                "PullRequestResolutionInputs",
                EngineRequest::PullRequestResolutionInputs(dead_reply()),
                false,
            ),
            (
                "SessionWorktree",
                EngineRequest::SessionWorktree("s1".into(), dead_reply()),
                false,
            ),
            (
                "SessionBranchDeleteInputs",
                EngineRequest::SessionBranchDeleteInputs("s1".into(), dead_reply()),
                false,
            ),
            (
                "PtyKeyForPaneId",
                EngineRequest::PtyKeyForPaneId("s1".into(), dead_reply()),
                false,
            ),
            (
                "FileDropDestination",
                EngineRequest::FileDropDestination("s1".into(), dead_reply()),
                false,
            ),
            (
                "FileDropTreeDestination",
                EngineRequest::FileDropTreeDestination("s1".into(), "src".into(), dead_reply()),
                false,
            ),
            (
                "FileDropRefreshTarget",
                EngineRequest::FileDropRefreshTarget("s1".into(), dead_reply()),
                false,
            ),
            (
                "ProjectPath",
                EngineRequest::ProjectPath("p1".into(), dead_reply()),
                false,
            ),
            (
                "ResourceTargets",
                EngineRequest::ResourceTargets(dead_reply()),
                false,
            ),
            ("Bootstrap", EngineRequest::Bootstrap(dead_reply()), false),
            ("Spine", EngineRequest::Spine(dead_reply()), false),
            ("SpineJson", EngineRequest::SpineJson(dead_reply()), false),
            (
                "Session",
                EngineRequest::Session("s1".into(), dead_reply()),
                false,
            ),
            (
                "CreatedSessionForOp",
                EngineRequest::CreatedSessionForOp("op1".into(), dead_reply()),
                false,
            ),
            (
                "NextChangesRev",
                EngineRequest::NextChangesRev("s1".into(), dead_reply()),
                false,
            ),
            (
                "EditorDefault",
                EngineRequest::EditorDefault(dead_reply()),
                false,
            ),
            (
                "BrowseStartDir",
                EngineRequest::BrowseStartDir(dead_reply()),
                false,
            ),
            (
                "RefreshChangedFiles",
                EngineRequest::RefreshChangedFiles("/tmp/wt".into()),
                false,
            ),
            (
                "ProjectWorktreeInputs",
                EngineRequest::ProjectWorktreeInputs("p1".into(), dead_reply()),
                false,
            ),
            (
                "SessionStartupLogContext",
                EngineRequest::SessionStartupLogContext("s1".into(), dead_reply()),
                false,
            ),
            (
                "ProjectStartupLogContext",
                EngineRequest::ProjectStartupLogContext("p1".into(), dead_reply()),
                false,
            ),
            (
                "ReadRawConfig",
                EngineRequest::ReadRawConfig(dead_reply()),
                false,
            ),
            (
                "WriteRawConfig",
                EngineRequest::WriteRawConfig("x = 1".into(), dead_reply()),
                false,
            ),
            (
                "FirstLoadInputs",
                EngineRequest::FirstLoadInputs(dead_reply()),
                false,
            ),
            (
                "MarkVersionSeen",
                EngineRequest::MarkVersionSeen("1.2.3".into(), dead_reply()),
                false,
            ),
            ("Shutdown", EngineRequest::Shutdown(dead_reply()), true),
        ]
    }

    #[test]
    fn spine_gate_answers_every_request_kind_as_documented() {
        for (name, req, expected) in request_kind_answers() {
            assert_eq!(
                request_mutates_spine(&req),
                expected,
                "request_mutates_spine({name}) must answer {expected}"
            );
        }
    }

    /// Run exactly one iteration of the real actor loop and hand the engine back.
    /// `control` is consulted at the TOP of each iteration, so answering
    /// `Continue` once and `Exit` afterwards runs the body exactly once. The
    /// handle is kept alive for the duration so the request channel does not read
    /// as disconnected.
    fn one_loop_iteration(engine: Engine) -> Engine {
        let (handle, ends) = build_actor_channels(&engine);
        let mut ran = false;
        let engine = run_engine_loop(engine, ends, ShutdownEcho::Stderr, move || {
            if ran {
                LoopControl::Exit
            } else {
                ran = true;
                LoopControl::Continue
            }
        });
        drop(handle);
        engine
    }

    /// The changed-files poll cadence (2s busy / 10s idle) is picked from
    /// `Engine::has_active_processes`, and the web surface has to keep that flag
    /// true while any PTY is alive. It never stored to the flag at all, so
    /// `dux server` polled on the 10-second idle cadence however many agents and
    /// terminals were running. Both directions matter: the flag must rise when a
    /// process appears and fall when it exits.
    #[test]
    fn the_loop_keeps_has_active_processes_in_step_with_live_ptys() {
        use std::sync::atomic::Ordering;

        let (_tmp, paths) = temp_paths();
        let mut engine = bootstrap_engine(&paths).expect("engine");
        let flag = Arc::clone(&engine.has_active_processes);

        engine = one_loop_iteration(engine);
        assert!(
            !flag.load(Ordering::Relaxed),
            "with no PTYs alive the workspace is idle"
        );

        // `read` blocks on stdin, so this child stays up until the test feeds it a
        // line. No polling, no load: it simply waits.
        let client = PtyClient::spawn_with_env(
            "sh",
            &["-c".to_string(), "read line".to_string()],
            &paths.root,
            24,
            80,
            100,
            &[],
        )
        .expect("spawn sh");
        engine.providers.insert(TabId::new("s1"), client);
        engine = one_loop_iteration(engine);
        assert!(
            flag.load(Ordering::Relaxed),
            "a live provider PTY must mark the workspace active"
        );

        // Let the child finish. The loop's own exit prune drops it from
        // `providers`, and the flag has to follow it back down.
        engine
            .providers
            .get(TabIdRef::new("s1"))
            .expect("provider")
            .write_bytes(b"\n")
            .expect("write to pty");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            engine = one_loop_iteration(engine);
            if !flag.load(Ordering::Relaxed) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the flag never fell back to false after the last PTY exited"
            );
        }
    }

    #[test]
    fn spine_gate_covers_every_request_kind() {
        // A variant added to `EngineRequest` without a row above would leave this
        // count behind. The exhaustive `match` is what forces an ANSWER; this is
        // what forces the answer to be EXERCISED, so a new kind cannot be waved
        // through with a copied-from-its-neighbour `false` that nothing reads.
        assert_eq!(
            request_kind_answers().len(),
            43,
            "every EngineRequest kind needs a row in request_kind_answers; \
             update the count deliberately when adding one"
        );
    }
}
