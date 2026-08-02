import { afterEach, describe, expect, it, vi } from "vitest"

import { fetchServerIdentity, serverChanged } from "./buildApi"

afterEach(() => {
  vi.unstubAllGlobals()
})

function stubFetch(impl: (url: string) => unknown) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string) => impl(String(url))),
  )
}

describe("serverChanged", () => {
  const base = { version: "development", process: "run-1" }

  it("says NO when both reads are the same run of the same build", () => {
    // A network blip. The tab reconnects in place; whatever the user had open
    // (an editor tab, a half-typed command) survives.
    expect(serverChanged(base, { version: "development", process: "run-1" })).toBe(
      false,
    )
  })

  it("says YES when the run id moved but the version did not", () => {
    // THE development case, and the one a version-only check would miss:
    // `dux_version` is the constant string "development" for every build that is
    // not a tagged release, so a rebuild-and-restart looks identical by version.
    expect(serverChanged(base, { version: "development", process: "run-2" })).toBe(
      true,
    )
  })

  it("says YES when the version moved", () => {
    expect(serverChanged(base, { version: "v0.2.0", process: "run-1" })).toBe(true)
    expect(serverChanged(base, { version: "v0.2.0", process: "run-2" })).toBe(true)
  })

  it("says NO when either side is unknown", () => {
    // A failed probe proves nothing, and reloading on "I could not ask" would
    // reload a tab every time the server was briefly unreachable, which is
    // exactly when the user is least able to afford it.
    expect(serverChanged(null, base)).toBe(false)
    expect(serverChanged(base, null)).toBe(false)
    expect(serverChanged(null, null)).toBe(false)
  })
})

describe("fetchServerIdentity", () => {
  it("reads the two documented fields", async () => {
    stubFetch(() => ({
      ok: true,
      status: 200,
      json: async () => ({ version: "development", process: "run-7" }),
    }))
    await expect(fetchServerIdentity()).resolves.toEqual({
      version: "development",
      process: "run-7",
    })
  })

  it("answers null rather than throwing when the server is unreachable", async () => {
    stubFetch(() => {
      throw new Error("network down")
    })
    await expect(fetchServerIdentity()).resolves.toBeNull()
  })

  it("answers null on a non-2xx", async () => {
    stubFetch(() => ({ ok: false, status: 503, json: async () => ({}) }))
    await expect(fetchServerIdentity()).resolves.toBeNull()
  })

  it("answers null on a body missing either field", async () => {
    // Older server, proxy error page, anything that is not the document. Unknown
    // must never masquerade as "changed".
    stubFetch(() => ({
      ok: true,
      status: 200,
      json: async () => ({ version: "development" }),
    }))
    await expect(fetchServerIdentity()).resolves.toBeNull()
    stubFetch(() => ({ ok: true, status: 200, json: async () => "nope" }))
    await expect(fetchServerIdentity()).resolves.toBeNull()
  })
})
