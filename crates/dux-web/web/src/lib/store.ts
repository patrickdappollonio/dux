import { useSyncExternalStore } from "react"
import { sanitizeAgentName } from "./agentName"
import { git } from "./git"
import {
  projectsApi,
  ProjectsApiError,
  type PatchProjectBody,
} from "./projectsApi"
import { existingBranchConflict, sessionsApi, SessionsApiError } from "./sessionsApi"

import { ordersMatch, reorderById } from "./reorder"
import { sortedSessionIds, type SortKey } from "./sortSessions"
import { nextActiveSessionId, type FlatSortKey } from "./flatList"
import { isMobileViewport } from "@/hooks/use-mobile"
import { EventsSocket } from "./eventsSocket"
import { getActivePtySocket } from "./ptySocket"
import { getComposeInsertSink } from "./composeInsert"
import { notifyPtyOwner, resetPtyOwnerEpochs } from "./ptyOwnership"
import { macroPayloadBytes } from "./macros"
import { terminalsApi } from "./terminalsApi"
import { tabsApi } from "./tabsApi"
import { browseApi } from "./browseApi"
import { configApi } from "./configApi"
import { setConnectionId } from "./connection"
import {
  fetchServerIdentity,
  serverChanged,
  type ServerIdentity,
} from "./buildApi"
import { reloadPage } from "./reloadPage"
import { noteServerRunProbe } from "./serverRun"
import {
  DIVIDER_STORAGE_KEYS,
  readStoredPanePercent,
  readStoredText,
  writeStoredPanePercent,
  writeStoredText,
} from "./paneDivider"
import { DEFAULT_SIDEBAR_WIDTH } from "./sidebarResize"
import {
  ChangesFetchError,
  fetchChanges,
  type SessionChangesResponse,
} from "./changesApi"
import {
  type Bootstrap,
  type PendingFirstLoad,
  type ReleaseNotesView,
  fetchBootstrap,
} from "./bootstrapApi"
import { firstLoadApi } from "./firstLoadApi"
import {
  dismissNotification,
  notify,
  notifyError,
  notifyInfo,
  notifyStatus,
  notifySuccess,
  notifyWarning,
  setStatusClearSeconds,
} from "./notify"
import { publishConnectionTiming } from "./connectionTiming"
import { clearServerValidated, noteServerValidated } from "./serverValidated"
import { registerPageLifecycle } from "./pageLifecycle"
import { worktreeDeleteReport } from "./worktreeDelete"
import { attentionCountForSurface, formatTabTitle } from "./attention"
import { applyAttentionFavicon } from "./favicon"
import { statusToastAllowed } from "./statusRouting"
import { pageTitle, resolveInstanceTitle } from "./instanceTitle"
import {
  type RawWorkspace,
  type Spine,
  fetchWorkspace,
  normalizeWorkspace,
} from "./workspaceApi"
import {
  type PendingSlotTab,
  isFirstTab,
  isSlotTabTarget,
  resolveFocusedTab,
  shouldRefireFocusPut,
  slotTabIdOf,
  slotTabTargetId,
} from "./agentTabs"
import {
  activateTab as editorActivateTabPure,
  closeTab as editorCloseTabPure,
  closeTabsUnderPath as editorCloseTabsUnderPathPure,
  emptyTabsState,
  hasAnyDirtyTab,
  openFile as editorOpenFilePure,
  pinTab as editorPinTabPure,
  renameTabPaths as editorRenameTabPathsPure,
  setTabDirty as editorSetTabDirtyPure,
  setTabMode as editorSetTabModePure,
} from "./editorTabs"
import type { EditorTabsState } from "./editorTabs"
import {
  agentRoot,
  editorRootForTarget,
  rootHasDiff,
  rootKey,
  sameRoot,
  type EditorRoot,
  type TerminalTarget,
} from "./editorRoot"
import {
  clearRootDrafts,
  pruneRootDrafts,
  syncBeforeUnloadGuard,
} from "./editorDrafts"
import { isImagePreviewPath } from "./editorPreview"
import { newClientId } from "./uid"
import { assertNever } from "./assertNever"
import {
  matchOwner,
  ownerRefFromWire,
  ownerSessionId,
  type TerminalOwnerRef,
} from "./terminalOwner"
import { ownerHasTerminal } from "./terminals"
import {
  clearTheaterMemory,
  readTheaterMemory,
  splitTheaterHash,
  theaterMemoryKey,
  theaterMemoryKeyForPty,
  theaterSerializable,
  withTheaterHash,
  writeTheaterMemory,
} from "./theater"
import type {
  BranchWarningView,
  InspectKind,
  ChangedFileView,
  ConnState,
  DirEntryView,
  EventsServerMessage,
  MacroView,
  ProjectWorktreeEntryView,
  SessionView,
  StartupLogContent,
  StartupLogEntry,
  TerminalView,
} from "./types"
import { workspaceProjectId } from "@/lib/agentWorkspace"

// Who a companion terminal belongs to, defined in `lib/terminalOwner.ts`
// alongside the exhaustive switches that consume it and re-exported here for
// the `from "@/lib/store"` imports. Every consumer switches on `kind` and ends
// that switch in `assertNever`; there is deliberately no bare-id accessor, so
// no owner kind can be silently ignored by session-shaped code.
export type { TerminalOwnerRef } from "./terminalOwner"

// What the editor is rooted at: an agent's worktree, or the directory a
// terminal was spawned in. Re-exported so the many `from "@/lib/store"` editor
// imports keep working.
export type { EditorRoot } from "./editorRoot"

// The currently-streamed target: either an agent session or a companion
// terminal. An agent target carries a `sessionId` for session-scoped UI (the
// breadcrumb, changed files); a terminal target carries its OWNER, which is a
// session or a project, and a project terminal has no session context at all.
export type SelectedTarget =
  // An agent tab. `tabId` is the session's `slot_tab_id` for the session-slot
  // tab; an extra tab carries
  // its own id. The streamed PTY and all per-tab UI resolve from `tabId`, while
  // session-scoped UI keeps using `sessionId`.
  | { kind: "agent"; sessionId: string; tabId: string }
  | TerminalTarget

// The mobile hub-&-spoke shell shows one screen at a time: the project/session
// hub ("home"), the focused terminal, or the changed-files view. Desktop never
// reads this; it renders all three panes at once.
export type MobileScreen = "home" | "terminal" | "changes"

// A route the URL names but the workspace cannot resolve. Only agents get one:
// a session id is stable and a link to one is worth telling the truth about,
// whereas terminal ids are ephemeral by design and fall back to their owner.
export interface RouteNotFound {
  kind: "agent"
  sessionId: string
}

/** Which of the two first-load screens the dialog is showing. */
export type FirstLoadScreen = "welcome" | "whats_new"

/**
 * Which set of startup-command runs the log viewer is showing: one agent's own
 * runs, or every run across every agent of a project. The TS mirror of
 * `dux_core::startup::StartupCommandLogScope`, which is the one place the two
 * meanings are defined; the server serves each scope from its own route.
 */
export type StartupLogsScope = "agent" | "project"

/**
 * The open first-load dialog. ONE dialog serves both screens; only the text and
 * the two buttons differ, so this carries the union of what either needs.
 */
export interface FirstLoadDialogState {
  screen: FirstLoadScreen
  /**
   * True when this is THIS LAUNCH's automatic screen (the server offered it in
   * the bootstrap document). Closing an automatic screen DISMISSES it; the
   * server records the version as seen in SQLite, settling it for the TUI too.
   * An on-demand open from the app menu is `false` and dismisses nothing:
   * looking something up is not the same as acknowledging this launch's screen.
   */
  automatic: boolean
  /** The release notes, for the what's-new screen. Null while loading, or when
   *  the screen is the welcome one. */
  notes: ReleaseNotesView | null
  /** An in-flight on-demand notes fetch. The automatic screen never loads: the
   *  server already had the notes in hand before offering the screen. */
  loading: boolean
  /** A failed on-demand fetch, shown in place of the body. Also toasted, so a
   *  failure is never silent. */
  error: string | null
}

// The name-input dialog (one component, two modes) targets either a fresh agent
// in a project or a fork of an existing session. The shared draft/randomize/
// generated/pending state below drives both; only the dispatch target differs.
export type CreateAgentTarget =
  | { kind: "new"; projectId: string }
  | { kind: "fork"; sessionId: string }
  // `projectId: null` is the REFERENCE-FIRST shape: opened from the global
  // command, no project is chosen and none is asked for. dux works out which
  // project the reference belongs to on submit. `Some` is the project-first
  // shape, opened from a project's own menu, which behaves exactly as before.
  | { kind: "pr"; projectId: string | null }

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

// Where the agent this client is creating will land, which is what makes a new
// session in the next spine recognizable as ours (see `armCreateFocus`).
//
// A tagged pair rather than a nullable project id: a nullable field spells
// "standalone" and "the project could not be resolved" the same way, and a
// caller that cannot resolve a project must skip arming rather than arm a token
// that grabs the next standalone agent to appear.
export type CreateFocusScope =
  | { kind: "project"; projectId: string }
  | { kind: "standalone" }

// The changed-files request state machine for the selected session, and the one
// source of changed-files data in the app.
//
//   - `idle`    nothing selected (or the slice was cleared, e.g. a 404).
//   - `loading` a fetch is in flight for `sessionId`.
//   - `loaded`  `staged`/`unstaged` are current for `sessionId` at `rev`.
//   - `error`   the last fetch failed; `error` carries why. Self-heals on the
//               next `session.changes` event (which always refetches in this
//               state, side-stepping the `rev > undefined` trap).
//
// Consumers trust the slice only while `sessionId` equals their own. `rev` is
// monotonic per session; an older `rev` is dropped as an out-of-order reply.
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
// this socket; each focused terminal attaches to its own dedicated `PtySocket`
// (`lib/ptySocket.ts`).

export interface DuxState {
  // The workspace "spine": projects, sessions, and the core-computed sidebar
  // grouping, fetched once and pushed thereafter. A server that does not push,
  // or a frame this client cannot read, falls back to re-fetching on the coarse
  // change events. `null` until the first document lands, with every consumer
  // falling back to empty lists in that window.
  spine: Spine | null
  // The build-static and config-derived document: providers, macros, palette
  // commands, welcome tips, version, UI flags, global env. Re-fetched on a
  // `config.changed` event, and `null` until the first fetch lands, with every
  // consumer falling back to a sensible default in that window.
  bootstrap: Bootstrap | null
  // Set to true synchronously when boot runs (at module load). Tests wait on
  // this as a settled signal.
  booted: boolean
  conn: ConnState
  // Sticky "the events socket is not connected" flag behind `OfflineOverlay`.
  // Distinct from `conn`, which re-enters "connecting" between drops and would
  // flicker the modal off on every retry: this latches on a drop and clears
  // only on `open`. False during the first boot connect, with nothing lost yet.
  offline: boolean
  selectedTarget: SelectedTarget | null
  // THEATER MODE: the focused pane fills the surface and every piece of dux's
  // own chrome around it leaves. A property of the POSITION, not of the app: it
  // rides the address as a modifier, it is restored per pane from the browser's
  // local storage on selection, and losing input ownership of the pane takes it
  // away (see `noteTheaterOwnershipLost`). Never true with nothing focused.
  theater: boolean
  // The desktop layout theater is temporarily overriding, captured on the way
  // in and put back on the way out. `null` whenever theater is off, and also
  // while theater is on if the page BOOTED into it: nothing was ever on screen
  // to capture, so leaving lands on the stored preferences as they stand.
  theaterLayout: TheaterLayoutSnapshot | null
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
  /// The compose drafts, keyed by target id (an agent tab id or a terminal id).
  ///
  /// They live here rather than in the pane because a Retry bumps
  /// `terminalEpoch` and remounts it, and Retry is what a user reaches for after
  /// a bad network, with an unsent message typed. Keyed rather than single, so
  /// switching agents keeps each pane's draft; an entry is dropped only when its
  /// target leaves the spine.
  composeDrafts: Record<string, string>
  commitTarget: string | null
  commitDraft: string
  deleteTarget: string | null
  // The companion terminal id pending close confirmation, or null. Mirrors the
  // TUI, which ALWAYS confirms terminal deletion (the running process is killed).
  deleteTerminalTarget: string | null
  // The tab pending close confirmation, or null. Closing always confirms.
  // Closing a tab ends it, closing the tab in the session slot hands the slot
  // to the next tab in strip order, and closing the agent's last live tab
  // detaches the agent (which stays in Projects, reopenable).
  closeTabTarget: { sessionId: string; tabId: string } | null
  // The agent pending stop confirmation, or null. Distinct from
  // `closeTabTarget`: stopping ends the agent's first tab's process and leaves
  // the tab and the agent in the list, where closing destroys the tab for good.
  // Raised by the Task Manager's row for an agent's first tab.
  stopAgentTarget: string | null
  // Session ids with a tab-create request in flight, so the strip's "+" disables
  // until it resolves (a double-click can't spawn two tabs). The per-agent tab
  // cap still guards the server; this is the common-case UX guard.
  createTabInFlight: string[]
  // Tab ids the user explicitly started from their dormant card, kept only for
  // the gap between the server accepting that start and the spine reporting
  // what came of it. It lets the pane mount over a tab still reporting no live
  // process, so the card does not sit in front of a launch already on its way.
  // See `markTabStarted`; every other launch needs no entry here.
  startedDormantTabs: string[]
  // The unstaged file pending discard confirmation, or null. The TUI confirms
  // every discard (it's destructive); the web mirrors that.
  discardTarget: DiscardTarget | null
  globalEnvOpen: boolean
  projectSettingsTarget: string | null
  // The agent (session) whose startup-command / project-env editor is open, or
  // null. Both edit the agent's PROJECT (env and startup command are
  // project-scoped in dux; there is no per-agent env), surfaced from the agent
  // menu for quick access (mirroring the TUI's per-agent palette commands). The
  // dialog resolves the owning project from the session id.
  agentStartupCommandTarget: string | null
  agentEnvTarget: string | null
  // The entity whose startup-command log viewer is open, or null. The log list
  // and the displayed file are fetched over REST into the fields below.
  //
  // `startupLogsTarget` is a session id in "agent" scope and a project id in
  // "project" scope, matching `dux_core::startup::StartupCommandLogScope`. It
  // is a separate field rather than a tagged target so agent-scope callers keep
  // reading a plain id.
  startupLogsScope: StartupLogsScope
  startupLogsTarget: string | null
  startupLogsEntries: StartupLogEntry[]
  startupLogsSelected: StartupLogContent | null
  startupLogsLoading: boolean
  startupLogsError: string | null
  // The project whose read-only info modal is open, or null (closed). Pure
  // presentation of existing ViewModel data, no wire command, no git read.
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
  /** The standalone-agent folder picker. It shares the `browse*` slice with the
   * add-project picker, which is right: only one folder picker is open at a
   * time, and the browsing itself is the same act. What differs is what
   * happens to the folder you choose. */
  standaloneAgentPickerOpen: boolean
  // Why the picker was opened: "add" (the default) or "init" (the split
  // button's "Initialize a repository…" entry). The intent's ONLY effect is a
  // header hint in the dialog; the primary-action ladder does the real work
  // either way. Cleared on close.
  addProjectIntent: "add" | "init"
  browsePath: string
  browseEntries: DirEntryView[]
  browseLoading: boolean
  // Branch pre-flight for the add-project flow. The reply lands here keyed by
  // `path`, so a stale reply for a previously-selected repo is ignored. A null
  // `warning` beside a resolved `path` means the repo is on its default branch
  // and there is no warning step; null overall means nothing is selected.
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
  // True when the Worktrees dialog was reached by drilling through the project
  // picker, which is when a Back control makes sense: it returns to the project
  // list. Opened straight from a project's own menu there is nothing above it,
  // so no Back is offered and Cancel is the only way out.
  attachWorktreeFromPicker: boolean
  // The worktree pending a delete confirmation, or null. Carries the whole entry
  // (not just its path) so the confirmation can name the branch and say whether
  // there is uncommitted work to lose without re-deriving either.
  deleteWorktreeTarget: { projectId: string; entry: ProjectWorktreeEntryView } | null
  // Managed-worktree counts per project id for the project picker's row labels,
  // or null before the first answer lands. Fetched when the picker opens in the
  // worktree intent, not kept live: it is a label, not a live view.
  projectWorktreeCounts: Record<string, number> | null
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
  // agent; it changes the provider for the next reconnect).
  changeProviderTarget: string | null
  // The session pending a manual pull-request attach (pin), or null. The
  // dialog holds one text field for the raw reference; the draft lives here
  // (like renameDraft) so the input stays fully store-controlled.
  attachPullRequestTarget: string | null
  attachPullRequestDraft: string
  // New-agent dialog state lives in the store so the input is fully
  // store-controlled: the server's generated-name reply fills it through a
  // callback, never a set-state-in-effect. `createAgentGeneratedName` is the
  // last name the server generated, so an uncheck clears the input only while
  // it still equals that name, and is null once the user edits away from it.
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
  // The one first-load dialog (first-run welcome / post-upgrade what's-new).
  // Null when closed. ONE dialog serves both screens; they differ only in text
  // and buttons, so the shape is shared. See `FirstLoadDialogState`.
  firstLoad: FirstLoadDialogState | null
  // Set the moment THIS session dismisses an automatic first-load screen, so a
  // later `config.changed` refetch that still carries the (not yet cleared)
  // pending screen cannot pop it straight back up. Purely a re-open guard: the
  // durable record is the server's SQLite row.
  firstLoadDismissed: boolean
  // The macro-editor dialog. `macrosDialogOpen` gates the modal; `macrosDraft`
  // is the working copy of the whole macro list the user edits before saving
  // (the save is wholesale: `update_macros` replaces the entire `[macros]`
  // map, mirroring the TUI editor). Seeded from `bootstrap.macros` on open so
  // there is no set-state-in-effect. Empty draft when closed.
  macrosDialogOpen: boolean
  macrosDraft: MacroView[]
  // Which screen the mobile shell is showing. DERIVED from the route, never kept
  // independently of it: no focused target is home, a focused target is the
  // terminal screen, and the changes screen is a `/changes` suffix on the
  // target's hash. Desktop maintains it the same way and simply ignores it.
  mobileScreen: MobileScreen
  // Set when the URL names an agent this workspace does not have, which happens
  // on a stale bookmark and on pressing Back onto an agent that has since been
  // deleted. The surfaces render `AgentNotFound` for it rather than quietly
  // pretending the link said nothing. Null whenever the route resolves.
  routeNotFound: RouteNotFound | null
  // Optimistic overlay for the flat model's GLOBAL agent order: the complete list
  // of session ids in the just-dragged order, cleared once the spine confirms it.
  pendingAgentOrder: string[] | null
  // Optimistic overlay for the flat Terminals section's GLOBAL order: the complete
  // list of terminal ids (any owner) in the just-dragged order, cleared once the
  // spine confirms it. Mirrors `pendingAgentOrder` exactly (see `reorderTerminals`).
  pendingTerminalOrder: string[] | null
  // Optimistic overlay for a session's slot pointer, keyed by session id: the
  // tab a slot-tab close just promoted, and the tab that close destroyed. The
  // DELETE's answer is the only thing that knows this until the next spine, and
  // the difference is user-visible, so it is held rather than waited on.
  //
  // Retired by the closed tab, not the promoted one: see
  // `reconcilePendingSlotTab`.
  pendingSlotTab: Record<string, PendingSlotTab>
  // While an agent-create this client started is in flight: the session ids
  // that existed at submit, plus where the new agent will land. Creation has no
  // per-client reply, so the new agent is recognized as the id matching `scope`
  // that was not in `knownIds`. Only the arming client reacts, so nobody else
  // is yanked off what they are viewing. Null when nothing awaits focus.
  // `armedAt` (epoch ms) bounds the token by `CREATE_FOCUS_TTL_MS`: a create
  // that never lands would otherwise mis-focus the next matching session.
  pendingCreateFocus: {
    knownIds: string[]
    scope: CreateFocusScope
    armedAt: number
  } | null
  // Explicit project expand/collapse choices, keyed by project id. A project not
  // present here falls back to its default (open when it has agents). The sidebar
  // reads this so a collapse survives re-renders, and creating an agent under a
  // collapsed project can force it open (see `focusNewlyCreatedSession`).
  projectOpen: Record<string, boolean>
  // The flat agent list's display sort, persisted server-side in
  // `config.ui.agent_sort` so it survives restarts and every client agrees.
  // This field is the optimistic override (null follows config), reconciled by
  // `applyBootstrap`; the effective mode is
  // `agentSort ?? bootstrap.agent_sort ?? "active"`. A drag flips it to
  // "manual" so the dropped order sticks.
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
  // worktree). The launcher corner's ⋯ menu sets this; a bare open defaults to
  // "new".
  newAgentPickerIntent: "new" | "from_pr" | "from_worktree"
  // When set, the picker lists ONLY these project ids. Used when a pull-request
  // reference matched several projects (one repository checked out twice):
  // showing every project there would bury the two that actually have it.
  newAgentPickerOnlyIds: string[] | null
  // A pull-request reference the user has already typed, held across a trip
  // through the picker so the project they choose completes it rather than
  // reopening an empty field.
  pendingPrReference: string | null
  // A resolve is in flight, so the dialog's Create button shows it is working
  // and cannot be pressed twice.
  createAgentPrResolving: boolean
  // A field-level refusal shown under the reference input, or null. Used for
  // the refusals dux can make WITHOUT asking the server: today that is a bare
  // number with no project chosen, which names no repository, so there is
  // nothing for a resolve to look for.
  createAgentPrError: string | null
  // The generation of the ONE resolve this dialog is waiting for, or null when
  // it is waiting for none.
  //
  // A resolve is a git call per project, so it can still be out after the user
  // cancels, retargets or submits a different reference. A reply acts only when
  // its generation is still current: checking merely that some pull-request
  // dialog is open would create an agent from a reference already replaced.
  createAgentPrRequestId: number | null
  // Whether the desktop sidebar is expanded; false is the icon rail. It lives
  // here rather than in `SidebarProvider` because theater has to hold it still:
  // with the panel unmounted, the primitive's keyboard toggle would flip the
  // state behind the mode's back. The provider is controlled from here, so the
  // store owns the persistence too (`persistSidebarOpen`).
  sidebarOpen: boolean
  sidebarWidth: string
  // Optimistic override for the Changes pane's visibility (desktop). `null`
  // follows the persisted config (`bootstrap.show_changes_pane`); the palette and
  // the Changes actions menu set an explicit bool for instant feedback. The
  // toggle persists to config via the server; this clears once the broadcast
  // confirms (or on command error / disconnect, which roll it back).
  changesPaneOverride: boolean | null
  // The Changes panel's current width as a percentage of the desktop panel
  // group (0..100), lifted out by its onLayoutChange so the header above can
  // mirror it as a right-hand spacer. Mirroring the percentage rather than
  // pixels is what keeps that alignment right at any zoom or window size.
  //
  // Runtime-only and not persisted: this reports the split, never decides it.
  changesPanePercent: number
  // Optimistic override for the touch terminal-keys bar. `null` follows the
  // persisted config (`bootstrap.mobile_accessory_bar`); the input ⋯ menu's
  // quick toggle sets an explicit bool for instant feedback. Reconciled
  // against the next bootstrap exactly like `changesPaneOverride` above, and
  // rolled back (with a toast) when the settings PATCH fails.
  mobileAccessoryBarOverride: boolean | null
  // Per-PTY ownership verdicts from mounted TerminalPanes, keyed by pty id.
  // The pane is the freshest source, learning handovers from `pty.owner` ahead
  // of the next spine refetch, so it overrides the spine in both directions.
  // "mine" is the pane's live belief, not a server receipt: it starts as the
  // pane's optimistic foreground guess. An entry lives only as long as the
  // pane's socket; without one, `AgentTabView.input_owner` takes over.
  ptyOwnership: Record<string, "mine" | "elsewhere">
  // This client's own live PTY-socket connection ids, one per mounted pane with
  // an open socket. A spine tab whose `input_owner` is not in this set is owned
  // by some other connection. Deliberately the PTY-socket id space, not the
  // events-socket `X-Connection-Id`: ownership is recorded per PTY socket
  // server-side and the `pty.owner` frames already speak this id space.
  ownPtyConnIds: Record<string, true>
  // The session whose code-editor overlay is open, the file to auto-open on
  // launch (null = none preselected), and the view it opens in: "file" (editable
  // Monaco buffer) or "diff" (read-only Monaco DiffEditor, HEAD vs working copy).
  // The editor always operates on the SELECTED session, so opening it selects
  // that session first and reuses the existing changed-files broadcast for its
  // file list. Null = overlay closed.
  editorTarget: {
    root: EditorRoot
    initialPath: string | null
    initialMode: EditorViewMode
  } | null
  // The editor's live position and the source of the URL's editor suffix: whose
  // editor is open, and the active tab's mode and path (a null path is an
  // editor open with no file). Distinct from `editorTarget`, a one-shot mount
  // seed: this tracks every active-tab change so `currentRoute()` can serialize
  // where the editor actually is. Null while the editor is closed.
  editorRoute: {
    root: EditorRoot
    mode: EditorViewMode
    path: string | null
  } | null
  // True while this tab is the standalone editor surface: `App()` renders
  // `StandaloneEditorShell` and `EditorOverlay` stands down, so the two can
  // never both mount an `EditorBody` over the same files. Seeded from the boot
  // address and kept in line with the URL by `applyUrlRoute`, which is what
  // makes the standalone header's plain anchor back into the app work.
  standaloneEditor: boolean
  // Per-ROOT editor tab metadata (pure client state; the heavy Monaco buffers
  // live in the `EditorBody` component, keyed by tab id). Keyed by `rootKey`,
  // not by a bare id: agent ids and terminal ids come from different counters,
  // so only the namespaced key keeps two editors apart. A root absent here has
  // no tabs (not yet opened, or cleared when its target vanished).
  editorTabs: Record<string, EditorTabsState>
  // The tab pending the dirty-close confirmation (destructive-confirm
  // pattern), or null. Closing a NON-dirty tab skips this and closes directly.
  editorCloseTabTarget: { root: EditorRoot; tabId: string } | null
  // The root of a STANDALONE editor tab whose target vanished while a buffer
  // was dirty, or null. Set instead of tearing the editor down, because the
  // root that could have saved that text is exactly what is gone: the tab
  // stays up so the text can be read and copied out, and the confirm behind
  // this is what discards it. Every other vanish leaves silently but says so
  // (see `endVanishedStandaloneEditor`).
  editorTargetGone: EditorRoot | null
  // Changed-files state for the selected session (see `ChangesSlice`). The single
  // source for changed-files data: replaces the global `viewModel.changed_files`
  // broadcast, which a second client could clobber.
  changes: ChangesSlice
}

