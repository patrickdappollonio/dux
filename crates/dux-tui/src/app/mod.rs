use std::collections::{HashMap, HashSet};
use std::io::{Write as _, stdout};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Utc;
use crossterm::event::{
    self, DisableFocusChange, DisableMouseCapture, EnableFocusChange, EnableMouseCapture, Event,
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, ListState, Paragraph, StatefulWidget, Widget, Wrap,
};
use uuid::Uuid;

use crate::clipboard::Clipboard;
#[allow(deprecated)]
// importing the deprecated TUI save_config for use in the blessed sync-direct project-sync helpers
use crate::config::{
    Config, DuxPaths, MacroSurface, ensure_config, provider_config, save_config, validate_keys,
};
use crate::diff::SyntaxCache;
use crate::editor::DetectedEditor;
use crate::git;
use crate::keybindings::{
    Action, BindingScope, HintContext, InteractiveBytePatterns, RuntimeBindings,
};
use crate::lockfile::SingleInstanceLock;
use crate::logger;
use crate::model::{
    AgentSession, AgentTab, ChangedFile, CompanionTerminalStatus, Project, ProjectBranchStatus,
    ProviderKind, SessionStatus, SessionSurface,
};

use crate::pty::PtyClient;
use crate::pty::TerminalSnapshot;
use crate::statusline::{BUSY_TIMEOUT, KeyedStatusController, StatusTone};
use crate::storage::SessionStore;
use crate::theme::Theme;
use dux_core::engine::{Command, Engine};
pub(crate) use dux_core::model::CompanionTerminal;
pub(crate) use dux_core::model::TerminalOwner;

use text_input::TextInput;

pub(crate) use dux_core::worker::{
    AgentLaunchKind, AgentLaunchRequest, BranchWarningKind, BrowserEntry,
    CreateAgentBranchInspection, CreateAgentRequest, NonDefaultBranchAction,
    ProjectPersistenceAction, ProjectWorktreeEntry, PullTarget, ResourceKind, ResourceStats,
    WorkerEvent,
};
#[cfg(test)]
pub(crate) use dux_core::worker::{AgentLaunchReadyData, ProcessInfo};

/// Maximum agent-passthrough bytes written to the host terminal per tick. A larger
/// burst is split, with the remainder carried to the next tick, so one oversized
/// forward can never stall the single-threaded run loop on a blocking `write_all`.
const HOST_FORWARD_MAX_PER_TICK: usize = 32 * 1024;

/// Minimum interval between logged host-forward write failures, so a persistently
/// broken stdout logs at most once per interval instead of every tick.
const HOST_FORWARD_ERROR_LOG_INTERVAL: Duration = Duration::from_secs(5);

pub struct App {
    pub(crate) engine: Engine,
    pub(crate) bindings: RuntimeBindings,
    pub(crate) selected_left: usize,
    pub(crate) left_section: LeftSection,
    pub(crate) selected_terminal_index: usize,
    pub(crate) right_section: RightSection,
    pub(crate) files_index: usize,
    pub(crate) files_search: TextInput,
    pub(crate) files_search_active: bool,
    pub(crate) commit_input: TextInput,
    pub(crate) left_width_pct: u16,
    pub(crate) right_width_pct: u16,
    pub(crate) terminal_pane_height_pct: u16,
    pub(crate) staged_pane_height_pct: u16,
    pub(crate) commit_pane_height_pct: u16,
    pub(crate) focus: FocusPane,
    pub(crate) center_mode: CenterMode,
    pub(crate) left_collapsed: bool,
    pub(crate) right_collapsed: bool,
    pub(crate) right_hidden: bool,
    pub(crate) resize_mode: bool,
    pub(crate) help_scroll: Option<u16>,
    pub(crate) last_help_height: u16,
    pub(crate) last_help_lines: u16,
    pub(crate) fullscreen_overlay: FullscreenOverlay,
    pub(crate) startup_log_viewer: Option<StartupLogViewer>,
    pub(crate) status: KeyedStatusController,
    pub(crate) prompt: PromptState,
    pub(crate) input_target: InputTarget,
    pub(crate) session_surface: SessionSurface,
    pub(crate) clipboard: Clipboard,
    pub(crate) active_terminal_id: Option<String>,
    /// Which tab is focused in the center pane, per session (session_id →
    /// tab_id). Missing or equal-to-session-id means the session-slot tab. Only the
    /// center pane resolves the focused tab; sidebar/session labels stay
    /// Main-scoped. Pruned when a session is torn down.
    pub(crate) focused_tabs: HashMap<String, String>,
    /// Passthrough bytes captured but not yet written to the host terminal because
    /// a single tick's forward exceeded [`HOST_FORWARD_MAX_PER_TICK`]. Carried to
    /// the next tick so a burst is bounded without dropping data. The host stdout
    /// is one continuous byte stream, so splitting a sequence across ticks is safe.
    pub(crate) host_forward_carry: Vec<u8>,
    /// When the last host-forward write error was logged, so a persistently failing
    /// stdout logs at most once per [`HOST_FORWARD_ERROR_LOG_INTERVAL`] rather than
    /// every tick. `None` until the first failure.
    pub(crate) host_forward_error_logged_at: Option<Instant>,
    /// Click hit-boxes for the agent tab strip, rebuilt each frame while the
    /// strip is drawn: (tab_id, cell rect) for each tab, plus the `+` add rect.
    /// Kept out of `MouseLayoutState` (which is `Copy`); reset by the strip
    /// renderer, empty when the strip is hidden (< 2 tabs) or in fullscreen.
    pub(crate) agent_tab_regions: Vec<(String, Rect)>,
    pub(crate) agent_tab_add_region: Option<Rect>,
    pub(crate) terminal_return_to_list: bool,
    pub(crate) last_pty_size: (u16, u16),
    pub(crate) prev_scrollback_offset: usize,
    pub(crate) show_diff_line_numbers: bool,
    pub(crate) last_diff_height: u16,
    pub(crate) last_diff_visual_lines: u16,
    pub(crate) theme: Theme,
    pub(crate) tick_count: u64,
    /// Wall-clock reference for time-based animations (spinners). Using
    /// elapsed time instead of `tick_count` keeps animation speed constant
    /// regardless of how fast the event loop is running.
    pub(crate) start_time: Instant,
    pub(crate) readonly_nudge_tick: Option<u64>,
    pub(crate) collapsed_projects: HashSet<String>,
    pub(crate) left_items_cache: Vec<LeftItem>,
    pub(crate) mouse_layout: MouseLayoutState,
    pub(crate) overlay_layout: OverlayMouseLayoutState,
    pub(crate) mouse_drag: Option<ResizeDragState>,
    pub(crate) last_mouse_click: Option<RecentMouseClick>,
    /// Tracks an in-flight modal-button press: which button received
    /// mouse-down and whether the cursor is still inside it. Set on
    /// `MouseEventKind::Down(Left)` over a button, updated on `Drag`,
    /// cleared on `Up` (firing the button's action only when the cursor
    /// is still inside) and on any keystroke or modal-close event.
    pub(crate) pressed_button: Option<components::PressedButton>,
    pub(crate) interactive_patterns: InteractiveBytePatterns,
    pub(crate) raw_input_parser: crate::raw_input::RawInputParser,
    pub(crate) raw_input_buf: Vec<u8>,
    /// Separate buffer for scanning ExitInteractive during the loading phase.
    /// Kept independent of `raw_input_buf` so that suppressed keystrokes
    /// cannot leak into the first post-loading `process_raw_input_bytes` call.
    pub(crate) loading_input_buf: Vec<u8>,
    /// True while processing bytes between bracket-paste markers
    /// (`ESC[200~` … `ESC[201~`). Inside a paste, intercept matching is
    /// skipped so pasted text doesn't trigger keybindings.
    pub(crate) in_bracket_paste: bool,
    /// Host terminal-window focus, tracked via DEC mode 1004 focus reports.
    /// Gates the per-tick "viewed" stamp so an unfocused window stops
    /// suppressing the focused agent's attention flag. Fails open until the
    /// first focus event of the run is observed.
    pub(crate) terminal_focus: crate::focus::TerminalFocus,
    pub(crate) macro_bar: Option<MacroBarState>,
    pub(crate) sigwinch_flag: Arc<AtomicBool>,
    /// Registration id for the SIGWINCH handler, unregistered when the App is
    /// consumed by the TUI→server flip so repeated flip cycles don't accumulate
    /// orphaned signal-hook registrations. `None` only in tests that build the
    /// App directly without registering a real handler.
    pub(crate) sigwinch_sig_id: Option<signal_hook::SigId>,
    /// Set by the SIGTERM/SIGINT/SIGHUP handlers so the run loop can break with
    /// [`RunExit::Quit`] and wind the agents down gracefully (SIGTERM + grace)
    /// instead of letting the process die straight to the hard SIGKILL in
    /// `PtyClient::drop`. Mirrors the server's signal-triggered `shutdown_ptys`.
    pub(crate) shutdown_flag: Arc<AtomicBool>,
    /// Registration ids for the shutdown-signal handlers, unregistered in
    /// `into_engine` so the TUI→server flip doesn't leave the TUI's handlers
    /// firing alongside the server's own. Empty only in tests that build the App
    /// directly without registering real handlers.
    pub(crate) shutdown_sig_ids: Vec<signal_hook::SigId>,
    pub(crate) force_redraw: bool,
    pub(crate) welcome_tip_index: usize,
    /// Whether the ASCII logo was rendered in the previous frame.
    pub(crate) welcome_logo_visible: bool,
    /// The left-pane selection index when the logo last rendered a tip.
    pub(crate) welcome_tip_selection: usize,
    /// When true, show the alternate (duck) logo instead of the text logo.
    pub(crate) welcome_logo_alt: bool,
    pub(crate) pr_banner_at_bottom: bool,
    /// Cached syntax highlighting resources shared across diff computations.
    pub(crate) syntax_cache: SyntaxCache,
    /// Reusable snapshot buffer to avoid per-frame allocation of terminal cells.
    pub(crate) snapshot_buf: TerminalSnapshot,
    /// ID of the provider that last populated `snapshot_buf`, used to detect
    /// agent switches and force a snapshot rebuild.
    last_snapshot_id: Option<String>,
    /// Active text selection in the terminal viewport, if any.
    pub(crate) terminal_selection: Option<TerminalSelection>,
    /// Active text selection in the startup command log output pane, if any.
    pub(crate) startup_log_selection: Option<TerminalSelection>,
    /// When set, the run loop exits with [`RunExit::FlipToServer`], handing the
    /// pre-bound listeners and their display URLs to the binary so the web server
    /// can take over the same process (PTYs keep running). Populated by the
    /// `StartWebServer` palette action only after its (worker) pre-flight
    /// succeeds. LOCAL MODE may bind more than one address (loopback + Tailscale).
    pub(crate) pending_server_flip: Option<(Vec<std::net::TcpListener>, Vec<String>)>,
    /// In-flight guard for the server-flip pre-flight. `start_web_server` spawns a
    /// worker that races to `bind` the LOCAL MODE ports; two quick invocations
    /// would both spawn workers and the second would hit a confusing EADDRINUSE.
    /// Set true when a worker is dispatched, cleared when its
    /// `ServerFlipPreflightReady` event lands (BOTH the Ok and Err arms). While
    /// set — or while `pending_server_flip` is already stashed — a repeat
    /// invocation is refused with an actionable status instead of spawning a
    /// second worker.
    pub(crate) server_flip_preflight_pending: bool,
    /// In-flight project-persistence status ops whose final is decided in the
    /// completion handler. Each non-`Add` persistence dispatch mints a
    /// [`dux_core::engine::HandlerStatusOp`] (its own opaque id), shows its
    /// pending busy, and stashes it here keyed by that id. The matching
    /// `ProjectPersistenceOutcome` carries the id back; the handler removes the
    /// op, builds a [`PersistFinalOutcome`] (Saved / DbFailed / ConfigWriteFailed)
    /// and resolves it into the keyed final. The op encapsulates the per-action
    /// success and db-failure message text declared at dispatch, so the handler
    /// only supplies which branch fired and any error string.
    pub(crate) pending_persist_ops:
        HashMap<String, dux_core::engine::HandlerStatusOp<PersistFinalOutcome>>,
    /// In-flight worktree-picker load ops whose final is decided in the
    /// completion handler. The picker dispatch mints a
    /// [`dux_core::engine::HandlerStatusOp`] (its own opaque id), shows its
    /// pending busy, and stashes it here keyed by that id. The matching
    /// [`dux_core::engine::EventReaction::ProjectWorktreesArrived`] carries the
    /// id back; the handler pops the op and resolves it against the
    /// handler-computed [`WorktreesFinalOutcome`] (the final depends on whether
    /// the picker is still open and matching, which the worker can't see).
    pub(crate) pending_worktree_ops:
        HashMap<String, dux_core::engine::HandlerStatusOp<WorktreesFinalOutcome>>,
    /// In-flight PR-lookup status ops (the "Resolving PR for project…" busy).
    /// The lookup dispatch mints a [`dux_core::engine::HandlerStatusOp`] (its own
    /// opaque id), shows its pending busy, stashes it here keyed by that id, and
    /// threads the id through the lookup worker. Both terminal outcomes resolve
    /// the op to a [`dux_core::engine::Final::Clear`] in `drain_events` when the
    /// `PullRequestResolved` event returns (keyed off the id it carries back):
    /// the SUCCESS path then opens the name prompt and shows its own `set_info`,
    /// and the FAILURE path lets the engine's error `Status` show — so the op
    /// only needs to DISMISS its keyed busy, never author a message. The opaque
    /// correlation guarantees the spinner is replaced instead of stranding to the
    /// busy timeout, even though the visible final comes from elsewhere.
    pub(crate) pending_pr_lookup_ops:
        HashMap<String, dux_core::engine::HandlerStatusOp<PrLookupFinalOutcome>>,
    /// In-flight async worktree-deletion status ops (the "Removing worktree for
    /// agent …" busy). When `begin_delete_session` takes the async path the TUI
    /// mints a [`dux_core::engine::HandlerStatusOp`] (its own opaque id), shows
    /// its pending busy, and stashes it here keyed by the **session id** (not the
    /// op id — the completion event carries `session_id`, so that is the natural
    /// correlation handle). The matching
    /// [`dux_core::engine::EventReaction::WorktreeRemoveSucceeded`] /
    /// [`WorktreeRemoveFailed`] pops the op and resolves it against the
    /// handler-computed [`TuiDeleteOutcome`]. The resolver, declared at dispatch,
    /// captures the provider / project name / branch name / display name then in
    /// scope (the session is still present at dispatch — cleanup is deferred until
    /// git succeeds) and reproduces the surface's exact wording.
    pub(crate) pending_delete_ops:
        HashMap<String, dux_core::engine::HandlerStatusOp<TuiDeleteOutcome>>,
    /// In-flight reconnect / fresh-restart status ops (the "Launching agent …" /
    /// "Starting fresh agent …" busy). When `reconnect_selected_session` or
    /// `restart_selected_session_fresh` dispatches a launch the TUI mints a
    /// [`dux_core::engine::HandlerStatusOp`] (its own opaque id), shows its pending
    /// busy, and stashes it here keyed by the **session id** — the natural
    /// correlation handle because the shared `AgentLaunchReadyView` /
    /// `AgentLaunchFailedOutcome` reactions that produce the final all carry the
    /// session id. The matching ready (Reconnect / ResumeFallback / SessionMissing)
    /// or failed (Reconnect / ForceReconnect) view pops the op and resolves it
    /// against the handler-computed [`TuiReconnectOutcome`], reproducing the exact
    /// final wording. Create-kind launches are NOT routed through this map: their
    /// busy/final ride the SHARED engine-side create op
    /// (`Engine::pending_create_ops`), resolved engine-side to a keyed `Status` that
    /// both surfaces apply.
    pub(crate) pending_reconnect_ops:
        HashMap<String, dux_core::engine::HandlerStatusOp<TuiReconnectOutcome>>,
    /// In-flight checkout / branch-inspection status ops. Three TUI dispatches feed
    /// this one map, all keyed by their op's own opaque id and all resolving to a
    /// [`dux_core::engine::Final::Clear`] (the visible final comes from elsewhere —
    /// the engine's unkeyed `Status` for the inspect/switch terminals, or a TUI
    /// `set_info`/`finish_add_project_with_status`/`set_error` in the view handler),
    /// so the op only DISMISSES its keyed busy and never strands to the busy
    /// timeout:
    ///
    /// 1. `dispatch_non_default_branch_checkout` (add-project & checkout-default
    ///    switch): id threaded through `run_add_project_checkout_job`, resolved when
    ///    `NonDefaultBranchCheckoutCompleted` returns carrying it.
    /// 2. `dispatch_create_agent_branch_inspection`: id threaded through the
    ///    inspection job, resolved when `CreateAgentBranchInspected` returns.
    /// 3. `checkout_selected_project_default_branch`: id threaded through worker 1,
    ///    resolved when `CheckoutProjectDefaultBranchInspected` short-circuits
    ///    (already-leading / heuristic / inspect-failed), OR — on the Known case —
    ///    re-emitted as a `progress` busy and the SAME id forwarded into worker 2 so
    ///    ONE op spans the inspect→switch chain (one spinner, changing text),
    ///    resolved when that worker's `NonDefaultBranchCheckoutCompleted` returns.
    pub(crate) pending_checkout_inspect_ops:
        HashMap<String, dux_core::engine::HandlerStatusOp<TuiCheckoutInspectOutcome>>,
    /// In-flight server-flip status op (the "Starting the web server …" busy). A
    /// flip is terminal — guarded so only one can be in flight — so a single
    /// `Option` is the natural home rather than a map. `start_web_server` mints a
    /// [`dux_core::engine::HandlerStatusOp`], shows its keyed busy, and stashes it
    /// here. When `ServerFlipPreflightReady` lands: the plain-success arm re-emits
    /// the busy text (now carrying the serve URLs) via [`progress`] on the SAME id
    /// and LEAVES the op stashed — there is no success final, the spinner simply
    /// shows until the run loop tears the TUI down for the flip; the
    /// success-with-warning arm resolves the op to a warning final; the error arm
    /// resolves it to an error final.
    ///
    /// [`progress`]: dux_core::engine::HandlerStatusOp::progress
    pub(crate) pending_server_flip_op:
        Option<dux_core::engine::HandlerStatusOp<TuiServerFlipOutcome>>,
    /// In-flight config-reload status op (the "Reloading config.toml." busy).
    /// `reload_config_from_disk` mints a [`dux_core::engine::HandlerStatusOp`]
    /// only when a reload worker actually spawned, shows its keyed busy, and
    /// stashes it here (a reload is terminal, so an `Option` suffices). The
    /// matching `ApplyReloadedConfig` (success) or `OpenConfigReloadFailedModal`
    /// (failure) handler pops the op and resolves it against the handler-computed
    /// [`TuiConfigReloadOutcome`], REPLACING the legacy `set_info`/`set_error`.
    /// The shared engine `ConfigReloadReady`/`ApplyReloadedConfig` logic (which
    /// also drives the web and replays deferred commands) is untouched — only the
    /// TUI's view-handler final is routed through the op.
    pub(crate) pending_config_reload_op:
        Option<dux_core::engine::HandlerStatusOp<TuiConfigReloadOutcome>>,
}

/// Handler-resolved outcome for the server-flip op (see
/// [`App::pending_server_flip_op`]). The plain-success case never resolves the op
/// (it re-emits `progress` and lets the spinner ride until the flip), so only the
/// two terminal-with-message cases are represented here.
pub enum TuiServerFlipOutcome {
    /// Pre-flight succeeded but carried a non-fatal warning (e.g. Tailscale not
    /// detected, serving loopback-only). Resolves to a warning final whose text is
    /// byte-identical to the legacy `set_warning`.
    Warned(String),
    /// Pre-flight failed; resolves to an error final whose text is byte-identical
    /// to the legacy `set_error`.
    Failed(String),
}

/// Handler-resolved outcome for the config-reload op (see
/// [`App::pending_config_reload_op`]). Each variant carries the exact, byte-
/// identical message the legacy `set_info`/`set_error` produced.
pub enum TuiConfigReloadOutcome {
    /// The reloaded config applied cleanly. Resolves to the success info line.
    Applied,
    /// Validation passed but applying the config failed. Resolves to the
    /// apply-failure error line, interpolating the error detail.
    ApplyFailed(String),
    /// Validation failed; the reload-failed modal is opened. Resolves to the
    /// review-the-modal error line.
    ValidationFailed,
}

/// Handler-resolved outcome for a checkout / branch-inspection op (see
/// [`App::pending_checkout_inspect_ops`]). The single `Done` variant resolves to a
/// clear: every visible final message is authored elsewhere (the engine's unkeyed
/// `Status`, or a TUI `set_info`/`set_error` in the view handler), so the op's only
/// job is to dismiss its keyed busy once that final is in place.
pub enum TuiCheckoutInspectOutcome {
    Done,
}

/// Handler-computed outcome for a reconnect / fresh-restart status op (see
/// [`App::pending_reconnect_ops`]). The terminal message is fully carried by the
/// async launch reaction (the engine computes the success `status_message`; the
/// failure arms carry `branch_name` + `message`), so the resolver reads it from
/// this outcome rather than capturing dispatch-time state.
pub enum TuiReconnectOutcome {
    /// The launch is ready (Reconnect / ResumeFallback). `status_message` is the
    /// engine-computed success line, shown verbatim.
    Ready { status_message: String },
    /// A plain reconnect launch failed; produces the "Reconnect failed …" line.
    ReconnectFailed {
        branch_name: String,
        message: String,
    },
    /// A fresh-restart (force-reconnect) launch failed; produces the "Fresh
    /// restart failed …" line.
    ForceReconnectFailed {
        branch_name: String,
        message: String,
    },
    /// The session vanished between dispatch and launch (SessionMissing). Dismiss
    /// the busy with no replacement message, mirroring the legacy clear.
    Missing,
}

/// Map a [`TuiReconnectOutcome`] to its final user message. Shared by the keyed
/// op resolver (`build_reconnect_status_op`) and the anonymous fallback path
/// (`resolve_reconnect_op_or`, used when no op was stashed) so the wording cannot
/// drift between the two.
pub(crate) fn reconnect_final(o: &TuiReconnectOutcome) -> dux_core::engine::Final {
    match o {
        TuiReconnectOutcome::Ready { status_message } => {
            dux_core::engine::Final::info(status_message.clone())
        }
        TuiReconnectOutcome::ReconnectFailed {
            branch_name,
            message,
        } => dux_core::engine::Final::error(format!(
            "Reconnect failed for agent \"{branch_name}\": {message}"
        )),
        TuiReconnectOutcome::ForceReconnectFailed {
            branch_name,
            message,
        } => dux_core::engine::Final::error(format!(
            "Fresh restart failed for agent \"{branch_name}\": {message}"
        )),
        TuiReconnectOutcome::Missing => dux_core::engine::Final::clear(),
    }
}

