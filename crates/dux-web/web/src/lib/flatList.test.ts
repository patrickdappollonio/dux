import { readFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

import {
  FLAT_SORT_LABELS,
  flatRowKey,
  listDragBaseline,
  nextActiveSessionId,
  orderFlatRows,
  planListDrop,
  quietTailForcedOpen,
  stateWord,
  type FlatRow,
  type FlatSortKey,
  inInactiveTail,
  isQuietSession,
} from "@/lib/flatList"
import type { SessionView, TerminalView } from "@/lib/types"

function makeSession(over: Partial<SessionView> & { id: string }): SessionView {
  return {
    workspace: {
      kind: "managed",
      project_id: "p1",
      branch_name: over.id,
      initial_branch: over.id,
      branch_provenance: "created",
      source_branch: "main",
      worktree_path: `/tmp/${over.id}`,
    },
    title: over.id,
    provider: "claude",
    status: "active",
    auto_reopen_enabled: false,
    terminals: [],
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

function makeTerminal(over: Partial<TerminalView> & { id: string }): TerminalView {
  return {
    owner: { kind: "project", project_id: "p1" },
    label: over.id,
    has_output: false,
    working: false,
    typing: false,
    foreground_cmd: null,
    sort_order: 0,
    created_at: "2026-07-17T12:00:00Z",
    updated_at: "2026-07-17T12:00:00Z",
    list_after: null,
    ...over,
  }
}

// An agent's own terminal, which is never a row of the list.
function ownTerminal(id: string, sessionId: string): TerminalView {
  return makeTerminal({ id, owner: { kind: "session", session_id: sessionId } })
}

const ids = (rows: FlatRow[]) =>
  rows.map((row) => (row.kind === "agent" ? row.session.id : row.terminal.id))

// The other half of `the_shared_flat_list_fixture_holds` in dux-core's
// `flat_list.rs`: the terminal UI and the browser order the list alike.
const fixture = JSON.parse(
  readFileSync(
    join(
      dirname(fileURLToPath(import.meta.url)),
      "../../../../dux-core/tests/fixtures/flat_list_cross_language.json",
    ),
    "utf8",
  ),
) as {
  cases: {
    what: string
    mode: string
    agents: {
      id: string
      name: string
      status: SessionView["status"]
      created: string
      updated: string
      hot: boolean
      running_terminal: boolean
    }[]
    terminals: {
      id: string
      name: string
      created: string
      updated: string
      busy: boolean
      after: string | null
    }[]
    main: string[]
    inactive: string[]
  }[]
}

// The config spells the two name modes the way the core does.
const FIXTURE_MODES: Record<string, FlatSortKey> = {
  active: "active",
  updated: "updated",
  created: "created",
  name: "name",
  name_desc: "name_desc",
  manual: "manual",
}

describe("the shared flat list fixture", () => {
  for (const c of fixture.cases) {
    it(c.what, () => {
      const sessions = c.agents.map((a) =>
        makeSession({
          id: a.id,
          title: a.name,
          status: a.status,
          created_at: a.created,
          updated_at: a.updated,
          working: a.hot,
        }),
      )
      const terminals = [
        ...c.terminals.map((t) =>
          makeTerminal({
            id: t.id,
            label: t.name,
            created_at: t.created,
            updated_at: t.updated,
            working: t.busy,
            list_after: t.after,
          }),
        ),
        ...c.agents
          .filter((a) => a.running_terminal)
          .map((a) => ownTerminal(`own-${a.id}`, a.id)),
      ]
      const order = orderFlatRows(sessions, terminals, FIXTURE_MODES[c.mode])
      expect(ids(order.main)).toEqual(c.main)
      expect(order.inactive.map((s) => s.id)).toEqual(c.inactive)
    })
  }
})

describe("orderFlatRows", () => {
  it("never lists an agent's own terminal", () => {
    const order = orderFlatRows(
      [makeSession({ id: "a" })],
      [ownTerminal("own", "a"), makeTerminal({ id: "t", list_after: "a" })],
      "manual",
    )
    expect(ids(order.main)).toEqual(["a", "t"])
  })

  it("floats a typed-into terminal with the hot rows", () => {
    const order = orderFlatRows(
      [makeSession({ id: "a" })],
      [makeTerminal({ id: "t", typing: true, list_after: "a" })],
      "active",
    )
    expect(ids(order.main)).toEqual(["t", "a"])
  })

  it("sorts a terminal by the name it was opened with, never its foreground app", () => {
    const order = orderFlatRows(
      [makeSession({ id: "m", title: "mike" })],
      [makeTerminal({ id: "t", label: "zulu", foreground_cmd: "aaa", list_after: null })],
      "name",
    )
    expect(ids(order.main)).toEqual(["m", "t"])
  })

  // SHARED VECTORS with dux-core `flat_list.rs`, agents alone.
  it("floats hot agents and sorts the tail most recently active first", () => {
    const sessions = [
      makeSession({ id: "zeta", updated_at: "2026-07-17T12:00:00Z" }),
      makeSession({ id: "gone", status: "exited", updated_at: "2026-07-17T10:00:00Z" }),
      makeSession({ id: "alpha", updated_at: "2026-07-17T12:00:00Z" }),
      makeSession({ id: "hot", needs_attention: true }),
      makeSession({ id: "parked", status: "detached", updated_at: "2026-07-17T14:00:00Z" }),
    ]
    const order = orderFlatRows(sessions, [], "active")
    expect(ids(order.main)).toEqual(["hot", "zeta", "alpha"])
    expect(order.inactive.map((s) => s.id)).toEqual(["parked", "gone"])
  })

  it("leaves the tail verbatim in every non-active mode", () => {
    const sessions = [
      makeSession({ id: "old", status: "detached", updated_at: "2026-07-17T10:00:00Z" }),
      makeSession({ id: "newest", status: "exited", updated_at: "2026-07-17T14:00:00Z" }),
      makeSession({ id: "mid", status: "detached", updated_at: "2026-07-17T12:00:00Z" }),
    ]
    for (const key of ["manual", "name", "updated", "created"] as const) {
      expect(orderFlatRows(sessions, [], key).inactive.map((s) => s.id)).toEqual([
        "old",
        "newest",
        "mid",
      ])
    }
  })
})

// The baseline a drop is made against, and what the drop then asks for.
describe("listDragBaseline and planListDrop", () => {
  const sessions = [
    makeSession({ id: "zeta", title: "Zeta" }),
    makeSession({ id: "gone", title: "Gone", status: "exited" }),
    makeSession({ id: "alpha", title: "Alpha" }),
  ]
  const terminals = [
    ownTerminal("own", "zeta"),
    makeTerminal({ id: "t", label: "Mid", list_after: "zeta" }),
  ]
  const key = (id: string) =>
    flatRowKey(
      listDragBaseline(sessions, terminals, "manual").find(
        (row) => (row.kind === "agent" ? row.session.id : row.terminal.id) === id,
      )!,
    )

  it("totalizes the displayed order under a computed sort, the tail last", () => {
    expect(ids(listDragBaseline(sessions, terminals, "name"))).toEqual([
      "alpha",
      "t",
      "zeta",
      "gone",
    ])
  })

  it("keeps the stored order under manual, tail agents in place", () => {
    expect(ids(listDragBaseline(sessions, terminals, "manual"))).toEqual([
      "zeta",
      "t",
      "gone",
      "alpha",
    ])
  })

  it("anchors a dropped terminal after the agent above it, and first at the top", () => {
    const baseline = listDragBaseline(sessions, terminals, "manual")
    expect(planListDrop(baseline, key("t"), key("alpha"), sessions, terminals)).toEqual({
      agentOrder: null,
      placements: { t: "alpha" },
      terminalOrder: null,
    })
    expect(planListDrop(baseline, key("t"), key("zeta"), sessions, terminals)).toEqual({
      agentOrder: null,
      placements: { t: null },
      terminalOrder: null,
    })
  })

  it("reorders the agents alone when an agent is dropped, and nothing when it lands where it was", () => {
    const baseline = listDragBaseline(sessions, terminals, "manual")
    expect(planListDrop(baseline, key("alpha"), key("zeta"), sessions, terminals)).toEqual({
      agentOrder: ["alpha", "zeta", "gone"],
      placements: {},
      terminalOrder: null,
    })
    expect(planListDrop(baseline, key("t"), key("t"), sessions, terminals)).toBeNull()
  })

  it("orders two terminals that share an anchor among themselves, leaving an agent's own alone", () => {
    const two = [
      ...terminals,
      makeTerminal({ id: "u", label: "Other", list_after: "zeta" }),
    ]
    const baseline = listDragBaseline(sessions, two, "manual")
    expect(ids(baseline)).toEqual(["zeta", "t", "u", "gone", "alpha"])
    const uKey = flatRowKey(baseline[2])
    const tKey = flatRowKey(baseline[1])
    expect(planListDrop(baseline, uKey, tKey, sessions, two)).toEqual({
      agentOrder: null,
      placements: {},
      terminalOrder: ["u", "t", "own"],
    })
  })
})

describe("nextActiveSessionId", () => {
  const previous = [
    makeSession({ id: "a" }),
    makeSession({ id: "b" }),
    makeSession({ id: "c" }),
  ]

  it("picks the row after the gone agent in the displayed order", () => {
    const current = [previous[0], previous[2]]
    expect(nextActiveSessionId(previous, current, "b", "manual")).toBe("c")
  })

  it("wraps to the first row when the last agent goes", () => {
    // A create and a delete landing in the SAME spine is what makes the wrap
    // load-bearing: "d" is new and now heads the list, so the "every neighbour
    // is gone too" fallback would answer "d". Only walking off the end of the
    // old order and back round to the top reaches "a", the row that follows the
    // last one. With `current` merely a subset of `previous`, both routes agree
    // and the wrap can be deleted without a single test noticing.
    const created = makeSession({ id: "d" })
    const current = [created, previous[0], previous[1]]
    expect(nextActiveSessionId(previous, current, "c", "manual")).toBe("a")
  })

  it("skips a detached neighbour and keeps walking", () => {
    const detached = makeSession({ id: "c", status: "detached" })
    const current = [previous[0], detached]
    expect(nextActiveSessionId(previous, current, "b", "manual")).toBe("a")
  })

  it("never lands on a stopped agent kept in the list by its own terminal", () => {
    const detached = makeSession({ id: "c", status: "detached" })
    const current = [previous[0], detached]
    expect(
      nextActiveSessionId(previous, current, "b", "manual", [ownTerminal("own", "c")]),
    ).toBe("a")
  })

  it("returns null when nothing active is left", () => {
    const current = [makeSession({ id: "c", status: "detached" })]
    expect(nextActiveSessionId(previous, current, "b", "manual")).toBeNull()
  })

  it("follows the active-first order rather than the raw list order", () => {
    // "active" floats working agents to the top, so the displayed order is
    // c, a, b, d and losing c lands on a, the row that was directly under it.
    const ordered = [
      makeSession({ id: "a" }),
      makeSession({ id: "b" }),
      makeSession({ id: "c", working: true }),
      makeSession({ id: "d" }),
    ]
    const current = [ordered[0], ordered[1], ordered[3]]
    expect(nextActiveSessionId(ordered, current, "c", "active")).toBe("a")
  })

  it("takes the first row when the gone agent was not in the main bucket", () => {
    const quiet = makeSession({ id: "z", status: "exited" })
    const before = [...previous, quiet]
    expect(nextActiveSessionId(before, previous, "z", "manual")).toBe("a")
  })
})

// SHARED VECTORS with dux-core `quiet_tail.rs` `quiet_tail_forced_open`: the
// forced-open decision, keyed on the NORMALIZED query, mirrored there.
describe("quietTailForcedOpen", () => {
  it("forces open only for an undismissed, hitting, non-empty query", () => {
    expect(quietTailForcedOpen("", null, true)).toBe(false)
    expect(quietTailForcedOpen("vim", null, true)).toBe(true)
    expect(quietTailForcedOpen("vim", null, false)).toBe(false)
    expect(quietTailForcedOpen("vim", "vim", true)).toBe(false)
    expect(quietTailForcedOpen("nvim", "vim", true)).toBe(true)
  })

  it("keeps the tail dismissed for the same normalized query", () => {
    // The caller normalizes, so "vim ", " VIM" all arrive as "vim".
    expect(quietTailForcedOpen("vim", "vim", true)).toBe(false)
  })
})

describe("FLAT_SORT_LABELS", () => {
  it("labels name ascending and descending symmetrically", () => {
    expect(FLAT_SORT_LABELS.name).toBe("Name (A to Z)")
    expect(FLAT_SORT_LABELS.name_desc).toBe("Name (Z to A)")
  })
})

// SHARED VECTORS with dux-core `row_state.rs` `agent_row_state`: the priority
// ladder (label ordering) is mirrored there. A change to the priority in one
// language that is not mirrored fails a test on the other side.
describe("stateWord", () => {
  it("prefers needs-you over typing and working", () => {
    expect(
      stateWord(
        makeSession({ id: "a", working: true, typing: true, needs_attention: true }),
      ).label,
    ).toBe("Needs you")
  })

  it("prefers typing over working for an active agent", () => {
    const word = stateWord(makeSession({ id: "a", working: true, typing: true }))
    expect(word.label).toBe("Typing")
    // Styled through the soft-violet typing token, never a hardcoded hue.
    expect(word.className).toBe("text-dux-typing")
  })

  it("maps each flag combination to its word in TUI priority order", () => {
    expect(stateWord(makeSession({ id: "a", typing: true })).label).toBe("Typing")
    expect(stateWord(makeSession({ id: "a", working: true })).label).toBe("Working")
    expect(stateWord(makeSession({ id: "a" })).label).toBe("Idle")
    expect(stateWord(makeSession({ id: "a", status: "detached" })).label).toBe("Detached")
    expect(stateWord(makeSession({ id: "a", status: "exited" })).label).toBe("Exited")
  })

  it("ignores typing/working for a non-active agent (detached/exited unaffected)", () => {
    expect(
      stateWord(makeSession({ id: "a", status: "detached", typing: true })).label,
    ).toBe("Detached")
    expect(
      stateWord(makeSession({ id: "a", status: "exited", typing: true, working: true }))
        .label,
    ).toBe("Exited")
  })
})

// Two questions about a stopped agent with a terminal of its own still running:
// the list keeps it out of the Inactive tail, while the agent alone is still
// inactive, which is what the pull-request poller's slow clock goes by.
describe("a stopped agent whose terminal still runs", () => {
  const stopped = { status: "detached" } as SessionView
  it("stays out of the tail and is still inactive by itself", () => {
    expect(inInactiveTail(stopped, true)).toBe(false)
    expect(inInactiveTail(stopped, false)).toBe(true)
    expect(isQuietSession(stopped)).toBe(true)
  })
})
