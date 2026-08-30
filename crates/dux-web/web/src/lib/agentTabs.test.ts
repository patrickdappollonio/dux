import { describe, expect, it } from "vitest"

import {
  defaultProviderForSession,
  isFocusedTabDormant,
  isFirstTab,
  isTabGone,
  resolveFocusedTab,
  shouldRefireFocusPut,
  shouldShowTabStrip,
  tabLabels,
} from "./agentTabs"
import { branchDriftOf } from "@/lib/agentWorkspace"
import type { Spine } from "./workspaceApi"
import type { SelectedTarget } from "./store"
import type { AgentTabView, SessionView } from "./types"

// A minimal AgentTabView for the pure helpers (only `provider` matters here).
function tab(provider: string): AgentTabView {
  return {
    id: `${provider}-id`,
    provider,
    order: 0,
    working: false,
    has_output: false,
    has_live_process: true,
  }
}

function agentTarget(sessionId: string, tabId: string): SelectedTarget {
  return { kind: "agent", sessionId, tabId }
}

function extraTab(id: string, live: boolean): AgentTabView {
  return { ...tab("codex"), id, has_live_process: live }
}

// SHARED VECTORS with dux-core `agent_tabs.rs` `branch_drifted`: these cases are
// mirrored there. A change to the drift rule in one language that is not mirrored
// fails a test on the other side.
describe("branchDriftOf", () => {
  it("flags drift when the current branch differs from the original", () => {
    expect(
      branchDriftOf({
        kind: "managed" as const,
        project_id: "",
        branch_name: "agent-tabs",
        initial_branch: "server-mode",
        branch_provenance: "created" as const,
        source_branch: "",
        worktree_path: "",
      }),
    ).toEqual({ drifted: true, initial: "server-mode" })
  })

  it("does not flag drift when current matches the original", () => {
    expect(
      branchDriftOf({
        kind: "managed" as const,
        project_id: "",
        branch_name: "server-mode",
        initial_branch: "server-mode",
        branch_provenance: "created" as const,
        source_branch: "",
        worktree_path: "",
      }),
    ).toEqual({ drifted: false, initial: "server-mode" })
  })

  it("does not flag drift when the original branch is missing (older server)", () => {
    expect(
      branchDriftOf({
        kind: "managed" as const,
        project_id: "",
        branch_name: "server-mode",
        initial_branch: "",
        branch_provenance: "created" as const,
        source_branch: "",
        worktree_path: "",
      }),
    ).toEqual({ drifted: false, initial: "" })
  })
})

describe("shouldShowTabStrip", () => {
  it("is false for zero or one tab, true for two or more", () => {
    expect(shouldShowTabStrip([])).toBe(false)
    expect(shouldShowTabStrip([tab("claude")])).toBe(false)
    expect(shouldShowTabStrip([tab("claude"), tab("codex")])).toBe(true)
    expect(shouldShowTabStrip([tab("a"), tab("b"), tab("c")])).toBe(true)
  })

  it("honors the always-show preference for zero or one tab, but does not affect two or more", () => {
    expect(shouldShowTabStrip([], true)).toBe(true)
    expect(shouldShowTabStrip([tab("claude")], true)).toBe(true)
    expect(shouldShowTabStrip([tab("claude")], false)).toBe(false)
    expect(shouldShowTabStrip([tab("claude"), tab("codex")], false)).toBe(true)
  })
})

describe("isFirstTab", () => {
  it("is true only for the tab whose id equals the session id", () => {
    expect(isFirstTab("s1", "s1")).toBe(true)
    expect(isFirstTab("s1", "tab-1")).toBe(false)
  })
})

describe("isFocusedTabDormant", () => {
  // The agent's FIRST tab is dormant like any other. It used to be excluded,
  // which made focusing a first tab left dormant by a restart launch it
  // silently.
  it("is true for a dormant first tab until it has been started", () => {
    const sessionSlot = { ...extraTab("s1", false), id: "s1" }
    expect(isFocusedTabDormant(agentTarget("s1", "s1"), sessionSlot, [])).toBe(
      true,
    )
    expect(
      isFocusedTabDormant(agentTarget("s1", "s1"), sessionSlot, ["s1"]),
    ).toBe(false)
  })

  it("is false for a live first tab", () => {
    const live = { ...extraTab("s1", true), id: "s1" }
    expect(isFocusedTabDormant(agentTarget("s1", "s1"), live, [])).toBe(false)
  })

  it("is true for an extra tab with no live process until it has been started", () => {
    const dormant = extraTab("tab-1", false)
    expect(isFocusedTabDormant(agentTarget("s1", "tab-1"), dormant, [])).toBe(true)
    // Once explicitly started, the card is suppressed for that tab id.
    expect(
      isFocusedTabDormant(agentTarget("s1", "tab-1"), dormant, ["tab-1"]),
    ).toBe(false)
  })

  it("is false for an extra tab that has a live process", () => {
    expect(
      isFocusedTabDormant(agentTarget("s1", "tab-1"), extraTab("tab-1", true), []),
    ).toBe(false)
  })

  it("is false for a terminal target or a missing focused tab", () => {
    const terminal: SelectedTarget = {
      kind: "terminal",
      terminalId: "t1",
      sessionId: "s1",
    }
    expect(isFocusedTabDormant(terminal, extraTab("tab-1", false), [])).toBe(false)
    expect(isFocusedTabDormant(agentTarget("s1", "tab-1"), undefined, [])).toBe(false)
    expect(isFocusedTabDormant(null, extraTab("tab-1", false), [])).toBe(false)
  })
})

