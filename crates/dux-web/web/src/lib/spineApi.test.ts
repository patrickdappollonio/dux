import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { SpineFetchError, fetchSpine } from "./spineApi"

// The spine client is a thin GET wrapper (mirrors bootstrapApi/changesApi): on
// 2xx it returns the parsed JSON (coercing each session's `tabs` to an array so
// an older server that omits the field degrades safely); on a non-2xx it throws a
// `SpineFetchError` carrying the HTTP status; on a transport failure it throws 0.

beforeEach(() => {
  vi.restoreAllMocks()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe("fetchSpine", () => {
  it("issues a same-origin GET and returns the parsed body on 200", async () => {
    const body = {
      projects: [{ id: "p1" }],
      sessions: [{ id: "s1", project_id: "p1" }],
      sidebar: { groups: [], agentless_start: null },
    }
    const fetchMock = vi.fn(async () => ({
      ok: true,
      status: 200,
      json: async () => body,
      text: async () => "",
      headers: { get: () => null },
    })) as unknown as typeof fetch
    vi.stubGlobal("fetch", fetchMock)

    const result = await fetchSpine()
    // A session that omits `tabs`/`initial_branch`/`source_branch`/
    // `needs_attention`/`last_focused_tab` (an older server) is coerced to
    // `tabs: []`, empty-string branch fields, `needs_attention: false`, and
    // `last_focused_tab: null`.
    expect(result).toEqual({
      ...body,
      // This body carries no terminals in EITHER shape (no flat collection and
      // no nested arrays), so the flat collection is empty. A body that nests
      // them is a different case, covered below.
      terminals: [],
      sessions: [
        {
          id: "s1",
          project_id: "p1",
          tabs: [],
          // A session that omits `typing` (an older server) is coerced to
          // `typing: false`.
          typing: false,
          initial_branch: "",
          source_branch: "",
          needs_attention: false,
          last_focused_tab: null,
        },
      ],
    })
    expect(fetchMock).toHaveBeenCalledWith("/api/v1/spine", {
      credentials: "same-origin",
    })
  })

  // Leaving a browser tab open while dux restarts is ordinary, so a client can
  // outlive the server that served it and end up talking to a build that sends
  // the OTHER shape. A server predating the flat collection nests each terminal
  // inside its owner and puts no owner on the terminal itself, so the ingestion
  // boundary flattens those and attaches the owner they were nested under.
  // Reading only the flat field and normalizing a missing one to `[]` would show
  // that user no terminals at all, which is not what the old client did with the
  // very same body.
  it("flattens an older server's nested terminals and attaches the owner they were nested under", async () => {
    const body = {
      projects: [
        {
          id: "p1",
          terminals: [{ id: "t-p1", label: "Terminal 2", sort_order: 2 }],
        },
      ],
      sessions: [
        {
          id: "s1",
          project_id: "p1",
          terminals: [
            { id: "t-s1a", label: "Terminal 1", sort_order: 1 },
            { id: "t-s1b", label: "Terminal 3", sort_order: 3 },
          ],
        },
      ],
      sidebar: { groups: [], agentless_start: null },
    }
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({
        ok: true,
        status: 200,
        json: async () => body,
        text: async () => "",
        headers: { get: () => null },
      })) as unknown as typeof fetch,
    )

    const result = await fetchSpine()
    // Flat, in the `sort_order` order the new shape promises, each carrying the
    // owner it was nested under.
    expect(result.terminals.map((t) => [t.id, t.owner])).toEqual([
      ["t-s1a", { kind: "session", session_id: "s1" }],
      ["t-p1", { kind: "project", project_id: "p1" }],
      ["t-s1b", { kind: "session", session_id: "s1" }],
    ])
    // Older servers omit the newer terminal fields as well; they are coerced
    // here exactly as a flat terminal's are.
    expect(result.terminals[0].working).toBe(false)
    expect(result.terminals[0].typing).toBe(false)
    // The nested arrays do not survive: the flat collection is the one place
    // terminals live, so nothing downstream can read a second, staler copy.
    expect(
      (result.sessions[0] as unknown as { terminals?: unknown }).terminals,
    ).toBeUndefined()
    expect(
      (result.projects[0] as unknown as { terminals?: unknown }).terminals,
    ).toBeUndefined()
  })

  // The flat collection wins when a body somehow carries both, so the owner-
  // tagged shape is never second-guessed by a nested leftover.
  it("prefers the flat collection and drops nested arrays when a body carries both", async () => {
    const body = {
      projects: [{ id: "p1", terminals: [{ id: "stale-p", label: "old" }] }],
      sessions: [
        { id: "s1", project_id: "p1", terminals: [{ id: "stale-s", label: "old" }] },
      ],
      terminals: [
        {
          id: "t-1",
          owner: { kind: "session", session_id: "s1" },
          label: "Terminal 1",
        },
      ],
      sidebar: { groups: [], agentless_start: null },
    }
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({
        ok: true,
        status: 200,
        json: async () => body,
        text: async () => "",
        headers: { get: () => null },
      })) as unknown as typeof fetch,
    )

    const result = await fetchSpine()
    expect(result.terminals.map((t) => t.id)).toEqual(["t-1"])
  })

  it("passes a session's last_focused_tab through verbatim when the server sends one", async () => {
    const body = {
      projects: [],
      sessions: [{ id: "s1", project_id: "p1", last_focused_tab: "tab-1" }],
      sidebar: { groups: [], agentless_start: null },
    }
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({
        ok: true,
        status: 200,
        json: async () => body,
        text: async () => "",
        headers: { get: () => null },
      })) as unknown as typeof fetch,
    )

    const result = await fetchSpine()
    expect(result.sessions[0].last_focused_tab).toBe("tab-1")
  })

  it("coerces missing initial_branch/source_branch to empty strings at ingestion", async () => {
    const body = {
      projects: [],
      // A session from an older server omits the two branch fields (and tabs).
      sessions: [{ id: "s1", project_id: "p1" }],
      sidebar: { groups: [], agentless_start: null },
    }
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({
        ok: true,
        status: 200,
        json: async () => body,
        text: async () => "",
        headers: { get: () => null },
      })) as unknown as typeof fetch,
    )

    const result = await fetchSpine()
    expect(result.sessions[0]).toMatchObject({
      id: "s1",
      project_id: "p1",
      tabs: [],
      initial_branch: "",
      source_branch: "",
    })
  })

  it("throws a SpineFetchError carrying the HTTP status on a non-2xx", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({
        ok: false,
        status: 503,
        json: async () => ({}),
        text: async () => "server starting",
        headers: { get: () => null },
      })) as unknown as typeof fetch,
    )
    await expect(fetchSpine()).rejects.toMatchObject({
      name: "SpineFetchError",
      status: 503,
      message: "server starting",
    })
  })

  it("throws status 0 on a transport failure", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new Error("offline")
      }) as unknown as typeof fetch,
    )
    const err = await fetchSpine().catch((e) => e)
    expect(err).toBeInstanceOf(SpineFetchError)
    expect(err.status).toBe(0)
  })
})
