// Pure helpers for the agent tab strip, kept out of the component so they are
// unit-testable without mounting React. Mirrors the TUI's `tab_labels` /
// strip-visibility logic (a shared fixture keeps the two in parity).

import type { Spine } from "./workspaceApi"
import type { AgentTabView, SessionView } from "./types"
import type { SelectedTarget } from "./store"
import { workspaceProjectId } from "@/lib/agentWorkspace"

// The agent's first tab, named by the server on `SessionView.slot_tab_id`.
// Every slot-ness decision with a session in hand asks this, so that nothing
// compares a tab id against a session id itself.
export function isFirstTab(session: SessionView, tabId: string): boolean {
  return tabId === session.slot_tab_id
}

// Mirror of dux-core's `agent_tabs::ONLY_TAB_CLOSE_REFUSAL`, which the server's
// own 400 carries: the browser refuses the gesture up front and must refuse in
// exactly the same words. Pinned as a literal by the test below.
export const ONLY_TAB_CLOSE_REFUSAL =
  "This is the agent's only tab, so closing it would leave the agent with no tab at all. Detach the agent instead to stop everything it is running, or add another tab first."

// Slot-ness for layers that hold two ids and no session record (the URL
// grammar, the PTY socket URL, the selection target). The session id is only a
// placeholder for the real slot tab id, so pass `slotTabId` whenever the spine
// has published it: without it this says "not the slot tab" about the slot tab.
// Callers holding a `SessionView` use `isFirstTab` instead.
export function isSlotTabTarget(
  sessionId: string,
  tabId: string,
  slotTabId?: string,
): boolean {
  return tabId === (slotTabId ?? slotTabTargetId(sessionId))
}

// The placeholder tab id the id-only layers use to mean "this agent's first
// tab, whichever it is", resolved to the real slot tab id as soon as the spine
// names one. Twin of `isSlotTabTarget`; see its note.
export function slotTabTargetId(sessionId: string): string {
  return sessionId
}

// What a close of the slot tab leaves behind until a spine catches up. Keyed on
// the closed tab because that is the fact a spine can retire; keying it on the
// promoted id would pin a dead answer once another promotion moved the slot on.
export interface PendingSlotTab {
  closedTabId: string
  promotedTabId: string
}

// The tab holding a session's slot as far as THIS client knows: a promotion it
// just performed wins over the not-yet-caught-up spine. Every reader of the
// slot goes through here, or the overlay applies to some questions and not
// others and a promoted tab briefly reads as an extra one, covered by a
// Start-session card nobody asked for. `undefined` when neither knows it.
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
//   dormant slot tab whose      card, as the diagnosis surface: it would
//     last run ENDED BADLY      otherwise relaunch every time the user looks
//     (`last_run_failed`)       at it, with no way out
//   started by this client      no card: the press is sent and the spine has not
//     (`startedDormantTabs`)    caught up (see `startDormantTab`)
//   no session                  card: defensive only, since callers derive
//                               `focusedTab` from a session's own tab list
//
// Pass `slotTabId` (`slotTabIdOf`), the client's live slot answer: without it a
// just-promoted tab is judged an extra while the spine is stale and flashes the
// card at a user who asked for nothing of the kind.
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

// Whether an exited agent should drop the user back to the welcome screen. A
// run that ended badly does not eject: that tab's dormant card is the diagnosis
// surface and the welcome screen would replace it. Gated on `everReady` so a
// pane that never came up cannot eject on a status it never saw change, and on
// slot-ness because an extra tab's exit only turns that tab dormant.
export function exitEjectsToWelcome(
  isSessionSlotTab: boolean,
  everReady: boolean,
  sessionStatus: string | undefined,
  lastRunFailed: boolean,
): boolean {
  if (!isSessionSlotTab || !everReady) return false
  if (!sessionStatus || sessionStatus === "active") return false
  return !lastRunFailed
}

// Whether an extra tab has vanished from the spine's tab list: its socket must
// stop reconnecting rather than retrying against a route that keeps 404ing.
// Only meaningful for an extra tab, since the slot tab's disappearance is its
// whole agent's and is handled separately.
export function isTabGone(tabs: AgentTabView[], tabId: string): boolean {
  return !tabs.some((t) => t.id === tabId)
}

// Whether the tab strip renders: two or more tabs, or the operator's
// `bootstrap.always_show_tab_strip` opt-in. A single-tab agent otherwise gets a
// chrome-free pane.
export function shouldShowTabStrip(
  tabs: AgentTabView[],
  alwaysShow = false,
): boolean {
  return alwaysShow || tabs.length >= 2
}

