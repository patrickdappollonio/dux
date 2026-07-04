// Pure helpers for the agent tab strip, kept out of the component so they are
// unit-testable without mounting React. Mirrors the TUI's `tab_labels` /
// strip-visibility logic (a shared fixture keeps the two in parity).

import type { AgentTabView } from "./types"
import type { SelectedTarget } from "./store"

// Whether the focused target is an extra tab that is currently DORMANT (reopened
// after a restart with no live process, and not yet explicitly started this
// session). A dormant tab must render its "Start session" card WITHOUT mounting
// the terminal pane, because mounting subscribes to the PTY socket which
// force-launches the provider — so focus alone must never launch it. The
// session-slot tab (`focusedTab.id === target.sessionId`) is never dormant.
export function isExtraTabDormant(
  target: SelectedTarget | null,
  focusedTab: AgentTabView | undefined,
  startedDormantTabs: string[],
): boolean {
  return (
    !!target &&
    target.kind === "agent" &&
    !!focusedTab &&
    focusedTab.id !== target.sessionId &&
    !focusedTab.has_live_process &&
    !startedDormantTabs.includes(focusedTab.id)
  )
}

// Whether an extra tab that used to exist for a session is no longer present in
// the spine's tab list (e.g. another client closed it while this client's PTY
// socket was retrying). A gone tab's socket must stop reconnecting instead of
// retrying forever against a route that will keep 404ing. Only meaningful for an
// extra tab (a session-slot tab has no row of its own; its owning session's
// presence is the authoritative signal there, handled separately).
export function isTabGone(tabs: AgentTabView[], tabId: string): boolean {
  return !tabs.some((t) => t.id === tabId)
}

// Whether the tab strip should render for a session: only when there are two or
// more tabs. A single-tab agent shows today's chrome-free pane.
export function shouldShowTabStrip(tabs: AgentTabView[]): boolean {
  return tabs.length >= 2
}

// Display labels for a session's tabs: the provider name, disambiguated with a
// trailing " 2", " 3", … for the k-th occurrence of a repeated provider (in tab
// order). The first occurrence stays bare. Order matches the input (the
// session-slot tab first).
export function tabLabels(tabs: AgentTabView[]): string[] {
  const seen = new Map<string, number>()
  return tabs.map((tab) => {
    const n = (seen.get(tab.provider) ?? 0) + 1
    seen.set(tab.provider, n)
    return n === 1 ? tab.provider : `${tab.provider} ${n}`
  })
}
