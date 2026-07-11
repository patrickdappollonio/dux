import { describe, expect, it } from "vitest"

import { buildAgentVitals } from "@/lib/agentVitals"
import type { AgentTabView, SessionView } from "@/lib/types"

function tab(overrides: Partial<AgentTabView> = {}): AgentTabView {
  return {
    id: "tab-1",
    provider: "claude",
    order: 0,
    working: false,
    needs_attention: false,
    has_output: false,
    has_live_process: false,
    ...overrides,
  }
}

function session(overrides: Partial<SessionView> = {}): SessionView {
  return {
    id: "s1",
    project_id: "p1",
    title: null,
    provider: "claude",
    branch_name: "feature/foo",
    initial_branch: "feature/foo",
    source_branch: "main",
    worktree_path: "/home/user/worktrees/feature-foo",
    status: "active",
    auto_reopen_enabled: true,
    terminals: [],
    tabs: [tab({ id: "s1" })],
    has_output: false,
    working: false,
    needs_attention: false,
    created_at: "",
    updated_at: "",
    ...overrides,
  }
}

describe("buildAgentVitals", () => {
  it("renders the plain branch when there is no drift", () => {
    const model = buildAgentVitals(session(), "myproject", null)
    const branchRow = model.rows.find((r) => r.key === "branch")
    expect(branchRow?.value).toBe("feature/foo")
  })

  it("renders 'initial -> current' drift form when branches differ", () => {
    const model = buildAgentVitals(
      session({ initial_branch: "main", branch_name: "feature/foo" }),
      "myproject",
      null,
    )
    const branchRow = model.rows.find((r) => r.key === "branch")
    expect(branchRow?.value).toBe("main → feature/foo")
  })

  it("omits the tabs row for a single-tab agent", () => {
    const model = buildAgentVitals(session(), "myproject", null)
    expect(model.rows.find((r) => r.key === "tabs")).toBeUndefined()
  })

  it("shows live/total tab liveness for multi-tab agents", () => {
    const model = buildAgentVitals(
      session({
        tabs: [
          tab({ id: "s1", has_live_process: true }),
          tab({ id: "t2", has_live_process: true }),
          tab({ id: "t3", has_live_process: false }),
        ],
      }),
      "myproject",
      null,
    )
    const tabsRow = model.rows.find((r) => r.key === "tabs")
    expect(tabsRow?.value).toBe("2 of 3 live")
  })

  it("includes the PR row only when a PR is present", () => {
    const withoutPr = buildAgentVitals(session(), "myproject", null)
    expect(withoutPr.rows.find((r) => r.key === "pr")).toBeUndefined()

    const withPr = buildAgentVitals(
      session({ pr: { number: 42, state: "open", title: "x", url: "https://x" } }),
      "myproject",
      null,
    )
    expect(withPr.rows.find((r) => r.key === "pr")?.value).toBe("#42 open")
  })

  it("omits the changes row when count is null or zero", () => {
    expect(
      buildAgentVitals(session(), "myproject", null).rows.find(
        (r) => r.key === "changes",
      ),
    ).toBeUndefined()
    expect(
      buildAgentVitals(session(), "myproject", 0).rows.find(
        (r) => r.key === "changes",
      ),
    ).toBeUndefined()
  })

  it("shows a singular/plural changes count when available", () => {
    expect(
      buildAgentVitals(session(), "myproject", 1).rows.find(
        (r) => r.key === "changes",
      )?.value,
    ).toBe("1 file")
    expect(
      buildAgentVitals(session(), "myproject", 3).rows.find(
        (r) => r.key === "changes",
      )?.value,
    ).toBe("3 files")
  })

  it("derives the status label per StatusBadge semantics", () => {
    expect(
      buildAgentVitals(session({ status: "active", working: true }), "p", null)
        .statusLabel,
    ).toBe("Working")
    expect(
      buildAgentVitals(session({ status: "active", working: false }), "p", null)
        .statusLabel,
    ).toBe("Active")
    expect(
      buildAgentVitals(session({ status: "detached" }), "p", null).statusLabel,
    ).toBe("Detached")
    expect(
      buildAgentVitals(session({ status: "exited" }), "p", null).statusLabel,
    ).toBe("Exited")
  })

  it("needs_attention overrides the status label regardless of working state", () => {
    expect(
      buildAgentVitals(
        session({ status: "active", working: true, needs_attention: true }),
        "p",
        null,
      ).statusLabel,
    ).toBe("Needs attention")
  })

  it("always includes worktree path when present", () => {
    const model = buildAgentVitals(session(), "myproject", null)
    expect(model.rows.find((r) => r.key === "worktree")?.value).toBe(
      "/home/user/worktrees/feature-foo",
    )
  })
})
