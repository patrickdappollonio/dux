import { describe, expect, it } from "vitest"

import {
  ONLY_TAB_CLOSE_REFUSAL,
  closeTabConsequences,
  defaultProviderForSession,
  dormantTabNeedsCard,
  isFirstTab,
  isSlotTabTarget,
  isTabGone,
  resolveFocusedTab,
  shouldRefireFocusPut,
  shouldShowTabStrip,
  slotTabIdOf,
  slotTabTargetId,
  tabLabels,
  tabProseLabel,
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
  // Slot-ness is whatever the server published on `slot_tab_id`, never a
  // comparison against the session id. Today the server publishes the session
  // id, which the first case pins; the second pins that the helper reads the
  // FIELD by making the two disagree.
  it("is true only for the tab the session names as its slot tab", () => {
    const s = sessionWithTabs("s1", [], null)
    expect(isFirstTab(s, "s1")).toBe(true)
    expect(isFirstTab(s, "tab-1")).toBe(false)

    const moved = { ...s, slot_tab_id: "tab-1" }
    expect(isFirstTab(moved, "tab-1")).toBe(true)
    expect(isFirstTab(moved, "s1")).toBe(false)
  })
})

describe("isSlotTabTarget", () => {
  // The id-only twin, for the URL grammar and the PTY socket URL choice, which
  // hold two ids and no session record. It carries the rule rather than the
  // answer, so it is pinned separately from `isFirstTab`.
  it("falls back to the session id as a placeholder when no slot id is known", () => {
    expect(slotTabTargetId("s1")).toBe("s1")
    expect(isSlotTabTarget("s1", "s1")).toBe(true)
    expect(isSlotTabTarget("s1", "tab-1")).toBe(false)
  })

  // The real slot tab carries a GENERATED id, so a caller that knows it must
  // pass it: without it the helper calls the slot tab an extra tab, which arms
  // an extra tab's socket-retry guard over the one socket that must reconnect
  // forever.
  it("prefers the slot id the spine published over the placeholder", () => {
    expect(isSlotTabTarget("s1", "slot-abc", "slot-abc")).toBe(true)
    expect(isSlotTabTarget("s1", "s1", "slot-abc")).toBe(false)
    expect(isSlotTabTarget("s1", "tab-1", "slot-abc")).toBe(false)
  })
})

