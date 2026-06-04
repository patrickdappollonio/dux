import { describe, expect, it } from "vitest"

import {
  applyPendingOrders,
  moveItem,
  ordersMatch,
  reorderById,
  reorderProjectsInGroup,
  spliceGroupOrder,
} from "./reorder"
import type { ProjectView, SessionView } from "./types"

describe("moveItem", () => {
  it("moves an item forward to the over slot", () => {
    expect(moveItem(["a", "b", "c", "d"], "a", "c")).toEqual([
      "b",
      "c",
      "a",
      "d",
    ])
  })

  it("moves an item backward to the over slot", () => {
    expect(moveItem(["a", "b", "c", "d"], "d", "b")).toEqual([
      "a",
      "d",
      "b",
      "c",
    ])
  })

  it("is a no-op when active equals over", () => {
    const ids = ["a", "b", "c"]
    expect(moveItem(ids, "b", "b")).toEqual(["a", "b", "c"])
  })

  it("returns the original order when an id is missing", () => {
    expect(moveItem(["a", "b"], "x", "a")).toEqual(["a", "b"])
    expect(moveItem(["a", "b"], "a", "x")).toEqual(["a", "b"])
  })

  it("does not mutate the input array", () => {
    const ids = ["a", "b", "c"]
    moveItem(ids, "a", "c")
    expect(ids).toEqual(["a", "b", "c"])
  })
})

describe("spliceGroupOrder", () => {
  it("rewrites only the reordered group's slots in the full list", () => {
    // Full list: with-agents [p1, p2] then no-agents [p3, p4].
    // Reorder the with-agents group to [p2, p1]; p3/p4 keep their slots.
    const full = ["p1", "p2", "p3", "p4"]
    const group = ["p1", "p2"]
    const newGroup = ["p2", "p1"]
    expect(spliceGroupOrder(full, group, newGroup)).toEqual([
      "p2",
      "p1",
      "p3",
      "p4",
    ])
  })

  it("rewrites the no-agents group while keeping with-agents in place", () => {
    const full = ["p1", "p2", "p3", "p4"]
    const group = ["p3", "p4"]
    const newGroup = ["p4", "p3"]
    expect(spliceGroupOrder(full, group, newGroup)).toEqual([
      "p1",
      "p2",
      "p4",
      "p3",
    ])
  })

  it("preserves non-contiguous group slots", () => {
    // Group members interleaved with non-members in the full list.
    const full = ["a", "x", "b", "y", "c"]
    const group = ["a", "b", "c"]
    const newGroup = ["c", "a", "b"]
    expect(spliceGroupOrder(full, group, newGroup)).toEqual([
      "c",
      "x",
      "a",
      "y",
      "b",
    ])
  })

  it("ignores stale ids not present in the full list", () => {
    const full = ["a", "b"]
    const group = ["a", "b"]
    const newGroup = ["b", "ghost", "a"]
    expect(spliceGroupOrder(full, group, newGroup)).toEqual(["b", "a"])
  })
})

describe("reorderProjectsInGroup", () => {
  it("drags within the with-agents group and splices into the full list", () => {
    const full = ["p1", "p2", "p3", "p4"]
    const group = ["p1", "p2"]
    expect(reorderProjectsInGroup(full, group, "p1", "p2")).toEqual([
      "p2",
      "p1",
      "p3",
      "p4",
    ])
  })

  it("drags within the no-agents group, leaving with-agents untouched", () => {
    const full = ["p1", "p2", "p3", "p4"]
    const group = ["p3", "p4"]
    expect(reorderProjectsInGroup(full, group, "p4", "p3")).toEqual([
      "p1",
      "p2",
      "p4",
      "p3",
    ])
  })

  it("returns the full order unchanged on a no-op drag", () => {
    const full = ["p1", "p2", "p3"]
    const group = ["p1", "p2"]
    expect(reorderProjectsInGroup(full, group, "p1", "p1")).toEqual(full)
  })
})

