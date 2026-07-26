use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::io::{Write as _, stdout};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
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
    text_field_owns_key,
};
use crate::lockfile::SingleInstanceLock;
use crate::logger;
use crate::model::{
    AgentSession, AgentTab, ChangedFile, CompanionTerminalStatus, Project, ProjectBranchStatus,
    ProviderKind, SessionSurface,
};
// `Utc` and `SessionStatus` are now referenced only by the `#[cfg(test)]`
// submodules (via their `use super::*`): the non-test call sites that used them
// (the rename optimistic-title write and the exit-loop detach marking) moved
// into dux-core with the branch-rename and prune convergences. Gating the
// imports on `test` keeps them available to the tests without tripping the
// unused-import lint in the production build.
#[cfg(test)]
use crate::model::SessionStatus;
#[cfg(test)]
use chrono::Utc;

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
    /// Visible height and total line count of the first-load modal's content
    /// pane, recorded at render so the scroll keys can clamp (the same shape as
    /// `last_help_height`/`last_help_lines`, for the same reason: the extent
    /// depends on the wrap width, which only the renderer knows).
    pub(crate) last_first_load_height: u16,
    pub(crate) last_first_load_lines: u16,
    /// Visible height and total wrapped-row count of the error dialogs'
    /// (`ConfigReloadFailed`, `AddProjectFailed`) message pane, recorded at
    /// render so the scroll keys can clamp. Same shape and same reason as
    /// `last_help_height`/`last_help_lines`: the extent depends on the wrap
    /// width, which only the renderer knows. Only one such dialog can be open at
    /// a time, so the pair is shared.
    pub(crate) last_error_dialog_height: u16,
    pub(crate) last_error_dialog_lines: u16,
    /// The startup gate's plan, held from the moment the release-notes fetch is
    /// dispatched until the worker returns and
    /// [`dux_core::first_load::after_fetch`] folds the outcome in. `None` once
    /// consumed; a `Welcome` or `Nothing` plan never lands here because neither
    /// needs the network.
    pub(crate) pending_first_load: Option<dux_core::first_load::FirstLoadPlan>,
    /// Payload channel for the in-flight release-notes worker. The keyed
    /// busy→final status rides the engine's own worker channel; only the notes
    /// themselves come back here. `Some` means a fetch is in flight, which is
    /// what stops the palette command from starting a second one.
    pub(crate) notes_fetch_rx: Option<mpsc::Receiver<NotesFetched>>,
    /// Notes that arrived while the user had a DIFFERENT modal open.
    /// `PromptState` is a single slot, so showing the what's-new screen the
    /// instant the fetch lands would discard whatever the user was typing. The
    /// notes are parked here (with `pending_first_load` left un-consumed and the
    /// version deliberately UNSTAMPED, per the stamp-on-dismissal contract) and
    /// re-offered on a later tick once the prompt slot is free.
    pub(crate) deferred_first_load_notes: Option<Box<dux_core::release_notes::ReleaseNotes>>,
    /// "Someone is now explicitly waiting on the in-flight release-notes fetch."
    ///
    /// Shared with the worker thread, because the fetch's `NotesFetchPurpose` is
    /// baked into its status closures by `move` at spawn time and cannot be
    /// changed from here afterwards. `show-release-notes` sets it when it finds a
    /// fetch already running, and both the failure closure and
    /// `apply_notes_fetch` read it so the user's explicit request fails loudly
    /// instead of being swallowed by the automatic path's silence. A fresh `Arc`
    /// is minted per fetch, so the flag can never be stale.
    pub(crate) notes_fetch_explicit_request: Arc<AtomicBool>,
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
    /// The one-shot "this modal is refusing to close" cue, armed by an outside
    /// click on a modal whose outside-click policy is
    /// [`overlay_dismiss::OutsideClickPolicy::Blink`]. `None` means no cue is
    /// running; see [`RefusalBlink`] for why it also remembers which modal
    /// armed it.
    pub(crate) refusal_blink: Option<RefusalBlink>,
    pub(crate) readonly_nudge_tick: Option<u64>,
    /// Whether the flat list's "Inactive" tail (detached/exited agents) is
    /// collapsed. Starts collapsed, matching the web. Replaces the old
    /// per-project collapse now that there are no project headers.
    pub(crate) inactive_collapsed: bool,
    /// Once the user toggles the Inactive section by hand, stop auto-managing its
    /// collapsed state. Until then it auto-expands when every agent is inactive
    /// (so a wholly-dormant workspace is not hidden) and collapses when any agent
    /// is active. See `rebuild_left_items`.
    pub(crate) inactive_collapse_overridden: bool,
    /// The normalized filter query under which the user explicitly collapsed a
    /// search-expanded Inactive tail. While the agent filter has a hit inside
    /// the tail, the tail renders open as DERIVED state (the collapse
    /// preference is untouched); collapsing it during that query is an explicit
    /// act recorded here, winning until the query changes (a changed or cleared
    /// query expires it in `rebuild_left_items`). Mirrors the web QuietTail's
    /// `dismissedQuery`.
    pub(crate) inactive_search_dismissed: Option<String>,
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
    /// against the handler-computed [`dux_core::engine::LaunchOutcome`], reproducing the exact
    /// final wording. Create-kind launches are NOT routed through this map: their
    /// busy/final ride the SHARED engine-side create op
    /// (`Engine::pending_create_ops`), resolved engine-side to a keyed `Status` that
    /// both surfaces apply.
    pub(crate) pending_reconnect_ops:
        HashMap<String, dux_core::engine::HandlerStatusOp<dux_core::engine::LaunchOutcome>>,
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
    /// The project the `manage-projects` chooser picked, if any. When set (and
    /// the project still exists), `selected_project()` resolves to it instead of
    /// the selected agent's project, so project-scoped palette commands act on
    /// the chosen project. Cleared when the user navigates to a different agent
    /// row, so it never silently hijacks selection.
    pub(crate) project_chooser_context: Option<String>,
    /// The live agent-list filter, mirroring the web sidebar/hub search box.
    /// `Some` means filter mode is active: a one-line search input renders at the
    /// top of the left pane and printable keys type into this query, live-filtering
    /// the flat list via `dux_core::agent_search::matches_session`. `None` means the
    /// full, unfiltered list is shown. This is a DISPLAY filter only: it never
    /// mutates `engine.sessions`, never persists, and composes with the sort mode.
    pub(crate) agent_filter: Option<TextInput>,
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

// The reconnect / fresh-restart status-op outcome is the CORE-owned
// `dux_core::engine::LaunchOutcome`, and its message mapping is the core-owned
// `dux_core::engine::launch_outcome_final`, shared byte-for-byte with the web
// (the TUI's own reconnect-outcome enum and `reconnect_final` mapper were
// deleted in favor of that single source).

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
    /// Search + hovered-row state. `list.selected` is the hovered visible index;
    /// multi-select (`selected_ids`) and `focus` stay separate.
    pub(crate) list: SearchableList,
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
    pub(crate) mode: ChangeAgentProviderMode,
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

// The branch-drift predicate is the core-owned `dux_core::agent_tabs::branch_drifted`
// (cross-language twin of the web's `branchDrift`, pinned by shared vectors).
pub(crate) use dux_core::agent_tabs::branch_drifted;

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

/// A running "this modal is refusing to close" cue.
///
/// Wall-clock only: the phase is a function of `started.elapsed()`, never of
/// `tick_count`, so the cue runs at the same speed whatever the frame cadence
/// is (the animations tenet). It expires by itself once the elapsed time passes
/// [`overlay_dismiss::REFUSAL_BLINK_MS`], which is what guarantees it ends at
/// rest instead of freezing mid-cycle.
///
/// `prompt` records WHICH modal armed the cue. Without it a blink armed on one
/// modal would keep flashing a different modal opened within the cue's lifetime
/// (dismiss the refusing modal by Esc, open another straight away, and the new
/// one would inherit the flash). The two `EditMacros` arms share a discriminant,
/// so a blink on the macro editor can bleed onto the delete-confirm raised out
/// of it inside the same 800ms; that is a deliberate non-problem, since the user
/// only gets there by opening the confirm themselves.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RefusalBlink {
    pub(crate) started: Instant,
    pub(crate) prompt: std::mem::Discriminant<PromptState>,
}

/// Build the body lines of the Agent Info modal from a session: name, provider,
/// the current/original/forked-from branches, a drift note when the current
/// branch differs from the branch the agent was created on, then the worktree,
/// creation time, and status. Each line carries its semantic tone so the
/// renderer never has to substring-match prose. Pure and unit-tested.
pub(crate) fn agent_info_lines(
    session: &AgentSession,
    project_default: Option<ProviderKind>,
) -> Vec<(String, AgentInfoTone)> {
    let name = session
        .title
        .clone()
        .unwrap_or_else(|| session.branch_name.clone());
    // Mirror the header's "current provider" note: when the agent runs a provider
    // other than its project's default, spell out the divergence here too.
    let provider_line = match project_default {
        Some(default) if default != session.provider => format!(
            "Provider:     {} (project default: {})",
            session.provider.as_str(),
            default.as_str()
        ),
        _ => format!("Provider:     {}", session.provider.as_str()),
    };
    let mut lines = vec![
        (format!("Name:         {name}"), AgentInfoTone::Neutral),
        (provider_line, AgentInfoTone::Neutral),
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

/// Why the project chooser modal is open. A flat agent list has no project
/// header to select, so every project-scoped creation entry point (and the
/// palette's `manage-projects`) routes through the chooser, which lists ALL
/// projects (agent-less included). The intent decides what confirming a project
/// does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProjectChooserIntent {
    /// Create a new agent in the chosen project.
    NewAgent,
    /// Create a new agent from a GitHub PR in the chosen project.
    FromPr,
    /// Create a new agent from an existing worktree of the chosen project.
    FromWorktree,
    /// Make the chosen project the target for project-scoped palette commands.
    Manage,
    /// Spawn a project-owned terminal at the chosen project's repo root.
    ProjectTerminal,
}

impl ProjectChooserIntent {
    /// Modal title reflecting the pending action.
    pub(crate) fn title(self) -> &'static str {
        match self {
            ProjectChooserIntent::NewAgent => "New agent in project",
            ProjectChooserIntent::FromPr => "New agent from PR",
            ProjectChooserIntent::FromWorktree => "New agent from worktree",
            ProjectChooserIntent::Manage => "Manage project",
            ProjectChooserIntent::ProjectTerminal => "New terminal in project",
        }
    }
}

/// One row in the project chooser: a project plus derived, display-only facts
/// (its agent count and whether its path is missing). Built from the live
/// `engine.projects` + `engine.sessions`; never persisted.
#[derive(Clone, Debug)]
pub(crate) struct ProjectChooserEntry {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) agent_count: usize,
    pub(crate) path_missing: bool,
}

