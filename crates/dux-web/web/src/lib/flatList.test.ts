import { describe, expect, it } from "vitest"

import {
  activeFirstSessions,
  displayedSessionOrder,
  FLAT_SORT_LABELS,
  nextActiveSessionId,
  partitionQuiet,
  quietTailForcedOpen,
  sortMainSessions,
  sortQuietTail,
  stateWord,
} from "@/lib/flatList"
import type { SessionView } from "@/lib/types"

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

describe("partitionQuiet", () => {
  it("keeps active sessions in main and sinks detached/exited into quiet", () => {
    const sessions = [
      makeSession({ id: "a", status: "active" }),
      makeSession({ id: "b", status: "detached" }),
      makeSession({ id: "c", status: "exited" }),
      makeSession({ id: "d", status: "active" }),
    ]
    const { main, quiet } = partitionQuiet(sessions)
    expect(main.map((s) => s.id)).toEqual(["a", "d"])
    expect(quiet.map((s) => s.id)).toEqual(["b", "c"])
  })
})

describe("activeFirstSessions", () => {
  it("floats working and needs-attention agents above the rest, stably", () => {
    const sessions = [
      makeSession({ id: "idle1" }),
      makeSession({ id: "working", working: true }),
      makeSession({ id: "idle2" }),
      makeSession({ id: "attn", needs_attention: true }),
    ]
    expect(activeFirstSessions(sessions).map((s) => s.id)).toEqual([
      "working",
      "attn",
      "idle1",
      "idle2",
    ])
  })

  it("preserves core order when nothing is hot", () => {
    const sessions = [makeSession({ id: "a" }), makeSession({ id: "b" })]
    expect(activeFirstSessions(sessions).map((s) => s.id)).toEqual(["a", "b"])
  })
})

describe("sortMainSessions", () => {
  const sessions = [
    makeSession({ id: "b", title: "Beta", created_at: "2026-07-17T10:00:00Z", updated_at: "2026-07-17T10:00:00Z" }),
    makeSession({ id: "a", title: "Alpha", working: true, created_at: "2026-07-17T09:00:00Z", updated_at: "2026-07-17T11:00:00Z" }),
  ]

  it("floats hot agents for the active key", () => {
    expect(sortMainSessions(sessions, "active").map((s) => s.id)).toEqual(["a", "b"])
  })

  it("returns the input order verbatim for manual", () => {
    expect(sortMainSessions(sessions, "manual").map((s) => s.id)).toEqual(["b", "a"])
  })

  it("sorts by name case-insensitively", () => {
    expect(sortMainSessions(sessions, "name").map((s) => s.id)).toEqual(["a", "b"])
  })

  it("sorts by most recently updated", () => {
    expect(sortMainSessions(sessions, "updated").map((s) => s.id)).toEqual(["a", "b"])
  })

  it("sorts by name descending (Z to A) for a TUI-set name_desc", () => {
    // The web does not offer name_desc in its picker but must DISPLAY it.
    expect(sortMainSessions(sessions, "name_desc").map((s) => s.id)).toEqual(["b", "a"])
  })

  it("does not mutate the input array", () => {
    const input = sessions.slice()
    sortMainSessions(input, "name")
    expect(input.map((s) => s.id)).toEqual(["b", "a"])
  })
})

// The drag baseline for a drop made in a NON-manual sort mode: the complete
// session list as the user sees it, totalized (sorted main list first, then the
// quiet tail in base order), so the persisted manual order matches the screen.
describe("displayedSessionOrder", () => {
  const sessions = [
    makeSession({ id: "zeta", title: "Zeta" }),
    makeSession({ id: "gone", title: "Gone", status: "exited" }),
    makeSession({ id: "alpha", title: "Alpha" }),
    makeSession({ id: "hot", title: "Hot", working: true }),
    makeSession({ id: "parked", title: "Parked", status: "detached" }),
  ]

  it("captures the name-sorted main list with the quiet tail appended, totally", () => {
    // The tail rides at the END (it renders below the main list) in its own
    // base relative order, and every session is present: the persisted order
    // must be total, not just the visible subset.
    expect(displayedSessionOrder(sessions, "name")).toEqual([
      "alpha",
      "hot",
      "zeta",
      "gone",
      "parked",
    ])
  })

  it("captures the active-first float for the active key", () => {
    expect(displayedSessionOrder(sessions, "active")).toEqual([
      "hot",
      "zeta",
      "alpha",
      "gone",
      "parked",
    ])
  })

  it("returns the base order VERBATIM for manual (quiet stays interleaved)", () => {
    // Manual drags behaved this way before drag-from-any-mode existed: the
    // move applies over the raw base order, quiet sessions in place. Keeping
    // that exact behavior is a requirement, so manual does not re-home the
    // tail to the back.
    expect(displayedSessionOrder(sessions, "manual")).toEqual([
      "zeta",
      "gone",
      "alpha",
      "hot",
      "parked",
    ])
  })
})

// The quiet (inactive) tail is ordered MOST-RECENTLY-ACTIVE-FIRST
// (Reverse(updated_at)) in the "active" sort mode, matching the TUI's
// build_left_items; every other mode leaves the tail verbatim. SHARED VECTORS
// with dux-core `flat_list.rs`.
describe("sortQuietTail", () => {
  const quiet = [
    makeSession({
      id: "old",
      status: "detached",
      updated_at: "2026-07-17T10:00:00Z",
    }),
    makeSession({
      id: "newest",
      status: "exited",
      updated_at: "2026-07-17T14:00:00Z",
    }),
    makeSession({
      id: "mid",
      status: "detached",
      updated_at: "2026-07-17T12:00:00Z",
    }),
  ]

  it("orders the tail most-recently-active-first in active mode", () => {
    expect(sortQuietTail(quiet, "active").map((s) => s.id)).toEqual([
      "newest",
      "mid",
      "old",
    ])
  })

  it("leaves the tail verbatim in every non-active mode", () => {
    for (const key of ["manual", "name", "updated", "created"] as const) {
      expect(sortQuietTail(quiet, key).map((s) => s.id)).toEqual([
        "old",
        "newest",
        "mid",
      ])
    }
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

  it("takes the first row when the gone agent was not in the active bucket", () => {
    const quiet = makeSession({ id: "z", status: "exited" })
    const before = [...previous, quiet]
    expect(nextActiveSessionId(before, previous, "z", "manual")).toBe("a")
  })
})
