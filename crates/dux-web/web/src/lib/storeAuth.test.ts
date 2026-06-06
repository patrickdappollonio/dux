import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// `store` reads `location`, `localStorage`, and registers a `popstate` listener
// at module load (the test env is node, not a DOM), AND now fires a boot
// `/api/me` fetch and constructs a `DuxSocket` (which references `WebSocket`).
// Stub the minimum surface so the import succeeds; the boot fetch is steered to
// a 401 so the store settles in the "anonymous" phase, the clean starting point
// for the login() transition tests below.
//
// This mirrors the module-load side-effect precedent in paletteRegistry.test.ts,
// extended with a fetch + WebSocket stub because A3 added the boot round-trip.

// A queue of fetch responses; each call shifts the next one. Tests push the
// responses they expect IN ORDER. The boot fetch (one /api/me) consumes the
// first entry.
let fetchQueue: Array<{
  status: number
  body?: unknown
  headers?: Record<string, string>
}> = []

function makeResponse(entry: {
  status: number
  body?: unknown
  headers?: Record<string, string>
}) {
  return {
    status: entry.status,
    json: async () => entry.body ?? null,
    headers: {
      get: (name: string) => entry.headers?.[name.toLowerCase()] ?? null,
    },
  }
}

const fetchMock = vi.fn(async () => {
  const next = fetchQueue.shift()
  if (!next) throw new Error("fetch called with no queued response")
  return makeResponse(next) as unknown as Response
})

// A no-op WebSocket stand-in: DuxSocket only constructs one when `.connect()` is
// called, and login()/the boot path may call connect(); we never assert on the
// socket here, only on auth state, so a constructible stub suffices.
class FakeWebSocket {
  onopen: (() => void) | null = null
  onclose: (() => void) | null = null
  onerror: (() => void) | null = null
  onmessage: (() => void) | null = null
  binaryType = ""
  readyState = 0
  close() {}
  send() {}
}

beforeEach(() => {
  fetchQueue = []
  vi.stubGlobal("location", { host: "localhost:0" })
  vi.stubGlobal("localStorage", {
    getItem: () => null,
    setItem: () => {},
    removeItem: () => {},
  })
  vi.stubGlobal("window", { addEventListener: () => {} })
  vi.stubGlobal("WebSocket", FakeWebSocket)
  vi.stubGlobal("fetch", fetchMock)
  vi.resetModules()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

// Import the store fresh AFTER queuing the boot response, so the module-load
// `bootAuth()` consumes it. Returns the store module once the boot phase has
// settled out of "checking".
async function loadStoreAfterBoot(bootStatus: number, bootBody?: unknown) {
  fetchQueue.push({ status: bootStatus, body: bootBody })
  const mod = await import("./store")
  // bootAuth() is async (fetch + json + setState); poll until it settles.
  await vi.waitFor(() => {
    expect(mod.getSnapshot().auth.phase).not.toBe("checking")
  })
  return mod
}

describe("store auth boot", () => {
  it("settles in anonymous when boot /api/me is 401", async () => {
    const mod = await loadStoreAfterBoot(401)
    // The store does not export its snapshot directly, but useDux's underlying
    // getSnapshot is the source; we read it via a subscribe trick.
    expect(currentPhase(mod)).toBe("anonymous")
  })

  it("settles in disabled when boot /api/me reports auth off", async () => {
    const mod = await loadStoreAfterBoot(200, { auth: "disabled" })
    expect(currentPhase(mod)).toBe("disabled")
  })

  it("settles in authed when boot /api/me returns a username", async () => {
    const mod = await loadStoreAfterBoot(200, { username: "alice" })
    expect(currentPhase(mod)).toBe("authed")
    expect(currentUsername(mod)).toBe("alice")
  })
})

describe("store login()", () => {
  it("flips to authed on a 200 with the username", async () => {
    const mod = await loadStoreAfterBoot(401)
    expect(currentPhase(mod)).toBe("anonymous")

    fetchQueue.push({ status: 200, body: { username: "alice" } })
    await mod.login("alice", "pw")

    expect(currentPhase(mod)).toBe("authed")
    expect(currentUsername(mod)).toBe("alice")
    expect(currentError(mod)).toBeNull()
  })

  it("surfaces the generic message on a 401", async () => {
    const mod = await loadStoreAfterBoot(401)

    fetchQueue.push({ status: 401 })
    await mod.login("alice", "wrong")

    expect(currentPhase(mod)).toBe("anonymous")
    expect(currentError(mod)).toBe("Invalid username or password.")
  })

  it("surfaces a throttle message with the Retry-After window on a 429", async () => {
    const mod = await loadStoreAfterBoot(401)

    fetchQueue.push({ status: 429, headers: { "retry-after": "30" } })
    await mod.login("alice", "pw")

    expect(currentPhase(mod)).toBe("anonymous")
    expect(currentError(mod)).toBe("Too many attempts — try again in 30 s.")
  })

  it("surfaces a network message when the fetch rejects", async () => {
    const mod = await loadStoreAfterBoot(401)

    // An empty queue makes the mock throw, simulating a network failure.
    await mod.login("alice", "pw")

    expect(currentPhase(mod)).toBe("anonymous")
    expect(currentError(mod)).toBe(
      "Could not reach the server. Please try again.",
    )
  })
})

describe("store logout()", () => {
  it("returns to anonymous and clears the username", async () => {
    const mod = await loadStoreAfterBoot(200, { username: "alice" })
    expect(currentPhase(mod)).toBe("authed")

    fetchQueue.push({ status: 204 })
    await mod.logout()

    expect(currentPhase(mod)).toBe("anonymous")
    expect(currentUsername(mod)).toBeNull()
  })
})

// --- Snapshot readers ------------------------------------------------------
// useDux() wraps useSyncExternalStore (needs React, absent in this node test
// env), so we read the live state via the store's `getSnapshot` — the same
// accessor React consumes. Store mutations emit synchronously, so the snapshot
// is current immediately after each awaited action.

type StoreModule = typeof import("./store")

function currentPhase(mod: StoreModule) {
  return mod.getSnapshot().auth.phase
}
function currentUsername(mod: StoreModule) {
  return mod.getSnapshot().auth.username
}
function currentError(mod: StoreModule) {
  return mod.getSnapshot().auth.error
}