/// Leave a search row, discarding whatever was typed into it.
///
/// The ONE Escape semantics every filterable modal in dux shares: the close key
/// leaves search mode and clears the query in the SAME press, so the list comes
/// back whole and the next close key shuts the modal. There used to be a middle
/// state (leave search but keep filtering, then a second press to unfilter,
/// then a third to close), which meant a user who typed a query had to press
/// the close key three times to get out of a two-line dialog.
///
/// Returns `true` when there was something to leave, which is exactly the
/// caller's "stay open" signal; `false` means the search row was already empty
/// and idle, so the press belongs to the modal and should close it.
///
/// Two of the five callers hold a [`SearchableList`] and go through
/// [`SearchableList::exit_search_clearing_filter`]; the other three keep their
/// `searching` flag and their query in separate fields for reasons of their own
/// (the startup-log picker's `selected` is an ABSOLUTE entry index rather than a
/// visible one, and the fullscreen log viewer has no row selection at all), so
/// they call this directly. One definition, three call shapes.
pub(crate) fn exit_search_clearing_filter(searching: &mut bool, filter: &mut TextInput) -> bool {
    let had_something = *searching || !filter.is_empty();
    *searching = false;
    filter.clear();
    had_something
}

/// Shared search + selection state for filterable list modals (the project
/// chooser, the kill-running dialog, …). Owns the `/` query, whether search mode
/// is active, and the selection index INTO THE VISIBLE (filtered) list. Each
/// modal supplies its own per-item match predicate, so field semantics stay
/// modal-specific while the fiddly filter/clamp/toggle mechanics live in one
/// place. `selected` always indexes the visible list, never the full item list.
#[derive(Clone, Debug, Default)]
pub(crate) struct SearchableList {
    pub(crate) filter: TextInput,
    pub(crate) searching: bool,
    pub(crate) selected: usize,
}

impl SearchableList {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// True when a search row should be shown: search mode is active, or a query
    /// has been committed (so the filtered result stays visible).
    pub(crate) fn is_filtering(&self) -> bool {
        self.searching || !self.filter.is_empty()
    }

    /// Indices of `items` matching the current query. `matches(item, needle)` is
    /// the modal's predicate; `needle` is trimmed and lowercased. An empty query
    /// matches everything and preserves order.
    pub(crate) fn visible_indices<T>(
        &self,
        items: &[T],
        matches: impl Fn(&T, &str) -> bool,
    ) -> Vec<usize> {
        let needle = self.filter.text.trim().to_lowercase();
        items
            .iter()
            .enumerate()
            .filter(|(_, it)| needle.is_empty() || matches(it, &needle))
            .map(|(i, _)| i)
            .collect()
    }

    /// Enter search mode, cursor at the end of any existing query.
    pub(crate) fn begin_search(&mut self) {
        self.filter.move_end();
        self.searching = true;
    }

    /// Leave search mode AND drop the query, resetting the selection to the top
    /// of the restored list (`selected` indexes the VISIBLE list, so the index
    /// it held meant something different a moment ago).
    ///
    /// See [`exit_search_clearing_filter`] for why this is one press.
    pub(crate) fn exit_search_clearing_filter(&mut self) -> bool {
        let left = exit_search_clearing_filter(&mut self.searching, &mut self.filter);
        if left {
            self.selected = 0;
        }
        left
    }

    pub(crate) fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub(crate) fn move_down(&mut self, visible_len: usize) {
        if self.selected + 1 < visible_len {
            self.selected += 1;
        }
    }

    /// Keep `selected` within `[0, visible_len)`. Call after the query changes.
    pub(crate) fn clamp_selected(&mut self, visible_len: usize) {
        if self.selected >= visible_len {
            self.selected = visible_len.saturating_sub(1);
        }
    }
}

/// The project chooser's match predicate: case-insensitive substring over the
/// project name or its path. `needle` is expected pre-lowercased.
pub(crate) fn pick_project_matches(entry: &ProjectChooserEntry, needle: &str) -> bool {
    entry.name.to_lowercase().contains(needle) || entry.path.to_lowercase().contains(needle)
}

/// The kill-running dialog's match predicate: case-insensitive substring over a
/// runtime's precomputed `search_text` (agent name + project + provider + noun).
/// `needle` is expected pre-lowercased.
pub(crate) fn kill_running_matches(runtime: &KillableRuntime, needle: &str) -> bool {
    runtime.search_text.to_lowercase().contains(needle)
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
    pub(crate) focus: ConfirmFocus,
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

/// Which selectable element has focus in the Rename Agent modal.
///
/// Mirrors [`NameNewAgentFocus`]: the modal pairs a single-line name field with
/// a checkbox, so movement keys need somewhere to move focus TO. Without this,
/// the movement action had to be wired straight to the checkbox value, which
/// made Tab flip the box instead of highlighting it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RenameSessionFocus {
    Input,
    RenameBranchCheckbox,
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
        /// First visible row of the message. The failure text can run long (a
        /// path plus a git error, several lines of it), and every line matters
        /// when it is how the user learns why the project was rejected, so the
        /// body scrolls instead of being truncated. Clamped at render against
        /// [`App::last_error_dialog_lines`]/[`App::last_error_dialog_height`].
        scroll: u16,
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
        focus: ConfirmFocus, // Cancel (default) or Initialize & Add
        /// The prompt to restore on cancel (the project browser with the
        /// user's location and typed path intact), the `AddProjectFailed`
        /// pattern.
        return_prompt: Box<PromptState>,
    },
    ChangeAgentProvider(ChangeAgentProviderPrompt),
    AgentInfo(AgentInfoPrompt),
    /// One of the two first-load screens (first-run welcome, or what's-new after
    /// a version change). They share one modal frame; see
    /// [`crate::app::first_load`]. Routed through `PromptState` so `Esc` and the
    /// generic overlay dismissal keep working uniformly — the one addition is
    /// that dismissal stamps the running version as seen when the plan says to.
    FirstLoad(FirstLoadPrompt),
    ChangeDefaultProvider(ChangeDefaultProviderPrompt),
    ChangeProjectDefaultProvider(ChangeProjectDefaultProviderPrompt),
    ChangeTheme(ChangeThemePrompt),
    ConfigureStartupCommand {
        project_id: String,
        project_name: String,
        input: TextInput,
        focus: ConfigureFieldFocus,
    },
    ConfigureProjectEnv {
        project_id: String,
        project_name: String,
        input: TextInput,
        focus: ConfigureFieldFocus,
    },
    ConfigureGlobalEnv {
        project_name: String,
        input: TextInput,
        focus: ConfigureFieldFocus,
    },
    #[allow(dead_code)]
    /// The windowed startup-log browser.
    ///
    /// **Nothing outside `#[cfg(test)]` constructs this today.** The
    /// "read startup command logs" journey resolves
    /// `EventReaction::StartupLogArrived`, which opens the FULLSCREEN viewer
    /// (`FullscreenOverlay::StartupLog` + [`App::startup_log_viewer`])
    /// instead, so this modal is unreachable from the UI. It still renders and
    /// still handles keys, and the modal registry still covers it, but do not
    /// read a green suite here as evidence that a user can open it. Pinned by
    /// `input::tests::the_startup_log_journey_opens_the_fullscreen_viewer_not_the_modal`.
    ///
    /// Consequence worth knowing before "fixing" it: it has four interactive
    /// regions (the filter, the Runs list, the Output body, the Close button)
    /// and no focus concept, so its Close button is unreachable by keyboard.
    /// That is a real gap in a surface no user can reach; whether to wire a
    /// focus model or delete the variant is an owner's decision, not a
    /// tidy-up.
    StartupCommandLogs(StartupCommandLogPrompt),
    /// The project chooser: lists every project (agent-less included) so a
    /// project-scoped action can target one when the flat agent list has no
    /// project header to select. `selected` indexes the VISIBLE (filtered) list,
    /// not `entries`; resolve it through [`visible_pick_project_indices`].
    PickProject {
        intent: ProjectChooserIntent,
        entries: Vec<ProjectChooserEntry>,
        /// Search + selection state; `selected` indexes the visible (filtered)
        /// list. `/`-search over project name + path (parity with the web).
        list: SearchableList,
    },
    PickProjectWorktree(PickProjectWorktreePrompt),
    KillRunning(KillRunningPrompt),
    ConfirmKillRunning(ConfirmKillRunningPrompt),
    ConfigReloadFailed {
        error: String,
        recover_old_config: bool,
        focus: ConfigReloadFailedFocus,
        /// First visible row of the validation error. A TOML validation failure
        /// is normally many lines and the tail is usually the part naming the
        /// actual problem, so the body scrolls rather than dropping it. Clamped
        /// at render against
        /// [`App::last_error_dialog_lines`]/[`App::last_error_dialog_height`].
        scroll: u16,
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
        focus: ConfirmFocus, // Cancel (default) or Delete
    },
    /// Close/detach an agent tab. Closing the session-slot tab (`is_main`) detaches the
    /// agent (non-destructive, stays in Projects); closing an extra tab ends
    /// that session for good (destructive), so it defaults to Cancel.
    ConfirmCloseTab {
        session_id: String,
        tab_id: String,
        provider_label: String,
        is_main: bool,
        focus: ConfirmFocus, // Cancel (default) or Close/Detach
    },
    ConfirmQuit {
        agent_count: usize,
        terminal_count: usize,
        focus: ConfirmFocus, // Cancel (default) or Quit
    },
    ConfirmDiscardFile {
        file_path: String,
        // Deliberately NO tracked/untracked flag here: discard is destructive
        // (delete an untracked file vs restore a tracked one from HEAD), and the
        // file's state can change between when this prompt opens and when the user
        // confirms (an agent may be mutating the worktree). The classification is
        // therefore re-derived from LIVE git status at confirm time via
        // `git::discard_classify`, never snapshotted here.
        focus: ConfirmFocus, // Cancel (default) or Discard
    },
    /// Shown when adding a project whose repo has no commits yet (a fresh
    /// `git init` with an unborn HEAD). Confirming creates an empty initial
    /// commit so the repo can back worktrees, then registers the project.
    ConfirmCreateInitialCommit {
        /// Absolute, validated path of the repo being added.
        path: String,
        /// Display name entered by the user (empty → derived from the path).
        name: String,
        focus: ConfirmFocus, // Cancel (default) or Create & Add
    },
    RenameSession {
        session_id: String,
        input: TextInput,
        rename_branch: bool,
        focus: RenameSessionFocus,
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
        focus: ConfirmFocus, // Cancel (default) or Use Existing
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
    pub(crate) focus: MacroEditFocus,
}

/// Which control has focus in the macro editor.
///
/// The macro editor used to be a one-way two-stage wizard (`EditName` then
/// `EditText`) where Escape SAVED from the second stage and the surface
/// selector was unreachable once you left the first. It is now an ordinary
/// modal: every control is a focus stop, movement keys move between them and
/// change nothing, Space acts on whichever one has focus, and Escape cancels.
///
/// Declared in visual order, which is also the forward focus order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MacroEditFocus {
    /// The single-line name field. Typing is immediate: there is no mode,
    /// because nothing a single-line field consumes collides with a modal key.
    Name,
    /// The multiline body. Needs the engage step (see
    /// [`App::macro_text_engaged`]) because Enter is CONTENT here and is also
    /// every modal's confirm key, so the two cannot share an unmoded field.
    Text,
    /// The Agent/Terminal/Both selector. Space advances it; movement keys move
    /// past it without touching its value.
    Surface,
    Cancel,
    Save,
}

