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
    workspace: {
      kind: "managed",
      project_id: "p1",
      branch_name: "feature/foo",
      initial_branch: "feature/foo",
      branch_provenance: "created",
      source_branch: "main",
      worktree_path: "/home/user/worktrees/feature-foo",
    },
    title: null,
    provider: "claude",
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
      session({
        workspace: {
          kind: "managed",
          project_id: "p1",
          branch_name: "feature/foo",
          initial_branch: "main",
          branch_provenance: "created",
          source_branch: "main",
          worktree_path: "/home/user/worktrees/feature-foo",
        },
      }),
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

  it("has no worktree row; the branch row already names the worktree", () => {
    const model = buildAgentVitals(session(), "myproject", null)
    expect(model.rows.find((r) => r.key === "worktree")).toBeUndefined()
  })

  // A managed agent needs no directory row, because its worktree is named
  // after its branch and the branch row already identifies it. That reasoning
  // does not hold for a standalone agent: its folder is the user's own, named
  // nothing in particular, and it is the single most useful fact about the
  // agent. So it gets the row the managed shape deliberately does without.
  it("names a standalone agent's folder, and shows no branch rows at all", () => {
    const model = buildAgentVitals(
      session({
        title: "notes",
        workspace: {
          kind: "folder",
          folder_path: "/home/someone/work/notes",
          folder_label: "~/work/notes",
          repo_status: "no_repo",
          quiet_reason: "This folder has no git repository.",
        },
      }),
      "",
      null,
    )
    expect(model.rows.find((r) => r.key === "folder")?.value).toBe(
      "~/work/notes",
    )
    expect(model.rows.find((r) => r.key === "branch")).toBeUndefined()
    expect(model.rows.find((r) => r.key === "source")).toBeUndefined()
    expect(model.name).toBe("notes")
  })

  it("shows the source branch, skipping it when it matches the current branch", () => {
    const forked = buildAgentVitals(session(), "myproject", null)
    expect(forked.rows.find((r) => r.key === "source")?.value).toBe("main")

    const onSource = buildAgentVitals(
      session({
        workspace: {
          kind: "managed",
          project_id: "p1",
          branch_name: "feature/foo",
          initial_branch: "feature/foo",
          branch_provenance: "created",
          source_branch: "feature/foo",
          worktree_path: "/home/user/worktrees/feature-foo",
        },
      }),
      "myproject",
      null,
    )
    expect(onSource.rows.find((r) => r.key === "source")).toBeUndefined()

    const noSource = buildAgentVitals(
      session({
        workspace: {
          kind: "managed",
          project_id: "p1",
          branch_name: "feature/foo",
          initial_branch: "feature/foo",
          branch_provenance: "created",
          source_branch: "",
          worktree_path: "/home/user/worktrees/feature-foo",
        },
      }),
      "myproject",
      null,
    )
    expect(noSource.rows.find((r) => r.key === "source")).toBeUndefined()
  })

  it("summarizes a single-tab agent's provider as just the provider name", () => {
    const model = buildAgentVitals(session(), "myproject", null)
    expect(model.provider).toBe("claude")
  })

  it("aggregates multi-tab providers in first-appearance order with counts", () => {
    const model = buildAgentVitals(
      session({
        tabs: [
          tab({ id: "s1", provider: "claude" }),
          tab({ id: "t2", provider: "codex" }),
          tab({ id: "t3", provider: "claude" }),
          tab({ id: "t4", provider: "copilot" }),
          tab({ id: "t5", provider: "copilot" }),
        ],
      }),
      "myproject",
      null,
    )
    expect(model.provider).toBe("claude (2), codex, copilot (2)")
  })

  it("falls back to the session provider when the tabs list is empty", () => {
    const model = buildAgentVitals(session({ tabs: [] }), "myproject", null)
    expect(model.provider).toBe("claude")
  })
})
