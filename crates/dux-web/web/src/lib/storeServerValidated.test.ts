// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// Loaded once at collection, where no test timeout applies. Its first load is
// by far the slowest step in the file; left to the first test, a busy machine
// pushes that test past its timeout. Every test still imports its own fresh
// instance after `vi.resetModules`. Under jsdom there is a window, so this
// instance boots for real; its app socket is stopped at once, or it would keep
// reconnecting on real timers into the tests' fakes.
import { eventsSocket as collectionSocket } from "./store"
collectionSocket.dispose()

// The run-identity hard reload, recorded rather than performed: jsdom cannot
// navigate.
const { reloadPage } = vi.hoisted(() => ({ reloadPage: vi.fn() }))
vi.mock("./reloadPage", () => ({ reloadPage }))

// JSDOM DELIBERATELY, AND WITHOUT IT THE FILE TESTS THE WRONG BRANCH. The store
// runs `boot()` only where there is a `window`, so in the default node
// environment this file exercised the path no browser ever takes: no boot, no
// baseline read, no sockets. The regression pinned below lives entirely inside
// what boot does, so it is only reachable here.

// THE PTY RETRY GATE IS "VALIDATED", NOT "OPEN".
//
// `eventsSocket.onOpen` fires the run-identity probe as
// `void reloadIfServerChanged()`, an async fetch that resolves later, so
// `conn === "open"` is true well before the check has answered. A PTY socket
// allowed to retry in that window can force-launch a provider on a RESTARTED
// server, which is exactly what the check exists to prevent. This drives the
// real store's own socket callbacks and asserts the signal the gate reads.

// The store fires fetches at import and from `onOpen`; serve them all with an
// empty-but-valid body, and the build endpoint with a stable identity so the run
// never looks changed.
/// Whether the run-identity endpoint answers, flipped per test. "unreachable"
/// is the UNKNOWN answer: the probe resolves to null and the tab learns nothing.
let buildAnswers: "ok" | "unreachable" = "ok"
/// How many times the run-identity endpoint has been asked, so a test can pin
/// that an unknown answer is followed by another question.
let buildProbeCount = 0
/// The run the identity endpoint reports, moved by a test to stand for a restart.
let buildProcess = "run-1"

