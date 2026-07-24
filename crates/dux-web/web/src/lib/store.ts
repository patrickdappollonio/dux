import { useSyncExternalStore } from "react"
import { toast } from "sonner"

import { sanitizeAgentName } from "./agentName"
import { git } from "./git"
import { projectsApi, type PatchProjectBody } from "./projectsApi"
import { existingBranchConflict, sessionsApi, SessionsApiError } from "./sessionsApi"

import { ordersMatch } from "./reorder"
import { sortedSessionIds, type SortKey } from "./sortSessions"
import type { FlatSortKey } from "./flatList"
import { EventsSocket } from "./eventsSocket"
import { getActivePtySocket } from "./ptySocket"
import { notifyPtyOwner, resetPtyOwnerEpochs } from "./ptyOwnership"
import { macroPayloadBytes } from "./macros"
import { terminalsApi } from "./terminalsApi"
import { tabsApi } from "./tabsApi"
import { browseApi } from "./browseApi"
import { configApi } from "./configApi"
import { setConnectionId } from "./connection"
import {
  ChangesFetchError,
  fetchChanges,
  type SessionChangesResponse,
} from "./changesApi"
import { type Bootstrap, fetchBootstrap } from "./bootstrapApi"
import { attentionCount, formatTabTitle } from "./attention"
import { applyAttentionFavicon } from "./favicon"
import { resolveInstanceTitle } from "./instanceTitle"
import { type Spine, fetchSpine } from "./spineApi"
import { resolveFocusedTab, shouldRefireFocusPut } from "./agentTabs"
import {
  activateTab as editorActivateTabPure,
  closeTab as editorCloseTabPure,
  closeTabsUnderPath as editorCloseTabsUnderPathPure,
  emptyTabsState,
  openFile as editorOpenFilePure,
  pinTab as editorPinTabPure,
  renameTabPaths as editorRenameTabPathsPure,
  setTabDirty as editorSetTabDirtyPure,
  setTabMode as editorSetTabModePure,
} from "./editorTabs"
import type { EditorTabsState } from "./editorTabs"
import { newClientId } from "./uid"
import type {
  BranchWarningView,
  InspectKind,
  ChangedFileView,
  ConnState,
  DirEntryView,
  EventsServerMessage,
  MacroView,
  ProjectWorktreeEntryView,
  StartupLogContent,
  StartupLogEntry,
} from "./types"

// Who a companion terminal belongs to: an agent session (spawned in that
// agent's worktree) or a project (a "project terminal", spawned at the
// project's repo root with no agent attached). Mirrors the Rust
// `TerminalOwner`. Every consumer must branch on `kind`; there is
// deliberately no bare-id accessor, so the project variant can never be
// silently ignored by session-shaped code.
export type TerminalOwnerRef =
  | { kind: "session"; sessionId: string }
  | { kind: "project"; projectId: string }

// The currently-streamed target: either an agent session or a companion
// terminal. An agent target carries a `sessionId` for session-scoped UI (the
// breadcrumb, changed files); a terminal target carries its OWNER, which is a
// session or a project, and a project terminal has no session context at all.
export type SelectedTarget =
  // An agent tab. `tabId === sessionId` for the session-slot tab; an extra tab carries
  // its own id. The streamed PTY and all per-tab UI resolve from `tabId`, while
  // session-scoped UI keeps using `sessionId`.
  | { kind: "agent"; sessionId: string; tabId: string }
  | { kind: "terminal"; terminalId: string; owner: TerminalOwnerRef }

// The mobile hub-&-spoke shell shows one screen at a time: the project/session
// hub ("home"), the focused terminal, or the changed-files view. Desktop never
// reads this — it renders all three panes at once.
export type MobileScreen = "home" | "terminal" | "changes"

// The name-input dialog (one component, two modes) targets either a fresh agent
// in a project or a fork of an existing session. The shared draft/randomize/
// generated/pending state below drives both; only the dispatch target differs.
export type CreateAgentTarget =
  | { kind: "new"; projectId: string }
  | { kind: "fork"; sessionId: string }
  | { kind: "pr"; projectId: string }

// The file pending discard-confirmation, or null. `untracked` drives the
// dialog's warning copy (a tracked file is restored from HEAD; an untracked
// file is permanently deleted). Derived from the file's git status at the moment
// the affordance is clicked; the server independently re-derives and re-validates
// it, so this is only a UI hint.
export interface DiscardTarget {
  sessionId: string
  path: string
  untracked: boolean
}

// An optimistic session-order overlay for one project: while a drag-and-drop
// reorder is in flight, the UI renders `ids` (the new complete order of that
// project's sessions) instead of the server's order, so the row doesn't snap
// back during the ≤50ms round-trip. Cleared when a ViewModel arrives whose order
// already matches, or on any error status.
export interface PendingSessionOrder {
  projectId: string
  ids: string[]
}

// The changed-files request state machine for the SELECTED session. This is the
// single source of truth for changed-files data across the app (the changes
// pane, commit dialog, discard dialog, mobile badge, and editor markers all read
// it), fed by `GET /api/v1/sessions/:id/changes` and invalidated by
// `session.changes` events over `/ws/events`.
//
//   - `idle`    nothing selected (or the slice was cleared, e.g. a 404).
//   - `loading` a fetch is in flight for `sessionId`.
//   - `loaded`  `staged`/`unstaged` are current for `sessionId` at `rev`.
//   - `error`   the last fetch failed; `error` carries why. Self-heals on the
//               next `session.changes` event (which always refetches in this
//               state, side-stepping the `rev > undefined` trap).
//
// `sessionId` is the session these lists belong to; consumers only trust the
// slice when it equals their own session id. `rev` is the monotonic per-session
// revision of the applied data; a response or event with an older `rev` is
// dropped (out-of-order / lost-race protection).
export type ChangesPhase = "idle" | "loading" | "loaded" | "error"

export interface ChangesSlice {
  sessionId: string | null
  phase: ChangesPhase
  rev: number
  staged: ChangedFileView[]
  unstaged: ChangedFileView[]
  error: string | null
}

// A tiny external store backed by `useSyncExternalStore`. A single module-level
// `EventsSocket` (`/ws/events`) feeds it: resource-change events plus the
// connection id and status frames (surfaced as sonner toasts). Every action is a
// REST `/api/v1/*` call. The PTY byte stream is NOT kept in React state nor on
// this socket — each focused terminal attaches to its own dedicated `PtySocket`
// (`lib/ptySocket.ts`).

export interface DuxState {
  // The workspace "spine" from `GET /api/v1/spine`: projects, sessions, and the
  // core-computed sidebar grouping. These three fields used to ride the broadcast
  // `ViewModel`; they now live here, fetched once after auth resolves (alongside
  // the bootstrap document) and re-fetched on a `projects.changed` /
  // `sessions.changed` event or an events-socket reconnect. `null` until the
  // first fetch lands — every consumer falls back to empty lists so nothing
  // crashes in that pre-load window.
  spine: Spine | null
  // The build-static / config-derived document from `GET /api/v1/bootstrap`
  // (providers, macros, palette commands, welcome tips, version, UI flags,
  // global env). Fetched once after auth resolves and re-fetched on a
  // `config.changed` event. `null` until the first fetch lands — every consumer
  // falls back to a sensible default (empty list / true / the scrollback
  // default) so nothing crashes in that pre-load window.
  bootstrap: Bootstrap | null
  // Set to true synchronously when boot runs (at module load). Tests wait on
  // this as a settled signal instead of the old auth.phase guard.
  booted: boolean
  conn: ConnState
  // Sticky "the app-wide events socket is not connected" flag that drives the
  // full-screen offline modal (`OfflineOverlay`). Distinct from `conn` because a
  // reconnect attempt re-enters `conn === "connecting"` between drops, and gating
  // the modal on the raw state would flicker it off on every retry. So this latches
  // true on the first drop (`closed`/`failed`) and clears ONLY when we are `open`
  // again; an intermediate `connecting` leaves it untouched. False during the very
  // first boot connect (no prior connection to have lost).
  offline: boolean
  selectedTarget: SelectedTarget | null
  // Derived from `selectedTarget`: the owning session id. Session-scoped UI
  // (breadcrumb, changed files, statusbar) reads this so it keeps working
  // whether an agent or one of its terminals is focused. Kept in `state` (not
  // recomputed per snapshot) so `getSnapshot` stays referentially stable.
  selectedSessionId: string | null
  // Bumped on every reconnect/force-reconnect so the focused TerminalPane
  // remounts and re-subscribes. The reconnect replaces the server-side provider
  // with a new PtyClient; the old PtyClient's byte forwarder is dead, so an
  // already-focused pane (same target id) must re-issue `subscribe` to attach to
  // the new provider. Folded into the pane's React key alongside the target id.
  terminalEpoch: number
  commitTarget: string | null
  commitDraft: string
  deleteTarget: string | null
  // The companion terminal id pending close confirmation, or null. Mirrors the
  // TUI, which ALWAYS confirms terminal deletion (the running process is killed).
  deleteTerminalTarget: string | null
  // The tab pending close confirmation, or null. Closing always confirms; all
  // tabs are generic. Closing a tab ends it, and closing the agent's last tab
  // detaches the agent (which stays in Projects, reopenable).
  closeTabTarget: { sessionId: string; tabId: string } | null
  // Session ids with a tab-create request in flight, so the strip's "+" disables
  // until it resolves (a double-click can't spawn two tabs). The per-agent tab
  // cap still guards the server; this is the common-case UX guard.
  createTabInFlight: string[]
  // extra tab ids the user explicitly started from their dormant card. Lets the
  // pane mount (subscribe = launch fresh) for a tab whose server-reported
  // `has_live_process` is still false — so FOCUS alone never launches a dormant
  // tab, only the "Start session" button does.
  startedDormantTabs: string[]
  // The unstaged file pending discard confirmation, or null. The TUI confirms
  // every discard (it's destructive); the web mirrors that.
  discardTarget: DiscardTarget | null
  globalEnvOpen: boolean
  projectSettingsTarget: string | null
  // The agent (session) whose startup-command / project-env editor is open, or
  // null. Both edit the agent's PROJECT (env and startup command are
  // project-scoped in dux — there is no per-agent env), surfaced from the agent
  // menu for quick access (mirroring the TUI's per-agent palette commands). The
  // dialog resolves the owning project from the session id.
  agentStartupCommandTarget: string | null
  agentEnvTarget: string | null
  // The agent (session) whose startup-command log viewer is open, or null. The
  // log files + the displayed file's contents are fetched over REST into the
  // fields below when the viewer opens (mirroring the attach-worktree listing).
  startupLogsTarget: string | null
  startupLogsEntries: StartupLogEntry[]
  startupLogsSelected: StartupLogContent | null
  startupLogsLoading: boolean
  startupLogsError: string | null
  // The project whose read-only info modal is open, or null (closed). Pure
  // presentation of existing ViewModel data — no wire command, no git read.
  projectInfoTarget: string | null
  // The agent (session id) whose read-only info modal is open, or null (closed).
  // Like `projectInfoTarget`, pure presentation of existing ViewModel data.
  agentInfoTarget: string | null
  // The agent (session id) whose force-recreate confirmation is open, or null
  // (closed). Confirmed via ConfirmForceReconnectDialog because a forced
  // reconnect abandons the provider's current conversation for a fresh one.
  forceReconnectTarget: string | null
  // The pending existing-branch attach confirmation, or null (closed). The
  // server refused an unconfirmed create whose name matches an existing branch;
  // this carries the retry params so the confirm re-POSTs with
  // `use_existing_branch: true`. Confirmed via ConfirmUseExistingBranchDialog so
  // an agent never silently adopts an existing branch's history.
  existingBranchTarget: {
    projectId: string
    name: string
    copyChanges: boolean | undefined
    location: "local" | "remote"
  } | null
  addProjectOpen: boolean
  // Why the picker was opened: "add" (the default) or "init" (the split
  // button's "Initialize a repository…" entry). The intent's ONLY effect is a
  // header hint in the dialog; the primary-action ladder does the real work
  // either way. Cleared on close.
  addProjectIntent: "add" | "init"
  browsePath: string
  browseEntries: DirEntryView[]
  browseLoading: boolean
  // Branch pre-flight for the add-project flow, mirroring the TUI's
  // `ConfirmNonDefaultBranch` prompt. When the user selects a git repo the
  // dialog fires `inspectProjectPath`; the reply lands here keyed by `path` so a
  // stale reply for a previously-selected repo is ignored. `loading` drives the
  // dialog's spinner; `warning` null (with a resolved `path`) means the repo is
  // on its default branch — no warning step. `null` overall means no inspection
  // is pending or resolved (nothing selected).
  projectPathInspection: {
    path: string
    // Path classification from the server ("repo" | "bare" | "repo_subdir" |
    // "plain"). Defaults to "repo" while loading / on error / under version
    // skew, so no init or blocked panel appears before inspection confirms it.
    kind: InspectKind
    // The enclosing repository root when `kind` is "repo_subdir" and the
    // server could name one; null inside git's internal directory.
    repoRoot: string | null
    // Starter-.gitignore candidates found in a "plain" folder.
    gitignoreCandidates: string[]
    currentBranch: string | null
    warning: BranchWarningView | null
    // `false` when the repo is a fresh `git init` with no commits (unborn HEAD).
    // Defaults to `true` while loading / on error so the "no commits" offer only
    // appears once inspection confirms it.
    hasCommits: boolean
    error: string | null
    loading: boolean
  } | null
  removeProjectTarget: string | null
  // The project pending a destructive cascade-delete confirmation, or null. The
  // cascade removes the project, its agents, AND their worktrees from disk
  // (delete_worktrees=true); the plain keep-worktrees variant uses
  // `removeProjectTarget` above. Only offered for real projects, so unlike
  // `removeProjectTarget` this routes through the vanish guard.
  deleteProjectTarget: string | null
  // The project pending a default-branch checkout confirmation, or null. The
  // checkout moves the source checkout's HEAD, so the web confirms first (the
  // TUI runs it straight from a deliberate palette/keybinding action).
  checkoutDefaultBranchTarget: string | null
  // The project whose managed worktrees are being browsed for adoption, or null
  // (closed). The dialog requests the listing on open; `attachWorktreeEntries`
  // holds the server's classification and `attachWorktreeLoading` drives the
  // spinner until the `project_worktrees` reply lands. Mirrors the TUI's
  // `new-agent-from-worktree` picker.
  attachWorktreeTarget: string | null
  attachWorktreeEntries: ProjectWorktreeEntryView[]
  attachWorktreeLoading: boolean
  // The name-input dialog target: a fresh agent in a project, a fork of an
  // existing session, or null (closed). One dialog component switches on `kind`.
  createAgentTarget: CreateAgentTarget | null
  // The session pending a rename, or null. The dialog pre-fills the current
  // title (or empty, so the placeholder shows the branch name).
  renameTarget: string | null
  renameDraft: string
  // The session pending a provider swap, or null. The dialog pre-selects the
  // session's current provider; the swap takes effect on the next launch
  // (mirroring the TUI's `change-agent-provider`, which never kills a running
  // agent — it changes the provider for the next reconnect).
  changeProviderTarget: string | null
  // New-agent dialog state lives in the store (like `commitDraft`) so the input
  // is fully store-controlled: the server's generated-name reply fills it via an
  // event-driven callback, never a set-state-in-effect. Mirrors the TUI prompt.
  //   - `createAgentDraft`: the sanitized branch-name input.
  //   - `createAgentRandomize`: the "Use randomized pet name" checkbox.
  //   - `createAgentGeneratedName`: the last name the server generated, so an
  //     uncheck clears the input ONLY when it still equals that name (exact TUI
  //     semantics); null once the user edits away from it or no name is pending.
  createAgentDraft: string
  createAgentRandomize: boolean
  //   - `createAgentCopyChanges`: the "Copy uncommitted changes from the
  //     project checkout" checkbox. Seeded from the bootstrap's config default
  //     when the dialog opens; only "new" mode surfaces it (forks always copy,
  //     the other flows never do).
  createAgentCopyChanges: boolean
  createAgentGeneratedName: string | null
  //   - `createAgentPrInput`: the raw PR reference (URL, `#123`, or `123`) for
  //     the "From PR" mode. Free text (NOT agent-name-sanitized); the server
  //     parses it against the project's GitHub remote. Empty in the other modes.
  createAgentPrInput: string
  //   - `createAgentNamePending`: a generate-name request is in flight. Drives
  //     the dialog's spinner and disables the input so a late reply can never
  //     clobber text the user typed in the meantime. Explicit rather than
  //     inferred from an empty draft, so manually clearing the input doesn't
  //     fake a phantom "generating" state.
  createAgentNamePending: boolean
  // The Task Manager (the app menu's "Task Manager…"). Lists every running agent
  // tab and companion terminal with its CPU/memory/process count, and stops each
  // on demand (agents detach and can be reconnected; terminals are destroyed).
  // The rows are derived live from the spine joined to the polled stats, so the
  // dialog needs no state beyond this open flag.
  taskManagerOpen: boolean
  // Whether the "Stop all…" confirmation (nested inside the Task Manager) is up.
  // Every stop confirms, the bulk one most of all: it ends every agent and
  // terminal at once.
  stopAllOpen: boolean
  // The Monaco config.toml editor (the app menu's "Edit config file…"). `configEditorOpen`
  // gates the modal; the raw text is fetched into `configEditorContent` on open
  // so the editor seeds from a settled value (no set-state-in-effect).
  // `configEditorLoading` drives the load spinner; `configEditorError` shows the
  // server's inline validation/parse message when a save is rejected, WITHOUT
  // closing the modal so the user can fix the TOML.
  configEditorOpen: boolean
  configEditorContent: string
  configEditorLoading: boolean
  configEditorError: string | null
  // The Preferences dialog (the app menu's "Preferences…"). Gates the
  // modal that sets the browser tab title + favicon colour + Changes pane
  // visibility; the dialog seeds its fields from the bootstrap document, so it
  // needs no state beyond this flag.
  customizeWebappOpen: boolean
  // The macro-editor dialog. `macrosDialogOpen` gates the modal; `macrosDraft`
  // is the working copy of the whole macro list the user edits before saving
  // (the save is wholesale — `update_macros` replaces the entire `[macros]`
  // map, mirroring the TUI editor). Seeded from `bootstrap.macros` on open so
  // there is no set-state-in-effect. Empty draft when closed.
  macrosDialogOpen: boolean
  macrosDraft: MacroView[]
  // Which screen the mobile shell is showing. Always "home" on desktop, which
  // ignores it. Only the mobile UI advances it past "home".
  mobileScreen: MobileScreen
  // Optimistic drag-and-drop ordering overlays (see `applyPendingOrders`). Each
  // is set the moment a drag ends and cleared once the server's next spine
  // confirms the new order (or an error status arrives). Null when no reorder is
  // in flight, which is the overwhelmingly common case.
  pendingSessionOrder: PendingSessionOrder | null
  pendingProjectOrder: string[] | null
  // Optimistic overlay for the flat model's GLOBAL agent order: the complete list
  // of session ids in the just-dragged order, cleared once the spine confirms it.
  pendingAgentOrder: string[] | null
  // Optimistic overlay for the flat Terminals section's GLOBAL order: the complete
  // list of terminal ids (any owner) in the just-dragged order, cleared once the
  // spine confirms it. Mirrors `pendingAgentOrder` exactly (see `reorderTerminals`).
  pendingTerminalOrder: string[] | null
  // While an agent-create THIS client initiated is in flight, holds the session
  // ids that already existed when we submitted, plus the project the new agent
  // will land in. Agent creation is an async server job whose only completion
  // signal is a `sessions.changed` event + spine refetch (no per-client reply, no request/echo
  // correlation), so we recognize "our" new agent as the session id that appears
  // in `projectId` and wasn't in `knownIds`, then focus it — mirroring the TUI,
  // which jumps selection to a freshly created agent when its launch completes.
  // Only the client that armed this reacts, so other connected clients aren't
  // yanked off whatever they're viewing. Null when no create is awaiting focus.
  // See `armCreateFocus` and `focusNewlyCreatedSession`.
  // `armedAt` (epoch ms) bounds the token's lifetime: a create that never lands
  // (the dispatch failed silently server-side, or the agent took absurdly long)
  // would otherwise leave the token armed forever, ready to mis-focus the next
  // unrelated session that happens to appear in `projectId`. See
  // `CREATE_FOCUS_TTL_MS` and `focusNewlyCreatedSession`.
  pendingCreateFocus: {
    knownIds: string[]
    projectId: string
    armedAt: number
  } | null
  // Explicit project expand/collapse choices, keyed by project id. A project not
  // present here falls back to its default (open when it has agents). The sidebar
  // reads this so a collapse survives re-renders, and creating an agent under a
  // collapsed project can force it open (see `focusNewlyCreatedSession`).
  projectOpen: Record<string, boolean>
  // The flat agent list's display sort (shared by desktop + mobile), persisted
  // SERVER-SIDE in `config.ui.agent_sort` so it survives restarts and every client
  // agrees. This field is the optimistic OVERRIDE (null = follow config), reconciled
  // by applyBootstrap exactly like `changesPaneOverride`; the effective mode is
  // `agentSort ?? bootstrap.agent_sort ?? "active"`. A drag flips it to "manual" so
  // the dropped order (stored in SQLite) sticks. Active-first is recomputed live.
  agentSort: FlatSortKey | null
  // The shared search query filtering the flat agent/terminal list on both
  // surfaces. Empty string shows everything.
  agentSearch: string
  // Whether the New-agent picker dialog is open. The picker is the home for agent
  // creation and every project action now that there are no project headers.
  newAgentPickerOpen: boolean
  // How the New-agent picker was opened, so it can guide the right creation flow:
  // "new" (pick project + provider + Create), "from_pr" (pick a project to create
  // an agent from a PR), or "from_worktree" (pick a project to adopt an existing
  // worktree). The split button's ⋯ menu sets this; a bare open defaults to "new".
  newAgentPickerIntent: "new" | "from_pr" | "from_worktree"
  sidebarWidth: string
  // Optimistic override for the Changes pane's visibility (desktop). `null`
  // follows the persisted config (`bootstrap.show_changes_pane`); the palette and
  // the Changes actions menu set an explicit bool for instant feedback. The
  // toggle persists to config via the server; this clears once the broadcast
  // confirms (or on command error / disconnect, which roll it back).
  changesPaneOverride: boolean | null
  // The session whose code-editor overlay is open, the file to auto-open on
  // launch (null = none preselected), and the view it opens in: "file" (editable
  // Monaco buffer) or "diff" (read-only Monaco DiffEditor, HEAD vs working copy).
  // The editor always operates on the SELECTED session, so opening it selects
  // that session first and reuses the existing changed-files broadcast for its
  // file list. Null = overlay closed.
  editorTarget: {
    sessionId: string
    initialPath: string | null
    initialMode: EditorViewMode
  } | null
  // Per-session editor tab metadata (pure client state; the heavy Monaco
  // buffers live in the `EditorBody` component, keyed by tab id). Keyed by
  // session id so reopening a session's editor restores its tab list. A
  // session absent here has no tabs (not yet opened, or cleared on delete).
  editorTabs: Record<string, EditorTabsState>
  // The tab pending the dirty-close confirmation (destructive-confirm
  // pattern), or null. Closing a NON-dirty tab skips this and closes directly.
  editorCloseTabTarget: { sessionId: string; tabId: string } | null
  // Changed-files state for the selected session (see `ChangesSlice`). The single
  // source for changed-files data — replaces the global `viewModel.changed_files`
  // broadcast, which a second client could clobber.
  changes: ChangesSlice
}

