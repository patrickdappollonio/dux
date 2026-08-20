import { describe, expect, it } from "vitest"
import {
  attentionCount,
  attentionCountForSurface,
  formatTabTitle,
} from "./attention"
import type { SessionView } from "./types"

function session(id: string, needs_attention: boolean): SessionView {
  return {
    id,
    workspace: {
      kind: "managed",
      project_id: "p1",
      branch_name: "b",
      initial_branch: "b",
      branch_provenance: "created",
      source_branch: "main",
      worktree_path: "/tmp/w",
    },
    title: null,
    provider: "claude",
    status: "active",
    auto_reopen_enabled: false,
    terminals: [],
    tabs: [],
    has_output: true,
    working: false,
    needs_attention,
    created_at: "",
    updated_at: "",
  } as SessionView
}

describe("attentionCount", () => {
  it("counts only flagged sessions", () => {
    expect(attentionCount([])).toBe(0)
    expect(
      attentionCount([session("a", false), session("b", false)]),
    ).toBe(0)
    expect(
      attentionCount([
        session("a", true),
        session("b", false),
        session("c", true),
      ]),
    ).toBe(2)
  })
})

describe("attentionCountForSurface", () => {
  const flagged = [session("a", true), session("b", true), session("c", false)]

  it("counts flagged sessions on the workspace surface", () => {
    expect(attentionCountForSurface(flagged, false)).toBe(2)
    expect(attentionCountForSurface([], false)).toBe(0)
  })

  it("is always zero in the standalone editor tab", () => {
    expect(attentionCountForSurface(flagged, true)).toBe(0)
    expect(attentionCountForSurface([], true)).toBe(0)
  })
})

describe("formatTabTitle", () => {
  it("leaves the title bare at zero", () => {
    expect(formatTabTitle("dux — laptop", 0)).toBe("dux — laptop")
  })

  it("prefixes the count when above zero", () => {
    expect(formatTabTitle("dux — laptop", 2)).toBe("(2) dux — laptop")
    expect(formatTabTitle("dux", 1)).toBe("(1) dux")
  })
})
