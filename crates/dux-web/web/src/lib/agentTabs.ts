// Pure helpers for the agent tab strip, kept out of the component so they are
// unit-testable without mounting React. Mirrors the TUI's `tab_labels` /
// strip-visibility logic (a shared fixture keeps the two in parity).

import type { Spine } from "./spineApi"
import type { AgentTabView, SessionView } from "./types"
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
export function tabLabels(tabs: AgentTabView[]): string[] {
  const seen = new Map<string, number>()
  return tabs.map((tab) => {
    const n = (seen.get(tab.provider) ?? 0) + 1
    seen.set(tab.provider, n)
    return n === 1 ? tab.provider : `${tab.provider} ${n}`
  })
}

// Whether a session's current branch has drifted from the immutable branch the
// agent was created on. `drifted` is true only when `initial_branch` is present
// (an older server omits it, coerced to `""` at ingestion) and truly differs from
// the current `branch_name`. `initial` is passed through so callers that surface
// the original branch don't re-read it. Shared by the header drift crumb and the
// agent info dialog so the two never disagree.
export function branchDrift(
  session: Pick<SessionView, "branch_name" | "initial_branch">,
): { drifted: boolean; initial: string } {
  return {
    drifted:
      !!session.initial_branch &&
      session.initial_branch !== session.branch_name,
    initial: session.initial_branch,
  }
}

// The provider a plain (no-provider-arg) `addTab(session.id)` actually launches:
// the session's owning project's `default_provider`, mirroring the server's own
// resolution (`CreateTabBody.provider` omitted → project default). Falls back to
// the session's own `provider` field only when the owning project can't be found
// in the spine (should not happen in practice, but keeps the "+" quick-add and
// its picker's "default" marker from silently disagreeing with what gets
// launched).
export function defaultProviderForSession(
  spine: Spine | null,
  session: SessionView,
): string {
  const project = spine?.projects.find((p) => p.id === session.project_id)
  return project?.default_provider ?? session.provider
}
