import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { SpineFetchError, fetchSpine } from "./spineApi"

// The spine client is a thin GET wrapper (mirrors bootstrapApi/changesApi): on
// 2xx it returns the parsed JSON; on a non-2xx it throws a `SpineFetchError`
// carrying the HTTP status; on a transport failure it throws status 0.

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
    expect(result).toEqual(body)
    expect(fetchMock).toHaveBeenCalledWith("/api/v1/spine", {
      credentials: "same-origin",
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
