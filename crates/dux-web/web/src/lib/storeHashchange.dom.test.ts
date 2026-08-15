// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// Any plain hash anchor (a bookmark, a link a user pastes, markdown-preview
// links) drives the router with no click handler and no history call — the
// browser navigates the fragment and the router follows the URL. Whether a
// browser delivers that navigation as a `popstate`, a `hashchange`, or both
// is not guaranteed (jsdom, notably, fires only `hashchange` for an anchor
// click), so the store listens to BOTH and `applyUrlRoute` is idempotent and
// never writes the URL back. This test clicks a REAL anchor under jsdom —
// the environment where popstate alone would silently miss — rather than
// synthesizing a popstate.

const SPINE = {
  projects: [{ id: "p1", name: "Repo", path: "/tmp/p1" }],
  sessions: [
    {
      id: "s1",
      project_id: "p1",
      status: "active",
      title: "s1",
      branch_name: "s1",
      working: false,
      needs_attention: false,
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-01T00:00:00Z",
      tabs: [{ id: "s1" }],
    },
  ],
  terminals: [],
  sidebar: { groups: [], agentless_start: null },
}

const fetchMock = vi.fn(async (url: string) => {
  const u = String(url)
  const body = u.includes("/api/v1/workspace")
    ? SPINE
    : u.includes("/changes")
      ? { rev: 1, staged: [], unstaged: [] }
      : {}
  return {
    ok: true,
    status: 200,
    json: async () => body,
    text: async () => JSON.stringify(body),
    headers: { get: () => null },
  } as unknown as Response
})

class FakeWebSocket {
  onopen: (() => void) | null = null
  onclose: (() => void) | null = null
  onerror: (() => void) | null = null
  onmessage: (() => void) | null = null
  binaryType = ""
  readyState = 1
  close() {}
  send() {}
}

describe("a plain hash anchor drives the router", () => {
  beforeEach(() => {
    const mem = new Map<string, string>()
    vi.stubGlobal("localStorage", {
      getItem: (k: string) => mem.get(k) ?? null,
      setItem: (k: string, v: string) => void mem.set(k, String(v)),
      removeItem: (k: string) => void mem.delete(k),
      clear: () => mem.clear(),
    })
    vi.stubGlobal("fetch", fetchMock)
    vi.stubGlobal("WebSocket", FakeWebSocket)
    vi.resetModules()
  })

  afterEach(() => {
    document.body.innerHTML = ""
    window.location.hash = ""
    vi.unstubAllGlobals()
  })

  it("leaves the standalone surface when its open-in-dux anchor is clicked", async () => {
    window.location.hash = "#/editor/agent/s1"
    const mod = await import("./store")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().spine).not.toBeNull()
    })
    expect(mod.getSnapshot().standaloneEditor).toBe(true)
    expect(mod.getSnapshot().editorRoute).not.toBeNull()

    const anchor = document.createElement("a")
    anchor.href = "#/agent/s1"
    document.body.appendChild(anchor)
    anchor.click()

    await vi.waitFor(() => {
      expect(mod.getSnapshot().standaloneEditor).toBe(false)
    })
    // The in-app address names no editor, so the editor closed with the move,
    // and the selection follows the address.
    expect(mod.getSnapshot().editorRoute).toBeNull()
    expect(mod.getSnapshot().selectedSessionId).toBe("s1")
    expect(window.location.hash).toBe("#/agent/s1")
  })
})