// Which view the code editor opens in (and toggles between): the editable Monaco
// buffer, or the read-only Monaco diff (HEAD vs working copy). Opening a changed
// file defaults to "diff"; the file tree / edit actions default to "file".
export type EditorViewMode = "file" | "diff"

// The expanded sidebar width is drag-resizable and persisted across reloads.
// 18rem gives agent names breathing room next to the PR/status badges; a
// previously persisted width still wins.
const SIDEBAR_WIDTH_KEY = "dux:sidebar-width"
const DEFAULT_SIDEBAR_WIDTH = "18rem"

function loadSidebarWidth(): string {
  return localStorage.getItem(SIDEBAR_WIDTH_KEY) || DEFAULT_SIDEBAR_WIDTH
}

// One-time cleanup: the diff line-number toggle (and its persisted preference)
// went away when the web diff moved to Monaco, which manages its own gutters.
// Drop the orphaned key so it can't linger or be misread by a future feature.
localStorage.removeItem("dux:show-diff-line-numbers")

// The `/ws/events` topic for one session's changed files.
function changesTopic(sessionId: string): string {
  return `session:${sessionId}:changes`
}

// A cleared changed-files slice (nothing selected, no data).
function emptyChanges(): ChangesSlice {
  return {
    sessionId: null,
    phase: "idle",
    rev: 0,
    staged: [],
    unstaged: [],
    error: null,
  }
}

// A fresh slice for `sessionId` entering its loading window. `rev: 0` so the
// first successful response (rev >= 1 from the server) always applies.
function loadingChanges(sessionId: string): ChangesSlice {
  return {
    sessionId,
    phase: "loading",
    rev: 0,
    staged: [],
    unstaged: [],
    error: null,
  }
}

let state: DuxState = {
  spine: null,
  bootstrap: null,
  booted: false,
  conn: "connecting",
  offline: false,
  selectedTarget: null,
  selectedSessionId: null,
  terminalEpoch: 0,
  commitTarget: null,
  commitDraft: "",
  deleteTarget: null,
  deleteTerminalTarget: null,
  closeTabTarget: null,
  createTabInFlight: [],
  startedDormantTabs: [],
  discardTarget: null,
  globalEnvOpen: false,
  projectSettingsTarget: null,
  agentStartupCommandTarget: null,
  agentEnvTarget: null,
  startupLogsTarget: null,
  startupLogsEntries: [],
  startupLogsSelected: null,
  startupLogsLoading: false,
  startupLogsError: null,
  projectInfoTarget: null,
  agentInfoTarget: null,
  forceReconnectTarget: null,
  existingBranchTarget: null,
  addProjectOpen: false,
  addProjectIntent: "add",
  browsePath: "",
  browseEntries: [],
  browseLoading: false,
  projectPathInspection: null,
  removeProjectTarget: null,
  deleteProjectTarget: null,
  checkoutDefaultBranchTarget: null,
  attachWorktreeTarget: null,
  attachWorktreeEntries: [],
  attachWorktreeLoading: false,
  createAgentTarget: null,
  renameTarget: null,
  renameDraft: "",
  changeProviderTarget: null,
  createAgentDraft: "",
  createAgentRandomize: false,
  createAgentCopyChanges: true,
  createAgentGeneratedName: null,
  createAgentNamePending: false,
  createAgentPrInput: "",
  taskManagerOpen: false,
  stopAllOpen: false,
  configEditorOpen: false,
  configEditorContent: "",
  configEditorLoading: false,
  configEditorError: null,
  customizeWebappOpen: false,
  macrosDialogOpen: false,
  macrosDraft: [],
  mobileScreen: "home",
  pendingSessionOrder: null,
  pendingProjectOrder: null,
  pendingAgentOrder: null,
  pendingTerminalOrder: null,
  pendingCreateFocus: null,
  projectOpen: {},
  agentSort: null,
  agentSearch: "",
  newAgentPickerOpen: false,
  newAgentPickerIntent: "new",
  sidebarWidth: loadSidebarWidth(),
  changesPaneOverride: null,
  editorTarget: null,
  editorTabs: {},
  editorCloseTabTarget: null,
  changes: emptyChanges(),
}

const listeners = new Set<() => void>()

function emit(): void {
  for (const listener of listeners) listener()
}