impl MacroEditFocus {
    /// Visual order, which is also the forward focus order.
    pub(crate) const ORDER: [MacroEditFocus; 5] = [
        MacroEditFocus::Name,
        MacroEditFocus::Text,
        MacroEditFocus::Surface,
        MacroEditFocus::Cancel,
        MacroEditFocus::Save,
    ];

    /// The next focus stop in `forward` direction, wrapping at both ends.
    pub(crate) fn step(self, forward: bool) -> Self {
        let order = Self::ORDER;
        let index = order
            .iter()
            .position(|f| *f == self)
            .expect("every variant is in ORDER");
        let next = if forward {
            (index + 1) % order.len()
        } else {
            (index + order.len() - 1) % order.len()
        };
        order[next]
    }

    /// Whether this stop is actually TAKING keystrokes right now.
    ///
    /// The single-line name field always is: typing there is immediate. The
    /// multiline body only is once it has been engaged, and an unengaged body
    /// owns nothing, so the movement keys keep working while focus is parked on
    /// it. That distinction is what [`MacroEditFocus::is_text_field`] cannot
    /// make on its own.
    pub(crate) fn owns_keys(self, body_engaged: bool) -> bool {
        match self {
            MacroEditFocus::Name => true,
            MacroEditFocus::Text => body_engaged,
            MacroEditFocus::Surface | MacroEditFocus::Cancel | MacroEditFocus::Save => false,
        }
    }
}

/// Which control has focus in the three `Configure*` modals (startup command,
/// project environment, global environment).
///
/// All three hold ONE full-text field, so Enter cannot mean "submit": it is
/// content the moment the field is engaged. The field therefore keeps the
/// engage step, and the modal carries the Cancel/Save pair that gives Enter its
/// third, unambiguous meaning. That is the dual-mode rule, and these three used
/// to be the only modals breaking it.
///
/// Declared in visual order, which is also the forward focus order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ConfigureFieldFocus {
    /// The full-text body. Unengaged it takes no keystrokes at all; engaging it
    /// is an explicit act (the confirm key, the engage binding, or a double
    /// click on the field).
    #[default]
    Input,
    Cancel,
    Save,
}

impl ConfigureFieldFocus {
    /// The next focus stop in `forward` direction, wrapping at both ends.
    /// Every stop is unconditional here, but the shared ring is used anyway so
    /// this modal's focus order is declared as data like everyone else's.
    pub(crate) fn step(self, forward: bool) -> Self {
        let ring: [(ConfigureFieldFocus, bool); 3] = [
            (ConfigureFieldFocus::Input, true),
            (ConfigureFieldFocus::Cancel, true),
            (ConfigureFieldFocus::Save, true),
        ];
        components::focus_ring::next_focus(&ring, self, forward)
    }
}

/// Which control has focus in a two-button confirmation.
///
/// Nine confirmations used to carry a bare `confirm_selected: bool` for this.
/// Two states is the right CARDINALITY (there are exactly two buttons), so the
/// defect was never the arity: it was that a `bool` cannot SAY which control
/// has focus, and `confirm_selected: false` at a construction site reads as a
/// checkbox that is off rather than as focus resting on Cancel. Every other
/// modal in dux names its focus with an enum, and these do now too; the shared
/// type is what stops nine near-identical `…Focus` enums appearing instead.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ConfirmFocus {
    /// The safe default: a confirmation opens with Cancel focused.
    #[default]
    Cancel,
    /// The committing button (Delete, Quit, Discard, Use Existing, …).
    Confirm,
}

impl ConfirmFocus {
    /// Move focus to the other button. A two-control focus ring is its own
    /// inverse, so forwards and backwards are the same step.
    pub(crate) fn toggled(self) -> Self {
        match self {
            Self::Cancel => Self::Confirm,
            Self::Confirm => Self::Cancel,
        }
    }

