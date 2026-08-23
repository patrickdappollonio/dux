import { describe, expect, it } from "vitest"

import { formatDisplayDate, projectLiveCounts } from "./projectInfo"
import type { SessionView, TerminalView } from "./types"

// Minimal fixture: projectLiveCounts only reads project_id off a session.
function session(
  fields: Omit<Partial<SessionView>, "workspace"> & {
    id: string
    project_id: string
  },
): SessionView {
  return {
    title: null,
    ...fields,
    workspace: { kind: "managed", project_id: fields.project_id },
  } as unknown as SessionView
}

// Minimal terminal fixture: projectLiveCounts only reads `owner`.
function sessionTerm(id: string, sessionId: string): TerminalView {
  return {
    id,
    owner: { kind: "session", session_id: sessionId },
  } as unknown as TerminalView
}

function projectTerm(id: string, projectId: string): TerminalView {
  return {
    id,
    owner: { kind: "project", project_id: projectId },
  } as unknown as TerminalView
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
      session({ id: "a", project_id: "p1" }),
      session({ id: "b", project_id: "p1" }),
      session({ id: "c", project_id: "p2" }),
    ]
    const terminals = [
      sessionTerm("t1", "a"),
      sessionTerm("t2", "a"),
      sessionTerm("t3", "b"),
      sessionTerm("t4", "c"),
    ]
    expect(projectLiveCounts("p1", sessions, terminals)).toEqual({
      agents: 2,
      terminals: 3,
    })
  })

  it("includes the project's own project terminals", () => {
    // The count must include the project's own terminals, not only its
    // sessions' terminals.
    const sessions = [session({ id: "a", project_id: "p1" })]
    const terminals = [
      sessionTerm("t1", "a"),
      projectTerm("pt1", "p1"),
      projectTerm("pt2", "p1"),
    ]
    expect(projectLiveCounts("p1", sessions, terminals)).toEqual({
      agents: 1,
      terminals: 3,
    })
  })

  it("returns zeros for a project with no sessions", () => {
    expect(projectLiveCounts("ghost", [])).toEqual({ agents: 0, terminals: 0 })
  })
})

describe("formatDisplayDate", () => {
  it("returns Unknown for an empty string (no store row yet)", () => {
    expect(formatDisplayDate("")).toBe("Unknown")
    expect(formatDisplayDate("   ")).toBe("Unknown")
  })

  it("returns Unknown for an unparseable value", () => {
    expect(formatDisplayDate("not-a-date")).toBe("Unknown")
  })

  it("formats a valid RFC 3339 timestamp as a human-readable date", () => {
    // Midday UTC avoids date-boundary shifts across the runner's timezone.
    const formatted = formatDisplayDate("2026-02-03T12:00:00+00:00")
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
