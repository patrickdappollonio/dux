import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { BootstrapFetchError, fetchBootstrap } from "./bootstrapApi"

// The bootstrap client is a thin GET wrapper (mirrors changesApi): on 2xx it
// returns the parsed JSON; on a non-2xx it throws a `BootstrapFetchError`
// carrying the HTTP status; on a transport failure it throws status 0.

beforeEach(() => {
  vi.restoreAllMocks()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe("fetchBootstrap", () => {
  it("issues a same-origin GET and returns the parsed body on 200", async () => {
    const body = {
      available_providers: ["claude"],
      macros: [],
      palette_commands: [],
      welcome_tips: ["hi"],
      dux_version: "v1.0.0",
      randomize_agent_names_by_default: true,
      gh_available: false,
      pr_banner_position: "top",
      agent_scrollback_lines: 10000,
      show_changes_pane: true,
      global_env: {},
    }
    const fetchMock = vi.fn(async () => ({
      ok: true,
      status: 200,
      json: async () => body,
      text: async () => "",
      headers: { get: () => null },
    })) as unknown as typeof fetch
    vi.stubGlobal("fetch", fetchMock)

    const result = await fetchBootstrap()
    expect(result).toEqual(body)
    expect(fetchMock).toHaveBeenCalledWith("/api/v1/bootstrap", {
      credentials: "same-origin",
    })
  })

  it("throws a BootstrapFetchError carrying the HTTP status on a non-2xx", async () => {
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
    await expect(fetchBootstrap()).rejects.toMatchObject({
      name: "BootstrapFetchError",
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
    const err = await fetchBootstrap().catch((e) => e)
    expect(err).toBeInstanceOf(BootstrapFetchError)
    expect(err.status).toBe(0)
  })
})
