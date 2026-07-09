import { afterEach, describe, expect, it, vi } from "vitest"

import { configApi } from "./configApi"

// The CustomizeWebappDialog test mocks the store wholesale, so this is the one
// place the REAL client → wire shape is pinned: a swapped field, wrong path, or
// wrong method here would otherwise only be caught by the Rust endpoint tests
// (which hand-author the JSON and never run this browser code).

afterEach(() => {
  vi.unstubAllGlobals()
})

function stubFetchOk(): ReturnType<typeof vi.fn> {
  const fetchMock = vi.fn(async () => ({
    ok: true,
    status: 200,
    text: async () => "",
  }))
  vi.stubGlobal("fetch", fetchMock)
  return fetchMock
}

describe("configApi.setInstanceIdentity", () => {
  it("POSTs the title + favicon body verbatim to /api/v1/config/instance-identity", async () => {
    const fetchMock = stubFetchOk()
    await configApi.setInstanceIdentity({ title: "dux (prod)", favicon: "blue" })

    expect(fetchMock).toHaveBeenCalledTimes(1)
    const [path, opts] = fetchMock.mock.calls[0] as [string, RequestInit]
    expect(path).toBe("/api/v1/config/instance-identity")
    expect(opts.method).toBe("POST")
    expect(JSON.parse(opts.body as string)).toEqual({
      title: "dux (prod)",
      favicon: "blue",
    })
  })

  it("sends only the field that was provided (partial update)", async () => {
    const fetchMock = stubFetchOk()
    await configApi.setInstanceIdentity({ favicon: "amber" })

    const [, opts] = fetchMock.mock.calls[0] as [string, RequestInit]
    // JSON.stringify drops `undefined`, so a favicon-only call sends no `title`
    // key — matching the backend's `#[serde(default)]` "absent = leave unchanged".
    expect(JSON.parse(opts.body as string)).toEqual({ favicon: "amber" })
  })

  it("sends empty strings for a reset-to-default", async () => {
    const fetchMock = stubFetchOk()
    await configApi.setInstanceIdentity({ title: "", favicon: "" })

    const [, opts] = fetchMock.mock.calls[0] as [string, RequestInit]
    expect(JSON.parse(opts.body as string)).toEqual({ title: "", favicon: "" })
  })
})
