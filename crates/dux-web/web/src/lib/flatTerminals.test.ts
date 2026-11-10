import { describe, expect, it } from "vitest"

import { matchesTerminalQuery } from "@/lib/agentSearch"
import {
  assembleFlatTerminals,
  terminalStateWord,
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

function session(
  over: Omit<Partial<SessionView>, "workspace"> & {
    id: string
    project_id?: string
  },
): SessionView {
  return {
    workspace: {
      kind: "managed",
      project_id: over.project_id ?? "p1",
      branch_name: `${over.id}-branch`,
      initial_branch: `${over.id}-branch`,
      branch_provenance: "created",
      source_branch: "main",
      worktree_path: `/tmp/${over.id}`,
    },
    title: null,
    provider: "claude",
    status: "active",
    auto_reopen_enabled: false,
    tabs: [],
    has_output: false,
    working: false,
    typing: false,
    needs_attention: false,
    slot_tab_id: over.id,
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
    expect(word.className).toBe("text-dux-working")
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

// The search over the assembled terminals, with BOTH kinds of owner present.
describe("the flat terminal list pipeline with both owners", () => {
  const projectName = (id: string) => ({ p1: "Web App", p2: "API" })[id] ?? id
  const sessions = [
    session({ id: "s1", title: "Login flow", project_id: "p1" }),
    session({ id: "s2", title: "Payments", project_id: "p2" }),
  ]
  const projects = [project({ id: "p1" }), project({ id: "p2" })]

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

  // Assemble for the owner labels, then filter by the query.
  function pipeline(query = ""): string[] {
    const assembled = assembleFlatTerminals(
      terminals,
      sessions,
      projects,
      projectName,
    )
    return assembled
      .filter((ft) =>
        matchesTerminalQuery(ft.terminal, ft.ownerLabel, ft.projectName, query),
      )
      .map((ft) => ft.terminal.id)
  }

  it("search matches an owner label of either kind", () => {
    // "web" is p1's project name, which is the project terminal's whole owner
    // label AND the tail of the `agent@project` label on s1's terminal, so the
    // result crosses the owner grouping.
    expect(pipeline("web")).toEqual(["t-p1", "t-s1"])
    // An agent's own name matches only its terminal.
    expect(pipeline("payments")).toEqual(["t-s2"])
    // A foreground command matches whichever terminal is running it.
    expect(pipeline("htop")).toEqual(["t-s1"])
  })

})

// A terminal that belongs to nothing is an ordinary sidebar element. It has no
// owner to name, so its row's second line names the DIRECTORY it opened in, and
// that is also what the sidebar search matches. These are the journeys that
// would have silently failed before the owner became a tagged value: a kind with
// no loop of its own simply never appeared.
describe("the flat terminal list with a standalone terminal in it", () => {
  const projectName = (id: string) => ({ p1: "Web App" })[id] ?? id
  const sessions = [session({ id: "s1", title: "Login flow", project_id: "p1" })]
  const projects = [project({ id: "p1" })]

  const terminals = [
    term({
      id: "t-s1",
      owner: { kind: "session", session_id: "s1" },
      sort_order: 0,
      foreground_cmd: "htop",
      created_at: "2026-08-01T12:00:00Z",
      updated_at: "2026-08-01T12:00:00Z",
    }),
    term({
      id: "t-solo",
      owner: { kind: "standalone", cwd_label: "~/code" },
      label: "Terminal 2",
      sort_order: 1,
      foreground_cmd: null,
      created_at: "2026-08-01T12:00:10Z",
      updated_at: "2026-08-01T12:00:10Z",
    }),
    term({
      id: "t-p1",
      owner: { kind: "project", project_id: "p1" },
      sort_order: 2,
      foreground_cmd: "vim",
      created_at: "2026-08-01T12:00:20Z",
      updated_at: "2026-08-01T12:00:20Z",
    }),
  ]

  function pipeline(query = ""): string[] {
    const assembled = assembleFlatTerminals(
      terminals,
      sessions,
      projects,
      projectName,
    )
    return assembled
      .filter((ft) =>
        matchesTerminalQuery(ft.terminal, ft.ownerLabel, ft.projectName, query),
      )
      .map((ft) => ft.terminal.id)
  }

  it("labels a standalone terminal with its directory and no project tag", () => {
    const flat = assembleFlatTerminals(
      terminals,
      sessions,
      projects,
      projectName,
    )
    const solo = flat.find((f) => f.terminal.id === "t-solo")
    expect(solo?.ownerLabel).toBe("~/code")
    // Truthfully empty: it belongs to no project, so there is no project tag.
    expect(solo?.projectName).toBe("")
    expect(solo?.owner).toEqual({ kind: "standalone" })
  })

  it("groups two standalone terminals as each other's siblings", () => {
    const two = [
      term({ id: "a", owner: { kind: "standalone", cwd_label: "~" } }),
      term({ id: "b", owner: { kind: "standalone", cwd_label: "~/code" } }),
      term({ id: "c", owner: { kind: "session", session_id: "s1" } }),
    ]
    const flat = assembleFlatTerminals(two, sessions, projects, projectName)
    expect(flat[0].siblings.map((t) => t.id)).toEqual(["a", "b"])
  })

  it("is findable by searching for its directory", () => {
    expect(pipeline("code")).toEqual(["t-solo"])
    expect(pipeline("~")).toEqual(["t-solo"])
    // And is not swept up by a query matching somebody else's owner.
    expect(pipeline("login")).toEqual(["t-s1"])
  })

})
