// Pure helpers for the agent tab strip, kept out of the component so they are
// unit-testable without mounting React. Mirrors the TUI's `tab_labels` /
// strip-visibility logic (a shared fixture keeps the two in parity).

import type { Spine } from "./workspaceApi"
import type { AgentTabView, SessionView } from "./types"
import type { SelectedTarget } from "./store"
import { workspaceProjectId } from "@/lib/agentWorkspace"

// The agent's FIRST tab: the session-slot tab, named by the server on
// `SessionView.slot_tab_id`. Closing it hands the slot to the next tab in strip
// order, so it is closable like any other; what cannot be closed is an agent's
// ONLY tab. Every slot-ness decision with a session in hand asks this, and
// nothing compares a tab id against a session id itself.
export function isFirstTab(session: SessionView, tabId: string): boolean {
  return tabId === session.slot_tab_id
}

// The one sentence dux says when closing a tab would leave an agent with no tab
// at all. MIRROR of dux-core's `agent_tabs::ONLY_TAB_CLOSE_REFUSAL`, which is
// what the server's own 400 carries: the browser refuses the gesture up front
// rather than letting the user through a dialog into that refusal, and it must
// refuse in exactly the same words. Pinned as a literal by the test below, the
// way the `tabLabels` vectors are.
export const ONLY_TAB_CLOSE_REFUSAL =
  "This is the agent's only tab, so closing it would leave the agent with no tab at all. Detach the agent instead to stop everything it is running, or add another tab first."

// Slot-ness for the layers that hold two ids and no session record: the URL
// grammar (which parses a hash before any spine has arrived), the PTY socket
// URL choice, and the selection target those two build.
//
// An agent's slot tab id is a GENERATED id its session merely points at, so the
// session id is only a PLACEHOLDER for "whichever tab is in the slot" and is
// right just while the real one is unknown. Pass `slotTabId` whenever the spine
// has published it; every answer that gates behavior must, because without it
// this says "not the slot tab" about the slot tab. Callers holding a
// `SessionView` use `isFirstTab` instead.
export function isSlotTabTarget(
  sessionId: string,
  tabId: string,
  slotTabId?: string,
): boolean {
  return tabId === (slotTabId ?? slotTabTargetId(sessionId))
}

// The PLACEHOLDER tab id those same id-only layers use to mean "this agent's
// first tab, whichever it is": the bare `#/agent/<sid>` address, the standalone
// editor's flattened target, and a fresh selection of an agent. It is resolved
// to the real slot tab id as soon as the spine names one. Twin of
// `isSlotTabTarget`; see its note.
export function slotTabTargetId(sessionId: string): string {
  return sessionId
}

// What a close of the SLOT tab leaves behind until a spine catches up: which
// tab the close destroyed, and which tab took the slot. Keyed on the CLOSED tab
// because that is the fact a spine can retire (the tab is gone from the
// session's list); keying it on the promoted id would pin a dead answer forever
// as soon as somebody else's promotion moved the slot on again.
export interface PendingSlotTab {
  closedTabId: string
  promotedTabId: string
}

// The tab holding a session's slot as far as THIS client knows. A promotion it
// just performed wins over the spine, which has not caught up with the close
// yet; otherwise the spine answers. `undefined` when neither knows the session.
// Every reader of the slot goes through here so the overlay cannot apply to
// some questions and not others: were the card rule and a pane's own slot-ness
// question to disagree, a promoted tab would briefly read as an extra one and
// get covered by the Start-session card nobody asked for. A target with no
// owning session has no slot to ask about, which is what an empty key resolves
// to.
export function slotTabIdOf(
  sessionId: string,
  session: SessionView | undefined,
  pending: Record<string, PendingSlotTab>,
): string | undefined {
  return pending[sessionId]?.promotedTabId ?? session?.slot_tab_id
}

