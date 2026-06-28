import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { setConnectionId } from "./connection"
import { TerminalsApiError, terminalsApi } from "./terminalsApi"

// Wire-level coverage for the Phase 5 companion-terminal REST client: create and
// delete hit the exact nested endpoint with the right method, stamp the
// per-connection id as `X-Connection-Id`, and surface a non-2xx as a typed error.
// Mirrors `restActions.test.ts`'s style.

function stubOkFetch(status = 200, jsonBody: unknown = {}) {
  const fetchMock = vi.fn(async () => ({
    ok: status >= 200 && status < 300,
    status,
    json: async () => jsonBody,
    text: async () => (jsonBody ? JSON.stringify(jsonBody) : ""),
    headers: { get: () => null },
  })) as unknown as typeof fetch
  vi.stubGlobal("fetch", fetchMock)
  return fetchMock as unknown as ReturnType<typeof vi.fn>
}

function lastCall(fetchMock: ReturnType<typeof vi.fn>) {
  const [url, init] = fetchMock.mock.calls.at(-1) as [string, RequestInit]
  return {
    url,
    method: init.method,
    headers: init.headers as Record<string, string>,
  }
}

beforeEach(() => {
  setConnectionId("conn-7")
})

afterEach(() => {
  vi.unstubAllGlobals()
  setConnectionId(null)
})

describe("terminalsApi", () => {
  it("create POSTs the nested terminals endpoint and returns the body", async () => {
    const fetchMock = stubOkFetch(201, { terminal_id: "t1", label: "Terminal 1" })
    const created = await terminalsApi.create("s1")
    const c = lastCall(fetchMock)
    expect(c.url).toBe("/api/v1/sessions/s1/terminals")
    expect(c.method).toBe("POST")
    expect(c.headers["x-connection-id"]).toBe("conn-7")
    expect(created).toEqual({ terminal_id: "t1", label: "Terminal 1" })
  })

  it("remove DELETEs the nested terminal endpoint (encoding ids)", async () => {
    const fetchMock = stubOkFetch(204, null)
    await terminalsApi.remove("s 1", "t/2")
    const c = lastCall(fetchMock)
    expect(c.url).toBe("/api/v1/sessions/s%201/terminals/t%2F2")
    expect(c.method).toBe("DELETE")
    expect(c.headers["x-connection-id"]).toBe("conn-7")
  })

  it("omits the connection-id header until it is known", async () => {
    setConnectionId(null)
    const fetchMock = stubOkFetch(201, { terminal_id: "t1", label: "Terminal 1" })
    await terminalsApi.create("s1")
    expect(lastCall(fetchMock).headers["x-connection-id"]).toBeUndefined()
  })

  it("throws a typed TerminalsApiError carrying status + message on non-2xx", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({
        ok: false,
        status: 404,
        text: async () => "unknown terminal",
        headers: { get: () => null },
      })) as unknown as typeof fetch,
    )
    const err = await terminalsApi.remove("s1", "t1").catch((e) => e)
    expect(err).toBeInstanceOf(TerminalsApiError)
    expect(err.status).toBe(404)
    expect(err.message).toBe("unknown terminal")
  })
})