const fetchMock = vi.fn(async (url: string) => {
  // The public status route answers as every dux does: no password here.
  if (String(url).includes("/api/v1/auth/status")) {
    return new Response(
      JSON.stringify({ password_set: false, required_here: false, signed_in: false }),
      { status: 200, headers: { "content-type": "application/json" } },
    )
  }
  const u = String(url)
  if (u.includes("/api/v1/build")) {
    buildProbeCount += 1
    if (buildAnswers === "unreachable") throw new Error("unreachable")
    return {
      ok: true,
      status: 200,
      json: async () => ({ version: "development", process: buildProcess }),
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

// A WebSocket that neither opens nor closes. `boot()` connects the events socket
// for real, and jsdom answers a `ws://` URL with an immediate failure whose
// `close` would clear the very signal these tests read. The socket's own
// lifecycle is driven by hand below, through the store's public callbacks.
class InertWebSocket {
  static readonly CONNECTING = 0
  readyState = 0
  binaryType = "blob"
  onopen: (() => void) | null = null
  onmessage: (() => void) | null = null
  onclose: (() => void) | null = null
  onerror: (() => void) | null = null
  constructor(readonly url: string) {}
  send(): void {}
  close(): void {}
}

let store: typeof import("./store")
let serverValidated: () => boolean

beforeEach(async () => {
  vi.resetModules()
  fetchMock.mockClear()
  buildAnswers = "ok"
  buildProbeCount = 0
  buildProcess = "run-1"
  reloadPage.mockClear()
  // Re-stub per test, BEFORE the store is imported: `boot()` fires its reads at
  // import time, and `afterEach` unstubs, so a module-scope stub would leave
  // every test after the first talking to jsdom's own fetch.
  vi.stubGlobal("fetch", fetchMock)
  vi.stubGlobal("WebSocket", InertWebSocket)
  // jsdom does not publish `localStorage` as a bare global here, and the store
  // touches it at module scope; the same stub every other store-importing test
  // installs.
  const mem = new Map<string, string>()
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => mem.get(k) ?? null,
    setItem: (k: string, v: string) => void mem.set(k, String(v)),
    removeItem: (k: string) => void mem.delete(k),
    clear: () => mem.clear(),
  })
  // Both imports must come from the SAME module graph as the reset store, or the
  // test would read a signal nothing writes.
  store = await import("./store")
  serverValidated = (await import("./serverValidated")).serverValidated
})

// `vi.resetModules` gives the next test a fresh store but does not stop this
// one: its app socket would keep reconnecting on real timers, and every socket
// and request it makes would land on the next test's fakes.
afterEach(async () => {
  const store = await import("./store")
  store.eventsSocket.dispose()
  vi.unstubAllGlobals()
})

/// Let every promise the callback kicked off settle.
const settle = () => new Promise<void>((resolve) => setTimeout(resolve, 0))

/// Drive the store to the state a RECONNECT starts from: boot has settled, the
/// FIRST events open has happened (which is the one `boot` arms the
/// skip-the-duplicate-load flag for, so it fires no probe), and the socket has
/// then dropped. From here the next open is a real reconnect and does probe.
async function reachReconnect(): Promise<void> {
  await settle()
  store.eventsSocket.onOpen()
  store.eventsSocket.onConn("closed")
}

describe("the PTY retry gate", () => {
  it("is HELD while a RECONNECTING events socket is open but the run check has not answered", async () => {
    // Boot's own baseline read opens the gate, so the held window this test is
    // about is a RECONNECT's: the drop shut the gate, and the open callback
    // fires the probe and returns long before it resolves.
    await reachReconnect()
    expect(serverValidated()).toBe(false)
    store.eventsSocket.onOpen()
    expect(serverValidated()).toBe(false)
  })

  it("is RELEASED once the probe resolves against the same run", async () => {
    await reachReconnect()
    store.eventsSocket.onOpen()
    await settle()
    expect(serverValidated()).toBe(true)
  })

  // THE REGRESSION THIS TEST EXISTS FOR. `boot()` arms `skipNextEventsOnOpenLoad`
  // so the first events open does not duplicate the load the boot driver already
  // performed, and the run-identity probe used to live ONLY inside the branch
  // that flag skips. So on a freshly loaded page the signal was never set, the
  // PTY gate never opened, and a dropped PTY socket re-armed its retry timer
  // forever without ever attempting one: a terminal that lost its socket on a
  // fresh page never came back, with no cover and no Reconnect box to say so.
  // Measured in the preview container. Boot itself now answers the question,
  // because the baseline read that boot performs is BY CONSTRUCTION a round trip
  // to the very server this tab loaded from.
  it("is RELEASED by boot alone, before any events open, on a freshly loaded page", async () => {
    await settle()
    expect(serverValidated()).toBe(true)
  })

  it("is RE-HELD when the events socket closes, because the next open owes a fresh check", async () => {
    await settle()
    expect(serverValidated()).toBe(true)
    store.eventsSocket.onConn("closed")
    expect(serverValidated()).toBe(false)
  })

  // THE BASELINE HAS THE SAME HOLE. `loadServerIdentityBaseline` is
  // fire-and-forget, so a page loaded during a blip used to keep a null baseline
  // for its whole life, and a null baseline never reports a change: the
  // run-identity hard reload was silently switched off for that tab. It re-asks
  // now, backing off, and never gives up while it has no baseline, because a
  // reconnect has nothing to compare a new run against until it does.
  it("keeps asking for a missing boot baseline, so a restart after the endpoint recovers still reloads", async () => {
    // Rebuild the store with the endpoint already unreachable, so the read boot
    // itself performs is the one that comes back unknown.
    store.eventsSocket.dispose()
    vi.resetModules()
    buildAnswers = "unreachable"
    buildProbeCount = 0
    vi.useFakeTimers()
    try {
      store = await import("./store")
      await vi.advanceTimersByTimeAsync(1)
      expect(buildProbeCount).toBe(1)
      // The boot's own open, which is not a reconnect and asks nothing.
      store.eventsSocket.onOpen()
      // Each re-ask lands at its backoff step and not a millisecond before, and
      // past the last step the asking carries on at that step's gap.
      let asked = 1
      for (const gap of [5000, 10000, 20000, 40000, 60000, 60000, 60000]) {
        await vi.advanceTimersByTimeAsync(gap - 2)
        expect(buildProbeCount).toBe(asked)
        await vi.advanceTimersByTimeAsync(2)
        asked += 1
        expect(buildProbeCount).toBe(asked)
      }

      // The endpoint answers again, still the same run: the next re-ask learns
      // it as the baseline within one gap, and the asking ends there.
      buildAnswers = "ok"
      await vi.advanceTimersByTimeAsync(60_000)
      expect(buildProbeCount).toBe(asked + 1)
      await vi.advanceTimersByTimeAsync(600_000)
      expect(buildProbeCount).toBe(asked + 1)
      expect(reloadPage).not.toHaveBeenCalled()

      // dux restarts, and the reconnect that follows sees the new run.
      buildProcess = "run-2"
      store.eventsSocket.onConn("closed")
      store.eventsSocket.onOpen()
      await vi.advanceTimersByTimeAsync(1)
      expect(reloadPage).toHaveBeenCalledTimes(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it("stops the endless asking for a missing baseline when the app socket is disposed", async () => {
    store.eventsSocket.dispose()
    vi.resetModules()
    buildAnswers = "unreachable"
    buildProbeCount = 0
    vi.useFakeTimers()
    try {
      store = await import("./store")
      await vi.advanceTimersByTimeAsync(400_000)
      const asked = buildProbeCount
      expect(asked).toBeGreaterThan(7)
      store.eventsSocket.dispose()
      await vi.advanceTimersByTimeAsync(600_000)
      expect(buildProbeCount).toBe(asked)
    } finally {
      vi.useRealTimers()
    }
  })

  // With a baseline known, an unknown run check is worth a few re-asks and not
  // a request a minute for the life of the tab: the next reconnect asks again.
  it("backs off a run check that follows a known baseline, stops after the last step, and restarts on a reconnect", async () => {
    await reachReconnect()
    buildAnswers = "unreachable"
    vi.useFakeTimers()
    try {
      store.eventsSocket.onOpen()
      await vi.advanceTimersByTimeAsync(1)
      let asked = buildProbeCount
      for (const gap of [5000, 10000, 20000, 40000, 60000, 60000]) {
        await vi.advanceTimersByTimeAsync(gap - 2)
        expect(buildProbeCount).toBe(asked)
        await vi.advanceTimersByTimeAsync(2)
        asked += 1
        expect(buildProbeCount).toBe(asked)
      }
      // The sixth re-ask was the last one.
      await vi.advanceTimersByTimeAsync(600_000)
      expect(buildProbeCount).toBe(asked)

      // A reconnect asks at once and backs off from the first step again.
      store.eventsSocket.onConn("closed")
      store.eventsSocket.onOpen()
      await vi.advanceTimersByTimeAsync(1)
      expect(buildProbeCount).toBe(asked + 1)
      await vi.advanceTimersByTimeAsync(5000)
      expect(buildProbeCount).toBe(asked + 2)
    } finally {
      vi.useRealTimers()
    }
  })

  // UNKNOWN IS NOT LATCHED. A probe that cannot reach the endpoint opens the
  // gate, because unknown is not evidence of a change and holding every terminal
  // shut over one unreachable endpoint is the wrong failure. But it is not
  // evidence of SAMENESS either, so the tab asks again; without the re-ask, one
  // transient failure on a reconnect after a restart left the page running old
  // code against a new run for the rest of its life.
  it("re-asks after an UNKNOWN answer instead of latching it", async () => {
    await reachReconnect()
    buildAnswers = "unreachable"
    vi.useFakeTimers()
    try {
      store.eventsSocket.onOpen()
      await vi.advanceTimersByTimeAsync(1)
      expect(serverValidated()).toBe(true)
      const asked = buildProbeCount
      await vi.advanceTimersByTimeAsync(6000)
      expect(buildProbeCount).toBeGreaterThan(asked)
    } finally {
      vi.useRealTimers()
    }
  })
})