// Whether the focused tab must render the "Start session" card INSTEAD of the
// terminal pane. Mounting the pane subscribes to the PTY socket, and subscribing
// starts a dormant tab, so this one answer decides both what is on screen and
// whether the tab launches.
//
//   live tab                    no card
//   dormant extra tab           card: the user added it deliberately, so it
//                               stays put until a press asks for it
//   dormant slot tab            no card: selecting an agent is asking for it,
//                               and starting in one click is the whole gesture
//   dormant slot tab whose      card, as the diagnosis surface: a resume against
//     last run ENDED BADLY      a conversation that is not there, or a provider
//     (`last_run_failed`)       gone from the PATH, otherwise relaunches every
//                               time the user looks at it with no way out
//   started by this client      no card: the press is sent and the spine has not
//     (`startedDormantTabs`)    caught up (see `startDormantTab`)
//
// A missing `session` is DEFENSIVE, not a case the rule turns on: both callers
// derive `focusedTab` from that same session's tab list, so a tab with no session
// cannot reach here. The branch falls to the cautious side rather than guessing.
//
// `slotTabId` is the client's live answer to which tab holds the slot
// (`slotTabIdOf`), which the promotion overlay knows before the spine does:
// without it a just-promoted tab is judged an extra for as long as the spine is
// stale and flashes the card at a user who asked for nothing of the kind.
export function dormantTabNeedsCard(
  target: SelectedTarget | null,
  session: SessionView | undefined,
  focusedTab: AgentTabView | undefined,
  startedDormantTabs: string[],
  slotTabId?: string,
): boolean {
  if (!target || target.kind !== "agent") return false
  if (!focusedTab || focusedTab.has_live_process) return false
  if (startedDormantTabs.includes(focusedTab.id)) return false
  const slot = slotTabId ?? session?.slot_tab_id
  if (!session || focusedTab.id !== slot) return true
  return focusedTab.last_run_failed === true
}

// Whether an extra tab has vanished from the spine's tab list (e.g. another
// client closed it while this client's PTY
// socket was retrying). A gone tab's socket must stop reconnecting instead of
// retrying forever against a route that will keep 404ing. Only meaningful for an
// extra tab (the session-slot tab's disappearance is its whole agent's, which
// is the authoritative signal there and is handled separately).
export function isTabGone(tabs: AgentTabView[], tabId: string): boolean {
  return !tabs.some((t) => t.id === tabId)
}

// Whether the tab strip should render for a session: when there are two or more
// tabs, or when the operator has opted into always showing it
// (`bootstrap.always_show_tab_strip`, `config.ui.always_show_tab_strip`). A
// single-tab agent shows today's chrome-free pane unless that preference is on.
export function shouldShowTabStrip(
  tabs: AgentTabView[],
  alwaysShow = false,
): boolean {
  return alwaysShow || tabs.length >= 2
}

// Display labels for a session's tabs: the provider name, disambiguated with a
// trailing " 2", " 3", … for the k-th occurrence of a repeated provider (in tab
// order). The first occurrence stays bare. Order matches the input (the
// session-slot tab first).
//
// TWIN of dux-core `agent_tabs::tab_labels` (the core-owned rule); pinned by
// shared vectors (`agentTabs.test.ts` mirrors `agent_tabs.rs`'s tests). Keep the
// two in lockstep.
export function tabLabels(tabs: AgentTabView[]): string[] {
  const seen = new Map<string, number>()
  return tabs.map((tab) => {
    const n = (seen.get(tab.provider) ?? 0) + 1
    seen.set(tab.provider, n)
    return n === 1 ? tab.provider : `${tab.provider} ${n}`
  })
}

// The way PROSE names one of a session's tabs: its strip label with the first
// character upper-cased. Pills are lower-case because they are chrome; a
// sentence names a tab the way a sentence names anything, and the
// disambiguating suffix rides along, so "Codex 2" in a confirmation is the pill
// the user is looking at. `undefined` for a tab this session does not have.
//
// TWIN of dux-core's `Engine::tab_prose_label` / `agent_tabs::prose_tab_label`,
// which is what the server's own status messages are built from, so a
// confirmation here and the toast that follows it cannot name the tab
// differently. `AgentTabView.provider` is already the EFFECTIVE provider (the
// running pin when a retarget happened mid-run), which is the same input the
// Rust side uses.
export function tabProseLabel(
  tabs: AgentTabView[],
  tabId: string,
): string | undefined {
  const i = tabs.findIndex((t) => t.id === tabId)
  if (i < 0) return undefined
  const label = tabLabels(tabs)[i]
  return label.charAt(0).toUpperCase() + label.slice(1)
}

