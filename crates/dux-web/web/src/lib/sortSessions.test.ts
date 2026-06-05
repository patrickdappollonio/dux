import { describe, expect, it } from "vitest"

import { sortedSessionIds } from "./sortSessions"
import type { SessionView } from "./types"

// Minimal fixture: sortedSessionIds only reads id/title/branch_name/created_at/
// updated_at, so cast a partial like reorder.test.ts does.
function session(
  fields: Partial<SessionView> & { id: string },
): SessionView {
  return {
    title: null,
    branch_name: fields.id,
    created_at: "2026-01-01T00:00:00+00:00",
    updated_at: "2026-01-01T00:00:00+00:00",
    ...fields,
  } as unknown as SessionView
}

describe("sortedSessionIds — updated", () => {
  it("orders newest updated first (Reverse(updated_at))", () => {
    const sessions = [
      session({ id: "old", updated_at: "2026-01-01T00:00:00+00:00" }),
      session({ id: "new", updated_at: "2026-03-01T00:00:00+00:00" }),
      session({ id: "mid", updated_at: "2026-02-01T00:00:00+00:00" }),
    ]
    expect(sortedSessionIds(sessions, "updated")).toEqual(["new", "mid", "old"])
  })

  it("keeps original order for equal updated_at (stable sort)", () => {
    const ts = "2026-05-05T05:05:05+00:00"
    const sessions = [
      session({ id: "a", updated_at: ts }),
      session({ id: "b", updated_at: ts }),
      session({ id: "c", updated_at: ts }),
    ]
    expect(sortedSessionIds(sessions, "updated")).toEqual(["a", "b", "c"])
  })

  it("does not use created_at when sorting by updated", () => {
    const sessions = [
      session({
        id: "x",
        created_at: "2026-09-01T00:00:00+00:00",
        updated_at: "2026-01-01T00:00:00+00:00",
      }),
      session({
        id: "y",
        created_at: "2026-01-01T00:00:00+00:00",
        updated_at: "2026-09-01T00:00:00+00:00",
      }),
    ]
    expect(sortedSessionIds(sessions, "updated")).toEqual(["y", "x"])
  })
})

describe("sortedSessionIds — created", () => {
  it("orders newest created first (Reverse(created_at))", () => {
    const sessions = [
      session({ id: "first", created_at: "2026-01-01T00:00:00+00:00" }),
      session({ id: "third", created_at: "2026-03-01T00:00:00+00:00" }),
      session({ id: "second", created_at: "2026-02-01T00:00:00+00:00" }),
    ]
    expect(sortedSessionIds(sessions, "created")).toEqual([
      "third",
      "second",
      "first",
    ])
  })

  it("keeps original order for equal created_at (stable sort)", () => {
    const ts = "2026-04-04T04:04:04+00:00"
    const sessions = [
      session({ id: "a", created_at: ts }),
      session({ id: "b", created_at: ts }),
    ]
    expect(sortedSessionIds(sessions, "created")).toEqual(["a", "b"])
  })
})

describe("sortedSessionIds — name", () => {
  it("orders case-insensitively ascending on the title", () => {
    const sessions = [
      session({ id: "1", title: "Zephyr" }),
      session({ id: "2", title: "apple" }),
      session({ id: "3", title: "Mango" }),
    ]
    expect(sortedSessionIds(sessions, "name")).toEqual(["2", "3", "1"])
  })

  it("falls back to branch_name when title is null", () => {
    const sessions = [
      session({ id: "1", title: null, branch_name: "zebra" }),
      session({ id: "2", title: null, branch_name: "alpha" }),
    ]
    expect(sortedSessionIds(sessions, "name")).toEqual(["2", "1"])
  })

  it("mixes title and branch_name fallback by the effective name", () => {
    const sessions = [
      session({ id: "1", title: "beta", branch_name: "ignored" }),
      session({ id: "2", title: null, branch_name: "alpha" }),
      session({ id: "3", title: "gamma", branch_name: "ignored" }),
    ]
    expect(sortedSessionIds(sessions, "name")).toEqual(["2", "1", "3"])
  })

  it("compares case-insensitively (lowercased keys)", () => {
    const sessions = [
      session({ id: "upper", title: "BANANA" }),
      session({ id: "lower", title: "apple" }),
    ]
    expect(sortedSessionIds(sessions, "name")).toEqual(["lower", "upper"])
  })

  it("keeps original order for equal names (stable sort)", () => {
    const sessions = [
      session({ id: "a", title: "dup" }),
      session({ id: "b", title: "Dup" }),
      session({ id: "c", title: "dup" }),
    ]
    expect(sortedSessionIds(sessions, "name")).toEqual(["a", "b", "c"])
  })
})

describe("sortedSessionIds — purity", () => {
  it("does not mutate the input array", () => {
    const sessions = [
      session({ id: "b", updated_at: "2026-02-01T00:00:00+00:00" }),
      session({ id: "a", updated_at: "2026-01-01T00:00:00+00:00" }),
    ]
    const ids = sessions.map((s) => s.id)
    sortedSessionIds(sessions, "updated")
    expect(sessions.map((s) => s.id)).toEqual(ids)
  })

  it("returns an empty array for no sessions", () => {
    expect(sortedSessionIds([], "name")).toEqual([])
  })
})
