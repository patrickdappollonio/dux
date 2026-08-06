import { useSyncExternalStore } from "react"
import { toast } from "sonner"

import { sanitizeAgentName } from "./agentName"
import { git } from "./git"
import { projectsApi, type PatchProjectBody } from "./projectsApi"
import { existingBranchConflict, sessionsApi, SessionsApiError } from "./sessionsApi"

import { ordersMatch, reorderById } from "./reorder"
import { sortedSessionIds, type SortKey } from "./sortSessions"
import { nextActiveSessionId, type FlatSortKey } from "./flatList"
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
  fetchServerIdentity,
  serverChanged,
  type ServerIdentity,
} from "./buildApi"
import { reloadPage } from "./reloadPage"
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
import { type FinalTone, showFinalToast } from "./finalToast"
import { statusToastDuration } from "./statusToast"
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
import { assertNever } from "./assertNever"
import {
  matchOwner,
  ownerRefFromWire,
  ownerSessionId,
  type TerminalOwnerRef,
} from "./terminalOwner"
import { ownerHasTerminal } from "./terminals"
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

// Who a companion terminal belongs to. The type now lives in
// `lib/terminalOwner.ts` alongside the exhaustive switches that consume it, and
// is re-exported here so the many existing `from "@/lib/store"` imports keep
// working. Every consumer must switch on `kind` and end that switch in
// `assertNever`; there is deliberately no bare-id accessor, so no owner kind can
// be silently ignored by session-shaped code.
export type { TerminalOwnerRef } from "./terminalOwner"

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
   * the bootstrap document). Closing an automatic screen DISMISSES it — the
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
  // The entity whose startup-command log viewer is open, or null. The log files
  // + the displayed file's contents are fetched over REST into the fields below
  // when the viewer opens (mirroring the attach-worktree listing).
  //
  // `startupLogsTarget` is a SESSION id in "agent" scope and a PROJECT id in
  // "project" scope, matching `dux_core::startup::StartupCommandLogScope`: an
  // agent's own runs, or every run across every agent of a project. The scope
  // picks the REST client (sessionsApi vs projectsApi) and the dialog's title,
  // subtitle, empty state and vanished-target lookup. It is a separate field
  // rather than a tagged target so the agent-scope callers and their tests keep
  // reading a plain id.
  startupLogsScope: StartupLogsScope
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
  // (the save is wholesale — `update_macros` replaces the entire `[macros]`
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
  // A resolve is a git call per project on the server, so it can easily still
  // be out when the user has cancelled the dialog, retargeted it at a project,
  // or submitted a different reference. Nothing can recall a reply already in
  // flight, so the only safe rule is that a reply acts when its generation is
  // still the current one. Checking merely that SOME pull-request dialog is
  // open does not catch it: the open one may be a different question, and
  // acting would create an agent from the reference the user replaced and
  // close the dialog they are looking at.
  createAgentPrRequestId: number | null
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

// Is there a browser around this module at all? Everything in the app runs in
// one, and every existing guard below (`typeof location === "undefined"`, the
// `typeof document` check in `refreshAttentionChrome`) says the same thing in
// its own words; this names it once.
//
// It is false in exactly one place: a build-time render outside a browser. The
// marketing site renders the REAL components to static HTML (see
// `website/src/figure/`), which imports this module under plain Node, where
// `localStorage`, `location` and `window` do not exist and a WebSocket has
// nothing to connect to. Under jsdom (the unit tests) and in a browser this is
// true and NOTHING below changes.
const hasBrowser = typeof window !== "undefined"

// The expanded sidebar width is drag-resizable and persisted across reloads.
// 18rem gives agent names breathing room next to the PR/status badges; a
// previously persisted width still wins.
const SIDEBAR_WIDTH_KEY = "dux:sidebar-width"
const DEFAULT_SIDEBAR_WIDTH = "18rem"

function loadSidebarWidth(): string {
  if (!hasBrowser) return DEFAULT_SIDEBAR_WIDTH
  return localStorage.getItem(SIDEBAR_WIDTH_KEY) || DEFAULT_SIDEBAR_WIDTH
}

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
  firstLoad: null,
  firstLoadDismissed: false,
  macrosDialogOpen: false,
  macrosDraft: [],
  mobileScreen: "home",
  routeNotFound: null,
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
  newAgentPickerOnlyIds: null,
  pendingPrReference: null,
  createAgentPrResolving: false,
  createAgentPrError: null,
  createAgentPrRequestId: null,
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