/// Handler-computed outcome for an async worktree-deletion op (see
/// [`App::pending_delete_ops`]). The completion event only knows whether the git
/// removal succeeded and (on success) whether the branch was already gone; the
/// handler additionally observes whether the session record is still present.
/// The resolver (declared at dispatch, where the session was in scope) maps this
/// to the final user message.
pub enum TuiDeleteOutcome {
    /// Git removal succeeded and the session record is still present (the normal
    /// case — cleanup runs now). `branch_already_deleted` selects the message.
    SucceededPresent { branch_already_deleted: bool },
    /// Git removal succeeded but the session was already removed by another path
    /// (e.g. its project was deleted) before the worker reported back. Resolve to
    /// the legacy "Worktree removal finished." line when our busy was still
    /// showing, else a clear (no message) to preserve the old suppression.
    SucceededGone { our_busy_still_showing: bool },
    /// Git removal failed and the session record is still present at completion,
    /// so the message can name the agent.
    FailedNamed { message: String },
    /// Git removal failed and the session was already gone at completion, so the
    /// message cannot name the agent.
    FailedBare { message: String },
}

/// Handler-resolved outcome for a PR-lookup op (see [`App::pending_pr_lookup_ops`]).
/// Both variants resolve to a clear: the user-visible message is produced by the
/// downstream path (the name prompt's `set_info` on success, the engine's error
/// `Status` on failure), so the op's only job is to dismiss its keyed busy.
pub enum PrLookupFinalOutcome {
    /// The lookup resolved; the TUI opens the name prompt (whose `set_info` is the
    /// visible final), so this op's busy is dismissed with no replacement message.
    HandedOff,
    /// The lookup failed; the engine already emitted the error `Status`, so this
    /// op's busy is dismissed with no replacement message.
    Failed,
}

/// Handler-computed outcome for a worktree-picker load op (see
/// [`App::pending_worktree_ops`]). Which final fires depends on whether the
/// picker is still open and matching when the worktrees arrive, a fact the
/// worker never sees. The op's resolver (declared at dispatch) maps this to the
/// final user message.
pub enum WorktreesFinalOutcome {
    /// The picker is still open and the worktrees loaded successfully.
    Loaded,
    /// The picker is still open but the load failed; carries the error.
    Failed(String),
    /// The picker was dismissed or switched before its worktrees loaded, so
    /// nothing consumed the result. Dismiss the busy with no message.
    Dismissed,
}

/// Handler-computed outcome for a project-persistence op (see
/// [`App::pending_persist_ops`]). The worker writes only SQLite; the TUI handler
/// then runs the fallible config write, producing one of three results the
/// worker never sees. The op's resolver (declared at dispatch) maps this to the
/// final user message.
pub enum PersistFinalOutcome {
    /// SQLite write succeeded and the post-worker config.toml write succeeded.
    Saved,
    /// The SQLite write itself failed; carries the formatted error.
    DbFailed(String),
    /// SQLite succeeded but the post-worker config.toml write failed; carries
    /// the formatted error.
    ConfigWriteFailed(String),
}

/// How [`App::run`] returned: a plain quit, or a request to flip the current
/// process into the web server while keeping the live agents running.
pub enum RunExit {
    Quit,
    FlipToServer {
        listeners: Vec<std::net::TcpListener>,
        urls: Vec<String>,
    },
}

/// Whether the shared App constructor should relaunch prior sessions. First
/// boot restores them from the database; a resume after the web server stops
/// skips restoration because the providers are already live.
enum SessionRestore {
    Restore,
    Skip,
}

/// Signal wiring handed to `App::assemble`: the flags the run loop polls plus
/// the signal-hook registration ids, all unregistered in `into_engine` so flip
/// cycles don't accumulate handlers. `sigwinch_sig_id` is `None` (and
/// `shutdown_sig_ids` empty) only in tests that build the App directly without
/// registering real handlers.
struct SignalHandles {
    sigwinch_flag: Arc<AtomicBool>,
    sigwinch_sig_id: Option<signal_hook::SigId>,
    shutdown_flag: Arc<AtomicBool>,
    shutdown_sig_ids: Vec<signal_hook::SigId>,
}

