import { describe, expect, it } from "vitest"

import { isSupportTabDormant, shouldShowTabStrip, tabLabels } from "./agentTabs"
import type { SelectedTarget } from "./store"
import type { AgentTabView } from "./types"

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

function supportTab(id: string, live: boolean): AgentTabView {
  return { ...tab("codex"), id, has_live_process: live }
}

describe("shouldShowTabStrip", () => {
  it("is false for zero or one tab, true for two or more", () => {
    expect(shouldShowTabStrip([])).toBe(false)
    expect(shouldShowTabStrip([tab("claude")])).toBe(false)
    expect(shouldShowTabStrip([tab("claude"), tab("codex")])).toBe(true)
    expect(shouldShowTabStrip([tab("a"), tab("b"), tab("c")])).toBe(true)
  })
})

describe("isSupportTabDormant", () => {
  it("is false for the Main tab even with no live process", () => {
    const main = { ...supportTab("s1", false), id: "s1" }
    expect(isSupportTabDormant(agentTarget("s1", "s1"), main, [])).toBe(false)
  })

  it("is true for a Support tab with no live process until it has been started", () => {
    const dormant = supportTab("tab-1", false)
    expect(isSupportTabDormant(agentTarget("s1", "tab-1"), dormant, [])).toBe(true)
    // Once explicitly started, the card is suppressed for that tab id.
    expect(
      isSupportTabDormant(agentTarget("s1", "tab-1"), dormant, ["tab-1"]),
    ).toBe(false)
  })

  it("is false for a Support tab that has a live process", () => {
    expect(
      isSupportTabDormant(agentTarget("s1", "tab-1"), supportTab("tab-1", true), []),
    ).toBe(false)
  })

  it("is false for a terminal target or a missing focused tab", () => {
    const terminal: SelectedTarget = {
      kind: "terminal",
      terminalId: "t1",
      sessionId: "s1",
    }
    expect(isSupportTabDormant(terminal, supportTab("tab-1", false), [])).toBe(false)
    expect(isSupportTabDormant(agentTarget("s1", "tab-1"), undefined, [])).toBe(false)
    expect(isSupportTabDormant(null, supportTab("tab-1", false), [])).toBe(false)
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