// What closing one tab actually costs, as the close confirmation states it. The
// three answers are derived together because they are one reading of the same
// session, and kept pure so each branch is testable without mounting a dialog.
export interface CloseTabConsequences {
  /// Names the conversation the close ends, falling back when no provider is known.
  sessionLabel: string
  /// The close removes the agent's LAST live tab, so the agent detaches.
  willDetach: boolean
  /// The tab that takes the session slot, absent unless the slot tab is closing.
  successorLabel: string | undefined
}

export function closeTabConsequences(
  session: SessionView | undefined,
  tab: AgentTabView | undefined,
): CloseTabConsequences {
  const provider = tab?.provider
  // Detaching is counted by LIVENESS: dormant siblings left by a restart do not
  // keep the agent running, so they cannot save it from detaching.
  const liveTabs = session?.tabs.filter((t) => t.has_live_process).length ?? 0
  const willDetach = (tab?.has_live_process ?? false)
    ? liveTabs <= 1
    : liveTabs === 0
  // Closing the tab in the session slot promotes the next tab in strip order, so
  // the successor is the first tab that is not this one. That reading is sound
  // because `SessionView.tabs` is published slot-tab-first then extras in strip
  // order (see the field's contract on `AgentTabView`/`SessionView` in
  // `lib/types.ts`, which the ViewModel guarantees), which is the same ordering
  // the engine promotes by.
  const successor =
    session && tab && isFirstTab(session, tab.id)
      ? session.tabs.find((t) => t.id !== tab.id)
      : undefined
  return {
    sessionLabel: provider ? `the ${provider} session` : "the session",
    willDetach,
    successorLabel:
      successor && session
        ? tabProseLabel(session.tabs, successor.id)
        : undefined,
  }
}

// Branch drift moved to `branchDriftOf` in `lib/agentWorkspace.ts`, beside the
// rest of the workspace questions: the two branches it compares now live inside
// the managed shape, and a standalone agent has neither, so the answer belongs
// with the matcher that can say so.

// Resolve the tab id a session should focus when the user navigates to it via
// the sidebar or the bare `#/agent/:id` route (an explicit `#/agent/:id/tab/:t`
// deep link always wins over this — see `restoreDeepLink` in `store.ts`, which
// is intentionally untouched by this helper). Mirrors the shared resolution rule
// (`AgentSession::resolved_focused_tab` in dux-core): the remembered
// `last_focused_tab` wins only when it is present AND still names a live tab in
// `session.tabs`; otherwise (no memory, it names the session-slot tab, or it
// names a tab that has since closed) falls back to the session-slot tab.
export function resolveFocusedTab(session: SessionView): string {
  const remembered = session.last_focused_tab
  if (
    remembered &&
    !isFirstTab(session, remembered) &&
    session.tabs.some((t) => t.id === remembered)
  ) {
    return remembered
  }
  return session.slot_tab_id
}

// Decide whether a fire-and-forget `PUT .../focused-tab` response must trigger
// a corrective re-issue. `selectTab` may be called in rapid succession (fast
// tab switching), and network responses can settle out of order, so each
// session keeps only its LATEST intended `(generation, tabId)`. When a
// response settles for a stale generation, its ordering at the server tells
// us nothing, so we only re-fire when the settled value actually differs from
// the current intent (matching generations, or a stale generation that
// happens to already carry the right value, need no correction).
export function shouldRefireFocusPut(
  latest: { generation: number; tabId: string | null },
  settled: { generation: number; tabId: string | null },
): boolean {
  return latest.generation !== settled.generation && latest.tabId !== settled.tabId
}

// The provider a plain (no-provider-arg) `addTab(session.id)` actually launches:
// the session's owning project's `default_provider`. This is the client-side
// TWIN of core's `Engine::default_provider_for_new_tab` (the single Rust source
// both server surfaces call, `crates/dux-core/src/engine/mod.rs`): the spine's
// `project.default_provider` is already the effective value (an explicit project
// override, else the global config default resolved server-side), so for a
// project present in the spine this agrees with the server byte-for-byte. It
// falls back to the session's own `provider` only when the owning project can't
// be found in the spine (unreachable in practice, since a session always ships
// with its project), keeping the "+" quick-add and its picker's "default" marker
// from silently disagreeing with what gets launched. Pinned by the tests below.
export function defaultProviderForSession(
  spine: Spine | null,
  session: SessionView,
): string {
  const project = spine?.projects.find(
    (p) => p.id === workspaceProjectId(session.workspace),
  )
  return project?.default_provider ?? session.provider
}
