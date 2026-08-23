// Pure helpers for the agent tab strip, kept out of the component so they are
// unit-testable without mounting React. Mirrors the TUI's `tab_labels` /
// strip-visibility logic (a shared fixture keeps the two in parity).

import type { Spine } from "./workspaceApi"
import type { AgentTabView, SessionView } from "./types"
import type { SelectedTarget } from "./store"
import { workspaceProjectId } from "@/lib/agentWorkspace"

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

// Whether an extra tab has vanished from the spine's tab list (e.g. another
// client closed it while this client's PTY
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
// `session.tabs`; otherwise (no memory, it equals the session-slot id, or it
// names a tab that has since closed) falls back to the session-slot tab
// (`session.id`).
export function resolveFocusedTab(session: SessionView): string {
  const remembered = session.last_focused_tab
  if (
    remembered &&
    remembered !== session.id &&
    session.tabs.some((t) => t.id === remembered)
  ) {
    return remembered
  }
  return session.id
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