// Which view the code editor opens in (and toggles between): the editable Monaco
// buffer, or the read-only Monaco diff (HEAD vs working copy). Opening a changed
// file defaults to "diff"; the file tree / edit actions default to "file".
export type EditorViewMode = "file" | "diff"

// Is there a browser around this module at all? False only for a build-time
// render outside one: `website/src/figure/` imports this module under plain
// Node, where `localStorage`, `location` and `window` do not exist. Under jsdom
// and in a browser it is true and nothing below changes.
const hasBrowser = typeof window !== "undefined"

// The expanded sidebar width is drag-resizable and persisted across reloads.
// 18rem gives agent names room beside the PR and status badges; a persisted
// width wins over it.
const SIDEBAR_WIDTH_KEY = DIVIDER_STORAGE_KEYS.sidebarWidth

// The Changes panel's mount-time size, in percent. Shared by the panel's own
// `defaultSize` and the store's initial `changesPanePercent`, so the header's
// spacer is already the right width on the very first frame, before any layout
// callback has fired.
export const CHANGES_PANE_DEFAULT_PERCENT = 26
// The panel's own `minSize`, and the floor below which a released split is not
// worth remembering: a pane restored under this would come back unusable.
export const CHANGES_PANE_MIN_PERCENT = 14

// The terminal panel's own floor, and therefore the widest the Changes pane may
// ever be remembered as: a stored split that squeezed its neighbour under this
// could not be applied and would be silently clamped on every load.
export const TERMINAL_PANE_MIN_PERCENT = 30
export const CHANGES_PANE_MAX_PERCENT = 100 - TERMINAL_PANE_MIN_PERCENT

// What the Changes split mounts at: the width the user last released the
// divider on, or the default.
//
// Read live, never captured at module load. The panel unmounts when the pane is
// hidden and comes back at its `defaultSize`, so a constant frozen at page load
// would overwrite whatever the user has dragged to since.
export function changesPaneMountPercent(): number {
  if (!hasBrowser) return CHANGES_PANE_DEFAULT_PERCENT
  return readStoredPanePercent(
    DIVIDER_STORAGE_KEYS.changesPanePercent,
    CHANGES_PANE_DEFAULT_PERCENT,
    CHANGES_PANE_MIN_PERCENT,
    CHANGES_PANE_MAX_PERCENT,
  )
}

// The sidebar's expanded/collapsed preference, at the cookie name and lifetime
// the shadcn primitive uses. Written on every toggle and read at boot, so a
// sidebar left collapsed comes back collapsed, and so a page that booted
// straight into theater has something to land on when it leaves.
const SIDEBAR_OPEN_COOKIE = "sidebar_state"
const SIDEBAR_OPEN_COOKIE_MAX_AGE = 60 * 60 * 24 * 7
export const SIDEBAR_DEFAULT_OPEN = true

function loadSidebarOpen(): boolean {
  // Same two hazards as the write below, plus a third: anything at all can be
  // in a cookie jar, so only the two values this ever writes are believed.
  try {
    if (typeof document === "undefined") return SIDEBAR_DEFAULT_OPEN
    const jar = document.cookie ?? ""
    for (const part of jar.split(";")) {
      const [name, ...rest] = part.split("=")
      if (name.trim() !== SIDEBAR_OPEN_COOKIE) continue
      const value = rest.join("=").trim()
      if (value === "true") return true
      if (value === "false") return false
      return SIDEBAR_DEFAULT_OPEN
    }
  } catch {
    // Cookies refused, so the sidebar opens on its default.
  }
  return SIDEBAR_DEFAULT_OPEN
}

function persistSidebarOpen(open: boolean): void {
  // No `document` under the website's Node-side figure renderer, and a browser
  // with cookies blocked throws on the write. Losing the memory is the whole
  // cost either way.
  try {
    if (typeof document === "undefined") return
    document.cookie = `${SIDEBAR_OPEN_COOKIE}=${open}; path=/; max-age=${SIDEBAR_OPEN_COOKIE_MAX_AGE}`
  } catch {
    // Refused. The live state still works for the life of the page.
  }
}

function loadSidebarWidth(): string {
  if (!hasBrowser) return DEFAULT_SIDEBAR_WIDTH
  return readStoredText(SIDEBAR_WIDTH_KEY) || DEFAULT_SIDEBAR_WIDTH
}

// The width the page loaded with, which is what a double-click on the sidebar's
// edge restores. The Changes divider's double-click restores its own mount
// size, so this is the same promise on the other side.
export const SIDEBAR_INITIAL_WIDTH = loadSidebarWidth()

// One-time cleanup: the diff line-number toggle (and its persisted preference)
// went away when the web diff moved to Monaco, which manages its own gutters.
// Drop the orphaned key so it can't linger or be misread by a future feature.
if (hasBrowser) localStorage.removeItem("dux:show-diff-line-numbers")

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

// Whether this tab booted on the standalone editor's whole-tab address. The
// shell choice must settle at module init, or the ordinary shell flashes and
// opens its sockets first, so this calls `parseStandaloneEditorRoute`, which is
// hoisted and does not read the route constants below. Its strictness is the
// point: a malformed tail boots the normal shell and takes the ordinary
// route-correction path rather than marooning the tab on an empty one.
function bootIsStandaloneEditor(): boolean {
  if (typeof location === "undefined") return false
  return parseStandaloneEditorRoute(location.hash ?? "") !== null
}

// Whether this tab booted on a theater address, so the first painted layout is
// already the theater one. The flag otherwise commits with the selection, which
// is too late: the chrome would collapse as a transition under a mounting pane,
// and the terminal would fit, parse its replay, and re-grid at two geometries.
//
// Deliberately looser than `parseRoute`, which cannot run at module init
// because it reads route constants declared below. A hand-made address that
// gets past this costs a moment of hidden chrome and nothing else, since route
// resolution commits the honest value for the position it lands on.
function bootIsTheater(): boolean {
  if (typeof location === "undefined") return false
  const { hash, theater } = splitTheaterHash(location.hash ?? "")
  return theater && hash !== "" && hash !== "#"
}

let state: DuxState = {
  spine: null,
  bootstrap: null,
  booted: false,
  conn: "connecting",
  offline: false,
  selectedTarget: null,
  theater: bootIsTheater(),
  selectedSessionId: null,
  terminalEpoch: 0,
  composeDrafts: {},
  commitTarget: null,
  commitDraft: "",
  deleteTarget: null,
  deleteTerminalTarget: null,
  closeTabTarget: null,
  stopAgentTarget: null,
  createTabInFlight: [],
  startedDormantTabs: [],
  discardTarget: null,
  globalEnvOpen: false,
  projectSettingsTarget: null,
  agentStartupCommandTarget: null,
  agentEnvTarget: null,
  startupLogsScope: "agent",
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
  standaloneAgentPickerOpen: false,
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
  attachWorktreeFromPicker: false,
  deleteWorktreeTarget: null,
  projectWorktreeCounts: null,
  createAgentTarget: null,
  renameTarget: null,
  renameDraft: "",
  changeProviderTarget: null,
  attachPullRequestTarget: null,
  attachPullRequestDraft: "",
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
  firstLoad: null,
  firstLoadDismissed: false,
  macrosDialogOpen: false,
  macrosDraft: [],
  mobileScreen: "home",
  routeNotFound: null,
  pendingAgentOrder: null,
  pendingTerminalOrder: null,
  pendingSlotTab: {},
  pendingCreateFocus: null,
  projectOpen: {},
  agentSort: null,
  agentSearch: "",
  newAgentPickerOpen: false,
  newAgentPickerIntent: "new",
  newAgentPickerOnlyIds: null,
  pendingPrReference: null,
  createAgentPrResolving: false,
  createAgentPrError: null,
  createAgentPrRequestId: null,
  sidebarOpen: loadSidebarOpen(),
  sidebarWidth: SIDEBAR_INITIAL_WIDTH,
  theaterLayout: null,
  changesPaneOverride: null,
  changesPanePercent: changesPaneMountPercent(),
  mobileAccessoryBarOverride: null,
  ptyOwnership: {},
  ownPtyConnIds: {},
  editorTarget: null,
  editorRoute: null,
  standaloneEditor: bootIsStandaloneEditor(),
  editorTabs: {},
  editorCloseTabTarget: null,
  editorTargetGone: null,
  changes: emptyChanges(),
}

const listeners = new Set<() => void>()

function emit(): void {
  for (const listener of listeners) listener()
}

