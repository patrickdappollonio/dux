import { describe, expect, it } from "vitest"

import {
  applyFrozenOrder,
  orderProjectsByRecency,
  projectAgentCounts,
} from "@/lib/projectOrder"
import type { ProjectView, SessionView } from "@/lib/types"

function at(day: number): string {
  return `2026-07-${String(day).padStart(2, "0")}T09:00:00Z`
}

function project(id: string, createdAt: string): ProjectView {
  return {
    id,
    name: id,
    path: `/tmp/${id}`,
    default_provider: "claude",
    explicit_default_provider: null,
    auto_reopen_agents: null,
    startup_command: null,
    env: {},
    current_branch: "main",
    branch_status: "leading",
    path_missing: false,
    leading_branch: "main",
    created_at: createdAt,
  }
}

function agent(id: string, projectId: string, createdAt: string): SessionView {
  return {
    id,
    title: id,
    provider: "claude",
    workspace: {
      kind: "managed",
      project_id: projectId,
      branch_name: id,
      initial_branch: id,
      branch_provenance: "created",
      source_branch: "main",
      worktree_path: `/tmp/${id}`,
    },
    status: "active",
    auto_reopen_enabled: false,
    slot_tab_id: id,
    tabs: [],
    has_output: false,
    working: false,
    typing: false,
    needs_attention: false,
    created_at: createdAt,
    updated_at: createdAt,
  }
}

function ids(projects: ProjectView[], sessions: SessionView[]): string[] {
  return orderProjectsByRecency(projects, sessions).map((p) => p.id)
}

// ── SHARED VECTORS with dux-core's project_order.rs ────────────────────────────

describe("orderProjectsByRecency", () => {
  it("carries the project holding the newest agent to the top", () => {
    const projects = [project("old", at(1)), project("new", at(2))]
    expect(ids(projects, [agent("a1", "old", at(5))])).toEqual(["old", "new"])
  })

  it("ranks a project added after its agents by the added date", () => {
    const projects = [project("added-late", at(9)), project("other", at(2))]
    expect(ids(projects, [agent("a1", "added-late", at(3))])).toEqual([
      "added-late",
      "other",
    ])
  })

  it("ranks an empty project by the date it was added", () => {
    const projects = [project("with-agent", at(1)), project("empty", at(4))]
    expect(ids(projects, [agent("a1", "with-agent", at(2))])).toEqual([
      "empty",
      "with-agent",
    ])
  })

  it("lifts no project for a standalone agent", () => {
    const projects = [project("first", at(2)), project("second", at(1))]
    const standalone: SessionView = {
      ...agent("s1", "ignored", at(9)),
      workspace: {
        kind: "folder",
        folder_path: "/home/someone/work",
        folder_label: "~/work",
        repo_status: "working_repo",
        quiet_reason: "",
      },
    }
    expect(ids(projects, [standalone])).toEqual(["first", "second"])
  })

  it("puts a project with no instant at all last", () => {
    const projects = [
      project("unstored", ""),
      project("dated", at(1)),
      project("also-unstored", ""),
    ]
    expect(ids(projects, [])).toEqual(["dated", "unstored", "also-unstored"])
  })

  it("ranks an unstored project by its agent", () => {
    const projects = [project("dated", at(3)), project("unstored", "")]
    expect(ids(projects, [agent("a1", "unstored", at(6))])).toEqual([
      "unstored",
      "dated",
    ])
  })

  it("keeps the incoming order on ties", () => {
    const projects = [project("one", at(3)), project("two", at(3)), project("three", at(3))]
    expect(ids(projects, [])).toEqual(["one", "two", "three"])
  })
})

describe("applyFrozenOrder", () => {
  it("keeps the frozen order whatever order the live list arrives in", () => {
    const frozen = ["a", "b", "c"]
    const live = [project("c", at(1)), project("a", at(2)), project("b", at(3))]
    expect(applyFrozenOrder(frozen, live).map((p) => p.id)).toEqual([
      "a",
      "b",
      "c",
    ])
  })

  it("skips a project that has gone", () => {
    const frozen = ["a", "b", "c"]
    const live = [project("a", at(1)), project("c", at(1))]
    expect(applyFrozenOrder(frozen, live).map((p) => p.id)).toEqual(["a", "c"])
  })

  it("appends a project the frozen order never saw, at the end", () => {
    const frozen = ["a", "b"]
    const live = [project("new", at(9)), project("a", at(1)), project("b", at(2))]
    expect(applyFrozenOrder(frozen, live).map((p) => p.id)).toEqual([
      "a",
      "b",
      "new",
    ])
  })

  it("appends several newcomers in incoming order", () => {
    const frozen = ["a"]
    const live = [project("x", at(1)), project("a", at(1)), project("y", at(1))]
    expect(applyFrozenOrder(frozen, live).map((p) => p.id)).toEqual([
      "a",
      "x",
      "y",
    ])
  })
})

describe("projectAgentCounts", () => {
  it("counts agents per project, orphaned ones under their gone project, and standalone ones nowhere", () => {
    const standalone = {
      ...agent("s1", "unused", at(1)),
      workspace: {
        kind: "folder",
        folder_path: "/home/me/notes",
        folder_label: "~/notes",
        repo_status: "no_repo",
        quiet_reason: "",
      },
    } as SessionView
    const counts = projectAgentCounts([
      agent("a1", "p1", at(1)),
      agent("a2", "p1", at(2)),
      agent("g1", "gone", at(3)),
      standalone,
    ])
    expect([...counts.entries()]).toEqual([
      ["p1", 2],
      ["gone", 1],
    ])
  })
})
