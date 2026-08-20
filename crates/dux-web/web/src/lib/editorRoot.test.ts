import { describe, expect, it } from "vitest"

import {
  agentRoot,
  editorRootForTarget,
  rootApiBase,
  rootKey,
  rootPtyId,
  rootSessionId,
  sameRoot,
  type EditorRoot,
} from "./editorRoot"

const projectTerminal: EditorRoot = {
  kind: "terminal",
  terminalId: "term-1",
  owner: { kind: "project", projectId: "p1" },
}

const standaloneTerminal: EditorRoot = {
  kind: "terminal",
  terminalId: "term-2",
  owner: { kind: "standalone" },
}

describe("the editor's root", () => {
  it("keys agents and terminals in namespaces that cannot collide", () => {
    // An agent id and a terminal id are minted by different counters, so a bare
    // id is not a key: the namespace is what keeps two editors apart.
    expect(rootKey(agentRoot("alpha"))).toBe("agent:alpha")
    expect(rootKey(projectTerminal)).toBe("terminal:term-1")
    expect(rootKey(standaloneTerminal)).toBe("terminal:term-2")
    expect(rootKey(agentRoot("term-1"))).not.toBe(rootKey(projectTerminal))
  })

  it("sends a session-owned terminal's editor to its agent, not to a terminal root", () => {
    // The terminal spawned in the agent's worktree already has an editor: the
    // agent's, with the full git surface. Its rows and its existing address
    // must keep meaning exactly that.
    expect(
      editorRootForTarget({
        kind: "terminal",
        terminalId: "term-9",
        owner: { kind: "session", sessionId: "alpha" },
      }),
    ).toEqual(agentRoot("alpha"))
    expect(
      editorRootForTarget({ kind: "agent", sessionId: "alpha", tabId: "tab-2" }),
    ).toEqual(agentRoot("alpha"))
    expect(editorRootForTarget(projectTerminal)).toEqual(projectTerminal)
    expect(editorRootForTarget(standaloneTerminal)).toEqual(standaloneTerminal)
  })

  it("addresses each root at the namespace the server serves it from", () => {
    expect(rootApiBase(agentRoot("a/b"))).toBe("/api/v1/sessions/a%2Fb")
    expect(rootApiBase(projectTerminal)).toBe(
      "/api/v1/projects/p1/terminals/term-1",
    )
    expect(rootApiBase(standaloneTerminal)).toBe("/api/v1/terminals/term-2")
  })

  it("names the pty a file dropped on this root should be uploaded through", () => {
    expect(rootPtyId(agentRoot("alpha"))).toBe("alpha")
    expect(rootPtyId(projectTerminal)).toBe("term-1")
  })

  it("answers which roots have session-scoped state and which have none", () => {
    expect(rootSessionId(agentRoot("alpha"))).toBe("alpha")
    expect(rootSessionId(projectTerminal)).toBeNull()
    expect(rootSessionId(standaloneTerminal)).toBeNull()
  })

  it("compares roots by identity, nulls included", () => {
    expect(sameRoot(agentRoot("alpha"), agentRoot("alpha"))).toBe(true)
    expect(sameRoot(agentRoot("alpha"), agentRoot("beta"))).toBe(false)
    expect(sameRoot(projectTerminal, { ...projectTerminal })).toBe(true)
    expect(sameRoot(null, null)).toBe(true)
    expect(sameRoot(null, agentRoot("alpha"))).toBe(false)
  })
})
