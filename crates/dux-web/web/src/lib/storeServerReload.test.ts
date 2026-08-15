// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest"

// The store half of the restart check: when the tab gets back onto a socket, it
// asks the server which run of which build it is before carrying on. Same run,
// same build, and the reconnect is the ordinary in-place one it has always been;
// anything else and the tab is running code that no longer matches the server,
// so it hard reloads.
//
// jsdom, because the store only boots (and so only learns its baseline) when
// there is a `window`. `reloadPage` is stubbed so the test can watch the
// decision without jsdom trying to navigate.
const reloadPage = vi.fn()
vi.mock("./reloadPage", () => ({ reloadPage }))

// What `GET /api/v1/build` answers next. Mutated per test.
let identity: { version: string; process: string } | null = {
  version: "development",
  process: "run-1",
}
let buildReads = 0

const fetchMock = vi.fn(async (url: string) => {
  const u = String(url)
  if (u.includes("/api/v1/build")) {
    buildReads++
    if (identity === null) throw new Error("unreachable")
    return {
      ok: true,
      status: 200,
      json: async () => identity,
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  if (u.includes("/api/v1/workspace")) {
    return {
      ok: true,
      status: 200,
      json: async () => ({
        projects: [],
        sessions: [],
        terminals: [],
        sidebar: { groups: [], agentless_start: null },
      }),
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  return {
    ok: true,
    status: 200,
    json: async () => ({}),
    text: async () => "",
    headers: { get: () => null },
  } as unknown as Response
})

const mem = new Map<string, string>()
vi.stubGlobal("localStorage", {
  getItem: (k: string) => mem.get(k) ?? null,
  setItem: (k: string, v: string) => void mem.set(k, String(v)),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})
vi.stubGlobal("fetch", fetchMock)

const store = await import("./store")

// Let the boot reads (including the baseline build probe) settle.
async function settle(): Promise<void> {
  for (let i = 0; i < 20; i++) await Promise.resolve()
  await new Promise((r) => setTimeout(r, 0))
}

// The very first `onOpen` belongs to boot and is skipped by the store's own
// one-shot guard; every later one is a RECONNECT. Consume the boot open once,
// up front, so each test drives a genuine reconnect.
await settle()
store.eventsSocket.onOpen()

beforeEach(() => {
  reloadPage.mockClear()
})

describe("reconnect against a possibly-restarted server", () => {
  it("reads the baseline once at boot", () => {
    expect(buildReads).toBeGreaterThan(0)
  })

  it("reconnects in place when it is the same run of the same build", async () => {
    identity = { version: "development", process: "run-1" }
    store.eventsSocket.onOpen()
    await settle()
    expect(reloadPage).not.toHaveBeenCalled()
  })

  it("hard reloads when the run id moved but the version did not", async () => {
    // The development case: `dux_version` is the constant string "development"
    // for every non-release build, so rebuilding dux and restarting it looks
    // identical by version. Without the run id this reconnect would carry on
    // silently against a server the tab's code no longer matches.
    identity = { version: "development", process: "run-2" }
    store.eventsSocket.onOpen()
    await settle()
    expect(reloadPage).toHaveBeenCalledTimes(1)
  })

  it("hard reloads when the version moved", async () => {
    identity = { version: "v9.9.9", process: "run-1" }
    store.eventsSocket.onOpen()
    await settle()
    expect(reloadPage).toHaveBeenCalledTimes(1)
  })

  it("does not reload when the probe itself fails", async () => {
    // Unknown is not "changed". The socket just came back; the probe failing is
    // ordinary flakiness, and reloading on it would throw the tab away at the
    // worst moment.
    identity = null
    store.eventsSocket.onOpen()
    await settle()
    expect(reloadPage).not.toHaveBeenCalled()
  })
})