// The screen and the not-found flag are DERIVED from the focused target, never
// tracked beside it, so a patch that changes the target settles both in the same
// commit: clearing the target lands on home, focusing one lands on the terminal
// screen, and either way the route now resolves so nothing is missing. A patch
// that states `mobileScreen` or `routeNotFound` outright wins, which is how the
// changes screen opens and how a URL naming a deleted agent is recorded.
function setState(patch: Partial<DuxState>): void {
  const next = { ...state, ...patch }
  if ("selectedTarget" in patch) {
    if (!("mobileScreen" in patch)) {
      next.mobileScreen = patch.selectedTarget ? "terminal" : "home"
    }
    if (!("routeNotFound" in patch)) next.routeNotFound = null
  }
  state = next
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

// Seed the store directly, for a render that has no server to fetch from: the
// marketing site's static figure runs at build time under plain Node, where
// `boot()` is skipped. Deliberately a thin pass-through to `setState`, so the
// seeded state settles through the same derivation a live patch does. It opens
// no socket and fires no fetch.
export function seedStaticSnapshot(patch: Partial<DuxState>): void {
  setState(patch)
}

// Derive the WebSocket scheme from the page protocol so an HTTPS deployment uses
// `wss://` (a hardcoded `ws://` would be blocked as mixed content under HTTPS).
const wsScheme = hasBrowser && location.protocol === "https:" ? "wss:" : "ws:"

// The host half of the same URL. Off-browser there is no page to derive it from
// and no socket will ever be opened (`boot()` is skipped), so the placeholder
// only has to keep the `EventsSocket` constructor happy.
const wsHost = hasBrowser ? location.host : "localhost"

// The single JSON socket for the whole app (`/ws/events`), separate from the
// per-PTY byte sockets (`lib/ptySocket.ts`). It carries resource-change events
// (changed files, spine, config) and the control frames (`connected` id,
// `status`/`status_cleared` toasts), and owns the connection-state UX (the
// status-bar indicator). Exported so tests can drive
// its callbacks / inspect its interest set; connected on boot.
export const eventsSocket = new EventsSocket(
  `${wsScheme}//${wsHost}/ws/events`,
)

// App-wide coarse topics, subscribed once at module load: added to the interest
// set immediately (sent on the first open, re-sent on every reconnect), so the
// bootstrap and spine refetches ride one channel with no per-feature subscribe
// site.
eventsSocket.subscribe(["sessions", "projects", "config"])

// PAGE LIFECYCLE for the app-wide socket. `pagehide` closes it (a page held in
// the bfcache with an open socket is evicted anyway, and the server keeps a
// phantom connection until its next send fails), `pageshow` and Chromium's
// `resume` reopen it plain, and `freeze` parks it. It lives as long as the page
// does, so the registration is never retired.
registerPageLifecycle(eventsSocket)

function handleStatusEvent(event: EventsServerMessage): void {
  if (event.tone === "error") setState({ ...clearPendingClientIntent() })
  if (!statusToastAllowed(event.scope, state.standaloneEditor)) return
  showStatusToast(
    event.key,
    event.tone ?? "info",
    event.message ?? "",
    event.sticky ?? false,
  )
}

function applyPushedWorkspace(event: EventsServerMessage): void {
  try {
    applyWorkspace(
      normalizeWorkspace(event.workspace as RawWorkspace),
      loadWorkspaceSeq,
    )
    serverPushesWorkspace = true
  } catch (error) {
    console.warn("[dux] pushed workspace document rejected", error)
  }
}

function handleSessionChanges(event: EventsServerMessage): void {
  const id = event.id
  if (id === undefined || id !== state.selectedSessionId) return
  const rev = event.rev
  if (
    state.changes.phase === "error" ||
    rev === undefined ||
    rev >= state.changes.rev
  ) {
    loadChanges(id)
  }
}

function routeEvent(event: EventsServerMessage): void {
  switch (event.event) {
    case "connected":
      if (typeof event.id === "string") setConnectionId(event.id)
      return
    case "status":
      handleStatusEvent(event)
      return
    case "status_cleared":
      dismissNotification(event.key ?? ANON_TOAST_ID)
      return
    case "config.changed":
      loadBootstrap()
      return
    case "workspace":
      applyPushedWorkspace(event)
      return
    case "projects.changed":
    case "sessions.changed":
      if (!serverPushesWorkspace) loadWorkspace()
      return
    case "pty.owner":
      if (typeof event.id === "string") {
        notifyPtyOwner(event.id, event.owner, event.epoch, event.device)
      }
      return
    case "session.changes":
      handleSessionChanges(event)
      return
  }
}

eventsSocket.onEvent = routeEvent

// `boot()` starts the first bootstrap and spine load alongside
// `eventsSocket.connect()`, so the first `onOpen` consumes this flag rather
// than duplicating that load. Every reconnect open leaves it false and always
// refetches, deliberately not keyed off `state.bootstrap !== null`: a failed
// first fetch leaves that null forever, and the app would never recover.
let skipNextEventsOnOpenLoad = false

// Which run of which build of dux this tab loaded against, read once at boot
// (see `buildApi.ts`). `null` until that read lands, or forever if it failed,
// and a null baseline never forces a reload: unknown is not "changed".
let serverIdentityBaseline: ServerIdentity | null = null

// How long before an identity read that answered UNKNOWN is asked again. A blip
// on the one endpoint that decides whether this tab is running against the
// server that served it is not a reason to stop asking, and re-asking is one
// small GET.
const IDENTITY_REPROBE_MS = 5000
let identityReprobeTimer: ReturnType<typeof setTimeout> | null = null

// Ask again, once, after the gap. Idempotent: several unknown answers in flight
// at once still schedule a single re-ask.
function scheduleIdentityReprobe(again: () => void): void {
  if (!hasBrowser) return
  if (identityReprobeTimer !== null) return
  identityReprobeTimer = setTimeout(() => {
    identityReprobeTimer = null
    again()
  }, IDENTITY_REPROBE_MS)
}

// Learn the run-identity baseline the hard reload compares against. A tab that
// never learns one has that protection off for its whole life, so an unknown
// answer is retried rather than accepted.
//
// Completing this read, with an answer or without one, is also what opens the
// PTY retry gate: it is a round trip to the server this tab loaded from, so the
// run cannot have moved unnoticed. A failed read validates too, since holding
// every terminal shut over one unreachable endpoint is the wrong failure.
async function loadServerIdentityBaseline(): Promise<void> {
  serverIdentityBaseline = await fetchServerIdentity()
  noteServerValidated()
  if (serverIdentityBaseline === null) {
    scheduleIdentityReprobe(() => {
      void loadServerIdentityBaseline()
    })
  }
}

// Is this the server that served this tab? A reconnect is the only moment dux
// can have been restarted underneath it, so the answer picks between an
// in-place refetch and a hard reload with no prompt.
//
// It runs alongside that refetch rather than gating it: blocking recovery on a
// network round-trip would strand the app whenever the probe hung, and a reload
// discards whatever the early refetch produced.
async function reloadIfServerChanged(): Promise<void> {
  const current = await fetchServerIdentity()
  if (serverChanged(serverIdentityBaseline, current)) {
    // Published BEFORE the reload, because the reload is not instantaneous and
    // the memories keyed to the old run's counters (a pane's ghost connection
    // ids, the applied replay generation) can be read in the meantime. See
    // `serverRun.ts`.
    noteServerRunProbe("changed")
    reloadPage()
    return
  }
  noteServerRunProbe(current === null ? "unknown" : "same")
  // The check has RESOLVED and the run has not moved, which is the moment a PTY
  // socket may attach again. `conn === "open"` is true a whole round trip before
  // this, and attaching an agent's pty launches its provider, so the gate reads
  // this signal and never that one. See `serverValidated.ts`.
  noteServerValidated()
  // An UNKNOWN answer opens the gate (unknown is not evidence of a change) but it
  // is not evidence of SAMENESS either, so it is never latched. Without the
  // re-ask, one transient failure on a reconnect after a restart left the tab
  // running old code against a new run indefinitely, which is the exact thing
  // this check exists to prevent.
  if (current === null) {
    scheduleIdentityReprobe(() => {
      void reloadIfServerChanged()
    })
  }
}

// After a (re)connect the socket has re-sent the whole interest set; re-fetch so
// anything missed while disconnected is recovered (an event that arrived during
// the outage is gone otherwise). The `config` coarse topic is always subscribed,
// so refetch the bootstrap document too; a `config.changed` missed during the
// outage would otherwise leave stale providers/macros/UI flags until the next
// config edit. The selected session's changes are also recovered when one is set.
eventsSocket.onOpen = () => {
  if (skipNextEventsOnOpenLoad) {
    // First open after a boot/login load: skip the duplicate fetch this once.
    skipNextEventsOnOpenLoad = false
  } else {
    // Re-fetch both so anything missed during the outage recovers. Concurrent
    // loads are safe: spine and bootstrap loads are both seq-guarded.
    //
    // The deep-linked route is captured before `loadWorkspace` so a transient
    // exit-eject during the reconnect cannot wipe the hash first.
    //
    // The identity probe comes first: dux may have been restarted during the
    // outage, and refetching state into old code is the wrong recovery.
    void reloadIfServerChanged()
    armReconnectDeepLink()
    loadBootstrap()
    loadWorkspace()
    // The server's ownership epoch counter restarts at zero if the server itself
    // restarted during the outage; clear our per-pty high-water marks so a fresh
    // post-restart `pty.owner` is not wrongly ignored as stale. A reconnect is the
    // only path a restarted server's epochs reach us, and there is no `pty.owner`
    // replay, so this can never drop a still-relevant in-flight handover.
    resetPtyOwnerEpochs()
  }
  // Both branches: the socket that is about to deliver pushed documents may
  // belong to a different run of the server than the last one did, and its
  // revisions start again from 1. Forget what was applied so the first push of
  // this generation lands. (On the boot open this only risks re-applying the
  // document the boot fetch just applied, which is idempotent.)
  resetAppliedWorkspaceRev()
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
// guarded apply and error handlers, so a failed fetch never surfaces as an
// unhandled rejection. The returned promise never rejects; it exists for a
// caller that must report on the result, such as the forced refresh.
function loadChanges(sessionId: string): Promise<void> {
  return fetchChanges(sessionId)
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

// Apply a failed fetch. A 404 means the session is gone; clear the slice (the
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

// Re-fetch the selected session's changes; a no-op when nothing is selected.
//
// This only re-reads, and the server answers from its per-session cache, so it
// is right after an error or an event and wrong for a user-driven "refresh
// now", which would hand back the same answer. Use `forceRefreshChanges`.
export function refreshChanges(): void {
  const id = state.selectedSessionId
  if (id === null) return
  setState({ changes: loadingChanges(id) })
  loadChanges(id)
}

// Force the server to ask git again, then re-read. Rejects when the forcing
// POST fails so the caller can report it, but the re-read runs either way: a
// pane stuck in `loading` after a failed force is worse than a stale one.
//
// The success toast is raised here rather than by the engine's status stream,
// which this route does not emit into because it mutates nothing, and it is
// skipped when the re-read did not land: counts from a slice this refresh did
// not fill would be made up.
export async function forceRefreshChanges(): Promise<void> {
  const id = state.selectedSessionId
  if (id === null) return
  setState({ changes: loadingChanges(id) })
  try {
    await git.refreshChanges(id)
  } finally {
    await loadChanges(id)
  }
  const slice = state.changes
  if (slice.sessionId !== id || slice.phase !== "loaded") return
  notifySuccess(
    `Changed files refreshed: ${slice.staged.length} staged, ` +
      `${slice.unstaged.length} unstaged.`
  )
}

// Monotonic sequence for bootstrap loads, mirroring `loadWorkspaceSeq`. Rapid
// `config.changed` events fire concurrent `fetchBootstrap()`s and nothing else
// orders the replies, so without this a client goes on applying config the
// server has already replaced until an edit happens to come back in order.
let loadBootstrapSeq = 0

// Fetch the bootstrap document and fold it into state. Errors are swallowed: on
// first boot the slice stays `null` (consumers fall back to defaults) and a
// later `config.changed` event or a reconnect retries; on a refetch the last
// good bootstrap is kept rather than blanking the UI. Never surfaces as an
// unhandled rejection.
function loadBootstrap(): void {
  const seq = ++loadBootstrapSeq
  fetchBootstrap()
    .then((b) => {
      // Discard this (now-stale) result once a newer load has started. Same rule
      // as `applyWorkspace`: the newest request wins, whatever order the replies
      // arrive in.
      if (seq < loadBootstrapSeq) return
      applyBootstrap(b)
    })
    .catch((err) => {
      // Keep the previous bootstrap (null on first boot); a config.changed event
      // or reconnect will retry. Warn so a persistently-failing fetch (e.g. a
      // first boot that stays empty) is visible in the console rather than silent.
      console.warn("[dux] bootstrap fetch failed; will retry on reconnect", err)
    })
}

function reconcileConfirmedOverride<T>(
  override: T | null,
  configured: unknown,
): T | null {
  return override !== null && override === configured ? null : override
}

// Applies fresh config and retires optimistic overrides once the server confirms
// them, leaving config as the shared source of truth.
function applyBootstrap(b: Bootstrap): void {
  // Publish the user's auto-clear window to the one raiser, which reads it on
  // the way past on every notification. Nothing downstream captures it, so
  // nothing downstream can hold a stale copy: a config edit reaches the next
  // notification raised anywhere in the app, including one raised from a mount
  // effect that ran long before the bootstrap document landed.
  setStatusClearSeconds(b.status_clear_seconds)
  // The four connection timings, on the same idiom and for the same reason: the
  // socket, cover and heartbeat callbacks that read them are long-lived, so they
  // read through the module rather than closing over a render's copy.
  publishConnectionTiming(b)
  setState({
    bootstrap: b,
    changesPaneOverride: reconcileConfirmedOverride(
      state.changesPaneOverride,
      b.show_changes_pane,
    ),
    // Same reconcile for the accessory-bar override: drop the optimistic
    // override once the refetched config confirms it, so config becomes the
    // single source of truth across every connected client.
    mobileAccessoryBarOverride: reconcileConfirmedOverride(
      state.mobileAccessoryBarOverride,
      b.mobile_accessory_bar ?? true,
    ),
    agentSort: reconcileConfirmedOverride(state.agentSort, b.agent_sort),
  })
  // Reflect the configured instance name and favicon in the browser tab, plus the
  // live attention count/dot. Guarded inside `refreshAttentionChrome` because the
  // store also runs under the Node test environment, where `document` is absent
  // unless a test stubs it. Runs on first load and on every config.changed
  // refetch, so a live rename updates the tab (and re-applies the current dot)
  // without a reload.
  refreshAttentionChrome()
  // The server decided this launch's first-load screen once, at startup, and
  // holds it in memory, so it arrives on the FIRST bootstrap of a client that
  // connects at any point, and on the `config.changed`-driven refetch the server
  // emits the moment the decision resolves (which is how a browser already open
  // during a slow release-notes fetch still gets the screen). Guarded inside.
  offerAutomaticFirstLoad(b.pending_first_load ?? null)
}

// Repaint the tab title's `(N) ` prefix and the favicon's attention dot from
// the agents flagged in the current spine. Call it whenever the count, the base
// title or favicon, or the surface bit could have changed;
// `applyAttentionFavicon` composes at most once per state, so it is cheap on
// every spine apply. Self-guards on the DOM.
function refreshAttentionChrome(): void {
  if (typeof document === "undefined") return
  const count = attentionCountForSurface(
    state.spine?.sessions ?? [],
    state.standaloneEditor,
  )
  const base = pageTitle(
    resolveInstanceTitle(state.bootstrap?.title),
    state.standaloneEditor,
  )
  document.title = formatTabTitle(base, count)
  applyAttentionFavicon(state.bootstrap?.favicon, count > 0)
}

// Monotonic sequence for spine loads: rapid change events fire concurrent
// `fetchWorkspace()`s, and without this an older reply resolving last would
// overwrite a newer spine. Each `loadWorkspace` captures the seq it bumped to
// and `applyWorkspace` discards a result once a newer load has started.
//
// It orders fetch against fetch and nothing else. Fetch against push is ordered
// by the server's `rev` below, since only the server knows which is later.
let loadWorkspaceSeq = 0

// Whether this server pushes the workspace document. Set by the first pushed
// frame this client could actually process, never unset: until then (an older
// server, or a server whose frames dux cannot read) the coarse pings keep
// driving a refetch, which is the whole fallback.
let serverPushesWorkspace = false

// The highest workspace revision applied, or `null` for "none yet". A document
// carrying a revision at or below this one describes a state already applied and
// is discarded, whichever way it arrived.
//
// Revisions are scoped to one run of the server and one socket generation: a
// restart mints them from 1 again, so a client holding the previous run's
// high-water mark would discard every push and freeze the sidebar.
// `eventsSocket.onOpen` therefore clears this. The run-id reload is not the
// guard here: it answers whether the code changed, and it never reloads on an
// unanswered probe.
let appliedWorkspaceRev: number | null = null

// Forget the applied revision. Called on every events-socket open, including
// the first: the boot fetch may already have applied a revision, and re-applying
// the same document is merely redundant, while discarding a fresh one would be
// wrong.
function resetAppliedWorkspaceRev(): void {
  appliedWorkspaceRev = null
}

// Fetch the workspace spine and fold it into state. Errors are swallowed: on
// first boot the slice stays `null` (consumers fall back to empty lists) and a
// later `projects.changed`/`sessions.changed` event or a reconnect retries; on a
// refetch the last good spine is kept rather than blanking the sidebar. Never
// surfaces as an unhandled rejection.
function loadWorkspace(): void {
  const seq = ++loadWorkspaceSeq
  fetchWorkspace().then(
    (s) => {
      // Applying is not fetching: folding a throw from the apply into the
      // rejection handler below would report a good fetch as a failed one. A
      // backstop for the apply as a whole, not the history-write guard, which
      // lives in `syncUrl` because most URL writes are user clicks.
      try {
        applyWorkspace(s, seq)
      } catch (err) {
        console.warn("[dux] spine apply failed", err)
      }
    },
    (err) => {
      // Keep the previous spine (null on first boot); an event or reconnect will
      // retry. Warn so a persistently-failing fetch (e.g. a first boot that stays
      // empty) is visible in the console rather than silent.
      console.warn("[dux] spine fetch failed; will retry on reconnect", err)
    },
  )
}

// Apply a freshly fetched spine, the single place sidebar data lands. Order
// matters: set the slice with reconciled overlays first, then focus (which only
// selects a session present in the spine, so the prune leaves it alone), then
// prune.
//
// `seq` is the `loadWorkspaceSeq` the originating `loadWorkspace` captured; a
// stale result is discarded so a slow reply cannot overwrite a fresher spine or
// re-run focus and prune against outdated data.
function applyWorkspace(rawSpine: Spine, seq: number): void {
  if (seq < loadWorkspaceSeq) return
  // The server's ordering, applied to both delivery paths from one place. A
  // document with no revision at all comes from a server that predates the
  // push; it cannot be ordered, so it is applied and leaves the high-water mark
  // alone rather than pinning it to a guess.
  const rev = rawSpine.rev
  if (rev !== undefined) {
    if (appliedWorkspaceRev !== null && rev <= appliedWorkspaceRev) return
    // The high-water mark is recorded at the END of this function, not here:
    // the reconciliation steps below can throw (the callers' try/catch says
    // so), and a rev recorded before a failed apply would make the fallback
    // refetch of this same document read as stale and be discarded.
  }
  // `tabs` is normalized to an array at the fetch boundary (`fetchWorkspace`), so an
  // older server that omits the field degrades to an empty strip rather than
  // throwing on the `session.tabs` derefs downstream.
  const spine = rawSpine
  // The outgoing session list, captured before `setState` swaps the spine. A
  // vanished agent picks its replacement from this ordering (see
  // `navigateAfterVanish`), since the new list no longer holds its position.
  const previousSessions = state.spine?.sessions ?? []
  // Retire the "explicitly started" latch for any tab whose launch has been
  // answered, either way: the process is up, or the run is recorded as failed.
  // Dropping it on liveness lets a later exit re-show the card, and dropping it
  // on a recorded failure stops a press from hiding the diagnosis surface
  // forever when the retry fails too. Tabs still waiting keep their latch.
  const answeredTabIds = new Set(
    spine.sessions.flatMap((s) =>
      s.tabs.filter((t) => t.has_live_process || t.last_run_failed).map((t) => t.id),
    ),
  )
  const prunedDormant = state.startedDormantTabs.filter((id) => !answeredTabIds.has(id))
  setState({
    spine,
    startedDormantTabs:
      prunedDormant.length === state.startedDormantTabs.length ? state.startedDormantTabs : prunedDormant,
    // A draft outlives a remount on purpose; it must not outlive its TARGET. An
    // agent tab or terminal that has left the spine has no surface to type into
    // and no way back to one, so its unsent text is unreachable rather than
    // preserved.
    composeDrafts: pruneComposeDrafts(state.composeDrafts, spine),
    pendingAgentOrder: reconcilePendingAgentOrder(spine, state.pendingAgentOrder),
    pendingTerminalOrder: reconcilePendingTerminalOrder(spine, state.pendingTerminalOrder),
    pendingSlotTab: reconcilePendingSlotTab(spine, state.pendingSlotTab),
  })
  // Restore a boot-time deep-link before focus/prune: it selects only a session
  // present in this spine (so prune leaves it alone), and it is a one-shot that
  // self-clears, so it never fights a create-focus or a later refetch.
  restoreDeepLink(spine)
  // Retire a not-found screen the moment this spine proves its URL right again.
  // Its position relative to the focus step below is not load-bearing: a
  // freshly created agent wins the focus either way, because `focusNewlyCreatedSession`
  // running second simply overwrites the retry's selection, and running first
  // clears `routeNotFound` (any patch carrying a target does, see `setState`),
  // which makes the retry return immediately. Reading in URL-then-create order.
  retryRouteNotFound(spine)
  focusNewlyCreatedSession(spine)
  // The open editor's own ending runs FIRST, before the prunes: on the
  // standalone tab the selection prune would clear the surface flag along
  // with the selection and the tab would go blank before anything had said
  // why, and on either surface the editor prune would silently delete the
  // tabs a dirty hold exists to protect.
  if (!endOpenEditorIfRootGone(spine)) {
    pruneSelectionIfGone(spine, previousSessions)
    pruneEditorStateIfGone(spine)
  }
  // Re-restore a reconnect deep-link once its agent is present and back to
  // `active`, undoing a transient exit-eject that fired during the reconnect.
  restoreReconnectDeepLink(spine)
  // The flagged-agent count may have changed with this spine: refresh the
  // browser-tab count prefix and the favicon dot. Backgrounded tabs update too,
  // since spines arrive from server pushes without a visit.
  refreshAttentionChrome()
  // Only now that every step above survived does this revision count as
  // applied; see the comment at the staleness check.
  if (rev !== undefined) appliedWorkspaceRev = rev
}

// Drop editor-tab state for any root whose target has left the spine, and close
// the editor if it pointed at that target. The editor's own out-of-band clear,
// mirroring `pruneSelectionIfGone` for the main selection.
//
// A terminal root is checked against the live terminals: an editor must not
// outlive its target, and a terminal id is never reused, so there is nothing to
// come back for.
function pruneEditorStateIfGone(spine: Spine): void {
  const live = liveEditorRootKeys(spine)
  pruneDeadEditorTabs(live, null)
  const openRoot = state.editorTarget?.root ?? state.editorRoute?.root ?? null
  if (openRoot !== null && !live.has(rootKey(openRoot))) {
    // State only, NO URL write: `pruneSelectionIfGone` (which runs before
    // this in the same `applyWorkspace` pass) is the single URL writer for a
    // vanished session. Its navigation already serializes without the
    // editor suffix, because `currentRoute` drops an editor arm whose
    // session no longer matches the focused target. A second writer here
    // (calling `closeEditor()` here) would push or double-write.
    clearEditorStateSilently()
  }
}

// Every root key this spine can still answer for. The terminal owner here is
// fabricated: only the STANDALONE variant is written, whatever the terminal's
// real owner is, and that is correct only because `rootKey` keys a terminal
// root by its id alone and ignores the owner.
function liveEditorRootKeys(spine: Spine): Set<string> {
  return new Set([
    ...spine.sessions.map((session) => rootKey(agentRoot(session.id))),
    ...spine.terminals.map((terminal) =>
      rootKey({
        kind: "terminal",
        terminalId: terminal.id,
        owner: { kind: "standalone" },
      }),
    ),
  ])
}

// Drop the tabs (and their drafts) of every root the spine no longer has,
// except the one a dirty vanish-hold is protecting, when there is one. The
// hold shields exactly the root whose unsaved text is still on screen; a
// different dead root has nothing on screen and no question pending, so its
// tabs are pruned in the same pass rather than surviving behind the hold and
// keeping the beforeunload guard armed for text nobody can see.
function pruneDeadEditorTabs(live: Set<string>, heldKey: string | null): void {
  for (const key of Object.keys(state.editorTabs)) {
    if (key === heldKey) continue
    if (!live.has(key)) clearEditorTabsForKey(key)
  }
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

// Mirror of `reconcilePendingAgentOrder` for the flat Terminals section. The
// spine carries EVERY terminal (any owner) in one flat collection, and the
// authoritative flat order is that collection sorted by the global `sort_order`
// (which a reorder restamps to the dragged order). The overlay clears once that
// server order matches what we optimistically applied.
function reconcilePendingTerminalOrder(
  spine: Spine,
  pending: string[] | null,
): string[] | null {
  if (!pending) return null
  const serverIds = spine.terminals
    .slice()
    .sort((a, b) => a.sort_order - b.sort_order)
    .map((t) => t.id)
  return ordersMatch(serverIds, pending) ? null : pending
}

// Drop a promoted-slot overlay once the spine has caught up with the close it
// covers, or the session is gone. The overlay only covers the window between
// the close's answer and the spine confirming it.
//
// "Caught up" is the closed tab having left the tab list, not the spine naming
// the promoted tab as the slot: another surface promoting again first would
// make that wait forever, while the close's own disappearance is a fact any
// later spine carries.
function reconcilePendingSlotTab(
  spine: Spine,
  pending: Record<string, PendingSlotTab>,
): Record<string, PendingSlotTab> {
  const entries = Object.entries(pending).filter(([sessionId, promotion]) => {
    const session = spine.sessions.find((s) => s.id === sessionId)
    return (
      session !== undefined &&
      session.tabs.some((t) => t.id === promotion.closedTabId)
    )
  })
  if (entries.length === Object.keys(pending).length) return pending
  return Object.fromEntries(entries)
}

// The slot tab id for an agent this client is acting on. A promotion this
// client just performed wins over the spine, which has not caught up with it
// yet; otherwise resolved from the live spine when the agent is in it, and from
// the id-only rule when it is not (an action fired against a session the spine
// has not caught up with yet).
function slotTabIdFor(sessionId: string): string {
  const session = state.spine?.sessions.find((s) => s.id === sessionId)
  return (
    slotTabIdOf(sessionId, session, state.pendingSlotTab) ??
    slotTabTargetId(sessionId)
  )
}

// Slot-ness when this module holds two ids rather than a session record.
//
// The placeholder spelling counts too: a target built before any spine arrived
// names the slot tab by the session id, which is the URL grammar's way of
// saying "the first tab, whichever it is". Both spellings must answer alike, or
// one position serializes to two hashes. A real tab cannot collide, since a
// session's own id is never handed out as a tab id.
function isSlotTabOf(sessionId: string, tabId: string): boolean {
  return tabId === slotTabIdFor(sessionId) || isSlotTabTarget(sessionId, tabId)
}

// Move the user to a real destination when what they were looking at no longer
// exists in the latest spine. Agents persist after exiting (their session stays,
// marked detached), so they only vanish on deletion; terminals are removed
// outright when their PTY exits. `previous` is the session list from the spine
// before this one, which is what gives the gone agent a position to pick a
// neighbour from.
function pruneSelectionIfGone(spine: Spine, previous: SessionView[]): void {
  const target = state.selectedTarget
  if (!target) return
  if (target.kind === "agent") {
    const session = spine.sessions.find((s) => s.id === target.sessionId)
    // The session must still exist; if an extra tab is focused, it must still be
    // in that session's tab list (an extra tab can be closed by ANOTHER client,
    // whose local retarget-to-session-slot never ran here; this is the shared-workspace
    // heal path). A gone extra tab falls back to the session-slot tab rather than
    // ejecting the user to the welcome screen.
    if (!session) {
      navigateAfterVanish(spine, previous, target.sessionId)
    } else if (
      !isFirstTab(session, target.tabId) &&
      !session.tabs.some((t) => t.id === target.tabId)
    ) {
      // A rewrite, like every other vanish path: the user did not ask to leave
      // the tab, so pushing would leave the dead tab's entry underneath. The
      // `changes` flag is carried across because changed files are
      // session-scoped, so the screen being read survives the tab going away.
      selectSessionRoute(
        target.sessionId,
        "replace",
        state.mobileScreen === "changes",
      )
    }
    return
  }
  // A terminal must still exist under its owner. `ownerHasTerminal` checks both
  // halves at once, the id being present and its owner tag matching the address
  // in hand, so nothing here knows how each owner kind nests.
  const owner = target.owner
  const stillExists = ownerHasTerminal(spine.terminals, owner, target.terminalId)
  if (!stillExists) {
    // A terminal that exited has no "next terminal" worth guessing at, so the
    // destination is one level up: the owning agent, or home when there is
    // none. Like the deep-link path, this rewrites the current entry rather
    // than stepping history.
    //
    // The lossy `ownerSessionId` suffices because "is there an agent above this
    // terminal" is the whole decision.
    const ownerSession = ownerSessionId(owner)
    const fallback =
      ownerSession !== null && spine.sessions.some((s) => s.id === ownerSession)
        ? ownerSession
        : null
    selectSessionRoute(fallback, "replace")
  }
}

// The destination when the focused agent vanishes under the user: the hub on a
// phone, and on a computer the next active agent in the order already on screen
// (`nextActiveSessionId`), or home when every remaining agent is dormant. The
// URL is rewritten rather than pushed, so one Back can land on the screen the
// user is already on. Accepted: it beats being thrown out of the app.
function navigateAfterVanish(
  spine: Spine,
  previous: SessionView[],
  goneSessionId: string,
): void {
  // On a phone the destination is the hub, not the next row: the agents list is
  // a screen of its own there, so a neighbouring agent would fill the display
  // identically and read as a delete that hit the wrong one. On a computer the
  // list stays visible beside the pane, so the next row is truthful.
  //
  // A rewrite either way: the entry pushed on the way in names an agent that no
  // longer exists, so pushing over it leaves Back on a not-found screen.
  if (isMobileViewport()) {
    selectSessionRoute(null, "replace")
    return
  }
  // The overlay first, exactly as `FlatAgentList` does before it partitions and
  // sorts: while a drag is applied but not yet confirmed by the server, the
  // order on screen is the overlay's, so a destination computed from the raw
  // spine would name a row that is not the one below the agent that vanished.
  const pending = state.pendingAgentOrder
  const next = nextActiveSessionId(
    pending ? reorderById(previous, pending) : previous,
    pending ? reorderById(spine.sessions, pending) : spine.sessions,
    goneSessionId,
    agentSortValue(state),
  )
  selectSessionRoute(next, "replace")
}

// How long an armed create-focus token stays live. Above the longest
// server-side create window (`FROM_PR_CREATE_AWAIT_TIMEOUT`, 60s) so a slow
// create still auto-focuses, but bounded so a create that never lands cannot
// keep a stale token armed to grab a later, unrelated session.
const CREATE_FOCUS_TTL_MS = 90_000

// Snapshot the session ids that exist now and arm auto-focus for an agent this
// client is creating. Call it immediately before dispatching the create.
// Re-arming supersedes any earlier create whose agent never arrived. The scope
// must be the one the new agent lands in: a caller that cannot resolve its
// project skips arming rather than passing a placeholder.
function armCreateFocus(scope: CreateFocusScope): void {
  const knownIds = (state.spine?.sessions ?? []).map((s) => s.id)
  setState({
    pendingCreateFocus: { knownIds, scope, armedAt: Date.now() },
  })
}

// Whether a session is the one an armed token is waiting for, by where it
// lives. Exhaustive on the scope so a third way to create an agent has to say
// how it is recognized rather than falling into someone else's arm.
function sessionInCreateScope(
  session: SessionView,
  scope: CreateFocusScope,
): boolean {
  const projectId = workspaceProjectId(session.workspace)
  switch (scope.kind) {
    case "project":
      return projectId === scope.projectId
    // A standalone agent belongs to no project, and that is the whole test: the
    // folder cannot be compared, because the server canonicalizes the path it
    // was handed and may answer with a different string than the one typed.
    case "standalone":
      return projectId === null
    default:
      return assertNever(scope)
  }
}

// Focus the agent this client just created, the instant it shows up: with a
// token armed by `armCreateFocus`, the incoming spine is scanned for a session
// unknown at submit time that matches the armed scope. A cheap no-op when
// nothing is pending, and other clients armed nothing, so focus moves only on
// the client that initiated the create.
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
    (s) => !known.has(s.id) && sessionInCreateScope(s, pending.scope),
  )
  if (!created) return
  // Consume the token before selecting so a later spine can't re-fire.
  setState({ pendingCreateFocus: null })
  // Force the owning project open so the new agent is actually visible: a
  // project the user had collapsed would otherwise hide the row we just
  // selected.
  // A standalone agent has no project group to open; it is a top-level row.
  const createdProjectId = workspaceProjectId(created.workspace)
  if (createdProjectId) setProjectOpen(createdProjectId, true)
  // No latch is needed for the tab the create just started: a brand-new agent's
  // first tab has no recorded failure behind it, so selecting it shows the pane
  // and not the card, whether or not the PTY is up yet.
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
  // command or its rejection may have been lost, and nothing would reconcile
  // the overlay afterwards. It also voids a pending create-focus, whose
  // `knownIds` snapshot predates the disconnect and could mis-identify an
  // unrelated session as ours.
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
  // falls back to scope `All` (broadcast), visible to this client once it
  // reconnects, the safe default. The next `connected` frame re-issues a fresh id.
  if (conn === "closed" || conn === "failed") setConnectionId(null)
  // Every drop owes a fresh run-identity check before any PTY socket attaches
  // again: what was confirmed was confirmed about a connection that is gone, and
  // the server may have been replaced in the gap. The next `onOpen`'s probe
  // re-publishes it.
  if (conn === "closed" || conn === "failed") clearServerValidated()
  // The per-session changed-files subscription re-establishes on reconnect in
  // `eventsSocket.onOpen` (which also refetches); nothing to re-arm here.
}

// Every action is a REST verb whose failure rejects its promise (the caller
// toasts it and rolls back optimistic state); keyed busy/success/clear arrives
// as `status`/`status_cleared` events over `/ws/events` (see
// `eventsSocket.onEvent`).

// Reset the optimistic agent-order overlay. Returned as a patch so callers can
// fold it into a single `setState`. Used on every error path so a rejected
// reorder snaps the UI back to the server's authoritative order.
function clearPendingOrders(): Partial<DuxState> {
  return {
    pendingAgentOrder: null,
  }
}

// Clear every transient client intent at once, for the failure and teardown
// paths where an in-flight create can no longer be trusted to resolve: a
// surviving create-focus token would mis-identify a later session as ours, and
// a surviving pane override would strand the pane until reload. Deliberately
// not folded into `clearPendingOrders`, which user actions like sorting also
// call and which must not cancel an in-flight create-focus.
function clearPendingClientIntent(): Partial<DuxState> {
  return {
    ...clearPendingOrders(),
    pendingCreateFocus: null,
    changesPaneOverride: null,
    mobileAccessoryBarOverride: null,
  }
}

// Stable sonner id for the anonymous (no-key) status slot. Sonner otherwise
// assigns a random id on each call, making anonymous clears a no-op and every
// anonymous update a new transient toast instead of an in-place update.
const ANON_TOAST_ID = "dux-anon-status"

// Route a keyed (or anonymous) engine status to a notification. The key acts as
// the notification id so updates re-render in place (busy → success swaps the
// spinner without a new toast) and clears can dismiss by id.
//
// `lib/notify.ts` owns everything else: the severity-graded window, the busy
// leak guard, the `0` opt-out, and `sticky`. A status arrives sticky when the
// engine says so, and an absent flag means not sticky.
function showStatusToast(
  key: string | null | undefined,
  tone: string,
  message: string,
  sticky: boolean,
): void {
  const id = key ?? ANON_TOAST_ID // no key → stable anonymous-slot id
  notifyStatus(tone, message, { id, sticky })
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
  loadWorkspace()
  // Remember which server served this tab, so a later reconnect can tell whether
  // it is still talking to it. Completing this read is also what opens the PTY
  // retry gate for this page; see the function's own doc for why boot has to be
  // the one to do it.
  void loadServerIdentityBaseline()
}
// Off-browser (a build-time static render) there is no server to talk to and no
// socket to open, so the store simply stays at its initial state until whoever
// is rendering seeds it. In a browser and under jsdom this runs exactly as before.
if (hasBrowser) boot()

// Browser/hardware Back and Forward. Registered ONCE at module scope (never in a
// React effect) so it survives re-renders and shell switches. The browser has
// already moved its own cursor by the time this fires, so the only job here is
// to read the URL it landed on and make the app match it. Nothing is derived
// from `event.state`, and nothing is counted: the hash alone says where we are.
if (hasBrowser) {
  window.addEventListener("popstate", () => {
    applyUrlRoute()
  })
  // Fragment navigation the page itself initiates (the standalone header's
  // plain-anchor "Open in dux" link is the one shipping case) is delivered
  // as `hashchange`, and whether a `popstate` accompanies it varies by
  // environment (jsdom fires only `hashchange`; browsers fire both). Listen
  // to both: `applyUrlRoute` is idempotent and by contract never writes the
  // URL back, so a double delivery settles on the same state.
  window.addEventListener("hashchange", () => {
    applyUrlRoute()
  })
}

export function useDux(): DuxState {
  // The third argument is the SERVER snapshot, which React demands whenever a
  // component is rendered outside a browser (`renderToString`). The state lives
  // in a module-level variable rather than in the DOM, so the server reads the
  // same one the client does and `getSnapshot` serves both. In a browser React
  // never calls it.
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot)
}

// --- Routing (a tiny hash router) -----------------------------------------
//
// `location.hash` is the source of truth for the whole position, the mobile
// screen included. Session ids are stable across a reload; terminal ids are
// ephemeral, so a hash whose terminal is gone falls back to the agent, and a
// hash naming an absent session resolves to `routeNotFound`, never home.
//
// A screen change pushes, in both directions; a move within one screen
// replaces. The app never steps history relatively (`history.go` appears
// nowhere). The only screen changes that replace are a restore and a
// correction, both of which name a position the browser is already parked on.

// Parse a deep-link hash into a target, or null when it is absent/malformed.
// The three shapes are mutually exclusive, so the first one that MATCHES
// answers, malformed contents included.
//
// The patterns are function-local rather than module constants: this runs at
// module init, where a constant declared further down the file is still in the
// temporal dead zone.
function parseSelectionHash(hash: string): SelectedTarget | null {
  // A project terminal has its own grammar, because the agent shapes embed a
  // session id and it has none.
  const project = hash.match(/^#\/project\/([^/]+)\/terminal\/([^/]+)$/)
  if (project) return projectTerminalTarget(project)
  // A standalone terminal deep-links with no owner segment, because it has no
  // owner. It cannot be confused with the two nested shapes, which both begin
  // `#/agent/` or `#/project/`.
  const standalone = hash.match(/^#\/terminal\/([^/]+)$/)
  if (standalone) return standaloneTerminalTarget(standalone)
  // The bare agent (its session-slot tab), an extra tab, or a companion
  // terminal, disambiguated by the literal `tab` or `terminal` keyword so a tab
  // named "tab" cannot be confused.
  const agent = hash.match(/^#\/agent\/([^/]+)(?:\/(tab|terminal)\/([^/]+))?$/)
  return agent ? agentTarget(agent) : null
}

// One decoded path segment, or null when it is empty or its encoding is
// malformed. `decodeURIComponent` throws a URIError on `%ZZ`, and parsing runs
// at module init, where an unguarded throw would blank the whole app.
function decodeSegment(raw: string): string | null {
  try {
    return decodeURIComponent(raw) || null
  } catch {
    return null
  }
}

function projectTerminalTarget(m: RegExpMatchArray): SelectedTarget | null {
  const projectId = decodeSegment(m[1])
  const terminalId = decodeSegment(m[2])
  if (!projectId || !terminalId) return null
  return { kind: "terminal", terminalId, owner: { kind: "project", projectId } }
}

function standaloneTerminalTarget(m: RegExpMatchArray): SelectedTarget | null {
  const terminalId = decodeSegment(m[1])
  if (!terminalId) return null
  return { kind: "terminal", terminalId, owner: { kind: "standalone" } }
}

function agentTarget(m: RegExpMatchArray): SelectedTarget | null {
  const sessionId = decodeSegment(m[1])
  if (!sessionId) return null
  if (m[2] === "terminal") {
    const terminalId = decodeSegment(m[3])
    if (!terminalId) return null
    return { kind: "terminal", terminalId, owner: { kind: "session", sessionId } }
  }
  if (m[2] === "tab") {
    const tabId = decodeSegment(m[3])
    if (!tabId) return null
    // A self-aliased `#/agent/<sid>/tab/<sid>` is the session-slot tab written
    // the long way, `selectionHash` normalizes it back to the canonical bare
    // form on the way out, so there is only ever one representation of it.
    return { kind: "agent", sessionId, tabId }
  }
  return { kind: "agent", sessionId, tabId: slotTabTargetId(sessionId) }
}

// The hash for a target (or the bare path when nothing is selected). The `/tab/`
// segment is emitted ONLY for an extra tab; the session-slot tab stays the bare
// `#/agent/<sid>` so existing bookmarks remain valid.
function selectionHash(target: SelectedTarget | null): string {
  if (!target) return ""
  if (target.kind === "terminal") {
    // The URL SHAPE is an owner decision, so it is a switch, not a conditional:
    // each owner kind has its own grammar and a new one needs its own, which is
    // a thing to write rather than a thing to fall through into.
    const owner = target.owner
    const tid = encodeURIComponent(target.terminalId)
    switch (owner.kind) {
      case "project":
        return `#/project/${encodeURIComponent(owner.projectId)}/terminal/${tid}`
      case "session":
        return `#/agent/${encodeURIComponent(owner.sessionId)}/terminal/${tid}`
      // No owner segment, because there is no owner. The terminal id alone
      // names it, which is the whole grammar.
      case "standalone":
        return `#/terminal/${tid}`
      default:
        return assertNever(owner)
    }
  }
  const base = `#/agent/${encodeURIComponent(target.sessionId)}`
  // Spine-aware: the slot tab's real id is a generated one, and emitting it as
  // a `/tab/` segment would change every bookmark an agent already has.
  return isSlotTabOf(target.sessionId, target.tabId)
    ? base
    : `${base}/tab/${encodeURIComponent(target.tabId)}`
}

// A position in the app: what is focused, whether the changes screen is open
// on top of it, and whether (and where) the editor is open on top of it. This
// is everything the URL encodes and everything the screen is derived from.
// Exported for the route-grammar round-trip tests only.
export interface Route {
  target: SelectedTarget | null
  changes: boolean
  // The editor suffix: the active tab's mode and path, path null when the
  // editor is open with no file. Mutually exclusive with `changes` in the
  // SERIALIZED form: `routeHash` emits at most one suffix (editor wins), and
  // `parseRoute` tries the editor suffix first.
  editor: { mode: EditorViewMode; path: string | null } | null
  // Theater mode: the focused pane with dux's chrome taken away. A MODIFIER on
  // the position rather than a shape of its own, so it is spelled as a trailing
  // `?view=theater` on whatever grammar the position already has (see
  // `lib/theater.ts`), and it is dropped for every address with no pane to
  // fill: home, the changes screen, and both editor surfaces.
  theater: boolean
  // True when the address is the STANDALONE editor's whole-tab form
  // (`#/editor/agent/<sid>[/<mode>/<encoded-path>]`) rather than the in-app
  // suffix. Only meaningful with a non-null `editor`; it decides which shell
  // `App()` renders and which grammar `routeHash` writes back.
  standalone: boolean
}

// The changes screen rides as a suffix on the focused target's hash, so it
// bookmarks and shares like any other position.
const CHANGES_SUFFIX = "/changes"

// The editor rides as a suffix too: `#/agent/<sid>/editor` (open, no file) or
// `#/agent/<sid>/editor/<mode>/<encoded-path>` (mode = file | diff, path
// encodeURIComponent-encoded, so it is one slashless segment).
const EDITOR_SUFFIX = "/editor"

// Parse the editor suffix off a hash, or null when it carries none. The prefix
// must itself parse as a target of any kind, a terminal included.
//
// Not one greedy regex, deliberately: a file named "editor" puts "/editor" in
// the hash twice, and splitting at the last one leaves a prefix that is not a
// target. Every occurrence is tried, rightmost first, and the first candidate
// whose prefix parses wins.
function parseEditorRoute(hash: string): Route | null {
  let at = hash.length
  while ((at = hash.lastIndexOf(EDITOR_SUFFIX, at - 1)) > 0) {
    const tail = hash.slice(at + EDITOR_SUFFIX.length)
    const tm = tail.match(/^(?:\/(file|diff)\/([^/]+))?$/)
    if (!tm) continue
    const target = parseSelectionHash(hash.slice(0, at))
    if (!target) continue
    const editor = parseEditorSegment(tm[1], tm[2])
    return { target, changes: false, editor, standalone: false, theater: false }
  }
  return null
}

// The standalone editor's whole-tab address: the root's own spelling plus the
// same optional mode/path pair as the in-app suffix. Three shapes, one per
// root the surface can carry:
//
//   #/editor/agent/<sid>
//   #/editor/terminal/<tid>
//   #/editor/project/<pid>/terminal/<tid>
//
// None can collide with the target grammars, which never begin `#/editor/`. An
// agent address always names the session-slot target: the standalone surface is
// the editor, not a tab strip.
function parseStandaloneEditorRoute(hash: string): Route | null {
  const m = hash.match(
    /^#\/editor\/(agent\/[^/]+|terminal\/[^/]+|project\/[^/]+\/terminal\/[^/]+)(?:\/(file|diff)\/([^/]+))?$/,
  )
  if (!m) return null
  try {
    // The root half is the ordinary selection grammar with the `#/editor`
    // prefix peeled off, so the two can never drift: one parser, one spelling.
    const target = parseSelectionHash(`#/${m[1]}`)
    if (!target) return null
    const editor = parseEditorSegment(m[2], m[3])
    // STRICT, unlike the in-app suffix (which degrades a malformed path to
    // the bare agent): a standalone route with no editor half is a shell
    // with nothing to show, so a mangled tail is not a standalone route at
    // all; it boots the NORMAL shell and takes the ordinary
    // route-correction path there.
    if (editor === null) return null
    return { target, changes: false, editor, standalone: true, theater: false }
  } catch {
    return null
  }
}

// The shared mode/path tail of both editor grammars. No mode segment means
// "open with no file" (mode normalized to "file"); malformed
// percent-encoding in the path degrades to no editor half (the same "treat
// as no/invalid deep link" rule parseSelectionHash applies) rather than
// throwing at module init.
function parseEditorSegment(
  mode: string | undefined,
  encodedPath: string | undefined,
): Route["editor"] {
  if (encodedPath === undefined) return { mode: "file", path: null }
  try {
    const path = decodeURIComponent(encodedPath)
    if (!path) return null
    return { mode: mode as EditorViewMode, path }
  } catch {
    return null
  }
}

// Parse a hash into a route. A hash that names no valid target is home.
// Exported for the round-trip tests only (`routeHash` likewise): the grammar
// must stay an exact inverse pair, and that is only checkable from outside.
export function parseRoute(rawHash: string): Route {
  // THE MODIFIER COMES OFF FIRST. Every grammar below is end-anchored, so a
  // trailing `?view=theater` would make each of them fail to match; peeling it
  // here is what lets one modifier ride every position shape without a second
  // parser per shape. It goes back on only where a pane exists to fill the
  // screen, which is the same rule `routeHash` serializes by.
  const { hash, theater } = splitTheaterHash(rawHash)
  const route = parsePosition(hash)
  return theater && theaterSerializable(route) ? { ...route, theater: true } : route
}

function parsePosition(hash: string): Route {
  // The standalone editor's grammar is prefix-disjoint from everything else
  // (`#/editor/…` vs `#/agent/…`, `#/project/…`, `#/terminal/…`), so its
  // position in this ladder decides nothing; it goes first as the most
  // specific shape.
  const standalone = parseStandaloneEditorRoute(hash)
  if (standalone) return standalone
  const direct = parseSelectionHash(hash)
  if (direct) return { target: direct, changes: false, editor: null, standalone: false, theater: false }
  // The editor suffix is tried before the changes suffix, which is what keeps
  // the two mutually exclusive: a file literally named "changes" must resolve
  // as an editor route, never as a changes route with a mangled tail. Every
  // regex here is end-anchored, so no hash parses two ways.
  const editor = parseEditorRoute(hash)
  if (editor) return editor
  if (hash.endsWith(CHANGES_SUFFIX)) {
    const target = parseSelectionHash(hash.slice(0, -CHANGES_SUFFIX.length))
    if (target) return { target, changes: true, editor: null, standalone: false, theater: false }
  }
  return { target: null, changes: false, editor: null, standalone: false, theater: false }
}

// The hash for a route. Home is the empty hash; a suffix only applies on top of
// a focused target and at most one is emitted, the editor winning over changes,
// which `parseRoute` mirrors by trying the editor suffix first. A pathless
// editor suffix carries no mode segment, so a pathless route's mode normalizes
// to "file" on the way back in.
export function routeHash(route: Route): string {
  return withTheaterHash(
    positionHash(route),
    route.theater && theaterSerializable(route),
  )
}

function positionHash(route: Route): string {
  // The standalone form replaces the whole address rather than riding as a
  // suffix, and is session-slot only with no changes screen: an extra-tab
  // target serializes by its session id alone and a changes flag is dropped,
  // which the parser mirrors. A standalone route that lost its session, or its
  // editor half, falls through to the ordinary grammar.
  if (route.standalone && route.editor && route.target !== null) {
    // The root half is the selection grammar with `#/editor` in front of it,
    // the exact inverse of the parser's peel.
    const base = `#/editor${selectionHash(slotTargetFor(route.target)).slice(1)}`
    if (route.editor.path === null) return base
    return `${base}/${route.editor.mode}/${encodeURIComponent(route.editor.path)}`
  }
  const base = selectionHash(route.target)
  if (base === "") return base
  if (route.editor) {
    if (route.editor.path === null) return base + EDITOR_SUFFIX
    return `${base}${EDITOR_SUFFIX}/${route.editor.mode}/${encodeURIComponent(route.editor.path)}`
  }
  if (!route.changes) return base
  return base + CHANGES_SUFFIX
}

// The one rule a standalone editor address is spelled by: an agent is named by
// its session-slot tab, which is the only form the parser can produce, and a
// terminal is already its own whole spelling.
//
// Lossy on purpose from a selection: an extra tab's id is dropped in favour of
// the slot's, mirroring that normalization. From an editor root there is no tab
// id to drop, so the step is the plain inverse of `editorRootForTarget`.
function slotTargetFor(target: SelectedTarget | EditorRoot): SelectedTarget {
  if (target.kind === "terminal") return target
  return {
    kind: "agent",
    sessionId: target.sessionId,
    tabId: slotTabTargetId(target.sessionId),
  }
}

// The screen a route puts the mobile shell on. This is the whole derivation:
// screen state is never tracked independently of the route.
function routeScreen(route: Route): MobileScreen {
  if (!route.target) return "home"
  return route.changes ? "changes" : "terminal"
}

// What `syncUrl` compares to decide push versus replace. Deliberately not
// `routeScreen`, whose output IS `mobileScreen`: folding the editor bit in
// would leak an "editor" screen into the mobile shell. Opening the editor
// changes the key, so it pushes; switching files inside it does not.
//
// The key says nothing about which root the editor is on, so retargeting a
// standalone editor tab replaces rather than pushes. Accepted: only a
// hand-edited address can produce that.
export function routePushKey(route: Route): string {
  return `${routeScreen(route)}${route.editor ? "+editor" : ""}${route.standalone ? "+standalone" : ""}`
}

// The standalone editor's address for a root, optionally carrying the file
// position the affordance should hand over. Pure, and built on `routeHash` so
// the open-in-new-tab anchors can never drift from the parser's grammar.
export function standaloneEditorHash(
  root: EditorRoot,
  editor: { mode: EditorViewMode; path: string | null } | null = null,
): string {
  return routeHash({
    target: slotTargetFor(root),
    changes: false,
    editor: editor ?? { mode: "file", path: null },
    standalone: true,
    // The editor surface has no PTY to give the height to, so it never carries
    // the modifier. Stated rather than left to `theaterSerializable` to drop.
    theater: false,
  })
}

// The route the app currently holds in state.
function currentRoute(): Route {
  const target = state.selectedTarget
  const er = state.editorRoute
  // The editor arm is serialized only while it names the SAME root the focused
  // target resolves to: in the one window where they can disagree (the
  // editor's target vanished and the selection prune is navigating away before
  // the editor prune clears the state), the URL must not carry a dead editor
  // suffix on the new address.
  const editor =
    er !== null && target !== null && sameRoot(editorRootForTarget(target), er.root)
      ? { mode: er.mode, path: er.path }
      : null
  return {
    target,
    changes: state.mobileScreen === "changes",
    editor,
    theater: state.theater,
    // The surface bit, so a file switch inside the standalone tab writes the
    // standalone grammar back rather than silently converting the address to
    // the in-app form.
    standalone: state.standaloneEditor && editor !== null,
  }
}

// Bring the URL in line with the app's current position: pushes on a screen
// change, replaces within one screen. `mode: "replace"` forces a replace for a
// move the user did not ask for (a restore, or leaving the not-found screen).
//
// Best-effort and never throws at its caller: browsers rate-limit history calls
// and every call site runs after the screen has already moved, so a refusal
// propagating would leave the screen and the URL disagreeing. This is the one
// place a history call is made, which is what makes the one guard enough.
function syncUrl(mode?: "replace" | "push"): void {
  if (typeof history === "undefined" || typeof history.replaceState !== "function") {
    return
  }
  const next = routeHash(currentRoute())
  const current = currentHash()
  // Belt and braces: when the address is already what we would write, the
  // branch below would take the replace arm anyway (an unchanged hash is an
  // unchanged screen) and rewrite the identical URL. Skipping the write is
  // cheaper and keeps `history.state` untouched, but nothing depends on it.
  if (current === next) return
  const url = historyUrlFor(next)
  const movedScreen = movesScreen(mode, next, current)
  try {
    if (mode !== "replace" && movedScreen && typeof history.pushState === "function") {
      history.pushState({ duxRoute: next }, "", url)
      return
    }
    history.replaceState(history.state, "", url)
  } catch (err) {
    console.warn("[dux] history write refused", err)
  }
}

// The hash the browser is parked on, or "" where there is no browser to ask:
// the store is imported by the marketing site's static render, which runs under
// plain Node.
function currentHash(): string {
  if (typeof location === "undefined") return ""
  return location.hash ?? ""
}

// The URL to write for a hash. An empty target hash collapses to the bare path
// so the URL doesn't keep a dangling "#"; otherwise write just the hash,
// preserving path + query.
function historyUrlFor(hash: string): string {
  if (hash !== "") return hash
  if (typeof location === "undefined") return ""
  return (location.pathname ?? "") + (location.search ?? "")
}

// Whether writing `next` over `current` is a move Back should come out of.
//
// `routePushKey`, not `routeScreen`: the editor-open bit must push and pop like
// a screen without being one. `mode: "push"` is for a move the key cannot
// describe: entering theater is a position Back must come out of, while still
// being the terminal screen. Leaving replaces, so Back never re-enters a mode
// just dismissed.
function movesScreen(
  mode: "replace" | "push" | undefined,
  next: string,
  current: string,
): boolean {
  if (mode === "push") return true
  return routePushKey(parseRoute(next)) !== routePushKey(parseRoute(current))
}

// Adopt the route the URL currently names. Called from popstate, where the
// browser has already moved its cursor, so this only mirrors the destination
// into state and must never write the URL back.
function applyUrlRoute(): void {
  const hash = currentHash()
  const route = parseRoute(hash)
  // The surface bit follows the URL immediately, spine or no spine: which
  // shell renders must not wait on a fetch, and it is what lets the
  // standalone header's plain-anchor open-in-dux link (and a Back across it)
  // swap surfaces with no code of its own.
  if (route.standalone !== state.standaloneEditor) {
    setState({ standaloneEditor: route.standalone })
    // The tab title carries an "Editor" prefix on the standalone surface, so
    // it must re-render the moment the surface bit flips (the other refresh
    // triggers are spine applies and bootstraps, which need not coincide).
    refreshAttentionChrome()
  }
  const spine = state.spine
  if (!spine) {
    // A popstate before the first spine landed. The route cannot be left to the
    // boot deep-link restore, which resolves the BOOT hash the browser has
    // since moved off: that either strands the address bar on an agent the app
    // never selects, or silently undoes the Back.
    //
    // So the pending boot link is replaced by where the browser actually is,
    // for `restoreDeepLink` to resolve against the first spine, and a route
    // naming home replaces it with null. Resolving a target needs a session
    // list, so nothing more can happen here.
    pendingDeepLink = route.target
    pendingDeepLinkChanges = route.changes
    pendingDeepLinkEditor = route.editor
    pendingDeepLinkTheater = route.theater
    return
  }
  if (!route.target) {
    // Home names no editor, so a Back that lands here closes an open one:
    // state only; the selection clear below is the URL's writer (and writes
    // nothing, since the browser is already parked on home).
    clearEditorStateSilently()
    // Through `selectSessionRoute`, not `clearSelection`, because pressing Back
    // to home is the user taking control: it must disarm the reconnect
    // deep-link intent, or a reconnect could yank them back to the agent they
    // just left.
    selectSessionRoute(null)
    return
  }
  resolveRoute(spine, route)
}

// Clear the editor's open state WITHOUT writing the URL, for the paths that
// must not write it: reconstitution, where the browser is already parked on an
// address naming no editor, and the spine prune, whose single URL writer is
// `pruneSelectionIfGone`. Everything user-initiated uses `closeEditor`.
//
// It also drops the standalone-surface flag, because a shell left up over a
// null `editorTarget` is a permanent boot spinner. Popstate is unaffected:
// `applyUrlRoute` re-syncs that flag from the URL before this runs.
function clearEditorStateSilently(): void {
  if (
    state.editorTarget === null &&
    state.editorRoute === null &&
    !state.standaloneEditor
  ) {
    return
  }
  setState({ editorTarget: null, editorRoute: null, standaloneEditor: false })
}

// Has the open editor, on either surface, lost what it was rooted at? An editor
// does not outlive its target, but only a destructive confirm may discard typed
// text, and both surfaces compose those two rulings alike.
//
// Answering true owns the whole pass, so the ordinary selection and editor
// prunes do not also run: the editor prune would delete the very tabs a dirty
// hold protects, and the selection prune would blank the standalone tab.
function endOpenEditorIfRootGone(spine: Spine): boolean {
  const root = state.editorTarget?.root ?? state.editorRoute?.root ?? null
  if (root === null || rootIsLive(spine, root)) return false
  if (hasDirtyTabForRoot(root)) {
    // Hold the pass for this root: what vanished is the root that could have
    // saved this text, so the tab stays as it is until the confirm behind
    // `editorTargetGone` is answered, and the dead selection it holds resolves
    // then. Other dead roots have nothing on screen and are pruned as usual.
    const key = rootKey(root)
    pruneDeadEditorTabs(liveEditorRootKeys(spine), key)
    if (vanishedEditorAsked !== key) {
      vanishedEditorAsked = key
      setState({ editorTargetGone: root })
    }
    return true
  }
  // Nothing to lose: say what happened and let the ordinary prunes do the
  // leaving, which already lands on a real surface (pinned for the standalone
  // tab by "swaps the standalone tab to the ordinary shell when its session
  // vanishes" in storeRouting.test.ts).
  notifyWarning(vanishedEditorMessage(root))
  return false
}

// Which root the vanish question has already been asked about, so a second
// spine does not ask again. Module state rather than store state: it is about
// a question that was answered, not about anything the UI renders.
let vanishedEditorAsked: string | null = null

function hasDirtyTabForRoot(root: EditorRoot): boolean {
  const tabs = state.editorTabs[rootKey(root)]
  return tabs !== undefined && tabs.tabs.some((tab) => tab.dirty)
}

function vanishedEditorMessage(root: EditorRoot): string {
  return root.kind === "terminal"
    ? "That terminal closed, so the editor rooted at its directory went with it. \
Nothing unsaved was open. You are back at the workspace."
    : "That agent is gone, so its editor went with it. Nothing unsaved was \
open. You are back at the workspace."
}

// Leave the vanished editor: drop its state and land on home. The confirm
// behind the dirty case calls this, and so does the clean case directly.
export function discardVanishedEditor(): void {
  setState({ editorTargetGone: null })
  clearEditorStateSilently()
  // There is no selection worth keeping once the editor is gone: the held
  // selection named the vanished root itself. Landing on home is the one URL
  // write in the pass, and it is a replace, because the address behind it
  // names a target that no longer exists and must not be re-enterable by
  // Back. Both surfaces take this same exit.
  selectSessionRoute(null, "replace")
}

// Keep it open: the text stays on screen to be copied out, and nothing asks
// again for this root.
export function keepVanishedEditor(): void {
  setState({ editorTargetGone: null })
}

// Mirror a parsed route's editor half into state, directly and silently. The
// reconstitution path: it never calls `openEditor`, whose `selectSession` would
// re-write an address the browser is already parked on, and writes no URL
// itself. An editor half naming a session this spine does not have, or no
// editor half at all, closes an open editor as state only.
function syncEditorStateFromRoute(spine: Spine, route: Route): void {
  const root =
    route.editor !== null && route.target !== null
      ? editorRootForTarget(route.target)
      : null
  if (route.editor === null || root === null || !rootIsLive(spine, root)) {
    clearEditorStateSilently()
    return
  }
  const mode = editorMode(root, route.editor.mode, route.editor.path)
  const { path } = route.editor
  // Keep the existing mount seed when it already points at this root:
  // `EditorBody` is keyed by the root key, so churning the seed would remount
  // it for nothing on every Back/Forward inside the same editor.
  const editorTarget =
    state.editorTarget !== null && sameRoot(state.editorTarget.root, root)
      ? state.editorTarget
      : { root, initialPath: path, initialMode: mode }
  setState({ editorTarget, editorRoute: { root, mode, path } })
  if (path !== null) editorOpenFile(root, path, { mode })
}

// Does this spine still have the thing the root names? The two kinds are
// looked up in different lists, and both have to be, or a closed terminal
// would keep an editor open over a directory nothing is running in.
function rootIsLive(spine: Spine, root: EditorRoot): boolean {
  if (root.kind === "agent") {
    return spine.sessions.some((session) => session.id === root.sessionId)
  }
  return spine.terminals.some((terminal) => terminal.id === root.terminalId)
}

// Resolve a whole parsed route against a spine: the editor half first (state
// only), then the target half, which owns any URL correction. Shared by
// popstate, the boot deep-link restore, and the not-found retry, so all three
// reconstitute the editor identically.
function resolveRoute(spine: Spine, route: Route): void {
  if (route.target === null) return
  syncEditorStateFromRoute(spine, route)
  // The address wins over the pane's remembered mode while a route resolves,
  // because the route is the user's position: a shared theater link and a Back
  // out of theater both have to override that memory. Armed around the commit
  // and dropped in a `finally`, so a route resolving to not-found cannot leave
  // the override armed for the next selection.
  pendingTheater = route.theater
  try {
    resolveRouteTarget(spine, route.target, route.changes)
  } finally {
    pendingTheater = undefined
  }
}

// The theater flag a route being resolved carries, consumed by the very next
// selection commit (see `theaterPatch`). Module state rather than a parameter
// threaded through five restore paths: every one of them ends in one of the
// three selection actions, and this is the one place that has to know.
let pendingTheater: boolean | undefined

// The theater half of a selection commit, landing in the SAME state patch as
// the target so `syncUrl` never writes an address from a half-applied
// position. With no route override in flight it is the pane's own remembered
// mode; with one, the address wins and is written back to the memory, because
// following a link into theater is as much a choice as pressing the button.
function theaterPatch(
  target: SelectedTarget | null,
  override?: boolean,
): { theater: boolean } {
  const explicit = override ?? pendingTheater
  // Always dropped, override or not: a route's flag belongs to the very next
  // commit and nothing else.
  pendingTheater = undefined
  const theater = (() => {
    if (target === null) return false
    const key = theaterMemoryKey(target)
    if (explicit === undefined) return readTheaterMemory(key)
    writeTheaterMemory(key, explicit)
    return explicit
  })()
  // Selecting a pane whose memory says theater is an entry like any other, and
  // selecting away from one is an exit: the side panels have to be captured and
  // restored on those transitions too, not only on the button's. The snapshot
  // itself is left to `withTheaterLayout`, which every commit that carries this
  // patch goes through: a caller's `extra` can still overrule the flag, and the
  // snapshot has to follow the value that wins.
  return { theater }
}

// Resolve a route's target against a spine and commit it, or record not-found
// when the agent it names is gone. The URL is not rewritten on the not-found
// path: the address the user is looking at stays truthful, and Forward still
// works.
function resolveRouteTarget(
  spine: Spine,
  target: SelectedTarget,
  changes: boolean,
): void {
  let sessionId: string
  if (target.kind === "terminal") {
    // How a terminal route RESOLVES depends on its owner, so this is a switch:
    // an owner that is not a session cannot resolve through the session list and
    // must say what it resolves against instead.
    const owner = target.owner
    switch (owner.kind) {
      case "project":
        // A project terminal belongs to no session, so it resolves against the
        // project list on its own.
        applyProjectTerminalDeepLink(
          spine,
          target.terminalId,
          owner.projectId,
          "replace",
          changes,
        )
        return
      case "session":
        sessionId = owner.sessionId
        break
      case "standalone":
        // No owner to resolve against, so the only question is whether the
        // terminal is still there.
        applyStandaloneTerminalDeepLink(
          spine,
          target.terminalId,
          "replace",
          changes,
        )
        return
      default:
        return assertNever(owner)
    }
  } else {
    sessionId = target.sessionId
  }
  const session = spine.sessions.find((s) => s.id === sessionId)
  if (!session) {
    setRouteNotFound(sessionId)
    return
  }
  // `changes` travels with the target rather than after it: `syncUrl` reads the
  // screen off state, so committing them separately would write the address
  // from a half-applied route and strip `/changes` off the URL being resolved.
  //
  // The `"replace"` is belt and braces (every caller is already parked on this
  // hash), passed so the call site states the intent: a restore, not a new
  // position.
  applyDeepLinkSelection(session, spine.terminals, target, "replace", changes)
}

// The session a route target belongs to, or null for a terminal owned by
// something that is not a session. The lossy `ownerSessionId` is right here
// because every caller only asks whether there is session-scoped state to
// resolve; a non-session owner has none, whichever kind it is.
function targetSessionId(target: SelectedTarget): string | null {
  if (target.kind === "agent") return target.sessionId
  return ownerSessionId(target.owner)
}

// Retire the not-found screen once a spine carries the agent its URL names.
// Nothing else on the spine path clears the flag: the prune returns early with
// no selection to prune, so without this the screen sticks after the agent
// comes back, and on a phone it is the whole shell.
//
// Re-checking that the URL still names the flagged agent is belt and braces:
// any patch carrying a target already clears the flag. It stays because it
// makes "never re-read a stale hash" true by inspection.
function retryRouteNotFound(spine: Spine): void {
  const missing = state.routeNotFound
  if (!missing) return
  const route = parseRoute(currentHash())
  if (!route.target) return
  if (targetSessionId(route.target) !== missing.sessionId) return
  if (!spine.sessions.some((s) => s.id === missing.sessionId)) return
  resolveRoute(spine, route)
}

// The URL names an agent this workspace does not have. Clear the selection and
// hand the surfaces something truthful to render (see `AgentNotFound`); pressing
// Back onto a deleted agent is a normal thing to do.
function setRouteNotFound(sessionId: string): void {
  const prev = state.selectedSessionId
  setState(
    withTheaterLayout({
      selectedTarget: null,
      selectedSessionId: null,
      changes: emptyChanges(),
      mobileScreen: "home",
      theater: false,
      routeNotFound: { kind: "agent", sessionId },
    }),
  )
  switchChangesSubscription(prev, null)
}

// The route parsed from the URL at module load, restored once the first spine
// lands (a target can't be resolved until the session list exists). One-shot:
// consumed (and cleared) on the first `applyWorkspace` so later spine refetches don't
// re-yank a user who has since navigated away.
const bootRoute: Route =
  typeof location !== "undefined"
    ? parseRoute(location.hash ?? "")
    : { target: null, changes: false, editor: null, standalone: false, theater: false }
// All three halves are mutable: a popstate that beats the first spine
// overwrites them with the address the browser actually moved to (see
// `applyUrlRoute`), so the restore resolves that rather than a boot hash the
// user has already left.
let pendingDeepLink: SelectedTarget | null = bootRoute.target
let pendingDeepLinkChanges = bootRoute.changes
let pendingDeepLinkEditor = bootRoute.editor
let pendingDeepLinkTheater = bootRoute.theater

// Route a normalized route target onto an already-resolved session: restore a
// still-present terminal or extra tab, else fall back to the session-slot tab.
// Shared by every restore path so all of them honor tabs and terminals alike.
// `changes` is committed in the same patch as the target, never after it, so
// the URL is only ever written from a whole route.
function applyDeepLinkSelection(
  session: Spine["sessions"][number],
  terminals: readonly TerminalView[],
  target: SelectedTarget,
  urlMode?: "replace",
  changes?: boolean,
): void {
  if (target.kind === "terminal") {
    // A handler per owner variant, not `if (owner.kind !== "session") return`.
    //
    // An early return is silent about which owner it declines and keeps
    // declining a third kind that has no other restore path, dropping the
    // user's position with no trace. Written as a match, a new variant cannot
    // be added without answering for it here.
    const terminal = target
    matchOwner(target.owner, {
      session: (owner) => {
        if (ownerHasTerminal(terminals, owner, terminal.terminalId)) {
          selectTerminal(terminal.terminalId, owner, { urlMode, changes })
          return
        }
        // Terminal id gone, so fall back to the owning agent, keeping the
        // changes screen: changed files are SESSION-scoped, so they survive any
        // fallback that stays inside the same session.
        selectSessionRoute(owner.sessionId, urlMode, changes)
      },
      // Project-terminal links restore through `applyProjectTerminalDeepLink`,
      // which resolves them against the project rather than a session, so they
      // never reach this function. Nothing to do, stated rather than implied.
      project: () => {},
      // Standalone links restore through `applyStandaloneTerminalDeepLink` for
      // the same reason, and never reach this function either.
      standalone: () => {},
    })
    return
  }
  if (
    !isFirstTab(session, target.tabId) &&
    session.tabs.some((t) => t.id === target.tabId)
  ) {
    // An extra-tab deep link: restore it only if the tab still exists, else fall
    // through to the session-slot tab. `persist: false` because merely FOLLOWING
    // a shared link must not rewrite the workspace-shared remembered tab for
    // everyone that opens it.
    selectTab(target.sessionId, target.tabId, { persist: false, urlMode, changes })
    return
  }
  selectSessionRoute(target.sessionId, urlMode, changes)
}

// Restore the boot URL against the first spine: the terminal when it still
// exists, else the session, else the not-found screen. The mobile shell lands
// on the screen the URL names, which is what makes an agent link open its
// terminal rather than leaving the hub on top of it. Nothing is pushed: the
// browser is already parked on this entry.
function restoreDeepLink(spine: Spine): void {
  const link = pendingDeepLink
  if (!link) return
  pendingDeepLink = null // one-shot, whatever the outcome
  resolveRoute(spine, {
    target: link,
    changes: pendingDeepLinkChanges,
    editor: pendingDeepLinkEditor,
    standalone: state.standaloneEditor,
    theater: pendingDeepLinkTheater,
  })
}

// Restore a project-terminal route against a spine, landing home when either
// the project or the terminal is gone. Landing home writes the URL too, or the
// address bar would name a terminal the app is not showing. There is
// deliberately no not-found screen here: terminal ids are ephemeral, so a
// closed terminal is ordinary rather than a broken link.
function applyProjectTerminalDeepLink(
  spine: Spine,
  terminalId: string,
  projectId: string,
  urlMode?: "replace",
  changes?: boolean,
): void {
  const owner: TerminalOwnerRef = { kind: "project", projectId }
  if (
    spine.projects.some((p) => p.id === projectId) &&
    ownerHasTerminal(spine.terminals, owner, terminalId)
  ) {
    selectTerminal(terminalId, owner, { urlMode, changes })
    return
  }
  selectSessionRoute(null, urlMode)
}

// The standalone twin of `applyProjectTerminalDeepLink`: select the terminal
// when the workspace still carries it, and land home (URL included, never
// silently) when it is gone. There is no owner to check first, which is the only
// difference; a standalone terminal that is gone is gone.
function applyStandaloneTerminalDeepLink(
  spine: Spine,
  terminalId: string,
  urlMode?: "replace",
  changes?: boolean,
): void {
  const owner: TerminalOwnerRef = { kind: "standalone" }
  if (ownerHasTerminal(spine.terminals, owner, terminalId)) {
    selectTerminal(terminalId, owner, { urlMode, changes })
    return
  }
  selectSessionRoute(null, urlMode)
}

// Deep-link intent re-armed on an events-socket reconnect, distinct from the
// boot `pendingDeepLink` one-shot: a reconnect can eject to the welcome screen
// while the agent is momentarily `detached`, wiping the hash, and the boot
// one-shot is spent. The route is captured before any spine apply can wipe it
// and restored only once the agent is back to `active`, since restoring while
// still `detached` ping-pongs with the eject.
//
// It carries the whole position, not just the target: `parseSelectionHash`
// reads `#/agent/<sid>/changes` as no link, so a target-only intent would drop
// a user reading changed files onto the terminal screen.
interface ReconnectDeepLink {
  target: SelectedTarget
  changes: boolean
  armedAt: number
}
let reconnectDeepLink: ReconnectDeepLink | null = null

// How long the re-armed intent stays live, measured from the latest
// events-socket reopen, which `armReconnectDeepLink` refreshes `armedAt` on. A
// best-effort bound well above a normal provider resume: an agent that never
// returns to `active` within it gives up rather than chase indefinitely. A
// sleep gets a fresh window on the wake-triggered reopen, so this mostly only
// bites a resume that is genuinely stuck.
const RECONNECT_DEEPLINK_TTL_MS = 60_000

// Set by `ejectSelectionForReconnect` immediately around its own
// `selectSession(null)` call, and cleared (to `false`) by every OTHER entry
// into `selectSession`. This is how `restoreReconnectDeepLink` tells "the
// center pane's transient reconnect-eject just cleared the selection" apart
// from "the user deliberately navigated (including to home) on their own":
// only the former should be undone once the agent comes back.
let lastClearWasReconnectEject = false

// Capture the current route as the reconnect deep-link intent. Called from the
// events socket's reconnect `onOpen` BEFORE `loadWorkspace`, so the hash is read
// while it still names the agent (the transient eject happens later, during the
// spine apply).
function armReconnectDeepLink(): void {
  if (typeof location === "undefined") {
    reconnectDeepLink = null
    return
  }
  // Through `parseRoute`, never `parseSelectionHash`: the latter's regexes are
  // anchored, so a hash carrying the `/changes` suffix parses as null and the
  // whole reconnect would arm nothing.
  const route = parseRoute(location.hash ?? "")
  if (route.target) {
    reconnectDeepLink = {
      target: route.target,
      changes: route.changes,
      armedAt: Date.now(),
    }
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
  const armedTarget = armed.target
  if (armedTarget.kind === "agent") {
    restoreSessionScopedReconnect(spine, armed, armedTarget.sessionId)
    return
  }
  // A handler per owner variant, not a predicate plus a nullable id: reducing
  // the owner to a nullable session id would silently drop an unhandled kind's
  // restoration intent, while the matcher's object literal is missing a key the
  // moment a variant is added, which is a compile error here.
  const terminal = armedTarget
  matchOwner(terminal.owner, {
    session: (owner) =>
      restoreSessionScopedReconnect(spine, armed, owner.sessionId),
    project: (owner) =>
      restoreAgentlessTerminalReconnect(spine, armed, terminal.terminalId, owner),
    // A standalone terminal restores on exactly the same terms as a project one
    // (no resume phase, no reconnect eject, so the selection normally survives
    // on its own), so it shares the restore rather than getting a near-copy of
    // it. The owner is passed through because it is what `selectTerminal` needs.
    standalone: (owner) =>
      restoreAgentlessTerminalReconnect(spine, armed, terminal.terminalId, owner),
  })
}

// The agentless half of `restoreReconnectDeepLink`: a terminal owned by a
// project or by nothing, which restore on identical terms.
//
// Neither has a resume phase and neither pane issues the reconnect eject, so
// the selection normally survives on its own and this disarms as a no-op. The
// one restorable gap is a selection cleared by our own eject while the intent
// was armed; any deliberate navigation disarms instead.
function restoreAgentlessTerminalReconnect(
  spine: Spine,
  armed: ReconnectDeepLink,
  terminalId: string,
  owner: Exclude<TerminalOwnerRef, { kind: "session" }>,
): void {
  const sel = state.selectedSessionId
  const cur = state.selectedTarget
  if (cur?.kind === "terminal" && cur.terminalId === terminalId) {
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
  // The owner must still be there too, where there IS one. A standalone
  // terminal has no owner that could have gone, so its existence is the whole
  // question; a project terminal's project must have survived as well.
  const ownerStillThere =
    owner.kind === "project"
      ? spine.projects.some((p) => p.id === owner.projectId)
      : true
  const exists =
    ownerStillThere && ownerHasTerminal(spine.terminals, owner, terminalId)
  if (!exists) return // keep waiting within the TTL (the spine may lag)
  // A replace: this restores the position the browser is ALREADY parked on
  // (the hash still names it, or our own eject rewrote it). Pushing would add
  // an entry per reconnect, so a flaky link would put an unbounded pile of
  // duplicates between the user and home.
  selectTerminal(terminalId, owner, {
    urlMode: "replace",
    changes: armed.changes,
  })
  reconnectDeepLink = null
}

// The session-scoped half of `restoreReconnectDeepLink`: an agent target, or a
// terminal owned by a session. `armedSessionId` is the session to wait for and
// is never null, because the caller reached here by MATCHING an owner variant
// that has one rather than by reducing an owner to a nullable id.
function restoreSessionScopedReconnect(
  spine: Spine,
  armed: ReconnectDeepLink,
  armedSessionId: string,
): void {
  const sel = state.selectedSessionId
  // The user actively moved to a DIFFERENT agent since we armed, respect it and
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
  // A replace, for the same reason as the project-terminal branch above: this is
  // a restore of the position the URL already named, not a move the user made.
  if (sel !== armedSessionId) {
    applyDeepLinkSelection(
      session,
      spine.terminals,
      armed.target,
      "replace",
      armed.changes,
    )
  }
  reconnectDeepLink = null
}

// Select an agent session as the streamed target.
//
// Restores the agent's remembered tab-focus (`resolveFocusedTab`, backed by
// `SessionView.last_focused_tab`) by routing through `selectTab` when that tab
// is still live, so the wiring matches an explicit tab click. A read of the
// memory only: `selectTab` owns persisting an actual switch.
export function selectSession(id: string | null): void {
  selectSessionRoute(id, undefined)
}

// The screen half of a route commit. A selection carries it so the target and
// the screen land in ONE state patch, which is what lets `syncUrl` write a whole
// route; `undefined` means "the ordinary derivation" (a target is the terminal
// screen), and only a route that explicitly names `/changes` passes `true`.
function screenPatch(changes?: boolean): { mobileScreen: MobileScreen } | object {
  return changes ? { mobileScreen: "changes" as const } : {}
}

// The editor half of a selection commit. Moving to a session other than the
// open editor's closes the editor in the same patch, because the hash must name
// the visible position and dropping the suffix alone leaves the two
// disagreeing. Closing also drops the standalone surface flag, as every editor
// clear outside popstate does. `openEditor` overrides this patch with its own,
// which is how its selection move and its open land as one commit.
function editorSelectionPatch(target: SelectedTarget | null): Partial<DuxState> {
  const openRoot = state.editorRoute?.root ?? state.editorTarget?.root ?? null
  if (openRoot === null) return {}
  // Compared as ROOTS, not as session ids, so a terminal-rooted editor is kept
  // by a selection of its own terminal and closed by everything else. An
  // agent's editor keeps its old behavior for free: every tab and every
  // session-owned terminal of that agent resolves to the same agent root.
  if (target !== null && sameRoot(editorRootForTarget(target), openRoot)) return {}
  return { editorTarget: null, editorRoute: null, standaloneEditor: false }
}

// `selectSession` with control over how the URL is written. `urlMode:
// "replace"` is for a move the user did not make. `extra` is a state patch
// committed in the same `setState` as the selection, and so serialized by the
// same single `syncUrl` write, which is what lets a selection move and an
// editor open land as one history entry.
function selectSessionRoute(
  id: string | null,
  urlMode?: "replace",
  changes?: boolean,
  extra?: Partial<DuxState>,
): void {
  // Any deliberate selection (to an agent OR to null/home) means the user took
  // control. See `ejectSelectionForReconnect` below for the one carve-out.
  lastClearWasReconnectEject = false
  const prev = state.selectedSessionId
  if (id === null) {
    clearSelection(urlMode, extra)
    return
  }
  const session = state.spine?.sessions.find((s) => s.id === id)
  const focusedTab = session ? resolveFocusedTab(session) : id
  if (focusedTab !== id) {
    // A remembered extra tab is still live: select it directly so the
    // hash/changes wiring and the persistence write match an explicit tab
    // click exactly.
    selectTab(id, focusedTab, { urlMode, changes, extra })
    return
  }
  const slotTab = session ? session.slot_tab_id : slotTabTargetId(id)
  setState(
    withTheaterLayout({
      selectedTarget: { kind: "agent", sessionId: id, tabId: slotTab },
      selectedSessionId: id,
      ...theaterPatch({ kind: "agent", sessionId: id, tabId: slotTab }),
      // Re-selecting the same session keeps its loaded data; a real switch
      // enters the loading window so the pane shows a spinner, not the
      // previous session's files.
      changes: prev === id ? state.changes : loadingChanges(id),
      ...editorSelectionPatch({ kind: "agent", sessionId: id, tabId: slotTab }),
      ...screenPatch(changes),
      // LAST, and the snapshot is decided around the whole merged patch for
      // exactly that reason: a caller's `extra` may carry a mode of its own
      // (the editor's suspend does), and it is the winning flag the snapshot
      // has to agree with.
      ...(extra ?? {}),
    }),
  )
  // Move the per-session changed-files subscription, THEN fetch; subscribing
  // before the GET means an invalidation that races the fetch is never missed.
  switchChangesSubscription(prev, id)
  syncUrl(urlMode)
  if (prev !== id) loadChanges(id)
}

// Drop the focused target and land on home. The target is cleared FIRST so any
// synchronous re-render shows the fallback; the URL is written after, and the
// screen follows the empty target (see `setState`). Home names no editor, so
// an open one closes in the same commit (`editorSelectionPatch`).
function clearSelection(urlMode?: "replace", extra?: Partial<DuxState>): void {
  const prev = state.selectedSessionId
  setState(
    withTheaterLayout({
      selectedTarget: null,
      selectedSessionId: null,
      // Nothing focused is nothing to fill the screen with, so home is never in
      // theater; the pane's own memory is untouched and brings it back.
      theater: false,
      changes: emptyChanges(),
      ...editorSelectionPatch(null),
      ...(extra ?? {}),
    }),
  )
  // Drop the previous session's changed-files subscription; there is no global
  // watch to clear, so the cross-client clobber is gone by construction.
  switchChangesSubscription(prev, null)
  syncUrl(urlMode)
}

// The one carve-out to `selectSession`'s "any clear disarms the reconnect
// intent" rule, for the center pane's transient reconnect-eject and never for a
// user-initiated navigation. It marks this `selectSession(null)` as our own
// eject, so `restoreReconnectDeepLink` can tell it apart from a deliberate home
// navigation: only the former is undone once the agent is `active` again.
export function ejectSelectionForReconnect(): void {
  // A replace, not a push: the eject is transient (the reconnect re-restore
  // undoes it), so it must not leave a home entry between the user and the
  // agent they were on.
  selectSessionRoute(null, "replace")
  lastClearWasReconnectEject = true
}

// Focus a specific provider tab of a session. Naming the session-slot tab is
// equivalent to `selectSession`. The changed files belong to the SESSION, so the
// subscription/fetch key off `sessionId` regardless of tab.
//
// Persists the choice as the agent's remembered tab-focus, fire and forget, so
// a later `selectSession` restores it on any client of the same server. Pass
// `persist: false` for a selection that must not rewrite that shared memory,
// such as following a link rather than stating an intent.
export function selectTab(
  sessionId: string,
  tabId: string,
  opts?: {
    persist?: boolean
    urlMode?: "replace"
    changes?: boolean
    // Same contract as `selectSessionRoute`'s: a patch committed in the same
    // setState, threaded through when a remembered tab diverts an
    // `openEditor` selection here.
    extra?: Partial<DuxState>
    // The mode the switch carries. A tab switch made from inside theater must
    // not consult the destination's memory: the user asked for a different tab
    // in a full-screen pane, so the mode follows and the destination remembers
    // it. The same override the route path arms, stated at the call site.
    theater?: boolean
  },
): void {
  const prev = state.selectedSessionId
  setState(
    withTheaterLayout({
      selectedTarget: { kind: "agent", sessionId, tabId },
      selectedSessionId: sessionId,
      ...theaterPatch({ kind: "agent", sessionId, tabId }, opts?.theater),
      changes: prev === sessionId ? state.changes : loadingChanges(sessionId),
      ...editorSelectionPatch({ kind: "agent", sessionId, tabId }),
      ...screenPatch(opts?.changes),
      ...(opts?.extra ?? {}),
    }),
  )
  switchChangesSubscription(prev, sessionId)
  syncUrl(opts?.urlMode)
  if (prev !== sessionId) loadChanges(sessionId)
  if (opts?.persist === false) return
  persistFocusedTab(sessionId, isSlotTabOf(sessionId, tabId) ? null : tabId)
}

// Per-session bookkeeping for the fire-and-forget focus-tab PUT: only the
// latest intended `(generation, tabId)` is kept. Rapid tab switching settles
// out of order, so a stale generation whose value differs from the current
// intent re-issues a PUT (`shouldRefireFocusPut`), keeping the server's last
// write equal to the user's last click.
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
export function selectTerminal(
  terminalId: string,
  owner: TerminalOwnerRef,
  opts?: {
    urlMode?: "replace"
    changes?: boolean
    // Same contract as `selectSessionRoute`'s: a patch committed in the same
    // setState, so opening a terminal's editor is one navigation and one
    // history entry rather than a selection followed by an open.
    extra?: Partial<DuxState>
  },
): void {
  const prev = state.selectedSessionId
  // Lossy on purpose: `selectedSessionId` exists to scope session-only UI, so
  // "is this owner a session" is the entire question. Any other owner leaves it
  // null, which is exactly the state a project terminal already puts it in.
  const sessionId = ownerSessionId(owner)
  setState(
    withTheaterLayout({
      selectedTarget: { kind: "terminal", terminalId, owner },
      selectedSessionId: sessionId,
      ...theaterPatch({ kind: "terminal", terminalId, owner }),
      // Switching from the agent to one of its own terminals keeps the same
      // session's loaded changes; only a different session (or a project
      // terminal, which has none) enters loading/empty.
      changes:
        sessionId === null
          ? emptyChanges()
          : prev === sessionId
            ? state.changes
            : loadingChanges(sessionId),
      // A terminal of the editor's own session keeps the editor, and so does
      // the terminal whose OWN editor is open; anything else closes it, the
      // same rule as an agent selection.
      ...editorSelectionPatch({ kind: "terminal", terminalId, owner }),
      ...screenPatch(opts?.changes),
      ...(opts?.extra ?? {}),
    }),
  )
  // The changed files belong to the SESSION, so subscribe/fetch the parent
  // session even when a companion terminal is the streamed target; a project
  // terminal drops the subscription entirely.
  switchChangesSubscription(prev, sessionId)
  syncUrl(opts?.urlMode)
  if (sessionId !== null && prev !== sessionId) loadChanges(sessionId)
}

// Spawn a new companion terminal for a session via REST. The 201 reply
// carries the new terminal id, so we focus it immediately, opening its PTY
// socket (`TerminalPane`), rather than waiting for a `terminal_created` frame.
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
      notifyError(
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
      notifyError(
        e instanceof Error ? e.message : "Could not create the project terminal.",
      ),
    )
}

// Spawn a new STANDALONE terminal (a plain shell in the user's home directory,
// owned by neither an agent nor a project) via REST, then focus it, mirroring
// `createTerminal`. It takes no id, because nothing has to exist first.
export function createStandaloneTerminal(): void {
  terminalsApi
    .createStandalone()
    .then((created) =>
      selectTerminal(created.terminal_id, { kind: "standalone" }),
    )
    .catch((e) =>
      notifyError(
        e instanceof Error
          ? e.message
          : "Could not create the standalone terminal.",
      ),
    )
}

// Open the close-terminal confirmation dialog for a companion terminal. The TUI
// always confirms before killing a terminal's running process, so the web does
// too.
export function openDeleteTerminal(terminalId: string): void {
  setState({ deleteTerminalTarget: terminalId })
}

export function closeDeleteTerminal(): void {
  setState({ deleteTerminalTarget: null })
}

// Resolve a terminal's owner from the spine: the terminal carries its owner, so
// this is a lookup by id and a conversion. Undefined when the terminal has
// already vanished.
export function findTerminalOwner(
  terminalId: string,
): TerminalOwnerRef | undefined {
  const terminal = state.spine?.terminals.find((t) => t.id === terminalId)
  return terminal ? ownerRefFromWire(terminal.owner) : undefined
}

// The DELETE endpoint for a terminal is nested under its owner, so which URL to
// call is an owner decision and gets an exhaustive switch of its own.
function terminalDeleteRequest(
  owner: TerminalOwnerRef,
  terminalId: string,
): Promise<void> {
  switch (owner.kind) {
    case "session":
      return terminalsApi.remove(owner.sessionId, terminalId)
    case "project":
      return terminalsApi.removeForProject(owner.projectId, terminalId)
    // Un-nested, because there is no owner to nest under.
    case "standalone":
      return terminalsApi.removeStandalone(terminalId)
    default:
      return assertNever(owner)
  }
}

// Close a terminal via REST. The owner is resolved from the spine across every
// owner kind, since a session-only scan would make project terminals
// undeletable, and a terminal that already vanished is a no-op. A focused
// terminal's selection clears through the spine prune, not from here.
export function deleteTerminal(terminalId: string): void {
  const owner = findTerminalOwner(terminalId)
  if (owner === undefined) return
  const request = terminalDeleteRequest(owner, terminalId)
  request.catch((e) =>
    notifyError(
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
      notifyError(e instanceof Error ? e.message : "Could not create the tab.")
    })
}

// Open the close-tab confirmation; closing always confirms. Two gestures do not
// come here: an agent's only tab, whose close the server refuses and whose menu
// item is disabled with the reason, and the Task Manager's first-tab row, which
// is a Stop and routes to `openStopAgent`.
export function openCloseTab(sessionId: string, tabId: string): void {
  setState({ closeTabTarget: { sessionId, tabId } })
}

export function closeCloseTab(): void {
  setState({ closeTabTarget: null })
}

// Open the stop-agent confirmation. The Task Manager's row for an agent's first
// tab is a Stop control, not a close: what the user is asking for on a process
// monitor is to end the process the row is showing numbers for, not to delete
// the tab it runs in. `killSessionPty` behind it stops that tab's provider and leaves the agent
// in the list.
export function openStopAgent(sessionId: string): void {
  setState({ stopAgentTarget: sessionId })
}

export function closeStopAgent(): void {
  setState({ stopAgentTarget: null })
}

// Close a tab via REST. Any tab may go, the slot tab included: the server hands
// the slot on and names the successor back as `promoted`, and reports
// `{ detached }` when the close took the agent's last live tab. An agent's only
// tab is refused by the engine.
//
// Nothing is optimistic: focus and latches move only once the DELETE resolves,
// or a failed request would leave the UI navigated away from a live tab with no
// rollback. Focus must leave the closed tab, since subscribing relaunches it.
export function closeTab(sessionId: string, tabId: string): void {
  tabsApi
    .remove(sessionId, tabId)
    .then((closed) => {
      dropTabStarted(tabId)
      // Record the promotion before anything reads the slot: the spine is still
      // the pre-close one here, so `slotTabIdFor` would otherwise answer with
      // the tab that was just deleted.
      if (closed?.promoted) {
        setState({
          pendingSlotTab: {
            ...state.pendingSlotTab,
            [sessionId]: {
              closedTabId: tabId,
              promotedTabId: closed.promoted,
            },
          },
        })
      }
      const target = state.selectedTarget
      const focused =
        target?.kind === "agent" &&
        target.sessionId === sessionId &&
        target.tabId === tabId
      if (!focused) return
      // Focus whichever tab holds the slot NOW (the promoted one after a slot
      // close, the unchanged one after an extra tab's) DIRECTLY via
      // `selectTab`, not `selectSession`: the spine may still be stale at this
      // point (no `sessions.changed` refetch has pruned it yet), and
      // `selectSession` would resolve the remembered tab against that stale
      // spine, which can still name the tab we just deleted.
      selectTab(sessionId, slotTabIdFor(sessionId))
    })
    .catch((e) =>
      notifyError(e instanceof Error ? e.message : "Could not close the tab."),
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
    notifyError(providerNotConfigured(provider))
    return false
  }
  try {
    await tabsApi.patch(sessionId, tabId, provider)
    return true
  } catch (e) {
    notifyError(e instanceof Error ? e.message : "Could not change the provider.")
    return false
  }
}

// Explicitly start a dormant tab from its dormant card. Selection is immediate,
// because it is what the press meant and navigating away mid-flight must stick.
// The latch waits for the server's answer, so the card stays up and no PTY
// socket opens until the launch is dispatched: the socket path refuses a tab
// whose last run failed, and this route is the only way past that. A refusal
// leaves the card where it is and says why.
export function startDormantTab(sessionId: string, tabId: string): void {
  selectTab(sessionId, tabId)
  tabsApi
    .start(sessionId, tabId)
    .then(() => markTabStarted(tabId))
    .catch((e) =>
      notifyError(e instanceof Error ? e.message : "Could not start the tab."),
    )
}

// Latch a tab as started by this client so the dormant card does not sit in
// front of a launch already on its way. Only the card's own button needs it,
// for the gap between the server accepting the start and the spine reporting
// the tab live.
//
// `applyWorkspace` drops it when the tab goes live and equally when the spine
// says the run failed, which is the same launch coming back with a verdict.
// There is no timer: neither outcome arriving is not a state a clock improves.
function markTabStarted(tabId: string): void {
  if (state.startedDormantTabs.includes(tabId)) return
  setState({ startedDormantTabs: [...state.startedDormantTabs, tabId] })
}

function dropTabStarted(tabId: string): void {
  if (!state.startedDormantTabs.includes(tabId)) return
  setState({
    startedDormantTabs: state.startedDormantTabs.filter((t) => t !== tabId),
  })
}

// An extra tab's PTY socket found the tab gone (`isTabGone`), closed by another
// client mid-retry. The route will keep 404ing, so the started-dormant latch is
// cleared here: `applyWorkspace` only clears it once a tab goes live, which this
// one never will. The toast says why the pane stopped retrying.
export function handleTabGone(tabId: string): void {
  dropTabStarted(tabId)
  notifyError("This tab was closed elsewhere.")
}

// Open the discard-confirmation dialog for an unstaged file. The TUI confirms
// every discard because it's destructive: an untracked file is deleted, a
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
    .catch((e) => notifyError(e instanceof Error ? e.message : "discard failed"))
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

// Open the code-editor overlay for a session. Selecting the session first
// points the engine's changed-files watch at its worktree, so the file list
// comes from the same broadcast the changes pane uses. `initialPath` is seeded
// through `editorOpenFile`, so an external open uses the same preview model as
// the tree. Does not clear the session's tab list: `editorTabs` outlives
// `closeEditor` and only `editorClearSession` drops it.
export function openEditor(
  root: EditorRoot,
  initialPath: string | null = null,
  mode: EditorViewMode = "file",
  opts?: { urlMode?: "replace" },
): void {
  // An image path never opens in diff mode: there is no text to diff, so a
  // changed image clicked in the Changes pane (which asks for "diff") must
  // show the picture rather than dead-end on the binary-diff refusal. This
  // is the open choke point; `editorOpenFile` coerces too, and the render
  // keeps the image arm above the diff arm as defense in depth.
  const effectiveMode: EditorViewMode = editorMode(root, mode, initialPath)
  const editorPatch: Partial<DuxState> & { theater: boolean } = {
    editorTarget: { root, initialPath, initialMode: effectiveMode },
    editorRoute: { root, mode: effectiveMode, path: initialPath },
    ...theaterSuspendPatch(),
  }
  // Tab seeding first: it touches only `editorTabs`, which the URL never
  // serializes, so ordering it before the route commit changes nothing the
  // history sees.
  if (initialPath !== null)
    editorOpenFile(root, initialPath, { mode: effectiveMode })
  // Opening the editor pushes, so one Back closes it and lands where it was
  // opened from. A selection move and the editor open are one navigation, so
  // they commit as one setState and one `syncUrl`, never two pushes with a
  // never-visited agent screen between. `urlMode: "replace"` is for restores.
  //
  // A terminal root selects its terminal rather than a session: the editor's
  // address rides on the target's hash, so the two must name the same thing.
  if (root.kind === "terminal") {
    const selected = state.selectedTarget
    if (
      selected === null ||
      selected.kind !== "terminal" ||
      selected.terminalId !== root.terminalId
    ) {
      selectTerminal(root.terminalId, root.owner, {
        urlMode: opts?.urlMode,
        extra: editorPatch,
      })
      return
    }
    setState(withTheaterLayout(editorPatch))
    syncUrl(opts?.urlMode)
    return
  }
  if (state.selectedSessionId !== root.sessionId) {
    selectSessionRoute(root.sessionId, opts?.urlMode, undefined, editorPatch)
    return
  }
  setState(withTheaterLayout(editorPatch))
  syncUrl(opts?.urlMode)
}

export function closeEditor(opts?: { urlMode?: "replace" }): void {
  if (state.editorTarget === null && state.editorRoute === null) return
  // The surface flag drops with the editor state, the same rule every
  // non-popstate clear follows (see `clearEditorStateSilently`): a standalone
  // shell kept up over a closed editor is the boot spinner forever. Unreachable
  // from the standalone UI today (its body hides the Close button), so this is
  // defense in depth.
  setState(
    withTheaterLayout({
      editorTarget: null,
      editorRoute: null,
      standaloneEditor: false,
      // Landing back on the pane restores whatever mode it remembers, which is
      // the other half of the suspend `openEditor` wrote.
      ...theaterResumePatch(),
    }),
  )
  // Closing is a move too (Esc, the Close button): the push-key drops its
  // editor bit, so this pushes the closed position like any other navigation
  // between two real places. The popstate path never comes through here (see
  // `syncEditorStateFromRoute`), so Back itself writes nothing.
  syncUrl(opts?.urlMode)
}

// EditorBody reports its active tab here on every change of mode/path, which
// is what keeps the URL naming the file actually on screen. Same push key
// before and after (the editor stays open), so `syncUrl` REPLACES: switching
// files inside the editor never piles up history entries. A report for a
// session whose editor is not open is dropped: a late effect from an
// unmounting body must not resurrect a closed editor's suffix.
export function editorSyncActiveTab(
  root: EditorRoot,
  mode: EditorViewMode,
  path: string | null,
): void {
  const cur = state.editorRoute
  if (cur === null || !sameRoot(cur.root, root)) return
  if (cur.mode === mode && cur.path === path) return
  setState({ editorRoute: { root, mode, path } })
  syncUrl()
}

// --- Editor tabs: thin store wrappers over the pure reducer (lib/editorTabs.ts).
// Each mutates only `editorTabs[rootKey(root)]`, leaving every other root's
// tabs untouched. Components/dialogs call ONLY these, never the pure functions
// directly, so the store stays the single place that knows how to read/write
// the per-root slice.

export function editorTabsFor(root: EditorRoot): EditorTabsState {
  return state.editorTabs[rootKey(root)] ?? emptyTabsState()
}

// Skips `setState` when `next` is reference-equal to the session's current
// tabs state, which is what a reducer returns when nothing changed. `useDux()`
// is an unselective `useSyncExternalStore`, so every consumer re-renders on
// every `setState`, and a no-op dispatch on each keystroke would fan out a
// global re-render for nothing.
function setEditorTabsFor(root: EditorRoot, next: EditorTabsState): void {
  const key = rootKey(root)
  if (state.editorTabs[key] === next) return
  setState({ editorTabs: { ...state.editorTabs, [key]: next } })
  // A tab that no longer exists takes its cached draft with it (per-tab
  // discard confirmed, a deleted file closing its tabs, a rename collision),
  // whether or not an EditorBody is mounted at the time. And the unload
  // guard tracks the STORE dirty flags, which outlive the editor body: it
  // stays armed while the editor is closed over a dirty cached draft, and a
  // discard that clears the last flag disarms it.
  pruneRootDrafts(key, new Set(next.tabs.map((t) => t.id)))
  syncBeforeUnloadGuard(hasAnyDirtyTab(state.editorTabs))
}

// Open, activate or preview-replace a file in a session's tab list; the
// promotion rules live in `openFile` in `lib/editorTabs.ts`. `opts.mode` is an
// explicit mode intent and retargets an already-open tab; omit it for a plain
// activation, so re-clicking an open path never flips its diff view back.
export function editorOpenFile(
  root: EditorRoot,
  path: string,
  opts: { mode?: EditorViewMode; pin?: boolean } = {},
): void {
  // An image path never opens or retargets into diff mode (see openEditor's
  // comment); an undefined mode stays undefined so a plain activation keeps
  // its no-intent semantics.
  const mode =
    opts.mode !== undefined ? editorMode(root, opts.mode, path) : undefined
  setEditorTabsFor(
    root,
    editorOpenFilePure(editorTabsFor(root), path, {
      mode,
      pin: opts.pin,
      newId: () => newClientId(),
    }),
  )
}

// The mode a tab may actually open in. Two coercions, both to "file", and both
// because the alternative is a dead end rather than a view: an image has no
// text to diff, and a terminal root has no diff at all (see `rootHasDiff`).
function editorMode(
  root: EditorRoot,
  mode: EditorViewMode,
  path: string | null,
): EditorViewMode {
  if (!rootHasDiff(root)) return "file"
  return path !== null && isImagePreviewPath(path) ? "file" : mode
}

export function editorActivateTab(root: EditorRoot, tabId: string): void {
  setEditorTabsFor(root, editorActivateTabPure(editorTabsFor(root), tabId))
}

// Promote a tab to permanent: double-click on the row/pill, or the tab's first
// edit (a dirty preview tab is promoted so an edit is never silently discarded
// by a later preview-replace).
export function editorPinTab(root: EditorRoot, tabId: string): void {
  setEditorTabsFor(root, editorPinTabPure(editorTabsFor(root), tabId))
}

// Mirrors the buffer's dirty state up to the store so the strip's dot and the
// close-confirm gating read from one place, without putting file contents in
// the global store (see lib/editorTabs.ts header comment).
export function editorSetTabDirty(
  root: EditorRoot,
  tabId: string,
  dirty: boolean,
): void {
  setEditorTabsFor(root, editorSetTabDirtyPure(editorTabsFor(root), tabId, dirty))
}

export function editorSetTabMode(
  root: EditorRoot,
  tabId: string,
  mode: EditorViewMode,
): void {
  setEditorTabsFor(root, editorSetTabModePure(editorTabsFor(root), tabId, mode))
}

// Unconditional close (post-confirm, or the tab was clean). Picks the next
// active tab via the VS Code right-then-left rule.
export function editorCloseTab(root: EditorRoot, tabId: string): void {
  setEditorTabsFor(root, editorCloseTabPure(editorTabsFor(root), tabId))
}

// Rename retarget: rewrite the path of the tab(s) affected by renaming a file
// or folder from `from` to `to`. See `lib/editorTabs.ts` `renameTabPaths` for
// the folder-prefix rewrite and the pre-existing-destination-tab collision
// close.
export function editorRenameTabPaths(
  root: EditorRoot,
  from: string,
  to: string,
): void {
  setEditorTabsFor(root, editorRenameTabPathsPure(editorTabsFor(root), from, to))
}

// Close every tab under a deleted file or folder path. See
// `lib/editorTabs.ts` `closeTabsUnderPath`.
export function editorCloseTabsUnderPath(root: EditorRoot, path: string): void {
  setEditorTabsFor(root, editorCloseTabsUnderPathPure(editorTabsFor(root), path))
}

// Drop all of a root's editor tabs, because the thing it was rooted at is gone
// from the spine. See the `editorTabs` prune in `applyWorkspace`, which calls
// this for any key no longer present.
export function editorClearRoot(root: EditorRoot): void {
  clearEditorTabsForKey(rootKey(root))
}

// The same by raw key, for the prune, which is walking keys whose roots it can
// no longer reconstruct: the target they named is exactly what has vanished.
function clearEditorTabsForKey(key: string): void {
  clearRootDrafts(key)
  if (!(key in state.editorTabs)) return
  const next = { ...state.editorTabs }
  delete next[key]
  setState({ editorTabs: next })
  syncBeforeUnloadGuard(hasAnyDirtyTab(state.editorTabs))
}

// Open the dirty-tab close confirmation.
export function openEditorCloseTab(root: EditorRoot, tabId: string): void {
  setState({ editorCloseTabTarget: { root, tabId } })
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
// `deleteBranch` is the dialog's branch answer, or `null` when it had no
// checkbox to answer with.
export function deleteSession(
  sessionId: string,
  deleteWorktree: boolean,
  deleteBranch: boolean | null = null,
): void {
  sessionsApi
    .remove(sessionId, deleteWorktree, deleteBranch)
    .catch((e) => {
      // A 409 is a refusal (a tab is still launching, or a delete is already in
      // flight). The server already surfaces that message over the `/ws/events`
      // status stream, so don't toast it a second time. Mirrors
      // `toastCreateError`.
      if (e instanceof SessionsApiError && e.status === 409) return
      notifyError(
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
    notifyError(e instanceof Error ? e.message : "Could not rename the session.")
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

// Open the attach-pull-request dialog for a session with an empty draft. The
// dialog body names the currently shown PR (if any) so overriding is explicit.
export function openAttachPullRequest(sessionId: string): void {
  setState({ attachPullRequestTarget: sessionId, attachPullRequestDraft: "" })
}

export function closeAttachPullRequest(): void {
  setState({ attachPullRequestTarget: null, attachPullRequestDraft: "" })
}

export function setAttachPullRequestDraft(raw: string): void {
  setState({ attachPullRequestDraft: raw })
}

// Submit the attach dialog: fire the PUT and close immediately. The request is
// deferred server-side (202 + op id), so the outcome, the busy, the attached
// info or the lookup error, rides the status toast stream; only a synchronous
// HTTP refusal (gh unavailable, empty reference) is toasted here, matching the
// sibling deferred actions (e.g. deleteSession).
export function submitAttachPullRequest(): void {
  const id = state.attachPullRequestTarget
  if (!id) return
  const pr = state.attachPullRequestDraft.trim()
  if (!pr) return
  closeAttachPullRequest()
  sessionsApi.attachPullRequest(id, pr).catch((e) => {
    notifyError(
      e instanceof Error ? e.message : "Could not attach the pull request.",
    )
  })
}

// Detach a session's pull request: the pin goes if there is one, the badge
// clears, and dux stops looking for a PR on this agent. No confirm: nothing is
// destroyed and there are two ways back (attach one by hand, or resume
// autodetection from the same menu), so a dialog would only be in the way. The
// server's info status rides the toast stream.
export function detachPullRequest(sessionId: string): void {
  sessionsApi.detachPullRequest(sessionId).catch((e) => {
    notifyError(
      e instanceof Error ? e.message : "Could not detach the pull request.",
    )
  })
}

// The way back from a detach: autodetection is switched on again and one check
// runs now. No confirm, for the same reason as the detach it undoes.
export function resumePullRequestAutodetection(sessionId: string): void {
  sessionsApi.resumePullRequestAutodetection(sessionId).catch((e) => {
    notifyError(
      e instanceof Error
        ? e.message
        : "Could not resume pull-request autodetection.",
    )
  })
}

// Open the change-provider dialog for a session. The dialog pre-selects the
// session's current provider from the ViewModel.
export function openChangeProvider(sessionId: string): void {
  setState({ changeProviderTarget: sessionId })
}

export function closeChangeProvider(): void {
  setState({ changeProviderTarget: null })
}

// Whether `provider` is in the bootstrap document's configured provider list.
// The server re-validates, but checking first matters for the multi-field
// project PATCH, which is not atomic: a provider rejected mid-sequence would
// leave the rename and auto-reopen already committed. An empty list, before the
// bootstrap lands, treats every provider as unconfigured.
function providerIsConfigured(provider: string): boolean {
  return (state.bootstrap?.available_providers ?? []).includes(provider)
}

// The same refusal the server gives, so a pre-flight toast and a refused
// request read alike.
function providerNotConfigured(provider: string): string {
  return `Provider "${provider}" is not configured. Pick one of the configured providers.`
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
    notifyError(providerNotConfigured(provider))
    return false
  }
  try {
    await sessionsApi.patch(sessionId, { provider })
    return true
  } catch (e) {
    notifyError(e instanceof Error ? e.message : "Could not change the provider.")
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
      notifyError(
        e instanceof Error ? e.message : "Could not update auto-reopen.",
      ),
    )
}

// Ask the server to reconnect (relaunch) an agent. `force` starts a fresh
// session with no resume args; the default resumes the prior conversation where
// the provider supports it. `terminalEpoch` is bumped so the pane remounts and
// re-subscribes: the reconnect swaps in a new provider and the attached
// forwarder is dead, so even an already-focused pane must re-issue `subscribe`.
export function reconnectSession(sessionId: string, force: boolean): void {
  sessionsApi
    .reconnect(sessionId, force)
    .catch((e) =>
      notifyError(
        e instanceof Error ? e.message : "Could not reconnect the session.",
      ),
    )
  // No latch: the reconnect dispatches a launch server-side, and dispatching is
  // what clears the tab's recorded failure, so the card cannot show for the
  // reconnect's own tab.
  const reconnectTarget = {
    kind: "agent" as const,
    sessionId,
    // Reconnect is a session-slot-tab operation, so focus the session-slot tab.
    tabId: slotTabIdFor(sessionId),
  }
  setState(
    withTheaterLayout({
      selectedTarget: reconnectTarget,
      selectedSessionId: sessionId,
      // This IS a selection commit, so it restores the pane's remembered mode
      // like every other one. Skipping it left the flag saying whatever the
      // previously focused pane said, and the URL was written from that.
      ...theaterPatch(reconnectTarget),
      terminalEpoch: state.terminalEpoch + 1,
    }),
  )
  // This focuses an agent like any other selection, so the URL has to say so
  // too: a position the address bar does not name is a position Back cannot
  // return to.
  syncUrl()
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
      notifyError(
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
// is project-scoped; there is no per-agent startup command).
export function openAgentStartupCommand(sessionId: string): void {
  setState({ agentStartupCommandTarget: sessionId })
}

export function closeAgentStartupCommand(): void {
  setState({ agentStartupCommandTarget: null })
}

// Open the agent-scoped environment editor. The target is the SESSION id; the
// dialog resolves and edits that agent's PROJECT env (env is project-scoped; it
// applies to every agent and terminal in the project).
export function openAgentEnv(sessionId: string): void {
  setState({ agentEnvTarget: sessionId })
}

export function closeAgentEnv(): void {
  setState({ agentEnvTarget: null })
}

// Whether the viewer is still pointed at the (scope, id) a reply was issued for.
// Both halves matter: session ids and project ids live in separate namespaces,
// so an id alone could theoretically match across a scope switch and let a late
// agent-scope reply repopulate a project-scope viewer.
function startupLogsStillTargets(
  scope: StartupLogsScope,
  id: string,
): boolean {
  return state.startupLogsScope === scope && state.startupLogsTarget === id
}

// The REST pair for a scope. The two clients return the same shapes, which is
// what lets one viewer (and one set of store actions) serve both.
function startupLogsClient(scope: StartupLogsScope) {
  return scope === "project" ? projectsApi : sessionsApi
}

// Open the startup-command log viewer for `id` in `scope` and fetch its log
// files (with the newest file's contents pre-loaded). A reply is ignored once
// the viewer has closed or retargeted, so a late frame can't repopulate a stale
// viewer (the browse/attach-worktree precedent).
function loadStartupLogs(scope: StartupLogsScope, id: string): void {
  setState({
    startupLogsScope: scope,
    startupLogsTarget: id,
    startupLogsEntries: [],
    startupLogsSelected: null,
    startupLogsError: null,
    startupLogsLoading: true,
  })
  startupLogsClient(scope)
    .startupLogs(id)
    .then((res) => {
      if (!startupLogsStillTargets(scope, id)) return
      setState({
        startupLogsEntries: res.entries,
        startupLogsSelected: res.selected,
        startupLogsError: null,
        startupLogsLoading: false,
      })
    })
    .catch((e) => {
      if (!startupLogsStillTargets(scope, id)) return
      setState({
        startupLogsLoading: false,
        startupLogsError:
          e instanceof Error
            ? e.message
            : "Could not load the startup command logs.",
      })
    })
}

// Agent scope: one agent's runs, from the agent row's ⋯ menu.
export function openStartupLogs(sessionId: string): void {
  loadStartupLogs("agent", sessionId)
}

// Project scope: every run across every agent of the project, from the project
// row's ⋯ menu. The TUI reaches the same scope by running
// `read-startup-command-logs` with a project (not an agent) selected.
export function openProjectStartupLogs(projectId: string): void {
  loadStartupLogs("project", projectId)
}

// Switch the viewer to a different log file (fetches that file's contents from
// whichever scope is open).
export function selectStartupLog(name: string): void {
  const id = state.startupLogsTarget
  if (!id) return
  const scope = state.startupLogsScope
  setState({ startupLogsLoading: true, startupLogsError: null })
  startupLogsClient(scope)
    .startupLogContent(id, name)
    .then((res) => {
      if (!startupLogsStillTargets(scope, id)) return
      setState({ startupLogsSelected: res, startupLogsLoading: false })
    })
    .catch((e) => {
      if (!startupLogsStillTargets(scope, id)) return
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
    // Back to the default scope, so a closed viewer never leaves "project"
    // behind for a later agent-scope open to trip over.
    startupLogsScope: "agent",
    startupLogsTarget: null,
    startupLogsEntries: [],
    startupLogsSelected: null,
    startupLogsLoading: false,
    startupLogsError: null,
  })
}

// Re-run the agent's project startup command in its worktree (the TUI's
// `rerun-startup-command-on-agent`). The server runs it off-thread and reports
// busy/success/failure on the status stream; nothing to do here but fire the
// command and surface a transport/validation error if the request is rejected.
export function rerunStartupCommand(sessionId: string): void {
  sessionsApi
    .rerunStartupCommand(sessionId)
    .catch((e) =>
      notifyError(
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

// Browse a directory for the add-project picker over REST. A null path resolves
// the server's configured default start directory. The reply is ignored once the
// dialog has closed so a late response can't repopulate a closed picker.
/** Whether SOME folder picker is still open. The browse reply is dropped
 * otherwise, so a late response cannot repopulate a closed picker. Both
 * pickers share the slice, so both must be consulted. */
function browsingOpen(): boolean {
  return state.addProjectOpen || state.standaloneAgentPickerOpen
}

function runBrowse(path: string | null): void {
  browseApi
    .browse(path)
    .then((res) => {
      if (!browsingOpen()) return
      setState({
        browsePath: res.path,
        browseEntries: res.entries,
        browseLoading: false,
      })
    })
    .catch((e) => {
      if (!browsingOpen()) return
      setState({ browseEntries: [], browseLoading: false })
      notifyError(
        e instanceof Error ? e.message : "Could not browse the directory.",
      )
    })
}

/** Open the standalone-agent folder picker at the server's configured default
 * start directory. Deliberately no inspection, unlike the add-project picker:
 * a project must be a repository, while a standalone agent accepts whatever is
 * there, so there is nothing to check. */
export function openStandaloneAgentPicker(): void {
  setState({
    standaloneAgentPickerOpen: true,
    browseLoading: true,
    browseEntries: [],
  })
  runBrowse(null)
}

export function closeStandaloneAgentPicker(): void {
  setState({ standaloneAgentPickerOpen: false })
}

/** Create a standalone agent in `folder`, with an optional display name.
 *
 * Every refusal (a relative path, a folder that already hosts one) is the
 * server's, shared with the terminal UI, so the two surfaces cannot answer
 * differently. */
export function createStandaloneAgent(folder: string, name: string): void {
  // The same auto-focus token every other creation path arms, scoped to "an
  // agent with no project" because that is what a standalone agent is. Without
  // it the agent lands in the sidebar and the URL stays where it was, which is
  // the one creation gesture that did not take the user to what it made.
  armCreateFocus({ kind: "standalone" })
  void sessionsApi
    .create({ kind: "standalone", folder, name })
    .catch((e) =>
      notifyError(
        e instanceof Error
          ? e.message
          : "Could not create the standalone agent.",
      ),
    )
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

// Open the same picker with the "init" intent (the launcher corner's
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
  // Only this dialog's own flag: the standalone-agent picker is a separate
  // dialog with its own close (`closeStandaloneAgentPicker`), and the two are
  // never open at once.
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
// fills `projectPathInspection` when it lands; the dialog shows
// a warning step when it carries one. Runs in the click handler that selects the
// repo, never an effect, like `openAttachWorktree` kicks off its listing.
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
  // Resolve over REST. Ignore a stale reply whose path no longer matches the pending inspection (the
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

export function addProject(path: string, name: string): void {
  projectsApi
    .create({ path, name })
    .catch(toastAddProjectError)
}

// Check out the repo's default branch first, then add it; the TUI's
// "Check Out & Add" path. Only offered for the Known warning (the server
// re-validates and rejects otherwise). The switch + add run server-side through
// the worker chain; the status stream reports the outcome.
export function addProjectCheckoutDefault(path: string, name: string): void {
  projectsApi
    .create({ path, name, checkout_default: true })
    .catch(toastAddProjectError)
}

// Birth an unborn repo (fresh `git init`, no commits) with an empty initial
// commit, then add it; the server creates the commit before registering so the
// repo can back worktrees. Offered when inspect reports `hasCommits: false`.
export function addProjectCreateInitialCommit(path: string, name: string): void {
  projectsApi
    .create({ path, name, create_initial_commit: true })
    .catch(toastAddProjectError)
}

// Adopt a plain (non-repo) folder: the server runs `git init`, seeds a starter
// .gitignore, creates an empty initial commit, then registers the project.
// Offered when inspect reports `kind: "plain"`. Fire-and-forget like the other
// add variants; the keyed status stream reports the outcome.
export function initProject(path: string, name: string): void {
  projectsApi
    .create({ path, name, init_repo: true })
    .catch(toastAddProjectError)
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
      notifyError(e instanceof Error ? e.message : "Could not remove the project."),
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
      notifyError(e instanceof Error ? e.message : "Could not delete the project."),
    )
}

// Update a project's settings (provider / auto-reopen / startup-command / env)
// in one tri-state PATCH. The caller (ProjectSettingsDialog) includes only the
// fields that changed; an omitted field is left untouched, `null` clears it.
export async function updateProjectSettings(
  projectId: string,
  patch: PatchProjectBody,
): Promise<boolean> {
  // Empty patch (nothing changed) is a successful no-op, let the dialog close.
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
    notifyError(providerNotConfigured(patch.provider))
    return false
  }
  try {
    await projectsApi.patch(projectId, patch)
    return true
  } catch (e) {
    notifyError(
      e instanceof Error ? e.message : "Could not update project settings.",
    )
    return false
  }
}

// Refresh a project's source checkout from remote (the TUI's
// `refresh_selected_project`). The server resolves the project, runs the pull
// against its source checkout, and reports busy/success/failure on the status
// stream; nothing to do here but fire the command.
export function pullProject(projectId: string): void {
  projectsApi
    .pull(projectId)
    .catch((e) => notifyError(e instanceof Error ? e.message : "pull failed"))
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
      notifyError(e instanceof Error ? e.message : "checkout failed")
    )
}

// Open the attach-worktree dialog for a project and immediately request its
// managed-worktree listing (the server classifies in spawn_blocking). The
// listing reply fills `attachWorktreeEntries` when it lands. Runs in
// the click handler that opens the dialog, never an effect, mirroring how
// `openAddProject` kicks off its browse.
export function openAttachWorktree(
  projectId: string,
  fromPicker = false,
): void {
  setState({
    attachWorktreeTarget: projectId,
    attachWorktreeEntries: [],
    attachWorktreeLoading: true,
    attachWorktreeFromPicker: fromPicker,
  })
  loadProjectWorktrees(projectId)
}

// Fetch the managed-worktree listing over REST. Ignore a stale reply if
// the dialog closed (or switched projects) before it arrived. Shared by the
// opener and by the post-delete refresh so the list can never drift from what
// the server would answer.
function loadProjectWorktrees(projectId: string): void {
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
      notifyError(
        e instanceof Error ? e.message : "Could not list the worktrees.",
      )
    })
}

// Fetch the per-project managed-worktree counts for the project picker's row
// labels. Never toasts: an unavailable count degrades to no label at all, which
// is strictly better than a scary error for a decoration.
export function loadProjectWorktreeCounts(): void {
  projectsApi
    .worktreeCounts()
    .then((res) => setState({ projectWorktreeCounts: res.counts }))
    .catch(() => setState({ projectWorktreeCounts: null }))
}

// Arm the delete confirmation for one worktree. Only ever called from an
// ADOPTABLE row: an attached worktree has no delete action, because removing it
// from under a live agent leaves a broken session and deleting the agent is the
// supported route. The server refuses it too.
export function openDeleteWorktree(
  projectId: string,
  entry: ProjectWorktreeEntryView,
): void {
  setState({ deleteWorktreeTarget: { projectId, entry } })
}

export function closeDeleteWorktree(): void {
  setState({ deleteWorktreeTarget: null })
}

// Remove one managed worktree from disk, then reload the listing so the row
// disappears (and so a refusal the client did not predict shows up as the
// server's answer rather than a stale row).
export function deleteProjectWorktree(
  projectId: string,
  worktreePath: string,
  deleteBranch: boolean,
): void {
  projectsApi
    .deleteWorktree(projectId, worktreePath, deleteBranch)
    .then((reply) => {
      // The report comes from the SERVER'S answer, never from `deleteBranch`:
      // that flag is what was asked for, and `git branch -D` refuses a branch
      // that is still checked out somewhere. See lib/worktreeDelete.ts.
      const report = worktreeDeleteReport(worktreePath, reply)
      notify(report.tone, report.message, { sticky: report.sticky })
      if (state.attachWorktreeTarget === projectId) {
        loadProjectWorktrees(projectId)
      }
      loadProjectWorktreeCounts()
    })
    .catch((e) =>
      notifyError(
        e instanceof Error ? e.message : "Could not remove the worktree.",
      ),
    )
}

export function closeAttachWorktree(): void {
  setState({
    attachWorktreeTarget: null,
    attachWorktreeEntries: [],
    attachWorktreeLoading: false,
    attachWorktreeFromPicker: false,
    deleteWorktreeTarget: null,
  })
}

// Ask the server to adopt a managed worktree as a new agent. The server
// re-validates the path against a fresh classification (never trusting this
// list) and validates `name` as a display name, then dispatches the create
// worker; the outcome (busy/success/failure) arrives on the status stream.
export function attachWorktree(
  projectId: string,
  worktreePath: string,
  name: string,
): void {
  armCreateFocus({ kind: "project", projectId })
  sessionsApi
    .create({ kind: "from_worktree", project_id: projectId, worktree_path: worktreePath, name })
    .catch((e) => toastCreateError(e, "Could not attach the worktree."))
}

// Open the new-agent dialog. The checkbox starts checked when
// `randomize_agent_names_by_default` is set (mirroring the TUI prompt, which
// pre-checks when opened with no initial name); in that case we request a name
// right away so the input previews it. This runs in the click handler that opens
// the dialog, never an effect, so there is no set-state-in-effect.
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

// Open the name dialog in "from PR" mode.
//
// `projectId` is the project-first shape, opened from a project's own menu.
// `null` is the reference-first shape, opened from the global command: no
// project is chosen or asked for, and the reference decides where it lands.
export function openCreateAgentFromPr(projectId: string | null): void {
  openNameDialog({ kind: "pr", projectId })
}

// Request a fresh pet name over REST. The generated name is stashed as well as
// filled, so a later uncheck can tell it from one the user typed. A reply that
// lands after the dialog closed or the box was unchecked is ignored, and a
// failure stops the spinner so a name can be typed by hand.
function requestAgentName(): void {
  browseApi
    .agentName()
    .then((res) => {
      if (state.createAgentTarget !== null && state.createAgentRandomize) {
        setState({
          createAgentDraft: res.name,
          createAgentGeneratedName: res.name,
          createAgentNamePending: false,
          // This fill replaces the name too, so it retires an in-flight
          // resolve for the same reason typing one does.
          ...retireInFlightPrResolve(),
        })
      }
    })
    .catch(() => {
      if (state.createAgentTarget !== null) {
        setState({ createAgentNamePending: false })
      }
    })
}

// Shared opener for every mode of the name dialog. Pre-checks randomize and
// requests a name at once so the input previews it, except in PR mode, where a
// pet name would become the branch the PR head is fetched into. Runs in the
// click handler rather than an effect, so there is no set-state-in-effect.
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
    // A reference typed before a trip through the project picker travels back
    // into the field, so choosing a project never costs the user their text.
    createAgentPrInput:
      target.kind === "pr" ? (state.pendingPrReference ?? "") : "",
    createAgentPrResolving: false,
    createAgentPrError: null,
    // Retargeting the dialog retires whatever resolve was out for the previous
    // one: its answer is about a question this dialog is no longer asking.
    createAgentPrRequestId: null,
    pendingPrReference: null,
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
    createAgentPrResolving: false,
    createAgentPrError: null,
    // Closing retires the resolve. The reply cannot be recalled, so it has to
    // land on nothing when it arrives.
    createAgentPrRequestId: null,
  })
}

// Park a typed pull-request reference so the next PR dialog opens with it
// already in the field. Used by the secondary "or choose an existing project"
// action and by the resolution branches that hand over to the picker.
export function setPendingPrReference(reference: string | null): void {
  setState({ pendingPrReference: reference })
}

// Editing either field retires the active resolve generation so its response
// cannot create an agent from stale submitted text. Return the cancellation
// patch for the caller to fold into the same state write.
function retireInFlightPrResolve(): Partial<DuxState> {
  if (state.createAgentPrRequestId === null) return {}
  return { createAgentPrRequestId: null, createAgentPrResolving: false }
}

// Update the PR-reference field. Free text; unlike the agent name, this is NOT
// sanitized (a PR URL contains slashes, colons, etc.); the server parses it.
export function setCreateAgentPrInput(raw: string): void {
  // Editing the field retires its refusal: the user is answering it.
  setState({
    createAgentPrInput: raw,
    createAgentPrError: null,
    ...retireInFlightPrResolve(),
  })
}

// Update the input as the user types, sanitizing live (space -> dash, drop
// disallowed chars, etc.) exactly like the TUI char map. Editing away from the
// generated name clears the remembered name so a later uncheck keeps the edits.
export function setCreateAgentDraft(raw: string): void {
  const draft = sanitizeAgentName(raw)
  const generated =
    draft === state.createAgentGeneratedName ? state.createAgentGeneratedName : null
  setState({
    createAgentDraft: draft,
    createAgentGeneratedName: generated,
    ...retireInFlightPrResolve(),
  })
}

// Toggle the "Copy uncommitted changes from the project checkout" checkbox.
export function toggleCreateAgentCopyChanges(): void {
  setState({ createAgentCopyChanges: !state.createAgentCopyChanges })
}

// Toggle the "Use randomized pet name" checkbox with the TUI's exact semantics:
//   ON  -> request a fresh name (the reply fills the input).
//   OFF -> clear the input ONLY if it still equals the generated name; otherwise
//          keep the user's edits. Either way, forget the generated name.
export function toggleCreateAgentRandomize(): void {
  if (!state.createAgentRandomize) {
    setState({
      createAgentRandomize: true,
      createAgentNamePending: true,
      ...retireInFlightPrResolve(),
    })
    requestAgentName()
  } else {
    const keepText = state.createAgentDraft !== state.createAgentGeneratedName
    setState({
      createAgentRandomize: false,
      createAgentDraft: keepText ? state.createAgentDraft : "",
      createAgentGeneratedName: null,
      // Unchecking abandons any in-flight request; its reply is dropped
      // (randomize is false by then), so stop the spinner now.
      createAgentNamePending: false,
      ...retireInFlightPrResolve(),
    })
  }
}

// The two create refusals whose message this connection is ALSO told over the
// `/ws/events` status stream, so toasting them here would say the same thing
// twice: 409, the engine's in-flight guard, and 422, an operation that was
// dispatched and failed (the reply body is that failure's own final). Every
// other code, network failures included, is surfaced.
function alreadyOnTheStatusStream(status: number): boolean {
  return status === 409 || status === 422
}

// Surface a create-action REST error as a toast, unless the status stream is
// already carrying it.
function toastCreateError(e: unknown, fallback: string): void {
  if (e instanceof SessionsApiError && alreadyOnTheStatusStream(e.status)) return
  notifyError(e instanceof Error ? e.message : fallback)
}

// The same rule for the project add, whose four entry points all refuse the
// same way.
function toastAddProjectError(e: unknown): void {
  if (e instanceof ProjectsApiError && alreadyOnTheStatusStream(e.status)) return
  notifyError(e instanceof Error ? e.message : "Could not add the project.")
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

// Set the flat-list sort mode and persist it to `config.ui.agent_sort` through
// its dedicated endpoint. The override is optimistic, dropped by
// `applyBootstrap` once config matches, and cleared on failure so the UI snaps
// back to the authoritative value.
export function setAgentSort(sort: FlatSortKey): void {
  setState({ agentSort: sort })
  configApi.setAgentSort(sort).catch((e) => {
    setState({ agentSort: null })
    notifyError(
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
  onlyIds: string[] | null = null,
): void {
  setState({
    newAgentPickerOpen: true,
    newAgentPickerIntent: intent,
    newAgentPickerOnlyIds: onlyIds,
    // Drop a previous answer so a stale count never labels a row while the
    // fresh one is in flight.
    projectWorktreeCounts:
      intent === "from_worktree" ? null : state.projectWorktreeCounts,
  })
  // Only the worktree intent labels its rows with a count, so only it pays for
  // the listing (one git call per project). Kicked off from the click handler
  // that opens the picker, never an effect.
  if (intent === "from_worktree") loadProjectWorktreeCounts()
}

export function closeNewAgentPicker(): void {
  setState({ newAgentPickerOpen: false, newAgentPickerOnlyIds: null })
}

// Dismiss the picker WITHOUT picking anything. Distinct from
// `closeNewAgentPicker`, which a project row calls on its way to opening that
// project's dialog: a parked pull-request reference has to survive that hop and
// must NOT survive this one, or the next from-PR dialog would open prefilled
// with text the user walked away from.
export function dismissNewAgentPicker(): void {
  setState({
    newAgentPickerOpen: false,
    newAgentPickerOnlyIds: null,
    pendingPrReference: null,
  })
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

// Text that names a pull request number and nothing else, with or without the
// `#`. It names no repository, so with no project chosen the server can only
// refuse it, and the refusal belongs next to the field that fixes it.
//
// Deliberately the only shape refused in the browser: the full grammar lives in
// `dux_core::pr_reference` and a second copy here would drift.
export function isBareNumberReference(raw: string): boolean {
  return /^#?\d+$/.test(raw.trim())
}

const BARE_NUMBER_REFUSAL =
  "A pull request number on its own does not say which repository it is in. Paste a link, type owner/repo#123, or choose an existing project below."

// Generations for the reference-first resolve. Module-level rather than in
// state because it must keep counting across a dialog that opens and closes.
let prResolveGeneration = 0

// The reference-first submit: resolve the typed reference to a project, then
// branch on the answer. Three shapes, matching the terminal UI exactly.
//
// The resolve runs per submit and is never cached: the answer changes when an
// address is edited, when git's rewrite configuration changes, and when a
// project's path moves, and nothing the browser can see would say so.
function submitPrReferenceFirst(reference: string, name: string): void {
  if (isBareNumberReference(reference)) {
    // Refused before anything is sent, per the design: no project is chosen,
    // so this names nothing dux could look for.
    setState({ createAgentPrError: BARE_NUMBER_REFUSAL, createAgentPrResolving: false })
    return
  }
  // Stamp this submit. A resubmit bumps the generation, which is what
  // supersedes the reply already out.
  const generation = ++prResolveGeneration
  setState({
    createAgentPrResolving: true,
    createAgentPrError: null,
    createAgentPrRequestId: generation,
  })
  sessionsApi
    .resolvePullRequest(reference)
    .then((resolved) => {
      // The generation guard. A reply that is not the one this dialog is
      // waiting for belongs to a question the user has already replaced (they
      // cancelled, retargeted at a project, submitted a different reference,
      // or EDITED either field, since `reference` and `name` here are the
      // values as they were at submit), and acting on it would create an agent
      // from the old reference and close the dialog showing the new one.
      if (state.createAgentPrRequestId !== generation) return
      setState({ createAgentPrResolving: false, createAgentPrRequestId: null })
      const repository = resolved.repository ?? reference
      if (resolved.projects.length === 1) {
        const projectId = resolved.projects[0].id
        armCreateFocus({ kind: "project", projectId })
        createAgentFromPr(projectId, reference, name)
        closeCreateAgent()
        return
      }
      if (resolved.projects.length === 0) {
        // What dux may claim depends on whether the server managed to look at
        // everything. With a project it could not inspect, "no project is a
        // checkout of this" is a certainty dux does not have, and the one
        // project that mattered may be exactly the unreadable one. dux does not
        // clone, and neither wording may imply it might.
        notifyError(
          resolved.uninspected_summary
            ? `No project dux could check is a checkout of ${repository}, and dux could not check every project (${resolved.uninspected_summary}). Choose a project that already has it, or add one from a directory on disk.`
            : `No project in dux is a checkout of ${repository}. Choose a project that already has it, or add one from a directory on disk.`,
        )
      } else {
        notifyInfo(
          `${resolved.projects.length} projects are checkouts of ${repository}. Choose which one this agent belongs in.`,
        )
      }
      // Either way the picker is offered, over just the matches when there are
      // any. The reference rides across so the project they pick completes it.
      setState({ pendingPrReference: reference })
      closeCreateAgent()
      openNewAgentPicker(
        "from_pr",
        resolved.projects.length > 0
          ? resolved.projects.map((p) => p.id)
          : null,
      )
    })
    .catch((e) => {
      // The rejection path needs the same guard: without it a failed stale
      // request clears a newer submit's spinner and shows its error over a
      // dialog asking something else entirely.
      if (state.createAgentPrRequestId !== generation) return
      setState({ createAgentPrResolving: false, createAgentPrRequestId: null })
      notifyError(
        e instanceof Error
          ? e.message
          : "Could not work out which project that pull request is in.",
      )
    })
}

// Submit the name dialog: dispatch create, fork, or create-from-PR based on the
// current target, then close. Mirrors the TUI, where the same name prompt drives
// these flows.
export function submitNameDialog(name: string): void {
  const target = state.createAgentTarget
  if (!target) return
  if (target.kind === "new") {
    armCreateFocus({ kind: "project", projectId: target.projectId })
    createAgent(target.projectId, name, state.createAgentCopyChanges)
  } else if (target.kind === "fork") {
    // A fork lands in the same project as its source session; resolve it so the
    // focus diff is scoped to that project. If the source vanished from the
    // ViewModel, skip auto-focus rather than arming an unscoped token that could
    // grab any project's next new session.
    const forkSource = state.spine?.sessions.find(
      (s) => s.id === target.sessionId,
    )
    const projectId = forkSource
      ? workspaceProjectId(forkSource.workspace)
      : null
    if (projectId) armCreateFocus({ kind: "project", projectId })
    forkAgent(target.sessionId, name)
  } else if (target.projectId === null) {
    // Reference-first: dux has to work out the project before anything is
    // created, so the dialog stays open until the answer arrives.
    submitPrReferenceFirst(state.createAgentPrInput.trim(), name)
    return
  } else {
    armCreateFocus({ kind: "project", projectId: target.projectId })
    createAgentFromPr(target.projectId, state.createAgentPrInput.trim(), name)
  }
  closeCreateAgent()
}

// Reorder every agent as one global list. `orderedIds` must be the complete set
// of session ids: the server validates it as a strict permutation and rejects a
// partial or stale one. The optimistic overlay clears when the next spine
// confirms the order, or on error.
export function reorderAgents(orderedIds: string[]): void {
  setState({ pendingAgentOrder: orderedIds })
  sessionsApi.reorderGlobal(orderedIds).catch((e) => {
    // A rejected reorder is never reconciled by a spine, so the overlay would
    // linger forever. Clear it so the UI snaps back to the authoritative order,
    // then surface the failure.
    setState({ pendingAgentOrder: null })
    notifyError(e instanceof Error ? e.message : "Could not reorder the agents.")
  })
}

// Reorder every terminal as one global list, the twin of `reorderAgents`.
// `orderedIds` must be the complete set of terminal ids of any owner, validated
// server-side as a strict permutation. Terminal order is runtime-only, so it
// resets to creation order on restart.
export function reorderTerminals(orderedIds: string[]): void {
  setState({ pendingTerminalOrder: orderedIds })
  terminalsApi.reorder(orderedIds).catch((e) => {
    // A rejected reorder is never reconciled by a spine, so the overlay would
    // linger forever. Clear it so the UI snaps back to the authoritative order,
    // then surface the failure.
    setState({ pendingTerminalOrder: null })
    notifyError(e instanceof Error ? e.message : "Could not reorder the terminals.")
  })
}

// One-shot reorder of every project's sessions, distinct from `setAgentSort`,
// which sets the shared display mode. It persists through `reorder_sessions`,
// so the manual order the terminal UI shows stays in step by construction.
//
// No optimistic overlay: a sort touches every project's sessions at once, so
// nothing local could cover them all and the rows snap on the next spine.
export function sortAgents(by: SortKey): void {
  const sessions = state.spine?.sessions ?? []
  const projects = state.spine?.projects ?? []
  // A sort supersedes any in-flight drag: drop its overlay up front, or a
  // superseded drag order would linger on screen until something else clears
  // it (the overlay only retires on match/error/disconnect).
  setState(clearPendingOrders())
  for (const project of projects) {
    const projectSessions = sessions.filter(
      (s) => workspaceProjectId(s.workspace) === project.id,
    )
    if (projectSessions.length < 2) continue
    const orderedIds = sortedSessionIds(projectSessions, by)
    sessionsApi
      .reorder(project.id, orderedIds)
      .catch((e) =>
        notifyError(
          e instanceof Error ? e.message : "Could not reorder the sessions.",
        ),
      )
  }
}


// Where a picked macro landed: the compose draft, the PTY, or nowhere (unknown
// macro, no focused target, no active socket). The macro popover reads this to
// decide its close-focus target: a compose insert must land focus in the
// draft, while the PTY path keeps today's focus behavior.
export type MacroDestination = "compose" | "pty" | "none"

// Run a macro by name on the focused target, client-side: the text comes from
// the bootstrap document and goes to one of two destinations.
//
// With a compose-insert sink registered, the RAW text joins the compose draft
// at the caret, because the Send path owns the newline to keystroke transform.
// Otherwise `macroPayloadBytes` applies that transform and writes to the active
// PTY socket, which the picker's own filtering guarantees is the macro's
// target. Neither path appends a submit: the user presses Enter.
export function runMacro(name: string): MacroDestination {
  const macro = (state.bootstrap?.macros ?? []).find((m) => m.name === name)
  if (!macro) return "none"
  // Defensive: only inject when a terminal is actually focused. During a focus
  // switch the outgoing pane may not have cleared its registration yet; without
  // a selected target the active socket (or compose sink) is stale, and writing
  // to it would paste the macro into the wrong (just-detached) surface.
  if (state.selectedTarget === null) return "none"
  const compose = getComposeInsertSink()
  if (compose !== null) {
    compose.insert(macro.text)
    return "compose"
  }
  const pty = getActivePtySocket()
  if (pty === null) return "none"
  pty.sendInput(macroPayloadBytes(macro.text))
  return "pty"
}

// Open the macro-editor dialog, seeding the draft from the current bootstrap
// macros (a fresh copy so edits don't mutate the shared model). Runs in the
// click/palette handler that opens the dialog, never an effect.
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
// `bootstrap.macros`. The dialog closes optimistically; a rejection surfaces as
// an error toast, and reopening re-seeds from the (unchanged) bootstrap.
export function saveMacros(macros: MacroView[]): void {
  // `update_macros` is a WHOLESALE replace of the entire `[macros]` map. Before
  // the bootstrap document has loaded, `openMacrosDialog` seeded an EMPTY draft,
  // so saving would wipe the server's macros. Refuse until we hold the
  // authoritative list (the Save button is also disabled in this window).
  if (state.bootstrap === null) {
    notifyError("Macros aren't loaded yet. Try again in a moment.")
    return
  }
  configApi
    .updateMacros(macros)
    .catch((e) =>
      notifyError(e instanceof Error ? e.message : "Could not save the macros."),
    )
  closeMacrosDialog()
}

// Persist a drag-reorder of the macro editor's list through the same wholesale
// PUT as `saveMacros`, without closing the dialog. Resolves false on any
// refusal or failure so the dialog can snap its optimistic order back; the
// overlay lives in the dialog's own draft, which no spine reconciles.
export function persistMacroOrder(macros: MacroView[]): Promise<boolean> {
  // Same guard as saveMacros: before bootstrap loads the draft was seeded
  // empty, and a wholesale PUT from that base would wipe the server's macros.
  if (state.bootstrap === null) {
    notifyError("Macros aren't loaded yet. Try again in a moment.")
    return Promise.resolve(false)
  }
  return configApi
    .updateMacros(macros)
    .then(() => true)
    .catch((e) => {
      notifyError(
        e instanceof Error ? e.message : "Could not reorder the macros.",
      )
      return false
    })
}

// --- Theater mode ----------------------------------------------------------
//
// One pane, no chrome: the header, the pull-request band and the tab strip
// leave and the terminal takes their height.
//
// Both directions push, so a Back straight after enter-then-exit re-enters
// theater rather than doing nothing. Both write the pane's memory, so returning
// to the pane returns to the mode.

// Theater is a modifier on a pane, and neither the editor nor the phone's
// changes screen is that pane: the address cannot carry the modifier there
// (`theaterSerializable`), so a live flag would make state and URL disagree
// about a mode the user can neither see nor leave. Opening either suspends the
// mode and leaves the pane's memory alone, which is what makes it a round trip.
function theaterSuspendPatch(): { theater: boolean } {
  return { theater: false }
}

function theaterResumePatch(): { theater: boolean } {
  const theater =
    state.selectedTarget === null
      ? false
      : readTheaterMemory(theaterMemoryKey(state.selectedTarget))
  return { theater }
}

// The layout theater borrows, restored on the way out. Theater unmounts the
// sidebar and the Changes pane without the user having changed any preference,
// so the shell derives its layout from the live `theater` flag and writes none.
//
// The Changes pane's visibility is deliberately absent: it is server-persisted
// config shared by every client, so putting a captured value back would
// overwrite a preference changed elsewhere while the mode was on. The pane
// returns when the suppression lifts, which needs no write at all.
export interface TheaterLayoutSnapshot {
  sidebarOpen: boolean
  sidebarWidth: string
  changesPanePercent: number
}

// The one owner of the snapshot: hand it the patch a commit is about to apply
// and it appends the capture or restore for the `theater` value that patch
// settles on. Two halves of one commit can each have an opinion about the mode,
// both read against the pre-commit state, and the later spread wins the flag.
// Deciding beside either half pairs one half's flag with the other's snapshot,
// which restores a layout captured with the mode off.
function withTheaterLayout(
  patch: Partial<DuxState> & { theater?: boolean },
): Partial<DuxState> {
  const next = patch.theater ?? state.theater
  return { ...patch, ...theaterLayoutPatch(next) }
}

function theaterLayoutPatch(next: boolean): Partial<DuxState> {
  // Nothing changed, so nothing is captured and nothing is put back: a second
  // capture inside the mode would overwrite the layout the user came from with
  // the one the mode imposed.
  if (next === state.theater) return {}
  if (next) {
    return {
      theaterLayout: {
        sidebarOpen: state.sidebarOpen,
        sidebarWidth: state.sidebarWidth,
        changesPanePercent: state.changesPanePercent,
      },
    }
  }
  const captured = state.theaterLayout
  if (!captured) return { theaterLayout: null }
  return {
    theaterLayout: null,
    sidebarOpen: captured.sidebarOpen,
    sidebarWidth: captured.sidebarWidth,
    changesPanePercent: captured.changesPanePercent,
  }
}

/** Put the focused pane in theater. Nothing focused, nothing to do. */
export function enterTheater(): void {
  if (!state.selectedTarget || state.theater) return
  writeTheaterMemory(theaterMemoryKey(state.selectedTarget), true)
  setState(withTheaterLayout({ theater: true }))
  syncUrl("push")
}

/** Give the chrome back. */
export function exitTheater(): void {
  if (!state.theater) return
  writeTheaterMemory(theaterMemoryKey(state.selectedTarget), false)
  setState(withTheaterLayout({ theater: false }))
  syncUrl("push")
}

/** What the one button in the pane header calls. */
export function toggleTheater(): void {
  if (state.theater) exitTheater()
  else enterTheater()
}

/**
 * A mounted pane lost input ownership of its PTY, so the take-over card is
 * about to cover it. Theater leaves and so does the pane's memory of it:
 * deciding what to do about a take-over wants the chrome in view, and
 * re-entering is then a fresh press. Losing ownership itself stays sticky.
 *
 * Keyed on the PTY, not the selection, so an unfocused pane forgets too.
 */
export function noteTheaterOwnershipLost(
  kind: "agent" | "terminal",
  id: string,
): void {
  const key = theaterMemoryKeyForPty(kind, id)
  clearTheaterMemory(key)
  if (!state.theater) return
  if (theaterMemoryKey(state.selectedTarget) !== key) return
  setState(withTheaterLayout({ theater: false }))
  syncUrl("replace")
}

// Open the mobile changes screen over the focused agent. It is a position of its
// own in the URL (`#/agent/<sid>/changes`), so entering it pushes an entry and
// the browser's Back leaves it. There is no matching "go to the terminal screen"
// or "go home" call: focusing a target IS the terminal screen and clearing the
// target IS home, both through the ordinary selection functions.
export function openChangesScreen(): void {
  if (!state.selectedTarget || state.mobileScreen === "changes") return
  setState(withTheaterLayout({ mobileScreen: "changes", ...theaterSuspendPatch() }))
  syncUrl()
}

// Navigate to the parent route rather than stepping browser history. Real route
// changes push; correcting a not-found URL replaces so Back cannot reopen it.
export function navigateUp(): void {
  const urlMode = state.routeNotFound ? ("replace" as const) : undefined
  // Belt and braces for the standalone shell: with no editor state left there
  // is nothing for that surface to render but its boot spinner, so the way
  // out must land on the ordinary shell. The clear paths already drop the
  // flag with the editor state (`clearEditorStateSilently`); this guard makes
  // the escape hatch safe even if a future path forgets.
  if (
    state.standaloneEditor &&
    state.editorTarget === null &&
    state.editorRoute === null
  ) {
    setState({ standaloneEditor: false })
    // The surface bit drives the tab title and the attention count, so it has
    // to be re-rendered wherever the flag drops.
    refreshAttentionChrome()
  }
  if (state.mobileScreen === "changes" && state.selectedTarget) {
    setState(
      withTheaterLayout({ mobileScreen: "terminal", ...theaterResumePatch() }),
    )
    syncUrl(urlMode)
    return
  }
  selectSessionRoute(null, urlMode)
}

/// Drop every draft whose target is no longer in the spine. Returns the SAME
/// object when nothing changed, so an unrelated spine push does not invalidate
/// every subscriber that reads the map.
function pruneComposeDrafts(
  drafts: Record<string, string>,
  spine: Spine,
): Record<string, string> {
  const live = new Set<string>()
  for (const session of spine.sessions) {
    for (const tab of session.tabs) live.add(tab.id)
  }
  for (const terminal of spine.terminals) live.add(terminal.id)
  const keys = Object.keys(drafts)
  const kept = keys.filter((id) => live.has(id))
  if (kept.length === keys.length) return drafts
  const next: Record<string, string> = {}
  for (const id of kept) next[id] = drafts[id]
  return next
}

/// Record (or clear) one target compose draft. Called on every keystroke in the
/// box, so it writes only when the value really moved.
export function setComposeDraft(targetId: string, text: string): void {
  if (composeDraft(state, targetId) === text) return
  const next = { ...state.composeDrafts }
  if (text === "") delete next[targetId]
  else next[targetId] = text
  setState({ composeDrafts: next })
}

/// One target draft read at CALL time, outside a render. The compose-insert sink
/// is registered once and lives across every later keystroke, so reading the
/// draft out of its render closure would splice a macro into whatever the box
/// held when the sink was registered.
export function peekComposeDraft(targetId: string): string {
  return composeDraft(state, targetId)
}

/// One target draft, or the empty string.
///
/// The map is read defensively because a great many component tests build their
/// `DuxState` as a deliberately partial fixture cast through `unknown`, and a
/// selector that assumes a field is present turns every one of them into a crash
/// the moment a new field is added. Production always has the map.
export function composeDraft(s: DuxState, targetId: string): string {
  return (s.composeDrafts as Record<string, string> | undefined)?.[targetId] ?? ""
}

export function reconnect(): void {
  // Retrying is indefinite, so this is not a rescue: it is the user asking to
  // stop waiting out the backoff. `connect()` resets the events backoff to the
  // floor, and the `terminalEpoch` bump remounts the focused TerminalPane so
  // its PTY socket does the same rather than waiting out its own gap.
  //
  // That remount is why the compose draft lives in this store and not the pane.
  eventsSocket.connect()
  setState({ terminalEpoch: state.terminalEpoch + 1 })
}

// Expand or collapse the desktop sidebar, and remember the choice.
//
// Inert in theater, as is the width below: the panel is unmounted there, and
// the primitive's keyboard shortcut would otherwise flip a panel nobody can see
// and write the preference for it. Inert rather than exiting the mode, because
// a key meaning "collapse the sidebar" must not become a way out of an
// unrelated mode that has exits of its own.
export function setSidebarOpen(open: boolean): void {
  if (state.theater) return
  if (state.sidebarOpen === open) return
  setState({ sidebarOpen: open })
  persistSidebarOpen(open)
}

// Update the expanded sidebar width during a drag. Pass `persist` on release to
// write the final value to localStorage.
export function setSidebarWidth(width: string, persist = false): void {
  if (state.theater) return
  setState({ sidebarWidth: width })
  if (persist) {
    writeStoredText(SIDEBAR_WIDTH_KEY, width)
  }
}

// The Changes pane's effective visibility (desktop): the per-session override if
// set, else the config default from the bootstrap document, else visible (the
// pre-load window before the first bootstrap fetch lands).
export function changesPaneVisible(s: DuxState): boolean {
  return s.changesPaneOverride ?? s.bootstrap?.show_changes_pane ?? true
}

// Record the live split. Called from the panel group's onLayoutChange on every
// pointer move of a drag (the header's spacer must track the divider, not jump
// to it on release), so it is written far more often than it changes value:
// `setState` re-renders every subscriber, and a pointer move that lands the
// divider on the same percentage twice must not cost one.
export function setChangesPanePercent(percent: number): void {
  const next = Math.min(100, Math.max(0, percent))
  if (state.changesPanePercent === next) return
  setState({ changesPanePercent: next })
}

// Remember a released split, so the Changes divider comes back where it was
// left. Called at the END of a gesture only, never on the per-move cadence
// above, which is the same rule the sidebar's edge follows.
//
// A split at or under the pane's own minimum is not written: a collapse is
// carried by the visibility preference (see `collapseChangesPaneFromDrag`), and
// remembering the zero as well would re-hide a pane the user had reopened.
export function persistChangesPanePercent(percent: number): void {
  if (percent < CHANGES_PANE_MIN_PERCENT) return
  if (percent > CHANGES_PANE_MAX_PERCENT) return
  writeStoredPanePercent(DIVIDER_STORAGE_KEYS.changesPanePercent, percent)
}

// What the header must reserve on its right so the control before the spacer
// lands on the terminal pane's right edge. Zero when the Changes pane is hidden
// and zero in theater, by the same rule the shell lays out with
// (`changesPaneVisible(dux) && !theater`): the mode suppresses the pane without
// touching the preference, so the preference alone would reserve a strip of
// nothing.
export function changesSpacerPercent(s: DuxState): number {
  return changesPaneVisible(s) && !s.theater ? s.changesPanePercent : 0
}

// A collapsed collapsible panel snaps to its collapsedSize, which defaults to
// 0% (the Changes panel does not override it); the epsilon is that zero plus
// float slop in the reported percentages. Same threshold, and same reasoning,
// as `isExplorerCollapsed` in lib/editorLayout.ts.
export const CHANGES_PANE_COLLAPSE_EPSILON = 1

// Did this layout report take the Changes panel from a measured, open width to
// nothing? That is the one state that strands the pane: zero-width but still
// "visible", so its own menu is inside the zero. An undefined `prevPercent` is
// a panel's first report and never a collapse, or the pane would hide during
// its own mount.
export function isChangesPaneDragCollapse(
  percent: number,
  prevPercent: number | undefined,
): boolean {
  if (prevPercent === undefined) return false
  return (
    prevPercent >= CHANGES_PANE_COLLAPSE_EPSILON &&
    percent < CHANGES_PANE_COLLAPSE_EPSILON
  )
}

// What a layout report means for the drag-collapse latch.
//
//   arm      a collapse arrived mid-gesture; remember it, write nothing yet
//   commit   write the preference now
//   disarm   the pane came back out before the pointer was released
//   restore  nobody asked for this; put the split back and write nothing
//   none     nothing to do
export type ChangesPaneCollapseStep =
  | "none"
  | "arm"
  | "commit"
  | "disarm"
  | "restore"

// A collapse to zero is decided during the drag and written only at its end:
// writing on the report unmounts the separator mid-gesture, and
// react-resizable-panels 4.11.2 re-registers the dead group from its own
// `pointerup`, leaving a phantom separator that swallows presses.
//
// A zero is believed only when the gesture was real: a press the browser claims
// as a scroll reaches the library as a full-scale delta and drives the pane to
// zero unasked, and `reshowPending` is the cached mount width of a pane that
// left at zero, healed by the re-show rather than latched here.
export function changesPaneCollapseStep(args: {
  percent: number
  prevPercent: number | undefined
  pointerDown: boolean
  armed: boolean
  reshowPending: boolean
  pointerMoved: boolean
  cancelled: boolean
  keyboardStep: boolean
}): ChangesPaneCollapseStep {
  if (args.reshowPending) return "none"
  if (isChangesPaneDragCollapse(args.percent, args.prevPercent)) {
    if (args.keyboardStep) return "commit"
    if (args.cancelled || !args.pointerMoved) return "restore"
    return args.pointerDown ? "arm" : "commit"
  }
  if (args.armed && args.percent >= CHANGES_PANE_COLLAPSE_EPSILON) {
    return "disarm"
  }
  return "none"
}

// Is the Changes pane out of reach right now: the preference off, or on with
// the pane at zero width? The reopen button gates on this rather than the
// preference alone, so a zero-width pane still has a way back. It reads
// `changesPanePercent` raw, because `changesSpacerPercent` is defined as zero
// while the preference is off and cannot tell the two hidden states apart.
export function changesPaneEffectivelyHidden(s: DuxState): boolean {
  if (!changesPaneVisible(s)) return true
  return s.changesPanePercent < CHANGES_PANE_COLLAPSE_EPSILON
}

// A drag collapse writes the same preference the pane's hide item writes, so
// there is one hidden state with one way back. Guarded on current visibility
// because a panel can report zero repeatedly and each write is a config PUT,
// and on theater, which unmounts the pane it is only borrowing and may never
// write that preference for every connected client.
export function collapseChangesPaneFromDrag(): void {
  if (state.theater) return
  if (!changesPaneVisible(state)) return
  setChangesPaneVisibility(false)
}

// Show the Changes pane, healing a width of nothing on the way. The preference
// and the split are unrelated variables, so re-showing a pane that was dragged
// to zero would otherwise re-show a zero. A width the user actually chose is
// left alone.
export function showChangesPane(): void {
  if (state.changesPanePercent < CHANGES_PANE_COLLAPSE_EPSILON) {
    setChangesPanePercent(CHANGES_PANE_DEFAULT_PERCENT)
  }
  setChangesPaneVisibility(true)
}

// Set the Changes pane's visibility and persist it to
// `config.ui.show_changes_pane`. The override is optimistic so the pane moves
// at once, dropped by `applyBootstrap` once config confirms it, and rolled back
// with a toast on error. Resolves to whether the persist succeeded, so a caller
// can gate on it.
export function setChangesPaneVisibility(next: boolean): Promise<boolean> {
  setState({ changesPaneOverride: next })
  return configApi
    .setChangesPaneVisible(next)
    .then(() => true)
    .catch((e) => {
      // Roll the optimistic override back so the pane doesn't strand in the
      // toggled state when the persist fails.
      setState({ changesPaneOverride: null })
      notifyError(
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

// ── The hideable touch terminal-keys bar (ui.mobile_accessory_bar) ─────────

// The accessory key bar's effective visibility: the optimistic override if set,
// else the config default, else visible for the window before the first
// bootstrap fetch lands.
export function mobileAccessoryBarVisible(s: DuxState): boolean {
  return (
    s.mobileAccessoryBarOverride ?? s.bootstrap?.mobile_accessory_bar ?? true
  )
}

// Set the accessory key bar's visibility and persist it through the generic
// settings PATCH, which it may use because the preference is a pure render gate
// with no server-side effect. The override is optimistic so the bar moves on
// tap, dropped by `applyBootstrap` once config confirms it, and rolled back
// with a toast on error. Resolves to whether the persist succeeded.
export function setAccessoryBarVisibility(next: boolean): Promise<boolean> {
  const prev = state.mobileAccessoryBarOverride
  setState({ mobileAccessoryBarOverride: next })
  // `quiet: true`: success is silence. The bar visibly moving IS the
  // feedback, so the server is asked to skip its "Settings updated." status
  // for this write; a failed PATCH still toasts below.
  return configApi
    .patchSettings({ ui: { mobile_accessory_bar: next }, quiet: true })
    .then(() => true)
    .catch((e) => {
      // Roll the optimistic override back so the bar doesn't strand in the
      // toggled state when the persist fails, but ONLY while the override
      // still holds the value this call wrote. A newer tap may have landed
      // while this write was in flight, and rolling back over it would snap
      // the bar to a state the user already corrected.
      if (state.mobileAccessoryBarOverride === next)
        setState({ mobileAccessoryBarOverride: prev })
      notifyError(
        e instanceof Error
          ? e.message
          : "Could not save the terminal-keys bar preference.",
      )
      return false
    })
}

// ── Per-PTY input ownership (the `ptyOwnership` ledger) ─────────────────────

// TerminalPane's reporter: record this pane's live ownership verdict for the
// AGENT PTY it renders ("mine" while it holds input, "elsewhere" while another
// connection does), or retire the verdict ("unknown", from the unmount
// cleanup; with the pane gone this client has no live verdict and the
// server-published spine field takes over). Idempotent so the per-render
// effect churn never re-publishes an unchanged verdict.
export function noteAgentPtyOwnership(
  ptyId: string,
  verdict: "mine" | "elsewhere" | "unknown",
): void {
  const current = state.ptyOwnership[ptyId]
  const next = verdict === "unknown" ? undefined : verdict
  if (current === next) return
  const map = { ...state.ptyOwnership }
  if (next === undefined) delete map[ptyId]
  else map[ptyId] = next
  setState({ ptyOwnership: map })
}

// TerminalPane's other reporter: register (live) or retire (dead) one of this
// client's OWN PTY-socket connection ids, from the socket's `connected` frame
// and its reconnect/close paths. See the `ownPtyConnIds` field doc.
export function noteOwnPtyConnection(connId: string, live: boolean): void {
  const has = Boolean(state.ownPtyConnIds[connId])
  if (has === live) return
  const next = { ...state.ownPtyConnIds }
  if (live) next[connId] = true
  else delete next[connId]
  setState({ ownPtyConnIds: next })
}

// True while any of the agent's tab PTYs is input-owned by another connection.
// The agent menu disables its mutating entries on this; read-only ones stay.
//
// Per tab, freshest first: a mounted pane's `ptyOwnership` verdict, then the
// spine's `AgentTabView.input_owner` compared against this client's own ids,
// which is what gates an agent no pane on this device is attached to.
// The optional chains are deliberate: unit-test states are partial mocks.
export function sessionActiveElsewhere(
  s: DuxState,
  session: SessionView,
): boolean {
  // Belt-and-braces for states whose `tabs` is absent (partial test mocks, an
  // older server): the session-slot tab's id IS the session id, so when tabs
  // are present the loop below covers this entry too, with the full local
  // -then-server precedence. Do not "optimize" the loop to skip the slot tab.
  if (s.ptyOwnership?.[session.id] === "elsewhere") return true
  return (session.tabs ?? []).some((t) => {
    const local = s.ptyOwnership?.[t.id]
    if (local === "elsewhere") return true
    if (local === "mine") return false
    return Boolean(t.input_owner) && !s.ownPtyConnIds?.[t.input_owner as string]
  })
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
  }
  // One flat collection, so every terminal of every owner is reached by one
  // loop and no owner kind can be missed.
  for (const t of state.spine?.terminals ?? []) deleteTerminal(t.id)
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

// ── The two first-load screens ───────────────────────────────────────────────
//
// One dialog, two screens, three entry points: the server's automatic offer (via
// `applyBootstrap`) and the app menu's two on-demand items. Only the automatic
// one dismisses on close.

// Open the automatic screen the server offered in the bootstrap document, if any.
// Called from `applyBootstrap`, so it runs on first load AND on every
// `config.changed` refetch; hence the three guards below, each of which is the
// difference between "shown once" and "pops up while you work".
function offerAutomaticFirstLoad(pending: PendingFirstLoad | null): void {
  if (pending === null) {
    // The server has no pending screen. If THIS tab is showing the AUTOMATIC one,
    // it has been settled elsewhere: another browser tab dismissed it, and the
    // server emitted `config.changed` precisely so we find out. Close ours rather
    // than leaving a dialog up over a screen nobody owes an acknowledgement for.
    // Scoped to `automatic`: an on-demand dialog the user opened themselves is
    // never yanked away by a background refetch.
    if (state.firstLoad?.automatic) setState({ firstLoad: null })
    return
  }
  // Already dismissed in this browser session: the server clears its pending
  // screen on dismissal, but a refetch that raced the clear would otherwise
  // re-open what the user just closed.
  if (state.firstLoadDismissed) return
  // A dialog is already up (this screen, or the same screen opened on demand).
  // Re-opening would reset the user's scroll position mid-read.
  if (state.firstLoad !== null) return
  setState({
    firstLoad: {
      screen: pending.screen,
      automatic: true,
      // The server never offers the what's-new screen without notes in hand, so
      // the automatic path never loads and never fails.
      notes: pending.notes ?? null,
      loading: false,
      error: null,
    },
  })
}

// The app menu's "Welcome screen…". Needs no fetch: the copy rides the bootstrap
// document unconditionally, exactly so this entry always works.
export function openWelcomeScreen(): void {
  setState({
    firstLoad: {
      screen: "welcome",
      automatic: false,
      notes: null,
      loading: false,
      error: null,
    },
  })
}

// The app menu's "What's new…". Opens immediately in a loading state and fetches
// the notes, because the server may have to reach GitHub. Works even when
// `ui.disable_release_notes` is set: that preference suppresses the AUTOMATIC
// screen only. A failure lands in the dialog body AND a toast, never silent.
export function openReleaseNotes(): void {
  setState({
    firstLoad: {
      screen: "whats_new",
      automatic: false,
      notes: null,
      loading: true,
      error: null,
    },
  })
  firstLoadApi
    .fetchReleaseNotes()
    .then((notes) => {
      // Drop a late reply if the user closed the dialog or navigated to the
      // other screen meanwhile.
      if (state.firstLoad === null) return
      if (state.firstLoad.screen !== "whats_new") return
      setState({
        firstLoad: { ...state.firstLoad, notes, loading: false, error: null },
      })
    })
    .catch((e) => {
      const message =
        e instanceof Error ? e.message : "Could not load the release notes."
      notifyError(message)
      if (state.firstLoad === null) return
      if (state.firstLoad.screen !== "whats_new") return
      setState({
        firstLoad: { ...state.firstLoad, loading: false, error: message },
      })
    })
}

// Close the first-load dialog. Closing an automatic screen also dismisses it
// server-side, settling it on both surfaces; an on-demand open dismisses
// nothing.
//
// The close is optimistic and unconditional, since a failed dismissal must not
// trap the user behind a modal, and the re-open guard is set in the same
// `setState`, then rolled back if the write fails. It cannot wait for the POST:
// a `config.changed` arriving in that window would reopen the dialog.
export function closeFirstLoad(): void {
  const open = state.firstLoad
  if (open === null) return
  if (!open.automatic) {
    setState({ firstLoad: null })
    // Nothing to dismiss, but the offer may have been DROPPED while this dialog
    // was up: `offerAutomaticFirstLoad` runs only from `applyBootstrap` and bails
    // when a dialog is already open, and nothing else retries it. Re-check the
    // last bootstrap now that the slot is free, or a real pending screen that
    // landed mid-read is lost for this tab's whole session.
    offerAutomaticFirstLoad(state.bootstrap?.pending_first_load ?? null)
    return
  }
  setState({ firstLoad: null, firstLoadDismissed: true })
  firstLoadApi.dismiss().catch((e) => {
    // The durable record was not written, so this launch's screen is still
    // pending: drop the guard again so the next bootstrap can re-offer it.
    setState({ firstLoadDismissed: false })
    notifyError(
      e instanceof Error
        ? e.message
        : "Could not record this screen as seen; it may appear again.",
    )
  })
}

// Persist the instance identity (browser tab title and favicon colour).
// Nothing is hand-applied here: `applyBootstrap` re-applies the title,
// wordmark and favicon on every client once config confirms them. The success
// toast is the engine's routed status, so only a failure is surfaced here.
// Resolves to whether the persist succeeded, so a dialog can gate on it.
export function setInstanceIdentity(body: {
  title?: string
  favicon?: string
}): Promise<boolean> {
  return configApi
    .setInstanceIdentity(body)
    .then(() => true)
    .catch((e) => {
      notifyError(
        e instanceof Error ? e.message : "Could not rename this instance.",
      )
      return false
    })
}

// Persist the Settings modal's `[ui]` and `[capabilities]` fields; title and
// favicon stay on `setInstanceIdentity`. Same resolves-to-boolean and
// toast-on-error contract as that function, so the dialog can `Promise.all`
// both writes. Nothing is hand-applied here: the refetched bootstrap is the
// single source of truth and the dialog re-seeds from it.
export function saveSettings(
  patch: Parameters<typeof configApi.patchSettings>[0],
): Promise<boolean> {
  return configApi
    .patchSettings(patch)
    .then(() => true)
    .catch((e) => {
      notifyError(e instanceof Error ? e.message : "Could not save settings.")
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
      notifyError(
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
// existing reload (best-effort: the file is already persisted), close, and toast.
export function saveConfigEditor(content: string): void {
  setState({ configEditorError: null })
  configApi
    .writeRawConfig(content)
    .then(() => {
      // Save persists but does not apply: config.toml is written and the running
      // config is untouched until the user runs "Reload config", which the toast
      // says so the lack of a visible change is not read as a no-op. Not sticky:
      // nothing is lost, and the action it names is a permanent menu entry.
      closeConfigEditor()
      notifySuccess("Saved config.toml. Run “Reload config” to apply it.")
    })
    .catch((e) => {
      setState({
        configEditorError:
          e instanceof Error ? e.message : "Could not save config.toml.",
      })
    })
}