function setState(patch: Partial<DuxState>): void {
  state = { ...state, ...patch }
  emit()
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

// The external-store snapshot accessor `useSyncExternalStore` consumes. Exported
// so unit tests can read the live state after dispatching an action (there is no
// React harness in this test setup); production code reads it only via `useDux`.
export function getSnapshot(): DuxState {
  return state
}

// Derive the WebSocket scheme from the page protocol so an HTTPS deployment uses
// `wss://` (a hardcoded `ws://` would be blocked as mixed content under HTTPS).
const wsScheme = location.protocol === "https:" ? "wss:" : "ws:"

// The single JSON socket for the whole app (`/ws/events`), separate from the
// per-PTY byte sockets (`lib/ptySocket.ts`). Since the Phase 6 cutover it carries
// EVERYTHING the retired `/ws`/`DuxSocket` used to: resource-change events
// (changed files, spine, config) AND the control frames
// (`connected` id, `status`/`status_cleared` toasts). It also owns the
// connection-state UX (the status-bar indicator). Exported so tests can drive
// its callbacks / inspect its interest set; connected on boot.
export const eventsSocket = new EventsSocket(
  `${wsScheme}//${location.host}/ws/events`,
)

// App-wide coarse topics, subscribed once at module load. They are added to the
// interest set immediately (sent on the first open, re-sent on every reconnect).
// Phase 1 has no GET tied to these — they exist so later phases can refresh
// projects/sessions/config off the same channel without a new subscribe site.
eventsSocket.subscribe(["sessions", "projects", "config"])

// A `session.changes` event invalidates one session's changed files. Refetch
// when it is the selected session AND (the slice is in error — always re-fetch
// to self-heal, since the error path has no usable rev) OR the event's rev is at
// least the applied rev. Lag catch-up arrives as the same event, so this one
// handler covers it.
eventsSocket.onEvent = (ev: EventsServerMessage) => {
  // The per-connection id, delivered as the FIRST `/ws/events` frame (and re-sent
  // on every reconnect). Record it so the REST clients can stamp it as
  // `X-Connection-Id` and the server scopes their status toasts back to us.
  if (ev.event === "connected") {
    if (typeof ev.id === "string") setConnectionId(ev.id)
    return
  }
  // Engine status toasts, migrated off the retired `/ws`. The server already
  // scope-filtered, so the `scope` field is ignored client-side. An error-toned
  // async status also voids any in-flight create-focus (the create likely failed)
  // and unwinds any optimistic reorder overlay — mirroring the old `onStatus`.
  if (ev.event === "status") {
    if (ev.tone === "error") setState({ ...clearPendingClientIntent() })
    // A `tab-launch-<tabId>` keyed status carries BOTH outcomes of an extra-tab
    // launch: a `warning` on failure and an `info` on success (both keyed the same
    // so neither is clobbered by an unrelated toast). Only the FAILURE clears that
    // tab's "explicitly started" latch — so `isExtraTabDormant` goes true again,
    // the dormant retry card returns, and the pane stops auto-reconnecting the
    // failing launch. The SUCCESS path must NOT touch the latch here: the latch is
    // cleared race-free by `applySpine` in the same tick it flips `has_live_process`
    // true, and stripping it early (before the spine refetch lands) would briefly
    // re-mark the tab dormant, unmounting the just-launched pane and flashing the
    // "Start session" card back until the spine catches up.
    if (ev.tone === "warning" && ev.key?.startsWith("tab-launch-")) {
      const tabId = ev.key.slice("tab-launch-".length)
      if (state.startedDormantTabs.includes(tabId)) {
        setState({
          startedDormantTabs: state.startedDormantTabs.filter(
            (id) => id !== tabId,
          ),
        })
      }
    }
    showStatusToast(ev.key, ev.tone ?? "info", ev.message ?? "")
    return
  }
  // Dismiss the toast whose id matches the cleared key (anonymous slot when null).
  if (ev.event === "status_cleared") {
    toast.dismiss(ev.key ?? ANON_TOAST_ID)
    return
  }
  // A `config.changed` event invalidates the bootstrap document (the server's
  // config was edited/reloaded). Re-GET it so providers, macros, UI flags, etc.
  // reflect the new config without a reconnect. The `config` coarse topic is
  // subscribed at module load, so this fires for every client.
  if (ev.event === "config.changed") {
    loadBootstrap()
    return
  }
  // A `projects.changed` / `sessions.changed` event invalidates the workspace
  // spine (a project/session was added, removed, reordered, renamed, changed
  // status, etc.). Re-GET it so the sidebar, session lists, and selection logic
  // reflect the new state. The `projects`/`sessions` coarse topics are subscribed
  // at module load, so this fires for every client. The applied spine drives the
  // focus/prune/reorder reconciliation (see `applySpine`).
  if (ev.event === "projects.changed" || ev.event === "sessions.changed") {
    loadSpine()
    return
  }
  // A `pty.owner` event means a connection claimed (took over, or first-claimed an
  // unowned) PTY's sizing+input. Fan it out to the mounted terminal view for that
  // pty id along with the claimer's connection id (`owner`); the view compares that
  // against its own PTY-socket connection id to decide definitively whether it is
  // the owner (stays interactive) or has been taken over (read-only placeholder).
  // The id is the pty id (session id for an agent, terminal id for a companion).
  // Delivered on the coarse `sessions` topic, subscribed at module load.
  if (ev.event === "pty.owner") {
    // Pass the ownership epoch so the fan-out can ignore an out-of-order (older)
    // handover and converge on the latest claim regardless of arrival order.
    if (typeof ev.id === "string")
      notifyPtyOwner(ev.id, ev.owner, ev.epoch, ev.device)
    return
  }
  if (ev.event !== "session.changes") return
  const id = ev.id
  if (id === undefined || id !== state.selectedSessionId) return
  // A missing `rev` (the server's Lagged catch-up for a cold session carries
  // none) is a force-refetch: we can't compare it, so we must NOT let
  // `undefined >= rev` short-circuit to false and skip the refetch.
  const rev = ev.rev
  if (
    state.changes.phase === "error" ||
    rev === undefined ||
    rev >= state.changes.rev
  ) {
    loadChanges(id)
  }
}

// The `boot()` driver kicks off the very first bootstrap+spine load alongside
// `eventsSocket.connect()`, so the first `onOpen` that follows must NOT re-fetch
// them or it would duplicate that initial load. The driver sets this flag right
// before connecting; the first `onOpen` consumes it. Every
// later (RE-connect) open leaves it false and so always retries — crucially, even
// when the FIRST load FAILED: keying the retry off `state.bootstrap !== null`
// (the old guard) stranded a failed first fetch as null forever, so every
// reconnect skipped it and the app stayed empty with no recovery path.
let skipNextEventsOnOpenLoad = false

// After a (re)connect the socket has re-sent the whole interest set; re-fetch so
// anything missed while disconnected is recovered (an event that arrived during
// the outage is gone otherwise). The `config` coarse topic is always subscribed,
// so refetch the bootstrap document too — a `config.changed` missed during the
// outage would otherwise leave stale providers/macros/UI flags until the next
// config edit. The selected session's changes are also recovered when one is set.
eventsSocket.onOpen = () => {
  if (skipNextEventsOnOpenLoad) {
    // First open after a boot/login load: skip the duplicate fetch this once.
    skipNextEventsOnOpenLoad = false
  } else {
    // A reconnect (or an open the driver did not pre-load for): re-fetch both so
    // anything missed during the outage — or a load that failed on first boot —
    // recovers. Concurrent loads are safe: spine is seq-guarded and bootstrap
    // apply is idempotent.
    //
    // Capture the deep-linked route BEFORE `loadSpine` so a transient exit-eject
    // during the reconnect (the center pane resets to home while the agent is
    // momentarily `detached`) can be re-restored once the agent resumes. Reading
    // the hash here — before any spine apply runs — beats that eject wiping it.
    armReconnectDeepLink()
    loadBootstrap()
    loadSpine()
    // The server's ownership epoch counter restarts at zero if the server itself
    // restarted during the outage; clear our per-pty high-water marks so a fresh
    // post-restart `pty.owner` is not wrongly ignored as stale. A reconnect is the
    // only path a restarted server's epochs reach us, and there is no `pty.owner`
    // replay, so this can never drop a still-relevant in-flight handover.
    resetPtyOwnerEpochs()
  }
  const id = state.selectedSessionId
  if (id === null) return
  setState({ changes: loadingChanges(id) })
  loadChanges(id)
}

// Move the changed-files subscription from one session to another. A null side
// means "no session" (clear/select-nothing). A no-op when unchanged.
function switchChangesSubscription(
  prev: string | null,
  next: string | null,
): void {
  if (prev === next) return
  if (prev !== null) eventsSocket.unsubscribe([changesTopic(prev)])
  if (next !== null) eventsSocket.subscribe([changesTopic(next)])
}

// Fire a changed-files fetch for `sessionId` and route the outcome through the
// guarded apply/error handlers. Errors are caught here so a failed fetch can
// never surface as an unhandled rejection.
function loadChanges(sessionId: string): void {
  fetchChanges(sessionId)
    .then((resp) => applyChangesResponse(sessionId, resp))
    .catch((err) => applyChangesError(sessionId, err))
}

// Apply a fetch response, dropping it when it lost a race. Two guards:
//   1. the requested session must still be selected AND own the slice (a fast
//      session switch already moved on); and
//   2. the response `rev` must be >= the applied `rev` (an older, out-of-order
//      response must not overwrite newer data).
function applyChangesResponse(
  sessionId: string,
  resp: SessionChangesResponse,
): void {
  if (state.selectedSessionId !== sessionId) return
  if (state.changes.sessionId !== sessionId) return
  if (resp.rev < state.changes.rev) return
  setState({
    changes: {
      sessionId,
      phase: "loaded",
      rev: resp.rev,
      staged: resp.staged,
      unstaged: resp.unstaged,
      error: null,
    },
  })
}

// Apply a failed fetch. A 404 means the session is gone — clear the slice (the
// next spine's `pruneSelectionIfGone` clears the selection). Anything else
// (409 git lock, 5xx, network) lands in `error` so the pane shows a Refresh
// affordance; the poller's eventual recovery event self-heals it. Same staleness
// guards as the success path so a late failure can't clobber a newer state.
function applyChangesError(sessionId: string, err: unknown): void {
  if (state.selectedSessionId !== sessionId) return
  if (state.changes.sessionId !== sessionId) return
  if (err instanceof ChangesFetchError && err.status === 404) {
    setState({ changes: emptyChanges() })
    return
  }
  // Only the fetch that opened the current loading window may flip the slice to
  // error. A late failure that lost the race to a successful concurrent fetch
  // (e.g. a slow 409 arriving after a newer 200 already loaded the pane) must
  // not turn a loaded pane into an error pane. The next `session.changes` event
  // still self-heals an error state regardless.
  if (state.changes.phase !== "loading") return
  const message =
    err instanceof Error ? err.message : "Could not load changed files."
  setState({
    changes: { ...state.changes, sessionId, phase: "error", error: message },
  })
}

// Re-fetch the selected session's changes (the changes pane's Refresh button).
// No-op when nothing is selected.
export function refreshChanges(): void {
  const id = state.selectedSessionId
  if (id === null) return
  setState({ changes: loadingChanges(id) })
  loadChanges(id)
}

// Fetch the bootstrap document and fold it into state. Errors are swallowed: on
// first boot the slice stays `null` (consumers fall back to defaults) and a
// later `config.changed` event or a reconnect retries; on a refetch the last
// good bootstrap is kept rather than blanking the UI. Never surfaces as an
// unhandled rejection.
function loadBootstrap(): void {
  fetchBootstrap()
    .then((b) => applyBootstrap(b))
    .catch((err) => {
      // Keep the previous bootstrap (null on first boot); a config.changed event
      // or reconnect will retry. Warn so a persistently-failing fetch (e.g. a
      // first boot that stays empty) is visible in the console rather than silent.
      console.warn("[dux] bootstrap fetch failed; will retry on reconnect", err)
    })
}

// Apply a freshly fetched bootstrap. Also reconciles the optimistic Changes-pane
// override the same way the broadcast ViewModel used to: the toggle persists to
// config, the server emits `config.changed`, the refetched bootstrap carries the
// confirmed `show_changes_pane`, and the override is dropped once it matches so
// config becomes the single source of truth across every client.
function applyBootstrap(b: Bootstrap): void {
  setState({
    bootstrap: b,
    changesPaneOverride:
      state.changesPaneOverride !== null &&
      state.changesPaneOverride === b.show_changes_pane
        ? null
        : state.changesPaneOverride,
    // Same reconcile as changesPaneOverride: drop the optimistic sort override once
    // the refetched config confirms it, so config.ui.agent_sort is the single truth.
    agentSort:
      state.agentSort !== null && state.agentSort === b.agent_sort
        ? null
        : state.agentSort,
  })
  // Reflect the configured instance name and favicon in the browser tab, plus the
  // live attention count/dot. Guarded inside `refreshAttentionChrome` because the
  // store also runs under the Node test environment, where `document` is absent
  // unless a test stubs it. Runs on first load and on every config.changed
  // refetch, so a live rename updates the tab (and re-applies the current dot)
  // without a reload.
  refreshAttentionChrome()
}

// The instance title/favicon carry a live "needs attention" overlay: a `(N) `
// count prefix on the browser-tab title and a cyan dot composited onto the
// favicon, both driven by how many agents are flagged in the current spine. This
// runs whenever the count could change (a spine apply) or the base title/favicon
// changes (a bootstrap/config.changed). `applyAttentionFavicon` composes at most
// once per state and no-ops when nothing changed, so calling this on every spine
// apply is cheap. Self-guards on the DOM.
function refreshAttentionChrome(): void {
  if (typeof document === "undefined") return
  const count = attentionCount(state.spine?.sessions ?? [])
  const base = resolveInstanceTitle(state.bootstrap?.title)
  document.title = formatTabTitle(base, count)
  applyAttentionFavicon(state.bootstrap?.favicon, count > 0)
}

// Monotonic sequence for spine loads. Two rapid `sessions.changed`/
// `projects.changed` events fire concurrent `fetchSpine()`s; without a guard an
// older response resolving last would overwrite a newer spine (observable as a
// focus-then-prune-clear flicker on agent create). Each `loadSpine` captures the
// seq it bumped to; `applySpine` discards a result once a newer load has started.
// Mirrors the `applyChangesResponse` rev-guard, but with a client-side counter
// (the spine read has no server rev).
let loadSpineSeq = 0

// Fetch the workspace spine and fold it into state. Errors are swallowed: on
// first boot the slice stays `null` (consumers fall back to empty lists) and a
// later `projects.changed`/`sessions.changed` event or a reconnect retries; on a
// refetch the last good spine is kept rather than blanking the sidebar. Never
// surfaces as an unhandled rejection.
function loadSpine(): void {
  const seq = ++loadSpineSeq
  fetchSpine()
    .then((s) => applySpine(s, seq))
    .catch((err) => {
      // Keep the previous spine (null on first boot); an event or reconnect will
      // retry. Warn so a persistently-failing fetch (e.g. a first boot that stays
      // empty) is visible in the console rather than silent.
      console.warn("[dux] spine fetch failed; will retry on reconnect", err)
    })
}

// Apply a freshly fetched spine. This is the single place the projects/sessions/
// sidebar data lands, and it drives the same client-view reconciliation the
// broadcast ViewModel used to:
//   - retire the optimistic reorder overlays once the server's order matches;
//   - auto-focus an agent THIS client just created, the instant it appears;
//   - prune the selection when its target session/terminal has vanished.
// Order mirrors the legacy `onViewModel`: set the slice (with reconciled overlays)
// first, then focus (which only ever selects a session present in the spine, so
// the prune below leaves it alone), then prune.
//
// `seq` is the `loadSpineSeq` value the originating `loadSpine` captured; discard
// this (now-stale) result if a newer load has since started, so a slow older
// response can never overwrite a fresher spine (and re-run focus/prune against
// outdated data).
function applySpine(rawSpine: Spine, seq: number): void {
  if (seq < loadSpineSeq) return
  // `tabs` is normalized to an array at the fetch boundary (`fetchSpine`), so an
  // older server that omits the field degrades to an empty strip rather than
  // throwing on the `session.tabs` derefs downstream.
  const spine = rawSpine
  // Clear the "explicitly started" latch for any tab whose process is now live.
  // The latch only bridges the click->process-up gap; once the process is up it
  // is no longer needed, and dropping it means a *later* exit (has_live_process
  // back to false) correctly re-shows the dormant card so a plain refocus can't
  // force-launch. Tabs that are started-but-not-yet-live keep their latch.
  const liveTabIds = new Set(
    spine.sessions.flatMap((s) => s.tabs.filter((t) => t.has_live_process).map((t) => t.id)),
  )
  const prunedDormant = state.startedDormantTabs.filter((id) => !liveTabIds.has(id))
  setState({
    spine,
    startedDormantTabs:
      prunedDormant.length === state.startedDormantTabs.length ? state.startedDormantTabs : prunedDormant,
    pendingSessionOrder: reconcilePendingSessionOrder(spine, state.pendingSessionOrder),
    pendingProjectOrder: reconcilePendingProjectOrder(spine, state.pendingProjectOrder),
    pendingAgentOrder: reconcilePendingAgentOrder(spine, state.pendingAgentOrder),
    pendingTerminalOrder: reconcilePendingTerminalOrder(spine, state.pendingTerminalOrder),
  })
  // Restore a boot-time deep-link before focus/prune: it selects only a session
  // present in this spine (so prune leaves it alone), and it is a one-shot that
  // self-clears, so it never fights a create-focus or a later refetch.
  restoreDeepLink(spine)
  focusNewlyCreatedSession(spine)
  pruneSelectionIfGone(spine)
  pruneEditorStateIfGone(spine)
  // Re-restore a reconnect deep-link once its agent is present and back to
  // `active`, undoing a transient exit-eject that fired during the reconnect.
  restoreReconnectDeepLink(spine)
  // The flagged-agent count may have changed with this spine: refresh the
  // browser-tab count prefix and the favicon dot. Backgrounded tabs update too,
  // since spines arrive from server pushes without a visit.
  refreshAttentionChrome()
}

// Drop editor-tab state for any session that no longer exists in the spine
// (deleted here or by another client), and close the editor overlay if it was
// pointed at that now-gone session, the code-editor's own out-of-band-clear
// path, mirroring `pruneSelectionIfGone` for the main selection.
function pruneEditorStateIfGone(spine: Spine): void {
  const liveSessionIds = new Set(spine.sessions.map((s) => s.id))
  for (const sessionId of Object.keys(state.editorTabs)) {
    if (!liveSessionIds.has(sessionId)) editorClearSession(sessionId)
  }
  if (state.editorTarget && !liveSessionIds.has(state.editorTarget.sessionId)) {
    closeEditor()
  }
}

// The per-connection id now arrives as the `connected` event on `/ws/events`
// (see `eventsSocket.onEvent` above), which calls `setConnectionId`. The REST
// clients stamp it as `X-Connection-Id` so the server scopes their toasts back
// to this client.

// The broadcast ViewModel now carries ONLY `changed_files` (a residual frame);
// projects/sessions/sidebar moved to `GET /api/v1/spine` and the changed-files
// data is owned by the `changes` slice over REST. Nothing reads the residual
// frame anymore, so we deliberately do NOT install an `onViewModel` handler:
// storing it on every frame only triggered spurious re-renders. The
// focus/prune/reorder reconciliation runs on the spine apply path (`applySpine`),
// and changed files flow through the `changes` slice. The frame is removed at
// cutover (Phase 6); until then it is simply ignored (the socket default no-op).

// Drop the pending session-order overlay once the incoming spine's session
// order for that project already equals the overlay; otherwise keep it.
function reconcilePendingSessionOrder(
  spine: Spine,
  pending: PendingSessionOrder | null,
): PendingSessionOrder | null {
  if (!pending) return null
  const serverIds = spine.sessions
    .filter((s) => s.project_id === pending.projectId)
    .map((s) => s.id)
  return ordersMatch(serverIds, pending.ids) ? null : pending
}

// Drop the pending project-order overlay once the incoming spine's project
// order already equals the overlay; otherwise keep it.
function reconcilePendingProjectOrder(
  spine: Spine,
  pending: string[] | null,
): string[] | null {
  if (!pending) return null
  const serverIds = spine.projects.map((p) => p.id)
  return ordersMatch(serverIds, pending) ? null : pending
}

// Drop the global agent-order overlay once the server's session order (spine is
// already in global sort_order) matches what we optimistically applied. The
// server's list is the full session set, so compare against every session id.
function reconcilePendingAgentOrder(
  spine: Spine,
  pending: string[] | null,
): string[] | null {
  if (!pending) return null
  const serverIds = spine.sessions.map((s) => s.id)
  return ordersMatch(serverIds, pending) ? null : pending
}

// Mirror of `reconcilePendingAgentOrder` for the flat Terminals section. Terminals
// are split across `sessions[].terminals` and `projects[].terminals` in the spine,
// so the authoritative flat order is EVERY terminal (any owner) sorted by its
// global `sort_order` (which a reorder restamps to the dragged order). The overlay
// clears once that server order matches what we optimistically applied.
function reconcilePendingTerminalOrder(
  spine: Spine,
  pending: string[] | null,
): string[] | null {
  if (!pending) return null
  const all = [
    ...spine.sessions.flatMap((s) => s.terminals),
    ...spine.projects.flatMap((p) => p.terminals),
  ]
  const serverIds = all
    .slice()
    .sort((a, b) => a.sort_order - b.sort_order)
    .map((t) => t.id)
  return ordersMatch(serverIds, pending) ? null : pending
}

// Clear the selection when its target no longer exists in the latest spine.
// Agents persist after exiting (their session stays, marked detached), so they
// only vanish on deletion; terminals are removed outright when their PTY exits.
function pruneSelectionIfGone(spine: Spine): void {
  const target = state.selectedTarget
  if (!target) return
  if (target.kind === "agent") {
    const session = spine.sessions.find((s) => s.id === target.sessionId)
    // The session must still exist; if an extra tab is focused, it must still be
    // in that session's tab list (an extra tab can be closed by ANOTHER client,
    // whose local retarget-to-session-slot never ran here — this is the shared-workspace
    // heal path). A gone extra tab falls back to the session-slot tab rather than
    // ejecting the user to the welcome screen.
    if (!session) {
      selectSession(null)
    } else if (
      target.tabId !== target.sessionId &&
      !session.tabs.some((t) => t.id === target.tabId)
    ) {
      selectSession(target.sessionId)
    }
    return
  }
  // A terminal: it must still exist UNDER its owner. Branch on the owner kind:
  // a project terminal is scoped to `spine.projects`, never to a session (the
  // old `?? false` here made every project terminal look vanished and ejected
  // the user to home on every spine apply).
  const owner = target.owner
  const stillExists =
    owner.kind === "session"
      ? (spine.sessions
          .find((s) => s.id === owner.sessionId)
          ?.terminals.some((t) => t.id === target.terminalId) ?? false)
      : (spine.projects
          .find((p) => p.id === owner.projectId)
          ?.terminals.some((t) => t.id === target.terminalId) ?? false)
  if (!stillExists) {
    // `selectSession(null)` clears the target and, on mobile, unwinds the spoke
    // so the back stack matches the screen (see `unwindMobileSpoke`). This is
    // the other out-of-band clear path: a terminal whose PTY exited is dropped
    // from the ViewModel while the user may be sitting in its spoke.
    selectSession(null)
  }
}

// Snapshot the session ids that exist right now and arm auto-focus for an agent
// THIS client is creating, so the next spine carrying a new id in `projectId`
// is recognized as our new agent and focused (see `focusNewlyCreatedSession`).
// Call this immediately before dispatching an agent-create command; it is wired
// into `submitNameDialog` (new/fork/from-PR) and `attachWorktree`. Re-arming
// overwrites any prior pending focus, so a fresh create supersedes an earlier
// one whose agent never arrived. Always pass the project the new agent will land
// in — the match is project-scoped, so a caller that cannot resolve the project
// must skip arming rather than pass a placeholder.
// How long an armed create-focus token stays live before it self-expires. Set
// comfortably above the longest server-side create window (the from-PR create
// awaits up to 60s — see `FROM_PR_CREATE_AWAIT_TIMEOUT`) so a legitimate slow
// create still auto-focuses, but bounded so a create that never lands cannot keep
// a stale token armed to grab a later, unrelated session.
const CREATE_FOCUS_TTL_MS = 90_000

function armCreateFocus(projectId: string): void {
  const knownIds = (state.spine?.sessions ?? []).map((s) => s.id)
  setState({ pendingCreateFocus: { knownIds, projectId, armedAt: Date.now() } })
}

// Focus the agent THIS client just created, the instant it shows up. With a
// pending-focus token armed (`armCreateFocus`), scan the incoming spine for a
// session that wasn't known at submit time and lives in the expected project,
// select it (which points the changed-files watch at it; the focused TerminalPane
// subscribes its PTY on mount), and disarm. No-op — and cheap — when nothing is
// pending, the overwhelmingly common case. Other clients never armed a token, so
// they don't react: focus moves only on the client that initiated the create.
function focusNewlyCreatedSession(spine: Spine): void {
  const pending = state.pendingCreateFocus
  if (!pending) return
  // Expire a stale token rather than letting it focus an unrelated session that
  // appears long after the create it was armed for (a silently-failed create, or
  // one that never completed). Disarm and bail.
  if (Date.now() - pending.armedAt > CREATE_FOCUS_TTL_MS) {
    setState({ pendingCreateFocus: null })
    return
  }
  const known = new Set(pending.knownIds)
  const created = spine.sessions.find(
    (s) => !known.has(s.id) && s.project_id === pending.projectId,
  )
  if (!created) return
  // Consume the token before selecting so a later spine can't re-fire.
  setState({ pendingCreateFocus: null })
  // Force the owning project open so the new agent is actually visible — a
  // project the user had collapsed would otherwise hide the row we just
  // selected.
  setProjectOpen(created.project_id, true)
  selectSession(created.id)
}

// Record an explicit expand/collapse choice for a project. The sidebar reads
// `projectOpen[id]`, falling back to the default (open when it has agents) when
// absent.
export function setProjectOpen(projectId: string, open: boolean): void {
  if (state.projectOpen[projectId] === open) return
  setState({ projectOpen: { ...state.projectOpen, [projectId]: open } })
}

eventsSocket.onConn = (conn) => {
  // A connection break invalidates any in-flight optimistic reorder: the
  // command (or its rejection) may have been lost, and after the reconnect
  // nothing would ever reconcile a non-matching overlay — leaving the UI
  // showing an order the server never persisted. Snap back to authoritative.
  // The same break also voids any pending create-focus: its `knownIds` snapshot
  // predates the disconnect, so diffing it against the post-reconnect ViewModel
  // could mis-identify an unrelated session as "ours". Drop it and let the user
  // pick up the new agent from the sidebar.
  const patch =
    conn === "closed" || conn === "failed" ? clearPendingClientIntent() : {}
  // Latch the sticky offline flag that drives the full-screen `OfflineOverlay`.
  // `open` is the only state that clears it; `closed`/`failed` set it; an
  // intermediate `connecting` (a reconnect attempt between drops, OR the very
  // first boot connect) leaves the prior value so the modal neither flickers off
  // mid-retry nor flashes on at boot before we have ever connected.
  const offline =
    conn === "open"
      ? false
      : conn === "closed" || conn === "failed"
        ? true
        : state.offline
  setState({ conn, offline, ...patch })
  // Clear the per-connection id on a drop. It belongs to the now-dead socket; a
  // REST action fired during the reconnect window must NOT stamp it as
  // `X-Connection-Id`, or the server would scope that action's status toasts to a
  // connection that no longer exists and the user would never see them. A null id
  // falls back to scope `All` (broadcast) — visible to this client once it
  // reconnects, the safe default. The next `connected` frame re-issues a fresh id.
  if (conn === "closed" || conn === "failed") setConnectionId(null)
  // Changed files no longer ride this socket: the `/ws/events` channel owns the
  // per-session subscription and re-establishes it on its own reconnect (see
  // `eventsSocket.onOpen`, which also refetches). There is nothing to re-arm here.
}

// Since Phase 6 there is no `command_result`/`error` frame: every action is a
// REST verb whose failure rejects its promise (the caller toasts it and rolls
// back optimistic state), and every keyed busy/success/clear arrives as a
// `status`/`status_cleared` event over `/ws/events` (see `eventsSocket.onEvent`).

// Reset both optimistic order overlays. Returned as a patch so callers can fold
// it into a single `setState`. Used on every error path so a rejected reorder
// snaps the UI back to the server's authoritative order.
function clearPendingOrders(): Partial<DuxState> {
  return {
    pendingSessionOrder: null,
    pendingProjectOrder: null,
    pendingAgentOrder: null,
  }
}

// Clear every transient, optimistic client intent at once: the reorder overlays,
// any pending create-focus, AND the Changes-pane visibility override. Used on the
// failure/teardown paths (command error, async error status, socket disconnect)
// where an in-flight create can no longer be trusted to resolve — a surviving
// `pendingCreateFocus` snapshot would otherwise mis-identify a later, unrelated
// session as the one we created, and a surviving Changes-pane override would
// strand the pane in the toggled state until reload. NOT folded into
// `clearPendingOrders` because user actions like sorting also clear the order
// overlays but must NOT cancel an in-flight create-focus.
function clearPendingClientIntent(): Partial<DuxState> {
  return {
    ...clearPendingOrders(),
    pendingCreateFocus: null,
    changesPaneOverride: null,
  }
}

// Stable sonner id for the anonymous (no-key) status slot. Sonner otherwise
// assigns a random id on each call, making anonymous clears a no-op and every
// anonymous update a new transient toast instead of an in-place update.
const ANON_TOAST_ID = "dux-anon-status"

// Route a keyed (or anonymous) engine status to both the status bar and a
// sonner toast. The key acts as the sonner id so updates re-render in place
// (busy → success swaps the spinner without a new toast) and clears can dismiss
// by id. Busy must not auto-dismiss before its final state arrives, so its
// duration is Infinity.
function showStatusToast(
  key: string | null | undefined,
  tone: string,
  message: string,
): void {
  if (!message) return
  const id = key ?? ANON_TOAST_ID // no key → stable anonymous-slot id
  // Info/success toasts auto-clear after the configured window
  // (`config.ui.status_clear_seconds`, default 6); 0 disables auto-clear so they
  // stay sticky like a warning/error. Busy/warning/error never auto-dismiss
  // (their final state replaces them). A missing bootstrap (pre-load) falls back
  // to the 6s default.
  const secs = state.bootstrap?.status_clear_seconds ?? 6
  const infoDuration = secs === 0 ? Infinity : secs * 1000
  const duration = tone === "info" ? infoDuration : Infinity
  const opts = { id, duration }
  if (tone === "error") toast.error(message, opts)
  else if (tone === "warning") toast.warning(message, opts)
  else if (tone === "busy") toast.loading(message, opts)
  else toast.success(message, opts) // info/success
}

// Boot: connect the events socket and fetch the initial workspace data. No
// /api/me round-trip is needed -- the server is a trusted-local tool with no
// login gate. Setting booted synchronously lets tests use it as a settled signal.
function boot(): void {
  setState({ booted: true })
  // This driver owns the initial load, so the first onOpen must not duplicate it
  // (every later reconnect still retries -- see the flag's docs).
  skipNextEventsOnOpenLoad = true
  eventsSocket.connect()
  loadBootstrap()
  loadSpine()
}
boot()

// Hardware/browser Back for the mobile shell. Registered ONCE at module scope
// (never in a React effect) so it survives re-renders and shell switches. The
// browser has already popped its own entry by the time this fires, so we only
// mirror that into our screen state. The target is derived from our own state
// machine — changes unwinds to the terminal when a target is still focused
// (else home), terminal unwinds to home — not from event.state contents, which
// keeps it resilient to history entries we didn't author. When mobileScreen is
// already "home" there is no spoke to unwind, so we no-op; this is also why
// desktop (which never advances past "home") is unaffected.
window.addEventListener("popstate", () => {
  const current = state.mobileScreen
  if (current === "home") return
  if (current === "changes") {
    setState({ mobileScreen: state.selectedTarget ? "terminal" : "home" })
  } else {
    setState({ mobileScreen: "home" })
  }
})

// INVARIANT: the number of history entries we've pushed equals the spoke depth
// implied by `mobileScreen` (home = 0, terminal = 1, changes = 2). `mobileNavigate`
// pushes on the way in; the popstate listener above pops on the way out. When the
// focused target is cleared OUT-OF-BAND (an agent exits, or a terminal is pruned
// from the ViewModel) the screen would otherwise fall back to home content while
// our pushed entries linger, leaving Back as a stale no-op (terminal) or a
// double-back (changes). This collapses the whole spoke back to home in one
// `history.go`, which fires a SINGLE popstate at the destination; the listener
// above then derives `mobileScreen: "home"` (selectedTarget is null by the time
// it runs because callers clear it first), restoring the invariant.
function unwindMobileSpoke(): void {
  if (state.mobileScreen === "terminal") {
    history.go(-1)
  } else if (state.mobileScreen === "changes") {
    history.go(-2)
  }
  // "home": no spoke entries to unwind. Desktop never advances past "home", so
  // it never reaches this branch with entries to pop — desktop is untouched.
}

export function useDux(): DuxState {
  return useSyncExternalStore(subscribe, getSnapshot)
}

// --- Deep-linking (a tiny hash router) ------------------------------------
//
// The selected target is mirrored into `location.hash` so a tab can be bookmarked
// /shared/reloaded back to the same agent (and, when one is focused, terminal):
//   #/agent/<sessionId>
//   #/agent/<sessionId>/terminal/<terminalId>
// Session ids are stable (a reload restores the agent); terminal ids are
// ephemeral (a reload that finds the session but not the terminal falls back to
// the agent; one that finds neither ignores the link). The hash is written with
// `history.replaceState` so it never adds a back-stack entry (that would fight
// the mobile spoke/back-button model, which uses `pushState`/`go`).

// Parse a deep-link hash into a target, or null when it is absent/malformed.
function parseSelectionHash(hash: string): SelectedTarget | null {
  // Three mutually-exclusive shapes: bare agent (`#/agent/<sid>` = session-slot tab), a
  // extra tab (`#/agent/<sid>/tab/<tabId>`), or a companion terminal
  // (`#/agent/<sid>/terminal/<tid>`). The literal `tab`/`terminal` keyword
  // disambiguates, so a tab/terminal literally named "tab" can't be confused.
  // A project terminal deep-links as `#/project/<pid>/terminal/<tid>`, its own
  // grammar, because the agent shapes embed a session id and a project terminal
  // has none.
  const pm = hash.match(/^#\/project\/([^/]+)\/terminal\/([^/]+)$/)
  if (pm) {
    try {
      const projectId = decodeURIComponent(pm[1])
      const terminalId = decodeURIComponent(pm[2])
      if (!projectId || !terminalId) return null
      return {
        kind: "terminal",
        terminalId,
        owner: { kind: "project", projectId },
      }
    } catch {
      return null
    }
  }
  const m = hash.match(/^#\/agent\/([^/]+)(?:\/(tab|terminal)\/([^/]+))?$/)
  if (!m) return null
  // `decodeURIComponent` throws a URIError on malformed percent-encoding (e.g.
  // `#/agent/%ZZ`). This runs at module init, so an unguarded throw would blank
  // the whole app. Treat any decode failure as no/invalid deep link.
  try {
    const sessionId = decodeURIComponent(m[1])
    if (!sessionId) return null
    if (m[2] === "terminal") {
      const terminalId = decodeURIComponent(m[3])
      if (!terminalId) return null
      return {
        kind: "terminal",
        terminalId,
        owner: { kind: "session", sessionId },
      }
    }
    if (m[2] === "tab") {
      const tabId = decodeURIComponent(m[3])
      if (!tabId) return null
      // A self-aliased `#/agent/<sid>/tab/<sid>` is the session-slot tab written the
      // long way — normalize to the canonical bare session-slot target so there is only
      // ever one representation of the session-slot tab (a real extra tab never has
      // `tabId === sessionId`).
      return { kind: "agent", sessionId, tabId }
    }
    return { kind: "agent", sessionId, tabId: sessionId }
  } catch {
    return null
  }
}

// The hash for a target (or the bare path when nothing is selected). The `/tab/`
// segment is emitted ONLY for an extra tab; the session-slot tab (`tabId === sessionId`)
// stays the bare `#/agent/<sid>` so existing bookmarks remain valid.
function selectionHash(target: SelectedTarget | null): string {
  if (!target) return ""
  if (target.kind === "terminal") {
    const owner = target.owner
    if (owner.kind === "project") {
      return `#/project/${encodeURIComponent(owner.projectId)}/terminal/${encodeURIComponent(target.terminalId)}`
    }
    return `#/agent/${encodeURIComponent(owner.sessionId)}/terminal/${encodeURIComponent(target.terminalId)}`
  }
  const base = `#/agent/${encodeURIComponent(target.sessionId)}`
  return target.tabId === target.sessionId
    ? base
    : `${base}/tab/${encodeURIComponent(target.tabId)}`
}

// Mirror the current selection into the URL hash without growing the back stack.
// Defensive: in non-browser test environments `history.replaceState` / a real
// `location` may be absent, so this no-ops there.
function writeSelectionHash(): void {
  if (typeof history === "undefined" || typeof history.replaceState !== "function") {
    return
  }
  const next = selectionHash(state.selectedTarget)
  const current = typeof location !== "undefined" ? location.hash ?? "" : ""
  if (current === next) return
  // An empty target hash collapses to the bare path so the URL doesn't keep a
  // dangling "#"; otherwise replace just the hash, preserving path + query.
  const base =
    typeof location !== "undefined"
      ? (location.pathname ?? "") + (location.search ?? "")
      : ""
  history.replaceState(history.state, "", next === "" ? base : next)
}

// The deep-link parsed from the URL at module load, restored once the first spine
// lands (a target can't be resolved until the session list exists). One-shot:
// consumed (and cleared) on the first `applySpine` so later spine refetches don't
// re-yank a user who has since navigated away.
let pendingDeepLink: SelectedTarget | null =
  typeof location !== "undefined"
    ? parseSelectionHash(location.hash ?? "")
    : null

// Route a normalized deep-link target onto an already-resolved session: restore
// a still-present terminal or extra tab, else fall back to the session-slot tab.
// Shared by the boot deep-link restore and the reconnect re-restore so both
// honor tabs/terminals identically.
function applyDeepLinkSelection(
  session: Spine["sessions"][number],
  target: SelectedTarget,
): void {
  if (target.kind === "terminal") {
    // Only session-owned terminals resolve through a session; project-terminal
    // links restore via `applyProjectTerminalDeepLink` and never reach here.
    if (target.owner.kind !== "session") return
    const owner = target.owner
    if (session.terminals.some((t) => t.id === target.terminalId)) {
      selectTerminal(target.terminalId, owner)
      return
    }
    // Terminal id gone — fall back to the owning agent.
    selectSession(owner.sessionId)
    return
  }
  if (
    target.tabId !== target.sessionId &&
    session.tabs.some((t) => t.id === target.tabId)
  ) {
    // An extra-tab deep link: restore it only if the tab still exists, else fall
    // through to the session-slot tab. `persist: false` because merely FOLLOWING
    // a shared link must not rewrite the workspace-shared remembered tab for
    // everyone that opens it.
    selectTab(target.sessionId, target.tabId, { persist: false })
    return
  }
  selectSession(target.sessionId)
}

// Restore the boot-time deep-link against the first spine. Resolve the session in
// the spine; restore the terminal when it still exists, else fall back to the
// session; ignore the link entirely when the session is gone.
function restoreDeepLink(spine: Spine): void {
  const link = pendingDeepLink
  if (!link) return
  pendingDeepLink = null // one-shot, whatever the outcome
  if (link.kind === "terminal") {
    const owner = link.owner
    if (owner.kind === "project") {
      // A project-terminal link: restore it when it still exists; a vanished
      // terminal falls back to nothing selected (there is no agent to land on).
      applyProjectTerminalDeepLink(spine, link.terminalId, owner.projectId)
    } else {
      const session = spine.sessions.find((s) => s.id === owner.sessionId)
      // session id gone — ignore the link
      if (session) applyDeepLinkSelection(session, link)
    }
  } else {
    const session = spine.sessions.find((s) => s.id === link.sessionId)
    // session id gone — ignore the link
    if (session) applyDeepLinkSelection(session, link)
  }
  // If the boot deep-link resolved to a selection, advance the mobile shell to
  // the terminal spoke. Otherwise `mobileScreen` stays "home" and the hub covers
  // the deep-linked agent — and, because the terminal pane only mounts on the
  // terminal screen, the PTY never even subscribes/launches. Desktop has no such
  // screen state and renders the center pane straight from `selectedTarget`, so
  // it "just works" there. This mirrors a tap's `mobileNavigate("terminal")` and
  // the popstate derive; `setState` (not `mobileNavigate`) avoids pushing a
  // spurious history entry at boot.
  if (state.selectedTarget) {
    setState({ mobileScreen: "terminal" })
  }
}

// Restore a project-terminal deep link against a spine: select the terminal
// when its project still carries it, otherwise leave nothing selected.
function applyProjectTerminalDeepLink(
  spine: Spine,
  terminalId: string,
  projectId: string,
): void {
  const project = spine.projects.find((p) => p.id === projectId)
  if (!project) return
  if (project.terminals.some((t) => t.id === terminalId)) {
    selectTerminal(terminalId, { kind: "project", projectId })
  }
}

// Deep-link intent re-armed on an events-socket RECONNECT — distinct from the
// boot `pendingDeepLink` one-shot above. When the connection drops while the
// user is deep-linked to a running agent, the reconnect can transiently clear
// the selection: the center pane mirrors the TUI's "agent exited" behavior and
// ejects to the welcome screen while the agent is momentarily `detached` (it
// exited during the outage and has not finished resuming yet), and that eject
// wipes the URL hash back to home. Nothing then restores the route, because
// `restoreDeepLink` is a spent boot one-shot. So on every reconnect we capture
// the route from `location.hash` — BEFORE any spine apply or eject can wipe it
// — and re-restore it once the agent is present AND back to `active` (its resume
// has completed). Restoring earlier, while still `detached`, would ping-pong
// with the center pane's eject.
let reconnectDeepLink: { target: SelectedTarget; armedAt: number } | null = null

// Bound how long the re-armed intent stays live, measured from the LATEST
// events-socket reopen (armReconnectDeepLink refreshes `armedAt` on every
// reopen, including one that finds the hash already wiped by our own eject,
// see below). This is a best-effort bound, generously above a normal provider
// resume: an agent that never returns to `active` within a window after the
// last reopen (resume failed, or it was genuinely stopped) intentionally gives
// up rather than keep chasing it indefinitely. A slow resume across a laptop
// sleep gets a fresh full window on the wake-triggered reopen, so this mostly
// only bites a resume that is actually stuck.
const RECONNECT_DEEPLINK_TTL_MS = 60_000

// Set by `ejectSelectionForReconnect` immediately around its own
// `selectSession(null)` call, and cleared (to `false`) by every OTHER entry
// into `selectSession`. This is how `restoreReconnectDeepLink` tells "the
// center pane's transient reconnect-eject just cleared the selection" apart
// from "the user deliberately navigated (including to home) on their own":
// only the former should be undone once the agent comes back.
let lastClearWasReconnectEject = false

// Capture the current route as the reconnect deep-link intent. Called from the
// events socket's reconnect `onOpen` BEFORE `loadSpine`, so the hash is read
// while it still names the agent (the transient eject happens later, during the
// spine apply).
function armReconnectDeepLink(): void {
  if (typeof location === "undefined") {
    reconnectDeepLink = null
    return
  }
  const target = parseSelectionHash(location.hash ?? "")
  if (target) {
    reconnectDeepLink = { target, armedAt: Date.now() }
    return
  }
  // The hash reads as home. This is either a genuine "nothing was deep-linked"
  // reconnect, OR a second (or later) reopen arriving after OUR OWN transient
  // eject already wiped the hash while the agent was still resuming from the
  // first drop. The boot one-shot is already spent, so if we discard the
  // still-armed intent here the user is stranded. Keep it alive and refresh its
  // `armedAt` so this fresh reopen grants the agent another full resume window.
  if (
    reconnectDeepLink &&
    Date.now() - reconnectDeepLink.armedAt <= RECONNECT_DEEPLINK_TTL_MS
  ) {
    reconnectDeepLink = { ...reconnectDeepLink, armedAt: Date.now() }
    return
  }
  reconnectDeepLink = null
}

// Re-restore a reconnect deep-link once the agent is present and active again.
// Runs on every spine apply while an intent is armed; it self-clears on success,
// on a genuine deletion, on a deliberate navigation away, or on TTL expiry, so
// it never re-yanks a user or chases a phantom session.
function restoreReconnectDeepLink(spine: Spine): void {
  const armed = reconnectDeepLink
  if (!armed) return
  const sel = state.selectedSessionId
  const armedTarget = armed.target
  if (armedTarget.kind === "terminal" && armedTarget.owner.kind === "project") {
    // A project terminal has no resume phase and its pane never issues the
    // reconnect eject (that path is gated on the agent session-slot tab), so
    // its selection normally survives a reconnect on its own, so this branch's
    // usual job is to disarm as a no-op. The one restorable gap is a
    // selection cleared by OUR OWN eject while the intent was armed; any
    // deliberate navigation (a non-null selection that is not the armed
    // terminal, or a home nav without the eject flag) disarms instead.
    const target = armedTarget
    const owner = armedTarget.owner
    const cur = state.selectedTarget
    if (
      cur?.kind === "terminal" &&
      cur.terminalId === target.terminalId
    ) {
      // Still on the armed terminal: the route survived, nothing to undo.
      reconnectDeepLink = null
      return
    }
    if (sel !== null || cur !== null) {
      // The user moved somewhere else on their own; respect it.
      reconnectDeepLink = null
      return
    }
    if (!lastClearWasReconnectEject) {
      // A deliberate home navigation, not our eject.
      reconnectDeepLink = null
      return
    }
    if (Date.now() - armed.armedAt > RECONNECT_DEEPLINK_TTL_MS) {
      reconnectDeepLink = null
      return
    }
    const exists =
      spine.projects
        .find((p) => p.id === owner.projectId)
        ?.terminals.some((t) => t.id === target.terminalId) ?? false
    if (!exists) return // keep waiting within the TTL (the spine may lag)
    selectTerminal(target.terminalId, owner)
    reconnectDeepLink = null
    return
  }
  const armedSessionId =
    armedTarget.kind === "agent"
      ? armedTarget.sessionId
      : armedTarget.owner.kind === "session"
        ? armedTarget.owner.sessionId
        : null
  if (armedSessionId === null) {
    reconnectDeepLink = null
    return
  }
  // The user actively moved to a DIFFERENT agent since we armed — respect it and
  // drop the intent so we never yank them back.
  if (sel !== null && sel !== armedSessionId) {
    reconnectDeepLink = null
    return
  }
  // A cleared selection (`null`) is ambiguous on its own: it is either the
  // transient reconnect-eject we are here to undo, or the user deliberately
  // navigating home (a "back to home" control, say) while the agent was still
  // resuming. `lastClearWasReconnectEject` disambiguates: only our own eject
  // leaves it `true`. A deliberate clear must disarm, not merely wait.
  if (sel === null && !lastClearWasReconnectEject) {
    reconnectDeepLink = null
    return
  }
  if (Date.now() - armed.armedAt > RECONNECT_DEEPLINK_TTL_MS) {
    reconnectDeepLink = null
    return
  }
  const session = spine.sessions.find((s) => s.id === armedSessionId)
  if (!session) {
    // Genuinely gone (deleted here or by another client): a deletion legitimately
    // ejects to home, so drop the intent rather than resurrect a phantom.
    reconnectDeepLink = null
    return
  }
  // Wait until the agent has finished resuming (back to `active`). Until then we
  // stay armed: a still-`detached` agent is about to be ejected by the center
  // pane, and restoring now would just ping-pong with that eject.
  if (session.status !== "active") return
  // The agent is present and running again. If the eject already cleared the
  // selection, re-restore the captured route; if it never cleared, this is a
  // no-op. Either way, disarm.
  if (sel !== armedSessionId) {
    applyDeepLinkSelection(session, armed.target)
  }
  reconnectDeepLink = null
}

// Select an agent session as the streamed target. Signature kept stable so
// existing callers continue to work unchanged.
//
// Restores the agent's remembered tab-focus (`resolveFocusedTab`, backed by
// `SessionView.last_focused_tab`): when the spine has the session and its
// remembered tab is still a live extra tab, this routes through `selectTab`
// (so the hash/changes wiring is identical to an explicit tab click) instead
// of always landing on the session-slot tab. This is a READ of the memory,
// not a write — no persistence call happens here; `selectTab` below owns
// persisting an actual tab switch.
export function selectSession(id: string | null): void {
  // Any deliberate selection (to an agent OR to null/home) means the user took
  // control. See `ejectSelectionForReconnect` below for the one carve-out.
  lastClearWasReconnectEject = false
  const prev = state.selectedSessionId
  if (id === null) {
    // Clear the target FIRST so any synchronous re-render shows the fallback,
    // THEN collapse the mobile spoke so the back stack matches the screen. This
    // is the out-of-band clear path (e.g. an agent exit) — see
    // `unwindMobileSpoke`. Desktop stays on "home", so the unwind no-ops there.
    setState({
      selectedTarget: null,
      selectedSessionId: null,
      changes: emptyChanges(),
    })
    // Drop the previous session's changed-files subscription; there is no global
    // watch to clear, so the cross-client clobber is gone by construction.
    switchChangesSubscription(prev, null)
    writeSelectionHash()
    unwindMobileSpoke()
    return
  }
  const session = state.spine?.sessions.find((s) => s.id === id)
  const focusedTab = session ? resolveFocusedTab(session) : id
  if (focusedTab !== id) {
    // A remembered extra tab is still live: select it directly so the
    // hash/changes wiring and the persistence write match an explicit tab
    // click exactly.
    selectTab(id, focusedTab)
    return
  }
  setState({
    // Selecting a session focuses its session-slot tab (tabId === sessionId).
    selectedTarget: { kind: "agent", sessionId: id, tabId: id },
    selectedSessionId: id,
    // Re-selecting the same session keeps its loaded data; a real switch enters
    // the loading window so the pane shows a spinner, not the previous session's
    // files.
    changes: prev === id ? state.changes : loadingChanges(id),
  })
  // Move the per-session changed-files subscription, THEN fetch — subscribing
  // before the GET means an invalidation that races the fetch is never missed.
  switchChangesSubscription(prev, id)
  writeSelectionHash()
  if (prev !== id) loadChanges(id)
}

// The ONE carve-out to `selectSession`'s "any clear disarms the reconnect
// intent" rule: called exclusively by the center pane's transient
// reconnect-eject (mirroring the TUI's "agent exited" reset, see
// `TerminalPane`), never by a user-initiated navigation. Marks this specific
// `selectSession(null)` as OUR eject so `restoreReconnectDeepLink` can tell it
// apart from a deliberate home navigation the user made on their own while the
// agent was still resuming, only the former should be undone once the agent
// comes back to `active`.
export function ejectSelectionForReconnect(): void {
  selectSession(null)
  lastClearWasReconnectEject = true
}

// Focus a specific provider tab of a session. `tabId === sessionId` focuses the
// session-slot tab (equivalent to `selectSession`). The changed files belong to the
// SESSION, so the subscription/fetch key off `sessionId` regardless of tab.
//
// Persists the choice as the agent's remembered tab-focus (J3: fire-and-forget,
// no status/toast) so a later `selectSession` restores it, on this client or
// any other sharing the same server. `tabsApi.setFocusedTab` itself normalizes
// `tabId === sessionId` to "clear the memory" server-side. Pass `persist:
// false` for a selection that must not rewrite the workspace-shared memory
// (e.g. `restoreDeepLink`, which only follows a link, it doesn't set intent).
export function selectTab(
  sessionId: string,
  tabId: string,
  opts?: { persist?: boolean },
): void {
  const prev = state.selectedSessionId
  setState({
    selectedTarget: { kind: "agent", sessionId, tabId },
    selectedSessionId: sessionId,
    changes: prev === sessionId ? state.changes : loadingChanges(sessionId),
  })
  switchChangesSubscription(prev, sessionId)
  writeSelectionHash()
  if (prev !== sessionId) loadChanges(sessionId)
  if (opts?.persist === false) return
  persistFocusedTab(sessionId, tabId === sessionId ? null : tabId)
}

// Per-session bookkeeping for the fire-and-forget focus-tab PUT. `selectTab`
// can fire in rapid succession (fast tab switching) and the resulting network
// responses can settle out of order, so we keep only the LATEST intended
// `(generation, tabId)` per session. When a response settles for a stale
// generation whose value differs from the current intent
// (`shouldRefireFocusPut`), we re-issue a PUT for the latest intent so the
// server's last write always matches the user's last click, regardless of
// response ordering.
const focusPutIntent = new Map<
  string,
  { generation: number; tabId: string | null }
>()

function persistFocusedTab(sessionId: string, tabId: string | null): void {
  const generation = (focusPutIntent.get(sessionId)?.generation ?? 0) + 1
  focusPutIntent.set(sessionId, { generation, tabId })
  fireFocusedTabPut(sessionId, tabId, generation)
}

function fireFocusedTabPut(
  sessionId: string,
  tabId: string | null,
  generation: number,
): void {
  void tabsApi.setFocusedTab(sessionId, tabId).then(() => {
    const latest = focusPutIntent.get(sessionId)
    if (!latest) return
    if (shouldRefireFocusPut(latest, { generation, tabId })) {
      fireFocusedTabPut(sessionId, latest.tabId, latest.generation)
    }
  })
}

// Select a companion terminal as the streamed target. A session-owned terminal
// retains its owning session id so session-scoped UI keeps resolving; a project
// terminal has NO session context (`selectedSessionId` stays null and the
// changes pane shows its empty state, since changed files belong to a session's
// worktree, and the project's source checkout has no diff pipeline).
export function selectTerminal(terminalId: string, owner: TerminalOwnerRef): void {
  const prev = state.selectedSessionId
  const sessionId = owner.kind === "session" ? owner.sessionId : null
  setState({
    selectedTarget: { kind: "terminal", terminalId, owner },
    selectedSessionId: sessionId,
    // Switching from the agent to one of its own terminals keeps the same
    // session's loaded changes; only a different session (or a project
    // terminal, which has none) enters loading/empty.
    changes:
      sessionId === null
        ? emptyChanges()
        : prev === sessionId
          ? state.changes
          : loadingChanges(sessionId),
  })
  // The changed files belong to the SESSION, so subscribe/fetch the parent
  // session even when a companion terminal is the streamed target; a project
  // terminal drops the subscription entirely.
  switchChangesSubscription(prev, sessionId)
  writeSelectionHash()
  if (sessionId !== null && prev !== sessionId) loadChanges(sessionId)
}

// Spawn a new companion terminal for a session via REST (Phase 5). The 201 reply
// carries the new terminal id, so we focus it immediately — opening its PTY
// socket (`TerminalPane`) — rather than waiting for a `terminal_created` frame.
// The terminal also lands in the spine via the `sessions.changed` refetch, which
// fills in its label/status; focusing first is safe because the PTY socket only
// needs the ids the create returned. A failure surfaces as a toast.
export function createTerminal(sessionId: string): void {
  terminalsApi
    .create(sessionId)
    .then((created) =>
      selectTerminal(created.terminal_id, { kind: "session", sessionId }),
    )
    .catch((e) =>
      toast.error(
        e instanceof Error ? e.message : "Could not create the terminal.",
      ),
    )
}

// Spawn a new project terminal (a plain shell at the project's repo root with
// no agent attached) via REST, then focus it, mirroring `createTerminal`.
export function createProjectTerminal(projectId: string): void {
  terminalsApi
    .createForProject(projectId)
    .then((created) =>
      selectTerminal(created.terminal_id, { kind: "project", projectId }),
    )
    .catch((e) =>
      toast.error(
        e instanceof Error ? e.message : "Could not create the project terminal.",
      ),
    )
}

// Open the close-terminal confirmation dialog for a companion terminal. The TUI
// always confirms before killing a terminal's running process, so the web does
// too (the ✕ no longer deletes on a single click).
export function openDeleteTerminal(terminalId: string): void {
  setState({ deleteTerminalTarget: terminalId })
}

export function closeDeleteTerminal(): void {
  setState({ deleteTerminalTarget: null })
}

// Resolve a terminal's owner from the spine: a session whose terminal list
// carries it, else a project whose terminal list carries it, else undefined
// (already vanished).
export function findTerminalOwner(
  terminalId: string,
): TerminalOwnerRef | undefined {
  const sessionId = state.spine?.sessions.find((s) =>
    s.terminals.some((t) => t.id === terminalId),
  )?.id
  if (sessionId !== undefined) return { kind: "session", sessionId }
  const projectId = state.spine?.projects.find((p) =>
    p.terminals.some((t) => t.id === terminalId),
  )?.id
  if (projectId !== undefined) return { kind: "project", projectId }
  return undefined
}

// Close (delete) a companion terminal via REST (Phase 5). The endpoint is nested
// under the owner, so resolve it from the spine across BOTH owner kinds,
// sessions and projects (a session-only scan silently made project terminals
// undeletable); a terminal that already vanished (no owner) is a no-op. The
// terminal is removed from the workspace spine, and if it was the focused target
// the selection clears via the spine prune in `applySpine` (driven by the
// `sessions.changed` refetch). A failure surfaces as a toast.
export function deleteTerminal(terminalId: string): void {
  const owner = findTerminalOwner(terminalId)
  if (owner === undefined) return
  const request =
    owner.kind === "session"
      ? terminalsApi.remove(owner.sessionId, terminalId)
      : terminalsApi.removeForProject(owner.projectId, terminalId)
  request.catch((e) =>
    toast.error(
      e instanceof Error ? e.message : "Could not close the terminal.",
    ),
  )
}

// --- Agent tabs -------------------------------------------------------------

// Add an extra tab to a session, then focus it. The 201 reply carries the new
// tab id; focus it immediately (opening its PTY socket, which launches it fresh)
// rather than waiting for the spine refetch, mirroring `createTerminal`. The "+"
// is disabled while a create is in flight so a double-click can't spawn two tabs.
export function addTab(sessionId: string, provider?: string): void {
  if (state.createTabInFlight.includes(sessionId)) return
  setState({ createTabInFlight: [...state.createTabInFlight, sessionId] })
  const clearInFlight = () =>
    setState({
      createTabInFlight: state.createTabInFlight.filter((s) => s !== sessionId),
    })
  tabsApi
    .create(sessionId, provider)
    .then((created) => {
      clearInFlight()
      // A newly-added tab launches immediately (never dormant), so just focus it;
      // the pane subscribes on mount.
      selectTab(sessionId, created.tab_id)
    })
    .catch((e) => {
      clearInFlight()
      toast.error(e instanceof Error ? e.message : "Could not create the tab.")
    })
}

// Open the close-tab confirmation. Closing ALWAYS confirms; all tabs are generic.
// Closing a tab ends it, and closing the agent's last tab detaches the agent.
export function openCloseTab(sessionId: string, tabId: string): void {
  setState({ closeTabTarget: { sessionId, tabId } })
}

export function closeCloseTab(): void {
  setState({ closeTabTarget: null })
}

// Close a tab via REST. Closing the session-slot tab (`tabId === sessionId`) stops
// that tab and detaches the agent only if it was the last live tab (server 200,
// session survives); an extra tab is destroyed (also 200, same `{ detached }`
// shape). All focus/latch mutations wait for the DELETE to actually resolve —
// closing is NOT optimistic: mutating them beforehand (as this used to) left the
// UI navigated away from a tab that was still alive server-side whenever the
// request failed, with only a toast and no rollback. On success, if the closed
// tab was the focused target, move focus off it so the pane never sits on the
// just-closed tab and re-subscribes it (subscribing force-relaunches the
// provider): an extra tab falls back to the session-slot tab; the session-slot
// tab falls back to a live sibling using the server's authoritative `detached`
// flag (never a pre-close snapshot, which can go stale in a race with another
// client closing tabs concurrently) with none when the agent fully detached. A
// failure toasts and leaves all state untouched.
export function closeTab(sessionId: string, tabId: string): void {
  tabsApi
    .remove(sessionId, tabId)
    .then((result) => {
      setState({
        startedDormantTabs: state.startedDormantTabs.filter((t) => t !== tabId),
      })
      const target = state.selectedTarget
      const focused =
        target?.kind === "agent" &&
        target.sessionId === sessionId &&
        target.tabId === tabId
      if (!focused) return
      if (tabId !== sessionId) {
        // Focus the session-slot tab DIRECTLY via `selectTab`, not
        // `selectSession`: the spine may still be stale at this point (no
        // `sessions.changed` refetch has pruned it yet), and `selectSession`
        // would resolve the remembered tab against that stale spine, which
        // can still name the tab we just deleted, so use `selectTab` to
        // pick the session-slot tab without consulting that memory.
        selectTab(sessionId, sessionId)
        return
      }
      // Closing the focused session-slot tab: an older server that still replies
      // with a bodiless 204 gives no `detached` signal, so treat that as detached
      // (the safer default: leave selection put rather than guessing a sibling).
      const detached = result?.detached ?? true
      if (detached) return
      const liveSibling = state.spine?.sessions
        .find((s) => s.id === sessionId)
        ?.tabs.find((t) => t.id !== tabId && t.has_live_process)
      if (liveSibling) selectTab(sessionId, liveSibling.id)
    })
    .catch((e) =>
      toast.error(e instanceof Error ? e.message : "Could not close the tab."),
    )
}

// Retarget a tab's provider (effective on its next launch). Validated up front
// against the configured list, mirroring `changeAgentProvider`. Resolves `true`
// on success, `false` (after toasting) so a dialog can stay open.
export async function retargetTab(
  sessionId: string,
  tabId: string,
  provider: string,
): Promise<boolean> {
  if (!providerIsConfigured(provider)) {
    toast.error(`Provider "${provider}" is not configured.`)
    return false
  }
  try {
    await tabsApi.patch(sessionId, tabId, provider)
    return true
  } catch (e) {
    toast.error(e instanceof Error ? e.message : "Could not change the provider.")
    return false
  }
}

// Explicitly start a dormant extra tab from its dormant card: mark it started
// (so the pane mounts and subscribes = launches fresh) and focus it. This is the
// ONLY path that launches a dormant tab — focusing one never does.
export function startDormantTab(sessionId: string, tabId: string): void {
  if (!state.startedDormantTabs.includes(tabId)) {
    setState({ startedDormantTabs: [...state.startedDormantTabs, tabId] })
  }
  selectTab(sessionId, tabId)
}

// An extra tab's PTY socket discovered (via `isTabGone` against the current
// spine) that this tab no longer exists — another client closed it while this
// one was retrying the socket. The route will keep 404ing, so there is nothing
// left to reconnect to: clear the started-dormant latch (it would otherwise
// linger forever, since the tab is gone and `applySpine` only clears the latch
// once a tab goes LIVE, which this one now never will) and toast so the user
// knows why the pane stopped retrying instead of it just going quiet.
export function handleTabGone(tabId: string): void {
  setState({
    startedDormantTabs: state.startedDormantTabs.filter((t) => t !== tabId),
  })
  toast.error("This tab was closed elsewhere.")
}

// Open the discard-confirmation dialog for an unstaged file. The TUI confirms
// every discard because it's destructive — an untracked file is deleted, a
// tracked one loses its working-tree changes. The web mirrors that.
export function openDiscard(target: DiscardTarget): void {
  setState({ discardTarget: target })
}

export function closeDiscard(): void {
  setState({ discardTarget: null })
}

// Ask the server to discard a file's working-tree changes. The server re-derives
// the tracked/untracked distinction from live git status and rejects the command
// if the file is staged, so this never trusts the client about the destructive
// outcome.
export function discardFile(sessionId: string, path: string): void {
  git
    .discard(sessionId, path)
    .catch((e) => toast.error(e instanceof Error ? e.message : "discard failed"))
}

export function openCommit(sessionId: string): void {
  setState({ commitTarget: sessionId, commitDraft: "" })
}

export function closeCommit(): void {
  setState({ commitTarget: null, commitDraft: "" })
}

export function setCommitDraft(text: string): void {
  setState({ commitDraft: text })
}

// Open the code-editor overlay for a session. Selecting the session first points
// the engine's changed-files watch at its worktree so the editor's file list
// populates from the same broadcast the changes pane uses. `initialPath` (from a
// per-file affordance) auto-loads that file, seeded as a tab via
// `editorOpenFile`, so external opens (ChangedFiles Edit/Diff, Sidebar) funnel
// through the same VS Code preview model as the tree/search; `mode` chooses the
// opening view: "diff" when a changed file is clicked (show its diff first),
// "file" otherwise. Does NOT clear the session's existing tab list, so reopening
// the overlay on a session restores its tabs (`editorTabs` persists across
// `closeEditor`; only `editorClearSession`, on session delete, clears it).
export function openEditor(
  sessionId: string,
  initialPath: string | null = null,
  mode: EditorViewMode = "file",
): void {
  if (state.selectedSessionId !== sessionId) selectSession(sessionId)
  setState({ editorTarget: { sessionId, initialPath, initialMode: mode } })
  if (initialPath !== null) editorOpenFile(sessionId, initialPath, { mode })
}

export function closeEditor(): void {
  setState({ editorTarget: null })
}

// --- Editor tabs: thin store wrappers over the pure reducer (lib/editorTabs.ts).
// Each mutates only `editorTabs[sessionId]`, leaving every other session's tabs
// untouched. Components/dialogs call ONLY these, never the pure functions
// directly, so the store stays the single place that knows how to read/write
// the per-session slice.

function editorTabsFor(sessionId: string): EditorTabsState {
  return state.editorTabs[sessionId] ?? emptyTabsState()
}

// Skips `setState` entirely when `next` is REFERENCE-EQUAL to the session's
// current tabs state. A reducer that determined nothing actually changed
// (e.g. `setTabDirty` called with the flag it already has) returns the same
// object back. `useDux()` is an unselective `useSyncExternalStore`, so every
// consumer app-wide re-renders on every `setState`; without this guard a
// no-op dispatch (like the one `editorSetTabDirty` would otherwise fire on
// every keystroke) would still fan out a global re-render for nothing.
function setEditorTabsFor(sessionId: string, next: EditorTabsState): void {
  if (state.editorTabs[sessionId] === next) return
  setState({ editorTabs: { ...state.editorTabs, [sessionId]: next } })
}

// Open (or activate, or preview-replace) a file in a session's tab list. See
// `lib/editorTabs.ts` `openFile` for the exact promotion rules. `opts.mode` is
// an EXPLICIT mode intent (changed-files Edit/Diff, a brand-new file, an
// external deep-link via `openEditor`); it always drives a new/replaced tab's
// mode and, given, also retargets an already-open tab's mode. Omit it for a
// plain activation (tree/search click) so re-clicking an already-open path
// never silently flips its existing diff view back to file view.
export function editorOpenFile(
  sessionId: string,
  path: string,
  opts: { mode?: EditorViewMode; pin?: boolean } = {},
): void {
  setEditorTabsFor(
    sessionId,
    editorOpenFilePure(editorTabsFor(sessionId), path, {
      mode: opts.mode,
      pin: opts.pin,
      newId: () => newClientId(),
    }),
  )
}

export function editorActivateTab(sessionId: string, tabId: string): void {
  setEditorTabsFor(sessionId, editorActivateTabPure(editorTabsFor(sessionId), tabId))
}

// Promote a tab to permanent: double-click on the row/pill, or the tab's first
// edit (a dirty preview tab is promoted so an edit is never silently discarded
// by a later preview-replace).
export function editorPinTab(sessionId: string, tabId: string): void {
  setEditorTabsFor(sessionId, editorPinTabPure(editorTabsFor(sessionId), tabId))
}

// Mirrors the buffer's dirty state up to the store so the strip's dot and the
// close-confirm gating read from one place, without putting file contents in
// the global store (see lib/editorTabs.ts header comment).
export function editorSetTabDirty(
  sessionId: string,
  tabId: string,
  dirty: boolean,
): void {
  setEditorTabsFor(
    sessionId,
    editorSetTabDirtyPure(editorTabsFor(sessionId), tabId, dirty),
  )
}

export function editorSetTabMode(
  sessionId: string,
  tabId: string,
  mode: EditorViewMode,
): void {
  setEditorTabsFor(
    sessionId,
    editorSetTabModePure(editorTabsFor(sessionId), tabId, mode),
  )
}

// Unconditional close (post-confirm, or the tab was clean). Picks the next
// active tab via the VS Code right-then-left rule.
export function editorCloseTab(sessionId: string, tabId: string): void {
  setEditorTabsFor(sessionId, editorCloseTabPure(editorTabsFor(sessionId), tabId))
}

// Rename retarget: rewrite the path of the tab(s) affected by renaming a file
// or folder from `from` to `to`. See `lib/editorTabs.ts` `renameTabPaths` for
// the folder-prefix rewrite and the pre-existing-destination-tab collision
// close.
export function editorRenameTabPaths(
  sessionId: string,
  from: string,
  to: string,
): void {
  setEditorTabsFor(
    sessionId,
    editorRenameTabPathsPure(editorTabsFor(sessionId), from, to),
  )
}

// Close every tab under a deleted file or folder path. See
// `lib/editorTabs.ts` `closeTabsUnderPath`.
export function editorCloseTabsUnderPath(sessionId: string, path: string): void {
  setEditorTabsFor(
    sessionId,
    editorCloseTabsUnderPathPure(editorTabsFor(sessionId), path),
  )
}

// Drop all of a session's editor tabs (session deleted from the spine). See
// the `editorTabs` prune in `applySpine`, which calls this for any session
// key no longer present in the live spine.
export function editorClearSession(sessionId: string): void {
  if (!(sessionId in state.editorTabs)) return
  const next = { ...state.editorTabs }
  delete next[sessionId]
  setState({ editorTabs: next })
}

// Open the dirty-tab close confirmation.
export function openEditorCloseTab(sessionId: string, tabId: string): void {
  setState({ editorCloseTabTarget: { sessionId, tabId } })
}

export function closeEditorCloseTab(): void {
  setState({ editorCloseTabTarget: null })
}

export function openDelete(sessionId: string): void {
  setState({ deleteTarget: sessionId })
}

export function closeDelete(): void {
  setState({ deleteTarget: null })
}

// Ask the server to delete an agent session. `deleteWorktree` opts into the
// destructive removal of the git worktree on disk (default off in the UI).
export function deleteSession(sessionId: string, deleteWorktree: boolean): void {
  sessionsApi
    .remove(sessionId, deleteWorktree)
    .catch((e) => {
      // A 409 is a refusal (a tab is still launching, or a delete is already in
      // flight). The server already surfaces that message over the /ws status
      // stream, so don't toast it a second time. Mirrors `toastCreateError`.
      if (e instanceof SessionsApiError && e.status === 409) return
      toast.error(
        e instanceof Error ? e.message : "Could not delete the session.",
      )
    })
}

// Open the rename dialog for a session, pre-filling the current custom title
// (empty when none, so the placeholder shows the branch name).
export function openRename(sessionId: string): void {
  const session = state.spine?.sessions.find((s) => s.id === sessionId)
  setState({ renameTarget: sessionId, renameDraft: session?.title ?? "" })
}

export function closeRename(): void {
  setState({ renameTarget: null, renameDraft: "" })
}

export function setRenameDraft(raw: string): void {
  // Sanitize like the new-agent input: a custom title is validated as an agent
  // name server-side, so keep the dialog from accepting characters the server
  // would reject. Empty stays empty (clears the title back to the branch name).
  setState({ renameDraft: sanitizeAgentName(raw) })
}

// Ask the server to set a session's display title. An empty title clears it
// back to the branch name; a non-empty title is validated server-side. Resolves
// `true` on success, `false` (after toasting) on failure, so the rename dialog can
// stay open and preserve the user's input when the PATCH is rejected.
export async function renameSession(
  sessionId: string,
  title: string,
): Promise<boolean> {
  try {
    await sessionsApi.patch(sessionId, { title })
    return true
  } catch (e) {
    toast.error(e instanceof Error ? e.message : "Could not rename the session.")
    return false
  }
}

// Submit the rename dialog, closing it only once the PATCH succeeds. On failure
// the dialog stays open (the error is toasted) so the user does not lose the name
// they typed and can retry or cancel.
export async function submitRename(): Promise<void> {
  const id = state.renameTarget
  if (!id) return
  if (await renameSession(id, state.renameDraft.trim())) closeRename()
}

// Open the change-provider dialog for a session. The dialog pre-selects the
// session's current provider from the ViewModel.
export function openChangeProvider(sessionId: string): void {
  setState({ changeProviderTarget: sessionId })
}

export function closeChangeProvider(): void {
  setState({ changeProviderTarget: null })
}

// Whether `provider` is in the bootstrap document's configured provider list. The
// server re-validates authoritatively, but checking here first avoids firing a
// PATCH that the server will reject — important for the multi-field project PATCH,
// where a bad provider rejected mid-sequence would leave earlier fields (rename,
// auto-reopen) already committed (the PATCH is not atomic across independent
// fields). Empty list (pre-bootstrap) treats every provider as unconfigured.
function providerIsConfigured(provider: string): boolean {
  return (state.bootstrap?.available_providers ?? []).includes(provider)
}

// Ask the server to swap which provider a session uses. The provider is validated
// against the configured list up front (the server re-validates), persisted for
// the next launch, with the outcome (swapped / already-uses-it / still-running)
// reported on the status stream. Resolves `true` on success, `false` (after
// toasting) on a rejected/invalid provider so the dialog can stay open.
export async function changeAgentProvider(
  sessionId: string,
  provider: string,
): Promise<boolean> {
  if (!providerIsConfigured(provider)) {
    toast.error(`Provider "${provider}" is not configured.`)
    return false
  }
  try {
    await sessionsApi.patch(sessionId, { provider })
    return true
  } catch (e) {
    toast.error(e instanceof Error ? e.message : "Could not change the provider.")
    return false
  }
}

// Toggle a session's auto-reopen preference (PATCH `auto_reopen`). Shared by the
// desktop sidebar and the mobile session menu so the two surfaces never drift.
export function toggleSessionAutoReopen(
  sessionId: string,
  enabled: boolean,
): void {
  sessionsApi
    .patch(sessionId, { auto_reopen: enabled })
    .catch((e) =>
      toast.error(
        e instanceof Error ? e.message : "Could not update auto-reopen.",
      ),
    )
}

// Ask the server to reconnect (relaunch) an agent. `force` starts a fresh
// session with no resume args (the TUI's force-reconnect); the default resumes
// the prior conversation when the provider supports it. The web UI deliberately
// exposes only the forced variant (the confirmed "Force recreate agent…" menu
// item), so no web surface currently calls `force: false`; the parameter stays
// because the wire contract supports resume and the TUI still exposes plain
// reconnect as a separate action. Focus the session and
// bump `terminalEpoch` so the pane remounts and re-subscribes — the reconnect
// swaps in a new server-side provider, and the previously-attached forwarder is
// dead, so even an already-focused pane must re-issue `subscribe`. The server
// defers that subscribe until the freshly launched provider comes up.
export function reconnectSession(sessionId: string, force: boolean): void {
  sessionsApi
    .reconnect(sessionId, force)
    .catch((e) =>
      toast.error(
        e instanceof Error ? e.message : "Could not reconnect the session.",
      ),
    )
  setState({
    // Reconnect is a session-slot-tab operation, so focus the session-slot tab.
    selectedTarget: { kind: "agent", sessionId, tabId: sessionId },
    selectedSessionId: sessionId,
    terminalEpoch: state.terminalEpoch + 1,
  })
}

export function openGlobalEnv(): void {
  setState({ globalEnvOpen: true })
}

export function closeGlobalEnv(): void {
  setState({ globalEnvOpen: false })
}

export function saveGlobalEnv(env: Record<string, string>): void {
  configApi
    .persistGlobalEnv(env)
    .catch((e) =>
      toast.error(
        e instanceof Error ? e.message : "Could not save the global environment.",
      ),
    )
}

export function openProjectSettings(projectId: string): void {
  setState({ projectSettingsTarget: projectId })
}

export function closeProjectSettings(): void {
  setState({ projectSettingsTarget: null })
}

// Open the agent-scoped startup-command editor. The target is the SESSION id; the
// dialog resolves and edits that agent's PROJECT startup command (startup command
// is project-scoped — there is no per-agent startup command).
export function openAgentStartupCommand(sessionId: string): void {
  setState({ agentStartupCommandTarget: sessionId })
}

export function closeAgentStartupCommand(): void {
  setState({ agentStartupCommandTarget: null })
}

// Open the agent-scoped environment editor. The target is the SESSION id; the
// dialog resolves and edits that agent's PROJECT env (env is project-scoped — it
// applies to every agent and terminal in the project).
export function openAgentEnv(sessionId: string): void {
  setState({ agentEnvTarget: sessionId })
}

export function closeAgentEnv(): void {
  setState({ agentEnvTarget: null })
}

// Open the startup-command log viewer for an agent and fetch its log files (with
// the newest file's contents pre-loaded). A reply is ignored once the viewer has
// closed or retargeted, so a late frame can't repopulate a stale viewer (the
// browse/attach-worktree precedent).
export function openStartupLogs(sessionId: string): void {
  setState({
    startupLogsTarget: sessionId,
    startupLogsEntries: [],
    startupLogsSelected: null,
    startupLogsError: null,
    startupLogsLoading: true,
  })
  sessionsApi
    .startupLogs(sessionId)
    .then((res) => {
      if (state.startupLogsTarget !== sessionId) return
      setState({
        startupLogsEntries: res.entries,
        startupLogsSelected: res.selected,
        startupLogsError: null,
        startupLogsLoading: false,
      })
    })
    .catch((e) => {
      if (state.startupLogsTarget !== sessionId) return
      setState({
        startupLogsLoading: false,
        startupLogsError:
          e instanceof Error
            ? e.message
            : "Could not load the startup command logs.",
      })
    })
}

// Switch the viewer to a different log file (fetches that file's contents).
export function selectStartupLog(name: string): void {
  const sessionId = state.startupLogsTarget
  if (!sessionId) return
  setState({ startupLogsLoading: true, startupLogsError: null })
  sessionsApi
    .startupLogContent(sessionId, name)
    .then((res) => {
      if (state.startupLogsTarget !== sessionId) return
      setState({ startupLogsSelected: res, startupLogsLoading: false })
    })
    .catch((e) => {
      if (state.startupLogsTarget !== sessionId) return
      setState({
        startupLogsLoading: false,
        startupLogsError:
          e instanceof Error
            ? e.message
            : "Could not read the startup command log.",
      })
    })
}

export function closeStartupLogs(): void {
  setState({
    startupLogsTarget: null,
    startupLogsEntries: [],
    startupLogsSelected: null,
    startupLogsLoading: false,
    startupLogsError: null,
  })
}

// Re-run the agent's project startup command in its worktree (the TUI's
// `rerun-startup-command-on-agent`). The server runs it off-thread and reports
// busy/success/failure on the status stream — nothing to do here but fire the
// command and surface a transport/validation error if the request is rejected.
export function rerunStartupCommand(sessionId: string): void {
  sessionsApi
    .rerunStartupCommand(sessionId)
    .catch((e) =>
      toast.error(
        e instanceof Error
          ? e.message
          : "Could not rerun the startup command.",
      ),
    )
}

export function openProjectInfo(projectId: string): void {
  setState({ projectInfoTarget: projectId })
}

export function closeProjectInfo(): void {
  setState({ projectInfoTarget: null })
}

export function openAgentInfo(sessionId: string): void {
  setState({ agentInfoTarget: sessionId })
}

export function closeAgentInfo(): void {
  setState({ agentInfoTarget: null })
}

// The force-recreate confirmation ("Force recreate agent…" in the agent ⋯
// menus). Open/close only move the target; the dialog itself calls
// `reconnectSession(id, true)` on confirm.
export function openForceReconnect(sessionId: string): void {
  setState({ forceReconnectTarget: sessionId })
}

export function closeForceReconnect(): void {
  setState({ forceReconnectTarget: null })
}

// Browse a directory for the add-project picker over REST (replaces the retired
// `/ws` `browse_dir` → `dir_entries` round-trip). A null path resolves the
// server's configured default start directory. The reply is ignored once the
// dialog has closed so a late response can't repopulate a closed picker.
function runBrowse(path: string | null): void {
  browseApi
    .browse(path)
    .then((res) => {
      if (!state.addProjectOpen) return
      setState({
        browsePath: res.path,
        browseEntries: res.entries,
        browseLoading: false,
      })
    })
    .catch((e) => {
      if (!state.addProjectOpen) return
      setState({ browseEntries: [], browseLoading: false })
      toast.error(
        e instanceof Error ? e.message : "Could not browse the directory.",
      )
    })
}

export function openAddProject(): void {
  setState({
    addProjectOpen: true,
    addProjectIntent: "add",
    browseLoading: true,
    browseEntries: [],
  })
  // A null path tells the server to open at the configured default
  // (`defaults.start_directory`, resolved from the live config), not $HOME.
  runBrowse(null)
}

// Open the same picker with the "init" intent (the split button's
// "Initialize a repository…" entry). The intent only changes a header hint;
// the primary-action ladder decides the real action from the inspection.
export function openAddProjectForInit(): void {
  setState({
    addProjectOpen: true,
    addProjectIntent: "init",
    browseLoading: true,
    browseEntries: [],
  })
  runBrowse(null)
}

export function closeAddProject(): void {
  setState({
    addProjectOpen: false,
    addProjectIntent: "add",
    projectPathInspection: null,
  })
}

export function browseDir(path: string | null): void {
  // Navigating away abandons any pending/resolved branch inspection so a late
  // reply for the old selection can't resurface in the new directory.
  setState({ browseLoading: true, projectPathInspection: null })
  runBrowse(path)
}

// Fire the branch pre-flight for a selected git repo, mirroring the TUI's
// `add_project`, which inspects the current branch before adding. The reply
// fills `projectPathInspection` via `onProjectPathInspection`; the dialog shows
// a warning step when it carries one. Runs in the click handler that selects the
// repo — never an effect — like `openAttachWorktree` kicks off its listing.
export function inspectProjectPath(path: string): void {
  setState({
    projectPathInspection: {
      path,
      kind: "repo",
      repoRoot: null,
      gitignoreCandidates: [],
      currentBranch: null,
      warning: null,
      hasCommits: true,
      error: null,
      loading: true,
    },
  })
  // Resolve over REST (replaces the retired `/ws` `inspect_project_path` reply).
  // Ignore a stale reply whose path no longer matches the pending inspection (the
  // user picked a different repo, or the dialog closed) so a late frame can never
  // repopulate a closed/changed selection.
  projectsApi
    .inspectPath(path)
    .then((res) => {
      if (state.projectPathInspection?.path !== path) return
      setState({
        projectPathInspection: {
          path,
          // Treat a missing kind as "repo" (the same version-skew stance as
          // `has_commits !== false` below): an older backend never blocks or
          // offers init, it just behaves as before.
          kind: res.kind ?? "repo",
          repoRoot: res.repo_root ?? null,
          gitignoreCandidates: res.gitignore_candidates ?? [],
          currentBranch: res.current_branch,
          warning: res.warning,
          // Treat a missing/non-false value as "has commits" so an older
          // backend that predates this field (version skew: rolled-back server
          // + cached newer bundle) never wrongly flags every repo as unborn.
          hasCommits: res.has_commits !== false,
          error: null,
          loading: false,
        },
      })
    })
    .catch((e) => {
      if (state.projectPathInspection?.path !== path) return
      setState({
        projectPathInspection: {
          path,
          kind: "repo",
          repoRoot: null,
          gitignoreCandidates: [],
          currentBranch: null,
          warning: null,
          hasCommits: true,
          error: e instanceof Error ? e.message : "Could not inspect the path.",
          loading: false,
        },
      })
    })
}

// Drop any pending/resolved inspection (e.g. the user deselected the repo).
export function clearProjectInspection(): void {
  setState({ projectPathInspection: null })
}

export function addProject(path: string, name: string): void {
  projectsApi
    .create({ path, name })
    .catch((e) =>
      toast.error(e instanceof Error ? e.message : "Could not add the project."),
    )
}

// Check out the repo's default branch first, then add it — the TUI's
// "Check Out & Add" path. Only offered for the Known warning (the server
// re-validates and rejects otherwise). The switch + add run server-side through
// the worker chain; the status stream reports the outcome.
export function addProjectCheckoutDefault(path: string, name: string): void {
  projectsApi
    .create({ path, name, checkout_default: true })
    .catch((e) =>
      toast.error(e instanceof Error ? e.message : "Could not add the project."),
    )
}

// Birth an unborn repo (fresh `git init`, no commits) with an empty initial
// commit, then add it — the server creates the commit before registering so the
// repo can back worktrees. Offered when inspect reports `hasCommits: false`.
export function addProjectCreateInitialCommit(path: string, name: string): void {
  projectsApi
    .create({ path, name, create_initial_commit: true })
    .catch((e) =>
      toast.error(e instanceof Error ? e.message : "Could not add the project."),
    )
}

// Adopt a plain (non-repo) folder: the server runs `git init`, seeds a starter
// .gitignore, creates an empty initial commit, then registers the project.
// Offered when inspect reports `kind: "plain"`. Fire-and-forget like the other
// add variants; the keyed status stream reports the outcome.
export function initProject(path: string, name: string): void {
  projectsApi
    .create({ path, name, init_repo: true })
    .catch((e) =>
      toast.error(e instanceof Error ? e.message : "Could not add the project."),
    )
}

export function openRemoveProject(projectId: string): void {
  setState({ removeProjectTarget: projectId })
}

export function closeRemoveProject(): void {
  setState({ removeProjectTarget: null })
}

export function removeProject(projectId: string): void {
  projectsApi
    .remove(projectId)
    .catch((e) =>
      toast.error(e instanceof Error ? e.message : "Could not remove the project."),
    )
}

export function openDeleteProject(projectId: string): void {
  setState({ deleteProjectTarget: projectId })
}

export function closeDeleteProject(): void {
  setState({ deleteProjectTarget: null })
}

// The destructive cascade: removes the project, its agents, AND their worktrees
// from disk (delete_worktrees=true → WireCommand::DeleteProject). The plain
// keep-worktrees variant is `removeProject`. Fire-and-forget like the other
// project mutations; the keyed status stream reports the outcome, and a refusal
// (e.g. a tab still launching) surfaces as an error toast.
export function deleteProject(projectId: string): void {
  projectsApi
    .deleteWithWorktrees(projectId)
    .catch((e) =>
      toast.error(e instanceof Error ? e.message : "Could not delete the project."),
    )
}

// Update a project's settings (provider / auto-reopen / startup-command / env)
// in one tri-state PATCH. The caller (ProjectSettingsDialog) includes only the
// fields that changed; an omitted field is left untouched, `null` clears it.
export async function updateProjectSettings(
  projectId: string,
  patch: PatchProjectBody,
): Promise<boolean> {
  // Empty patch (nothing changed) is a successful no-op — let the dialog close.
  if (Object.keys(patch).length === 0) return true
  // Validate a provider SET (a non-null provider) up front: the PATCH dispatches
  // its fields as independent wire sub-commands with no rollback, so a provider the
  // server rejects mid-sequence would leave the earlier fields already committed.
  // Catching it here (and the backend's matching up-front check) keeps a bad
  // provider from partially applying. `null` clears the provider and needs no check.
  if (
    patch.provider != null &&
    !providerIsConfigured(patch.provider)
  ) {
    toast.error(`Provider "${patch.provider}" is not configured.`)
    return false
  }
  try {
    await projectsApi.patch(projectId, patch)
    return true
  } catch (e) {
    toast.error(
      e instanceof Error ? e.message : "Could not update project settings.",
    )
    return false
  }
}

// Refresh a project's source checkout from remote (the TUI's
// `refresh_selected_project`). The server resolves the project, runs the pull
// against its source checkout, and reports busy/success/failure on the status
// stream — nothing to do here but fire the command.
export function pullProject(projectId: string): void {
  projectsApi
    .pull(projectId)
    .catch((e) => toast.error(e instanceof Error ? e.message : "pull failed"))
}

// Open the confirm dialog for switching a project's source checkout back to its
// default branch. The actual git work happens server-side after the user
// confirms (the checkout moves HEAD, so it is gated behind a confirmation the
// TUI's deliberate palette action does not need).
export function openCheckoutDefaultBranch(projectId: string): void {
  setState({ checkoutDefaultBranchTarget: projectId })
}

export function closeCheckoutDefaultBranch(): void {
  setState({ checkoutDefaultBranchTarget: null })
}

// Tell the server to inspect and check out the project's default branch. The
// server reports the outcome (switched / already on it / can't determine) on
// the command result, so there is nothing to do here but fire the command.
export function checkoutDefaultBranch(projectId: string): void {
  projectsApi
    .checkoutDefault(projectId)
    .catch((e) =>
      toast.error(e instanceof Error ? e.message : "checkout failed")
    )
}

// Open the attach-worktree dialog for a project and immediately request its
// managed-worktree listing (the server classifies in spawn_blocking). The
// listing reply fills `attachWorktreeEntries` via `onProjectWorktrees`. Runs in
// the click handler that opens the dialog — never an effect — mirroring how
// `openAddProject` kicks off its browse.
export function openAttachWorktree(projectId: string): void {
  setState({
    attachWorktreeTarget: projectId,
    attachWorktreeEntries: [],
    attachWorktreeLoading: true,
  })
  // Fetch the managed-worktree listing over REST (replaces the retired `/ws`
  // `list_project_worktrees` → `project_worktrees` reply). Ignore a stale reply
  // if the dialog closed (or switched projects) before it arrived.
  projectsApi
    .worktrees(projectId)
    .then((res) => {
      if (state.attachWorktreeTarget !== projectId) return
      setState({
        attachWorktreeEntries: res.entries,
        attachWorktreeLoading: false,
      })
    })
    .catch((e) => {
      if (state.attachWorktreeTarget !== projectId) return
      setState({ attachWorktreeEntries: [], attachWorktreeLoading: false })
      toast.error(
        e instanceof Error ? e.message : "Could not list the worktrees.",
      )
    })
}

export function closeAttachWorktree(): void {
  setState({
    attachWorktreeTarget: null,
    attachWorktreeEntries: [],
    attachWorktreeLoading: false,
  })
}

// Ask the server to adopt a managed worktree as a new agent. The server
// re-validates the path against a fresh classification (never trusting this
// list) and validates `name` as a display name, then dispatches the create
// worker — the outcome (busy/success/failure) arrives on the status stream.
export function attachWorktree(
  projectId: string,
  worktreePath: string,
  name: string,
): void {
  armCreateFocus(projectId)
  sessionsApi
    .create({ kind: "from_worktree", project_id: projectId, worktree_path: worktreePath, name })
    .catch((e) => toastCreateError(e, "Could not attach the worktree."))
}

// Open the new-agent dialog. The checkbox starts checked when
// `randomize_agent_names_by_default` is set (mirroring the TUI prompt, which
// pre-checks when opened with no initial name); in that case we request a name
// right away so the input previews it. This runs in the click handler that opens
// the dialog — never an effect — so there is no set-state-in-effect.
export function openCreateAgent(projectId: string): void {
  openNameDialog({ kind: "new", projectId })
}

// Open the name dialog in fork mode for an existing session. Reuses the exact
// new-agent UX (sanitized input, pet-name checkbox, generated-name plumbing);
// only the dispatch target differs. Unlike create, a fork REQUIRES a name (the
// server rejects an empty fork), so the dialog's Fork button is disabled while
// the input is empty.
export function openForkAgent(sessionId: string): void {
  openNameDialog({ kind: "fork", sessionId })
}

// Open the name dialog in "from PR" mode for a project. Reuses the new-agent
// name UX (sanitized input, pet-name checkbox, generated-name plumbing) and adds
// a PR-reference field; on submit it dispatches `create_agent_from_pr`. Mirrors
// the TUI's `new-agent-from-pr` flow, which resolves the PR then names the agent.
export function openCreateAgentFromPr(projectId: string): void {
  openNameDialog({ kind: "pr", projectId })
}

// Shared opener for all modes of the name dialog. Pre-checks the randomize
// default and requests a name right away so the input previews it, exactly like
// the TUI prompt — EXCEPT in PR mode: the TUI seeds the PR's head branch as the
// name and never randomizes there (a pet name would become the branch the PR
// head is fetched into). The web doesn't know the head branch until after the
// lookup, so PR mode opens blank and the server's head-branch fallback applies.
// Runs in the click handler that opens the dialog — never an effect — so there
// is no set-state-in-effect.
// Request a fresh pet name for the new-agent dialog over REST (replaces the
// retired `/ws` `generate_agent_name` → `agent_name` reply). The TUI fills the
// input with the generated name (that fill IS the preview) and remembers it so a
// later uncheck can tell "still the generated name" from "user-edited". We mirror
// that: fill the draft and stash the name. Ignored if the dialog closed or the
// user unchecked the box before the reply landed (a stale reply must not refill).
// A failure stops the spinner so the user can type a name by hand.
function requestAgentName(): void {
  browseApi
    .agentName()
    .then((res) => {
      if (state.createAgentTarget !== null && state.createAgentRandomize) {
        setState({
          createAgentDraft: res.name,
          createAgentGeneratedName: res.name,
          createAgentNamePending: false,
        })
      }
    })
    .catch(() => {
      if (state.createAgentTarget !== null) {
        setState({ createAgentNamePending: false })
      }
    })
}

function openNameDialog(target: CreateAgentTarget): void {
  const randomize =
    target.kind !== "pr" &&
    (state.bootstrap?.randomize_agent_names_by_default ?? false)
  setState({
    createAgentTarget: target,
    createAgentDraft: "",
    createAgentRandomize: randomize,
    // Seeded from the config default; older servers omit the field, so fall
    // back to true (the server-side default). Only "new" mode surfaces it.
    createAgentCopyChanges:
      state.bootstrap?.copy_uncommitted_changes_by_default ?? true,
    createAgentGeneratedName: null,
    createAgentNamePending: randomize,
    createAgentPrInput: "",
  })
  if (randomize) requestAgentName()
}

export function closeCreateAgent(): void {
  setState({
    createAgentTarget: null,
    createAgentDraft: "",
    createAgentRandomize: false,
    createAgentGeneratedName: null,
    createAgentNamePending: false,
    createAgentPrInput: "",
  })
}

// Update the PR-reference field. Free text — unlike the agent name, this is NOT
// sanitized (a PR URL contains slashes, colons, etc.); the server parses it.
export function setCreateAgentPrInput(raw: string): void {
  setState({ createAgentPrInput: raw })
}

// Update the input as the user types, sanitizing live (space -> dash, drop
// disallowed chars, etc.) exactly like the TUI char map. Editing away from the
// generated name clears the remembered name so a later uncheck keeps the edits.
export function setCreateAgentDraft(raw: string): void {
  const draft = sanitizeAgentName(raw)
  const generated =
    draft === state.createAgentGeneratedName ? state.createAgentGeneratedName : null
  setState({ createAgentDraft: draft, createAgentGeneratedName: generated })
}

// Toggle the "Copy uncommitted changes from the project checkout" checkbox.
export function toggleCreateAgentCopyChanges(): void {
  setState({ createAgentCopyChanges: !state.createAgentCopyChanges })
}

// Toggle the "Use randomized pet name" checkbox with the TUI's exact semantics:
//   ON  -> request a fresh name (the reply fills the input via `onAgentName`).
//   OFF -> clear the input ONLY if it still equals the generated name; otherwise
//          keep the user's edits. Either way, forget the generated name.
export function toggleCreateAgentRandomize(): void {
  if (!state.createAgentRandomize) {
    setState({ createAgentRandomize: true, createAgentNamePending: true })
    requestAgentName()
  } else {
    const keepText = state.createAgentDraft !== state.createAgentGeneratedName
    setState({
      createAgentRandomize: false,
      createAgentDraft: keepText ? state.createAgentDraft : "",
      createAgentGeneratedName: null,
      // Unchecking abandons any in-flight request; its reply is ignored by
      // `onAgentName` (randomize is false by then), so stop the spinner now.
      createAgentNamePending: false,
    })
  }
}

// Surface a create-action REST error as a toast, EXCEPT a 409 Conflict. A 409
// means the engine's in-flight create guard refused: it returns an `Ok`
// error-toned status that the engine ALSO broadcasts over `/ws` (scoped to this
// connection) and that the REST handler maps to 409. Toasting the 409 here would
// double up — the user would see two identical toasts for one refusal. The `/ws`
// status stream is the single surface for that case; every other status still
// toasts. Network failures (`status === 0`) and all other codes are surfaced.
function toastCreateError(e: unknown, fallback: string): void {
  if (e instanceof SessionsApiError && e.status === 409) return
  toast.error(e instanceof Error ? e.message : fallback)
}

// Ask the server to create a new agent in a project. An empty name lets the
// server auto-generate a branch name (the equivalent outcome to the TUI's
// generate-a-pet-name path). With the checkbox checked the input is effectively
// never empty, so the empty path is the unchecked-and-blank case.
export function createAgent(
  projectId: string,
  name: string,
  copyUncommittedChanges?: boolean,
  useExistingBranch?: boolean,
): void {
  sessionsApi
    .create({
      kind: "new",
      project_id: projectId,
      name,
      copy_uncommitted_changes: copyUncommittedChanges,
      use_existing_branch: useExistingBranch,
    })
    .catch((e) => {
      // The server refused an unconfirmed existing-branch attach: open the
      // confirmation instead of toasting, so the user can consent (or cancel)
      // rather than silently adopting that branch's history.
      const conflict = existingBranchConflict(e)
      if (conflict) {
        setState({
          existingBranchTarget: {
            projectId,
            name,
            copyChanges: copyUncommittedChanges,
            location: conflict.location,
          },
        })
        return
      }
      toastCreateError(e, "Could not create the agent.")
    })
}

/** Confirm the pending existing-branch attach: re-create with the flag set. */
export function confirmCreateWithExistingBranch(): void {
  const target = state.existingBranchTarget
  if (!target) return
  setState({ existingBranchTarget: null })
  createAgent(target.projectId, target.name, target.copyChanges, true)
}

export function closeExistingBranch(): void {
  setState({ existingBranchTarget: null })
}

// Flat-list display controls (shared desktop + mobile), plus the New-agent
// picker's open/close. All plain state writes.
// Set the flat-list sort mode and persist it server-side (config.ui.agent_sort).
// Optimistic: set the override now, POST to the dedicated endpoint; the engine
// persists and emits config.changed, and the refetched bootstrap drops the
// override (applyBootstrap) once config matches. On failure, clear the override so
// the UI snaps back to the authoritative config value.
export function setAgentSort(sort: FlatSortKey): void {
  setState({ agentSort: sort })
  configApi.setAgentSort(sort).catch((e) => {
    setState({ agentSort: null })
    toast.error(
      e instanceof Error ? e.message : "Could not change the agent sort.",
    )
  })
}

// The effective sort mode: optimistic override, else the server-persisted config,
// else the default. Consumers read this, never the raw override field.
export function agentSortValue(s: DuxState): FlatSortKey {
  return s.agentSort ?? s.bootstrap?.agent_sort ?? "active"
}

export function setAgentSearch(query: string): void {
  setState({ agentSearch: query })
}

export function openNewAgentPicker(
  intent: DuxState["newAgentPickerIntent"] = "new",
): void {
  setState({ newAgentPickerOpen: true, newAgentPickerIntent: intent })
}

export function closeNewAgentPicker(): void {
  setState({ newAgentPickerOpen: false })
}

// Ask the server to fork an existing session into a fresh branched worktree.
// Unlike create, a fork requires a non-empty name (the server rejects empty).
export function forkAgent(sessionId: string, name: string): void {
  sessionsApi
    .create({ kind: "fork", session_id: sessionId, name })
    .catch((e) => toastCreateError(e, "Could not fork the session."))
}

// Ask the server to create an agent checked out on a GitHub PR's head branch.
// `pr` is the raw reference (URL, `#123`, or `123`); the server resolves it via
// `gh pr view`. An empty `name` falls back to the PR head branch, matching the
// TUI prompt default. The lookup+create runs asynchronously: the command returns
// a busy status synchronously and the outcome arrives on the status stream.
export function createAgentFromPr(projectId: string, pr: string, name: string): void {
  sessionsApi
    .create({ kind: "from_pr", project_id: projectId, pr, name })
    .catch((e) => toastCreateError(e, "Could not create the agent from the PR."))
}

// Submit the name dialog: dispatch create, fork, or create-from-PR based on the
// current target, then close. Mirrors the TUI, where the same name prompt drives
// these flows.
export function submitNameDialog(name: string): void {
  const target = state.createAgentTarget
  if (!target) return
  if (target.kind === "new") {
    armCreateFocus(target.projectId)
    createAgent(target.projectId, name, state.createAgentCopyChanges)
  } else if (target.kind === "fork") {
    // A fork lands in the same project as its source session; resolve it so the
    // focus diff is scoped to that project. If the source vanished from the
    // ViewModel, skip auto-focus rather than arming an unscoped token that could
    // grab any project's next new session.
    const projectId = state.spine?.sessions.find(
      (s) => s.id === target.sessionId,
    )?.project_id
    if (projectId) armCreateFocus(projectId)
    forkAgent(target.sessionId, name)
  } else {
    armCreateFocus(target.projectId)
    createAgentFromPr(target.projectId, state.createAgentPrInput.trim(), name)
  }
  closeCreateAgent()
}

// Optimistically reorder a project's sessions, then tell the server. `orderedIds`
// MUST be the complete ordered set of that project's session ids — the server
// validates it as a strict permutation and rejects partial/stale sets. The
// overlay clears when the next spine confirms the order (or on error).
// Flat model: reorder every agent as one global list. `orderedIds` MUST be the
// complete set of ALL session ids (the server validates it as a strict
// permutation and rejects partial/stale sets). Optimistic overlay clears when the
// next spine confirms the order (or on error).
export function reorderAgents(orderedIds: string[]): void {
  setState({ pendingAgentOrder: orderedIds })
  sessionsApi.reorderGlobal(orderedIds).catch((e) => {
    // A rejected reorder is never reconciled by a spine, so the overlay would
    // linger forever. Clear it so the UI snaps back to the authoritative order,
    // then surface the failure.
    setState({ pendingAgentOrder: null })
    toast.error(e instanceof Error ? e.message : "Could not reorder the agents.")
  })
}

// Flat model: reorder every terminal as one global list (the twin of
// `reorderAgents`). `orderedIds` MUST be the complete set of ALL terminal ids (any
// owner) in the desired order. The server validates it as a strict permutation and
// rejects a partial/stale set. The optimistic `pendingTerminalOrder` overlay clears
// when the next spine confirms the order (or on error). Terminal order is
// runtime-only, so this resets to creation order on restart.
export function reorderTerminals(orderedIds: string[]): void {
  setState({ pendingTerminalOrder: orderedIds })
  terminalsApi.reorder(orderedIds).catch((e) => {
    // A rejected reorder is never reconciled by a spine, so the overlay would
    // linger forever. Clear it so the UI snaps back to the authoritative order,
    // then surface the failure.
    setState({ pendingTerminalOrder: null })
    toast.error(e instanceof Error ? e.message : "Could not reorder the terminals.")
  })
}

export function reorderSessions(projectId: string, orderedIds: string[]): void {
  setState({ pendingSessionOrder: { projectId, ids: orderedIds } })
  sessionsApi
    .reorder(projectId, orderedIds)
    .catch((e) => {
      // A rejected reorder will never be reconciled by a spine (the server never
      // persisted this order), so the optimistic overlay would otherwise linger
      // forever — leaving the sidebar showing an order the server doesn't have and
      // compounding on the next drag. Clear the order overlays so the UI snaps back
      // to the authoritative spine order, then surface the failure.
      setState(clearPendingOrders())
      toast.error(
        e instanceof Error ? e.message : "Could not reorder the sessions.",
      )
    })
}

// Sort every project's sessions by the chosen key. This is the app menu's
// ONE-SHOT reorder (distinct from the flat-list sidebar sort control, which sets
// the shared `config.ui.agent_sort` display mode via setAgentSort): for each
// project we compute the sorted id order (sortedSessionIds, which mirrors the TUI
// comparators exactly) and send the EXISTING `reorder_sessions` command, which
// the server persists into the shared global order — so the stored order the TUI
// displays under "manual" stays in sync by construction.
//
// We deliberately DON'T set the optimistic `pendingSessionOrder` overlay here.
// That overlay holds a single project; a sort touches N projects, so an overlay
// could only cover one of them and would leave the rest snapping anyway. The
// spine echo arrives within tens of milliseconds, so the brief reflow is
// acceptable and keeps the single-project drag overlay invariant untouched.
// Projects with fewer than two sessions are skipped — sorting them is a no-op
// that would only churn the wire.
export function sortAgents(by: SortKey): void {
  const sessions = state.spine?.sessions ?? []
  const projects = state.spine?.projects ?? []
  // A sort supersedes any in-flight drag: drop its overlay up front, or a
  // superseded drag order would linger on screen until something else clears
  // it (the overlay only retires on match/error/disconnect).
  setState(clearPendingOrders())
  for (const project of projects) {
    const projectSessions = sessions.filter((s) => s.project_id === project.id)
    if (projectSessions.length < 2) continue
    const orderedIds = sortedSessionIds(projectSessions, by)
    sessionsApi
      .reorder(project.id, orderedIds)
      .catch((e) =>
        toast.error(
          e instanceof Error ? e.message : "Could not reorder the sessions.",
        ),
      )
  }
}

// Optimistically reorder the projects, then tell the server. `orderedIds` MUST
// be the complete ordered set of ALL project ids (both with and without agents);
// the server validates it as a strict permutation. The overlay clears when the
// next spine confirms the order (or on error).
export function reorderProjects(orderedIds: string[]): void {
  setState({ pendingProjectOrder: orderedIds })
  projectsApi
    .reorder(orderedIds)
    .catch((e) => {
      // As with sessions: a rejected reorder is never reconciled by a spine, so
      // the optimistic overlay would persist indefinitely. Clear it back to the
      // authoritative order before surfacing the error.
      setState(clearPendingOrders())
      toast.error(
        e instanceof Error ? e.message : "Could not reorder the projects.",
      )
    })
}


// Run a macro by name on the focused PTY. Since Phase 5 the web no longer sends a
// server-side `run_macro` command: it resolves the macro's text from the bootstrap
// document, applies the newline→Alt+Enter transform (`macroPayloadBytes`, an exact
// port of the engine's), and writes the payload straight to the active PTY socket
// as stdin — the same socket the focused terminal pane drives. The macro picker is
// already filtered to the focused surface, so the active socket IS the macro's
// target. No-op if the macro is unknown or no terminal is focused (no active
// socket). The text is pasted WITHOUT a trailing submit, mirroring the TUI: the
// user reviews it in the prompt and presses Enter to send.
export function runMacro(name: string): void {
  const macro = (state.bootstrap?.macros ?? []).find((m) => m.name === name)
  if (!macro) return
  // Defensive: only inject when a terminal is actually focused. During a focus
  // switch the outgoing pane may not have cleared its registration yet; without
  // a selected target the active socket is stale, and writing to it would paste
  // the macro into the wrong (just-detached) PTY.
  if (state.selectedTarget === null) return
  const pty = getActivePtySocket()
  if (pty === null) return
  pty.sendInput(macroPayloadBytes(macro.text))
}

// Open the macro-editor dialog, seeding the draft from the current bootstrap
// macros (a fresh copy so edits don't mutate the shared model). Runs in the
// click/palette handler that opens the dialog — never an effect.
export function openMacrosDialog(): void {
  const macros = state.bootstrap?.macros ?? []
  setState({
    macrosDialogOpen: true,
    macrosDraft: macros.map((m) => ({ ...m })),
  })
}

export function closeMacrosDialog(): void {
  setState({ macrosDialogOpen: false, macrosDraft: [] })
}

// Persist the draft wholesale via `update_macros`. The server validates
// (empty/duplicate names, empty text, unknown surface) and reports the outcome
// on the status lane; a config reload emits `config.changed`, refetching
// `bootstrap.macros`. The dialog closes optimistically — a rejection surfaces as
// an error toast, and reopening re-seeds from the (unchanged) bootstrap.
export function saveMacros(macros: MacroView[]): void {
  // `update_macros` is a WHOLESALE replace of the entire `[macros]` map. Before
  // the bootstrap document has loaded, `openMacrosDialog` seeded an EMPTY draft,
  // so saving would wipe the server's macros. Refuse until we hold the
  // authoritative list (the Save button is also disabled in this window).
  if (state.bootstrap === null) {
    toast.error("Macros aren't loaded yet. Try again in a moment.")
    return
  }
  configApi
    .updateMacros(macros)
    .catch((e) =>
      toast.error(e instanceof Error ? e.message : "Could not save the macros."),
    )
  closeMacrosDialog()
}

// Mobile hub-&-spoke navigation. Moving INTO a spoke ("terminal" or "changes")
// pushes a history entry so the hardware/browser Back button unwinds the stack
// one screen at a time (see the popstate listener above). Navigating to "home"
// is a programmatic return: rather than just flipping state (which would leave
// the pushed spoke entries dangling), it routes through `unwindMobileSpoke` so
// the history depth collapses to match — keeping the back stack honest for any
// future caller. Re-navigating to the screen we're already on is a no-op so we
// never stack duplicate history entries (e.g. switching sessions while already
// on the terminal screen must not deepen the back stack). The comparison reads
// the LATEST `state.mobileScreen`, so a tap that races a pending popstate still
// sees the up-to-date screen and won't double-push.
export function mobileNavigate(screen: MobileScreen): void {
  if (screen === state.mobileScreen) return
  if (screen === "home") {
    unwindMobileSpoke()
    return
  }
  setState({ mobileScreen: screen })
  history.pushState({ duxMobile: screen }, "")
}

export function reconnect(): void {
  // Both sockets now give up after the shared MAX_RECONNECT_ATTEMPTS and signal
  // "failed"; a manual Retry must restore EVERYTHING. connect() resets the events
  // socket's closedByUser/attempts/delay (safe on an exhausted socket), and the
  // `terminalEpoch` bump remounts the focused TerminalPane so its (now-capped)
  // PtySocket reconnects with a fresh budget too. Without the bump, one Retry
  // would revive the spine but leave the terminal dead.
  eventsSocket.connect()
  setState({ terminalEpoch: state.terminalEpoch + 1 })
}

// Update the expanded sidebar width during a drag. Pass `persist` on release to
// write the final value to localStorage.
export function setSidebarWidth(width: string, persist = false): void {
  setState({ sidebarWidth: width })
  if (persist) {
    localStorage.setItem(SIDEBAR_WIDTH_KEY, width)
  }
}

// The Changes pane's effective visibility (desktop): the per-session override if
// set, else the config default from the bootstrap document, else visible (the
// pre-load window before the first bootstrap fetch lands).
export function changesPaneVisible(s: DuxState): boolean {
  return s.changesPaneOverride ?? s.bootstrap?.show_changes_pane ?? true
}

// Set the Changes pane's visibility and persist it (config.ui.show_changes_pane).
// The override is set optimistically for an instant response; the server writes
// config.ui.show_changes_pane and emits `config.changed`, the refetched bootstrap
// document carries the confirmed value, and `applyBootstrap` drops the override
// so config is the single source of truth across every connected client. Rolls
// the optimistic override back with a toast on error. Resolves to whether the
// persist succeeded so a caller (the customize-webapp dialog) can gate on it;
// fire-and-forget callers ignore the returned promise.
export function setChangesPaneVisibility(next: boolean): Promise<boolean> {
  setState({ changesPaneOverride: next })
  return configApi
    .setChangesPaneVisible(next)
    .then(() => true)
    .catch((e) => {
      // Roll the optimistic override back so the pane doesn't strand in the
      // toggled state when the persist fails.
      setState({ changesPaneOverride: null })
      toast.error(
        e instanceof Error ? e.message : "Could not toggle the Changes pane.",
      )
      return false
    })
}

// Toggle the Changes pane's visibility. Called by the Changes actions menu; the
// saved preference itself is the `ui.show_changes_pane` Preferences row.
export function toggleChangesPane(): void {
  setChangesPaneVisibility(!changesPaneVisible(state))
}



// The Task Manager (the app menu's "Task Manager…"). Open/close just flip the
// gate; the dialog derives its rows from the spine and polls the stats itself
// while open.
export function openTaskManager(): void {
  setState({ taskManagerOpen: true })
}

export function closeTaskManager(): void {
  setState({ taskManagerOpen: false, stopAllOpen: false })
}

// The "Stop all…" confirmation nested inside the Task Manager.
export function openStopAll(): void {
  setState({ stopAllOpen: true })
}

export function closeStopAll(): void {
  setState({ stopAllOpen: false })
}

// Stop every running agent and companion terminal. Agents are DETACHED (the
// worktree and session survive and can be reconnected), which is why this stops
// each agent as a whole rather than closing its tabs one by one: closing tabs
// would also destroy the extra tabs' pills, and the panic button should leave as
// much recoverable as possible. Terminals have no detached state (existence ==
// running), so they are destroyed. Gated by its own confirmation.
export function stopAllRunning(): void {
  const sessions = state.spine?.sessions ?? []
  for (const s of sessions) {
    if (s.status === "active") killSessionPty(s.id)
    for (const t of s.terminals) deleteTerminal(t.id)
  }
  // Project terminals live on projects, not sessions; the panic button must
  // reach them too, or a hung project terminal survives "Stop all".
  for (const p of state.spine?.projects ?? []) {
    for (const t of p.terminals) deleteTerminal(t.id)
  }
}

// The Preferences dialog (the app menu's "Preferences…"). Open/close just flip the
// gate; the dialog seeds its title, favicon, and Changes pane fields from the
// bootstrap document.
export function openCustomizeWebapp(): void {
  setState({ customizeWebappOpen: true })
}

export function closeCustomizeWebapp(): void {
  setState({ customizeWebappOpen: false })
}

// Persist the instance identity (browser tab title + favicon colour). The
// server validates + writes config.toml and emits `config.changed`, so
// `applyBootstrap` re-applies the tab title, wordmark, and favicon on every
// client. We do NOT hand-apply here — config is the single source of truth. A
// success toast is the engine's routed status; here we only surface a failure.
// Resolves to whether the persist succeeded so the customize-webapp dialog can
// gate its close on it; fire-and-forget callers ignore the returned promise.
export function setInstanceIdentity(body: {
  title?: string
  favicon?: string
}): Promise<boolean> {
  return configApi
    .setInstanceIdentity(body)
    .then(() => true)
    .catch((e) => {
      toast.error(
        e instanceof Error ? e.message : "Could not rename this instance.",
      )
      return false
    })
}

// Persist an explicit patch of the Settings modal's `[ui]`/`[capabilities]`
// fields (everything the modal exposes EXCEPT title/favicon, which stay on
// `setInstanceIdentity`). Mirrors `setInstanceIdentity`'s resolves-to-boolean +
// toast-on-error contract so the dialog can `Promise.all` both writes and gate
// its close on every one succeeding. The server validates/clamps and emits
// `config.changed`; we do NOT hand-apply here. The refetched bootstrap is the
// single source of truth, so the dialog re-seeds from it.
export function saveSettings(
  patch: Parameters<typeof configApi.patchSettings>[0],
): Promise<boolean> {
  return configApi
    .patchSettings(patch)
    .then(() => true)
    .catch((e) => {
      toast.error(e instanceof Error ? e.message : "Could not save settings.")
      return false
    })
}

// Force-kill one agent's PTY. The agent detaches (it is NOT deleted) and can be
// reconnected; the spine refetch flips its row to detached. A success toast is
// the engine's routed status; here we only surface a failure. Companion
// terminals are killed through the existing `deleteTerminal`.
export function killSessionPty(sessionId: string): void {
  sessionsApi
    .kill(sessionId)
    .catch((e) =>
      toast.error(
        e instanceof Error ? e.message : "Could not kill the agent.",
      ),
    )
}

// The Monaco config.toml editor (the app menu's "Edit config file…"). Open fetches the raw
// file text into the store so the editor seeds from a settled value. The
// monotonic epoch makes each open session unique: a fetch reply is applied only
// if its epoch still matches, so an open-close-open (or Retry) within the fetch
// round-trip can't seed the editor with a previous session's stale content.
let configEditorEpoch = 0

export function openConfigEditor(): void {
  const epoch = ++configEditorEpoch
  setState({
    configEditorOpen: true,
    configEditorLoading: true,
    configEditorError: null,
    configEditorContent: "",
  })
  configApi
    .readRawConfig()
    .then((content) => {
      if (configEditorEpoch !== epoch) return
      setState({ configEditorContent: content, configEditorLoading: false })
    })
    .catch((e) => {
      if (configEditorEpoch !== epoch) return
      setState({
        configEditorLoading: false,
        configEditorError:
          e instanceof Error ? e.message : "Could not read config.toml.",
      })
    })
}

export function closeConfigEditor(): void {
  // Bump the epoch so any in-flight open fetch is ignored when it resolves.
  configEditorEpoch++
  setState({
    configEditorOpen: false,
    configEditorContent: "",
    configEditorLoading: false,
    configEditorError: null,
  })
}

// Save the edited config.toml. The server validates the TOML before writing: a
// rejection (invalid TOML) surfaces inline via `configEditorError` and keeps the
// modal open so the user can fix it. On a successful write we adopt it with the
// existing reload (best-effort — the file is already persisted), close, and toast.
export function saveConfigEditor(content: string): void {
  setState({ configEditorError: null })
  configApi
    .writeRawConfig(content)
    .then(() => {
      // Save PERSISTS but does not APPLY: the server writes config.toml and leaves
      // the running config untouched (no adopt, no `config.changed`) until the user
      // explicitly runs "Reload config". The toast states exactly that so the lack
      // of a visible change isn't mistaken for a no-op.
      closeConfigEditor()
      toast.success("Saved config.toml. Run “Reload config” to apply it.")
    })
    .catch((e) => {
      setState({
        configEditorError:
          e instanceof Error ? e.message : "Could not save config.toml.",
      })
    })
}
