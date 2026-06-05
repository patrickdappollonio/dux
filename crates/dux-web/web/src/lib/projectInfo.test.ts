import { describe, expect, it } from "vitest"

import { formatAddedDate, projectLiveCounts } from "./projectInfo"
import type { SessionView } from "./types"

// Minimal fixture: projectLiveCounts only reads project_id and terminals.
function session(
  fields: Partial<SessionView> & { id: string; project_id: string },
): SessionView {
  return {
    title: null,
    terminals: [],
    ...fields,
  } as unknown as SessionView
}

describe("projectLiveCounts", () => {
  it("counts only sessions for the target project", () => {
    const sessions = [
      session({ id: "a", project_id: "p1" }),
      session({ id: "b", project_id: "p1" }),
      session({ id: "c", project_id: "p2" }),
    ]
    expect(projectLiveCounts("p1", sessions)).toEqual({
      agents: 2,
      terminals: 0,
    })
  })

  it("sums companion terminals across the project's sessions", () => {
    const sessions = [
      session({
        id: "a",
        project_id: "p1",
        terminals: [{ id: "t1" }, { id: "t2" }],
      } as unknown as SessionView),
      session({
        id: "b",
        project_id: "p1",
        terminals: [{ id: "t3" }],
      } as unknown as SessionView),
      session({
        id: "c",
        project_id: "p2",
        terminals: [{ id: "t4" }],
      } as unknown as SessionView),
    ]
    expect(projectLiveCounts("p1", sessions)).toEqual({
      agents: 2,
      terminals: 3,
    })
  })

  it("returns zeros for a project with no sessions", () => {
    expect(projectLiveCounts("ghost", [])).toEqual({ agents: 0, terminals: 0 })
  })
})

describe("formatAddedDate", () => {
  it("returns Unknown for an empty string (no store row yet)", () => {
    expect(formatAddedDate("")).toBe("Unknown")
    expect(formatAddedDate("   ")).toBe("Unknown")
  })

  it("returns Unknown for an unparseable value", () => {
    expect(formatAddedDate("not-a-date")).toBe("Unknown")
  })

  it("formats a valid RFC 3339 timestamp as a human-readable date", () => {
    // Midday UTC avoids date-boundary shifts across the runner's timezone.
    const formatted = formatAddedDate("2026-02-03T12:00:00+00:00")
    expect(formatted).not.toBe("Unknown")
    expect(formatted).toContain("2026")
    // Matches what toLocaleDateString produces for the same instant/options,
    // independent of the runner's locale.
    const expected = new Date("2026-02-03T12:00:00+00:00").toLocaleDateString(
      undefined,
      { year: "numeric", month: "short", day: "numeric" },
    )
    expect(formatted).toBe(expected)
  })
})
