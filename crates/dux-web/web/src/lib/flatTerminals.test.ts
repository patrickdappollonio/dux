import { describe, expect, it } from "vitest"

import {
  assembleFlatTerminals,
  displayedTerminalOrder,
  sortFlatTerminals,
  terminalStateWord,
  type FlatTerminal,
} from "@/lib/flatTerminals"
import type { ProjectView, SessionView, TerminalView } from "@/lib/types"

function term(over: Partial<TerminalView> & { id: string }): TerminalView {
  return {
    label: "Terminal 1",
    has_output: false,
    working: false,
    typing: false,
    foreground_cmd: null,
    sort_order: 0,
    created_at: "2026-07-17T12:00:00Z",
    updated_at: "2026-07-17T12:00:00Z",
    ...over,
  }
}

// Wrap a bare terminal into a FlatTerminal for the sort tests; only `.terminal`
// matters to `sortFlatTerminals`, so the owner metadata is a stable stub.
function flat(t: TerminalView): FlatTerminal {
  return {
    terminal: t,
    owner: { kind: "session", sessionId: "s1" },
    ownerLabel: "Agent",
    projectName: "Web App",
    siblings: [t],
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

// SHARED VECTORS with dux-core `row_state.rs` `terminal_row_state`: the
// typing > running > idle priority is mirrored there.
describe("terminalStateWord", () => {
  it("prefers typing over working, styled through the typing token", () => {
    const word = terminalStateWord(term({ id: "t", working: true, typing: true }))
    expect(word.label).toBe("Typing")
    expect(word.className).toBe("text-dux-typing")
  })

  it("maps a busy terminal to the active-green 'Running' word", () => {
    const word = terminalStateWord(term({ id: "t", working: true }))
    expect(word.label).toBe("Running")
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

  it("labels a session terminal 'agent@project' (branch when the agent is untitled)", () => {
    const sessions = [
      session({ id: "s1", title: "Login flow", terminals: [term({ id: "a" })] }),
      session({ id: "s2", title: null, terminals: [term({ id: "b" })] }),
    ]
    const flat = assembleFlatTerminals(sessions, [], projectName)
    expect(flat[0].ownerLabel).toBe("Login flow@Web App")
    // Untitled agent falls back to its branch name, still at its project.
    expect(flat[1].ownerLabel).toBe("s2-branch@Web App")
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

describe("sortFlatTerminals", () => {
  // The caller passes the list already in global `sort_order` base order; these
  // fixtures follow that convention (ids ascend with sort_order).
  const base = () => [
    flat(term({ id: "a", sort_order: 0, foreground_cmd: "vim" })),
    flat(term({ id: "b", sort_order: 1, foreground_cmd: "bash" })),
    flat(term({ id: "c", sort_order: 2, foreground_cmd: "htop" })),
  ]
  const ids = (list: FlatTerminal[]) => list.map((f) => f.terminal.id)

  it("manual keeps the base order verbatim (the drag order)", () => {
    expect(ids(sortFlatTerminals(base(), "manual"))).toEqual(["a", "b", "c"])
  })

  it("active floats working-or-typing terminals to the top, keeping base order", () => {
    const list = [
      flat(term({ id: "a", sort_order: 0 })),
      flat(term({ id: "b", sort_order: 1, typing: true })),
      flat(term({ id: "c", sort_order: 2, working: true })),
      flat(term({ id: "d", sort_order: 3 })),
    ]
    // Hot (b typing, c working) first in base order, then the idle rest (a, d).
    expect(ids(sortFlatTerminals(list, "active"))).toEqual(["b", "c", "a", "d"])
  })

  it("created orders newest created_at first", () => {
    const list = [
      flat(term({ id: "a", sort_order: 0, created_at: "2026-07-17T12:00:00Z" })),
      flat(term({ id: "b", sort_order: 1, created_at: "2026-07-17T12:00:20Z" })),
      flat(term({ id: "c", sort_order: 2, created_at: "2026-07-17T12:00:10Z" })),
    ]
    expect(ids(sortFlatTerminals(list, "created"))).toEqual(["b", "c", "a"])
  })

  it("updated orders newest updated_at first", () => {
    const list = [
      flat(term({ id: "a", sort_order: 0, updated_at: "2026-07-17T12:00:00Z" })),
      flat(term({ id: "b", sort_order: 1, updated_at: "2026-07-17T12:00:30Z" })),
      flat(term({ id: "c", sort_order: 2, updated_at: "2026-07-17T12:00:15Z" })),
    ]
    expect(ids(sortFlatTerminals(list, "updated"))).toEqual(["b", "c", "a"])
  })

  it("name sorts by the DISPLAYED label (foreground_cmd, else label), A to Z", () => {
    // Displayed names: a=vim, b=bash, c=htop -> bash, htop, vim.
    expect(ids(sortFlatTerminals(base(), "name"))).toEqual(["b", "c", "a"])
  })

  it("name falls back to the label when foreground_cmd is empty", () => {
    const list = [
      flat(term({ id: "a", sort_order: 0, foreground_cmd: null, label: "zzz" })),
      flat(term({ id: "b", sort_order: 1, foreground_cmd: "", label: "aaa" })),
    ]
    // b's empty foreground_cmd falls back to label "aaa" < "zzz".
    expect(ids(sortFlatTerminals(list, "name"))).toEqual(["b", "a"])
  })

  it("name_desc is the exact reverse of name", () => {
    expect(ids(sortFlatTerminals(base(), "name_desc"))).toEqual(["a", "c", "b"])
  })

  it("does not mutate the caller's array", () => {
    const list = base()
    const before = ids(list)
    sortFlatTerminals(list, "name")
    expect(ids(list)).toEqual(before)
  })
})

// The terminal drag baseline, the twin of `displayedSessionOrder`: the complete
// flat terminal list in the order the shared sort mode displays it. Manual is
// verbatim (`sortFlatTerminals` already treats it so), matching pre-existing
// manual-drag behavior; a computed mode captures what the user sees so the
// persisted order matches the screen.
describe("displayedTerminalOrder", () => {
  const items = [
    flat(term({ id: "t-z", label: "zsh" })),
    flat(term({ id: "t-hot", label: "vim", working: true })),
    flat(term({ id: "t-a", label: "bash" })),
  ]

  it("captures the name-sorted displayed order", () => {
    expect(displayedTerminalOrder(items, "name")).toEqual(["t-a", "t-hot", "t-z"])
  })

  it("captures the active-first float for the active key", () => {
    expect(displayedTerminalOrder(items, "active")).toEqual(["t-hot", "t-z", "t-a"])
  })

  it("returns the base order verbatim for manual", () => {
    expect(displayedTerminalOrder(items, "manual")).toEqual(["t-z", "t-hot", "t-a"])
  })
})