    /// Whether the committing button is the one focus is on.
    pub(crate) fn is_confirm(self) -> bool {
        matches!(self, Self::Confirm)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PendingMacroDelete {
    pub(crate) name: String,
    pub(crate) focus: ConfirmFocus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InputTarget {
    None,
    Agent,
    Terminal,
    CommitMessage,
    StartupCommand,
    /// The macro editor's multiline body is engaged for typing. Its own variant
    /// rather than a reuse of `StartupCommand` so a future reader cannot mistake
    /// one modal's engage state for the other's.
    MacroText,
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

#[derive(Clone, Debug, Default)]
pub(crate) struct MouseLayoutState {
    pub(crate) body: Rect,
    pub(crate) left: Rect,
    pub(crate) center: Rect,
    pub(crate) right: Rect,
    pub(crate) left_list: Rect,
    /// Screen-row (relative to `left_list.y`) -> left-item index, rebuilt each
    /// render. The flat list's agent rows are two lines tall, so a click row no
    /// longer maps 1:1 to an item; this is the reverse map (see
    /// `render::left_row_to_item`).
    pub(crate) left_row_to_item: Vec<usize>,
    pub(crate) terminal_list: Rect,
    /// Screen-row (relative to `terminal_list.y`) -> terminal-item index, rebuilt
    /// each render. Terminal rows are now three lines tall (matching the agent
    /// rows), so a click row no longer maps 1:1 to a terminal; this is the
    /// reverse map (see `render::left_row_to_item`, reused for both lists).
    pub(crate) terminal_row_to_item: Vec<usize>,
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
        self.left_row_to_item.clear();
        self.terminal_list = Rect::default();
        self.terminal_row_to_item.clear();
        self.agent_term = None;
        self.unstaged_list = None;
        self.staged_list = None;
        self.commit_area = None;
        self.commit_text_area = None;
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct OverlayMouseLayoutState {
    pub(crate) active: OverlayMouseLayout,
    /// Outer rect of the TOPMOST modal painted this frame, or `None` when no
    /// modal was painted. Recorded by the one chokepoint every modal passes
    /// through, [`App::clear_overlay_area`], and read on a LATER event by the
    /// click-outside dismissal engine (see [`super::overlay_dismiss`]).
    ///
    /// Two properties are load-bearing:
    ///
    /// * It FAILS CLOSED. `None` means "no dismissal", never "dismiss on any
    ///   click": a prompt can be open while a fullscreen overlay is up, in
    ///   which case `render_overlay` returns before `render_prompt` and no rect
    ///   is recorded, yet the mouse still routes to prompt handling. A
    ///   zero-sized `Rect` sentinel would be a silent trap here, so the
    ///   `Option` is deliberate.
    /// * Last write wins, which is what makes nested modals work: the macro
    ///   editor paints its popup and THEN its nested delete-confirm paints a
    ///   smaller rect, so `frame` ends up as the modal actually on top.
    ///
    /// A [`Cell`] rather than a plain field because the recording happens from
    /// `&self`: `render_prompt` matches on `&self.prompt` and paints from
    /// inside those arms, so a `&mut self` chokepoint is not expressible there
    /// (measured: 30 borrow-checker errors across the render arms).
    pub(crate) frame: Cell<Option<Rect>>,
}

impl OverlayMouseLayoutState {
    pub(crate) fn reset(&mut self) {
        self.active = OverlayMouseLayout::None;
        self.frame.set(None);
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
    /// The first-load modal's two one-row pill buttons.
    FirstLoad {
        primary_button: Rect,
        secondary_button: Rect,
    },
    ChangeAgentProvider {
        list: Rect,
        items: usize,
        offset: usize,
    },
    ChangeDefaultProvider {
        list: Rect,
        items: usize,
        offset: usize,
    },
    ChangeProjectDefaultProvider {
        list: Rect,
        items: usize,
        offset: usize,
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
    PickProject {
        /// The `/`-search field, published only while it is drawn, so a click
        /// can land the caret in it.
        input: Option<Rect>,
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
        // No outer `area` here: the modal's outer rect is recorded once, for
        // every modal, in `OverlayMouseLayoutState::frame` (the click-outside
        // dismissal engine's store), so a per-variant copy would be a second
        // source of truth for the same rect.
        /// The filter field, published only while it is drawn.
        input: Option<Rect>,
        list: Rect,
        body: Rect,
        items: usize,
        offset: usize,
        close_button: Rect,
    },
    /// Shared by all three `Configure*` modals: one full-text field plus the
    /// Cancel/Save pair the dual-mode rule requires of it.
    ConfigureStartupCommand {
        input: Rect,
        cancel_button: Rect,
        save_button: Rect,
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
    /// The create-agent-from-PR modal's single text field.
    PullRequestInput {
        input: Rect,
    },
    NameNewAgent {
        input: Rect,
        checkbox: Option<OverlayCheckbox>,
        /// The "copy uncommitted changes" checkbox; present only for
        /// `CreateAgentRequest::NewProject` prompts.
        copy_checkbox: Option<OverlayCheckbox>,
    },
    /// The macro EDITOR (not the macro list, which has no click targets of its
    /// own, and not the nested delete-confirm, which publishes
    /// `ConfirmDeleteMacro` over the top of this).
    ///
    /// The two input rects are the fields' INNER areas, so a click maps
    /// straight onto a text position. `surface_options` is one rect per
    /// [`MacroSurface`] variant in `Agent, Terminal, Both` order, so clicking
    /// one selects exactly that option rather than advancing the cycle.
    /// The macro LIST: a Picker, so it publishes its rows and nothing else.
    /// (The EDITOR that opens on top of it publishes
    /// [`OverlayMouseLayout::EditMacros`] instead.)
    EditMacroList {
        list: Rect,
        items: usize,
        offset: usize,
    },
    EditMacros {
        name_input: Rect,
        text_input: Rect,
        surface_options: [Rect; 3],
        cancel_button: Rect,
        save_button: Rect,
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
    MacroTextInput,
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
    /// An agent row (index into `engine.sessions`). The flat model shows the
    /// project inline on the row rather than under a header.
    Session(usize),
    /// The collapsible "Inactive · N" toggle separating active agents (above) from
    /// detached/exited ones (below). Selectable; Enter/Space toggles the tail.
    InactiveToggle,
}

impl LeftItem {
    pub(crate) fn is_selectable(self) -> bool {
        matches!(self, LeftItem::Session(_) | LeftItem::InactiveToggle)
    }
}

/// The agent-list display sort mode, driven by the shared `config.ui.agent_sort`
/// preference. This is a pure DISPLAY ordering computed at render time: the TUI
/// never rewrites `engine.sessions` and never persists a sort order (that stored
/// order IS the `Manual` display order, set only by the web's drag-reorder).
///
/// The TUI's own picker OFFERS the five non-manual modes (see `TUI_CYCLE`) but
/// DISPLAYS `Manual` when the web set it. The web mirror OFFERS
/// active/updated/created/name/manual and DISPLAYS a TUI-set `NameDesc`. The
/// shared value set therefore has six modes; both surfaces render all six.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AgentSortMode {
    /// Working / needs-attention agents float to the top (a stable float, not a
    /// re-sort), each group keeping incoming order. The default.
    Active,
    /// Most recently updated first (`Reverse(updated_at)`).
    Updated,
    /// Most recently created first (`Reverse(created_at)`).
    Created,
    /// By name (title-or-branch, case-insensitive), A to Z.
    NameAsc,
    /// By name (title-or-branch, case-insensitive), Z to A.
    NameDesc,
    /// The stored global order verbatim (the web's drag-reorder order). Display
    /// only in the TUI, which never offers it.
    Manual,
}

impl AgentSortMode {
    /// The five modes the TUI's `sort-agents` picker cycles through. `Manual` is
    /// deliberately excluded (it is display-only, set only by the web's drag).
    pub(crate) const TUI_CYCLE: [AgentSortMode; 5] = [
        AgentSortMode::Active,
        AgentSortMode::Updated,
        AgentSortMode::Created,
        AgentSortMode::NameAsc,
        AgentSortMode::NameDesc,
    ];

    /// Parse the shared `config.ui.agent_sort` string. Unknown values fall back to
    /// `Active` (the default), matching the web's tolerance for a value it does
    /// not offer.
    pub(crate) fn from_config_str(s: &str) -> AgentSortMode {
        match s {
            "updated" => AgentSortMode::Updated,
            "created" => AgentSortMode::Created,
            "name" => AgentSortMode::NameAsc,
            "name_desc" => AgentSortMode::NameDesc,
            "manual" => AgentSortMode::Manual,
            _ => AgentSortMode::Active,
        }
    }

    /// Map to the core-owned ordering enum consumed by
    /// `dux_core::flat_list::order_sessions`. The TUI keeps `AgentSortMode` for its
    /// config/UI concerns (labels, cycle); the core enum owns only the ordering.
    pub(crate) fn to_flat_sort_mode(self) -> dux_core::flat_list::FlatSortMode {
        use dux_core::flat_list::FlatSortMode;
        match self {
            AgentSortMode::Active => FlatSortMode::Active,
            AgentSortMode::Updated => FlatSortMode::Updated,
            AgentSortMode::Created => FlatSortMode::Created,
            AgentSortMode::NameAsc => FlatSortMode::NameAsc,
            AgentSortMode::NameDesc => FlatSortMode::NameDesc,
            AgentSortMode::Manual => FlatSortMode::Manual,
        }
    }

    /// The `config.ui.agent_sort` string for this mode (the shared wire value).
    pub(crate) fn as_config_str(&self) -> &'static str {
        match self {
            AgentSortMode::Active => "active",
            AgentSortMode::Updated => "updated",
            AgentSortMode::Created => "created",
            AgentSortMode::NameAsc => "name",
            AgentSortMode::NameDesc => "name_desc",
            AgentSortMode::Manual => "manual",
        }
    }

    /// A human-readable label for status lines.
    pub(crate) fn label(&self) -> &'static str {
        match self {
            AgentSortMode::Active => "Active first",
            AgentSortMode::Updated => "Recently updated",
            AgentSortMode::Created => "Recently created",
            AgentSortMode::NameAsc => "Name (A to Z)",
            AgentSortMode::NameDesc => "Name (Z to A)",
            AgentSortMode::Manual => "Manual order",
        }
    }

    /// The next mode in the TUI cycle. If the current mode is not in the cycle
    /// (i.e. `Manual`, which the TUI never offers), start the cycle at `Active`.
    pub(crate) fn next_in_tui_cycle(&self) -> AgentSortMode {
        match Self::TUI_CYCLE.iter().position(|m| m == self) {
            Some(i) => Self::TUI_CYCLE[(i + 1) % Self::TUI_CYCLE.len()],
            None => AgentSortMode::Active,
        }
    }
}

/// Build the flat left-pane list: a single globally-ordered agent list with no
/// project grouping. Active agents come first; detached/exited ("inactive") agents
/// collapse under an `InactiveToggle` tail (default collapsed). Orphan
/// (removed-project) sessions are plain `Session` rows here; the renderer marks
/// them inline.
///
/// The ORDERING is the core-owned `dux_core::flat_list::order_sessions` (the
/// cross-language twin of the web's `flatList.ts`); this function only wraps the
/// ordered indices into `LeftItem`s and inserts the collapsible toggle. The
/// returned `LeftItem::Session(index)` values are indices into `sessions`
/// (unchanged meaning); `sessions` is never mutated. See `flat_list.rs` for the
/// per-mode rules (in `Active` the inactive tail sorts most-recently-active-first;
/// every other mode leaves the tail verbatim).
///
/// `is_hot(index)` reports whether the session at that index is working or needs
/// attention (used only by `Active`).
///
/// `is_visible(index)` is a symmetric DISPLAY filter: an index that fails it never
/// enters either bucket, so a filtered row disappears from the list entirely and
/// the `InactiveToggle` tail only appears when a visible inactive row remains. When
/// there is no active filter, callers pass `&|_| true`. Filtering is a pure display
/// concern here: `sessions` is never mutated.
pub(crate) fn build_left_items(
    sessions: &[AgentSession],
    inactive_collapsed: bool,
    sort_mode: AgentSortMode,
    is_hot: &dyn Fn(usize) -> bool,
    is_visible: &dyn Fn(usize) -> bool,
) -> Vec<LeftItem> {
    // The ORDERING (partition, bucket sort, and the Active-mode inactive-tail
    // recency rule) is the core-owned `flat_list::order_sessions` (cross-language
    // twin of the web's `flatList.ts`); this surface only wraps the ordered
    // indices into `LeftItem`s and inserts the collapsible toggle.
    let order = dux_core::flat_list::order_sessions(
        sessions,
        sort_mode.to_flat_sort_mode(),
        is_hot,
        is_visible,
    );

    let mut items: Vec<LeftItem> = order.active.into_iter().map(LeftItem::Session).collect();
    if !order.inactive.is_empty() {
        items.push(LeftItem::InactiveToggle);
        if !inactive_collapsed {
            items.extend(order.inactive.into_iter().map(LeftItem::Session));
        }
    }
    items
}

mod components;
mod first_load;
mod input;
pub(crate) mod modal;
mod overlay_dismiss;
mod render;
mod reorder;
mod sessions;
#[cfg(test)]
mod test_support;
pub(crate) mod text_input;
mod workers;

// Re-export the welcome wordmark so the server status screen
// (`crate::server_screen`) can reuse it without making `render` public.
pub(crate) use render::ASCII_LOGO;

pub(crate) use first_load::{FirstLoadButton, FirstLoadPrompt, NotesFetched};

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
        // Write the orientation hint into the anonymous slot and pin it so it
        // persists until the user's first action replaces it.
        //
        // KEPT despite the arrival of the welcome screen, which says all of this
        // and more. The comment here used to call this a "first-run hint", which
        // was never accurate: it is set on EVERY cold boot, not only the first, so
        // deleting it would take the orientation line away from every existing
        // user in exchange for de-duplicating one launch in a fresh install's
        // lifetime. On that one launch the welcome modal covers it anyway, and
        // dismissing the modal writes its own status over it.
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
            last_first_load_height: 0,
            last_first_load_lines: 0,
            last_error_dialog_height: 0,
            last_error_dialog_lines: 0,
            pending_first_load: None,
            notes_fetch_rx: None,
            deferred_first_load_notes: None,
            notes_fetch_explicit_request: Arc::new(AtomicBool::new(false)),
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
            terminal_return_to_list: false,
            last_pty_size: (0, 0),
            prev_scrollback_offset: 0,
            last_diff_height: 0,
            last_diff_visual_lines: 0,
            theme,
            tick_count: 0,
            start_time: Instant::now(),
            refusal_blink: None,
            readonly_nudge_tick: None,
            inactive_collapsed: true,
            inactive_search_dismissed: None,
            inactive_collapse_overridden: false,
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
            project_chooser_context: None,
            agent_filter: None,
        };
        // First boot relaunches prior sessions; a resume must not — the engine
        // handed back from the web server already owns the live providers, and
        // any session the user closed in the web UI must stay closed.
        if matches!(restore, SessionRestore::Restore) {
            app.restore_sessions();
            // The first-load gate runs on a COLD BOOT only. A web-server→TUI flip
            // comes through `resume` with `SessionRestore::Skip`, and re-showing
            // the welcome or what's-new screen on every flip would be noise (the
            // shared `last_seen_version` row may not even have been stamped yet if
            // the user is still looking at the screen in another surface).
            app.begin_first_load();
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
                    // Poll faster while a row animates (spinner + name shimmer +
                    // attention blink) so those render smoothly (~30fps); stay on
                    // the lazy 100ms cadence otherwise to keep idle CPU low.
                    let poll_ms = if self.any_row_animating() { 33 } else { 100 };
                    let ready = match crate::io_retry::retry_on_interrupt(|| {
                        event::poll(Duration::from_millis(poll_ms))
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
        // The Detached/Exited-by-worktree-existence rule is core-owned
        // (`Engine::normalize_restored_sessions`, the same normalizer the web
        // bootstrap calls) so the two surfaces cannot drift; the TUI only adds
        // its startup auto-reopen dispatch on top.
        self.engine.normalize_restored_sessions();
        self.auto_reopen_eligible_sessions();
    }

    /// Dispatch the startup auto-reopen launches. The ELIGIBILITY rule is
    /// core-owned (`Engine::auto_reopen_candidates`, shared with the web
    /// server's startup pass); only the TUI-side launch dispatch lives here.
    fn auto_reopen_eligible_sessions(&mut self) {
        for session in self.engine.auto_reopen_candidates() {
            let request =
                self.agent_launch_request(session, true, AgentLaunchKind::StartupAutoReopen);
            self.dispatch_agent_launch(request);
        }
    }

    /// Populate the in-memory PR status map from the database so the UI shows
    /// PR state immediately on startup, before the first background poll.
    fn seed_pr_statuses_from_db(&mut self) {
        // The seed (including the shared "OPEN"/"MERGED"/"CLOSED" decode) is the
        // core-owned `Engine::seed_pr_statuses_from_store`, shared with the web
        // server bootstrap so both surfaces show persisted PR badges on startup.
        self.engine.seed_pr_statuses_from_store();
    }

    /// Close the help overlay if it is open, reporting whether it was.
    ///
    /// The ONE place help is closed. The close-overlay key reaches it through
    /// [`App::close_top_overlay`] and an outside click reaches it from the help
    /// branch of `handle_mouse`, so the two devices cannot drift — help is not
    /// a [`PromptState`] variant, so it cannot ride the click-outside engine's
    /// [`App::cancel_prompt`] ladder (see [`super::overlay_dismiss`]) and needs
    /// its own shared close instead.
    ///
    /// Dropping `help_scroll` is what closes it, and that also discards the
    /// scroll offset: help always reopens at the top, by either route.
    ///
    /// `announce` is the only difference between the two callers. The keyboard
    /// says how to reopen; a click stays silent, matching the engine's
    /// deliberate no-status rule for every other outside-click dismissal (the
    /// user just watched the overlay disappear under their cursor).
    pub(crate) fn close_help_overlay(&mut self, announce: bool) -> bool {
        if self.help_scroll.is_none() {
            return false;
        }
        self.help_scroll = None;
        if announce {
            let key = self.bindings.label_for(Action::ToggleHelp);
            self.set_info(format!("Closed help overlay. Press {key} to reopen."));
        }
        true
    }

    pub(crate) fn close_top_overlay(&mut self) -> bool {
        // The agent-list filter is the top-most dismissible layer on the Left pane.
        // Esc clears the query and restores the full list without activating a row,
        // routed here so filter dismissal stays uniform with prompt dismissal.
        if self.agent_filter.is_some() {
            self.close_agent_filter();
            self.set_info("Cleared the agent filter. Showing every agent again.");
            return true;
        }
        if matches!(self.fullscreen_overlay, FullscreenOverlay::Terminal) {
            let return_to_list = self.terminal_return_to_list;
            // Snap to the live edge while the surface still resolves the
            // terminal client — a scrolled-back PTY pauses ingestion, and the
            // pane behind the dismissed overlay must come back live.
            self.reset_pty_scrollback();
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
        // The NON-interactive agent fullscreen (e.g. the dormant-tab relaunch
        // screen left up after a tab's CLI exited) dismisses like any other
        // overlay. Interactive mode never reaches this path — its keys go
        // through the raw-input passthrough, which has its own exit handling.
        if matches!(self.fullscreen_overlay, FullscreenOverlay::Agent) {
            // Same live-edge snap as `exit_interactive_mode`: the minimized
            // pane must never come back frozen on a paused, scrolled-back PTY.
            self.reset_pty_scrollback();
            self.fullscreen_overlay = FullscreenOverlay::None;
            self.input_target = InputTarget::None;
            let key = self.bindings.label_for(Action::FocusAgent);
            self.set_info(format!(
                "Minimized the agent pane. Press \"{key}\" to reopen it."
            ));
            return true;
        }
        // The first-load screens dismiss like any other prompt, EXCEPT that
        // dismissal is also what records the running version as seen (the core
        // gate's timing contract), so they cannot take the generic path that just
        // blanks `prompt`.
        if matches!(self.prompt, PromptState::FirstLoad(_)) {
            self.dismiss_first_load_prompt();
            let palette_key = self.bindings.label_for(Action::OpenPalette);
            self.set_info(format!(
                "Dismissed. Press {palette_key} and run show-welcome-screen or show-release-notes \
                 to see these again."
            ));
            return true;
        }
        if !matches!(self.prompt, PromptState::None) {
            self.prompt = PromptState::None;
            self.set_info("Dismissed dialog. Resume your work in the current pane.");
            return true;
        }
        if self.close_help_overlay(true) {
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

    /// Whether anything on screen currently has a live animation: a working
    /// agent or terminal (spinner + name shimmer), an attention blink, or the
    /// one-shot modal refusal cue. The run loop polls faster while this is true
    /// so those animations render smoothly, and falls back to the lazy cadence
    /// when everything is quiet.
    ///
    /// The refusal cue is time-bounded, so it stops answering `true` on its own
    /// and the loop goes lazy again the moment the cue is over. (Kept under the
    /// historical `any_row_animating` name because the run loop's only question
    /// is "must I redraw at animation cadence?", and the answer is one flag.)
    pub(crate) fn any_row_animating(&self) -> bool {
        if self.refusal_blink_running() {
            return true;
        }
        let attention_on = self.engine.config.ui.attention_indicator;
        let agents = self.engine.sessions.iter().any(|s| {
            matches!(s.status, crate::model::SessionStatus::Active)
                && (self.engine.session_is_streaming(&s.id)
                    || (attention_on && self.engine.session_needs_attention(&s.id)))
        });
        // `terminal_is_working`, not `is_agent_streaming`: a terminal running a
        // quiet foreground app is "Running" (its spinner and label shimmer
        // animate) even with no output streaming.
        let terminals = self
            .engine
            .companion_terminals
            .keys()
            .any(|id| self.engine.terminal_is_working(id));
        agents || terminals
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
                // No project rows in the flat list; warn when the selected agent's
                // own project has a missing path.
                LeftItem::Session(idx) => {
                    let s = self.engine.sessions.get(idx)?;
                    let p = self.engine.projects.iter().find(|p| p.id == s.project_id)?;
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
            // The terminal-move commands are offered only when a terminal exists;
            // the agent-move commands are always offered (they fall through to
            // `true` and guard at invoke, like the rest of the palette).
            Action::MoveTerminalUp
            | Action::MoveTerminalDown
            | Action::MoveTerminalTop
            | Action::MoveTerminalBottom => !self.engine.companion_terminals.is_empty(),
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
            "manage-projects" => self.open_project_chooser(ProjectChooserIntent::Manage),
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
            "move-agent-up" => {
                self.move_selected_agent(reorder::MoveDir::Up);
                Ok(())
            }
            "move-agent-down" => {
                self.move_selected_agent(reorder::MoveDir::Down);
                Ok(())
            }
            "move-agent-top" => {
                self.move_selected_agent(reorder::MoveDir::Top);
                Ok(())
            }
            "move-agent-bottom" => {
                self.move_selected_agent(reorder::MoveDir::Bottom);
                Ok(())
            }
            "move-terminal-up" => {
                self.move_selected_terminal(reorder::MoveDir::Up);
                Ok(())
            }
            "move-terminal-down" => {
                self.move_selected_terminal(reorder::MoveDir::Down);
                Ok(())
            }
            "move-terminal-top" => {
                self.move_selected_terminal(reorder::MoveDir::Top);
                Ok(())
            }
            "move-terminal-bottom" => {
                self.move_selected_terminal(reorder::MoveDir::Bottom);
                Ok(())
            }
            "show-agent" => self.activate_center_agent(true),
            "show-terminal" => self.show_or_open_first_terminal(),
            "new-terminal-for-agent" => self.new_companion_terminal(),
            "new-terminal-for-project" => {
                self.open_project_chooser(ProjectChooserIntent::ProjectTerminal)
            }
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
            "sort-agents" => {
                self.cycle_agent_sort();
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
            // Both first-load screens can be opened deliberately even when their
            // `[ui] disable_*` flag is set: the flags suppress only what dux does
            // on its own, never what the user asks for.
            "show-welcome-screen" => self.show_welcome_screen_command(),
            "show-release-notes" => self.show_release_notes_command(),
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
            scroll: 0,
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
        // No project-count clamp here: the flat list indexes agent rows, not
        // projects, so clamping `selected_left` against `projects.len()` was
        // meaningless and reset the cursor to the top. `rebuild_left_items`
        // (below) plus the length check re-clamp against the real list length.
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
        // Until the user toggles the Inactive section by hand, auto-manage it:
        // expand when every agent is inactive (don't hide a wholly-dormant
        // workspace behind a collapsed toggle), collapse once any agent is active.
        if !self.inactive_collapse_overridden {
            let has_active = self
                .engine
                .sessions
                .iter()
                .any(|s| matches!(s.status, crate::model::SessionStatus::Active));
            self.inactive_collapsed = has_active;
        }
        // Expire a stale query-scoped dismissal: it applies only to the exact
        // query it was made under, so a changed or cleared query drops it and
        // the derivation below re-applies.
        let current_query = self.active_filter_query();
        if self
            .inactive_search_dismissed
            .as_deref()
            .is_some_and(|dismissed| current_query.as_deref() != Some(dismissed))
        {
            self.inactive_search_dismissed = None;
        }
        // Derived search expansion: while the filter query hits something in
        // the Inactive tail (and the user has not dismissed it for this exact
        // query), the tail renders OPEN without mutating `inactive_collapsed`,
        // so clearing the query restores the user's collapse preference.
        let effective_collapsed = self.inactive_collapsed && !self.inactive_tail_forced_open();
        let mode = AgentSortMode::from_config_str(&self.engine.config.ui.agent_sort);
        // Precompute the "hot" bit (working || needs-attention) per session so the
        // predicate can borrow this Vec while `build_left_items` borrows
        // `self.engine.sessions` (avoids borrowing `self.engine` twice).
        let hot: Vec<bool> = self
            .engine
            .sessions
            .iter()
            .map(|s| {
                self.engine.session_is_streaming(&s.id)
                    || self.engine.session_needs_attention(&s.id)
            })
            .collect();
        // Precompute the display-filter visibility mask so its closure can borrow a
        // plain `Vec<bool>` while `build_left_items` borrows `self.engine.sessions`
        // (mirroring the hot mask above, and avoiding a second `self.engine` borrow).
        // An absent or whitespace-only query makes everything visible.
        let visible: Vec<bool> = self.agent_visibility_mask();
        self.left_items_cache = build_left_items(
            &self.engine.sessions,
            effective_collapsed,
            mode,
            &|i| hot[i],
            &|i| visible[i],
        );
        self.ensure_selectable_left_item();
    }

    /// Build the per-session visibility mask for the current agent-list filter.
    /// Each entry is `true` when the session at that index passes the live query
    /// (via the shared core matcher `dux_core::agent_search::matches_session`), so a
    /// filtered row is excluded from both the active and inactive buckets. When
    /// filter mode is off, or the query is empty/whitespace, every session is
    /// visible. Fields mirror the web sidebar search exactly: display name,
    /// project name, branch (provider names are deliberately not searched).
    fn agent_visibility_mask(&self) -> Vec<bool> {
        let query = match &self.agent_filter {
            Some(input) => input.text.as_str(),
            None => return vec![true; self.engine.sessions.len()],
        };
        if dux_core::agent_search::normalize_query(query).is_empty() {
            return vec![true; self.engine.sessions.len()];
        }
        self.engine
            .sessions
            .iter()
            .map(|session| {
                let project_name = self
                    .engine
                    .projects
                    .iter()
                    .find(|p| p.id == session.project_id)
                    .map(|p| p.name.as_str())
                    .unwrap_or("");
                dux_core::agent_search::matches_session(
                    session.title.as_deref(),
                    &session.branch_name,
                    project_name,
                    query,
                )
            })
            .collect()
    }

    /// Count the inactive (Detached/Exited) agents that are currently VISIBLE
    /// under the active search filter. This is what the "Inactive (N)" toggle
    /// must show: the number of rows actually revealed when the tail is expanded
    /// under the current filter, not the raw count of every inactive session.
    /// It intersects the inactive-status predicate with `agent_visibility_mask`,
    /// consistent with how the pane title's filtered "Agents (M/N)" count works.
    pub(crate) fn visible_inactive_count(&self) -> usize {
        let mask = self.agent_visibility_mask();
        self.engine
            .sessions
            .iter()
            .enumerate()
            .filter(|(index, session)| {
                mask.get(*index).copied().unwrap_or(false)
                    && matches!(
                        session.status,
                        crate::model::SessionStatus::Detached | crate::model::SessionStatus::Exited
                    )
            })
            .count()
    }

    /// The live agent-filter query, normalized, or `None` when filter mode is
    /// off or the query is empty/whitespace (which filters nothing).
    fn active_filter_query(&self) -> Option<String> {
        let raw = self.agent_filter.as_ref()?.text.as_str();
        let query = dux_core::agent_search::normalize_query(raw);
        (!query.is_empty()).then_some(query)
    }

    /// Whether the Inactive tail is currently held open by the search: a live
    /// query with at least one visible inactive hit, not dismissed for this
    /// exact query. Pure derivation; `inactive_collapsed` is never consulted or
    /// mutated here.
    fn inactive_tail_forced_open(&self) -> bool {
        // The decision is the core-owned `quiet_tail::quiet_tail_forced_open`
        // (cross-language twin of the web's `quietTailForcedOpen`), keyed on the
        // NORMALIZED query so a whitespace/case variant of a dismissed query does
        // not resurrect the tail. `active_filter_query` already normalizes.
        dux_core::quiet_tail::quiet_tail_forced_open(
            self.active_filter_query().as_deref(),
            self.inactive_search_dismissed.as_deref(),
            self.visible_inactive_count() > 0,
        )
    }

    /// Enter agent-list filter mode: seed an empty query and rebuild the list.
    /// While active, printable keys type into the query and the arrows navigate
    /// the filtered rows (mirroring the project browser's type-to-filter).
    pub(crate) fn open_agent_filter(&mut self) {
        self.agent_filter = Some(TextInput::new());
        self.rebuild_left_items();
    }

    /// Leave agent-list filter mode: clear the query and restore the full list,
    /// keeping a sensible (selectable) selection via `rebuild_left_items`.
    pub(crate) fn close_agent_filter(&mut self) {
        self.agent_filter = None;
        self.rebuild_left_items();
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

    pub(crate) fn first_selectable_left_item(&self) -> Option<usize> {
        self.left_items()
            .iter()
            .position(|item| item.is_selectable())
    }

    pub(crate) fn last_selectable_left_item(&self) -> Option<usize> {
        self.left_items()
            .iter()
            .enumerate()
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

    /// Advance the shared `config.ui.agent_sort` to the next TUI display mode
    /// (active -> updated -> created -> name A to Z -> name Z to A -> active),
    /// persist it via the engine, and rebuild the display order. This is purely a
    /// DISPLAY sort: `engine.sessions` is never reordered and no `sort_order` is
    /// persisted. If the current stored mode is the web-only `manual`, the cycle
    /// restarts at `active`.
    pub(crate) fn cycle_agent_sort(&mut self) {
        let current = AgentSortMode::from_config_str(&self.engine.config.ui.agent_sort);
        let next = current.next_in_tui_cycle();
        self.engine.set_agent_sort(next.as_config_str());
        self.rebuild_left_items();
        self.set_info(format!(
            "Sorting agents by {}.",
            next.label().to_lowercase()
        ));
    }

    /// Toggle the flat list's "Inactive" tail open/closed. Bound to the same key
    /// as the old per-project collapse (Space / `ToggleProject`) and to Enter on
    /// the `InactiveToggle` row. Keeps the cursor on the toggle row afterward.
    pub(crate) fn toggle_collapse_selected_project(&mut self) {
        // A manual toggle takes over from the auto-manage in `rebuild_left_items`.
        self.inactive_collapse_overridden = true;
        if self.inactive_tail_forced_open() {
            // Collapsing a tail the search is holding open is an explicit act:
            // record the dismissal for THIS query (it expires when the query
            // changes) and collapse the base state too, so clearing the query
            // lands on the state the user last chose. Mirrors the web QuietTail.
            self.inactive_search_dismissed = self.active_filter_query();
            self.inactive_collapsed = true;
        } else if self.active_filter_query().is_some() && self.inactive_search_dismissed.is_some() {
            // Reopening under the query that was dismissed: drop the dismissal
            // so the derivation applies again.
            self.inactive_search_dismissed = None;
            self.inactive_collapsed = false;
        } else {
            self.inactive_collapsed = !self.inactive_collapsed;
        }
        self.rebuild_left_items();
        if let Some(new_index) = self
            .left_items()
            .iter()
            .position(|item| matches!(item, LeftItem::InactiveToggle))
        {
            self.selected_left = new_index;
        }
    }

    /// The project of the currently-selected agent row (the flat list has no
    /// project rows to select). `None` when the selection is not an agent (e.g. the
    /// Inactive toggle) or the agent's project record is gone (orphan). Reaching an
    /// agent-less project's actions goes through the project chooser, not selection.
    pub(crate) fn selected_project(&self) -> Option<&Project> {
        // A `manage-projects` pick wins while it points at a project that still
        // exists; this is how project-scoped palette commands reach an
        // agent-less project. A stale id (the project was removed) falls through
        // to the selected agent's project.
        if let Some(id) = &self.project_chooser_context
            && let Some(project) = self.engine.projects.iter().find(|p| &p.id == id)
        {
            return Some(project);
        }
        match self.left_items().get(self.selected_left) {
            Some(LeftItem::Session(index)) => {
                self.engine.sessions.get(*index).and_then(|session| {
                    self.engine
                        .projects
                        .iter()
                        .find(|project| project.id == session.project_id)
                })
            }
            _ => None,
        }
    }

    pub(crate) fn selected_session(&self) -> Option<&AgentSession> {
        match self.left_items().get(self.selected_left) {
            Some(LeftItem::Session(index)) => self.engine.sessions.get(*index),
            _ => None,
        }
    }

    /// Resolve the target project for a project-scoped ACTION and consume the
    /// one-and-done `manage-projects` target. This clones the project that
    /// `selected_project()` resolves (chooser context first, else the selected
    /// agent's project) and then clears `project_chooser_context` so the picked
    /// target applies to exactly ONE action — a second project action falls
    /// back to the selected agent's project. Do NOT use this in display paths;
    /// use `selected_project()` there so rendering never clears the target.
    pub(crate) fn take_selected_project(&mut self) -> Option<Project> {
        let project = self.selected_project().cloned();
        if project.is_some() {
            self.project_chooser_context = None;
        }
        project
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
            let project_default = self
                .engine
                .projects
                .iter()
                .find(|p| p.id == session.project_id)
                .map(|p| p.default_provider.clone());
            self.input_target = InputTarget::None;
            self.fullscreen_overlay = FullscreenOverlay::None;
            self.prompt = PromptState::AgentInfo(AgentInfoPrompt {
                session_label: label,
                lines: agent_info_lines(&session, project_default),
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
                focus: RenameSessionFocus::Input,
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
        use dux_core::engine::{BranchRenamePlan, BranchRenameRejection};

        // Core owns the decision: name validation, the overlap guard, the
        // optimistic title write, no-op detection, and the expectation stash.
        // This surface keeps only the presentation: the error copy, the keyed
        // status wording, the worker dispatch, and the list rebuild.
        let dispatch = match self
            .engine
            .prepare_branch_rename(session_id, &new_name, rename_branch)
        {
            BranchRenamePlan::Rejected(BranchRenameRejection::EmptyName) => {
                self.set_error("Name cannot be empty.");
                return;
            }
            BranchRenamePlan::Rejected(BranchRenameRejection::MalformedName) => {
                self.set_error(
                    "Agent name may only contain letters, digits, dashes, underscores, or slashes. \
                     It cannot start with \"-\" or \"/\", end with \"/\", or contain \"//\".",
                );
                return;
            }
            BranchRenamePlan::Rejected(BranchRenameRejection::AlreadyInFlight) => {
                self.set_error(
                    "A rename is already in progress for this agent. Wait for it to finish before renaming again.",
                );
                return;
            }
            BranchRenamePlan::Noop => {
                // The session vanished before the branch could be resolved; stay
                // silent, but keep the list consistent with the optimistic write.
                self.rebuild_left_items();
                return;
            }
            BranchRenamePlan::TitleWritten {
                name,
                sync_branches,
            } => {
                self.rebuild_left_items();
                self.set_info(format!("Renamed agent to \"{name}\"."));
                if sync_branches {
                    self.engine.update_branch_sync_sessions();
                }
                return;
            }
            BranchRenamePlan::RenameBranch(dispatch) => {
                // The optimistic title landed and the expectation is stashed;
                // reflect the new name before the git worker runs.
                self.rebuild_left_items();
                dispatch
            }
        };

        let sid = dispatch.session_id;
        let old_branch = dispatch.old_branch;
        let worktree = dispatch.worktree_path;
        let new_branch = dispatch.new_branch;
        let previous_title = dispatch.previous_title;

        // Declare the loading→final states together; the worker resolves the
        // matching message and carries it back on BranchRenameCompleted.
        let success_branch = new_branch.clone();
        let op =
            dux_core::engine::status_op(format!("Renaming branch to \"{new_branch}\"\u{2026}"))
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
                let result =
                    git::rename_branch(Path::new(&job_worktree), &job_old_branch, &job_new_branch)
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
    /// ordered by the shared active sort mode (`config.ui.agent_sort`), mirroring
    /// the agent comparators in [`build_left_items`].
    ///
    /// The terminal comparators are kept in LOCKSTEP with the web surface's
    /// `sortFlatTerminals` (`crates/dux-web/web/src/lib/flatTerminals.ts`); the two
    /// are duplicated per surface by necessity, so any change here must change there
    /// too. The comparators:
    /// - `Manual`: base order (by `sort_order` ascending, i.e. creation order).
    /// - `Created`: newest first (`Reverse(created_at)`).
    /// - `Updated`: newest first, by the same PTY-activity-derived timestamp the
    ///   viewmodel exposes as `TerminalView::updated_at` (last activity, else spawn).
    /// - `NameAsc` / `NameDesc`: by the terminal's DISPLAYED primary label
    ///   (`foreground_cmd` when present and non-empty, else `label`), lowercased, so
    ///   name-sort is WYSIWYG.
    /// - `Active` (default): working-or-typing terminals float to the top (a stable
    ///   float keeping base order within each group); terminals have no attention.
    ///
    /// The base sort by `sort_order` runs first in every mode: `sort_by_key` is
    /// stable, so equal keys (and the `Active` float) keep the manual base order,
    /// matching the agents' tie-stability.
    pub(crate) fn terminal_items(&self) -> Vec<(&String, &CompanionTerminal)> {
        let mode = AgentSortMode::from_config_str(&self.engine.config.ui.agent_sort);
        let mut items: Vec<_> = self.engine.companion_terminals.iter().collect();
        // Base order: manual `sort_order` ascending (creation order). Every mode
        // builds on top of this stable base.
        items.sort_by_key(|(_, t)| t.sort_order);

        // Displayed primary label, lowercased: the WYSIWYG name-sort key.
        let name_key = |t: &CompanionTerminal| -> String {
            t.foreground_cmd
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or(&t.label)
                .to_lowercase()
        };
        // The updated-at instant the viewmodel projects: last PTY activity mapped
        // onto wall clock, falling back to spawn time when there is none yet.
        let updated_at = |id: &str, t: &CompanionTerminal| -> chrono::DateTime<chrono::Utc> {
            self.engine
                .pty_activity
                .get(id)
                .map(|last| {
                    let ago = chrono::Duration::from_std(last.elapsed()).unwrap_or_default();
                    chrono::Utc::now() - ago
                })
                .unwrap_or(t.created_at)
        };

        match mode {
            // Base order already applied.
            AgentSortMode::Manual => {}
            AgentSortMode::Created => items.sort_by_key(|(_, t)| std::cmp::Reverse(t.created_at)),
            AgentSortMode::Updated => {
                items.sort_by_key(|(id, t)| std::cmp::Reverse(updated_at(id, t)))
            }
            AgentSortMode::NameAsc => items.sort_by_key(|(_, t)| name_key(t)),
            AgentSortMode::NameDesc => items.sort_by_key(|(_, t)| std::cmp::Reverse(name_key(t))),
            AgentSortMode::Active => {
                // Stable float: working-or-typing terminals rise above the rest,
                // each group keeping base order. Matches the web `activeFirst`.
                let (hot, rest): (Vec<_>, Vec<_>) = items.into_iter().partition(|(id, _)| {
                    self.engine.is_agent_streaming(id) || self.engine.is_typing(id)
                });
                items = hot;
                items.extend(rest);
            }
        }
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

    /// Returns the number of running companion terminals for a given session.
    pub(crate) fn session_terminal_count(&self, session_id: &str) -> usize {
        self.engine
            .companion_terminals
            .values()
            .filter(|t| matches!(&t.owner, TerminalOwner::Session(sid) if sid == session_id))
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

// Disambiguated tab labels are the core-owned `dux_core::agent_tabs::tab_labels`
// (cross-language twin of the web's `tabLabels`, pinned by shared vectors).
pub(crate) use dux_core::agent_tabs::tab_labels;

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
    // The reconciliation DECISION (validate identity, merge per field, adopt
    // config-only, write store-only back) is the core-owned
    // `dux_core::config_sync::reconcile_config_projects`, shared with the web
    // server's bootstrap. Only PERSISTING is a surface concern: the TUI renders
    // the full commented template via `save_config`.
    dux_core::config_sync::reconcile_config_projects(config, session_store, |config| {
        save_config(&paths.config_path, config, bindings)
    })
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

// The project-reconciliation helpers (`validate_project_records`,
// `merge_project_records`, and the config-authoritative field merge) moved to
// core `dux_core::config_sync` and are reached there via
// `sync_config_projects_with_store` above. The low-level path helpers stay
// reachable to the OTHER TUI callers below (`persist_runtime_projects_*`,
// `runtime_project_to_config`) as re-exports of the single core source.
pub(crate) use dux_core::config_sync::portable_project_path;
use dux_core::config_sync::{expanded_project_path, same_expanded_project_path};

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

        let lines = agent_info_lines(&s, None);
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
        let lines = agent_info_lines(&s, None);
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
    fn agent_info_lines_omit_drift_line_when_initial_empty() {
        let mut s = test_session("s1", "p1", 0);
        s.branch_name = "main".into();
        s.initial_branch = String::new();
        let lines = agent_info_lines(&s, None);
        assert!(
            !lines
                .iter()
                .any(|(l, _)| l.to_lowercase().contains("changed since creation")),
            "an empty initial_branch must not produce a phantom drift line"
        );
    }

    #[test]
    fn agent_info_provider_line_notes_a_divergent_project_default() {
        let s = test_session("s1", "p1", 0); // provider "codex"
        // Matching default: plain provider line, no annotation.
        let same = agent_info_lines(&s, Some(ProviderKind::from_str("codex")));
        let provider_same = same
            .iter()
            .find(|(l, _)| l.starts_with("Provider:"))
            .expect("provider line");
        assert!(!provider_same.0.contains("project default"));

        // Divergent default: the line spells out the project default too.
        let diff = agent_info_lines(&s, Some(ProviderKind::from_str("claude")));
        let provider_diff = diff
            .iter()
            .find(|(l, _)| l.starts_with("Provider:"))
            .expect("provider line");
        assert!(provider_diff.0.contains("codex"));
        assert!(provider_diff.0.contains("project default: claude"));
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
    fn pick_project_filter_matches_by_name_and_path() {
        let entry = |id: &str, name: &str, path: &str| ProjectChooserEntry {
            id: id.to_string(),
            name: name.to_string(),
            path: path.to_string(),
            agent_count: 0,
            path_missing: false,
        };
        let entries = vec![
            entry("a", "alpha", "/home/me/alpha"),
            entry("b", "beta", "/home/me/work/beta"),
            entry("c", "gamma", "/srv/gamma"),
        ];
        let vis = |query: &str| {
            let mut list = SearchableList::new();
            list.filter = TextInput::with_text(query.to_string());
            list.visible_indices(&entries, pick_project_matches)
        };
        // Empty filter matches everything in order.
        assert_eq!(vis(""), vec![0, 1, 2]);
        // Name match, case-insensitive.
        assert_eq!(vis("BETA"), vec![1]);
        // Path match (segment not in any name).
        assert_eq!(vis("work"), vec![1]);
        // No match.
        assert!(vis("zzz").is_empty());
    }

    #[test]
    fn searchable_list_clamps_and_toggles() {
        let mut list = SearchableList::new();
        assert!(!list.is_filtering());
        list.begin_search();
        assert!(list.searching && list.is_filtering());
        // move_down is bounded by the visible length; move_up saturates at 0.
        list.selected = 0;
        list.move_down(2);
        assert_eq!(list.selected, 1);
        list.move_down(2); // already at the last visible row
        assert_eq!(list.selected, 1);
        list.move_up();
        list.move_up();
        assert_eq!(list.selected, 0);
        // Shrinking the visible set clamps the selection back into range.
        list.selected = 5;
        list.clamp_selected(3);
        assert_eq!(list.selected, 2);
        list.clamp_selected(0);
        assert_eq!(list.selected, 0);
        // Leaving search is one step: the mode goes off and the query goes
        // with it, so the next close key belongs to the modal.
        assert!(list.exit_search_clearing_filter());
        assert!(!list.searching);
        assert!(list.filter.is_empty());
        assert_eq!(list.selected, 0);
        assert!(
            !list.exit_search_clearing_filter(),
            "an idle, empty search row has nothing to leave"
        );
    }

    #[test]
    fn build_left_items_flat_splits_active_and_inactive() {
        let mut active = test_session("active-1", "p", 0);
        active.status = SessionStatus::Active;
        // test_session defaults to Detached, i.e. inactive.
        let inactive = test_session("gone-1", "p", 0);
        let sessions = vec![active, inactive];

        // Collapsed (default): the active row, then a single Inactive toggle; the
        // inactive session is hidden.
        assert_eq!(
            build_left_items(&sessions, true, AgentSortMode::Active, &|_| false, &|_| {
                true
            }),
            vec![LeftItem::Session(0), LeftItem::InactiveToggle],
        );

        // Expanded: the inactive session follows the toggle.
        assert_eq!(
            build_left_items(&sessions, false, AgentSortMode::Active, &|_| false, &|_| {
                true
            }),
            vec![
                LeftItem::Session(0),
                LeftItem::InactiveToggle,
                LeftItem::Session(1),
            ],
        );
    }

    #[test]
    fn build_left_items_flat_has_no_toggle_without_inactive() {
        let mut a = test_session("a", "p", 0);
        a.status = SessionStatus::Active;
        let mut b = test_session("b", "other", 0);
        b.status = SessionStatus::Active;
        let items = build_left_items(&[a, b], true, AgentSortMode::Active, &|_| false, &|_| true);
        assert_eq!(items, vec![LeftItem::Session(0), LeftItem::Session(1)]);
        assert!(!items.contains(&LeftItem::InactiveToggle));
    }

    #[test]
    fn build_left_items_flat_orphan_sessions_are_plain_rows() {
        // A session whose project record is gone is a normal Session row (the
        // renderer marks it inline); there is no separate orphan header, and no
        // project grouping interleaves the list.
        let mut real = test_session("real", "p", 0);
        real.status = SessionStatus::Active;
        let mut ghost = test_session("ghost", "gone-project", 0);
        ghost.status = SessionStatus::Active;
        assert_eq!(
            build_left_items(
                &[real, ghost],
                true,
                AgentSortMode::Active,
                &|_| false,
                &|_| true
            ),
            vec![LeftItem::Session(0), LeftItem::Session(1)],
        );
    }

    #[test]
    fn build_left_items_active_mode_floats_hot_and_orders_inactive_by_updated() {
        // Three active agents (indices 0,1,2) and two inactive (3,4). Only index 2
        // is hot. Active mode floats it above the non-hot actives while keeping
        // their incoming order; the inactive tail sorts most-recently-updated.
        let t0 = Utc::now();
        let mk = |id: &str, status: SessionStatus, updated_offset: i64| {
            let mut s = test_session(id, "p", 0);
            s.status = status;
            s.updated_at = t0 + chrono::Duration::seconds(updated_offset);
            s
        };
        let sessions = vec![
            mk("a0", SessionStatus::Active, 0),
            mk("a1", SessionStatus::Active, 0),
            mk("a2", SessionStatus::Active, 0),
            mk("i3", SessionStatus::Detached, 10), // older
            mk("i4", SessionStatus::Exited, 20),   // newer
        ];
        let hot = |i: usize| i == 2;
        let items = build_left_items(&sessions, false, AgentSortMode::Active, &hot, &|_| true);
        assert_eq!(
            items,
            vec![
                LeftItem::Session(2), // hot floats up
                LeftItem::Session(0),
                LeftItem::Session(1),
                LeftItem::InactiveToggle,
                LeftItem::Session(4), // newer updated_at first
                LeftItem::Session(3),
            ],
        );
    }

    #[test]
    fn build_left_items_name_and_name_desc_are_reversed_without_touching_sessions() {
        let mk = |id: &str, name: &str| {
            let mut s = test_session(id, "p", 0);
            s.status = SessionStatus::Active;
            s.branch_name = name.to_string();
            s.title = None;
            s
        };
        let sessions = vec![mk("s0", "charlie"), mk("s1", "alpha"), mk("s2", "bravo")];

        let asc = build_left_items(&sessions, true, AgentSortMode::NameAsc, &|_| false, &|_| {
            true
        });
        assert_eq!(
            asc,
            vec![
                LeftItem::Session(1), // alpha
                LeftItem::Session(2), // bravo
                LeftItem::Session(0), // charlie
            ],
        );

        let desc = build_left_items(
            &sessions,
            true,
            AgentSortMode::NameDesc,
            &|_| false,
            &|_| true,
        );
        assert_eq!(
            desc,
            vec![
                LeftItem::Session(0), // charlie
                LeftItem::Session(2), // bravo
                LeftItem::Session(1), // alpha
            ],
        );

        // `sessions` order is untouched: still the incoming order.
        assert_eq!(sessions[0].branch_name, "charlie");
        assert_eq!(sessions[1].branch_name, "alpha");
        assert_eq!(sessions[2].branch_name, "bravo");
    }

    #[test]
    fn build_left_items_manual_renders_verbatim_incoming_order() {
        // A web-set "manual" mode displays engine.sessions verbatim (the TUI shows
        // it but never offers it).
        let mk = |id: &str, name: &str| {
            let mut s = test_session(id, "p", 0);
            s.status = SessionStatus::Active;
            s.branch_name = name.to_string();
            s
        };
        let sessions = vec![mk("s0", "charlie"), mk("s1", "alpha"), mk("s2", "bravo")];
        let items = build_left_items(&sessions, true, AgentSortMode::Manual, &|_| false, &|_| {
            true
        });
        assert_eq!(
            items,
            vec![
                LeftItem::Session(0),
                LeftItem::Session(1),
                LeftItem::Session(2),
            ],
        );
    }

    #[test]
    fn build_left_items_nameasc_sorts_active_but_leaves_inactive_verbatim() {
        // The ACTIVE bucket sorts by name; the inactive tail stays in incoming
        // order (matching the web, which never sorts its dormant tail).
        let mk = |id: &str, name: &str, status: SessionStatus| {
            let mut s = test_session(id, "p", 0);
            s.status = status;
            s.branch_name = name.to_string();
            s.title = None;
            s
        };
        let sessions = vec![
            mk("a0", "charlie", SessionStatus::Active),
            mk("a1", "alpha", SessionStatus::Active),
            mk("i2", "zeta", SessionStatus::Detached),
            mk("i3", "aardvark", SessionStatus::Detached),
        ];
        let items = build_left_items(
            &sessions,
            false,
            AgentSortMode::NameAsc,
            &|_| false,
            &|_| true,
        );
        assert_eq!(
            items,
            vec![
                LeftItem::Session(1), // alpha (active, name-sorted)
                LeftItem::Session(0), // charlie
                LeftItem::InactiveToggle,
                LeftItem::Session(2), // zeta   (inactive, verbatim — NOT sorted)
                LeftItem::Session(3), // aardvark
            ],
            "active bucket name-sorted, inactive tail left in incoming order",
        );
    }

    #[test]
    fn build_left_items_active_mode_orders_inactive_tail_by_recency() {
        // Under Active mode (the ONLY mode that sorts the tail) the inactive
        // bucket orders most-recently-updated first, regardless of incoming order.
        let t0 = Utc::now();
        let mk = |id: &str, status: SessionStatus, off: i64| {
            let mut s = test_session(id, "p", 0);
            s.status = status;
            s.updated_at = t0 + chrono::Duration::seconds(off);
            s
        };
        let sessions = vec![
            mk("a0", SessionStatus::Active, 0),
            mk("i1", SessionStatus::Detached, 5),  // older
            mk("i2", SessionStatus::Detached, 50), // newer
        ];
        let items = build_left_items(&sessions, false, AgentSortMode::Active, &|_| false, &|_| {
            true
        });
        assert_eq!(
            items,
            vec![
                LeftItem::Session(0),
                LeftItem::InactiveToggle,
                LeftItem::Session(2), // newer updated_at first
                LeftItem::Session(1),
            ],
        );
    }

    #[test]
    fn build_left_items_updated_and_created_order_active_by_recency() {
        // The active bucket orders most-recent first under both Updated and
        // Created (test_session sets updated_at == created_at == now + offset).
        let mk = |id: &str, off: i64| {
            let mut s = test_session(id, "p", off);
            s.status = SessionStatus::Active;
            s
        };
        let sessions = vec![mk("s0", 0), mk("s1", 100), mk("s2", 50)];

        let updated =
            build_left_items(&sessions, true, AgentSortMode::Updated, &|_| false, &|_| {
                true
            });
        assert_eq!(
            updated,
            vec![
                LeftItem::Session(1), // newest
                LeftItem::Session(2),
                LeftItem::Session(0), // oldest
            ],
            "Updated must order the active bucket newest-first",
        );

        let created =
            build_left_items(&sessions, true, AgentSortMode::Created, &|_| false, &|_| {
                true
            });
        assert_eq!(
            created,
            vec![
                LeftItem::Session(1),
                LeftItem::Session(2),
                LeftItem::Session(0),
            ],
            "Created must order the active bucket newest-first",
        );
    }

    #[test]
    fn build_left_items_tie_stability_keeps_incoming_order() {
        // Two active agents with identical updated_at/created_at AND identical
        // names must keep incoming order under every comparator mode (the sorts
        // are stable), so a tie never reshuffles the list.
        let t = Utc::now();
        let mk = |id: &str| {
            let mut s = test_session(id, "p", 0);
            s.status = SessionStatus::Active;
            s.branch_name = "same".to_string();
            s.title = None;
            s.updated_at = t;
            s.created_at = t;
            s
        };
        let sessions = vec![mk("s0"), mk("s1")];
        for mode in [
            AgentSortMode::Updated,
            AgentSortMode::Created,
            AgentSortMode::NameAsc,
            AgentSortMode::NameDesc,
        ] {
            let items = build_left_items(&sessions, true, mode, &|_| false, &|_| true);
            assert_eq!(
                items,
                vec![LeftItem::Session(0), LeftItem::Session(1)],
                "ties must preserve incoming order under {mode:?}",
            );
        }
    }

    #[test]
    fn build_left_items_visibility_predicate_prunes_both_buckets() {
        // A visibility predicate hides one active and one inactive index; the
        // hidden rows must appear in NEITHER bucket, and the Inactive toggle still
        // appears because a visible inactive row remains.
        let mk = |id: &str, status: SessionStatus| {
            let mut s = test_session(id, "p", 0);
            s.status = status;
            s
        };
        let sessions = vec![
            mk("a0", SessionStatus::Active),   // 0: visible active
            mk("a1", SessionStatus::Active),   // 1: hidden active
            mk("i2", SessionStatus::Detached), // 2: visible inactive
            mk("i3", SessionStatus::Detached), // 3: hidden inactive
        ];
        let visible = |i: usize| i == 0 || i == 2;
        let items = build_left_items(
            &sessions,
            false,
            AgentSortMode::Active,
            &|_| false,
            &visible,
        );
        assert_eq!(
            items,
            vec![
                LeftItem::Session(0),
                LeftItem::InactiveToggle,
                LeftItem::Session(2),
            ],
        );
        assert!(!items.contains(&LeftItem::Session(1)));
        assert!(!items.contains(&LeftItem::Session(3)));
    }

    #[test]
    fn build_left_items_no_toggle_when_every_inactive_is_filtered_out() {
        // The only inactive row is hidden by the predicate, so no Inactive toggle
        // should render even though an inactive session exists in `sessions`.
        let mk = |id: &str, status: SessionStatus| {
            let mut s = test_session(id, "p", 0);
            s.status = status;
            s
        };
        let sessions = vec![
            mk("a0", SessionStatus::Active),
            mk("i1", SessionStatus::Detached),
        ];
        let visible = |i: usize| i == 0;
        let items = build_left_items(&sessions, true, AgentSortMode::Active, &|_| false, &visible);
        assert_eq!(items, vec![LeftItem::Session(0)]);
        assert!(!items.contains(&LeftItem::InactiveToggle));
    }

    /// App with one ACTIVE agent (index 0) and one EXITED "quiet-fox" agent
    /// (index 1), so the Inactive tail auto-collapses (an active agent exists)
    /// and a search can hit either bucket.
    fn quiet_search_app() -> App {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        app.engine.sessions[0].status = SessionStatus::Active;
        let mut quiet = app.engine.sessions[0].clone();
        quiet.id = "session-quiet".to_string();
        quiet.branch_name = "quiet-fox".to_string();
        quiet.initial_branch = "quiet-fox".to_string();
        quiet.title = None;
        quiet.status = SessionStatus::Exited;
        app.engine.sessions.push(quiet);
        app.rebuild_left_items();
        app
    }

    /// Search auto-expand is DERIVED: a query hitting a quiet agent shows its
    /// row without mutating the collapse preference, and clearing the query
    /// restores the collapsed tail.
    #[test]
    fn quiet_tail_auto_expands_when_search_hits_an_inactive_agent() {
        let mut app = quiet_search_app();
        assert!(
            app.inactive_collapsed,
            "an active agent auto-collapses the tail"
        );
        assert!(!app.left_items().contains(&LeftItem::Session(1)));

        app.agent_filter = Some(TextInput::with_text("fox".to_string()));
        app.rebuild_left_items();
        assert!(
            app.left_items().contains(&LeftItem::Session(1)),
            "a quiet-hit query must reveal the matching quiet row"
        );
        assert!(
            app.inactive_collapsed,
            "the derivation must not mutate the user's collapse preference"
        );

        app.close_agent_filter();
        assert!(
            !app.left_items().contains(&LeftItem::Session(1)),
            "clearing the query must restore the collapsed tail"
        );
    }

    #[test]
    fn quiet_tail_stays_collapsed_for_a_query_matching_only_active_agents() {
        let mut app = quiet_search_app();
        app.agent_filter = Some(TextInput::with_text("agent-branch".to_string()));
        app.rebuild_left_items();
        // The quiet agent does not match, so it is filtered out entirely: no
        // revealed row and no Inactive toggle (same as the web, whose section
        // hides when no quiet row matches).
        assert!(!app.left_items().contains(&LeftItem::Session(1)));
        assert!(!app.left_items().contains(&LeftItem::InactiveToggle));
    }

    /// Collapsing the tail WHILE a matching query holds it open is an explicit
    /// act that wins for that query; a changed query expires the dismissal.
    #[test]
    fn collapsing_a_search_expanded_tail_wins_until_the_query_changes() {
        let mut app = quiet_search_app();
        app.agent_filter = Some(TextInput::with_text("fox".to_string()));
        app.rebuild_left_items();
        assert!(app.left_items().contains(&LeftItem::Session(1)));

        app.toggle_collapse_selected_project();
        assert!(
            !app.left_items().contains(&LeftItem::Session(1)),
            "a manual collapse must win over the search derivation"
        );
        app.rebuild_left_items();
        assert!(
            !app.left_items().contains(&LeftItem::Session(1)),
            "the dismissal must hold across rebuilds under the same query"
        );

        // A different query that still hits the quiet agent: the dismissal was
        // scoped to the old query, so the tail derives open again.
        app.agent_filter = Some(TextInput::with_text("quiet".to_string()));
        app.rebuild_left_items();
        assert!(app.left_items().contains(&LeftItem::Session(1)));
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

        // `trap '' TERM HUP` makes the shell ignore the whole graceful salvo
        // (`terminate()` sends SIGTERM then SIGHUP); `echo ready` marks the
        // trap as installed; the busy loop keeps it alive until force-killed.
        let client = crate::pty::PtyClient::spawn(
            "sh",
            &[
                "-c".to_string(),
                "trap '' TERM HUP; echo ready; while true; do :; done".to_string(),
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
