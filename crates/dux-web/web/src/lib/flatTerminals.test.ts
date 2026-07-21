import { describe, expect, it } from "vitest"

import { assembleFlatTerminals, terminalStateWord } from "@/lib/flatTerminals"
import type { ProjectView, SessionView, TerminalView } from "@/lib/types"

function term(over: Partial<TerminalView> & { id: string }): TerminalView {
  return {
    label: "Terminal 1",
    has_output: false,
    working: false,
    typing: false,
    foreground_cmd: null,
    ...over,
  }
}

function session(
  over: Partial<SessionView> & { id: string },
): SessionView {
  return {
    project_id: "p1",
    title: null,
    provider: "claude",
    branch_name: `${over.id}-branch`,
    initial_branch: `${over.id}-branch`,
    source_branch: "main",
    worktree_path: `/tmp/${over.id}`,
    status: "active",
    auto_reopen_enabled: false,
    terminals: [],
    tabs: [],
    has_output: false,
    working: false,
    typing: false,
    needs_attention: false,
    created_at: "2026-07-17T12:00:00Z",
    updated_at: "2026-07-17T12:00:00Z",
    ...over,
  } as SessionView
}

function project(over: Partial<ProjectView> & { id: string }): ProjectView {
  return {
    name: over.id,
    path: `/repos/${over.id}`,
    default_provider: "claude",
    explicit_default_provider: null,
    auto_reopen_agents: null,
    startup_command: null,
    env: {},
    current_branch: "main",
    branch_status: "",
    path_missing: false,
    leading_branch: null,
    created_at: "",
    terminals: [],
    ...over,
  } as ProjectView
}

describe("terminalStateWord", () => {
  it("prefers typing over working, styled through the typing token", () => {
    const word = terminalStateWord(term({ id: "t", working: true, typing: true }))
    expect(word.label).toBe("Typing")
    expect(word.className).toBe("text-dux-typing")
  })

  it("maps working to the active-green word", () => {
    const word = terminalStateWord(term({ id: "t", working: true }))
    expect(word.label).toBe("Working")
    expect(word.className).toBe("text-green-500")
  })

  it("maps an idle terminal to the muted Idle word", () => {
    const word = terminalStateWord(term({ id: "t" }))
    expect(word.label).toBe("Idle")
    expect(word.className).toBe("text-muted-foreground")
  })
})

describe("assembleFlatTerminals", () => {
  const projectName = (id: string) =>
    ({ p1: "Web App", p2: "API" })[id] ?? id

  it("lists session terminals first (in session order), then project terminals", () => {
    const sessions = [
      session({
        id: "s1",
        title: "Login flow",
        project_id: "p1",
        terminals: [term({ id: "t-s1a" }), term({ id: "t-s1b" })],
      }),
      session({ id: "s2", project_id: "p2", terminals: [term({ id: "t-s2" })] }),
    ]
    const projects = [
      project({ id: "p1", terminals: [term({ id: "t-p1" })] }),
      project({ id: "p2", terminals: [] }),
    ]
    const flat = assembleFlatTerminals(sessions, projects, projectName)
    expect(flat.map((f) => f.terminal.id)).toEqual([
      "t-s1a",
      "t-s1b",
      "t-s2",
      "t-p1",
    ])
  })

  it("labels a session terminal with the agent title (or branch when untitled)", () => {
    const sessions = [
      session({ id: "s1", title: "Login flow", terminals: [term({ id: "a" })] }),
      session({ id: "s2", title: null, terminals: [term({ id: "b" })] }),
    ]
    const flat = assembleFlatTerminals(sessions, [], projectName)
    expect(flat[0].ownerLabel).toBe("Login flow")
    // Untitled agent falls back to its branch name.
    expect(flat[1].ownerLabel).toBe("s2-branch")
  })

  it("labels a project terminal with the project name and carries owner refs + siblings", () => {
    const projects = [
      project({ id: "p1", terminals: [term({ id: "a" }), term({ id: "b" })] }),
    ]
    const flat = assembleFlatTerminals([], projects, projectName)
    expect(flat[0].ownerLabel).toBe("Web App")
    expect(flat[0].projectName).toBe("Web App")
    expect(flat[0].owner).toEqual({ kind: "project", projectId: "p1" })
    // Siblings is the owner's whole terminal set (for title disambiguation).
    expect(flat[0].siblings.map((t) => t.id)).toEqual(["a", "b"])
  })

  it("carries the session owner ref for a companion terminal", () => {
    const sessions = [session({ id: "s1", terminals: [term({ id: "a" })] })]
    const flat = assembleFlatTerminals(sessions, [], projectName)
    expect(flat[0].owner).toEqual({ kind: "session", sessionId: "s1" })
    expect(flat[0].projectName).toBe("Web App")
  })

  it("returns an empty list when nothing owns a terminal", () => {
    expect(assembleFlatTerminals([], [], projectName)).toEqual([])
  })
})
