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
    // A session that omits `tabs`/`initial_branch`/`source_branch` (an older
    // server) is coerced to `tabs: []` and empty-string branch fields.
    expect(result).toEqual({
      ...body,
      sessions: [
        {
          id: "s1",
          project_id: "p1",
          tabs: [],
          initial_branch: "",
          source_branch: "",
        },
      ],
    })
    expect(fetchMock).toHaveBeenCalledWith("/api/v1/spine", {
      credentials: "same-origin",
    })
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