describe("isTabGone", () => {
  it("is false when the tab id is still present in the spine's tab list", () => {
    expect(isTabGone([extraTab("tab-1", false), extraTab("tab-2", true)], "tab-1")).toBe(
      false,
    )
  })

  it("is true when the tab id is no longer present (closed elsewhere)", () => {
    expect(isTabGone([extraTab("tab-2", true)], "tab-1")).toBe(true)
    expect(isTabGone([], "tab-1")).toBe(true)
  })
})

// A minimal SessionView for defaultProviderForSession (only `project_id` and
// `provider` matter here).
function session(projectId: string, provider: string): SessionView {
  return {
    workspace: { kind: "managed", project_id: projectId },
    provider,
  } as unknown as SessionView
}

function spine(projects: { id: string; default_provider: string }[]): Spine {
  return {
    projects,
    sessions: [],
    sidebar: { groups: [] },
  } as unknown as Spine
}

describe("defaultProviderForSession", () => {
  it("resolves the owning project's default_provider", () => {
    const sp = spine([{ id: "p1", default_provider: "codex" }])
    expect(defaultProviderForSession(sp, session("p1", "claude"))).toBe("codex")
  })

  it("falls back to the session's own provider when the project is missing", () => {
    const sp = spine([{ id: "other", default_provider: "codex" }])
    expect(defaultProviderForSession(sp, session("p1", "claude"))).toBe("claude")
  })

  it("falls back to the session's own provider when spine is null", () => {
    expect(defaultProviderForSession(null, session("p1", "claude"))).toBe("claude")
  })
})

// SHARED VECTORS with dux-core `agent_tabs.rs` `tab_labels`: these cases are
// mirrored there so the disambiguation rule cannot drift between surfaces.
describe("tabLabels", () => {
  it("leaves distinct providers bare", () => {
    expect(tabLabels([tab("claude"), tab("codex")])).toEqual(["claude", "codex"])
  })

  it("numbers the 2nd+ occurrence of a repeated provider, in order", () => {
    expect(tabLabels([tab("claude"), tab("claude")])).toEqual([
      "claude",
      "claude 2",
    ])
  })

  it("disambiguates three-way and mixed duplicates independently per provider", () => {
    expect(
      tabLabels([
        tab("codex"),
        tab("claude"),
        tab("codex"),
        tab("codex"),
        tab("claude"),
      ]),
    ).toEqual(["codex", "claude", "codex 2", "codex 3", "claude 2"])
  })
})

// A minimal SessionView for resolveFocusedTab (only `id`, `tabs`, and
// `last_focused_tab` matter here).
function sessionWithTabs(
  id: string,
  tabs: AgentTabView[],
  lastFocusedTab: string | null | undefined,
): SessionView {
  return {
    id,
    tabs,
    last_focused_tab: lastFocusedTab,
  } as unknown as SessionView
}

describe("resolveFocusedTab", () => {
  it("returns the session-slot tab when there is no remembered tab", () => {
    const s = sessionWithTabs("s1", [extraTab("t1", true)], null)
    expect(resolveFocusedTab(s)).toBe("s1")
  })

  it("returns the session-slot tab when the remembered value equals the session id", () => {
    const s = sessionWithTabs("s1", [extraTab("t1", true)], "s1")
    expect(resolveFocusedTab(s)).toBe("s1")
  })

  it("returns the session-slot tab when the remembered tab is no longer present", () => {
    const s = sessionWithTabs("s1", [extraTab("t1", true)], "gone")
    expect(resolveFocusedTab(s)).toBe("s1")
  })

  it("returns the remembered tab when it is a live extra tab of this session", () => {
    const s = sessionWithTabs(
      "s1",
      [extraTab("t1", true), extraTab("t2", true)],
      "t2",
    )
    expect(resolveFocusedTab(s)).toBe("t2")
  })

  it("returns the session-slot tab when last_focused_tab is undefined", () => {
    const s = sessionWithTabs("s1", [extraTab("t1", true)], undefined)
    expect(resolveFocusedTab(s)).toBe("s1")
  })
})

describe("shouldRefireFocusPut", () => {
  it("does not refire when the settled response matches the latest intent", () => {
    const latest = { generation: 2, tabId: "t2" }
    const settled = { generation: 2, tabId: "t2" }
    expect(shouldRefireFocusPut(latest, settled)).toBe(false)
  })

  it("does not refire a stale response whose value happens to already match the latest intent", () => {
    // Generation is stale, but the tab id it settled with is coincidentally
    // the same as the current intent, so there is nothing to correct.
    const latest = { generation: 3, tabId: "t2" }
    const settled = { generation: 2, tabId: "t2" }
    expect(shouldRefireFocusPut(latest, settled)).toBe(false)
  })

  it("refires when a stale response settles with a value different from the latest intent", () => {
    // A→B switch fired two PUTs; B's response settled first, A's settled
    // after with a different tab id — re-issue B so the server's last write
    // matches the user's last click regardless of response ordering.
    const latest = { generation: 2, tabId: "t2" }
    const settled = { generation: 1, tabId: "t1" }
    expect(shouldRefireFocusPut(latest, settled)).toBe(true)
  })
})