// Display labels for a session's tabs: the provider name, with a trailing " 2",
// " 3", … for repeats in tab order. Output order matches the input.
//
// Twin of dux-core `agent_tabs::tab_labels`, which owns the rule; pinned by
// shared vectors. Keep the two in lockstep.
export function tabLabels(tabs: AgentTabView[]): string[] {
  const seen = new Map<string, number>()
  return tabs.map((tab) => {
    const n = (seen.get(tab.provider) ?? 0) + 1
    seen.set(tab.provider, n)
    return n === 1 ? tab.provider : `${tab.provider} ${n}`
  })
}

// How prose names one of a session's tabs: its strip label, first character
// upper-cased, disambiguating suffix included. `undefined` for a tab this
// session does not have.
//
// Twin of dux-core's `Engine::tab_prose_label` / `agent_tabs::prose_tab_label`,
// which the server's own status messages are built from, so a confirmation here
// and the toast that follows it cannot name the tab differently.
export function tabProseLabel(
  tabs: AgentTabView[],
  tabId: string,
): string | undefined {
  const i = tabs.findIndex((t) => t.id === tabId)
  if (i < 0) return undefined
  const label = tabLabels(tabs)[i]
  return label.charAt(0).toUpperCase() + label.slice(1)
}

// What closing one tab costs, as the close confirmation states it. Derived
// together because they are one reading of the same session.
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
  return {
    sessionLabel: provider ? `the ${provider} session` : "the session",
    willDetach: closeDetachesAgent(session, tab),
    successorLabel: slotSuccessorLabel(session, tab),
  }
}

// Whether closing this tab leaves the agent with nothing running.
//
// Counted by LIVENESS: dormant siblings left by a restart do not keep the agent
// running, so they cannot save it from detaching.
export function closeDetachesAgent(
  session: SessionView | undefined,
  tab: AgentTabView | undefined,
): boolean {
  const liveTabs = session?.tabs.filter((t) => t.has_live_process).length ?? 0
  return (tab?.has_live_process ?? false) ? liveTabs <= 1 : liveTabs === 0
}

// The prose name of the tab that takes the session slot, absent unless the slot
// tab is the one closing.
//
// The successor is the first tab that is not this one, which relies on
// `SessionView.tabs` arriving slot-tab-first then extras in strip order (its
// contract in `lib/types.ts`), the same ordering the engine promotes by.
export function slotSuccessorLabel(
  session: SessionView | undefined,
  tab: AgentTabView | undefined,
): string | undefined {
  if (!session || !tab || !isFirstTab(session, tab.id)) return undefined
  const successor = session.tabs.find((t) => t.id !== tab.id)
  return successor ? tabProseLabel(session.tabs, successor.id) : undefined
}

// The tab id a session focuses when reached by the sidebar or the bare
// `#/agent/:id` route; an explicit `#/agent/:id/tab/:t` deep link wins over this
// in `restoreDeepLink` (`store.ts`), which this helper leaves alone. Mirrors
// `AgentSession::resolved_focused_tab` in dux-core: the remembered
// `last_focused_tab` wins only while it still names a tab in `session.tabs`,
// otherwise the slot tab does.
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

// Whether a settled fire-and-forget `PUT .../focused-tab` needs a corrective
// re-issue. Rapid `selectTab` calls settle out of order, so a session keeps only
// its latest intended `(generation, tabId)`; a stale generation says nothing
// about server ordering, so only a settled value differing from the current
// intent is worth re-firing.
export function shouldRefireFocusPut(
  latest: { generation: number; tabId: string | null },
  settled: { generation: number; tabId: string | null },
): boolean {
  return latest.generation !== settled.generation && latest.tabId !== settled.tabId
}

// The provider a plain `addTab(session.id)` launches, so the "+" quick-add and
// its picker's "default" marker cannot disagree with what starts. Twin of core's
// `Engine::default_provider_for_new_tab`: the spine's `project.default_provider`
// is already the effective value, so for a project in the spine this agrees with
// the server. The session's own `provider` is only the fallback for a session
// whose project is missing from the spine.
export function defaultProviderForSession(
  spine: Spine | null,
  session: SessionView,
): string {
  const project = spine?.projects.find(
    (p) => p.id === workspaceProjectId(session.workspace),
  )
  return project?.default_provider ?? session.provider
}