/// Register the SIGWINCH handler (terminal resize) plus the shutdown handlers
/// (SIGTERM/SIGINT/SIGHUP) that let the TUI wind agents down gracefully before
/// exit. SIGINT is included for an external `kill -INT`; an interactive Ctrl-c
/// is delivered as a key event in raw mode, not as SIGINT. Each handler only
/// sets its atomic flag (async-signal-safe); the run loop polls both flags.
///
/// This is also called from `App::resume` after a TUI→server→TUI flip. Both the
/// TUI and the server's `tokio::signal` register through the same process-global
/// `signal-hook-registry`, whose master OS handler is installed once (here, on
/// the TUI's first boot) and routes each signal to whatever actions are live. So
/// re-registering on resume re-arms graceful shutdown, provided the server does
/// not reset the disposition to `SIG_DFL` on hand-back, which it deliberately no
/// longer does (see the `ReturnToTui` branch of `serve_with_engine`).
fn register_signal_handles() -> Result<SignalHandles> {
    let sigwinch_flag = Arc::new(AtomicBool::new(false));
    let sigwinch_sig_id =
        signal_hook::flag::register(signal_hook::consts::SIGWINCH, Arc::clone(&sigwinch_flag))?;

    let shutdown_flag = Arc::new(AtomicBool::new(false));
    let mut shutdown_sig_ids = Vec::new();
    for signal in [
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGHUP,
    ] {
        shutdown_sig_ids.push(signal_hook::flag::register(
            signal,
            Arc::clone(&shutdown_flag),
        )?);
    }

    Ok(SignalHandles {
        sigwinch_flag,
        sigwinch_sig_id: Some(sigwinch_sig_id),
        shutdown_flag,
        shutdown_sig_ids,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FocusPane {
    Left,
    Center,
    Files,
}

impl FocusPane {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Left => Self::Center,
            Self::Center => Self::Files,
            Self::Files => Self::Left,
        }
    }

    pub(crate) fn previous(self) -> Self {
        match self {
            Self::Left => Self::Files,
            Self::Center => Self::Left,
            Self::Files => Self::Center,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RightSection {
    Unstaged,
    Staged,
    CommitInput,
}

impl RightSection {
    /// Returns the next section, or `None` to exit the pane.
    /// Order: Unstaged → Staged → CommitInput.
    pub(crate) fn next(self, has_staged: bool) -> Option<Self> {
        match self {
            Self::Unstaged if has_staged => Some(Self::Staged),
            Self::Unstaged => None,
            Self::Staged => Some(Self::CommitInput),
            Self::CommitInput => None,
        }
    }

    /// Returns the previous section, or `None` to exit the pane.
    pub(crate) fn previous(self) -> Option<Self> {
        match self {
            Self::CommitInput => Some(Self::Staged),
            Self::Staged => Some(Self::Unstaged),
            Self::Unstaged => None,
        }
    }

    /// First section when entering the pane (always Changes/Unstaged on top).
    pub(crate) fn first() -> Self {
        Self::Unstaged
    }

    pub(crate) fn last(has_staged: bool) -> Self {
        if has_staged {
            Self::CommitInput
        } else {
            Self::Unstaged
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FullscreenOverlay {
    None,
    Agent,
    Terminal,
    StartupLog,
}

#[derive(Clone, Debug)]
pub(crate) enum CenterMode {
    Agent,
    Diff {
        lines: Arc<Vec<Line<'static>>>,
        scroll: u16,
        /// Display-column width of the gutter (0 when line numbers are off).
        gutter_width: usize,
        /// Source paths for re-generating the diff on setting changes.
        worktree_path: String,
        rel_path: String,
    },
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum KillableRuntimeKind {
    Agent,
    Terminal,
}

impl KillableRuntimeKind {
    pub(crate) fn noun(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Terminal => "terminal",
        }
    }

    pub(crate) fn badge(self) -> &'static str {
        match self {
            Self::Agent => "AGENT",
            Self::Terminal => "TERM",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RuntimeTargetId {
    Agent(String),
    /// An extra tab's provider process (keyed by tab id). Killing it stops the
    /// process but keeps the tab row, so the tab becomes dormant (unlike
    /// `Agent`, which detaches the whole session).
    Tab(String),
    Terminal(String),
}

#[derive(Clone, Debug)]
pub(crate) struct KillableRuntime {
    pub(crate) id: RuntimeTargetId,
    pub(crate) kind: KillableRuntimeKind,
    pub(crate) label: String,
    pub(crate) context: String,
    pub(crate) search_text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KillRunningAction {
    Hovered,
    Selected,
    Visible,
}

impl KillRunningAction {
    pub(crate) fn button_label(self) -> &'static str {
        match self {
            Self::Hovered => "Kill Hovered",
            Self::Selected => "Kill Selected",
            Self::Visible => "Kill Visible",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KillRunningFooterAction {
    Cancel,
    Hovered,
    Selected,
    Visible,
}

impl KillRunningFooterAction {
    pub(crate) fn button_label(self) -> &'static str {
        match self {
            Self::Cancel => "Cancel",
            Self::Hovered => KillRunningAction::Hovered.button_label(),
            Self::Selected => KillRunningAction::Selected.button_label(),
            Self::Visible => KillRunningAction::Visible.button_label(),
        }
    }

    pub(crate) fn action(self) -> Option<KillRunningAction> {
        match self {
            Self::Cancel => None,
            Self::Hovered => Some(KillRunningAction::Hovered),
            Self::Selected => Some(KillRunningAction::Selected),
            Self::Visible => Some(KillRunningAction::Visible),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KillRunningFocus {
    List,
    Footer(KillRunningFooterAction),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChangeAgentProviderFocus {
    List,
    Cancel,
    Apply,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChangeAgentProviderMode {
    /// The ctrl+p retarget of an existing focused tab (change_tab_provider).
    Retarget,
    /// The new-agent-tab picker: create a fresh tab with the chosen provider
    /// (create_tab). There is no existing tab to retarget yet, so `tab_id` on
    /// the prompt is unused in this mode.
    NewTab,
}

#[derive(Clone, Debug)]
pub(crate) struct KillRunningPrompt {
    pub(crate) runtimes: Vec<KillableRuntime>,
    pub(crate) filter: TextInput,
    pub(crate) searching: bool,
    pub(crate) hovered_visible_index: usize,
    pub(crate) selected_ids: HashSet<RuntimeTargetId>,
    pub(crate) focus: KillRunningFocus,
}

#[derive(Clone, Debug)]
pub(crate) struct ChangeAgentProviderOption {
    pub(crate) provider: ProviderKind,
    /// True when this provider's config has `resume_args`. Providers
    /// without resume support (e.g. Copilot CLI) always start fresh.
    pub(crate) supports_resume: bool,
    /// True when `supports_resume` AND this provider has been launched on
    /// this worktree before, so the next launch will actually resume.
    pub(crate) resume_available: bool,
    pub(crate) is_current: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ChangeAgentProviderPrompt {
    pub(crate) session_id: String,
    /// The tab being retargeted. Equals `session_id` for the session-slot tab (which
    /// delegates to the session-level provider change); an extra tab id
    /// otherwise. Lets `ctrl+p` retarget the focused tab, not just the agent.
    /// Unused (set equal to `session_id` as a harmless placeholder) when
    /// `mode == ChangeAgentProviderMode::NewTab`, since there is no tab yet.
    pub(crate) tab_id: String,
    pub(crate) session_label: String,
    pub(crate) worktree_path: String,
    pub(crate) options: Vec<ChangeAgentProviderOption>,
    pub(crate) selected: usize,
    pub(crate) focus: ChangeAgentProviderFocus,
    pub(crate) mode: ChangeAgentProviderMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChangeDefaultProviderFocus {
    List,
    Cancel,
    Apply,
}

#[derive(Clone, Debug)]
pub(crate) struct ChangeDefaultProviderOption {
    pub(crate) provider: ProviderKind,
    pub(crate) is_current: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ChangeDefaultProviderPrompt {
    pub(crate) current: ProviderKind,
    pub(crate) options: Vec<ChangeDefaultProviderOption>,
    pub(crate) selected: usize,
    pub(crate) focus: ChangeDefaultProviderFocus,
}

#[derive(Clone, Debug)]
pub(crate) struct ChangeProjectDefaultProviderOption {
    pub(crate) provider: Option<ProviderKind>,
    pub(crate) is_current: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ChangeProjectDefaultProviderPrompt {
    pub(crate) project_id: String,
    pub(crate) project_name: String,
    pub(crate) current: ProviderKind,
    pub(crate) global_default: ProviderKind,
    pub(crate) inherits_global_default: bool,
    pub(crate) options: Vec<ChangeProjectDefaultProviderOption>,
    pub(crate) selected: usize,
    pub(crate) focus: ChangeDefaultProviderFocus,
}

/// Semantic tone of an Agent Info body line, computed once at build time so the
/// renderer styles by the tag rather than re-parsing the prose. Only the drift
/// note carries [`AgentInfoTone::Warning`]; everything else is neutral.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AgentInfoTone {
    Neutral,
    Warning,
}

/// Read-only Agent Info modal state: a display label plus the prebuilt body
/// lines (name, provider, branch lineage, worktree, created, status), each
/// tagged with its [`AgentInfoTone`]. Built once on open by [`agent_info_lines`];
/// the renderer only styles (by tag) and frames them.
#[derive(Clone, Debug)]
pub(crate) struct AgentInfoPrompt {
    pub(crate) session_label: String,
    pub(crate) lines: Vec<(String, AgentInfoTone)>,
}

/// True when an agent's `current` branch has drifted from the `initial` branch
/// it was created on. Guards against an empty `initial` (a legacy row that
/// predates the `initial_branch` column and was never backfilled, or a
/// transient pre-persist state) so an empty original never shows a phantom
/// `(orig: )` drift. This is the single source of drift truth for the TUI,
/// mirroring the web's `branchDrift()` helper; route every drift check through
/// it rather than recomputing `current != initial` inline.
pub(crate) fn branch_drifted(current: &str, initial: &str) -> bool {
    !initial.is_empty() && current != initial
}

/// The attention glyph's blink rhythm as a pure function of elapsed wall-clock
/// milliseconds: exactly two quick blinks (two hides), then steady until the
/// cycle restarts. Deliberately NO separator hide at the end of the cycle: a
/// hold bounded by hides on both sides reads as a third blink. Mirrors the web
/// dot's double-pulse-then-hold animation (`--animate-attention-pulse` in the
/// web `index.css`). Every window is a multiple of 200ms, two full event-loop
/// ticks at the 100ms poll cadence, so no phase can fall between redraws and
/// get swallowed.
pub(crate) fn attention_blink_phase(elapsed_ms: u128) -> bool {
    match elapsed_ms % 2000 {
        200..=399 => false, // blink 1: hide
        600..=799 => false, // blink 2: hide
        _ => true,          // leading show, the gap between blinks, and the hold
    }
}

/// Build the body lines of the Agent Info modal from a session: name, provider,
/// the current/original/forked-from branches, a drift note when the current
/// branch differs from the branch the agent was created on, then the worktree,
/// creation time, and status. Each line carries its semantic tone so the
/// renderer never has to substring-match prose. Pure and unit-tested.
pub(crate) fn agent_info_lines(session: &AgentSession) -> Vec<(String, AgentInfoTone)> {
    let name = session
        .title
        .clone()
        .unwrap_or_else(|| session.branch_name.clone());
    let mut lines = vec![
        (format!("Name:         {name}"), AgentInfoTone::Neutral),
        (
            format!("Provider:     {}", session.provider.as_str()),
            AgentInfoTone::Neutral,
        ),
        (
            format!("Current:      {}", session.branch_name),
            AgentInfoTone::Neutral,
        ),
        (
            format!("Original:     {}", session.initial_branch),
            AgentInfoTone::Neutral,
        ),
        (
            format!("Forked from:  {}", session.source_branch),
            AgentInfoTone::Neutral,
        ),
    ];
    if branch_drifted(&session.branch_name, &session.initial_branch) {
        lines.push((
            format!(
                "Branch changed since creation (orig: {})",
                session.initial_branch
            ),
            AgentInfoTone::Warning,
        ));
    }
    lines.push((
        format!("Worktree:     {}", session.worktree_path),
        AgentInfoTone::Neutral,
    ));
    lines.push((
        format!(
            "Created:      {}",
            session.created_at.format("%Y-%m-%d %H:%M")
        ),
        AgentInfoTone::Neutral,
    ));
    lines.push((
        format!("Status:       {}", session.status.as_str()),
        AgentInfoTone::Neutral,
    ));
    lines
}

#[derive(Clone, Debug)]
pub(crate) struct ChangeThemePrompt {
    pub(crate) options: Vec<crate::theme::ThemeListing>,
    pub(crate) selected: usize,
    pub(crate) current: String,
}

#[derive(Clone, Debug)]
pub(crate) struct StartupCommandLogPrompt {
    pub(crate) scope_label: String,
    pub(crate) entries: Vec<crate::startup::StartupCommandLogEntry>,
    pub(crate) selected: usize,
    pub(crate) filter: TextInput,
    pub(crate) searching: bool,
    pub(crate) content: String,
    pub(crate) scroll_offset: u16,
}

#[derive(Clone, Debug)]
pub(crate) struct StartupLogViewer {
    pub(crate) scope_label: String,
    pub(crate) path: Option<PathBuf>,
    pub(crate) display_name: String,
    pub(crate) content: String,
    pub(crate) scroll_offset: u16,
    pub(crate) search: TextInput,
    pub(crate) searching: bool,
}

#[derive(Clone, Debug)]
pub(crate) enum ProjectWorktreeVisualRow {
    Header(&'static str),
    Empty(String),
    Entry(usize),
}

#[derive(Clone, Debug)]
pub(crate) struct PickProjectWorktreePrompt {
    pub(crate) project: Project,
    pub(crate) entries: Vec<ProjectWorktreeEntry>,
    pub(crate) loading: bool,
    pub(crate) selected: Option<usize>,
    pub(crate) error: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct ConfirmKillRunningPrompt {
    pub(crate) previous: KillRunningPrompt,
    pub(crate) action: KillRunningAction,
    pub(crate) target_ids: Vec<RuntimeTargetId>,
    pub(crate) confirm_selected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConfigReloadFailedFocus {
    Close,
    Apply,
    Checkbox,
}

/// Which selectable element has focus in the Delete Agent confirmation modal.
/// Focus cycles through all three via Tab / arrow keys / h / l.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeleteAgentFocus {
    Cancel,
    Delete,
    Checkbox,
}

/// Which selectable element has focus in the Non-Default Branch confirmation
/// modal. `Checkbox` is only reachable when `BranchWarningKind::Known` — the
/// heuristic path has no checkbox to focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConfirmNonDefaultBranchFocus {
    Cancel,
    Add,
    Checkbox,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NameNewAgentFocus {
    Input,
    RandomizedNameCheckbox,
    /// Only reachable for `CreateAgentRequest::NewProject`: forks always
    /// copy and the other flows never do, so only fresh agents show the box.
    CopyChangesCheckbox,
}

#[derive(Clone, Debug)]
pub(crate) enum PromptState {
    None,
    Command {
        input: TextInput,
        selected: usize,
    },
    BrowseProjects {
        current_dir: PathBuf,
        entries: Vec<BrowserEntry>,
        loading: bool,
        selected: usize,
        filter: TextInput,
        searching: bool,
        editing_path: bool,
        path_input: TextInput,
        tab_completions: Vec<String>,
        tab_index: usize,
    },
    AddProjectFailed {
        message: String,
        return_prompt: Box<PromptState>,
    },
    /// Shown when adding a plain folder that is not a git repository (the
    /// adopt-a-folder flow). Confirming runs `git init`, seeds a starter
    /// `.gitignore` for the named candidate directories, creates an empty
    /// initial commit, then registers the project.
    ConfirmInitRepo {
        /// Absolute, canonical path of the folder being adopted.
        path: String,
        /// Display name entered by the user (empty derives it from the path).
        name: String,
        /// Starter-.gitignore candidate directory names found in the folder
        /// (display only; the worker re-derives the real list when seeding).
        candidates: Vec<String>,
        confirm_selected: bool, // false = Cancel (default), true = Initialize & Add
        /// The prompt to restore on cancel (the project browser with the
        /// user's location and typed path intact), the `AddProjectFailed`
        /// pattern.
        return_prompt: Box<PromptState>,
    },
    ChangeAgentProvider(ChangeAgentProviderPrompt),
    AgentInfo(AgentInfoPrompt),
    ChangeDefaultProvider(ChangeDefaultProviderPrompt),
    ChangeProjectDefaultProvider(ChangeProjectDefaultProviderPrompt),
    ChangeTheme(ChangeThemePrompt),
    ConfigureStartupCommand {
        project_id: String,
        project_name: String,
        input: TextInput,
    },
    ConfigureProjectEnv {
        project_id: String,
        project_name: String,
        input: TextInput,
    },
    ConfigureGlobalEnv {
        project_name: String,
        input: TextInput,
    },
    #[allow(dead_code)]
    StartupCommandLogs(StartupCommandLogPrompt),
    PickProjectWorktree(PickProjectWorktreePrompt),
    KillRunning(KillRunningPrompt),
    ConfirmKillRunning(ConfirmKillRunningPrompt),
    ConfigReloadFailed {
        error: String,
        recover_old_config: bool,
        focus: ConfigReloadFailedFocus,
    },
    ConfirmDeleteAgent {
        session_id: String,
        branch_name: String,
        focus: DeleteAgentFocus,
        delete_worktree: bool,
        /// True when one or more other sessions share this worktree. In that
        /// case the worktree is always preserved regardless of the user's
        /// choice, so the checkbox is hidden and a note is shown instead.
        worktree_shared: bool,
    },
    ConfirmDeleteTerminal {
        terminal_id: String,
        terminal_label: String,
        /// The foreground app running in the terminal, captured when the prompt
        /// opens, or `None` when only the shell is running. The "process will be
        /// killed" warning is shown only when an app is present, since closing
        /// an idle terminal merely ends the shell.
        foreground_cmd: Option<String>,
        confirm_selected: bool, // false = Cancel (default), true = Delete
    },
    /// Close/detach an agent tab. Closing the session-slot tab (`is_main`) detaches the
    /// agent (non-destructive, stays in Projects); closing an extra tab ends
    /// that session for good (destructive), so it defaults to Cancel.
    ConfirmCloseTab {
        session_id: String,
        tab_id: String,
        provider_label: String,
        is_main: bool,
        confirm_selected: bool, // false = Cancel (default), true = Close/Detach
    },
    ConfirmQuit {
        agent_count: usize,
        terminal_count: usize,
        confirm_selected: bool, // false = Cancel (default), true = Quit
    },
    ConfirmDiscardFile {
        file_path: String,
        is_untracked: bool,
        confirm_selected: bool, // false = Cancel (default), true = Discard
    },
    /// Shown when adding a project whose repo has no commits yet (a fresh
    /// `git init` with an unborn HEAD). Confirming creates an empty initial
    /// commit so the repo can back worktrees, then registers the project.
    ConfirmCreateInitialCommit {
        /// Absolute, validated path of the repo being added.
        path: String,
        /// Display name entered by the user (empty → derived from the path).
        name: String,
        confirm_selected: bool, // false = Cancel (default), true = Create & Add
    },
    RenameSession {
        session_id: String,
        input: TextInput,
        rename_branch: bool,
    },
    PullRequestInput {
        project: Project,
        input: TextInput,
    },
    NameNewAgent {
        request: CreateAgentRequest,
        input: TextInput,
        randomize_name: bool,
        randomized_name: Option<String>,
        /// Whether the new worktree copies the project checkout's uncommitted
        /// changes. Only surfaced (and only written into the request) for
        /// `CreateAgentRequest::NewProject`.
        copy_changes: bool,
        focus: NameNewAgentFocus,
    },
    PickEditor {
        session_label: String,
        worktree_path: String,
        editors: Vec<DetectedEditor>,
        selected: usize,
    },
    EditMacros {
        entries: Vec<(String, String, MacroSurface)>,
        selected: usize,
        editing: Option<MacroEditState>,
        pending_delete: Option<PendingMacroDelete>,
    },
    ConfirmNonDefaultBranch {
        action: NonDefaultBranchAction,
        current_branch: String,
        kind: BranchWarningKind,
        focus: ConfirmNonDefaultBranchFocus,
        /// When true and `kind == Known`, dux runs `git switch
        /// <default_branch>` in the source repo before registering the project.
        /// Ignored for `BranchWarningKind::Heuristic` because we can't
        /// confidently identify the target.
        checkout_default: bool,
    },
    ConfirmUseExistingBranch {
        request: CreateAgentRequest,
        branch_name: String,
        location: crate::git::BranchLocation,
        confirm_selected: bool, // false = Cancel (default), true = Use Existing
    },
    DebugInput {
        lines: Vec<Line<'static>>,
        scroll_offset: u16,
    },
    ResourceMonitor {
        rows: Vec<ResourceStats>,
        scroll_offset: u16,
        selected_row: usize,
        expanded: HashSet<u32>,
        last_refresh: Instant,
        /// Whether the MOST RECENT delivered sample had to re-establish its
        /// CPU baseline (`ResourceCollector::sample`'s `was_baseline`), and so
        /// is a real reading measured over the short baseline window rather
        /// than the normal ~2s poll interval. Drives the `~` marker in
        /// `render_resource_monitor`. This is a per-sample fact, not a
        /// "first sample since the overlay opened" guess: reopening the
        /// overlay within `STALE_BASELINE` of the collector's last refresh
        /// does not re-baseline, so it must not show `~` either. Starts
        /// `true` on open, since the very first sample of the app's lifetime
        /// (or after any real gap) always re-baselines.
        short_window_sample: bool,
    },
}

pub(crate) use dux_core::project_browser::leading_branch_for_project;

#[derive(Clone, Debug)]
pub(crate) enum VisualRow {
    /// Index into the `ResourceStats` rows vec.
    Parent(usize),
    /// (parent row index, child index within that parent's `children`).
    Child(usize, usize),
}

pub(crate) fn build_visual_rows(rows: &[ResourceStats], expanded: &HashSet<u32>) -> Vec<VisualRow> {
    let mut visual = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        visual.push(VisualRow::Parent(i));
        if let Some(pid) = row.pid
            && expanded.contains(&pid)
        {
            for (j, _) in row.children.iter().enumerate() {
                visual.push(VisualRow::Child(i, j));
            }
        }
    }
    visual
}

pub(crate) fn project_worktree_visual_rows(
    entries: &[ProjectWorktreeEntry],
    loading: bool,
    error: Option<&str>,
) -> Vec<ProjectWorktreeVisualRow> {
    if loading {
        return vec![ProjectWorktreeVisualRow::Empty(
            "Loading project worktrees...".to_string(),
        )];
    }
    if let Some(error) = error {
        return vec![ProjectWorktreeVisualRow::Empty(format!(
            "Could not load worktrees: {error}"
        ))];
    }

    let available = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.is_selectable)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let project_checkout = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.is_project_checkout)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let disabled = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| !entry.is_selectable && !entry.is_project_checkout)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();

    let mut rows = Vec::new();
    rows.push(ProjectWorktreeVisualRow::Header("Available Worktrees"));
    if available.is_empty() {
        rows.push(ProjectWorktreeVisualRow::Empty(
            "No available worktrees. Worktrees that already have agents are shown below."
                .to_string(),
        ));
    } else {
        rows.extend(available.into_iter().map(ProjectWorktreeVisualRow::Entry));
    }
    if !disabled.is_empty() {
        rows.push(ProjectWorktreeVisualRow::Header("Already Has Agent"));
        rows.extend(disabled.into_iter().map(ProjectWorktreeVisualRow::Entry));
    }
    if !project_checkout.is_empty() {
        rows.push(ProjectWorktreeVisualRow::Header("Project Checkout"));
        rows.extend(
            project_checkout
                .into_iter()
                .map(ProjectWorktreeVisualRow::Entry),
        );
    }
    rows
}

pub(crate) fn selectable_project_worktree_indices(entries: &[ProjectWorktreeEntry]) -> Vec<usize> {
    entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| entry.is_selectable.then_some(index))
        .collect()
}

#[derive(Clone, Debug)]
pub(crate) struct MacroBarState {
    pub(crate) input: TextInput,
    pub(crate) selected: usize,
    pub(crate) previous_input_target: InputTarget,
}

#[derive(Clone, Debug)]
pub(crate) struct MacroEditState {
    pub(crate) id: Option<String>,
    pub(crate) name_input: TextInput,
    pub(crate) text_input: TextInput,
    pub(crate) surface: MacroSurface,
    pub(crate) stage: MacroEditStage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MacroEditStage {
    EditName,
    EditText,
}

#[derive(Clone, Debug)]
pub(crate) struct PendingMacroDelete {
    pub(crate) name: String,
    pub(crate) confirm_selected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InputTarget {
    None,
    Agent,
    Terminal,
    CommitMessage,
    StartupCommand,
}

#[derive(Clone, Copy)]
pub(crate) enum ScrollDirection {
    Up,
    Down,
}

/// A position within the terminal grid (0-based row and column).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TermGridPos {
    pub row: u16,
    pub col: u16,
}

/// Active text selection in the terminal viewport.
#[derive(Clone, Debug)]
pub(crate) struct TerminalSelection {
    /// Where the drag started (anchor). Fixed during drag.
    pub anchor: TermGridPos,
    /// Current end of selection. Moves during drag.
    pub end: TermGridPos,
    /// True while the mouse button is held (still dragging).
    pub dragging: bool,
}

impl TerminalSelection {
    /// Returns (start, end) in reading order (top-left to bottom-right).
    pub fn ordered(&self) -> (TermGridPos, TermGridPos) {
        if self.anchor.row < self.end.row
            || (self.anchor.row == self.end.row && self.anchor.col <= self.end.col)
        {
            (self.anchor, self.end)
        } else {
            (self.end, self.anchor)
        }
    }

    /// Returns true if the given (row, col) is within the selection.
    /// Uses line-based (not rectangular) selection semantics.
    pub fn contains(&self, row: u16, col: u16) -> bool {
        let (start, end) = self.ordered();
        if row < start.row || row > end.row {
            return false;
        }
        if start.row == end.row {
            return col >= start.col && col <= end.col;
        }
        if row == start.row {
            return col >= start.col;
        }
        if row == end.row {
            return col <= end.col;
        }
        true // middle rows are fully selected
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct MouseLayoutState {
    pub(crate) body: Rect,
    pub(crate) left: Rect,
    pub(crate) center: Rect,
    pub(crate) right: Rect,
    pub(crate) left_list: Rect,
    pub(crate) terminal_list: Rect,
    pub(crate) agent_term: Option<Rect>,
    pub(crate) unstaged_list: Option<Rect>,
    pub(crate) staged_list: Option<Rect>,
    pub(crate) commit_area: Option<Rect>,
    pub(crate) commit_text_area: Option<Rect>,
}

impl MouseLayoutState {
    pub(crate) fn reset(&mut self, body: Rect, left: Rect, center: Rect, right: Rect) {
        self.body = body;
        self.left = left;
        self.center = center;
        self.right = right;
        self.left_list = Rect::default();
        self.terminal_list = Rect::default();
        self.agent_term = None;
        self.unstaged_list = None;
        self.staged_list = None;
        self.commit_area = None;
        self.commit_text_area = None;
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct OverlayMouseLayoutState {
    pub(crate) active: OverlayMouseLayout,
}

impl OverlayMouseLayoutState {
    pub(crate) fn reset(&mut self) {
        self.active = OverlayMouseLayout::None;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OverlayCheckboxId {
    DeleteAgentWorktree,
    RenameSessionBranch,
    NonDefaultBranchCheckoutDefault,
    NameNewAgentRandomizedPetName,
    NameNewAgentCopyChanges,
    ConfigReloadRecoverOldConfig,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OverlayCheckbox {
    pub(crate) id: OverlayCheckboxId,
    pub(crate) rect: Rect,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) enum OverlayMouseLayout {
    #[default]
    None,
    Help,
    Command {
        input: Rect,
        list: Rect,
        items: usize,
        offset: usize,
    },
    BrowseProjects {
        input: Option<Rect>,
        list: Rect,
        items: usize,
        offset: usize,
    },
    AddProjectFailed {
        ok_button: Rect,
    },
    AgentInfo {
        close_button: Rect,
    },
    ChangeAgentProvider {
        list: Rect,
        items: usize,
        offset: usize,
        cancel_button: Rect,
        apply_button: Rect,
    },
    ChangeDefaultProvider {
        list: Rect,
        items: usize,
        offset: usize,
        cancel_button: Rect,
        apply_button: Rect,
    },
    ChangeProjectDefaultProvider {
        list: Rect,
        items: usize,
        offset: usize,
        cancel_button: Rect,
        apply_button: Rect,
    },
    PickEditor {
        list: Rect,
        items: usize,
        offset: usize,
    },
    PickProjectWorktree {
        list: Rect,
        items: usize,
        offset: usize,
    },
    ChangeTheme {
        list: Rect,
        items: usize,
        offset: usize,
    },
    ResourceMonitor {
        list: Rect,
        items: usize,
        offset: usize,
    },
    StartupCommandLogs {
        area: Rect,
        list: Rect,
        body: Rect,
        items: usize,
        offset: usize,
        close_button: Rect,
    },
    ConfigureStartupCommand {
        input: Rect,
    },
    KillRunning {
        input: Option<Rect>,
        list: Rect,
        items: usize,
        offset: usize,
        cancel_button: Rect,
        hovered_button: Rect,
        selected_button: Rect,
        visible_button: Rect,
    },
    ConfirmKillRunning {
        cancel_button: Rect,
        kill_button: Rect,
    },
    ConfirmDeleteAgent {
        cancel_button: Rect,
        delete_button: Rect,
        checkbox: Option<OverlayCheckbox>,
    },
    ConfirmDeleteTerminal {
        cancel_button: Rect,
        delete_button: Rect,
    },
    ConfirmCloseTab {
        cancel_button: Rect,
        confirm_button: Rect,
    },
    ConfirmDeleteMacro {
        cancel_button: Rect,
        delete_button: Rect,
    },
    ConfirmQuit {
        cancel_button: Rect,
        quit_button: Rect,
    },
    ConfirmDiscardFile {
        cancel_button: Rect,
        discard_button: Rect,
    },
    ConfirmCreateInitialCommit {
        cancel_button: Rect,
        create_button: Rect,
    },
    ConfirmInitRepo {
        cancel_button: Rect,
        init_button: Rect,
    },
    ConfirmNonDefaultBranch {
        cancel_button: Rect,
        add_button: Rect,
        checkbox: Option<OverlayCheckbox>,
    },
    ConfirmUseExistingBranch {
        cancel_button: Rect,
        use_button: Rect,
    },
    ConfigReloadFailed {
        close_button: Rect,
        apply_button: Rect,
        checkbox: OverlayCheckbox,
    },
    RenameSession {
        input: Rect,
        checkbox: Option<OverlayCheckbox>,
    },
    NameNewAgent {
        input: Rect,
        checkbox: Option<OverlayCheckbox>,
        /// The "copy uncommitted changes" checkbox; present only for
        /// `CreateAgentRequest::NewProject` prompts.
        copy_checkbox: Option<OverlayCheckbox>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum ResizeDragState {
    LeftDivider,
    RightDivider,
    TerminalDivider,
    StagedDivider,
    CommitDivider,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MouseClickTarget {
    LeftPane,
    CenterPane,
    UnstagedPane,
    StagedPane,
    CommandPalette,
    StartupCommandInput,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RecentMouseClick {
    pub(crate) target: MouseClickTarget,
    pub(crate) item_index: Option<usize>,
    pub(crate) at: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LeftSection {
    Projects,
    Terminals,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LeftItem {
    Project(usize),
    Session(usize),
    /// Header for a group of orphaned sessions whose project record is gone
    /// (a removed project whose sessions outlived it). Carries a representative
    /// session index so the renderer can recover the ghost project id and its
    /// short display name. Non-selectable; its sessions are normal `Session`
    /// rows the user can still select and delete.
    OrphanProject(usize),
    EmptyProjectsSpacer,
    EmptyProjectsSeparator,
}

impl LeftItem {
    pub(crate) fn is_selectable(self) -> bool {
        matches!(self, LeftItem::Project(_) | LeftItem::Session(_))
    }
}

pub(crate) fn build_left_items(
    projects: &[Project],
    sessions: &[AgentSession],
    projects_with_terminals: &HashSet<String>,
    collapsed_projects: &HashSet<String>,
    empty_project_separator_min_projects: u16,
) -> Vec<LeftItem> {
    // Grouping, ordering, the agent-less split, and orphan detection are owned by
    // dux_core::sidebar so the TUI and web render an identical tree. Here we only
    // translate that core model into the TUI's index-based render items and apply
    // display state (collapse).
    let model = dux_core::sidebar::build_sidebar(
        projects,
        sessions,
        projects_with_terminals,
        empty_project_separator_min_projects,
    );
    let project_index: std::collections::HashMap<&str, usize> = projects
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id.as_str(), i))
        .collect();
    let session_index: std::collections::HashMap<&str, usize> = sessions
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.as_str(), i))
        .collect();

    let mut items = Vec::new();
    for (group_index, group) in model.groups.iter().enumerate() {
        if model.agentless_start == Some(group_index) {
            items.push(LeftItem::EmptyProjectsSpacer);
            items.push(LeftItem::EmptyProjectsSeparator);
        }
        let session_indices: Vec<usize> = group
            .session_ids
            .iter()
            .filter_map(|id| session_index.get(id.as_str()).copied())
            .collect();
        if group.orphaned {
            // Orphan groups always carry at least one session; use the first
            // index so the renderer can recover the ghost id and short name.
            let Some(&first) = session_indices.first() else {
                continue;
            };
            items.push(LeftItem::OrphanProject(first));
        } else if let Some(&index) = project_index.get(group.project_id.as_str()) {
            items.push(LeftItem::Project(index));
        } else {
            continue;
        }
        // Orphan groups are never collapsed: their header is non-selectable, so a
        // stale collapsed-projects entry for a ghost id could otherwise hide their
        // sessions permanently with no way to re-expand them.
        if group.path_missing || (!group.orphaned && collapsed_projects.contains(&group.project_id))
        {
            continue;
        }
        for index in session_indices {
            items.push(LeftItem::Session(index));
        }
    }
    items
}

mod components;
mod input;
mod render;
mod sessions;
#[cfg(test)]
mod test_support;
pub(crate) mod text_input;
mod workers;

// Re-export the welcome wordmark so the server status screen
// (`crate::server_screen`) can reuse it without making `render` public.
pub(crate) use render::ASCII_LOGO;

impl App {
    /// Bootstrap the TUI. The caller must have already resolved `paths`,
    /// created its directories, and acquired the single-instance lock.
    /// This ensures the lock covers every entrypoint (TUI + config
    /// subcommands) and that a losing process never touches shared state.
    pub fn bootstrap_with_lock(
        paths: DuxPaths,
        single_instance_lock: SingleInstanceLock,
    ) -> Result<Self> {
        let mut config = ensure_config(&paths)?;

        logger::init(&config.logging, &paths);
        logger::info("bootstrapping dux");

        // Validate and build runtime keybindings from config.
        if let Err(msg) = validate_keys(&config.keys) {
            eprintln!(
                "Configuration error in {}: {msg}",
                paths.config_path.display()
            );
            std::process::exit(1);
        }
        let bindings = RuntimeBindings::from_keys_config(&config.keys);
        let interactive_patterns = bindings.interactive_byte_patterns();

        // Register the SIGWINCH handler (so resizes are seen even when bypassing
        // crossterm's event reader during interactive mode) and the shutdown
        // handlers (SIGTERM/SIGINT/SIGHUP) so the run loop can wind agents down
        // gracefully instead of letting them die to the hard SIGKILL on drop.
        let signals = register_signal_handles()?;

        let session_store = SessionStore::open(&paths.sessions_db_path)?;
        sync_config_projects_with_store(&mut config, &paths, &bindings, &session_store)?;
        let projects = load_projects(
            &session_store.load_projects()?,
            &session_store.load_project_created_ats()?,
            &config,
        );
        persist_runtime_projects_to_config_and_store(
            &projects,
            &mut config,
            &paths,
            &bindings,
            &session_store,
        )?;
        let sessions = session_store.load_sessions()?;
        let agent_tabs = session_store.load_agent_tabs()?;
        let (worker_tx, worker_rx) = mpsc::channel();
        let watched_worktree: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
        let branch_sync_sessions = Arc::new(Mutex::new(Vec::new()));
        let pr_sync_sessions = Arc::new(Mutex::new(Vec::new()));
        let pr_sync_enabled = Arc::new(AtomicBool::new(false));
        let has_active_processes = Arc::new(AtomicBool::new(false));
        let initial_status = format!(
            "Press {} to add a project, {} to create an agent, {} for help.",
            bindings.label_for(Action::OpenProjectBrowser),
            bindings.label_for(Action::NewAgent),
            bindings.label_for(Action::ToggleHelp),
        );
        let (theme, theme_warning) = crate::theme::load_or_fallback(&config.ui.theme, &paths);
        let mut status = KeyedStatusController::with_clear_after(Duration::from_secs(
            config.ui.status_clear_seconds as u64,
        ));
        // Write the first-run hint into the anonymous slot and pin it so it
        // persists until the user's first action replaces it.
        status.set(Instant::now(), None, StatusTone::Info, initial_status);
        status.pin();
        if let Some(message) = theme_warning {
            status.set(Instant::now(), None, StatusTone::Warning, message);
        }
        let gh_integration_val = config.ui.github_integration;
        let config_writer =
            dux_core::config_queue::ConfigWriteQueue::new(paths.config_path.clone());
        let engine = Engine {
            config,
            paths,
            session_store,
            projects,
            sessions,
            staged_files: Vec::new(),
            unstaged_files: Vec::new(),
            terminal_counter: 0,
            github_integration_enabled: gh_integration_val,
            single_instance_lock,
            surface_kind: dux_core::term_identity::SurfaceKind::Tui,
            resource_collector: Default::default(),
            host_env: dux_core::term_identity::HostEnvProbe::from_env(),
            worker_tx,
            worker_rx,
            config_writer,
            surface: Box::new(crate::TuiConfigSurface),
            reloading: false,
            deferred_commands: Vec::new(),
            reload_guard: None,
            providers: HashMap::new(),
            running_provider_pins: HashMap::new(),
            companion_terminals: HashMap::new(),
            agent_tabs: agent_tabs.into_iter().map(|t| (t.id.clone(), t)).collect(),
            terminating_ptys: Vec::new(),
            pending_group_removals: Vec::new(),
            gh_status: crate::model::GhStatus::Unknown,
            pr_statuses: HashMap::new(),
            branch_sync_sessions,
            pr_sync_sessions,
            pr_sync_enabled,
            pr_poll_interval_secs: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            pr_backoff: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            refs_watcher: None,
            refs_watch_paths: HashMap::new(),
            resume_fallback_candidates: HashMap::new(),
            pending_deletions: HashSet::new(),
            closing_sessions: HashSet::new(),
            deletion_busy_messages: HashMap::new(),
            watched_worktree: Arc::clone(&watched_worktree),
            watched_session_id: None,
            has_active_processes,
            current_origin: dux_core::statusline::StatusScope::All,
            in_flight: HashSet::new(),
            rename_expected: std::collections::HashMap::new(),
            pr_last_checked: HashMap::new(),
            changed_files_poller_started: AtomicBool::new(false),
            branch_sync_worker_started: AtomicBool::new(false),
            pty_activity: HashMap::new(),
            pty_input: HashMap::new(),
            needs_attention: HashSet::new(),
            pty_progress: HashMap::new(),
            agent_viewed: HashMap::new(),
            last_foreground_refresh: None,
            pending_web_checkout_ops: HashMap::new(),
            pending_web_add_project_ops: HashMap::new(),
            pending_web_pr_lookup_ops: HashMap::new(),
            pending_delete_ops_web: HashMap::new(),
            pending_create_ops: HashMap::new(),
            pending_web_launch_ops: HashMap::new(),
            last_created_op_id: None,
            created_session_by_op: HashMap::new(),
        };
        Self::assemble(
            engine,
            bindings,
            interactive_patterns,
            signals,
            status,
            theme,
            SessionRestore::Restore,
        )
    }

    /// Shared App-struct construction used by both first-boot bootstrap and the
    /// post-server resume. The caller supplies the already-built `engine` plus
    /// the values that cannot be re-derived purely from `engine.config`
    /// (`status` may carry a theme warning; `sigwinch_flag` is a live handler
    /// registration). Everything else is derived here so bootstrap and resume
    /// share one body. `restore` gates whether prior sessions are relaunched:
    /// first boot restores them; resume skips restoration because the providers
    /// handed back from the web server are already live.
    fn assemble(
        engine: Engine,
        bindings: RuntimeBindings,
        interactive_patterns: InteractiveBytePatterns,
        signals: SignalHandles,
        status: KeyedStatusController,
        theme: Theme,
        restore: SessionRestore,
    ) -> Result<Self> {
        let pr_banner_at_bottom = engine.config.ui.pr_banner_position == "bottom";
        let show_diff_line_numbers = engine.config.ui.show_diff_line_numbers;
        // Seed the changes (right) pane's hidden state from config; the runtime
        // RemoveGitPane toggle (Ctrl-]) overrides it for the rest of the session.
        let right_hidden = !engine.config.ui.show_changes_pane;
        let left_width_pct = engine.config.ui.left_width_pct;
        let right_width_pct = engine.config.ui.right_width_pct;
        let terminal_pane_height_pct = engine.config.ui.terminal_pane_height_pct;
        let staged_pane_height_pct = engine.config.ui.staged_pane_height_pct;
        let commit_pane_height_pct = engine.config.ui.commit_pane_height_pct;
        let mut app = Self {
            show_diff_line_numbers,
            left_width_pct,
            right_width_pct,
            terminal_pane_height_pct,
            staged_pane_height_pct,
            commit_pane_height_pct,
            bindings,
            engine,
            selected_left: 0,
            left_section: LeftSection::Projects,
            selected_terminal_index: 0,
            right_section: RightSection::Unstaged,
            files_index: 0,
            files_search: TextInput::new(),
            files_search_active: false,
            commit_input: TextInput::new()
                .with_multiline(4)
                .with_placeholder("Type your commit message\u{2026}"),
            left_collapsed: false,
            right_collapsed: false,
            right_hidden,
            focus: FocusPane::Left,
            center_mode: CenterMode::Agent,
            resize_mode: false,
            help_scroll: None,
            last_help_height: 0,
            last_help_lines: 0,
            fullscreen_overlay: FullscreenOverlay::None,
            startup_log_viewer: None,
            status,
            prompt: PromptState::None,
            input_target: InputTarget::None,
            session_surface: SessionSurface::Agent,
            clipboard: Clipboard::new(),
            active_terminal_id: None,
            focused_tabs: HashMap::new(),
            host_forward_carry: Vec::new(),
            host_forward_error_logged_at: None,
            agent_tab_regions: Vec::new(),
            agent_tab_add_region: None,
            terminal_return_to_list: false,
            last_pty_size: (0, 0),
            prev_scrollback_offset: 0,
            last_diff_height: 0,
            last_diff_visual_lines: 0,
            theme,
            tick_count: 0,
            start_time: Instant::now(),
            readonly_nudge_tick: None,
            collapsed_projects: HashSet::new(),
            left_items_cache: Vec::new(),
            mouse_layout: MouseLayoutState::default(),
            overlay_layout: OverlayMouseLayoutState::default(),
            mouse_drag: None,
            last_mouse_click: None,
            pressed_button: None,
            interactive_patterns,
            raw_input_parser: crate::raw_input::RawInputParser::default(),
            raw_input_buf: Vec::new(),
            loading_input_buf: Vec::new(),
            in_bracket_paste: false,
            terminal_focus: crate::focus::TerminalFocus::new(),
            macro_bar: None,
            sigwinch_flag: signals.sigwinch_flag,
            sigwinch_sig_id: signals.sigwinch_sig_id,
            shutdown_flag: signals.shutdown_flag,
            shutdown_sig_ids: signals.shutdown_sig_ids,
            force_redraw: false,
            welcome_tip_index: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as usize)
                .unwrap_or(0),
            welcome_logo_visible: false,
            welcome_logo_alt: false,
            welcome_tip_selection: usize::MAX,
            pr_banner_at_bottom,
            syntax_cache: SyntaxCache::new(),
            snapshot_buf: TerminalSnapshot::empty(),
            last_snapshot_id: None,
            terminal_selection: None,
            startup_log_selection: None,
            pending_server_flip: None,
            server_flip_preflight_pending: false,
            pending_persist_ops: HashMap::new(),
            pending_worktree_ops: HashMap::new(),
            pending_pr_lookup_ops: HashMap::new(),
            pending_delete_ops: HashMap::new(),
            pending_reconnect_ops: HashMap::new(),
            pending_checkout_inspect_ops: HashMap::new(),
            pending_server_flip_op: None,
            pending_config_reload_op: None,
        };
        // First boot relaunches prior sessions; a resume must not — the engine
        // handed back from the web server already owns the live providers, and
        // any session the user closed in the web UI must stay closed.
        if matches!(restore, SessionRestore::Restore) {
            app.restore_sessions();
        }
        app.seed_pr_statuses_from_db();
        app.rebuild_left_items();
        app.reload_changed_files();
        app.engine.update_branch_sync_sessions();
        Ok(app)
    }

    /// Rebuild an App around an EXISTING engine after the web server hands it
    /// back. The engine's providers are live (PTYs never stopped across the
    /// flip), so session restoration is skipped; only view state is rebuilt.
    /// Keybindings, the interactive byte patterns, and the theme are re-derived
    /// from `engine.config` exactly as bootstrap does, and a fresh SIGWINCH
    /// handler is registered; the previous App's registration was removed in
    /// `into_engine`, so flip cycles don't accumulate handlers.
    pub fn resume(engine: Engine) -> Result<Self> {
        logger::info("resuming dux TUI after the web server stopped");
        let bindings = RuntimeBindings::from_keys_config(&engine.config.keys);
        let interactive_patterns = bindings.interactive_byte_patterns();
        // A fresh App means fresh handler registrations; the previous App's were
        // removed in `into_engine`, so flip cycles don't accumulate handlers.
        let signals = register_signal_handles()?;
        let (theme, theme_warning) =
            crate::theme::load_or_fallback(&engine.config.ui.theme, &engine.paths);
        let mut status = KeyedStatusController::with_clear_after(Duration::from_secs(
            engine.config.ui.status_clear_seconds as u64,
        ));
        // Write the post-flip guidance into the anonymous slot and pin it so it
        // persists until the user acts, not auto-clear like a confirmation.
        status.set(
            Instant::now(),
            None,
            StatusTone::Info,
            "Web server stopped. Your agents kept running — reconnect to any session to pick up where it left off.",
        );
        status.pin();
        if let Some(message) = theme_warning {
            status.set(Instant::now(), None, StatusTone::Warning, message);
        }
        Self::assemble(
            engine,
            bindings,
            interactive_patterns,
            signals,
            status,
            theme,
            SessionRestore::Skip,
        )
    }

    /// Consume the App and hand back its engine. Used by the TUI→server flip:
    /// the providers (PTYs) and the single-instance lock live in the engine and
    /// must survive the flip, so the engine is moved out wholesale. Neither
    /// `App` nor `Engine` implements `Drop`, so nothing is torn down here.
    /// PTY-activity tracking now lives on the engine (`pty_activity`), so the
    /// streaming/"working" state carries across the flip automatically with it.
    pub fn into_engine(self) -> Engine {
        // Unregister this App's signal handlers so repeated flip cycles don't
        // pile up orphaned registrations (each resume registers fresh flags;
        // without this, every signal would fire one stale setter per cycle) and
        // so the TUI's shutdown handlers don't fire alongside the server's own
        // once the engine is handed over.
        if let Some(sig_id) = self.sigwinch_sig_id {
            signal_hook::low_level::unregister(sig_id);
        }
        for sig_id in self.shutdown_sig_ids {
            signal_hook::low_level::unregister(sig_id);
        }
        self.engine
    }

    pub fn run(&mut self) -> Result<RunExit> {
        self.engine.spawn_changed_files_poller();
        self.engine.spawn_branch_sync_worker();
        self.engine.spawn_project_branch_status_checks();
        self.engine.spawn_gh_status_check();
        let mut terminal = ratatui::init();
        // EnableFocusChange (DEC mode 1004) so the host reports window focus,
        // gating the per-tick "viewed" stamp. Enabled/disabled symmetrically
        // with mouse capture at every terminal-mode lifecycle site below.
        execute!(stdout(), EnableMouseCapture, EnableFocusChange)?;

        let result: RunExit = {
            'main: loop {
                // A SIGTERM/SIGINT/SIGHUP arrived (e.g. the terminal closed, a
                // system shutdown, or `kill`): quit cleanly so the teardown below
                // SIGTERMs the agents and gives them a grace window, instead of
                // letting the process die straight to the hard SIGKILL on drop.
                if self.shutdown_flag.load(Ordering::Relaxed) {
                    break 'main RunExit::Quit;
                }

                self.drain_events();
                self.engine.poll_pty_activity();
                // Drain attention/progress signals: keeps the "working" override
                // truthful and maintains the per-tab attention flag. Must run
                // after `poll_pty_activity` so a progress report and the activity
                // it also produced are observed in the same tick. Stamp the tab
                // the user is looking at first so the poll suppresses/clears it.
                self.note_focused_agent_viewed();
                self.engine.poll_agent_signals();
                self.tick_count = self.tick_count.wrapping_add(1);
                // Expire a transient status (e.g. a success confirmation) after
                // its configured lifetime. Busy entries older than BUSY_TIMEOUT
                // are upgraded to Warning. Wall-clock, not tick count.
                self.status.tick(Instant::now(), BUSY_TIMEOUT);

                // Check SIGWINCH — needed when bypassing crossterm's event
                // reader (which would otherwise deliver Resize events).
                if self.sigwinch_flag.swap(false, Ordering::Relaxed)
                    && let Err(err) = crate::io_retry::retry_on_interrupt(|| terminal.autoresize())
                {
                    self.report_runtime_error("terminal resize failed", &err);
                }

                if self.force_redraw {
                    self.force_redraw = false;
                    if let Err(err) = terminal.clear() {
                        self.report_runtime_error("force redraw failed", &err);
                    }
                    // Re-enable mouse capture and focus reporting:
                    // terminal.clear() resets terminal state which drops both.
                    let _ = execute!(stdout(), EnableMouseCapture, EnableFocusChange);
                }

                if let Err(err) = terminal.draw(|frame| self.render(frame)) {
                    self.report_runtime_error("terminal draw failed", &err);
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }

                // Forward any captured agent passthrough sequences (notifications,
                // progress, clipboard writes) to the host terminal. Done AFTER a
                // successful draw and only here on the single-threaded run loop, so
                // nothing interleaves mid-frame. Every whitelisted sequence was
                // validated control-free at capture (C0 bytes are rejected, see
                // attention.rs), so a forwarded sequence is non-printing and
                // non-positional and cannot corrupt the frame.
                let focused_tab = if matches!(self.center_mode, CenterMode::Agent)
                    && self.active_terminal_id.is_none()
                {
                    self.selected_session().map(|s| self.focused_tab_id(&s.id))
                } else {
                    None
                };
                let under_tmux = self.engine.host_under_tmux();
                // Prepend any bytes carried over from a previous tick's cap, then
                // append this tick's fresh capture.
                let mut fwd = std::mem::take(&mut self.host_forward_carry);
                fwd.extend_from_slice(
                    &self
                        .engine
                        .take_host_passthrough(focused_tab.as_deref(), under_tmux),
                );
                if !fwd.is_empty() {
                    // Bound the bytes written this tick so a large burst cannot
                    // stall the run loop on one write_all; the remainder rides the
                    // carry to the next tick. Splitting at the cap is safe because
                    // the host stdout is a single continuous byte stream.
                    if fwd.len() > HOST_FORWARD_MAX_PER_TICK {
                        self.host_forward_carry = fwd.split_off(HOST_FORWARD_MAX_PER_TICK);
                    }
                    let mut out = stdout();
                    if let Err(err) = out.write_all(&fwd).and_then(|()| out.flush()) {
                        // A failing host stdout is unusual and would otherwise be
                        // silently swallowed; log it, throttled so a persistent
                        // failure does not spam once per tick.
                        let now = Instant::now();
                        let should_log = self
                            .host_forward_error_logged_at
                            .is_none_or(|at| at.elapsed() >= HOST_FORWARD_ERROR_LOG_INTERVAL);
                        if should_log {
                            self.host_forward_error_logged_at = Some(now);
                            logger::warn(&format!(
                                "failed to forward agent passthrough sequences to the host \
                                 terminal: {err}"
                            ));
                        }
                    }
                }

                // The `StartWebServer` palette action stashes a pre-bound
                // listener (its pre-flight already succeeded) and we break here,
                // after one more draw so the "Starting the web server…" Busy
                // status is visible for the brief remainder. Teardown below runs
                // identically to the quit path, then the binary takes over.
                if let Some((listeners, urls)) = self.pending_server_flip.take() {
                    break 'main RunExit::FlipToServer { listeners, urls };
                }

                if self.should_poll_raw_input() {
                    // Interactive mode: read raw stdin and forward to PTY.
                    // crossterm's event reader is not called — all bytes
                    // (keyboard, mouse, paste) go to the child process
                    // except intercepted bindings.
                    let should_exit = match self.poll_and_forward_raw_input() {
                        Ok(should_exit) => should_exit,
                        Err(err) => {
                            self.report_runtime_error(
                                "interactive input failed; staying in the current session",
                                err.as_ref(),
                            );
                            false
                        }
                    };
                    if should_exit {
                        break 'main RunExit::Quit;
                    }
                } else {
                    // Normal UI mode: use crossterm's structured event reader.
                    // Block up to 100ms for the first event, then drain any
                    // remaining queued events before rendering so that
                    // intermediate events (Up, scroll, etc.) don't each cost
                    // a full render cycle.  This keeps double-click timestamps
                    // close to wall-clock time.
                    let ready = match crate::io_retry::retry_on_interrupt(|| {
                        event::poll(Duration::from_millis(100))
                    }) {
                        Ok(ready) => ready,
                        Err(err) => {
                            self.report_runtime_error(
                                "event polling failed; input handling was skipped",
                                &err,
                            );
                            false
                        }
                    };
                    if ready {
                        let mut should_exit = false;
                        loop {
                            let event = match crate::io_retry::retry_on_interrupt(event::read) {
                                Ok(event) => event,
                                Err(err) => {
                                    self.report_runtime_error(
                                        "event read failed; input handling was skipped",
                                        &err,
                                    );
                                    break;
                                }
                            };
                            match event {
                                Event::Key(key) => {
                                    should_exit = match self.handle_key(key) {
                                        Ok(exit) => exit,
                                        Err(err) => {
                                            self.report_runtime_error(
                                                "key handling failed",
                                                err.as_ref(),
                                            );
                                            false
                                        }
                                    };
                                }
                                Event::Mouse(mouse) => {
                                    should_exit = self.handle_mouse(mouse);
                                }
                                Event::Resize(_, _) => {}
                                Event::FocusGained => {
                                    self.terminal_focus.on_focus_gained(Instant::now())
                                }
                                Event::FocusLost => self.terminal_focus.on_focus_lost(),
                                _ => {}
                            }

                            // Stop draining if exit was requested.
                            if should_exit {
                                break;
                            }

                            // Stop draining if we switched to interactive
                            // mode — remaining events must go through the
                            // raw stdin path.
                            if matches!(
                                self.input_target,
                                InputTarget::Agent | InputTarget::Terminal
                            ) {
                                break;
                            }

                            // Check for more queued events without blocking.
                            match crate::io_retry::retry_on_interrupt(|| {
                                event::poll(Duration::ZERO)
                            }) {
                                Ok(true) => continue,
                                _ => break,
                            }
                        }
                        if should_exit {
                            break 'main RunExit::Quit;
                        }
                    }
                }
            }
        };

        let _ = execute!(stdout(), DisableMouseCapture, DisableFocusChange);
        ratatui::restore();

        // On a real quit, wind the agents and companion terminals down
        // gracefully: SIGTERM each child and wait briefly so it can save state
        // for a later resume before `PtyClient::drop` hard-kills any straggler.
        // On a flip the engine (and its live PTYs) is handed to the server, so
        // it must NOT be touched here; `into_engine` moves it out intact.
        if matches!(result, RunExit::Quit) {
            self.shutdown_agents_gracefully();
        }
        Ok(result)
    }

    /// SIGTERM every running agent/terminal PTY and wait up to the configured
    /// `shutdown_timeout_seconds` for them to exit, the TUI analogue of the
    /// server's shutdown path. Runs after the terminal is restored, so the user
    /// is back at their shell while the wind-down happens. Echoes the same start
    /// and result lines `shutdown_ptys` logs to `dux.log`, but only when there is
    /// something to wait for — an agent-less quit stays silent.
    ///
    /// A second SIGINT/SIGTERM during the wait cuts it short: the run loop (the
    /// only thing that polled `shutdown_flag`) has already exited, so we clear the
    /// flag and hand it to `shutdown_ptys_interruptible` as an abort. Without this
    /// a child that ignores SIGTERM would trap the operator for the full grace
    /// (now up to the configured timeout) with only `kill -9` as an out.
    fn shutdown_agents_gracefully(&mut self) {
        let agents = self.engine.providers.len();
        let terminals = self.engine.companion_terminals.len();
        if agents + terminals == 0 {
            return;
        }
        let grace = dux_core::config::shutdown_grace(self.engine.config.shutdown_timeout_seconds);
        eprintln!(
            "{}",
            dux_core::engine::format_shutdown_start(agents, terminals, grace)
        );
        // Consume the signal that may have triggered this quit, then watch for a
        // fresh one during the wait.
        self.shutdown_flag
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let report = self
            .engine
            .shutdown_ptys_interruptible(grace, Some(&self.shutdown_flag));
        eprintln!("{}", dux_core::engine::format_shutdown_result(&report));
    }

    fn should_poll_raw_input(&self) -> bool {
        matches!(self.prompt, PromptState::None)
            && !matches!(self.fullscreen_overlay, FullscreenOverlay::StartupLog)
            && matches!(
                self.input_target,
                InputTarget::Agent | InputTarget::Terminal
            )
    }

    fn restore_sessions(&mut self) {
        logger::info(&format!(
            "restoring {} persisted sessions",
            self.engine.sessions.len()
        ));
        let ids: Vec<(String, bool)> = self
            .engine
            .sessions
            .iter()
            .map(|s| (s.id.clone(), Path::new(&s.worktree_path).exists()))
            .collect();
        for (id, exists) in ids {
            if exists {
                self.engine
                    .mark_session_status(&id, SessionStatus::Detached);
            } else {
                self.engine.mark_session_status(&id, SessionStatus::Exited);
            }
        }
        self.auto_reopen_eligible_sessions();
    }

    fn auto_reopen_eligible_sessions(&mut self) {
        if !self.engine.config.ui.auto_reopen_agents {
            return;
        }

        let sessions = self.engine.sessions.clone();
        for session in sessions {
            if !session.desired_running
                || !session.auto_reopen_enabled
                || !Path::new(&session.worktree_path).exists()
                || !self.engine.project_allows_auto_reopen(&session.project_id)
            {
                continue;
            }

            let cfg = provider_config(&self.engine.config, &session.provider);
            if !cfg.supports_session_resume() {
                continue;
            }

            let request =
                self.agent_launch_request(session, true, AgentLaunchKind::StartupAutoReopen);
            self.dispatch_agent_launch(request);
        }
    }

    /// Populate the in-memory PR status map from the database so the UI shows
    /// PR state immediately on startup, before the first background poll.
    fn seed_pr_statuses_from_db(&mut self) {
        if !self.engine.github_integration_enabled {
            return;
        }
        let stored = self
            .engine
            .session_store
            .load_all_latest_prs()
            .unwrap_or_default();
        for pr in stored {
            use crate::model::{PrInfo, PrState};
            let state = match pr.state.as_str() {
                "OPEN" => PrState::Open,
                "MERGED" => PrState::Merged,
                "CLOSED" => PrState::Closed,
                _ => continue,
            };
            self.engine.pr_statuses.insert(
                pr.session_id,
                PrInfo {
                    number: pr.pr_number,
                    state,
                    title: pr.title,
                    host: pr.host,
                    owner_repo: pr.owner_repo,
                    url: pr.url,
                },
            );
        }
        if !self.engine.pr_statuses.is_empty() {
            logger::info(&format!(
                "[gh-integration] seeded {} PR statuses from database",
                self.engine.pr_statuses.len(),
            ));
        }
    }

    pub(crate) fn close_top_overlay(&mut self) -> bool {
        if matches!(self.fullscreen_overlay, FullscreenOverlay::Terminal) {
            let return_to_list = self.terminal_return_to_list;
            self.fullscreen_overlay = FullscreenOverlay::None;
            self.session_surface = SessionSurface::Agent;
            self.input_target = InputTarget::None;
            if return_to_list {
                self.left_section = LeftSection::Terminals;
                self.clamp_terminal_cursor();
                self.focus = FocusPane::Left;
            }
            let key = self.bindings.label_for(Action::ToggleFullscreen);
            self.set_info(format!(
                "Closed fullscreen terminal. Press {key} to reopen."
            ));
            return true;
        }
        if matches!(self.fullscreen_overlay, FullscreenOverlay::StartupLog) {
            self.fullscreen_overlay = FullscreenOverlay::None;
            self.startup_log_viewer = None;
            self.terminal_selection = None;
            self.startup_log_selection = None;
            self.set_info("Closed startup command log.");
            return true;
        }
        if !matches!(self.prompt, PromptState::None) {
            self.prompt = PromptState::None;
            self.set_info("Dismissed dialog. Resume your work in the current pane.");
            return true;
        }
        if self.help_scroll.is_some() {
            self.help_scroll = None;
            let key = self.bindings.label_for(Action::ToggleHelp);
            self.set_info(format!("Closed help overlay. Press {key} to reopen."));
            return true;
        }
        if matches!(self.center_mode, CenterMode::Diff { .. }) {
            self.center_mode = CenterMode::Agent;
            self.focus = FocusPane::Files;
            self.set_info("Closed diff view, returned to agent output.");
            return true;
        }
        false
    }

    /// Closes the diff overlay if one is open, leaving other state (focus,
    /// input target, status line) untouched. Called when the left-pane
    /// selection moves to a different item so the middle pane falls back
    /// to the newly-selected agent's terminal. Silent by design: the user
    /// moved a cursor, they did not dismiss a dialog.
    pub(crate) fn close_diff_view(&mut self) {
        if matches!(self.center_mode, CenterMode::Diff { .. }) {
            self.center_mode = CenterMode::Agent;
        }
    }

    /// Returns the current braille spinner frame index based on wall-clock
    /// time (80ms per frame). Unlike `tick_count`, this stays constant-speed
    /// regardless of event loop frequency.
    pub(crate) fn spinner_frame_index(&self) -> usize {
        ((self.start_time.elapsed().as_millis() / 80) as usize) % crate::theme::SPINNER_FRAMES.len()
    }

    /// Whether the sidebar attention glyph is visible at this point of its
    /// blink cycle. Wall-clock based (like the spinner) so the blink cadence
    /// stays constant regardless of event-loop frequency, per the "animations
    /// use wall-clock time" tenet.
    pub(crate) fn attention_blink_on(&self) -> bool {
        attention_blink_phase(self.start_time.elapsed().as_millis())
    }

    pub(crate) fn set_info(&mut self, message: impl Into<String>) {
        self.status
            .set(Instant::now(), None, StatusTone::Info, message);
    }

    /// Set an anonymous (unkeyed) `Busy` status. Every production busy now rides a
    /// keyed [`dux_core::engine::HandlerStatusOp`] (or the engine's own keyed
    /// status), so this remains only as a test helper that simulates an unrelated
    /// busy already on screen.
    #[cfg(test)]
    pub(crate) fn set_busy(&mut self, message: impl Into<String>) {
        self.status
            .set(Instant::now(), None, StatusTone::Busy, message);
    }

    pub(crate) fn set_warning(&mut self, message: impl Into<String>) {
        self.status
            .set(Instant::now(), None, StatusTone::Warning, message);
    }

    pub(crate) fn set_error(&mut self, message: impl Into<String>) {
        self.status
            .set(Instant::now(), None, StatusTone::Error, message);
    }

    /// Show a status-line warning when a missing project is highlighted, or
    /// clear the warning when the selection moves away from one.
    pub(crate) fn update_missing_project_warning(&mut self) {
        let missing_path = self
            .left_items()
            .get(self.selected_left)
            .copied()
            .and_then(|item| match item {
                LeftItem::Project(idx) => {
                    let p = self.engine.projects.get(idx)?;
                    p.path_missing.then(|| p.path.clone())
                }
                _ => None,
            });
        if let Some(path) = missing_path {
            self.set_warning(format!("Project path not found: {path}"));
            return;
        }
        // Only clear if the current (most-recent) tone is Warning — don't clobber
        // Info/Busy/Error statuses from other operations.
        if matches!(
            self.status.most_recent_tui(),
            Some((StatusTone::Warning, _))
        ) {
            self.set_info(String::new());
        }
    }

    const NUDGE_DURATION_TICKS: u64 = 15; // ~1.5s at 100ms/tick

    pub(crate) fn is_nudge_active(&self) -> bool {
        self.readonly_nudge_tick
            .is_some_and(|t| self.tick_count.wrapping_sub(t) < Self::NUDGE_DURATION_TICKS)
    }

    fn report_runtime_error(&mut self, context: &str, err: &dyn std::error::Error) {
        logger::error(&format!("{context}: {err}"));
        self.set_error(format!("{context}: {err}"));
    }

    pub(crate) fn open_resource_monitor(&mut self) {
        self.prompt = PromptState::ResourceMonitor {
            rows: Vec::new(),
            scroll_offset: 0,
            selected_row: 0,
            expanded: HashSet::new(),
            last_refresh: Instant::now(),
            short_window_sample: true,
        };
        self.engine.spawn_resource_stats_worker();
    }

    fn is_palette_action_available(&self, action: Action) -> bool {
        match action {
            Action::OpenCurrentPullRequest => self.current_pr_info().is_some(),
            _ => true,
        }
    }

    pub(crate) fn github_pr_agent_command_available(&self) -> bool {
        self.engine.pr_agent_command_available()
    }

    /// Rebuilds `config.projects` from the live project list without writing to
    /// disk. Runtime reaction sites call this then route the save through
    /// `engine.config_writer.save_eager` so the write joins the queue instead
    /// of bypassing it.
    pub(crate) fn update_config_projects_from_runtime(&mut self) {
        let existing_projects = self.engine.config.projects.clone();
        self.engine.config.projects = self
            .engine
            .projects
            .iter()
            .map(|project| runtime_project_to_config(project, &existing_projects))
            .collect();
    }

    /// Syncs all runtime projects to SQLite and rebuilds `config.projects`
    /// (stripping `leading_branch` for portability) without writing to disk.
    /// Runtime reaction sites call this then route the save through
    /// `engine.config_writer.save_eager` so the write joins the queue.
    pub(crate) fn sync_projects_to_store_and_update_config(&mut self) -> Result<()> {
        let existing_projects = self.engine.config.projects.clone();
        let stored_project_configs = self
            .engine
            .projects
            .iter()
            .map(|project| runtime_project_to_config(project, &existing_projects))
            .collect::<Vec<_>>();
        let config_project_configs = stored_project_configs
            .iter()
            .cloned()
            .map(|mut project| {
                project.leading_branch = None;
                project
            })
            .collect::<Vec<_>>();
        let stored_projects = self.engine.session_store.load_projects()?;
        for (index, project_config) in stored_project_configs.iter().enumerate() {
            let stored_project = stored_projects.iter().find(|stored| {
                stored.id == project_config.id || same_expanded_project_path(stored, project_config)
            });
            if stored_project != Some(project_config) {
                self.engine
                    .session_store
                    .upsert_project_at(project_config, index as i64)?;
            }
        }
        if self.engine.config.projects != config_project_configs {
            self.engine.config.projects = config_project_configs;
        }
        Ok(())
    }

    pub(crate) fn filtered_palette_commands(
        &self,
        input: &str,
    ) -> Vec<&crate::keybindings::RuntimeBinding> {
        self.bindings
            .filtered_palette(input)
            .into_iter()
            .filter(|binding| {
                self.is_palette_action_available(binding.action)
                    && (binding.action != Action::NewAgentFromPr
                        || self.github_pr_agent_command_available())
            })
            .collect()
    }

    pub(crate) fn execute_command(&mut self, command: String) -> Result<()> {
        let command = command.trim();
        match command {
            "new-agent" => self.create_agent_for_selected_project(),
            "new-agent-from-pr" => self.open_new_agent_from_pr_prompt(),
            "new-agent-from-worktree" => self.create_agent_from_existing_worktree(),
            "fork-agent" => self.fork_selected_session(),
            "change-agent-provider" => self.open_change_agent_provider_prompt(),
            "new-agent-tab" => self.open_new_tab_provider_prompt(),
            "close-tab" => {
                self.close_focused_tab_prompt();
                Ok(())
            }
            "change-default-provider" => self.open_change_default_provider_prompt(),
            "change-project-default-provider" => self.open_change_project_default_provider_prompt(),
            "change-theme" => self.open_change_theme_prompt(),
            "reload-config" => self.reload_config_from_disk(),
            "start-web-server" => {
                self.start_web_server();
                Ok(())
            }
            "toggle-project-auto-reopen-agents" => self.toggle_project_auto_reopen_agents(),
            "toggle-agent-auto-reopen" => self.toggle_agent_auto_reopen(),
            "configure-startup-command" => self.open_configure_startup_command(),
            "configure-global-env" => self.open_configure_global_env(),
            "configure-project-env" => self.open_configure_project_env(),
            "rerun-startup-command-on-agent" => self.rerun_startup_command_on_agent(),
            "read-startup-command-logs" => self.open_startup_command_logs(),
            "pull-project" => self.refresh_selected_project(),
            "delete-project" => self.delete_selected_project(),
            "remove-project" => self.remove_selected_project(),
            "delete-agent" => self.confirm_delete_selected_session(),
            "rename-agent" => self.open_rename_session(),
            "agent-info" => self.open_agent_info(),
            "kill-running" => self.open_kill_running(),
            "reconnect-agent" => self.reconnect_selected_session(),
            "force-reconnect-agent" => self.force_reconnect_agent(),
            "show-agent" => self.activate_center_agent(true),
            "show-terminal" => self.show_or_open_first_terminal(),
            "new-terminal" => self.new_companion_terminal(),
            "add-project" => self.open_project_browser(),
            "copy-path" => self.copy_selected_path(),
            "open-worktree" => self.open_selected_worktree_in_default_editor(),
            "open-worktree-with" => self.open_worktree_editor_picker(),
            "open-current-pr" => self.open_current_pr_in_browser(),
            "toggle-project" => {
                self.toggle_collapse_selected_project();
                Ok(())
            }
            "toggle-sidebar" => {
                self.left_collapsed = !self.left_collapsed;
                Ok(())
            }
            "toggle-git-pane" => {
                self.right_collapsed = !self.right_collapsed;
                if self.right_collapsed && self.focus == FocusPane::Files {
                    self.focus = FocusPane::Center;
                }
                Ok(())
            }
            "toggle-remove-git-pane" => {
                self.toggle_git_pane_removed();
                Ok(())
            }
            "toggle-always-show-tabs" => {
                self.toggle_always_show_tab_strip();
                Ok(())
            }
            "help" => {
                self.help_scroll = Some(0);
                Ok(())
            }
            "sort-agents-by-updated" => {
                self.sort_sessions_by_updated();
                Ok(())
            }
            "sort-agents-by-created" => {
                self.sort_sessions_by_created();
                Ok(())
            }
            "sort-agents-by-name" => {
                self.sort_sessions_by_name();
                Ok(())
            }
            "edit-macros" => {
                self.open_edit_macros();
                Ok(())
            }
            "input-debugging" => {
                self.prompt = PromptState::DebugInput {
                    lines: Vec::new(),
                    scroll_offset: 0,
                };
                Ok(())
            }
            "resource-monitor" => {
                self.open_resource_monitor();
                Ok(())
            }
            "toggle-diff-line-numbers" => {
                self.show_diff_line_numbers = !self.show_diff_line_numbers;
                self.engine.config.ui.show_diff_line_numbers = self.show_diff_line_numbers;
                self.engine
                    .config_writer
                    .save_lazy(self.engine.config.clone());
                let _ = self.refresh_current_diff();
                let state = if self.show_diff_line_numbers {
                    "enabled"
                } else {
                    "disabled"
                };
                let palette_key = self.bindings.label_for(Action::OpenPalette);
                self.set_info(format!(
                    "Diff line numbers {state}. Press {palette_key} to open the palette and toggle back."
                ));
                Ok(())
            }
            "toggle-github-integration" => {
                self.engine.github_integration_enabled = !self.engine.github_integration_enabled;
                self.engine.config.ui.github_integration = self.engine.github_integration_enabled;
                if self.engine.github_integration_enabled
                    && matches!(self.engine.gh_status, crate::model::GhStatus::Available)
                {
                    self.engine.update_pr_sync_sessions();
                    self.engine.spawn_initial_pr_refresh();
                    self.engine.pr_sync_enabled.store(true, Ordering::Relaxed);
                } else if !self.engine.github_integration_enabled {
                    self.engine.pr_statuses.clear();
                    self.engine.pr_sync_enabled.store(false, Ordering::Relaxed);
                    self.rebuild_left_items();
                }
                let state = if self.engine.github_integration_enabled {
                    "enabled"
                } else {
                    "disabled"
                };
                if let Err(err) = self
                    .engine
                    .config_writer
                    .save_eager(self.engine.config.clone())
                {
                    self.set_error(format!(
                        "GitHub integration toggled this session, but saving to config failed: {err}"
                    ));
                } else {
                    self.set_info(format!("GitHub integration {state}."));
                }
                Ok(())
            }
            "toggle-randomized-pet-name-default" => {
                self.engine
                    .config
                    .defaults
                    .enable_randomized_pet_name_by_default = !self
                    .engine
                    .config
                    .defaults
                    .enable_randomized_pet_name_by_default;
                self.engine
                    .config_writer
                    .save_lazy(self.engine.config.clone());
                let state = if self
                    .engine
                    .config
                    .defaults
                    .enable_randomized_pet_name_by_default
                {
                    "enabled — new agent prompts start with a random pet name"
                } else {
                    "disabled — new agent prompts start empty"
                };
                let palette_key = self.bindings.label_for(Action::OpenPalette);
                self.set_info(format!(
                    "Random pet-name defaults {state}. Press {palette_key} to toggle back."
                ));
                Ok(())
            }
            "toggle-pr-banner-position" => {
                self.pr_banner_at_bottom = !self.pr_banner_at_bottom;
                let pos = if self.pr_banner_at_bottom {
                    "bottom"
                } else {
                    "top"
                };
                self.engine.config.ui.pr_banner_position = pos.to_string();
                self.engine
                    .config_writer
                    .save_lazy(self.engine.config.clone());
                self.set_info(format!("PR banner moved to {pos} of agent pane."));
                Ok(())
            }
            "force-redraw" => {
                self.force_redraw = true;
                self.set_info("Interface redrawn. All screen contents have been repainted.");
                Ok(())
            }
            "" => Ok(()),
            other => {
                self.set_error(format!("Unknown command: \"{other}\""));
                Ok(())
            }
        }
    }

    pub(crate) fn reload_config_from_disk(&mut self) -> Result<()> {
        let reaction = self.engine.apply(Command::ReloadConfig)?;
        // Only show the "Reloading…" busy when a reload worker was actually
        // spawned (the engine returns `Nothing` on that path). The early-return
        // cases — a reentrant reload or a busy config writer — return a `Status`
        // that already explains the situation; setting the busy here would both
        // clobber that message and strand a spinner that no worker will clear.
        let spawned = matches!(reaction, dux_core::engine::EventReaction::Nothing);
        self.apply_reaction(reaction);
        if spawned {
            // Mint the reload's keyed busy op. The TUI view handler for the shared
            // `ApplyReloadedConfig` (success) / `OpenConfigReloadFailedModal`
            // (failure) reactions resolves it into the keyed final, REPLACING the
            // legacy `set_info`/`set_error` (byte-identical messages).
            let op = dux_core::engine::status_op("Reloading config.toml.").resolve_in_handler(
                |o: &TuiConfigReloadOutcome| match o {
                    TuiConfigReloadOutcome::Applied => dux_core::engine::Final::info(
                        "Configuration reloaded. New settings are active now.",
                    ),
                    TuiConfigReloadOutcome::ApplyFailed(err) => dux_core::engine::Final::error(
                        format!("Config validation passed, but applying it failed: {err}"),
                    ),
                    TuiConfigReloadOutcome::ValidationFailed => dux_core::engine::Final::error(
                        "Config reload failed. Review the modal before retrying.",
                    ),
                },
            );
            self.apply_reaction(dux_core::engine::EventReaction::Status(op.pending_status()));
            self.pending_config_reload_op = Some(op);
        }
        Ok(())
    }

    fn open_config_reload_failed_modal(&mut self, error: String) {
        self.prompt = PromptState::ConfigReloadFailed {
            error,
            recover_old_config: false,
            focus: ConfigReloadFailedFocus::Close,
        };
    }

    fn apply_reloaded_config(&mut self, mut config: Config) -> Result<()> {
        let bindings = RuntimeBindings::from_keys_config(&config.keys);
        self.interactive_patterns = bindings.interactive_byte_patterns();
        self.bindings = bindings;

        let (theme, theme_warning) =
            crate::theme::load_or_fallback(&config.ui.theme, &self.engine.paths);
        self.theme = theme;
        self.show_diff_line_numbers = config.ui.show_diff_line_numbers;
        self.left_width_pct = config.ui.left_width_pct;
        self.right_width_pct = config.ui.right_width_pct;
        self.terminal_pane_height_pct = config.ui.terminal_pane_height_pct;
        self.staged_pane_height_pct = config.ui.staged_pane_height_pct;
        self.commit_pane_height_pct = config.ui.commit_pane_height_pct;
        self.engine.github_integration_enabled = config.ui.github_integration;
        self.pr_banner_at_bottom = config.ui.pr_banner_position == "bottom";
        // Re-seed the changes (right) pane's hidden state from the reloaded
        // config, mirroring startup; if it just became hidden while the Files
        // pane was focused, move focus to the center (matching the toggle).
        self.right_hidden = !config.ui.show_changes_pane;
        if self.right_hidden && self.focus == FocusPane::Files {
            self.focus = FocusPane::Center;
        }
        self.engine.projects = load_projects(
            &self.engine.session_store.load_projects()?,
            &self.engine.session_store.load_project_created_ats()?,
            &config,
        );
        persist_runtime_projects_to_config_and_store(
            &self.engine.projects,
            &mut config,
            &self.engine.paths,
            &self.bindings,
            &self.engine.session_store,
        )?;
        self.engine.config = config;

        self.engine.refresh_project_defaults();
        self.selected_left = self
            .selected_left
            .min(self.engine.projects.len().saturating_sub(1));
        self.rebuild_left_items();
        if self.selected_left >= self.left_items_cache.len() {
            self.selected_left = self.left_items_cache.len().saturating_sub(1);
        }
        self.engine.update_branch_sync_sessions();
        if self.engine.github_integration_enabled
            && matches!(self.engine.gh_status, crate::model::GhStatus::Available)
        {
            self.engine.update_pr_sync_sessions();
            self.engine.spawn_initial_pr_refresh();
            self.engine.pr_sync_enabled.store(true, Ordering::Relaxed);
        } else {
            self.engine.pr_statuses.clear();
            self.engine.pr_sync_enabled.store(false, Ordering::Relaxed);
        }
        self.reload_changed_files();
        self.refresh_current_diff()?;
        if let Some(message) = theme_warning {
            self.set_warning(message);
        }
        Ok(())
    }

    pub(crate) fn open_edit_macros(&mut self) {
        let entries: Vec<(String, String, MacroSurface)> = self
            .engine
            .config
            .macros
            .entries
            .iter()
            .map(|(k, v)| (k.clone(), v.text.clone(), v.surface))
            .collect();
        // Preserve declaration order from config file (IndexMap iteration order).
        self.prompt = PromptState::EditMacros {
            entries,
            selected: 0,
            editing: None,
            pending_delete: None,
        };
    }

    /// Return macros matching `query` and the current session surface,
    /// searching name first then text content.
    /// If `query` is empty, returns all surface-matching macros in config order.
    pub(crate) fn filtered_macros(&self, query: &str) -> Vec<(&str, &str)> {
        let surface = self.session_surface;
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return self
                .engine
                .config
                .macros
                .entries
                .iter()
                .filter(|(_, entry)| {
                    dux_core::macros::macro_matches_surface(entry.surface, surface)
                })
                .map(|(name, entry)| (name.as_str(), entry.text.as_str()))
                .collect();
        }
        let mut name_matches = Vec::new();
        let mut text_matches = Vec::new();
        for (name, entry) in &self.engine.config.macros.entries {
            if !dux_core::macros::macro_matches_surface(entry.surface, surface) {
                continue;
            }
            if name.to_lowercase().contains(&needle) {
                name_matches.push((name.as_str(), entry.text.as_str()));
            } else if entry.text.to_lowercase().contains(&needle) {
                text_matches.push((name.as_str(), entry.text.as_str()));
            }
        }
        name_matches.extend(text_matches);
        name_matches
    }

    pub(crate) fn left_items(&self) -> &[LeftItem] {
        &self.left_items_cache
    }

    pub(crate) fn rebuild_left_items(&mut self) {
        self.left_items_cache = build_left_items(
            &self.engine.projects,
            &self.engine.sessions,
            &self.engine.project_ids_with_terminals(),
            &self.collapsed_projects,
            self.engine.config.ui.empty_project_separator_min_projects,
        );
        self.ensure_selectable_left_item();
    }

    pub(crate) fn is_selectable_left_item(&self, index: usize) -> bool {
        self.left_items()
            .get(index)
            .is_some_and(|item| item.is_selectable())
    }

    pub(crate) fn next_selectable_left_item_after(&self, index: usize) -> Option<usize> {
        self.left_items()
            .iter()
            .enumerate()
            .skip(index.saturating_add(1))
            .find_map(|(idx, item)| item.is_selectable().then_some(idx))
    }

    pub(crate) fn previous_selectable_left_item_before(&self, index: usize) -> Option<usize> {
        self.left_items()
            .iter()
            .enumerate()
            .take(index)
            .rev()
            .find_map(|(idx, item)| item.is_selectable().then_some(idx))
    }

    pub(crate) fn ensure_selectable_left_item(&mut self) {
        if self.left_items_cache.is_empty() {
            self.selected_left = 0;
            return;
        }
        if self.selected_left >= self.left_items_cache.len() {
            self.selected_left = self.left_items_cache.len().saturating_sub(1);
        }
        if self.left_items_cache[self.selected_left].is_selectable() {
            return;
        }
        if let Some(next) = self.next_selectable_left_item_after(self.selected_left) {
            self.selected_left = next;
        } else if let Some(prev) = self.previous_selectable_left_item_before(self.selected_left) {
            self.selected_left = prev;
        }
    }

    pub(crate) fn sort_sessions_by_updated(&mut self) {
        self.engine
            .sessions
            .sort_by_key(|b| std::cmp::Reverse(b.updated_at));
        self.rebuild_left_items();
        if self.persist_sorted_session_order() {
            self.set_info("Agents sorted by most recently updated.");
        }
    }

    pub(crate) fn sort_sessions_by_created(&mut self) {
        self.engine
            .sessions
            .sort_by_key(|b| std::cmp::Reverse(b.created_at));
        self.rebuild_left_items();
        if self.persist_sorted_session_order() {
            self.set_info("Agents sorted by creation date (newest first).");
        }
    }

    pub(crate) fn sort_sessions_by_name(&mut self) {
        self.engine.sessions.sort_by(|a, b| {
            let name_a = a.title.as_deref().unwrap_or(&a.branch_name);
            let name_b = b.title.as_deref().unwrap_or(&b.branch_name);
            name_a.to_lowercase().cmp(&name_b.to_lowercase())
        });
        self.rebuild_left_items();
        if self.persist_sorted_session_order() {
            self.set_info("Agents sorted alphabetically by name.");
        }
    }

    /// Persist the freshly-sorted in-memory session order into SQLite so it
    /// survives a reload and matches the web UI by construction. Returns `true`
    /// on success. A failure is non-fatal: the sort still applies in-memory; we
    /// log and surface a status-line error rather than crash, and the caller
    /// skips its success message so the error stays visible.
    fn persist_sorted_session_order(&mut self) -> bool {
        match self.engine.persist_session_order() {
            Ok(()) => true,
            Err(err) => {
                logger::error(&format!("failed to persist sorted agent order: {err:#}"));
                self.set_error(format!(
                    "Sorted agents on screen, but couldn't save the new order: {err}"
                ));
                false
            }
        }
    }

    pub(crate) fn toggle_collapse_selected_project(&mut self) {
        if let Some(project) = self.selected_project() {
            let id = project.id.clone();
            let has_sessions = self.engine.sessions.iter().any(|s| s.project_id == id);
            if !has_sessions {
                return;
            }
            if self.collapsed_projects.contains(&id) {
                self.collapsed_projects.remove(&id);
            } else {
                self.collapsed_projects.insert(id.clone());
            }
            self.rebuild_left_items();

            // Move selection to the toggled project so collapsing from a
            // child session leaves the cursor on the parent header.
            if let Some(new_index) = self.left_items().iter().position(
                |item| matches!(item, LeftItem::Project(pi) if self.engine.projects[*pi].id == id),
            ) {
                self.selected_left = new_index;
            }
        }
    }

    pub(crate) fn selected_project(&self) -> Option<&Project> {
        match self.left_items().get(self.selected_left) {
            Some(LeftItem::Project(index)) => self.engine.projects.get(*index),
            Some(LeftItem::Session(index)) => {
                self.engine.sessions.get(*index).and_then(|session| {
                    self.engine
                        .projects
                        .iter()
                        .find(|project| project.id == session.project_id)
                })
            }
            Some(LeftItem::EmptyProjectsSpacer) => None,
            Some(LeftItem::EmptyProjectsSeparator) => None,
            // An orphaned group has no project record to return.
            Some(LeftItem::OrphanProject(_)) => None,
            None => None,
        }
    }

    pub(crate) fn selected_session(&self) -> Option<&AgentSession> {
        match self.left_items().get(self.selected_left) {
            Some(LeftItem::Session(index)) => self.engine.sessions.get(*index),
            _ => None,
        }
    }

    pub(crate) fn reload_changed_files(&mut self) {
        let session_id = self.selected_session().map(|s| s.id.clone());
        // Capture the previously-watched session BEFORE set_watched_session
        // overwrites it, so we can tell a genuine focus change from an incidental
        // reload of the already-selected session (commit, stage/discard, a
        // file-watcher refresh, etc. all call this helper).
        let previously_watched = self.engine.watched_session_id.clone();
        // The engine sets the watch (cheap, no git) and returns the worktree to
        // compute changed files for. The web computes this off-thread (the actor
        // thread serves every client), but the TUI is single-user on its own App
        // thread, so it computes inline: `set_watched_session` empties the lists,
        // then the inline read refills them within this same synchronous call —
        // no visible flicker.
        let worktree = self.engine.set_watched_session(session_id.as_deref());
        if let Some(path) = worktree {
            let (staged, unstaged) = git::changed_files(&path).unwrap_or_default();
            self.engine.staged_files = staged;
            self.engine.unstaged_files = unstaged;
        }
        self.clamp_files_cursor();
        if let Some(sid) = session_id {
            if previously_watched.as_deref() != Some(sid.as_str()) {
                // Genuine focus change → tight foreground refresh.
                self.engine.spawn_foreground_pr_check(&sid);
            } else {
                // Incidental reload of the already-focused session → keep the
                // normal background spacing so file ops don't over-poll `gh`.
                self.engine
                    .spawn_pr_check_for_session(&sid, dux_core::engine::PR_CHECK_MIN_INTERVAL);
            }
        }
    }

    pub(crate) fn selected_changed_file(&self) -> Option<&ChangedFile> {
        match self.right_section {
            RightSection::Staged => self.engine.staged_files.get(self.files_index),
            RightSection::Unstaged => self.engine.unstaged_files.get(self.files_index),
            RightSection::CommitInput => None,
        }
    }

    pub(crate) fn current_files_len(&self) -> usize {
        match self.right_section {
            RightSection::Staged => self.engine.staged_files.len(),
            RightSection::Unstaged => self.engine.unstaged_files.len(),
            RightSection::CommitInput => 0,
        }
    }

    pub(crate) fn clamp_files_cursor(&mut self) {
        if self.right_section == RightSection::CommitInput {
            return;
        }
        let len = self.current_files_len();
        if len == 0 {
            self.files_index = 0;
        } else if self.files_index >= len {
            self.files_index = len.saturating_sub(1);
        }
    }

    pub(crate) fn has_files_search(&self) -> bool {
        !self.files_search.is_empty()
    }

    pub(crate) fn clear_files_search(&mut self) {
        self.files_search.clear();
        self.files_search_active = false;
    }

    pub(crate) fn update_files_search(&mut self, query: String) -> bool {
        self.files_search.set_text(query);
        if self.files_search.is_empty() {
            return false;
        }
        self.select_files_search_match(0)
    }

    pub(crate) fn advance_files_search_match(&mut self) -> bool {
        let matches = self.files_search_matches();
        if matches.is_empty() {
            return false;
        }

        let current = (self.right_section, self.files_index);
        let next_index = matches
            .iter()
            .position(|candidate| *candidate == current)
            .map(|index| (index + 1) % matches.len())
            .unwrap_or(0);

        self.apply_files_match(matches[next_index]);
        true
    }

    fn select_files_search_match(&mut self, match_index: usize) -> bool {
        let matches = self.files_search_matches();
        if matches.is_empty() {
            return false;
        }

        let target = matches[match_index.min(matches.len().saturating_sub(1))];
        self.apply_files_match(target);
        true
    }

    fn apply_files_match(&mut self, target: (RightSection, usize)) {
        self.right_section = target.0;
        self.files_index = target.1;
        self.clamp_files_cursor();
    }

    fn files_search_matches(&self) -> Vec<(RightSection, usize)> {
        if self.files_search.is_empty() {
            return Vec::new();
        }

        let needle = self.files_search.text.to_lowercase();
        let mut matches = Vec::new();
        matches.extend(
            self.engine
                .unstaged_files
                .iter()
                .enumerate()
                .filter(|(_, file)| file.path.to_lowercase().contains(&needle))
                .map(|(index, _)| (RightSection::Unstaged, index)),
        );
        matches.extend(
            self.engine
                .staged_files
                .iter()
                .enumerate()
                .filter(|(_, file)| file.path.to_lowercase().contains(&needle))
                .map(|(index, _)| (RightSection::Staged, index)),
        );
        matches
    }

    /// Open the read-only Agent Info modal for the focused agent. Routed through
    /// `PromptState` so `Esc` closes it uniformly with every other prompt.
    pub(crate) fn open_agent_info(&mut self) -> Result<()> {
        if let Some(session) = self.selected_session().cloned() {
            let label = session
                .title
                .clone()
                .unwrap_or_else(|| session.branch_name.clone());
            self.input_target = InputTarget::None;
            self.fullscreen_overlay = FullscreenOverlay::None;
            self.prompt = PromptState::AgentInfo(AgentInfoPrompt {
                session_label: label,
                lines: agent_info_lines(&session),
            });
        } else {
            self.set_error("No agent session selected. Select an agent to see its info.");
        }
        Ok(())
    }

    pub(crate) fn open_rename_session(&mut self) -> Result<()> {
        if let Some(session) = self.selected_session().cloned() {
            if self
                .engine
                .is_in_flight(&dux_core::engine::InFlightKey::BranchRename(
                    session.id.clone(),
                ))
            {
                self.set_error(
                    "A rename is already in progress for this agent. Wait for it to finish before renaming again.",
                );
                return Ok(());
            }
            let current_name = session.title.unwrap_or_else(|| session.branch_name.clone());
            self.input_target = InputTarget::None;
            self.fullscreen_overlay = FullscreenOverlay::None;
            self.prompt = PromptState::RenameSession {
                session_id: session.id,
                input: TextInput::with_text(current_name)
                    .with_char_map(crate::git::agent_name_char_map),
                rename_branch: true,
            };
        } else {
            self.set_error("No agent session selected.");
        }
        Ok(())
    }

    pub(crate) fn apply_rename_session(
        &mut self,
        session_id: &str,
        new_name: String,
        rename_branch: bool,
    ) {
        let name = new_name.trim().to_string();
        if name.is_empty() {
            self.set_error("Name cannot be empty.");
            return;
        }
        if !git::is_valid_agent_name(&name) {
            self.set_error(
                "Agent name may only contain letters, digits, dashes, underscores, or slashes. \
                 It cannot start with \"-\" or \"/\", end with \"/\", or contain \"//\".",
            );
            return;
        }

        // Block overlapping renames: a second concurrent `git branch -m` on the
        // same worktree would race the first (and could corrupt the in-flight
        // drift-suppression bookkeeping). Mirror the CreateAgent busy-guard.
        if self
            .engine
            .is_in_flight(&dux_core::engine::InFlightKey::BranchRename(
                session_id.to_string(),
            ))
        {
            self.set_error(
                "A rename is already in progress for this agent. Wait for it to finish before renaming again.",
            );
            return;
        }

        // Capture the previous title before mutating, in case we need to
        // revert on a failed branch rename.
        let previous_title = self
            .engine
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .and_then(|s| s.title.clone());

        // Always update the display title immediately.
        if let Some(session) = self.engine.sessions.iter_mut().find(|s| s.id == session_id) {
            session.title = Some(name.clone());
            session.updated_at = Utc::now();
        }
        if let Some(session) = self.engine.sessions.iter().find(|s| s.id == session_id) {
            let _ = self.engine.session_store.upsert_session(session);
        }
        self.rebuild_left_items();

        // Optionally rename the git branch in a background worker.
        if rename_branch {
            let Some(session) = self.engine.sessions.iter().find(|s| s.id == session_id) else {
                return;
            };
            let old_branch = session.branch_name.clone();
            if name == old_branch {
                self.set_info(format!("Renamed agent to \"{name}\"."));
                return;
            }
            let worktree = session.worktree_path.clone();
            let sid = session.id.clone();
            let new_branch = name.clone();
            // Declare the loading→final states together; the worker resolves the
            // matching message and carries it back on BranchRenameCompleted.
            let success_branch = new_branch.clone();
            let op = dux_core::engine::status_op(format!("Renaming branch to \"{name}\"\u{2026}"))
                .on_success(move |_: &()| {
                    dux_core::engine::Final::info(format!(
                        "Renamed agent and branch to \"{success_branch}\"."
                    ))
                })
                .on_failure(|e: &String| {
                    dux_core::engine::Final::error(format!(
                        "Branch rename failed, reverted agent name: {e}"
                    ))
                });
            let op_key = op.key().to_string();
            let pending = op.pending_status();

            // Stash the expected branches so `BranchSyncReady` can distinguish
            // our own in-progress rename (silently skip) from an unrelated
            // external change landing mid-rename (log it). Cleared alongside the
            // in-flight marker in `BranchRenameCompleted`.
            self.engine.rename_expected.insert(
                sid.clone(),
                dux_core::engine::RenameExpectation {
                    old_branch: old_branch.clone(),
                    new_branch: new_branch.clone(),
                },
            );

            // Clones for the panic path: if the worker thread panics, the
            // synthesised `BranchRenameCompleted` still runs the handler, which
            // reverts the title AND clears both the in-flight marker and
            // `rename_expected` — so a panic can never permanently freeze drift
            // detection for this session.
            let panic_sid = sid.clone();
            let panic_new_branch = new_branch.clone();
            let panic_previous_title = previous_title.clone();
            // A separate clone for the synchronous-spawn-failure revert below:
            // if the worker thread never starts, no `BranchRenameCompleted`
            // fires, so we must unwind the optimistic title/marker/expectation
            // here instead.
            let revert_previous_title = previous_title.clone();

            // Route through the panic-safe background-worker primitive. Its
            // `in_flight_key` marks the rename in flight (so the branch-sync
            // poller skips it) and its `panic_event` guarantees the completion
            // event fires even on panic.
            let job_worktree = worktree;
            let job_old_branch = old_branch;
            let job_sid = sid.clone();
            let job_new_branch = new_branch;
            let outcome = self.engine.spawn_background_worker(
                dux_core::engine::BackgroundWorkerSpec {
                    label: format!("branch-rename[{job_sid}]"),
                    in_flight_key: Some(dux_core::engine::InFlightKey::BranchRename(sid.clone())),
                    panic_event: Some(Box::new(move |reason| WorkerEvent::BranchRenameCompleted {
                        session_id: panic_sid,
                        new_branch: panic_new_branch,
                        previous_title: panic_previous_title,
                        result: Err(reason.clone()),
                        status: dux_core::engine::ResolvedFinal::error(
                            op_key,
                            format!("Branch rename failed, reverted agent name: {reason}"),
                        ),
                    })),
                },
                move |tx| {
                    let result = git::rename_branch(
                        Path::new(&job_worktree),
                        &job_old_branch,
                        &job_new_branch,
                    )
                    .map_err(|e| e.to_string());
                    let status = op.resolve(&result);
                    let _ = tx.send(WorkerEvent::BranchRenameCompleted {
                        session_id: job_sid,
                        new_branch: job_new_branch,
                        previous_title,
                        result,
                        status,
                    });
                },
            );
            // Only apply the pending Busy if the worker actually started. On a
            // synchronous spawn failure no `BranchRenameCompleted` will ever
            // fire, so the Busy would hang forever and the optimistic title +
            // `rename_expected` would be orphaned — unwind them and surface an
            // error instead.
            match outcome {
                dux_core::engine::BackgroundSpawn::Spawned => {
                    self.apply_reaction(dux_core::engine::EventReaction::Status(pending));
                }
                dux_core::engine::BackgroundSpawn::SpawnFailed
                | dux_core::engine::BackgroundSpawn::AlreadyInFlight => {
                    self.engine
                        .revert_optimistic_rename(&sid, revert_previous_title);
                    self.rebuild_left_items();
                    self.set_error(
                        "Could not start the branch-rename worker; reverted the agent name. \
                         Please try again.",
                    );
                }
            }
        } else {
            self.set_info(format!("Renamed agent to \"{name}\"."));
            self.engine.update_branch_sync_sessions();
        }
    }

    /// Derives a companion terminal status for a session from the multi-terminal map.
    /// Running if any terminal exists for this session, NotLaunched otherwise.
    pub(crate) fn companion_terminal_status(&self, session_id: &str) -> CompanionTerminalStatus {
        if self.session_terminal_count(session_id) > 0 {
            CompanionTerminalStatus::Running
        } else {
            CompanionTerminalStatus::NotLaunched
        }
    }

    pub(crate) fn selected_companion_terminal_status(&self) -> CompanionTerminalStatus {
        self.selected_session()
            .map(|session| self.companion_terminal_status(&session.id))
            .unwrap_or(CompanionTerminalStatus::NotLaunched)
    }

    pub(crate) fn clear_companion_terminals_for_session(&mut self, session_id: &str) {
        self.engine
            .companion_terminals
            .retain(|_, t| !matches!(&t.owner, TerminalOwner::Session(sid) if sid == session_id));
        if let Some(ref id) = self.active_terminal_id
            && !self.engine.companion_terminals.contains_key(id)
        {
            self.active_terminal_id = None;
        }
    }

    pub(crate) fn running_process_count(&self) -> usize {
        self.engine.providers.len() + self.engine.companion_terminals.len()
    }

    pub(crate) fn running_companion_terminal_count(&self) -> usize {
        self.engine.companion_terminals.len()
    }

    /// Returns all running companion terminals as (terminal_id, terminal) pairs,
    /// sorted by creation order (terminal_id encodes the counter).
    pub(crate) fn terminal_items(&self) -> Vec<(&String, &CompanionTerminal)> {
        let mut items: Vec<_> = self.engine.companion_terminals.iter().collect();
        items.sort_by_key(|(id, _)| (*id).clone());
        items
    }

    pub(crate) fn has_terminal_items(&self) -> bool {
        !self.engine.companion_terminals.is_empty()
    }

    pub(crate) fn clamp_terminal_cursor(&mut self) {
        let count = self.terminal_items().len();
        if count == 0 {
            self.selected_terminal_index = 0;
            if self.left_section == LeftSection::Terminals {
                self.left_section = LeftSection::Projects;
            }
        } else if self.selected_terminal_index >= count {
            self.selected_terminal_index = count.saturating_sub(1);
        }
    }

    pub(crate) fn next_terminal_id(&mut self) -> String {
        self.engine.terminal_counter += 1;
        format!("term-{}", self.engine.terminal_counter)
    }

    /// Returns the number of running companion terminals for a given session.
    pub(crate) fn session_terminal_count(&self, session_id: &str) -> usize {
        self.engine
            .companion_terminals
            .values()
            .filter(|t| matches!(&t.owner, TerminalOwner::Session(sid) if sid == session_id))
            .count()
    }

    /// Returns the number of running project terminals for a given project.
    pub(crate) fn project_terminal_count(&self, project_id: &str) -> usize {
        self.engine
            .companion_terminals
            .values()
            .filter(|t| matches!(&t.owner, TerminalOwner::Project(pid) if pid == project_id))
            .count()
    }

    /// The `forward_scroll` policy for the currently selected terminal
    /// surface. Agents resolve it from their provider config; companion
    /// terminals have no provider config, so they always auto-detect (`None`).
    pub(crate) fn selected_surface_forward_scroll(&self) -> Option<bool> {
        match self.session_surface {
            SessionSurface::Agent => {
                let session = self.selected_session()?;
                let provider = self.focused_tab_provider(session);
                provider_config(&self.engine.config, &provider).forward_scroll
            }
            SessionSurface::Terminal => None,
        }
    }

    pub(crate) fn selected_terminal_surface_client(&self) -> Option<&PtyClient> {
        match self.session_surface {
            SessionSurface::Agent => {
                let session_id = self.selected_session()?.id.clone();
                self.engine.providers.get(&self.focused_tab_id(&session_id))
            }
            SessionSurface::Terminal => {
                let id = self.active_terminal_id.as_ref()?;
                self.engine.companion_terminals.get(id).map(|t| &t.client)
            }
        }
    }

    // ---- Agent tabs: per-session focused tab, switching, labels ----

    /// The focused tab id for a session, defaulting to the session-slot tab (the
    /// session id). Clamps to Main when the stored tab no longer exists, so
    /// every seam that resolves the focused tab is safe after a tab close.
    ///
    /// The in-process `focused_tabs` HashMap is the live value and wins when it
    /// has an entry for this session. When it has none (e.g. right after a
    /// restart, before this process has focused anything itself), falls back to
    /// the engine's persisted `AgentSession.last_focused_tab` via the shared
    /// resolver so the remembered tab survives a restart too.
    pub(crate) fn focused_tab_id(&self, session_id: &str) -> String {
        match self.focused_tabs.get(session_id) {
            Some(id) if id == session_id => id.clone(),
            Some(id)
                if self
                    .engine
                    .agent_tabs
                    .get(id)
                    .is_some_and(|t| t.session_id == session_id) =>
            {
                id.clone()
            }
            Some(_) => session_id.to_string(),
            None => match self.engine.sessions.iter().find(|s| s.id == session_id) {
                Some(session) => {
                    let live_extra_ids = self
                        .engine
                        .agent_tabs
                        .values()
                        .filter(|t| t.session_id == session_id)
                        .map(|t| t.id.as_str());
                    session.resolved_focused_tab(live_extra_ids).to_string()
                }
                None => session_id.to_string(),
            },
        }
    }

    /// The effective provider of a session's focused tab (Main resolves to the
    /// session's running/pinned provider; an extra tab to its own, honoring a
    /// swap-while-running pin). Used for the center title, caption, and
    /// scroll-routing config lookup.
    pub(crate) fn focused_tab_provider(&self, session: &AgentSession) -> ProviderKind {
        let tab = self.focused_tab_id(&session.id);
        // The pin -> tab-row -> session.provider chain is the single source of
        // truth `Engine::tab_running_provider` owns; the TUI's other tab-label
        // helper (`tab_provider_label` in render.rs) also delegates to it so the
        // two call sites can't drift.
        self.engine.tab_running_provider(session, &tab)
    }

    /// Ordered tab ids for a session: Main (session id) first, then Support
    /// tabs by (sort_order, created_at).
    pub(crate) fn session_tab_ids(&self, session_id: &str) -> Vec<String> {
        let mut support: Vec<&AgentTab> = self
            .engine
            .agent_tabs
            .values()
            .filter(|t| t.session_id == session_id)
            .collect();
        support.sort_by(|a, b| {
            a.sort_order
                .cmp(&b.sort_order)
                .then_with(|| a.created_at.cmp(&b.created_at))
        });
        let mut ids = Vec::with_capacity(support.len() + 1);
        ids.push(session_id.to_string());
        ids.extend(support.into_iter().map(|t| t.id.clone()));
        ids
    }

    /// Set the focused tab for a session (Main clears the entry). Switching
    /// forces a snapshot + PTY-size refresh and clears the terminal selection,
    /// mirroring the side effects of a surface change.
    ///
    /// Also writes the choice through to the engine's persisted
    /// `last_focused_tab` (a tiny synchronous UPDATE, like
    /// `ToggleAgentAutoReopen`) so it survives a restart and is visible to the
    /// web surface sharing the same SQLite file. This is a silent, best-effort
    /// persist: a failure here must not block or roll back the (already
    /// authoritative) in-process focus switch, so any error is intentionally
    /// discarded, matching the wire command's "no status" contract (J3).
    pub(crate) fn set_focused_tab(&mut self, session_id: &str, tab_id: &str) {
        if tab_id == session_id {
            self.focused_tabs.remove(session_id);
        } else {
            self.focused_tabs
                .insert(session_id.to_string(), tab_id.to_string());
        }
        let _ = self.engine.set_last_focused_tab(
            session_id,
            if tab_id == session_id {
                None
            } else {
                Some(tab_id)
            },
        );
        self.last_snapshot_id = None;
        self.last_pty_size = (0, 0);
        self.terminal_selection = None;
        // Switching tabs foregrounds the owning agent — refresh its PR status.
        self.engine.spawn_foreground_pr_check(session_id);
    }

    /// Drop a session's focused-tab entry (called on session teardown so
    /// stale entries can't leak). Mirrors `clear_companion_terminals_for_session`.
    pub(crate) fn clear_focused_tab_for_session(&mut self, session_id: &str) {
        self.focused_tabs.remove(session_id);
    }

    /// Move focus to the next/previous tab of the selected session (wrapping).
    /// No-op when the session has fewer than two tabs.
    pub(crate) fn focus_tab_relative(&mut self, forward: bool) {
        let Some(session_id) = self.selected_session().map(|s| s.id.clone()) else {
            return;
        };
        let ids = self.session_tab_ids(&session_id);
        if ids.len() < 2 {
            return;
        }
        let cur = self.focused_tab_id(&session_id);
        let idx = ids.iter().position(|i| *i == cur).unwrap_or(0);
        let next = if forward {
            (idx + 1) % ids.len()
        } else {
            (idx + ids.len() - 1) % ids.len()
        };
        let target = ids[next].clone();
        self.set_focused_tab(&session_id, &target);
    }

    /// Jump to the nth tab (0-based) of the selected session, if it exists.
    pub(crate) fn focus_tab_index(&mut self, n: usize) {
        let Some(session_id) = self.selected_session().map(|s| s.id.clone()) else {
            return;
        };
        let ids = self.session_tab_ids(&session_id);
        if let Some(target) = ids.get(n).cloned() {
            self.set_focused_tab(&session_id, &target);
        }
    }

    /// Refresh `self.snapshot_buf` from the currently selected terminal
    /// surface, reusing the existing cell allocation. Returns `true` if a
    /// provider was found and the snapshot was updated.
    pub(crate) fn refresh_snapshot_buf(&mut self) -> bool {
        let (client_id, client): (String, Option<&PtyClient>) = match self.session_surface {
            SessionSurface::Agent => {
                let session_id = match self.selected_session() {
                    Some(s) => s.id.clone(),
                    None => return false,
                };
                let tab_id = self.focused_tab_id(&session_id);
                let provider = self.engine.providers.get(&tab_id);
                (tab_id, provider)
            }
            SessionSurface::Terminal => {
                let id = match self.active_terminal_id.as_ref() {
                    Some(id) => id.clone(),
                    None => return false,
                };
                let provider = self.engine.companion_terminals.get(&id).map(|t| &t.client);
                (id, provider)
            }
        };
        if let Some(provider) = client {
            if self.last_snapshot_id.as_deref() != Some(&client_id) {
                provider.mark_dirty();
                self.last_snapshot_id = Some(client_id);
                self.terminal_selection = None;
            }
            let collect_links = self.engine.config.capabilities.hyperlinks;
            provider.snapshot_into(&mut self.snapshot_buf, collect_links);
            true
        } else {
            false
        }
    }
}

/// Disambiguated tab labels: each provider name is used as-is the first time
/// and suffixed with " 2", " 3", … for repeats, in order. Pure, unit-tested.
pub(crate) fn tab_labels(providers: &[&str]) -> Vec<String> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    let mut out = Vec::with_capacity(providers.len());
    for p in providers {
        let n = seen.entry(*p).or_insert(0);
        *n += 1;
        if *n == 1 {
            out.push((*p).to_string());
        } else {
            out.push(format!("{p} {n}"));
        }
    }
    out
}

pub(crate) use dux_core::project_browser::load_projects;

#[allow(deprecated)] // blessed sync-direct: bootstrap/reload-worker project-sync runs before/outside the queue
pub(crate) fn persist_runtime_projects_to_config_and_store(
    projects: &[Project],
    config: &mut Config,
    paths: &DuxPaths,
    bindings: &RuntimeBindings,
    session_store: &SessionStore,
) -> Result<()> {
    let existing_projects = config.projects.clone();
    let stored_project_configs = projects
        .iter()
        .map(|project| runtime_project_to_config(project, &existing_projects))
        .collect::<Vec<_>>();
    let config_project_configs = stored_project_configs
        .iter()
        .cloned()
        .map(|mut project| {
            project.leading_branch = None;
            project
        })
        .collect::<Vec<_>>();

    let stored_projects = session_store.load_projects()?;
    for (index, project_config) in stored_project_configs.iter().enumerate() {
        let stored_project = stored_projects.iter().find(|stored| {
            stored.id == project_config.id || same_expanded_project_path(stored, project_config)
        });
        if stored_project != Some(project_config) {
            session_store.upsert_project_at(project_config, index as i64)?;
        }
    }

    if config.projects != config_project_configs {
        config.projects = config_project_configs;
        save_config(&paths.config_path, config, bindings)?;
    }

    Ok(())
}

#[allow(deprecated)] // blessed sync-direct: bootstrap/reload-worker project-sync runs before/outside the queue
pub(crate) fn sync_config_projects_with_store(
    config: &mut Config,
    paths: &DuxPaths,
    bindings: &RuntimeBindings,
    session_store: &SessionStore,
) -> Result<()> {
    validate_project_records("config.toml", &config.projects)?;
    let mut stored = session_store.load_projects()?;
    validate_project_records("SQLite", &stored)?;

    let mut changed_config = false;
    let mut changed_store = false;
    let mut merged = config.projects.clone();

    for (index, cfg_project) in config.projects.iter().enumerate() {
        match stored.iter().position(|stored_project| {
            stored_project.id == cfg_project.id
                || same_expanded_project_path(stored_project, cfg_project)
        }) {
            Some(stored_index) => {
                let stored_project = &stored[stored_index];
                let (merged_config_project, merged_stored_project) =
                    merge_project_records(cfg_project, stored_project)?;
                if &merged_config_project != cfg_project {
                    merged[index] = merged_config_project;
                    changed_config = true;
                }
                if &merged_stored_project != stored_project {
                    session_store.upsert_project_at(&merged_stored_project, stored_index as i64)?;
                    stored[stored_index] = merged_stored_project;
                    changed_store = true;
                }
            }
            None => {
                session_store.upsert_project_at(cfg_project, index as i64)?;
                changed_store = true;
                stored.push(cfg_project.clone());
            }
        }
    }

    for stored_project in stored {
        let exists = merged.iter().any(|cfg_project| {
            cfg_project.id == stored_project.id
                || same_expanded_project_path(cfg_project, &stored_project)
        });
        if !exists {
            let mut portable = stored_project;
            portable.path = portable_project_path(&portable.path);
            merged.push(portable);
            changed_config = true;
        }
    }

    if changed_config {
        config.projects = merged;
        save_config(&paths.config_path, config, bindings)?;
    } else if changed_store {
        // Keep the on-disk config normalized when the database was repaired
        // from config-only projects.
        save_config(&paths.config_path, config, bindings)?;
    }
    Ok(())
}

/// Pre-flight for the in-process TUI→web flip: resolve LOCAL MODE addresses
/// (loopback:port plus the machine's Tailscale address:port when one was
/// detected) and actually bind a std `TcpListener` for each BEFORE the TUI tears
/// anything down. Returning the bound listeners (rather than addresses) means
/// there is no rebind race when the web server adopts them.
///
/// The flip is structurally local-only: this function takes `port` +
/// `tailscale_ip`, never a configurable bind host, so it can never open a public
/// listener.
/// Tailscale detection (`tailscale ip`) is a subprocess call, so the CALLER runs
/// it on a worker thread and hands the result here — this function does no
/// blocking work beyond the (fast, local) `TcpListener::bind`.
///
/// Required vs best-effort mirrors the CLI serve path: loopback is REQUIRED, so a
/// bind failure there is FATAL (the pre-flight fails, the TUI stays up, and the
/// failing address is logged); the Tailscale leg is BEST-EFFORT, so a bind
/// failure there is DROPPED with a warning (named in the returned `warnings`) and
/// the flip proceeds loopback-only. This matches how a Tailscale address that was
/// never DETECTED already degrades to loopback with a warning.
///
/// Each display URL reflects the listener's `local_addr`, so an ephemeral `:0`
/// port resolves to the real port the user can open. Returns `(listeners, urls,
/// warnings)`; on a REQUIRED bind failure the whole pre-flight fails and
/// already-bound listeners drop.
fn preflight_server_listeners(
    port: u16,
    tailscale_ip: Option<std::net::IpAddr>,
) -> Result<(Vec<std::net::TcpListener>, Vec<String>, Vec<String>)> {
    let addrs = dux_core::config::local_addrs(port, tailscale_ip);
    let mut listeners = Vec::with_capacity(addrs.len());
    let mut urls = Vec::with_capacity(addrs.len());
    let mut warnings = Vec::new();
    for plan_addr in addrs {
        let addr = plan_addr.addr();
        match std::net::TcpListener::bind(addr) {
            Ok(listener) => {
                let bound = listener.local_addr().unwrap_or(addr);
                urls.push(format!("http://{bound}"));
                listeners.push(listener);
            }
            Err(err) if plan_addr.is_required() => {
                // Loopback (required): the flip cannot serve without it. Log the
                // failing address to dux.log, then fail the pre-flight so the TUI
                // surfaces the error and stays up.
                dux_core::logger::error(&format!(
                    "[server] could not start the web server: {err} \
                     (is something already listening on {addr}?)"
                ));
                return Err(anyhow::anyhow!(
                    "could not start the web server: {err} \
                     (is something already listening on {addr}?)"
                ));
            }
            Err(err) => {
                // Tailscale leg (best-effort): drop it, warn, serve loopback-only.
                let warning = format!(
                    "Could not bind the Tailscale address {addr}: {err} — something else is \
                     already listening there; serving on loopback only. Stop that process or \
                     change [server] port to also serve on Tailscale."
                );
                dux_core::logger::warn(&format!("[server] {warning}"));
                warnings.push(warning);
            }
        }
    }
    Ok((listeners, urls, warnings))
}

fn validate_project_records(source: &str, projects: &[crate::config::ProjectConfig]) -> Result<()> {
    for (index, project) in projects.iter().enumerate() {
        for other in projects.iter().skip(index + 1) {
            if project.id == other.id {
                anyhow::bail!(
                    "Project sync conflict in {source}: duplicate project id \"{}\". Remove or rename one [[projects]] entry, then restart dux.",
                    project.id
                );
            }
            if same_expanded_project_path(project, other) {
                anyhow::bail!(
                    "Project sync conflict in {source}: duplicate project path \"{}\". Remove one duplicate project entry, then restart dux.",
                    expanded_project_path(project).unwrap_or_else(|| project.path.clone())
                );
            }
        }
    }
    Ok(())
}

fn merge_project_records(
    config_project: &crate::config::ProjectConfig,
    stored_project: &crate::config::ProjectConfig,
) -> Result<(crate::config::ProjectConfig, crate::config::ProjectConfig)> {
    let config_path = expanded_project_path(config_project);
    let stored_path = expanded_project_path(stored_project);
    if config_project.id == stored_project.id && config_path != stored_path {
        anyhow::bail!(
            "Project sync conflict for id \"{}\": config.toml points to \"{}\" but SQLite points to \"{}\". Edit config.toml or remove/re-add the project so both stores agree.",
            config_project.id,
            config_project.path,
            stored_project.path
        );
    }
    if config_path == stored_path && config_project.id != stored_project.id {
        anyhow::bail!(
            "Project sync conflict for path \"{}\": config.toml uses id \"{}\" but SQLite uses id \"{}\". Edit config.toml or remove/re-add the project so both stores agree.",
            config_path.unwrap_or_else(|| config_project.path.clone()),
            config_project.id,
            stored_project.id
        );
    }

    let mut merged_config = config_project.clone();
    let mut merged_stored = stored_project.clone();

    sync_config_authoritative_project_field(&mut merged_config.name, &mut merged_stored.name);
    sync_config_authoritative_project_field(
        &mut merged_config.default_provider,
        &mut merged_stored.default_provider,
    );
    if merged_stored.leading_branch.is_none() {
        merged_stored.leading_branch = merged_config.leading_branch.clone();
    }
    merged_config.leading_branch = None;
    sync_config_authoritative_project_field(
        &mut merged_config.startup_command,
        &mut merged_stored.startup_command,
    );
    sync_config_authoritative_project_field(
        &mut merged_config.auto_reopen_agents,
        &mut merged_stored.auto_reopen_agents,
    );
    merged_stored.env = merged_config.env.clone();

    Ok((merged_config, merged_stored))
}

fn sync_config_authoritative_project_field<T>(
    config_value: &mut Option<T>,
    stored_value: &mut Option<T>,
) where
    T: Clone,
{
    match config_value.as_ref() {
        Some(config) => {
            *stored_value = Some(config.clone());
        }
        None => {
            *config_value = stored_value.clone();
        }
    }
}

fn same_expanded_project_path(
    left: &crate::config::ProjectConfig,
    right: &crate::config::ProjectConfig,
) -> bool {
    expanded_project_path(left).is_some_and(|left_path| {
        expanded_project_path(right).is_some_and(|right_path| left_path == right_path)
    })
}

fn expanded_project_path(project: &crate::config::ProjectConfig) -> Option<String> {
    crate::config::expand_path(&project.path)
}

pub(crate) fn portable_project_path(path: &str) -> String {
    let Some(home) = home::home_dir() else {
        return path.to_string();
    };
    let path_buf = Path::new(path);
    if let Ok(relative) = path_buf.strip_prefix(&home) {
        let relative = relative.to_string_lossy();
        if relative.is_empty() {
            "$HOME".to_string()
        } else {
            format!("$HOME/{}", relative)
        }
    } else {
        path.to_string()
    }
}

pub(crate) fn runtime_project_to_config(
    project: &Project,
    existing_projects: &[crate::config::ProjectConfig],
) -> crate::config::ProjectConfig {
    let path = existing_projects
        .iter()
        .find(|existing| {
            existing.id == project.id
                && expanded_project_path(existing).is_some_and(|expanded| expanded == project.path)
        })
        .map(|existing| existing.path.clone())
        .unwrap_or_else(|| portable_project_path(&project.path));

    crate::config::ProjectConfig {
        id: project.id.clone(),
        path,
        name: Some(project.name.clone()),
        default_provider: project
            .explicit_default_provider
            .as_ref()
            .map(|provider| provider.as_str().to_string()),
        leading_branch: project.leading_branch.clone(),
        auto_reopen_agents: project.auto_reopen_agents,
        startup_command: project.startup_command.clone(),
        env: project.env.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attention_blink_phase_double_blinks_then_holds() {
        // Blink 1: show then hide.
        assert!(attention_blink_phase(0));
        assert!(attention_blink_phase(199));
        assert!(!attention_blink_phase(200));
        assert!(!attention_blink_phase(399));
        // Blink 2: show then hide.
        assert!(attention_blink_phase(400));
        assert!(!attention_blink_phase(600));
        assert!(!attention_blink_phase(799));
        // Hold: steady visibility until the cycle restarts. No separator hide
        // at the end (it would read as a third blink).
        assert!(attention_blink_phase(800));
        assert!(attention_blink_phase(1300));
        assert!(attention_blink_phase(1800));
        assert!(attention_blink_phase(1999));
        // The next cycle starts with its first hide at the same offset, so
        // exactly two hides happen per cycle.
        assert!(attention_blink_phase(2000));
        assert!(!attention_blink_phase(2000 + 250));
        assert!(attention_blink_phase(2000 + 450));
        assert!(!attention_blink_phase(2000 + 650));
    }

    fn test_project(id: &str) -> Project {
        Project {
            id: id.to_string(),
            name: id.to_string(),
            path: format!("/tmp/{id}"),
            explicit_default_provider: None,
            default_provider: ProviderKind::from_str("codex"),
            leading_branch: Some("main".to_string()),
            auto_reopen_agents: None,
            startup_command: None,
            env: Default::default(),
            current_branch: "main".to_string(),
            branch_status: ProjectBranchStatus::Unknown,
            path_missing: false,
            created_at: None,
        }
    }

    fn test_session(id: &str, project_id: &str, created_offset: i64) -> AgentSession {
        let now = Utc::now() + chrono::Duration::seconds(created_offset);
        AgentSession {
            id: id.to_string(),
            project_id: project_id.to_string(),
            project_path: Some(format!("/tmp/{project_id}")),
            provider: ProviderKind::from_str("codex"),
            source_branch: "main".to_string(),
            branch_name: id.to_string(),
            initial_branch: id.to_string(),
            worktree_path: format!("/tmp/worktrees/{id}"),
            title: None,
            started_providers: Vec::new(),
            desired_running: false,
            auto_reopen_enabled: true,
            status: SessionStatus::Detached,
            created_at: now,
            updated_at: now,
            last_focused_tab: None,
        }
    }

    #[test]
    fn agent_info_lines_include_lineage_and_drift() {
        let mut s = test_session("s1", "p1", 0);
        s.title = Some("server-mode".into());
        s.branch_name = "agent-tabs".into();
        s.initial_branch = "server-mode".into();
        s.source_branch = "main".into();

        let lines = agent_info_lines(&s);
        assert!(lines.iter().any(|(l, _)| l.contains("agent-tabs"))); // current branch
        assert!(lines.iter().any(|(l, _)| l.contains("server-mode"))); // original
        assert!(lines.iter().any(|(l, _)| l.contains("main"))); // forked from
        // The drift note is present AND tagged Warning (structured tone, not
        // substring-matched by the renderer).
        let drift = lines
            .iter()
            .find(|(l, _)| l.to_lowercase().contains("changed since creation"))
            .expect("drift line present");
        assert_eq!(drift.1, AgentInfoTone::Warning);
        // Every other line is Neutral.
        assert!(
            lines
                .iter()
                .filter(|(l, _)| !l.to_lowercase().contains("changed since creation"))
                .all(|(_, tone)| *tone == AgentInfoTone::Neutral)
        );
    }

    #[test]
    fn agent_info_lines_omit_drift_line_when_no_drift() {
        let mut s = test_session("s1", "p1", 0);
        s.branch_name = "main".into();
        s.initial_branch = "main".into();
        let lines = agent_info_lines(&s);
        assert!(
            !lines
                .iter()
                .any(|(l, _)| l.to_lowercase().contains("changed since creation"))
        );
        // With no drift, every line is Neutral — no Warning tone anywhere.
        assert!(
            lines
                .iter()
                .all(|(_, tone)| *tone == AgentInfoTone::Neutral)
        );
    }

    #[test]
    fn branch_drifted_guards_empty_initial() {
        // Real drift: current differs from a non-empty initial.
        assert!(branch_drifted("agent-tabs", "server-mode"));
        // No drift when equal.
        assert!(!branch_drifted("main", "main"));
        // Empty initial (legacy/never-backfilled row) is NEVER drift, even
        // though current != "". This is the phantom "(orig: )" guard.
        assert!(!branch_drifted("main", ""));
        assert!(!branch_drifted("", ""));
    }

    #[test]
    fn agent_info_lines_omit_drift_line_when_initial_empty() {
        let mut s = test_session("s1", "p1", 0);
        s.branch_name = "main".into();
        s.initial_branch = String::new();
        let lines = agent_info_lines(&s);
        assert!(
            !lines
                .iter()
                .any(|(l, _)| l.to_lowercase().contains("changed since creation")),
            "an empty initial_branch must not produce a phantom drift line"
        );
    }

    #[test]
    fn tab_labels_disambiguate_duplicate_providers() {
        assert_eq!(tab_labels(&["claude", "codex"]), vec!["claude", "codex"]);
        assert_eq!(
            tab_labels(&["codex", "codex"]),
            vec!["codex".to_string(), "codex 2".to_string()]
        );
        assert_eq!(
            tab_labels(&["claude", "codex", "codex", "claude"]),
            vec![
                "claude".to_string(),
                "codex".to_string(),
                "codex 2".to_string(),
                "claude 2".to_string()
            ]
        );
        assert!(tab_labels(&[]).is_empty());
    }

    #[test]
    fn preflight_binds_loopback_and_reports_actual_port() {
        // Port 0 lets the OS pick a free port; the display URL must reflect the
        // ACTUAL bound port (via local_addr), not the configured ":0". With no
        // Tailscale address, LOCAL MODE binds loopback only.
        let (listeners, urls, warnings) =
            preflight_server_listeners(0, None).expect("loopback bind should succeed");
        assert_eq!(listeners.len(), 1, "no tailscale → loopback only");
        assert_eq!(urls.len(), 1);
        assert!(warnings.is_empty(), "no tailscale leg → no warnings");

        let bound = listeners[0]
            .local_addr()
            .expect("listener has a local addr");
        assert!(bound.ip().is_loopback());
        assert_ne!(bound.port(), 0, "OS must have assigned a real port");
        assert_eq!(urls[0], format!("http://{bound}"));
        assert!(
            !urls[0].ends_with(":0"),
            "URL must not show the placeholder port"
        );
    }

    #[test]
    fn preflight_is_local_only_and_never_reads_bind_host() {
        // STRUCTURAL: the flip pre-flight takes a port + optional Tailscale IP,
        // never the configurable [server] host, so it can only ever bind loopback
        // (and Tailscale). Even with a public host configured, the flip path is
        // unaffected because it does not consult that field at all; this test
        // documents the local-only guarantee by exercising the only inputs the
        // flip can take.
        let (listeners, _urls, _warnings) =
            preflight_server_listeners(0, None).expect("loopback-only pre-flight succeeds");
        assert!(
            listeners
                .iter()
                .all(|l| l.local_addr().expect("addr").ip().is_loopback()),
            "the flip must bind loopback only when no tailscale address is present"
        );
    }

    #[test]
    fn preflight_reports_port_already_in_use() {
        // Hold a loopback port, then ask the pre-flight to bind the same one. The
        // loopback leg is REQUIRED, so the pre-flight FAILS (the flip is refused).
        let held = std::net::TcpListener::bind("127.0.0.1:0").expect("hold a port");
        let addr = held.local_addr().expect("held addr");

        let err = preflight_server_listeners(addr.port(), None)
            .expect_err("binding an in-use loopback port must fail pre-flight");
        let text = format!("{err:#}");
        assert!(
            text.contains("could not start the web server") && text.contains(&addr.to_string()),
            "collision error should name the address: {text}"
        );
    }

    #[test]
    fn preflight_best_effort_tailscale_bind_failure_degrades_to_loopback() {
        // Reproduce the real-world bug: a third-party process already holds the
        // Tailscale ip:port while loopback:port is free. The "Tailscale" leg is
        // best-effort, so the pre-flight must SUCCEED on loopback only, drop the
        // failed leg, and carry a warning naming the busy address.
        //
        // The whole 127.0.0.0/8 range is loopback on Linux, so a SECOND loopback
        // address (127.0.0.2) stands in for the Tailscale IP: hold 127.0.0.2:P,
        // leave 127.0.0.1:P free. local_addrs builds required(127.0.0.1:P) +
        // best_effort(127.0.0.2:P) — distinct addresses (no dedupe), so the bind
        // path is exercised exactly as production would hit it.
        let held = std::net::TcpListener::bind("127.0.0.2:0").expect("hold a second-loopback port");
        let held_addr = held.local_addr().expect("held addr");
        let port = held_addr.port();
        let ts_ip: std::net::IpAddr = "127.0.0.2".parse().unwrap();

        let (listeners, urls, warnings) = preflight_server_listeners(port, Some(ts_ip))
            .expect("a busy Tailscale leg must NOT fail the pre-flight");

        // Only the required loopback leg bound; the best-effort Tailscale leg was
        // dropped. Every bound listener is genuine loopback → host-only.
        assert_eq!(listeners.len(), 1, "the best-effort leg must be dropped");
        assert_eq!(urls.len(), 1, "only the bound listener gets a URL");
        let bound = listeners[0].local_addr().expect("bound addr");
        assert_eq!(bound.ip(), std::net::Ipv4Addr::LOCALHOST);
        assert!(
            urls.iter().all(|u| u.contains("127.0.0.1")),
            "the URL list must exclude the failed Tailscale address: {urls:?}"
        );
        // The warning names the busy address and the degrade-to-loopback outcome.
        assert_eq!(warnings.len(), 1, "exactly one bind warning: {warnings:?}");
        assert!(
            warnings[0].contains(&held_addr.to_string()),
            "the warning must name the busy Tailscale address: {}",
            warnings[0]
        );
        assert!(
            warnings[0].to_lowercase().contains("loopback"),
            "the warning must say it degraded to loopback: {}",
            warnings[0]
        );
    }

    #[test]
    fn build_left_items_does_not_split_below_threshold() {
        let projects = vec![
            test_project("project-1"),
            test_project("project-2"),
            test_project("project-3"),
            test_project("project-4"),
        ];
        let sessions = vec![test_session("session-1", "project-2", 0)];

        let items = build_left_items(&projects, &sessions, &HashSet::new(), &HashSet::new(), 5);

        assert_eq!(
            items,
            vec![
                LeftItem::Project(0),
                LeftItem::Project(1),
                LeftItem::Session(0),
                LeftItem::Project(2),
                LeftItem::Project(3),
            ]
        );
    }

    #[test]
    fn build_left_items_splits_empty_projects_at_threshold() {
        let projects = vec![
            test_project("project-1"),
            test_project("project-2"),
            test_project("project-3"),
            test_project("project-4"),
            test_project("project-5"),
        ];
        let sessions = vec![
            test_session("session-1", "project-2", 0),
            test_session("session-2", "project-4", 0),
        ];

        let items = build_left_items(&projects, &sessions, &HashSet::new(), &HashSet::new(), 5);

        assert_eq!(
            items,
            vec![
                LeftItem::Project(1),
                LeftItem::Session(0),
                LeftItem::Project(3),
                LeftItem::Session(1),
                LeftItem::EmptyProjectsSpacer,
                LeftItem::EmptyProjectsSeparator,
                LeftItem::Project(0),
                LeftItem::Project(2),
                LeftItem::Project(4),
            ]
        );
    }

    #[test]
    fn build_left_items_moves_project_above_separator_when_session_is_added() {
        let projects = vec![
            test_project("project-1"),
            test_project("project-2"),
            test_project("project-3"),
            test_project("project-4"),
            test_project("project-5"),
        ];
        let sessions = vec![
            test_session("session-1", "project-2", 0),
            test_session("session-2", "project-4", 0),
            test_session("session-3", "project-3", 0),
        ];

        let items = build_left_items(&projects, &sessions, &HashSet::new(), &HashSet::new(), 5);

        assert_eq!(
            items,
            vec![
                LeftItem::Project(1),
                LeftItem::Session(0),
                LeftItem::Project(2),
                LeftItem::Session(2),
                LeftItem::Project(3),
                LeftItem::Session(1),
                LeftItem::EmptyProjectsSpacer,
                LeftItem::EmptyProjectsSeparator,
                LeftItem::Project(0),
                LeftItem::Project(4),
            ]
        );
    }

    #[test]
    fn build_left_items_keeps_session_sort_order_within_project_grouping() {
        let projects = vec![
            test_project("project-1"),
            test_project("project-2"),
            test_project("project-3"),
            test_project("project-4"),
            test_project("project-5"),
        ];
        let mut sessions = vec![
            test_session("older", "project-2", 0),
            test_session("newer", "project-2", 10),
            test_session("other", "project-4", 5),
        ];
        sessions.sort_by_key(|session| std::cmp::Reverse(session.created_at));

        let items = build_left_items(&projects, &sessions, &HashSet::new(), &HashSet::new(), 5);

        assert_eq!(
            items,
            vec![
                LeftItem::Project(1),
                LeftItem::Session(0),
                LeftItem::Session(2),
                LeftItem::Project(3),
                LeftItem::Session(1),
                LeftItem::EmptyProjectsSpacer,
                LeftItem::EmptyProjectsSeparator,
                LeftItem::Project(0),
                LeftItem::Project(2),
                LeftItem::Project(4),
            ]
        );
    }

    #[test]
    fn build_left_items_can_disable_empty_project_split() {
        let projects = vec![
            test_project("project-1"),
            test_project("project-2"),
            test_project("project-3"),
            test_project("project-4"),
            test_project("project-5"),
        ];
        let sessions = vec![test_session("session-1", "project-2", 0)];

        let items = build_left_items(&projects, &sessions, &HashSet::new(), &HashSet::new(), 0);

        assert!(!items.contains(&LeftItem::EmptyProjectsSeparator));
        assert!(!items.contains(&LeftItem::EmptyProjectsSpacer));
        assert_eq!(items[0], LeftItem::Project(0));
    }

    #[test]
    fn build_left_items_omits_separator_when_no_project_has_sessions() {
        let projects = vec![
            test_project("project-1"),
            test_project("project-2"),
            test_project("project-3"),
            test_project("project-4"),
            test_project("project-5"),
        ];

        let items = build_left_items(&projects, &[], &HashSet::new(), &HashSet::new(), 5);

        assert_eq!(
            items,
            vec![
                LeftItem::Project(0),
                LeftItem::Project(1),
                LeftItem::Project(2),
                LeftItem::Project(3),
                LeftItem::Project(4),
            ]
        );
    }

    #[test]
    fn build_left_items_never_collapses_orphan_groups() {
        // A real (collapsible) project plus an orphan group — sessions whose
        // project record is gone. Both ids are in the collapsed set, but an orphan
        // group must stay expanded: it has no project header the user could click
        // to re-expand it, so collapsing would strand its sessions out of reach.
        let projects = vec![test_project("real")];
        let sessions = vec![
            test_session("real-s", "real", 0),
            test_session("ghost-s", "ghost", 0),
        ];
        let mut collapsed = HashSet::new();
        collapsed.insert("real".to_string());
        collapsed.insert("ghost".to_string());

        let items = build_left_items(&projects, &sessions, &HashSet::new(), &collapsed, 5);

        let session_id = |idx: &usize| sessions[*idx].id.as_str();
        // The real project is collapsed: header shown, its session hidden.
        assert!(items.contains(&LeftItem::Project(0)));
        assert!(
            !items
                .iter()
                .any(|i| matches!(i, LeftItem::Session(idx) if session_id(idx) == "real-s"))
        );
        // The orphan group is exempt: its header AND its session stay visible.
        assert!(
            items
                .iter()
                .any(|i| matches!(i, LeftItem::OrphanProject(_)))
        );
        assert!(
            items
                .iter()
                .any(|i| matches!(i, LeftItem::Session(idx) if session_id(idx) == "ghost-s"))
        );
    }

    #[test]
    fn config_only_project_is_synced_to_sqlite_and_preserved() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let root = dir.path().to_path_buf();
        let paths = DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            root: root.clone(),
        };
        std::fs::create_dir_all(&paths.worktrees_root).expect("worktrees");
        std::fs::write(
            &paths.config_path,
            r#"
[defaults]
provider = "codex"

[[projects]]
id = "project-1"
path = "$CODE/dux"
name = "dux"
default_provider = "claude"
leading_branch = "main"
"#,
        )
        .expect("write config");

        let mut config = ensure_config(&paths).expect("load config");
        let bindings = RuntimeBindings::from_keys_config(&config.keys);
        let store = SessionStore::open(&paths.sessions_db_path).expect("store");

        sync_config_projects_with_store(&mut config, &paths, &bindings, &store)
            .expect("sync projects");

        assert_eq!(config.projects.len(), 1);
        let projects = store.load_projects().expect("load projects");
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].id, "project-1");
        assert_eq!(projects[0].path, "$CODE/dux");
        assert_eq!(projects[0].name.as_deref(), Some("dux"));
        assert_eq!(projects[0].default_provider.as_deref(), Some("claude"));
        assert_eq!(projects[0].leading_branch.as_deref(), Some("main"));

        let saved = std::fs::read_to_string(&paths.config_path).expect("read config");
        assert!(saved.contains("[[projects]]"));
        assert!(saved.contains("project-1"));
        assert!(!saved.contains("leading_branch"));
    }

    #[test]
    fn sqlite_only_project_is_written_to_config() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let root = dir.path().to_path_buf();
        let paths = DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            root: root.clone(),
        };
        paths.ensure_dirs().expect("dirs");
        std::fs::write(&paths.config_path, "[defaults]\nprovider = \"codex\"\n").expect("config");
        let mut config = ensure_config(&paths).expect("load config");
        let bindings = RuntimeBindings::from_keys_config(&config.keys);
        let store = SessionStore::open(&paths.sessions_db_path).expect("store");
        store
            .upsert_project(&crate::config::ProjectConfig {
                id: "project-db".to_string(),
                path: root.join("repo").to_string_lossy().to_string(),
                name: Some("repo".to_string()),
                default_provider: Some("codex".to_string()),
                leading_branch: Some("main".to_string()),
                auto_reopen_agents: None,
                startup_command: Some("npm install".to_string()),
                env: Default::default(),
            })
            .expect("seed project");

        sync_config_projects_with_store(&mut config, &paths, &bindings, &store)
            .expect("sync projects");

        assert_eq!(config.projects.len(), 1);
        let saved = std::fs::read_to_string(&paths.config_path).expect("read config");
        assert!(saved.contains("id = \"project-db\""));
        assert!(saved.contains("startup_command = \"npm install\""));
        assert!(!saved.contains("leading_branch"));
    }

    #[test]
    fn config_project_backfills_missing_sqlite_optional_fields() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let root = dir.path().to_path_buf();
        let paths = DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            root: root.clone(),
        };
        paths.ensure_dirs().expect("dirs");
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        std::fs::write(
            &paths.config_path,
            format!(
                "[defaults]\nprovider = \"codex\"\n\n[[projects]]\nid = \"project-1\"\npath = \"{}\"\nname = \"repo\"\nleading_branch = \"main\"\n",
                repo.display()
            ),
        )
        .expect("config");
        let mut config = ensure_config(&paths).expect("load config");
        let bindings = RuntimeBindings::from_keys_config(&config.keys);
        let store = SessionStore::open(&paths.sessions_db_path).expect("store");
        store
            .upsert_project(&crate::config::ProjectConfig {
                id: "project-1".to_string(),
                path: repo.to_string_lossy().to_string(),
                name: Some("repo".to_string()),
                default_provider: None,
                leading_branch: None,
                auto_reopen_agents: None,
                startup_command: None,
                env: Default::default(),
            })
            .expect("seed project");

        sync_config_projects_with_store(&mut config, &paths, &bindings, &store)
            .expect("sync projects");

        let projects = store.load_projects().expect("load projects");
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].leading_branch.as_deref(), Some("main"));
        let saved = std::fs::read_to_string(&paths.config_path).expect("read config");
        assert!(!saved.contains("leading_branch"));
    }

    #[test]
    fn derived_project_leading_branch_is_persisted_to_sqlite_only() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let root = dir.path().to_path_buf();
        let paths = DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            root: root.clone(),
        };
        paths.ensure_dirs().expect("dirs");
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        std::process::Command::new("git")
            .arg("init")
            .arg(&repo)
            .output()
            .expect("git init");
        std::process::Command::new("git")
            .arg("checkout")
            .arg("-b")
            .arg("main")
            .current_dir(&repo)
            .output()
            .expect("git checkout main");
        std::fs::write(
            &paths.config_path,
            format!(
                "[defaults]\nprovider = \"codex\"\n\n[[projects]]\nid = \"project-1\"\npath = \"{}\"\nname = \"repo\"\n",
                repo.display()
            ),
        )
        .expect("config");
        let mut config = ensure_config(&paths).expect("load config");
        let bindings = RuntimeBindings::from_keys_config(&config.keys);
        let store = SessionStore::open(&paths.sessions_db_path).expect("store");
        store
            .upsert_project(&crate::config::ProjectConfig {
                id: "project-1".to_string(),
                path: repo.to_string_lossy().to_string(),
                name: Some("repo".to_string()),
                default_provider: None,
                leading_branch: None,
                auto_reopen_agents: None,
                startup_command: None,
                env: Default::default(),
            })
            .expect("seed project");

        sync_config_projects_with_store(&mut config, &paths, &bindings, &store)
            .expect("sync projects");
        let projects = load_projects(
            &store.load_projects().expect("load projects"),
            &store
                .load_project_created_ats()
                .expect("load project created_ats"),
            &config,
        );
        assert_eq!(projects[0].leading_branch.as_deref(), Some("main"));

        persist_runtime_projects_to_config_and_store(
            &projects,
            &mut config,
            &paths,
            &bindings,
            &store,
        )
        .expect("persist derived projects");

        let saved = std::fs::read_to_string(&paths.config_path).expect("read config");
        assert!(!saved.contains("leading_branch"));
        let stored = store.load_projects().expect("reload projects");
        assert_eq!(stored[0].leading_branch.as_deref(), Some("main"));
    }

    #[test]
    fn config_project_values_update_sqlite_on_sync() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let root = dir.path().to_path_buf();
        let paths = DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            root: root.clone(),
        };
        paths.ensure_dirs().expect("dirs");
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        std::fs::write(
            &paths.config_path,
            format!(
                "[defaults]\nprovider = \"codex\"\n\n[[projects]]\nid = \"project-1\"\npath = \"{}\"\nname = \"repo\"\nstartup_command = \"npm install\"\n",
                repo.display()
            ),
        )
        .expect("config");
        let mut config = ensure_config(&paths).expect("load config");
        let bindings = RuntimeBindings::from_keys_config(&config.keys);
        let store = SessionStore::open(&paths.sessions_db_path).expect("store");
        store
            .upsert_project(&crate::config::ProjectConfig {
                id: "project-1".to_string(),
                path: repo.to_string_lossy().to_string(),
                name: Some("repo".to_string()),
                default_provider: None,
                leading_branch: None,
                auto_reopen_agents: None,
                startup_command: Some("pnpm install".to_string()),
                env: Default::default(),
            })
            .expect("seed project");

        sync_config_projects_with_store(&mut config, &paths, &bindings, &store)
            .expect("sync projects");

        let projects = store.load_projects().expect("load projects");
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].startup_command.as_deref(), Some("npm install"));
    }

    #[test]
    fn build_visual_rows_respects_expansion() {
        let rows = vec![
            ResourceStats {
                id: None,
                kind: ResourceKind::Dux,
                label: "dux".into(),
                pid: Some(1),
                cpu_percent: 0.0,
                rss_bytes: 0,
                process_count: 1,
                children: Vec::new(),
            },
            ResourceStats {
                id: Some("s1".into()),
                kind: ResourceKind::Agent,
                label: "Agent".into(),
                pid: Some(100),
                cpu_percent: 5.0,
                rss_bytes: 1024,
                process_count: 3,
                children: vec![
                    ProcessInfo {
                        name: "node".into(),
                        pid: 101,
                        cpu_percent: 3.0,
                        rss_bytes: 512,
                        is_root: false,
                    },
                    ProcessInfo {
                        name: "claude".into(),
                        pid: 102,
                        cpu_percent: 2.0,
                        rss_bytes: 256,
                        is_root: false,
                    },
                ],
            },
            ResourceStats {
                id: None,
                kind: ResourceKind::Total,
                label: "TOTAL".into(),
                pid: None,
                cpu_percent: 5.0,
                rss_bytes: 1024,
                process_count: 4,
                children: Vec::new(),
            },
        ];

        // Nothing expanded: 3 visual rows (one per parent).
        let visual = build_visual_rows(&rows, &HashSet::new());
        assert_eq!(visual.len(), 3);

        // Expand PID 100: 3 parents + 2 children = 5 visual rows.
        let mut expanded = HashSet::new();
        expanded.insert(100);
        let visual = build_visual_rows(&rows, &expanded);
        assert_eq!(visual.len(), 5);
        assert!(matches!(visual[0], VisualRow::Parent(0)));
        assert!(matches!(visual[1], VisualRow::Parent(1)));
        assert!(matches!(visual[2], VisualRow::Child(1, 0)));
        assert!(matches!(visual[3], VisualRow::Child(1, 1)));
        assert!(matches!(visual[4], VisualRow::Parent(2)));

        // Expanding a PID that doesn't match any row: no effect.
        let mut expanded = HashSet::new();
        expanded.insert(999);
        let visual = build_visual_rows(&rows, &expanded);
        assert_eq!(visual.len(), 3);
    }

    #[test]
    fn project_worktree_visual_rows_separate_project_checkout() {
        let entries = vec![
            ProjectWorktreeEntry {
                path: PathBuf::from("/repo/managed"),
                branch_name: "feature".to_string(),
                is_managed_by_dux: true,
                existing_session_id: None,
                is_external: false,
                is_project_checkout: false,
                is_selectable: true,
            },
            ProjectWorktreeEntry {
                path: PathBuf::from("/repo/main"),
                branch_name: "main".to_string(),
                is_managed_by_dux: false,
                existing_session_id: None,
                is_external: true,
                is_project_checkout: true,
                is_selectable: false,
            },
        ];

        let rows = project_worktree_visual_rows(&entries, false, None);

        assert!(matches!(
            rows.first(),
            Some(ProjectWorktreeVisualRow::Header("Available Worktrees"))
        ));
        assert!(
            rows.iter()
                .any(|row| matches!(row, ProjectWorktreeVisualRow::Header("Project Checkout")))
        );
        assert_eq!(selectable_project_worktree_indices(&entries), vec![0]);
    }

    #[test]
    fn terminal_selection_ordered_forward() {
        let sel = TerminalSelection {
            anchor: TermGridPos { row: 2, col: 5 },
            end: TermGridPos { row: 4, col: 10 },
            dragging: false,
        };
        let (start, end) = sel.ordered();
        assert_eq!(start, TermGridPos { row: 2, col: 5 });
        assert_eq!(end, TermGridPos { row: 4, col: 10 });
    }

    #[test]
    fn terminal_selection_ordered_reverse() {
        let sel = TerminalSelection {
            anchor: TermGridPos { row: 4, col: 10 },
            end: TermGridPos { row: 2, col: 5 },
            dragging: false,
        };
        let (start, end) = sel.ordered();
        assert_eq!(start, TermGridPos { row: 2, col: 5 });
        assert_eq!(end, TermGridPos { row: 4, col: 10 });
    }

    #[test]
    fn terminal_selection_ordered_same_row() {
        let sel = TerminalSelection {
            anchor: TermGridPos { row: 3, col: 15 },
            end: TermGridPos { row: 3, col: 5 },
            dragging: false,
        };
        let (start, end) = sel.ordered();
        assert_eq!(start, TermGridPos { row: 3, col: 5 });
        assert_eq!(end, TermGridPos { row: 3, col: 15 });
    }

    #[test]
    fn terminal_selection_contains_single_row() {
        let sel = TerminalSelection {
            anchor: TermGridPos { row: 3, col: 5 },
            end: TermGridPos { row: 3, col: 10 },
            dragging: false,
        };
        assert!(sel.contains(3, 5));
        assert!(sel.contains(3, 7));
        assert!(sel.contains(3, 10));
        assert!(!sel.contains(3, 4));
        assert!(!sel.contains(3, 11));
        assert!(!sel.contains(2, 7));
        assert!(!sel.contains(4, 7));
    }

    #[test]
    fn terminal_selection_contains_multi_row() {
        let sel = TerminalSelection {
            anchor: TermGridPos { row: 2, col: 10 },
            end: TermGridPos { row: 4, col: 5 },
            dragging: false,
        };
        // First row: from anchor col to end of line.
        assert!(sel.contains(2, 10));
        assert!(sel.contains(2, 50));
        assert!(!sel.contains(2, 9));
        // Middle row: fully selected.
        assert!(sel.contains(3, 0));
        assert!(sel.contains(3, 100));
        // Last row: from start of line to end col.
        assert!(sel.contains(4, 0));
        assert!(sel.contains(4, 5));
        assert!(!sel.contains(4, 6));
        // Outside rows.
        assert!(!sel.contains(1, 10));
        assert!(!sel.contains(5, 0));
    }

    #[test]
    fn terminal_selection_contains_reverse_anchor() {
        // Anchor after end — should still work via ordered().
        let sel = TerminalSelection {
            anchor: TermGridPos { row: 4, col: 5 },
            end: TermGridPos { row: 2, col: 10 },
            dragging: false,
        };
        assert!(sel.contains(2, 10));
        assert!(sel.contains(3, 0));
        assert!(sel.contains(4, 5));
        assert!(!sel.contains(2, 9));
        assert!(!sel.contains(4, 6));
    }

    /// A reentrant config reload (one already in flight) returns an Info status
    /// and spawns no worker, so `reload_config_from_disk` must NOT set the
    /// "Reloading…" busy — doing so would clobber the Info and strand a spinner
    /// that nothing would ever clear.
    #[test]
    fn reentrant_reload_does_not_strand_a_busy() {
        let mut app = test_support::test_app(test_support::default_bindings());
        app.engine.reloading = true; // pretend a reload is already in flight

        app.reload_config_from_disk().expect("reload returns Ok");

        assert_ne!(
            app.status.tone(),
            crate::statusline::StatusTone::Busy,
            "a rejected reload must not show a busy spinner, got: {}",
            app.status.text(),
        );
        assert!(
            app.status.message().contains("already in progress"),
            "the engine's Info must survive, got: {}",
            app.status.message(),
        );
    }

    /// Quitting the TUI must SIGTERM the running agents (the analogue of the
    /// server's shutdown path) so they get a grace window to save state, rather
    /// than being hard-killed by `PtyClient::drop`. We drive the wind-down step
    /// directly because the full run loop needs a TTY.
    #[test]
    fn shutdown_agents_gracefully_terminates_running_provider() {
        let mut app = test_support::test_app(test_support::default_bindings());

        // `cat` ignores EOF-less stdin and runs until signalled, so it can only
        // be gone if the graceful SIGTERM actually reached it.
        let client =
            crate::pty::PtyClient::spawn("cat", &[], std::path::Path::new("/tmp"), 24, 80, 1000)
                .expect("spawn cat for test");
        app.engine.providers.insert("session-1".to_string(), client);

        app.shutdown_agents_gracefully();

        let client = app.engine.providers.get_mut("session-1").unwrap();
        assert!(
            client.is_exited() || client.try_wait().is_some(),
            "cat should have exited after the graceful SIGTERM on quit"
        );
        let session = app
            .engine
            .sessions
            .iter()
            .find(|s| s.id == "session-1")
            .unwrap();
        assert_eq!(session.status, SessionStatus::Detached);
    }

    #[test]
    fn shutdown_agents_gracefully_uses_the_top_level_timeout_not_server() {
        // Regression guard for the config wiring: the TUI quit path must read the
        // top-level `shutdown_timeout_seconds`, not `[server]` and not the 30s
        // default. We set the top-level value to 1s and the server value to a much
        // larger 20s, then time a quit with a SIGTERM-ignoring agent. If the call
        // site read the wrong field, the wait would be ~20s (or the 30s default)
        // instead of ~1s.
        let mut app = test_support::test_app(test_support::default_bindings());
        app.engine.config.shutdown_timeout_seconds = 1;
        app.engine.config.server.shutdown_timeout_seconds = 20;

        // `trap '' TERM` makes the shell ignore SIGTERM; `echo ready` marks the
        // trap as installed; the busy loop keeps it alive until force-killed.
        let client = crate::pty::PtyClient::spawn(
            "sh",
            &[
                "-c".to_string(),
                "trap '' TERM; echo ready; while true; do :; done".to_string(),
            ],
            std::path::Path::new("/tmp"),
            24,
            80,
            1000,
        )
        .expect("spawn sigterm-ignorer for test");
        app.engine.providers.insert("session-1".to_string(), client);

        // Wait until the trap is installed before quitting.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !app.engine.providers.get("session-1").unwrap().has_output() {
            assert!(
                std::time::Instant::now() < deadline,
                "ignorer never readied"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        let start = std::time::Instant::now();
        app.shutdown_agents_gracefully();
        let elapsed = start.elapsed();

        assert!(
            elapsed >= std::time::Duration::from_millis(900),
            "the 1s top-level grace should have been waited out, took {elapsed:?}"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "a ~1s top-level grace was configured, but the wait took {elapsed:?} — \
             the quit path likely read [server] (20s) or the 30s default instead"
        );
    }

    /// Proves the mechanism behind the TUI↔server graceful-shutdown handoff and
    /// why `serve_with_engine` must NOT reset SIGINT/SIGTERM to `SIG_DFL` on a
    /// flip-back. Both surfaces register through the same process-global
    /// `signal-hook-registry`, which installs its master OS handler exactly once
    /// per signal and routes a delivered signal to whatever actions are live.
    ///
    /// We use `SIGURG` (default action: ignore) so a *dormant* handler plus a
    /// `raise` cannot terminate the test process; a missed signal shows up as an
    /// unset flag, not a killed test. `raise` delivers synchronously to the
    /// calling thread, so the handler has run by the time it returns.
    #[test]
    fn signal_hook_master_handler_survives_reregistration_but_not_sig_dfl_reset() {
        // Phase 1, the flip we rely on: register, unregister (flip to server),
        // then register again (resume the TUI). The signal still reaches the
        // freshly registered flag, because the master handler stays installed.
        let first = Arc::new(AtomicBool::new(false));
        let first_id =
            signal_hook::flag::register(libc::SIGURG, Arc::clone(&first)).expect("register SIGURG");
        signal_hook::low_level::unregister(first_id);

        let after_resume = Arc::new(AtomicBool::new(false));
        let resume_id = signal_hook::flag::register(libc::SIGURG, Arc::clone(&after_resume))
            .expect("re-register SIGURG after a flip");
        unsafe { libc::raise(libc::SIGURG) };
        assert!(
            after_resume.load(Ordering::SeqCst),
            "a re-registered handler must still fire: this is what lets a resumed \
             TUI catch SIGTERM and wind agents down gracefully"
        );
        signal_hook::low_level::unregister(resume_id);

        // Phase 2, the regression guard: if the web server forced the OS
        // disposition back to SIG_DFL (as it used to via `libc::signal`), the
        // registry will NOT re-arm on the resume's register (the slot already
        // exists, so no fresh `sigaction`), leaving the TUI handler dormant.
        let pre_reset = Arc::new(AtomicBool::new(false));
        let pre_reset_id = signal_hook::flag::register(libc::SIGURG, Arc::clone(&pre_reset))
            .expect("register SIGURG before reset");
        signal_hook::low_level::unregister(pre_reset_id);
        unsafe { libc::signal(libc::SIGURG, libc::SIG_DFL) };

        let dormant = Arc::new(AtomicBool::new(false));
        let dormant_id = signal_hook::flag::register(libc::SIGURG, Arc::clone(&dormant))
            .expect("re-register SIGURG after a SIG_DFL reset");
        unsafe { libc::raise(libc::SIGURG) };
        assert!(
            !dormant.load(Ordering::SeqCst),
            "after a SIG_DFL reset the re-registration cannot re-arm the OS \
             disposition, so the handler is dormant: exactly why serve_with_engine \
             must not perform that reset"
        );
        signal_hook::low_level::unregister(dormant_id);
    }
}
