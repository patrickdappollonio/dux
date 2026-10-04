import { afterEach, describe, expect, it, vi } from "vitest"

import { ConfigChangedError, configApi } from "./configApi"

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
    // key, matching the backend's `#[serde(default)]` "absent = leave unchanged".
    expect(JSON.parse(opts.body as string)).toEqual({ favicon: "amber" })
  })

  it("sends empty strings for a reset-to-default", async () => {
    const fetchMock = stubFetchOk()
    await configApi.setInstanceIdentity({ title: "", favicon: "" })

    const [, opts] = fetchMock.mock.calls[0] as [string, RequestInit]
    expect(JSON.parse(opts.body as string)).toEqual({ title: "", favicon: "" })
  })
})

describe("configApi raw config", () => {
  it("reads the content and the token the save must carry back", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({
        ok: true,
        status: 200,
        json: async () => ({ content: "[ui]\n", token: "abc" }),
        text: async () => "",
      })),
    )
    expect(await configApi.readRawConfig()).toEqual({ content: "[ui]\n", token: "abc" })
  })

  it("sends the token with the save", async () => {
    const fetchMock = stubFetchOk()
    await configApi.writeRawConfig("[ui]\n", "abc")
    const [path, opts] = fetchMock.mock.calls[0] as [string, RequestInit]
    expect(path).toBe("/api/v1/config/raw")
    expect(opts.method).toBe("PUT")
    expect(JSON.parse(opts.body as string)).toEqual({ content: "[ui]\n", token: "abc" })
  })

  it("throws a ConfigChangedError with the server's sentence on a 409 conflict", async () => {
    const body = JSON.stringify({ error: "config_changed", message: "config.toml changed on disk" })
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({
        ok: false,
        status: 409,
        text: async () => body,
        clone() {
          return this
        },
      })),
    )
    const err = await configApi.writeRawConfig("[ui]\n", "abc").catch((e: unknown) => e)
    expect(err).toBeInstanceOf(ConfigChangedError)
    expect((err as Error).message).toBe("config.toml changed on disk")
  })

  it("keeps a plain 400 as an ordinary error", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({ ok: false, status: 400, text: async () => "bad toml" })),
    )
    const err = await configApi.writeRawConfig("x", "abc").catch((e: unknown) => e)
    expect(err).not.toBeInstanceOf(ConfigChangedError)
    expect((err as Error).message).toBe("bad toml")
  })
})