describe("dormantTabNeedsCard", () => {
  // A session whose slot tab is `slot`, which is all this helper asks of it.
  const withSlot = (slot: string) =>
    ({ id: "s1", slot_tab_id: slot }) as unknown as SessionView
  const slotSession = withSlot("s1")

  // The one-click rule. Selecting an agent is asking for the agent, so its own
  // first tab starts rather than showing a card and asking a second time.
  it("is false for a healthy dormant first tab, which selecting starts", () => {
    const sessionSlot = { ...extraTab("s1", false), id: "s1" }
    expect(
      dormantTabNeedsCard(agentTarget("s1", "s1"), slotSession, sessionSlot, []),
    ).toBe(false)
  })

  // The loop-breaker: a first tab that tried and failed waits for a press,
  // instead of relaunching every time the user looks at it.
  it("is true for a first tab whose last run failed", () => {
    const failed = { ...extraTab("s1", false), id: "s1", last_run_failed: true }
    expect(
      dormantTabNeedsCard(agentTarget("s1", "s1"), slotSession, failed, []),
    ).toBe(true)
    // ...and a press retires it while the launch it asked for is in flight.
    expect(
      dormantTabNeedsCard(agentTarget("s1", "s1"), slotSession, failed, ["s1"]),
    ).toBe(false)
  })

  it("is false for a live first tab, failed last run or not", () => {
    const live = { ...extraTab("s1", true), id: "s1", last_run_failed: true }
    expect(
      dormantTabNeedsCard(agentTarget("s1", "s1"), slotSession, live, []),
    ).toBe(false)
  })

  // Extra tabs are unchanged: the user added them deliberately, so they wait.
  it("is true for any dormant extra tab until it has been started", () => {
    const dormant = extraTab("tab-1", false)
    expect(
      dormantTabNeedsCard(agentTarget("s1", "tab-1"), slotSession, dormant, []),
    ).toBe(true)
    expect(
      dormantTabNeedsCard(agentTarget("s1", "tab-1"), slotSession, dormant, [
        "tab-1",
      ]),
    ).toBe(false)
  })

  it("is false for an extra tab that has a live process", () => {
    expect(
      dormantTabNeedsCard(
        agentTarget("s1", "tab-1"),
        slotSession,
        extraTab("tab-1", true),
        [],
      ),
    ).toBe(false)
  })

  // Pins the DEFENSIVE branch. No caller can reach it (both derive the focused
  // tab from the session's own tab list); the test says which way it falls if
  // one ever does: show the card, launch nothing.
  it("is true for a dormant tab with no session to ask about slot-ness", () => {
    const sessionSlot = { ...extraTab("s1", false), id: "s1" }
    expect(
      dormantTabNeedsCard(agentTarget("s1", "s1"), undefined, sessionSlot, []),
    ).toBe(true)
  })

  // After a promotion the slot is a tab whose id is nothing like the session's,
  // so the one-click rule has to travel with the POINTER. The promoted tab gets
  // the first-tab treatment (start on selection, card only after a failed run)
  // and every sibling, the tab that used to be addressed by the session id
  // included, gets the extra-tab treatment.
  it("applies the first-tab rule to a promoted slot and the extra-tab rule to its siblings", () => {
    const promoted = withSlot("t2")
    const healthy = extraTab("t2", false)
    expect(
      dormantTabNeedsCard(agentTarget("s1", "t2"), promoted, healthy, []),
    ).toBe(false)
    const failed = { ...healthy, last_run_failed: true }
    expect(
      dormantTabNeedsCard(agentTarget("s1", "t2"), promoted, failed, []),
    ).toBe(true)
    expect(
      dormantTabNeedsCard(agentTarget("s1", "t2"), promoted, failed, ["t2"]),
    ).toBe(false)
    // A sibling waits for a press, whatever its id looks like.
    expect(
      dormantTabNeedsCard(
        agentTarget("s1", "t3"),
        promoted,
        extraTab("t3", false),
        [],
      ),
    ).toBe(true)
    expect(
      dormantTabNeedsCard(
        agentTarget("s1", "s1"),
        promoted,
        { ...extraTab("s1", false), id: "s1" },
        [],
      ),
    ).toBe(true)
  })

  // The promotion overlay knows the slot moved before the spine does. Without
  // it, the tab a close just promoted is judged an extra tab and covered with
  // the Start-session card, which the user never asked for and which stays up
  // until the spine lands.
  it("gives the first-tab rule to the tab an overlay says was just promoted", () => {
    const stale = withSlot("t1")
    const promoted = extraTab("t2", false)
    expect(
      dormantTabNeedsCard(agentTarget("s1", "t2"), stale, promoted, []),
    ).toBe(true)
    expect(
      dormantTabNeedsCard(agentTarget("s1", "t2"), stale, promoted, [], "t2"),
    ).toBe(false)
    // The tab the close destroyed no longer gets the rule, even while the stale
    // spine still calls it the slot.
    expect(
      dormantTabNeedsCard(
        agentTarget("s1", "t1"),
        stale,
        extraTab("t1", false),
        [],
        "t2",
      ),
    ).toBe(true)
  })

  it("is false for a terminal target or a missing focused tab", () => {
    const terminal: SelectedTarget = {
      kind: "terminal",
      terminalId: "t1",
      sessionId: "s1",
    }
    expect(
      dormantTabNeedsCard(terminal, slotSession, extraTab("tab-1", false), []),
    ).toBe(false)
    expect(
      dormantTabNeedsCard(agentTarget("s1", "tab-1"), slotSession, undefined, []),
    ).toBe(false)
    expect(
      dormantTabNeedsCard(null, slotSession, extraTab("tab-1", false), []),
    ).toBe(false)
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

// SHARED VECTORS with dux-core `agent_tabs.rs` `prose_tab_label`.
describe("tabProseLabel", () => {
  it("upper-cases the first character and keeps the disambiguating suffix", () => {
    const tabs = [
      { ...tab("codex"), id: "t1" },
      { ...tab("codex"), id: "t2" },
      { ...tab("opencode"), id: "t3" },
    ]
    expect(tabProseLabel(tabs, "t1")).toBe("Codex")
    expect(tabProseLabel(tabs, "t2")).toBe("Codex 2")
    expect(tabProseLabel(tabs, "t3")).toBe("Opencode")
  })

  it("is undefined for a tab the session does not have", () => {
    expect(tabProseLabel([{ ...tab("codex"), id: "t1" }], "nope")).toBeUndefined()
  })
})

// MIRROR of dux-core `agent_tabs.rs` `ONLY_TAB_CLOSE_REFUSAL`, pinned as a
// literal in both languages: the browser refuses this close in the words the
// server would have refused it in, so a reword on one side that is not mirrored
// fails here.
describe("ONLY_TAB_CLOSE_REFUSAL", () => {
  it("is the one core-owned sentence", () => {
    expect(ONLY_TAB_CLOSE_REFUSAL).toBe(
      "This is the agent's only tab, so closing it would leave the agent with no tab at all. Detach the agent instead to stop everything it is running, or add another tab first.",
    )
  })
})

describe("slotTabIdOf", () => {
  const session = { id: "s1", slot_tab_id: "t1" } as unknown as SessionView

  it("prefers a promotion this client performed over the spine", () => {
    expect(slotTabIdOf("s1", session, {})).toBe("t1")
    expect(
      slotTabIdOf("s1", session, {
        s1: { closedTabId: "t1", promotedTabId: "t2" },
      }),
    ).toBe("t2")
  })

  it("is undefined when neither the spine nor an overlay knows the session", () => {
    expect(slotTabIdOf("s9", undefined, {})).toBeUndefined()
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
    slot_tab_id: id,
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

// A session whose slot tab is named explicitly, so a close of the slot tab and a
// close of an extra tab can both be posed to `closeTabConsequences`.
function sessionWithSlot(slotTabId: string, tabs: AgentTabView[]): SessionView {
  return { id: "s1", slot_tab_id: slotTabId, tabs } as unknown as SessionView
}

describe("closeTabConsequences", () => {
  it("names the provider's session and detaches when the closing tab is the last live one", () => {
    const only = extraTab("t1", true)
    const result = closeTabConsequences(sessionWithSlot("t1", [only]), only)
    expect(result.sessionLabel).toBe("the codex session")
    expect(result.willDetach).toBe(true)
  })

  it("does not detach while a live sibling keeps the agent running", () => {
    const closing = extraTab("t2", true)
    const session = sessionWithSlot("t1", [extraTab("t1", true), closing])
    expect(closeTabConsequences(session, closing).willDetach).toBe(false)
  })

  it("detaches on a dormant tab's close only when no sibling is live", () => {
    const dormant = extraTab("t2", false)
    const withLive = sessionWithSlot("t1", [extraTab("t1", true), dormant])
    const allDormant = sessionWithSlot("t1", [extraTab("t1", false), dormant])
    expect(closeTabConsequences(withLive, dormant).willDetach).toBe(false)
    expect(closeTabConsequences(allDormant, dormant).willDetach).toBe(true)
  })

  it("names the successor when the slot tab closes, and none when an extra tab does", () => {
    const slot = extraTab("t1", true)
    const extra = extraTab("t2", true)
    const session = sessionWithSlot("t1", [slot, extra])
    expect(closeTabConsequences(session, slot).successorLabel).toBe("Codex 2")
    expect(closeTabConsequences(session, extra).successorLabel).toBeUndefined()
  })

  it("falls back to the bare session wording when no tab is in hand", () => {
    const result = closeTabConsequences(undefined, undefined)
    expect(result.sessionLabel).toBe("the session")
    expect(result.willDetach).toBe(true)
    expect(result.successorLabel).toBeUndefined()
  })
})
