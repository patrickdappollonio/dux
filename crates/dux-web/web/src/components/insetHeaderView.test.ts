import { describe, expect, it } from "vitest"

import { insetHeaderChips } from "./insetHeaderView"
import type { DuxState, SelectedTarget } from "@/lib/store"
import type { SessionView } from "@/lib/types"

type Spine = DuxState["spine"]

function agent(id: string, branch: string): SessionView {
  return {
    id,
    workspace: {
      kind: "managed",
      project_id: "p1",
      branch_name: branch,
      initial_branch: branch,
      branch_provenance: "created",
      source_branch: "main",
      worktree_path: `/tmp/${id}`,
    },
    title: null,
    provider: "claude",
    status: "active",
    tabs: [
      { id, provider: "claude", order: 0 },
      { id: `${id}-t2`, provider: "codex", order: 1 },
    ],
  } as unknown as SessionView
}

function spineWith(terminals: unknown[] = []): Spine {
  return {
    projects: [{ id: "p1", name: "Repo" }],
    sessions: [agent("s1", "feature-x")],
    terminals,
  } as unknown as Spine
}

function kinds(chips: { kind: string }[]): string[] {
  return chips.map((c) => c.kind)
}

describe("insetHeaderChips", () => {
  it("names the project and the agent for a selected agent", () => {
    const target: SelectedTarget = {
      kind: "agent",
      sessionId: "s1",
      tabId: "s1",
    }
    const chips = insetHeaderChips(spineWith(), agent("s1", "feature-x"), target)
    expect(kinds(chips)).toEqual(["project", "agent", "assistant"])
    expect(chips.find((c) => c.kind === "agent")?.value).toBe("feature-x")
  })

  it("reads the assistant off the FOCUSED tab, not the session's own provider", () => {
    const session = agent("s1", "feature-x")
    const chips = insetHeaderChips(spineWith(), session, {
      kind: "agent",
      sessionId: "s1",
      tabId: "s1-t2",
    })
    expect(chips.find((c) => c.kind === "assistant")?.value).toBe("codex")
  })

  it("has nothing to say with no session and no terminal", () => {
    expect(insetHeaderChips(spineWith(), undefined, null)).toEqual([])
  })

  it("hands the primary slot to a session-owned terminal, keeping the agent's fields", () => {
    const terminals = [
      {
        id: "t1",
        owner: { kind: "session", session_id: "s1" },
        title: "shell",
      },
    ]
    const target: SelectedTarget = {
      kind: "terminal",
      terminalId: "t1",
      owner: { kind: "session", sessionId: "s1" },
    }
    const chips = insetHeaderChips(
      spineWith(terminals),
      agent("s1", "feature-x"),
      target,
    )
    expect(kinds(chips)).toContain("terminal")
    expect(chips.find((c) => c.kind === "terminal")?.primary).toBe(true)
    expect(chips.find((c) => c.kind === "agent")?.primary).toBeFalsy()
  })

  it("names the directory of a focused standalone terminal", () => {
    const terminals = [
      {
        id: "t9",
        owner: { kind: "standalone", cwd_label: "~/code" },
        title: "shell",
      },
    ]
    const chips = insetHeaderChips(spineWith(terminals), undefined, {
      kind: "terminal",
      terminalId: "t9",
      owner: { kind: "standalone" },
    })
    expect(chips.find((c) => c.kind === "directory")?.value).toBe("~/code")
  })

  it("says nothing about a terminal the spine no longer carries", () => {
    const chips = insetHeaderChips(spineWith(), agent("s1", "feature-x"), {
      kind: "terminal",
      terminalId: "gone",
      owner: { kind: "session", sessionId: "s1" },
    })
    expect(chips).toEqual([])
  })
})
