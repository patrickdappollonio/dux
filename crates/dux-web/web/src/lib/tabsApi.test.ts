import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { setConnectionId } from "./connection"
import { TabsApiError, tabsApi } from "./tabsApi"

// Wire-level coverage for the agent provider-tab REST client, mirroring
// `terminalsApi.test.ts`: each verb hits the exact nested endpoint with the right
// method + JSON body, stamps `X-Connection-Id`, and surfaces a non-2xx as a typed
// error.

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
    body: init.body as string | undefined,
  }
}

beforeEach(() => {
  setConnectionId("conn-7")
})

afterEach(() => {
  vi.unstubAllGlobals()
  setConnectionId(null)
})

describe("tabsApi", () => {
  it("create POSTs the nested tabs endpoint with the provider and returns the body", async () => {
    const fetchMock = stubOkFetch(201, { tab_id: "b1", provider: "codex" })
    const created = await tabsApi.create("s1", "codex")
    const c = lastCall(fetchMock)
    expect(c.url).toBe("/api/v1/sessions/s1/tabs")
    expect(c.method).toBe("POST")
    expect(c.headers["x-connection-id"]).toBe("conn-7")
    expect(c.headers["content-type"]).toBe("application/json")
    expect(JSON.parse(c.body ?? "{}")).toEqual({ provider: "codex" })
    expect(created).toEqual({ tab_id: "b1", provider: "codex" })
  })

  it("create omits the provider (empty body) when none is given", async () => {
    const fetchMock = stubOkFetch(201, { tab_id: "b1", provider: "claude" })
    await tabsApi.create("s1")
    expect(JSON.parse(lastCall(fetchMock).body ?? "null")).toEqual({})
  })

  it("remove DELETEs the nested tab endpoint (encoding ids) and resolves the authoritative detached flag", async () => {
    const fetchMock = stubOkFetch(200, { detached: true })
    const result = await tabsApi.remove("s 1", "b/2")
    const c = lastCall(fetchMock)
    expect(c.url).toBe("/api/v1/sessions/s%201/tabs/b%2F2")
    expect(c.method).toBe("DELETE")
    expect(c.headers["x-connection-id"]).toBe("conn-7")
    expect(result).toEqual({ detached: true })
  })

  it("remove resolves undefined for an older server's bodiless 204", async () => {
    const fetchMock = stubOkFetch(204, null)
    const result = await tabsApi.remove("s1", "b2")
    expect(lastCall(fetchMock).method).toBe("DELETE")
    expect(result).toBeUndefined()
  })

  it("patch PATCHes the tab endpoint with the new provider", async () => {
    const fetchMock = stubOkFetch(200, {})
    await tabsApi.patch("s1", "b1", "opencode")
    const c = lastCall(fetchMock)
    expect(c.url).toBe("/api/v1/sessions/s1/tabs/b1")
    expect(c.method).toBe("PATCH")
    expect(JSON.parse(c.body ?? "{}")).toEqual({ provider: "opencode" })
  })

  it("throws a typed TabsApiError carrying status + message on non-2xx", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({
        ok: false,
        status: 400,
        text: async () => 'provider "nope" is not configured',
        headers: { get: () => null },
      })) as unknown as typeof fetch,
    )
    const err = await tabsApi.patch("s1", "b1", "nope").catch((e) => e)
    expect(err).toBeInstanceOf(TabsApiError)
    expect(err.status).toBe(400)
    expect(err.message).toBe('provider "nope" is not configured')
  })
})
