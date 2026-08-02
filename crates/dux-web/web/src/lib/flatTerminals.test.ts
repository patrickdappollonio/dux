import { describe, expect, it } from "vitest"

import { matchesTerminalQuery } from "@/lib/agentSearch"
import type { FlatSortKey } from "@/lib/flatList"
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
    owner: { kind: "session", session_id: "s1" },
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

  // The projection walks the flat collection ONCE and emits it in the order it
  // arrives; it does not group by owner, and the caller's `sort_order` base is
  // what decides what reaches the screen. The fixture is deliberately
  // interleaved and deliberately NOT in owner order, so a projection that
  // re-grouped by owner (which is what the old nested one did, and what a future
  // edit could reintroduce) fails here instead of passing by coincidence, as it
  // would against an input already sorted the way the assertion expects.
  it("emits the input order verbatim, interleaved owners included, never regrouping by owner", () => {
    const sessions = [
      session({ id: "s1", title: "Login flow", project_id: "p1" }),
      session({ id: "s2", project_id: "p2" }),
    ]
    const projects = [project({ id: "p1" }), project({ id: "p2" })]
    const terminals = [
      term({ id: "t-p1", owner: { kind: "project", project_id: "p1" } }),
      term({ id: "t-s1a", owner: { kind: "session", session_id: "s1" } }),
      term({ id: "t-s2", owner: { kind: "session", session_id: "s2" } }),
      term({ id: "t-p2", owner: { kind: "project", project_id: "p2" } }),
      term({ id: "t-s1b", owner: { kind: "session", session_id: "s1" } }),
    ]
    const flat = assembleFlatTerminals(terminals, sessions, projects, projectName)
    expect(flat.map((f) => f.terminal.id)).toEqual([
      "t-p1",
      "t-s1a",
      "t-s2",
      "t-p2",
      "t-s1b",
    ])
    // Siblings are still resolved by OWNER, not by adjacency: t-s1a and t-s1b
    // are siblings across the three rows sitting between them.
    expect(flat[1].siblings.map((t) => t.id)).toEqual(["t-s1a", "t-s1b"])
  })

  it("labels a session terminal 'agent@project' (branch when the agent is untitled)", () => {
    const sessions = [
      session({ id: "s1", title: "Login flow" }),
      session({ id: "s2", title: null }),
    ]
    const terminals = [
      term({ id: "a", owner: { kind: "session", session_id: "s1" } }),
      term({ id: "b", owner: { kind: "session", session_id: "s2" } }),
    ]
    const flat = assembleFlatTerminals(terminals, sessions, [], projectName)
    expect(flat[0].ownerLabel).toBe("Login flow@Web App")
    // Untitled agent falls back to its branch name, still at its project.
    expect(flat[1].ownerLabel).toBe("s2-branch@Web App")
  })

  it("labels a project terminal with the project name and carries owner refs + siblings", () => {
    const projects = [project({ id: "p1" })]
    const terminals = [
      term({ id: "a", owner: { kind: "project", project_id: "p1" } }),
      term({ id: "b", owner: { kind: "project", project_id: "p1" } }),
    ]
    const flat = assembleFlatTerminals(terminals, [], projects, projectName)
    expect(flat[0].ownerLabel).toBe("Web App")
    expect(flat[0].projectName).toBe("Web App")
    expect(flat[0].owner).toEqual({ kind: "project", projectId: "p1" })
    // Siblings is the owner's whole terminal set (for title disambiguation).
    expect(flat[0].siblings.map((t) => t.id)).toEqual(["a", "b"])
  })

  it("carries the session owner ref for a companion terminal", () => {
    const sessions = [session({ id: "s1" })]
    const terminals = [
      term({ id: "a", owner: { kind: "session", session_id: "s1" } }),
    ]
    const flat = assembleFlatTerminals(terminals, sessions, [], projectName)
    expect(flat[0].owner).toEqual({ kind: "session", sessionId: "s1" })
    expect(flat[0].projectName).toBe("Web App")
  })

  it("returns an empty list when nothing owns a terminal", () => {
    expect(assembleFlatTerminals([], [], [], projectName)).toEqual([])
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

// The whole terminal list pipeline, exactly as `FlatAgentList` runs it, with
// BOTH kinds of owner present and interleaved.
//
// The sort fixtures above are session-owned only, and every one of them would
// pass unchanged if a sort silently grouped by owner. So this block builds the
// list the way the component does (assemble, re-sort into the global `sort_order`
// base, apply the shared sort mode, then filter by the search query) over a
// fixture whose every sort order deliberately CUTS ACROSS the owner grouping:
// no expected order below is in owner order, so a comparator that grouped by
// owner would fail all of them.
describe("the flat terminal list pipeline with both owners", () => {
  const projectName = (id: string) => ({ p1: "Web App", p2: "API" })[id] ?? id
  const sessions = [
    session({ id: "s1", title: "Login flow", project_id: "p1" }),
    session({ id: "s2", title: "Payments", project_id: "p2" }),
  ]
  const projects = [project({ id: "p1" }), project({ id: "p2" })]

  // Deliberately handed to the pipeline in OWNER order, so the base re-sort by
  // `sort_order` is what interleaves them: the input order proves nothing.
  const terminals = [
    // Project p1's own terminal.
    term({
      id: "t-p1",
      owner: { kind: "project", project_id: "p1" },
      sort_order: 0,
      foreground_cmd: "vim",
      created_at: "2026-07-17T12:00:20Z",
      updated_at: "2026-07-17T12:00:05Z",
    }),
    // Project p2's own terminal, being typed into, no foreground app (so it
    // falls back to its label for the name sort). The hot pair is deliberately
    // one terminal of EACH owner, so the active float has to cross the owner
    // grouping in both its halves.
    term({
      id: "t-p2",
      owner: { kind: "project", project_id: "p2" },
      label: "Terminal 3",
      sort_order: 2,
      foreground_cmd: null,
      typing: true,
      created_at: "2026-07-17T12:00:00Z",
      updated_at: "2026-07-17T12:00:20Z",
    }),
    // s1's companion terminal, streaming.
    term({
      id: "t-s1",
      owner: { kind: "session", session_id: "s1" },
      sort_order: 1,
      foreground_cmd: "htop",
      working: true,
      created_at: "2026-07-17T12:00:30Z",
      updated_at: "2026-07-17T12:00:30Z",
    }),
    // s2's companion terminal, being typed into.
    term({
      id: "t-s2",
      owner: { kind: "session", session_id: "s2" },
      sort_order: 3,
      foreground_cmd: "bash",
      created_at: "2026-07-17T12:00:10Z",
      updated_at: "2026-07-17T12:00:10Z",
    }),
  ]

  // The component's pipeline: assemble for the owner labels, re-sort into the
  // global `sort_order` base, apply the sort mode, then filter by the query.
  function pipeline(key: FlatSortKey, query = ""): string[] {
    const assembled = assembleFlatTerminals(
      terminals,
      sessions,
      projects,
      projectName,
    )
    const base = assembled
      .slice()
      .sort((a, b) => a.terminal.sort_order - b.terminal.sort_order)
    return sortFlatTerminals(base, key)
      .filter((ft) =>
        matchesTerminalQuery(ft.terminal, ft.ownerLabel, ft.projectName, query),
      )
      .map((ft) => ft.terminal.id)
  }

  it("manual is the drag order, which interleaves the two owners", () => {
    // sort_order 0..3: project, session, project, session.
    expect(pipeline("manual")).toEqual(["t-p1", "t-s1", "t-p2", "t-s2"])
  })

  it("active floats the working and typing terminals over both owners", () => {
    // Hot: t-s1 (a session terminal, working) and t-p2 (a project terminal,
    // typing), in base order; then the idle rest, also in base order. Both
    // halves interleave the owners.
    expect(pipeline("active")).toEqual(["t-s1", "t-p2", "t-p1", "t-s2"])
  })

  it("created orders newest first, across owners", () => {
    // 12:00:30 s1, 12:00:20 p1, 12:00:10 s2, 12:00:00 p2.
    expect(pipeline("created")).toEqual(["t-s1", "t-p1", "t-s2", "t-p2"])
  })

  it("updated orders newest first, across owners", () => {
    // 12:00:30 s1, 12:00:20 p2, 12:00:10 s2, 12:00:05 p1.
    expect(pipeline("updated")).toEqual(["t-s1", "t-p2", "t-s2", "t-p1"])
  })

  it("name sorts by the displayed label, across owners", () => {
    // Displayed: bash (s2), htop (s1), Terminal 3 (p2, no foreground), vim (p1).
    expect(pipeline("name")).toEqual(["t-s2", "t-s1", "t-p2", "t-p1"])
    expect(pipeline("name_desc")).toEqual(["t-p1", "t-p2", "t-s1", "t-s2"])
  })

  it("search matches an owner label of either kind", () => {
    // "web" is p1's project name, which is the project terminal's whole owner
    // label AND the tail of the `agent@project` label on s1's terminal, so the
    // result crosses the owner grouping.
    expect(pipeline("manual", "web")).toEqual(["t-p1", "t-s1"])
    // An agent's own name matches only its terminal.
    expect(pipeline("manual", "payments")).toEqual(["t-s2"])
    // A foreground command matches whichever terminal is running it.
    expect(pipeline("manual", "htop")).toEqual(["t-s1"])
  })

  it("the drag baseline is the TOTAL displayed order, unfiltered, across owners", () => {
    const assembled = assembleFlatTerminals(
      terminals,
      sessions,
      projects,
      projectName,
    )
    const base = assembled
      .slice()
      .sort((a, b) => a.terminal.sort_order - b.terminal.sort_order)
    // What a drop persists is what the user is looking at, in whatever mode
    // they are in, and it names every terminal of every owner.
    expect(displayedTerminalOrder(base, "name")).toEqual([
      "t-s2",
      "t-s1",
      "t-p2",
      "t-p1",
    ])
    expect(displayedTerminalOrder(base, "active")).toEqual([
      "t-s1",
      "t-p2",
      "t-p1",
      "t-s2",
    ])
    // Manual persists the base order verbatim, which is how a manual drag has
    // always computed its move.
    expect(displayedTerminalOrder(base, "manual")).toEqual([
      "t-p1",
      "t-s1",
      "t-p2",
      "t-s2",
    ])
  })
})
