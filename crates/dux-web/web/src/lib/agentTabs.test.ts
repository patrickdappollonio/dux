import { describe, expect, it } from "vitest"

import {
  defaultProviderForSession,
  isExtraTabDormant,
  isTabGone,
  shouldShowTabStrip,
  tabLabels,
} from "./agentTabs"
import type { Spine } from "./spineApi"
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

describe("isExtraTabDormant", () => {
  it("is false for the session-slot tab even with no live process", () => {
    const sessionSlot = { ...extraTab("s1", false), id: "s1" }
    expect(isExtraTabDormant(agentTarget("s1", "s1"), sessionSlot, [])).toBe(false)
  })

  it("is true for an extra tab with no live process until it has been started", () => {
    const dormant = extraTab("tab-1", false)
    expect(isExtraTabDormant(agentTarget("s1", "tab-1"), dormant, [])).toBe(true)
    // Once explicitly started, the card is suppressed for that tab id.
    expect(
      isExtraTabDormant(agentTarget("s1", "tab-1"), dormant, ["tab-1"]),
    ).toBe(false)
  })

  it("is false for an extra tab that has a live process", () => {
    expect(
      isExtraTabDormant(agentTarget("s1", "tab-1"), extraTab("tab-1", true), []),
    ).toBe(false)
  })

  it("is false for a terminal target or a missing focused tab", () => {
    const terminal: SelectedTarget = {
      kind: "terminal",
      terminalId: "t1",
      sessionId: "s1",
    }
    expect(isExtraTabDormant(terminal, extraTab("tab-1", false), [])).toBe(false)
    expect(isExtraTabDormant(agentTarget("s1", "tab-1"), undefined, [])).toBe(false)
    expect(isExtraTabDormant(null, extraTab("tab-1", false), [])).toBe(false)
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
  return { project_id: projectId, provider } as unknown as SessionView
}

function spine(projects: { id: string; default_provider: string }[]): Spine {
  return { projects, sessions: [], sidebar: { groups: [] } } as unknown as Spine
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
