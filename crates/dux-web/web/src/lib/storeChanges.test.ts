import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { ChangedFileView } from "./types"

// Exercises the store's `changes` slice end to end: the subscription wiring on
// selectSession/selectTerminal, the REST request state machine (loading ->
// loaded/error, self-heal, out-of-order/stale drops, wrong-session guard), and
// the reconnect re-subscribe + refetch. The events socket's own wire behaviour
// lives in eventsSocket.test.ts; here we spy its methods / drive its callbacks.
//
// The store connects the events socket and builds two sockets at import. We
// route `/changes` GETs to manually-resolvable deferred promises so a test
// controls fetch timing precisely.

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

// In-flight changes fetches, in call order. Tests resolve them explicitly.
let pendingChanges: { sessionId: string; d: Deferred; signal?: AbortSignal }[] = []
// A real fetch rejects once its signal aborts. A test that needs a late answer
// from a superseded fetch (a server that answered before the abort landed)
// turns this off.
let honorAbort = true
// Records subscribe/fetch ordering across the two different doubles.
let callOrder: string[] = []

function changesResponse(
  body: unknown,
  init?: { status?: number; retryAfter?: string },
) {
  const status = init?.status ?? 200
  return {
    ok: status >= 200 && status < 300,
    status,
    json: async () => body,
    text: async () => (typeof body === "string" ? body : JSON.stringify(body)),
    headers: {
      get: (name: string) =>
        name.toLowerCase() === "retry-after" ? (init?.retryAfter ?? null) : null,
    },
  }
}

const fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
  const u = String(url)
  if (u.includes("/changes")) {
    callOrder.push("fetch")
    const m = u.match(/sessions\/([^/]+)\/changes/)
    const sessionId = m ? decodeURIComponent(m[1]) : ""
    const d = defer()
    const signal = init?.signal ?? undefined
    if (honorAbort) {
      signal?.addEventListener("abort", () =>
        d.reject(new DOMException("aborted", "AbortError")),
      )
    }
    pendingChanges.push({ sessionId, d, signal })
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
  pendingChanges = []
  callOrder = []
  honorAbort = true
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

afterEach(() => {
  vi.unstubAllGlobals()
})

async function loadStore() {
  const mod = await import("./store")
  await vi.waitFor(() => {
    expect(mod.getSnapshot().booted).toBe(true)
  })
  return mod
}

// Flush the fetch promise chain (fetch -> json/text -> apply).
const tick = () => new Promise((r) => setTimeout(r, 0))

const file = (path: string): ChangedFileView => ({
  status: "M",
  path,
  additions: 1,
  deletions: 0,
  binary: false,
})

describe("changes slice: subscription wiring", () => {
  it("selectSession subscribes the session topic and fetches (subscribe before fetch)", async () => {
    const mod = await loadStore()
    const realSub = mod.eventsSocket.subscribe.bind(mod.eventsSocket)
    vi.spyOn(mod.eventsSocket, "subscribe").mockImplementation((topics) => {
      callOrder.push("subscribe")
      realSub(topics)
    })
    mod.selectSession("s1")
    expect(callOrder).toEqual(["subscribe", "fetch"])
    expect(mod.getSnapshot().changes).toMatchObject({
      sessionId: "s1",
      phase: "loading",
    })
  })

  it("switching sessions unsubscribes the previous and subscribes the next", async () => {
    const mod = await loadStore()
    mod.selectSession("s1")
    const sub = vi.spyOn(mod.eventsSocket, "subscribe")
    const unsub = vi.spyOn(mod.eventsSocket, "unsubscribe")
    mod.selectSession("s2")
    expect(unsub).toHaveBeenCalledWith(["session:s1:changes"])
    expect(sub).toHaveBeenCalledWith(["session:s2:changes"])
  })

  it("selectSession(null) unsubscribes and clears the slice", async () => {
    const mod = await loadStore()
    mod.selectSession("s1")
    const unsub = vi.spyOn(mod.eventsSocket, "unsubscribe")
    mod.selectSession(null)
    expect(unsub).toHaveBeenCalledWith(["session:s1:changes"])
    expect(mod.getSnapshot().changes).toMatchObject({
      sessionId: null,
      phase: "idle",
    })
  })

  it("selectTerminal subscribes the PARENT session topic", async () => {
    const mod = await loadStore()
    const sub = vi.spyOn(mod.eventsSocket, "subscribe")
    mod.selectTerminal("term-1", { kind: "session", sessionId: "s3" })
    expect(sub).toHaveBeenCalledWith(["session:s3:changes"])
    expect(mod.getSnapshot().changes.sessionId).toBe("s3")
  })
})

describe("changes slice: request state machine", () => {
  it("loading -> loaded applies the response", async () => {
    const mod = await loadStore()
    mod.selectSession("s1")
    expect(mod.getSnapshot().changes.phase).toBe("loading")
    expect(pendingChanges).toHaveLength(1)
    pendingChanges[0].d.resolve(
      changesResponse({ rev: 1, staged: [file("a")], unstaged: [] }),
    )
    await tick()
    const c = mod.getSnapshot().changes
    expect(c.phase).toBe("loaded")
    expect(c.rev).toBe(1)
    expect(c.staged).toHaveLength(1)
  })

  it("loading -> error on a non-2xx (git lock 409)", async () => {
    const mod = await loadStore()
    mod.selectSession("s1")
    pendingChanges[0].d.resolve(
      changesResponse("index.lock present", { status: 409, retryAfter: "2" }),
    )
    await tick()
    const c = mod.getSnapshot().changes
    expect(c.phase).toBe("error")
    expect(c.error).toContain("index.lock")
  })

  it("error self-heals on a later session.changes event", async () => {
    const mod = await loadStore()
    mod.selectSession("s1")
    pendingChanges[0].d.resolve(changesResponse("boom", { status: 409 }))
    await tick()
    expect(mod.getSnapshot().changes.phase).toBe("error")
    // An event (any rev) ALWAYS refetches out of the error state.
    mod.eventsSocket.onEvent({ event: "session.changes", id: "s1", rev: 2 })
    expect(pendingChanges).toHaveLength(2)
    pendingChanges[1].d.resolve(
      changesResponse({ rev: 2, staged: [], unstaged: [file("b")] }),
    )
    await tick()
    const c = mod.getSnapshot().changes
    expect(c.phase).toBe("loaded")
    expect(c.rev).toBe(2)
  })

  it("force-refetches on a rev-less event (Lagged cold-session catch-up)", async () => {
    const mod = await loadStore()
    mod.selectSession("s1")
    pendingChanges[0].d.resolve(
      changesResponse({ rev: 3, staged: [], unstaged: [] }),
    )
    await tick()
    expect(mod.getSnapshot().changes.rev).toBe(3)
    const before = pendingChanges.length
    // The server's Lagged catch-up for a cold session carries NO rev. It must
    // still trigger a refetch; `undefined >= 3` would otherwise skip it.
    mod.eventsSocket.onEvent({ event: "session.changes", id: "s1" })
    expect(pendingChanges.length).toBe(before + 1)
  })

  it("a late losing error does not clobber an already-loaded slice", async () => {
    const mod = await loadStore()
    mod.selectSession("s1")
    // A second in-flight fetch for the same session: events for one session
    // coalesce, so the race comes from leaving and coming straight back.
    mod.selectSession("s2")
    mod.selectSession("s1")
    expect(pendingChanges).toHaveLength(3)
    // The newer fetch wins the race and loads the pane.
    pendingChanges[2].d.resolve(
      changesResponse({ rev: 5, staged: [file("loaded")], unstaged: [] }),
    )
    await tick()
    expect(mod.getSnapshot().changes.phase).toBe("loaded")
    expect(mod.getSnapshot().changes.rev).toBe(5)
    // The first fetch loses the race and fails late (a slow 409). It must NOT
    // flip the loaded pane to an error pane.
    pendingChanges[0].d.resolve(
      changesResponse("index.lock present", { status: 409 }),
    )
    await tick()
    const c = mod.getSnapshot().changes
    expect(c.phase).toBe("loaded")
    expect(c.rev).toBe(5)
  })

  it("drops an out-of-order response with an older rev", async () => {
    const mod = await loadStore()
    mod.selectSession("s1")
    // Two fetches for s1 in flight: the initial load, and the one a quick
    // switch away and back started.
    mod.selectSession("s2")
    mod.selectSession("s1")
    expect(pendingChanges).toHaveLength(3)
    // The newer response lands first and is applied.
    pendingChanges[2].d.resolve(
      changesResponse({ rev: 9, staged: [file("new")], unstaged: [] }),
    )
    await tick()
    expect(mod.getSnapshot().changes.rev).toBe(9)
    // The older (stale) response arrives late and must be dropped.
    pendingChanges[0].d.resolve(
      changesResponse({ rev: 1, staged: [], unstaged: [] }),
    )
    await tick()
    const c = mod.getSnapshot().changes
    expect(c.rev).toBe(9)
    expect(c.staged).toHaveLength(1)
  })

  it("ignores a wrong-session event while a fetch is in flight", async () => {
    const mod = await loadStore()
    mod.selectSession("s1")
    expect(pendingChanges).toHaveLength(1)
    // An event for a session we are NOT viewing must not trigger a fetch.
    mod.eventsSocket.onEvent({ event: "session.changes", id: "s2", rev: 5 })
    expect(pendingChanges).toHaveLength(1)
    pendingChanges[0].d.resolve(
      changesResponse({ rev: 1, staged: [], unstaged: [] }),
    )
    await tick()
    expect(mod.getSnapshot().changes.phase).toBe("loaded")
  })

  it("a 404 clears the slice", async () => {
    const mod = await loadStore()
    mod.selectSession("s1")
    pendingChanges[0].d.resolve(changesResponse("unknown session", { status: 404 }))
    await tick()
    expect(mod.getSnapshot().changes).toMatchObject({
      sessionId: null,
      phase: "idle",
    })
  })

  it("drops a response whose session is no longer selected", async () => {
    const mod = await loadStore()
    mod.selectSession("s1")
    const first = pendingChanges[0]
    // Switch away before the first fetch resolves.
    mod.selectSession("s2")
    first.d.resolve(
      changesResponse({ rev: 5, staged: [file("stale")], unstaged: [] }),
    )
    await tick()
    // The slice still belongs to s2 and stayed in its loading window.
    expect(mod.getSnapshot().changes.sessionId).toBe("s2")
    expect(mod.getSnapshot().changes.phase).toBe("loading")
  })
})

describe("changes slice: reconnect", () => {
  it("re-fetches the selected session on socket reopen and keeps the topic set", async () => {
    const mod = await loadStore()
    mod.selectSession("s1")
    pendingChanges[0].d.resolve(
      changesResponse({ rev: 1, staged: [], unstaged: [] }),
    )
    await tick()
    const before = pendingChanges.length
    // Simulate the events socket reopening: the store's onOpen refetches.
    mod.eventsSocket.onOpen()
    expect(pendingChanges.length).toBe(before + 1)
    expect(mod.getSnapshot().changes.phase).toBe("loading")
    // The full interest set (coarse topics + the fine session topic) survives so
    // the socket can re-send it on the wire.
    expect(new Set(mod.eventsSocket.topics)).toEqual(
      new Set(["sessions", "projects", "config", "session:s1:changes"]),
    )
  })
})

describe("changes slice: subscribe catch-up", () => {
  // The server sends a `session.changes` frame immediately after a client
  // subscribes to a fine topic. When the server's cache is cold it omits `rev`.
  // The client must treat `rev === undefined` as a force-refetch so the changes
  // pane converges even when no rev comparison is possible.

  it("a revless session.changes event triggers loadChanges regardless of the current rev", async () => {
    const mod = await loadStore()
    mod.selectSession("s1")
    // First fetch resolves: the pane is loaded with rev 7.
    pendingChanges[0].d.resolve(
      changesResponse({ rev: 7, staged: [], unstaged: [] }),
    )
    await tick()
    expect(mod.getSnapshot().changes.rev).toBe(7)

    const before = pendingChanges.length
    // The server's subscribe catch-up carries no rev (cold cache). The store
    // must NOT short-circuit on `undefined >= 7` and must call loadChanges.
    mod.eventsSocket.onEvent({ event: "session.changes", id: "s1" })
    expect(pendingChanges.length).toBe(
      before + 1,
      "a revless catch-up must trigger a refetch even when a rev is already held",
    )
  })

  it("a revless session.changes event also triggers loadChanges from the loading phase", async () => {
    const mod = await loadStore()
    mod.selectSession("s1")
    // The initial fetch is still in-flight (loading phase, rev = 0).
    expect(mod.getSnapshot().changes.phase).toBe("loading")

    const before = pendingChanges.length
    // The subscribe catch-up arrives before the initial fetch resolves. The
    // fetch in flight may predate what the event reports, so one more follows
    // it; it waits for that one rather than racing it.
    mod.eventsSocket.onEvent({ event: "session.changes", id: "s1" })
    pendingChanges[0].d.resolve(
      changesResponse({ rev: 1, staged: [], unstaged: [] }),
    )
    await tick()
    expect(pendingChanges.length).toBe(
      before + 1,
      "a revless catch-up must trigger a second fetch even in the loading phase",
    )
  })
})

// A worktree with tens of thousands of changed files makes every fetch and
// every apply expensive: the response is megabytes of JSON and the pane lists
// every row. Refetching on an event that reports nothing new, racing several
// fetches for one session, and replacing an identical list all multiplied that.
describe("changes slice: refetch economy", () => {
  async function loadedAt(rev: number, unstaged: ChangedFileView[] = []) {
    const mod = await loadStore()
    mod.selectSession("s1")
    pendingChanges[0].d.resolve(changesResponse({ rev, staged: [], unstaged }))
    await tick()
    expect(mod.getSnapshot().changes.rev).toBe(rev)
    return mod
  }

  it("does not refetch for an event at the rev already applied", async () => {
    const mod = await loadedAt(4)
    const before = pendingChanges.length
    mod.eventsSocket.onEvent({ event: "session.changes", id: "s1", rev: 4 })
    expect(pendingChanges.length).toBe(before)
  })

  it("refetches for a strictly newer rev", async () => {
    const mod = await loadedAt(4)
    const before = pendingChanges.length
    mod.eventsSocket.onEvent({ event: "session.changes", id: "s1", rev: 5 })
    expect(pendingChanges.length).toBe(before + 1)
  })

  it("coalesces events that arrive while a fetch is in flight into one follow-up", async () => {
    const mod = await loadedAt(1)
    const before = pendingChanges.length
    mod.eventsSocket.onEvent({ event: "session.changes", id: "s1", rev: 2 })
    mod.eventsSocket.onEvent({ event: "session.changes", id: "s1", rev: 3 })
    mod.eventsSocket.onEvent({ event: "session.changes", id: "s1", rev: 4 })
    expect(pendingChanges.length).toBe(before + 1)

    pendingChanges.at(-1)!.d.resolve(
      changesResponse({ rev: 2, staged: [], unstaged: [] }),
    )
    await tick()
    // Exactly one follow-up, because the fetch that just landed may predate
    // the later events.
    expect(pendingChanges.length).toBe(before + 2)
    pendingChanges.at(-1)!.d.resolve(
      changesResponse({ rev: 4, staged: [], unstaged: [] }),
    )
    await tick()
    expect(pendingChanges.length).toBe(before + 2)
    expect(mod.getSnapshot().changes.rev).toBe(4)
  })

  it("leaves the state untouched when the answer is the one already applied", async () => {
    const mod = await loadedAt(4, [file("a")])
    mod.eventsSocket.onEvent({ event: "session.changes", id: "s1" })
    const before = mod.getSnapshot()
    pendingChanges.at(-1)!.d.resolve(
      changesResponse({ rev: 4, staged: [], unstaged: [file("a")] }),
    )
    await tick()
    expect(mod.getSnapshot()).toBe(before)
  })

  it("keeps the applied lists, and every unchanged file, when only part of the answer moved", async () => {
    const mod = await loadedAt(4, [file("a"), file("b")])
    const applied = mod.getSnapshot().changes
    mod.eventsSocket.onEvent({ event: "session.changes", id: "s1", rev: 5 })
    pendingChanges.at(-1)!.d.resolve(
      changesResponse({
        rev: 5,
        staged: [],
        unstaged: [file("a"), { ...file("b"), additions: 9 }],
      }),
    )
    await tick()
    const next = mod.getSnapshot().changes
    expect(next.rev).toBe(5)
    expect(next.staged).toBe(applied.staged)
    expect(next.unstaged).not.toBe(applied.unstaged)
    expect(next.unstaged[0]).toBe(applied.unstaged[0])
    expect(next.unstaged[1]!.additions).toBe(9)
  })
})

// Coalescing must never let one stuck request hold the pane hostage: a
// half-open connection never settles, and everything queued behind it would
// wait with it. Anything that puts the pane back into loading starts over.
describe("changes slice: a stuck fetch", () => {
  it("is superseded by a reconnect, which starts a new fetch that loads the pane", async () => {
    const mod = await loadStore()
    mod.selectSession("s1")
    const stuck = pendingChanges[0]!

    mod.eventsSocket.onOpen()

    expect(stuck.signal?.aborted).toBe(true)
    expect(pendingChanges).toHaveLength(2)
    pendingChanges[1]!.d.resolve(
      changesResponse({ rev: 3, staged: [], unstaged: [file("a")] }),
    )
    await tick()
    expect(mod.getSnapshot().changes).toMatchObject({ phase: "loaded", rev: 3 })
  })

  it("is superseded by Refresh, which starts a new fetch that loads the pane", async () => {
    const mod = await loadStore()
    mod.selectSession("s1")
    const stuck = pendingChanges[0]!

    mod.refreshChanges()

    expect(stuck.signal?.aborted).toBe(true)
    expect(pendingChanges).toHaveLength(2)
    pendingChanges[1]!.d.resolve(
      changesResponse({ rev: 2, staged: [], unstaged: [] }),
    )
    await tick()
    expect(mod.getSnapshot().changes.phase).toBe("loaded")
  })

  it("never flashes an error for a superseded fetch that fails late", async () => {
    honorAbort = false
    const mod = await loadStore()
    mod.selectSession("s1")
    const superseded = pendingChanges[0]!
    mod.refreshChanges()

    superseded.d.resolve(changesResponse("index.lock present", { status: 409 }))
    await tick()

    expect(mod.getSnapshot().changes.phase).toBe("loading")
    pendingChanges[1]!.d.resolve(
      changesResponse({ rev: 2, staged: [], unstaged: [] }),
    )
    await tick()
    expect(mod.getSnapshot().changes.phase).toBe("loaded")
  })

  it("ignores a superseded fetch's late answer", async () => {
    honorAbort = false
    const mod = await loadStore()
    mod.selectSession("s1")
    const superseded = pendingChanges[0]!
    mod.refreshChanges()

    superseded.d.resolve(
      changesResponse({ rev: 9, staged: [file("stale")], unstaged: [] }),
    )
    await tick()

    expect(mod.getSnapshot().changes.phase).toBe("loading")
  })

  it("does not book a follow-up behind a fetch a reset has superseded", async () => {
    const mod = await loadStore()
    mod.selectSession("s1")
    mod.eventsSocket.onEvent({ event: "session.changes", id: "s1" })
    mod.refreshChanges()
    pendingChanges[1]!.d.resolve(
      changesResponse({ rev: 2, staged: [], unstaged: [] }),
    )
    await tick()
    await tick()
    expect(pendingChanges).toHaveLength(2)
  })

  it("times out into an error that says what happened", async () => {
    const mod = await loadStore()
    vi.useFakeTimers()
    try {
      mod.selectSession("s1")
      await vi.advanceTimersByTimeAsync(30_000)
      const changes = mod.getSnapshot().changes
      expect(changes.phase).toBe("error")
      expect(changes.error).toMatch(/did not send/)
    } finally {
      vi.useRealTimers()
    }
  })

  it("times out on the deadline the bootstrap document configured", async () => {
    const mod = await loadStore()
    // The same module instance the store reads, since modules were reset.
    const timing = await import("./connectionTiming")
    timing.publishConnectionTiming({ changes_request_timeout_seconds: 2 })
    vi.useFakeTimers()
    try {
      mod.selectSession("s1")
      await vi.advanceTimersByTimeAsync(1_999)
      expect(mod.getSnapshot().changes.phase).toBe("loading")
      await vi.advanceTimersByTimeAsync(1)
      expect(mod.getSnapshot().changes.phase).toBe("error")
      expect(mod.getSnapshot().changes.error).toMatch(/within 2 seconds/)
    } finally {
      vi.useRealTimers()
      timing.publishConnectionTiming(undefined)
    }
  })
})