describe("ordersMatch", () => {
  it("is true for identical orders", () => {
    expect(ordersMatch(["a", "b"], ["a", "b"])).toBe(true)
  })

  it("is false for different positions", () => {
    expect(ordersMatch(["a", "b"], ["b", "a"])).toBe(false)
  })

  it("is false for different lengths", () => {
    expect(ordersMatch(["a"], ["a", "b"])).toBe(false)
  })

  it("is true for two empty arrays", () => {
    expect(ordersMatch([], [])).toBe(true)
  })
})

describe("reorderById", () => {
  const items = (ids: string[]) => ids.map((id) => ({ id }))

  it("reorders items to match the named order", () => {
    expect(reorderById(items(["a", "b", "c"]), ["c", "a", "b"])).toEqual(
      items(["c", "a", "b"]),
    )
  })

  it("keeps unnamed items in their original slots", () => {
    // Only a and c are named; b is unnamed and keeps its middle slot.
    expect(reorderById(items(["a", "b", "c"]), ["c", "a"])).toEqual(
      items(["c", "b", "a"]),
    )
  })

  it("ignores named ids not present in the list", () => {
    expect(reorderById(items(["a", "b"]), ["b", "ghost", "a"])).toEqual(
      items(["b", "a"]),
    )
  })
})

// Minimal fixtures: only the fields the overlay logic reads.
const project = (id: string): ProjectView =>
  ({ id, name: id }) as unknown as ProjectView
const session = (id: string, projectId: string): SessionView =>
  ({ id, project_id: projectId }) as unknown as SessionView

describe("applyPendingOrders", () => {
  it("returns inputs untouched when both overlays are null", () => {
    const projects = [project("p1"), project("p2")]
    const sessions = [session("s1", "p1")]
    const out = applyPendingOrders(projects, sessions, null, null)
    expect(out.projects).toBe(projects)
    expect(out.sessions).toBe(sessions)
  })

  it("applies the project overlay only", () => {
    const projects = [project("p1"), project("p2"), project("p3")]
    const sessions = [session("s1", "p1")]
    const out = applyPendingOrders(projects, sessions, null, ["p3", "p1", "p2"])
    expect(out.projects.map((p) => p.id)).toEqual(["p3", "p1", "p2"])
    expect(out.sessions).toBe(sessions)
  })

  it("applies a session overlay scoped to one project's slots", () => {
    // Flat sessions array interleaves projects; reordering p1's sessions must
    // not disturb p2's session's slot.
    const projects = [project("p1"), project("p2")]
    const sessions = [
      session("a", "p1"),
      session("z", "p2"),
      session("b", "p1"),
    ]
    const out = applyPendingOrders(
      projects,
      sessions,
      { projectId: "p1", ids: ["b", "a"] },
      null,
    )
    // p1's two slots (indices 0 and 2) refill as [b, a]; p2's z stays at index 1.
    expect(out.sessions.map((s) => s.id)).toEqual(["b", "z", "a"])
  })

  it("applies both overlays independently", () => {
    const projects = [project("p1"), project("p2")]
    const sessions = [session("a", "p1"), session("b", "p1")]
    const out = applyPendingOrders(
      projects,
      sessions,
      { projectId: "p1", ids: ["b", "a"] },
      ["p2", "p1"],
    )
    expect(out.projects.map((p) => p.id)).toEqual(["p2", "p1"])
    expect(out.sessions.map((s) => s.id)).toEqual(["b", "a"])
  })
})

describe("applyPendingOrders with a drifted ViewModel", () => {
  it("survives an overlay referencing a deleted session while a new one exists", () => {
    // The Issue-2 self-heal path: the user reordered [a, b], then the server
    // deleted "b" and a new session "c" appeared before the overlay cleared.
    // The overlay's ghost id must be ignored, the unnamed newcomer must keep
    // its slot, and nothing crashes or drops rows.
    const projects = [project("p1")]
    const sessions = [session("a", "p1"), session("c", "p1")]
    const out = applyPendingOrders(
      projects,
      sessions,
      { projectId: "p1", ids: ["b", "a"] },
      null,
    )
    const ids = out.sessions.map((s) => s.id).sort()
    expect(ids).toEqual(["a", "c"])
    expect(out.sessions).toHaveLength(2)
  })
})
