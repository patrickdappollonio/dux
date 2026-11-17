import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// Loaded once at collection; see storeAddProject.test.ts for why.
import "./store"

// The standalone-agent picker's two reads, against the real store: only the
// LATEST answer of each may land. A reply to a picker since closed and
// reopened, or to a browse a newer browse replaced, is dropped whole, toast
// included.

const { notifyErrorMock } = vi.hoisted(() => ({ notifyErrorMock: vi.fn() }))

vi.mock("./notify", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./notify")>()),
  notifyError: notifyErrorMock,
}))

interface Deferred {
  resolve: (value: unknown) => void
  reject: (reason: unknown) => void
  promise: Promise<unknown>
}

function defer(): Deferred {
  let resolve!: (value: unknown) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<unknown>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { resolve, reject, promise }
}

function jsonResponse(body: unknown) {
  return {
    ok: true,
    status: 200,
    json: async () => body,
    text: async () => JSON.stringify(body),
    headers: { get: () => null },
  }
}

// In-flight reads, in call order; tests answer them explicitly.
let pendingBrowse: Deferred[] = []
let pendingShortcuts: Deferred[] = []

const fetchMock = vi.fn(async (url: string) => {
  const u = String(url)
  if (u.includes("/api/v1/standalone-agent/project-shortcuts")) {
    const d = defer()
    pendingShortcuts.push(d)
    return d.promise as unknown as Response
  }
  if (u.includes("/api/v1/browse")) {
    const d = defer()
    pendingBrowse.push(d)
    return d.promise as unknown as Response
  }
  throw new Error(`unexpected fetch: ${u}`)
})

class FakeWebSocket {
  onopen: (() => void) | null = null
  onclose: (() => void) | null = null
  onerror: (() => void) | null = null
  onmessage: (() => void) | null = null
  binaryType = ""
  readyState = 1
  close() {}
  send() {}
}

beforeEach(() => {
  pendingBrowse = []
  pendingShortcuts = []
  notifyErrorMock.mockClear()
  vi.stubGlobal("location", { host: "localhost:0" })
  vi.stubGlobal("localStorage", {
    getItem: () => null,
    setItem: () => {},
    removeItem: () => {},
  })
  vi.stubGlobal("window", { addEventListener: () => {} })
  vi.stubGlobal("history", { go: () => {} })
  vi.stubGlobal("WebSocket", FakeWebSocket)
  vi.stubGlobal("fetch", fetchMock)
  vi.resetModules()
})

afterEach(async () => {
  const store = await import("./store")
  store.eventsSocket.dispose()
  vi.unstubAllGlobals()
})

async function loadStore() {
  const mod = await import("./store")
  await vi.waitFor(() => {
    expect(mod.getSnapshot().booted).toBe(true)
  })
  return mod
}

const list = (name: string) => ({
  caption: "caption",
  rows: [
    {
      project_id: name,
      name,
      folder_label: `~/${name}`,
      path: `/home/u/${name}`,
      state: "available",
      reason: null,
    },
  ],
})

describe("standalone picker reads", () => {
  it("keeps the reopened picker's shortcuts when the first open answers late", async () => {
    const store = await loadStore()
    store.openStandaloneAgentPicker()
    store.closeStandaloneAgentPicker()
    store.openStandaloneAgentPicker()
    await vi.waitFor(() => expect(pendingShortcuts).toHaveLength(2))

    pendingShortcuts[1].resolve(jsonResponse(list("new")))
    await vi.waitFor(() =>
      expect(
        store.getSnapshot().standaloneProjectShortcuts?.rows[0].name,
      ).toBe("new"),
    )
    pendingShortcuts[0].resolve(jsonResponse(list("stale")))
    await Promise.resolve()
    await new Promise((r) => setTimeout(r, 0))
    expect(store.getSnapshot().standaloneProjectShortcuts?.rows[0].name).toBe(
      "new",
    )
  })

  it("raises no toast for a failure that belongs to a picker since reopened", async () => {
    const store = await loadStore()
    store.openStandaloneAgentPicker()
    store.closeStandaloneAgentPicker()
    store.openStandaloneAgentPicker()
    await vi.waitFor(() => expect(pendingShortcuts).toHaveLength(2))

    pendingShortcuts[0].reject(new Error("boom"))
    await new Promise((r) => setTimeout(r, 0))
    expect(notifyErrorMock).not.toHaveBeenCalled()
  })

  it("lands only the latest browse and tells each caller whether its own landed", async () => {
    const store = await loadStore()
    store.openStandaloneAgentPicker()
    await vi.waitFor(() => expect(pendingBrowse).toHaveLength(1))
    // A shortcut is clicked while the opening browse is still in flight.
    const picked = store.browseDir("/home/u/site")
    await vi.waitFor(() => expect(pendingBrowse).toHaveLength(2))

    pendingBrowse[1].resolve(
      jsonResponse({ path: "/home/u/site", entries: [] }),
    )
    await expect(picked).resolves.toBe("/home/u/site")
    expect(store.getSnapshot().browsePath).toBe("/home/u/site")

    // The opening browse answers last and must not move the explorer back.
    pendingBrowse[0].resolve(jsonResponse({ path: "/home/u", entries: [] }))
    await new Promise((r) => setTimeout(r, 0))
    expect(store.getSnapshot().browsePath).toBe("/home/u/site")
    expect(store.getSnapshot().browseLoading).toBe(false)
  })

  it("answers null to a browse that failed, and says why", async () => {
    const store = await loadStore()
    store.openStandaloneAgentPicker()
    await vi.waitFor(() => expect(pendingBrowse).toHaveLength(1))
    const picked = store.browseDir("/home/u/gone")
    await vi.waitFor(() => expect(pendingBrowse).toHaveLength(2))

    pendingBrowse[1].reject(new Error("no such directory"))
    await expect(picked).resolves.toBeNull()
    expect(notifyErrorMock).toHaveBeenCalledTimes(1)
  })
})
