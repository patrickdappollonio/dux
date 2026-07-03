// Pure helpers for the agent tab strip, kept out of the component so they are
// unit-testable without mounting React. Mirrors the TUI's `tab_labels` /
// strip-visibility logic (a shared fixture keeps the two in parity).

import type { AgentTabView } from "./types"
import type { SelectedTarget } from "./store"

// Whether the focused target is a Support tab that is currently DORMANT (reopened
// after a restart with no live process, and not yet explicitly started this
// session). A dormant tab must render its "Start fresh session" card WITHOUT
// mounting the terminal pane, because mounting subscribes to the PTY socket which
// force-launches the provider — so focus alone must never launch it. The Main tab
// (`focusedTab.id === target.sessionId`) is never dormant.
export function isSupportTabDormant(
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

// Whether the tab strip should render for a session: only when there are two or
// more tabs. A single-tab agent shows today's chrome-free pane.
export function shouldShowTabStrip(tabs: AgentTabView[]): boolean {
  return tabs.length >= 2
}

// Display labels for a session's tabs: the provider name, disambiguated with a
// trailing " 2", " 3", … for the k-th occurrence of a repeated provider (in tab
// order). The first occurrence stays bare. Order matches the input (Main first).
export function tabLabels(tabs: AgentTabView[]): string[] {
  const seen = new Map<string, number>()
  return tabs.map((tab) => {
    const n = (seen.get(tab.provider) ?? 0) + 1
    seen.set(tab.provider, n)
    return n === 1 ? tab.provider : `${tab.provider} ${n}`
  })
}