// Seed the store directly, for a render that has no server to fetch from. The
// marketing site's static figure (`website/src/figure/`) is the only caller: it
// runs at build time under plain Node, where `boot()` is skipped and the state
// would otherwise sit at its empty initial value forever. It is deliberately a
// thin pass-through to `setState` so the seeded state settles through exactly
// the same derivation (mobile screen, not-found) that a live patch does. Not
// used by the app at runtime, and it opens no socket and fires no fetch.
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
// per-PTY byte sockets (`lib/ptySocket.ts`). Since the Phase 6 cutover it carries
// EVERYTHING the retired `/ws`/`DuxSocket` used to: resource-change events
// (changed files, spine, config) AND the control frames
// (`connected` id, `status`/`status_cleared` toasts). It also owns the
// connection-state UX (the status-bar indicator). Exported so tests can drive
// its callbacks / inspect its interest set; connected on boot.
export const eventsSocket = new EventsSocket(
  `${wsScheme}//${wsHost}/ws/events`,
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
    const id = ev.key ?? ANON_TOAST_ID
    cancelBusyToastGuard(id) // the toast is going now; the guard is moot
    toast.dismiss(id)
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

// Which run of which build of dux this tab loaded against, read once at boot
// (see `buildApi.ts`). `null` until that read lands, or forever if it failed,
// and a null baseline never forces a reload: unknown is not "changed".
let serverIdentityBaseline: ServerIdentity | null = null

// Learn the baseline. Called from `boot`, alongside the other initial reads.
async function loadServerIdentityBaseline(): Promise<void> {
  serverIdentityBaseline = await fetchServerIdentity()
}

// The reconnect's first question: is this the server that served this tab?
//
// A reconnect is the ONLY moment dux can have been restarted under an open tab,
// and a tab whose server was replaced is running code that no longer matches
// what it is being sent. So the answer decides between two whole recovery
// strategies: identical, and the ordinary in-place refetch below is exactly
// right; different, and there is nothing in the page worth keeping, so the
// window is hard reloaded with no prompt.
//
// It runs ALONGSIDE that refetch rather than gating it. The probe is a network
// round-trip, and blocking recovery on it would strand the app whenever the
// probe hung; letting the refetch start a moment early costs nothing, because a
// reload discards whatever it produced.
async function reloadIfServerChanged(): Promise<void> {
  const current = await fetchServerIdentity()
  if (serverChanged(serverIdentityBaseline, current)) reloadPage()
}

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
    // recovers. Concurrent loads are safe: both the spine and the bootstrap load
    // are seq-guarded, so an older reply cannot overwrite a newer one.
    //
    // Capture the deep-linked route BEFORE `loadSpine` so a transient exit-eject
    // during the reconnect (the center pane resets to home while the agent is
    // momentarily `detached`) can be re-restored once the agent resumes. Reading
    // the hash here — before any spine apply runs — beats that eject wiping it.
    //
    // First, though: ask whether this is even the same server. dux may have been
    // restarted during the outage, in which case refetching state into old code
    // is the wrong recovery and the window is reloaded instead.
    void reloadIfServerChanged()
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
// Returns the fetch's promise so a caller that must report on the result (the
// forced refresh, which names the counts it just read) can wait for it. Every
// other caller ignores it: errors are already handled here, so nothing it
// returns can reject.
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

// Re-fetch the selected session's changes (the changes pane's error-card Refresh
// button). No-op when nothing is selected.
//
// This only RE-READS. The server answers a GET from its per-session cache, which
// it drops when one of its own git or editor routes changes a file, so this is
// the right call after an error (nothing is cached) or an event (the cache was
// already dropped). It is
// the WRONG call for a user-driven "refresh now": the server would hand back the
// same cached answer and nothing would appear to change. Use
// `forceRefreshChanges` for that.
export function refreshChanges(): void {
  const id = state.selectedSessionId
  if (id === null) return
  setState({ changes: loadingChanges(id) })
  loadChanges(id)
}

// The Changes pane's "Refresh changes" action: force the server to ask git
// again, then re-read. dux has no file watcher, so a change it did not make
// through one of its own routes (a file the user changed from a terminal, an
// agent writing in its worktree) is only as fresh as the last poll: 2s while
// any agent or terminal in the workspace is running, 10s while none is. A file
// dropped onto a pane is not one of those: the upload refreshes the pane itself
// whenever the file lands in the agent's worktree.
//
// Rejects when the forcing POST fails so the caller can report it the way the
// pane's other quick actions do; the re-read still happens either way, since a
// pane left in `loading` after a failed force would be worse than a stale one.
//
// A success is announced, with the counts, because the common case is that
// nothing changed: the pane flickers and comes back identical, which is the
// wrong amount of evidence for an action whose whole purpose is proving dux
// looked again. Push and pull report themselves through the engine's keyed
// status stream, but this route emits no status (it mutates nothing), so the
// browser says it, in the same words the terminal UI's `refresh-changes`
// command uses. Nothing is said when the re-read did not land: a lost race or a
// failed GET has already told the pane what it needs to show, and claiming
// counts from a slice this refresh did not fill would be a made-up number.
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
  toast.success(
    `Changed files refreshed: ${slice.staged.length} staged, ` +
      `${slice.unstaged.length} unstaged.`
  )
}

// Monotonic sequence for bootstrap loads, mirroring `loadSpineSeq` exactly. Two
// `config.changed` events in quick succession (a config edit followed by another,
// or an edit landing during a reconnect refetch) fire concurrent
// `fetchBootstrap()`s, and nothing orders the replies. Without this an older
// response resolving last overwrites a newer one, and every client keeps applying
// config the server has already replaced until the next edit happens to come back
// in order. Every value the document carries is exposed to that; the one that
// prompted the guard is `provider_drop_paste`, where the stale answer decides how
// a dropped file's path is quoted.
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
      // as `applySpine`: the newest request wins, whatever order the replies
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
  // The server decided this launch's first-load screen once, at startup, and
  // holds it in memory — so it arrives on the FIRST bootstrap of a client that
  // connects at any point, and on the `config.changed`-driven refetch the server
  // emits the moment the decision resolves (which is how a browser already open
  // during a slow release-notes fetch still gets the screen). Guarded inside.
  offerAutomaticFirstLoad(b.pending_first_load ?? null)
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
  fetchSpine().then(
    (s) => {
      // Applying is not fetching, and the two failures need different names:
      // folding a throw from the apply into the rejection handler below would
      // report a perfectly good fetch as a failed one and send whoever reads
      // the console after the wrong thing. This is a backstop for the apply as a
      // whole, NOT the history-write guard: a refused history call is caught at
      // the write itself (`syncUrl`), because the apply is only one of many
      // paths that write the URL and the rest are user clicks.
      try {
        applySpine(s, seq)
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
  // The outgoing session list, captured before `setState` swaps the spine. A
  // vanished agent picks its replacement from this ordering (see
  // `navigateAfterVanish`), since the new list no longer holds its position.
  const previousSessions = state.spine?.sessions ?? []
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
  // Retire a not-found screen the moment this spine proves its URL right again.
  // Its position relative to the focus step below is not load-bearing: a
  // freshly created agent wins the focus either way, because `focusNewlyCreatedSession`
  // running second simply overwrites the retry's selection, and running first
  // clears `routeNotFound` (any patch carrying a target does, see `setState`),
  // which makes the retry return immediately. Reading in URL-then-create order.
  retryRouteNotFound(spine)
  focusNewlyCreatedSession(spine)
  pruneSelectionIfGone(spine, previousSessions)
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
    // whose local retarget-to-session-slot never ran here — this is the shared-workspace
    // heal path). A gone extra tab falls back to the session-slot tab rather than
    // ejecting the user to the welcome screen.
    if (!session) {
      navigateAfterVanish(spine, previous, target.sessionId)
    } else if (
      target.tabId !== target.sessionId &&
      !session.tabs.some((t) => t.id === target.tabId)
    ) {
      // A REWRITE, like every other vanish path: the user did not ask to leave
      // the tab, so this must not push an entry they never created and leave the
      // dead tab's entry sitting underneath it. `changes` is carried across
      // because changed files are session-scoped, so the screen the user is
      // reading survives the tab going away under it.
      //
      // Belt and braces, again: carrying `changes` is exactly what keeps the
      // SCREEN the same, so `syncUrl` would replace here with or without the
      // argument. It stays because it is the sentence above, written down.
      selectSessionRoute(
        target.sessionId,
        "replace",
        state.mobileScreen === "changes",
      )
    }
    return
  }
  // A terminal: it must still exist UNDER its owner. `ownerHasTerminal` checks
  // both halves at once (the id is present AND its owner tag matches the address
  // we are on), so this no longer has to know which collection each owner kind
  // would have been nested in.
  const owner = target.owner
  const stillExists = ownerHasTerminal(spine.terminals, owner, target.terminalId)
  if (!stillExists) {
    // The other out-of-band path: a terminal whose PTY exited is dropped from
    // the ViewModel while the user may be looking at it. A terminal is not an
    // agent and has no "next terminal" worth guessing at, so the destination is
    // whatever sits one level UP: the owning agent for a companion terminal
    // (which is alive and is a real position), home for a project terminal
    // (which has nothing above it). This matches what the deep-link path
    // already does, and both rewrite the current entry rather than stepping
    // history. Ejecting a companion terminal all the way to home threw away a
    // position that still existed.
    //
    // The lossy `ownerSessionId` is right here because "is there an agent above
    // this terminal" IS the whole decision: an owner that is not a session has
    // nothing above it, and home is already the answer for that.
    const ownerSession = ownerSessionId(owner)
    const fallback =
      ownerSession !== null && spine.sessions.some((s) => s.id === ownerSession)
        ? ownerSession
        : null
    selectSessionRoute(fallback, "replace")
  }
}

// The destination when the focused agent vanishes under the user: the next
// ACTIVE agent in the order the list is already showing (see
// `nextActiveSessionId`), or home when every remaining agent is dormant. The
// URL is REWRITTEN rather than pushed, so the entry pushed on the way in is
// gone: one Back can then land on the screen the user is already on and look
// inert. That is accepted, and it only happens when the world changed under
// them, which beats being thrown out of the app entirely.
function navigateAfterVanish(
  spine: Spine,
  previous: SessionView[],
  goneSessionId: string,
): void {
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

// Pending busy-toast leak guards, by sonner id.
//
// sonner deliberately never auto-closes a `loading` toast: its close-timer
// effect returns early on `toast.type === 'loading'`, so the duration passed to
// `toast.loading` is inert, and it renders no close button and refuses the
// swipe gesture for that type (all three pinned by tests in
// components/ui/sonner.test.tsx). A busy toast therefore has no exit of its
// own, and if its keyed final never arrives (the events socket dropped
// mid-operation) the spinner sits there forever claiming work is still
// happening. The store schedules the dismissal itself.
//
// The guard is always cancelled before a toast on that id changes, so it can
// only ever dismiss the exact spinner it was armed for: never a later final,
// and never a fresh busy that reused the key.
const busyToastGuards = new Map<string, ReturnType<typeof setTimeout>>()

function cancelBusyToastGuard(id: string): void {
  const handle = busyToastGuards.get(id)
  if (handle === undefined) return
  clearTimeout(handle)
  busyToastGuards.delete(id)
}

// Route a keyed (or anonymous) engine status to both the status bar and a
// sonner toast. The key acts as the sonner id so updates re-render in place
// (busy → success swaps the spinner without a new toast) and clears can dismiss
// by id.
//
// Every tone auto-dismisses; the window is graded by severity off the user's
// `ui.status_clear_seconds` and computed by `statusToastDuration`, which owns
// the policy (including the busy leak guard and the `0` opt-out).
function showStatusToast(
  key: string | null | undefined,
  tone: string,
  message: string,
): void {
  if (!message) return
  const id = key ?? ANON_TOAST_ID // no key → stable anonymous-slot id
  const duration = statusToastDuration(tone, state.bootstrap?.status_clear_seconds)
  const opts = { id, duration }
  // Whatever was armed for this id is now stale: this call replaces the toast.
  cancelBusyToastGuard(id)
  if (tone === "busy") {
    busyToastGuards.set(
      id,
      setTimeout(() => {
        busyToastGuards.delete(id)
        toast.dismiss(id)
      }, duration),
    )
  }
  if (tone === "busy") {
    toast.loading(message, opts)
    return
  }
  // Every other tone goes through the ONE shared final-toast raiser, so a
  // client-originated toast (the file-drop report) and an engine status can
  // never disagree about the configured dismiss window.
  showFinalToast(tone as FinalTone, message, {
    id,
    statusClearSeconds: state.bootstrap?.status_clear_seconds,
  })
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
  // Remember which server served this tab, so a later reconnect can tell whether
  // it is still talking to it.
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
// The URL is the SOURCE OF TRUTH for where the app is, including which screen
// the mobile shell shows. The selected target is mirrored into `location.hash`
// so a tab can be bookmarked/shared/reloaded back to the same place:
//   #/agent/<sessionId>
//   #/agent/<sessionId>/terminal/<terminalId>
//   #/agent/<sessionId>/changes
// Session ids are stable (a reload restores the agent); terminal ids are
// ephemeral (a reload that finds the session but not the terminal falls back to
// the agent). A hash naming a session the workspace does not have resolves to
// the not-found screen (`routeNotFound`) rather than silently landing home.
//
// Moving to a DIFFERENT screen pushes a history entry, in BOTH directions:
// going into an agent pushes, and the Up control that comes back out pushes too,
// because both are ordinary navigation between two real positions and the
// browser is supposed to accumulate those. Changing which agent or tab is
// focused within the same screen replaces the current entry, so switching around
// never piles up. Back and Forward are only ever the browser's own, and the app
// never steps history relatively: `history.go` appears nowhere. The screen is
// read from the URL, so there is no separate depth to keep in agreement with it.
//
// Only two things replace on a screen CHANGE, and both name a position the
// browser is already parked on rather than a new one: a RESTORE (the boot
// deep-link, the reconnect re-restore, the destination chosen when what the user
// was looking at vanished under them) and a CORRECTION (leaving the not-found
// screen, which is retiring a bad address, not visiting a place worth keeping).

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
  // A standalone terminal deep-links as `#/terminal/<tid>`: no owner segment,
  // because it has no owner. It cannot be confused with the two nested shapes,
  // which both begin `#/agent/` or `#/project/`.
  const sm = hash.match(/^#\/terminal\/([^/]+)$/)
  if (sm) {
    try {
      const terminalId = decodeURIComponent(sm[1])
      if (!terminalId) return null
      return { kind: "terminal", terminalId, owner: { kind: "standalone" } }
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
  return target.tabId === target.sessionId
    ? base
    : `${base}/tab/${encodeURIComponent(target.tabId)}`
}

// A position in the app: what is focused, plus whether the changes screen is
// open on top of it. This is everything the URL encodes and everything the
// screen is derived from.
interface Route {
  target: SelectedTarget | null
  changes: boolean
}

// The changes screen rides as a suffix on the focused target's hash, so it
// bookmarks and shares like any other position.
const CHANGES_SUFFIX = "/changes"

// Parse a hash into a route. A hash that names no valid target is home.
function parseRoute(hash: string): Route {
  const direct = parseSelectionHash(hash)
  if (direct) return { target: direct, changes: false }
  // The two branches are mutually exclusive rather than ranked: every regex in
  // `parseSelectionHash` is anchored, so no hash can parse both as a target and
  // as a target-plus-suffix. `#/agent/s1/tab/changes` parses directly and never
  // reaches here; `#/agent/s1/changes` does not parse directly and only the
  // strip finds it. Trying the direct parse first is just the common case
  // first, it decides nothing, and inverting the order would produce the same
  // answers.
  if (hash.endsWith(CHANGES_SUFFIX)) {
    const target = parseSelectionHash(hash.slice(0, -CHANGES_SUFFIX.length))
    if (target) return { target, changes: true }
  }
  return { target: null, changes: false }
}

// The hash for a route. Home is the empty hash; the changes suffix only applies
// on top of a focused target, since there is nothing to show changes for
// otherwise.
function routeHash(route: Route): string {
  const base = selectionHash(route.target)
  if (base === "" || !route.changes) return base
  return base + CHANGES_SUFFIX
}

// The screen a route puts the mobile shell on. This is the whole derivation:
// screen state is never tracked independently of the route.
function routeScreen(route: Route): MobileScreen {
  if (!route.target) return "home"
  return route.changes ? "changes" : "terminal"
}

// The route the app currently holds in state.
function currentRoute(): Route {
  return {
    target: state.selectedTarget,
    changes: state.mobileScreen === "changes",
  }
}

// Bring the URL in line with the app's current position. Pushes when the
// destination is a DIFFERENT screen from the one the URL names (the user moved
// between two positions, in either direction) and replaces when it is the same
// screen (switching agents or tabs in place), so switching around never piles up
// while every screen change stays reachable by Back. `mode: "replace"` forces a
// replace for a move the user did not ask for: the boot deep-link restore, the
// reconnect re-restore, the destination picked when what they were looking at
// vanished under them, and the way out of the not-found screen. Those last two
// deliberately discard the entry pushed on the way in, so one Back can land on
// the screen you are already on; that is accepted, and far better than either
// stepping out of the app or bouncing back onto a dead link.
//
// Defensive: in non-browser test environments `history.replaceState` /
// `history.pushState` / a real `location` may be absent, so this degrades
// rather than throwing.
//
// The write itself is BEST-EFFORT and never throws at its caller. A browser can
// refuse a history call (Safari rate-limits them), and every call site here is
// reached from a click handler AFTER the screen has already moved, so letting
// the refusal propagate would abort the handler mid-navigation and leave the
// screen and the URL disagreeing with no one to put them back. Swallow it, warn
// so a persistently-refusing browser is visible, and let the next successful
// write bring the address bar back in line. This is the ONE place a history call
// is made, which is what makes the one guard enough.
function syncUrl(mode?: "replace"): void {
  if (typeof history === "undefined" || typeof history.replaceState !== "function") {
    return
  }
  const next = routeHash(currentRoute())
  const current = typeof location !== "undefined" ? (location.hash ?? "") : ""
  // Belt and braces: when the address is already what we would write, the
  // branch below would take the replace arm anyway (an unchanged hash is an
  // unchanged screen) and rewrite the identical URL. Skipping the write is
  // cheaper and keeps `history.state` untouched, but nothing depends on it.
  if (current === next) return
  // An empty target hash collapses to the bare path so the URL doesn't keep a
  // dangling "#"; otherwise write just the hash, preserving path + query.
  const base =
    typeof location !== "undefined"
      ? (location.pathname ?? "") + (location.search ?? "")
      : ""
  const url = next === "" ? base : next
  const movedScreen =
    routeScreen(parseRoute(next)) !== routeScreen(parseRoute(current))
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

// Adopt the route the URL currently names. Called from popstate, where the
// browser has already moved its cursor, so this only mirrors the destination
// into state and must never write the URL back.
function applyUrlRoute(): void {
  const hash = typeof location !== "undefined" ? (location.hash ?? "") : ""
  const route = parseRoute(hash)
  const spine = state.spine
  if (!spine) {
    // A popstate before the first spine landed: a slow spine fetch, or a
    // session/bfcache restore that comes back with a back stack already. This
    // used to return and drop the route on the floor, on the claim that the
    // boot deep-link restore would resolve the hash later. It does not: that
    // restore resolves the BOOT hash, and the browser has since moved to a
    // different one. Both outcomes were measured. Booting on home and stepping
    // to an agent left the address bar naming an agent the app never selected,
    // permanently, since nothing retries. Booting on an agent and stepping back
    // to home had the boot restore silently undo the Back and overwrite the
    // entry the user had landed on.
    //
    // So the pending boot link is REPLACED by where the browser actually is,
    // and `restoreDeepLink` resolves that against the first spine. A route
    // naming home replaces it with null, which is exactly the cancellation the
    // second case needs. Nothing else can be done here: resolving a target
    // needs a session list, and there is none yet.
    pendingDeepLink = route.target
    pendingDeepLinkChanges = route.changes
    return
  }
  if (!route.target) {
    // Through `selectSessionRoute`, not `clearSelection`, because pressing Back
    // to home is the user taking control: it must disarm the reconnect
    // deep-link intent, or a reconnect could yank them back to the agent they
    // just left.
    selectSessionRoute(null)
    return
  }
  resolveRouteTarget(spine, route.target, route.changes)
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
  // `changes` travels WITH the target rather than being applied after it. The
  // URL names the screen as well as the focus, and `syncUrl` reads the screen
  // off state, so committing the target first and the screen second would write
  // the address from a half-applied route and strip the `/changes` segment off
  // the very URL being resolved.
  //
  // The `"replace"` here is BELT AND BRACES. Every caller has the browser
  // already parked on this hash, and the only rewrites this path can produce
  // (a gone tab or a gone terminal falling back to its session) stay on the
  // same SCREEN, so `syncUrl` would replace on its own. It is passed because
  // the intent, "this is a restore, never a new position", should be stated at
  // the call site rather than inferred from what the fallbacks happen to do.
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

// Retire the not-found screen once a spine carries the agent its URL names. The
// flag is set from the route, so only the route can clear it, and nothing else
// on the spine path touches it: the prune returns early (there is no selection
// to prune) and no state patch mentions the missing target. Without this the
// screen sticks after the agent comes back, and on a phone it replaces the whole
// shell, so its single button is the only way out.
//
// The check that the URL still names the agent we flagged is BELT AND BRACES,
// not a live guard: any move the user makes carries a target, and a patch
// carrying a target clears the flag (see `setState`), so by the time the hash
// disagrees there is no flag left to act on. It is kept because it is the one
// line that makes "never re-read a stale hash" true by inspection rather than
// by tracing every writer of `routeNotFound`.
function retryRouteNotFound(spine: Spine): void {
  const missing = state.routeNotFound
  if (!missing) return
  const route = parseRoute(typeof location !== "undefined" ? (location.hash ?? "") : "")
  if (!route.target) return
  if (targetSessionId(route.target) !== missing.sessionId) return
  if (!spine.sessions.some((s) => s.id === missing.sessionId)) return
  resolveRouteTarget(spine, route.target, route.changes)
}

// The URL names an agent this workspace does not have. Clear the selection and
// hand the surfaces something truthful to render (see `AgentNotFound`); pressing
// Back onto a deleted agent is a normal thing to do.
function setRouteNotFound(sessionId: string): void {
  const prev = state.selectedSessionId
  setState({
    selectedTarget: null,
    selectedSessionId: null,
    changes: emptyChanges(),
    mobileScreen: "home",
    routeNotFound: { kind: "agent", sessionId },
  })
  switchChangesSubscription(prev, null)
}

// The route parsed from the URL at module load, restored once the first spine
// lands (a target can't be resolved until the session list exists). One-shot:
// consumed (and cleared) on the first `applySpine` so later spine refetches don't
// re-yank a user who has since navigated away.
const bootRoute: Route =
  typeof location !== "undefined"
    ? parseRoute(location.hash ?? "")
    : { target: null, changes: false }
// Both halves are mutable: a popstate that beats the first spine overwrites
// them with the address the browser actually moved to (see `applyUrlRoute`), so
// the restore resolves that rather than a boot hash the user has already left.
let pendingDeepLink: SelectedTarget | null = bootRoute.target
let pendingDeepLinkChanges = bootRoute.changes

// Route a normalized route target onto an already-resolved session: restore a
// still-present terminal or extra tab, else fall back to the session-slot tab.
// Shared by the boot restore, the reconnect re-restore, and Back/Forward so all
// three honor tabs/terminals identically. `urlMode` is passed through to
// `syncUrl`: these are all restores of a position the URL already names (or a
// correction to one), never a fresh move in, so they replace. `changes` is the
// screen half of the route and is committed in the SAME state patch as the
// target, never after it, so the URL is only ever written from a whole route.
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
    // That early return was silent in both directions: it said nothing about
    // WHICH other owner it was declining, and it would have gone on declining a
    // third kind that has no other restore path at all, dropping the user's
    // position with no trace. Written as a match, a new variant cannot be added
    // without answering for it here.
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
    target.tabId !== target.sessionId &&
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

// Restore the boot URL against the first spine. Resolve the session in the
// spine; restore the terminal when it still exists, else fall back to the
// session; render the not-found screen when the session is gone. The mobile
// shell lands on the screen the URL names (`resolveRouteTarget` commits the
// target, and the screen follows from it), which is what makes an agent link
// open its terminal rather than leaving the hub on top of it. Nothing is pushed:
// the browser is already parked on this entry.
function restoreDeepLink(spine: Spine): void {
  const link = pendingDeepLink
  if (!link) return
  pendingDeepLink = null // one-shot, whatever the outcome
  resolveRouteTarget(spine, link, pendingDeepLinkChanges)
}

// Restore a project-terminal route against a spine: select the terminal when its
// project still carries it, and land home when either the project or the terminal
// is gone. Landing home is a real navigation, URL included: returning silently
// used to leave the address bar naming a terminal the app was not showing, which
// is the exact URL-versus-state disagreement this router exists to remove. There
// is no not-found screen for a terminal, deliberately, since terminal ids are
// ephemeral and a closed terminal is ordinary rather than a broken link.
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
// The `changes` half of the route rides along with the target: the intent is a
// whole POSITION, not just a focus. Arming from a target alone used to strand a
// user reading changed files twice over, first because the anchored
// `parseSelectionHash` read `#/agent/<sid>/changes` as no link at all, and then
// because the restore would have dropped them onto the terminal screen.
interface ReconnectDeepLink {
  target: SelectedTarget
  changes: boolean
  armedAt: number
}
let reconnectDeepLink: ReconnectDeepLink | null = null

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
  // A handler per owner variant, not a predicate plus a nullable id.
  //
  // This used to test `owner.kind === "project"` and send everything else
  // through `ownerSessionId`, so a third kind of owner would have answered null
  // and had its restoration intent silently dropped: the user's position would
  // quietly fail to come back after a reconnect, with nothing anywhere saying
  // why. That is precisely the class of silence the owner tag exists to remove,
  // and a predicate cannot remove it, because a predicate keeps compiling. The
  // matcher's object literal is missing a key the moment a variant is added,
  // which is a compile error HERE, where the decision is.
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

// The AGENTLESS half of `restoreReconnectDeepLink`: a terminal owned by a
// project, or by nothing at all. Both restore on identical terms, which is why
// they share this rather than getting two near-copies.
//
// Neither has a resume phase and neither pane issues the reconnect eject (that
// path is gated on the agent session-slot tab), so the selection normally
// survives a reconnect on its own and this usually just disarms as a no-op. The
// one restorable gap is a selection cleared by OUR OWN eject while the intent
// was armed; any deliberate navigation (a non-null selection that is not the
// armed terminal, or a home nav without the eject flag) disarms instead.
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
  selectSessionRoute(id, undefined)
}

// The screen half of a route commit. A selection carries it so the target and
// the screen land in ONE state patch, which is what lets `syncUrl` write a whole
// route; `undefined` means "the ordinary derivation" (a target is the terminal
// screen), and only a route that explicitly names `/changes` passes `true`.
function screenPatch(changes?: boolean): { mobileScreen: MobileScreen } | object {
  return changes ? { mobileScreen: "changes" as const } : {}
}

// `selectSession` with control over how the URL is written. `urlMode:
// "replace"` is for a move the user did not make: a restore, or the destination
// chosen when what they were looking at vanished. `changes` restores the changes
// screen for a route that names it.
function selectSessionRoute(
  id: string | null,
  urlMode?: "replace",
  changes?: boolean,
): void {
  // Any deliberate selection (to an agent OR to null/home) means the user took
  // control. See `ejectSelectionForReconnect` below for the one carve-out.
  lastClearWasReconnectEject = false
  const prev = state.selectedSessionId
  if (id === null) {
    clearSelection(urlMode)
    return
  }
  const session = state.spine?.sessions.find((s) => s.id === id)
  const focusedTab = session ? resolveFocusedTab(session) : id
  if (focusedTab !== id) {
    // A remembered extra tab is still live: select it directly so the
    // hash/changes wiring and the persistence write match an explicit tab
    // click exactly.
    selectTab(id, focusedTab, { urlMode, changes })
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
    ...screenPatch(changes),
  })
  // Move the per-session changed-files subscription, THEN fetch — subscribing
  // before the GET means an invalidation that races the fetch is never missed.
  switchChangesSubscription(prev, id)
  syncUrl(urlMode)
  if (prev !== id) loadChanges(id)
}

// Drop the focused target and land on home. The target is cleared FIRST so any
// synchronous re-render shows the fallback; the URL is written after, and the
// screen follows the empty target (see `setState`).
function clearSelection(urlMode?: "replace"): void {
  const prev = state.selectedSessionId
  setState({
    selectedTarget: null,
    selectedSessionId: null,
    changes: emptyChanges(),
  })
  // Drop the previous session's changed-files subscription; there is no global
  // watch to clear, so the cross-client clobber is gone by construction.
  switchChangesSubscription(prev, null)
  syncUrl(urlMode)
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
  // A replace, not a push: the eject is transient (the reconnect re-restore
  // undoes it), so it must not leave a home entry between the user and the
  // agent they were on.
  selectSessionRoute(null, "replace")
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
  opts?: { persist?: boolean; urlMode?: "replace"; changes?: boolean },
): void {
  const prev = state.selectedSessionId
  setState({
    selectedTarget: { kind: "agent", sessionId, tabId },
    selectedSessionId: sessionId,
    changes: prev === sessionId ? state.changes : loadingChanges(sessionId),
    ...screenPatch(opts?.changes),
  })
  switchChangesSubscription(prev, sessionId)
  syncUrl(opts?.urlMode)
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
export function selectTerminal(
  terminalId: string,
  owner: TerminalOwnerRef,
  opts?: { urlMode?: "replace"; changes?: boolean },
): void {
  const prev = state.selectedSessionId
  // Lossy on purpose: `selectedSessionId` exists to scope session-only UI, so
  // "is this owner a session" is the entire question. Any other owner leaves it
  // null, which is exactly the state a project terminal already puts it in.
  const sessionId = ownerSessionId(owner)
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
    ...screenPatch(opts?.changes),
  })
  // The changed files belong to the SESSION, so subscribe/fetch the parent
  // session even when a companion terminal is the streamed target; a project
  // terminal drops the subscription entirely.
  switchChangesSubscription(prev, sessionId)
  syncUrl(opts?.urlMode)
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
      toast.error(
        e instanceof Error
          ? e.message
          : "Could not create the standalone terminal.",
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

// Resolve a terminal's owner from the spine. The terminal CARRIES its owner now,
// so this is a lookup by id and a conversion rather than a scan of two nested
// collections in which "which list did I find it in" was the answer. Undefined
// when the terminal has already vanished.
export function findTerminalOwner(
  terminalId: string,
): TerminalOwnerRef | undefined {
  const terminal = state.spine?.terminals.find((t) => t.id === terminalId)
  return terminal ? ownerRefFromWire(terminal.owner) : undefined
}

// Close (delete) a companion terminal via REST (Phase 5). The endpoint is nested
// under the owner, so resolve it from the spine across BOTH owner kinds,
// sessions and projects (a session-only scan silently made project terminals
// undeletable); a terminal that already vanished (no owner) is a no-op. The
// terminal is removed from the workspace spine, and if it was the focused target
// the selection clears via the spine prune in `applySpine` (driven by the
// `sessions.changed` refetch). A failure surfaces as a toast.
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

export function deleteTerminal(terminalId: string): void {
  const owner = findTerminalOwner(terminalId)
  if (owner === undefined) return
  const request = terminalDeleteRequest(owner, terminalId)
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

// Open the name dialog in "from PR" mode.
//
// `projectId` is the project-first shape, opened from a project's own menu, and
// behaves exactly as it always did. `null` is the reference-first shape, opened
// from the global command: no project is chosen and none is asked for, and the
// reference decides which project the agent lands in.
export function openCreateAgentFromPr(projectId: string | null): void {
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

// Editing either field of the reference-first dialog retires whatever resolve
// is out for it, exactly as cancelling, retargeting and resubmitting already
// do. The reply carries the reference and the name AS THEY WERE AT SUBMIT, so
// letting it land after an edit creates the agent from text the user has
// already replaced and then closes the dialog on them. Nothing can recall a
// request in flight, so the reply has to arrive on a generation nobody is
// waiting for. Returns the patch to fold into the caller's own write, and
// nothing at all when no resolve is out, so an ordinary keystroke writes
// exactly what it used to.
function retireInFlightPrResolve(): Partial<DuxState> {
  if (state.createAgentPrRequestId === null) return {}
  return { createAgentPrRequestId: null, createAgentPrResolving: false }
}

// Update the PR-reference field. Free text — unlike the agent name, this is NOT
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
//   ON  -> request a fresh name (the reply fills the input via `onAgentName`).
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
      // Unchecking abandons any in-flight request; its reply is ignored by
      // `onAgentName` (randomize is false by then), so stop the spinner now.
      createAgentNamePending: false,
      ...retireInFlightPrResolve(),
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
  onlyIds: string[] | null = null,
): void {
  setState({
    newAgentPickerOpen: true,
    newAgentPickerIntent: intent,
    newAgentPickerOnlyIds: onlyIds,
  })
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
// `#`. It names NO repository, so with no project chosen there is nothing for a
// resolve to look for and the server would only refuse it. The refusal belongs
// here, in the field, next to the action that fixes it.
//
// This is deliberately the ONLY shape refused in the browser. The full
// reference grammar lives in Rust (`dux_core::pr_reference`) and reimplementing
// it here would be two grammars drifting apart; every other refusal comes back
// from the server, which stays the second line of defence for this one too.
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
        armCreateFocus(projectId)
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
        toast.error(
          resolved.uninspected_summary
            ? `No project dux could check is a checkout of ${repository}, and dux could not check every project (${resolved.uninspected_summary}). Choose a project that already has it, or add one from a directory on disk.`
            : `No project in dux is a checkout of ${repository}. Choose a project that already has it, or add one from a directory on disk.`,
        )
      } else {
        toast.info(
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
      toast.error(
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
  } else if (target.projectId === null) {
    // Reference-first: dux has to work out the project before anything is
    // created, so the dialog stays open until the answer arrives.
    submitPrReferenceFirst(state.createAgentPrInput.trim(), name)
    return
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

// Open the mobile changes screen over the focused agent. It is a position of its
// own in the URL (`#/agent/<sid>/changes`), so entering it pushes an entry and
// the browser's Back leaves it. There is no matching "go to the terminal screen"
// or "go home" call: focusing a target IS the terminal screen and clearing the
// target IS home, both through the ordinary selection functions.
export function openChangesScreen(): void {
  if (!state.selectedTarget || state.mobileScreen === "changes") return
  setState({ mobileScreen: "changes" })
  syncUrl()
}

// The destination one level UP from where the app is: the changes screen sits on
// top of the agent it belongs to, everything else sits on top of home. This is
// what the mobile shell's chevrons and the not-found screen's way out call, and
// it is emphatically NOT the browser's Back.
//
// Up must name a destination, because a relative step has no floor. A deep-link
// boot pushes nothing, so on that first screen dux's own entry IS the bottom of
// the stack and stepping back walks out of the application onto whatever page
// preceded it, which is the bug this whole model exists to remove.
//
// Up from a REAL screen PUSHES, because it is ordinary navigation to a different
// position and the browser is supposed to accumulate those: Back then alternates
// meaningfully between where you were and where you went. It used to replace,
// and that quietly grew the stack by one entry per trip, since going in pushed
// and coming out overwrote the top: ten trips in and out left ten identical home
// entries, so Back did nothing visible ten times before it did anything at all.
//
// Leaving the NOT-FOUND screen is the one exception, and it replaces. That is
// not navigation between two positions, it is a CORRECTION of a bad URL, and a
// corrected URL is not a position worth keeping: pushing there would put the
// dead end exactly one Back away, so the user's way out would bounce them
// straight back onto it.
export function navigateUp(): void {
  const urlMode = state.routeNotFound ? ("replace" as const) : undefined
  if (state.mobileScreen === "changes" && state.selectedTarget) {
    setState({ mobileScreen: "terminal" })
    syncUrl(urlMode)
    return
  }
  selectSessionRoute(null, urlMode)
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
  }
  // One flat collection, so every terminal of every owner is reached by one
  // loop: the panic button cannot miss a whole owner kind the way it once could
  // have missed project terminals.
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
// `config.changed` refetch — hence the three guards below, each of which is the
// difference between "shown once" and "pops up while you work".
function offerAutomaticFirstLoad(pending: PendingFirstLoad | null): void {
  if (pending === null) {
    // The server has no pending screen. If THIS tab is showing the AUTOMATIC one,
    // it has been settled elsewhere — another browser tab dismissed it, and the
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
// screen only. A failure lands in the dialog body AND a toast — never silent.
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
      toast.error(message)
      if (state.firstLoad === null) return
      if (state.firstLoad.screen !== "whats_new") return
      setState({
        firstLoad: { ...state.firstLoad, loading: false, error: message },
      })
    })
}

// Close the first-load dialog. Closing an AUTOMATIC screen also dismisses it:
// the server records the running version as seen in SQLite, the one row the TUI
// reads too, so the screen is settled on both surfaces. An on-demand open
// dismisses nothing.
//
// The close is optimistic and unconditional — a failed dismissal must not trap
// the user behind a modal — and the re-open guard is set SYNCHRONOUSLY, in the
// same `setState` that clears the dialog, then ROLLED BACK if the write fails. It
// cannot wait for the POST to resolve: a `config.changed` arriving in that window
// re-runs `applyBootstrap` → `offerAutomaticFirstLoad`, whose guards would both
// pass, and the just-dismissed dialog would reopen. The rollback is what keeps the
// failure behaviour honest — a failed write leaves the screen genuinely pending
// for the next load rather than silently swallowing it.
export function closeFirstLoad(): void {
  const open = state.firstLoad
  if (open === null) return
  if (!open.automatic) {
    setState({ firstLoad: null })
    // Nothing to dismiss — but the offer may have been DROPPED while this dialog
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
    toast.error(
      e instanceof Error
        ? e.message
        : "Could not record this screen as seen; it may appear again.",
    )
  })
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
