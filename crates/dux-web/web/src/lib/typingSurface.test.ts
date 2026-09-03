// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// The device-local typing-surface choice. It is transient UI state, not
// configuration: `ui.compose_bar` stays the only configuration surface, and
// this only remembers where the user last left the toggle ON THIS DEVICE so a
// reload does not snap the surface back under them.
// Every toast the module raised, through the one raiser. Mocked at
// `lib/notify` rather than at the toast library, because the raiser is the
// boundary this module is allowed to know about.
const notified: string[] = []
vi.mock("@/lib/notify", () => ({
  notifyInfo: (message: string) => void notified.push(message),
}))

function installStorage(seed: Record<string, string> = {}) {
  const mem = new Map(Object.entries(seed))
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => mem.get(k) ?? null,
    setItem: (k: string, v: string) => void mem.set(k, String(v)),
    removeItem: (k: string) => void mem.delete(k),
    clear: () => mem.clear(),
  })
  return mem
}

beforeEach(() => installStorage())
afterEach(() => vi.unstubAllGlobals())

const load = () => import("./typingSurface")

describe("typing-surface choice", () => {
  it("starts unchosen, so the capability answer stands", async () => {
    const m = await load()
    expect(m.readTypingSurface()).toBeNull()
  })

  it("remembers a choice under its own localStorage key", async () => {
    const mem = installStorage()
    const m = await load()
    m.setTypingSurface("direct")
    expect(mem.get(m.TYPING_SURFACE_KEY)).toBe("direct")
    expect(m.readTypingSurface()).toBe("direct")
  })

  // A RELOAD, modelled honestly: the module is evaluated again against the
  // storage the previous run left behind.
  it("survives a reload", async () => {
    const m = await load()
    m.setTypingSurface("compose")
    vi.resetModules()
    const fresh = await load()
    expect(fresh.readTypingSurface()).toBe("compose")
  })

  it("clears back to the capability answer", async () => {
    const m = await load()
    m.setTypingSurface("direct")
    m.setTypingSurface(null)
    expect(m.readTypingSurface()).toBeNull()
  })

  it("ignores a value it has no case for", async () => {
    installStorage({ "dux:typing-surface": "sideways" })
    const m = await load()
    expect(m.readTypingSurface()).toBeNull()
  })

  it("notifies subscribers so every open pane agrees at once", async () => {
    const m = await load()
    const seen: (string | null)[] = []
    const off = m.subscribeTypingSurface(() => seen.push(m.readTypingSurface()))
    m.setTypingSurface("direct")
    off()
    m.setTypingSurface("compose")
    expect(seen).toEqual(["direct"])
  })

  // A browser with storage denied (Safari private mode throws on write) must
  // still take the toggle for this session rather than crashing the pane.
  it("survives a storage that throws", async () => {
    // A fresh page: the in-memory fallback a previous test left behind is not
    // what is under test here.
    vi.resetModules()
    vi.stubGlobal("localStorage", {
      getItem: () => {
        throw new Error("denied")
      },
      setItem: () => {
        throw new Error("denied")
      },
      removeItem: () => {
        throw new Error("denied")
      },
    })
    const m = await load()
    expect(m.readTypingSurface()).toBeNull()
    expect(() => m.setTypingSurface("direct")).not.toThrow()
  })
})

// THE ONE-TIME HINT, for the switch that takes the whole bottom bar away.
//
// Choosing to type directly leaves nothing under the terminal: no message box,
// no key row, no `⋯`. The first time that happens on a device, dux says where
// the way back went. Once, and never again on this device.
describe("the direct-typing hint", () => {
  beforeEach(() => void notified.splice(0))

  it("fires on the first switch to direct, and never again", async () => {
    installStorage()
    vi.resetModules()
    const m = await load()
    m.switchTypingSurface("direct")
    expect(notified).toHaveLength(1)
    // It says where the way back is, which is the whole reason it exists.
    expect(notified[0]).toContain("Use virtual input")

    m.switchTypingSurface("compose")
    m.switchTypingSurface("direct")
    expect(notified).toHaveLength(1)
  })

  it("never fires for the way back", async () => {
    installStorage()
    vi.resetModules()
    const m = await load()
    m.switchTypingSurface("compose")
    expect(notified).toHaveLength(0)
  })

  it("stays quiet on a device that has already been told", async () => {
    const m = await load()
    installStorage({ [m.DIRECT_INPUT_HINT_KEY]: "shown" })
    vi.resetModules()
    const fresh = await load()
    fresh.switchTypingSurface("direct")
    expect(notified).toHaveLength(0)
  })

  // A storage that refuses reads also refuses writes, so a hint there would
  // fire on every single switch. It fires on none instead.
  it("stays quiet when storage refuses to remember it", async () => {
    vi.resetModules()
    vi.stubGlobal("localStorage", {
      getItem: () => {
        throw new Error("denied")
      },
      setItem: () => {
        throw new Error("denied")
      },
      removeItem: () => {
        throw new Error("denied")
      },
    })
    const m = await load()
    m.switchTypingSurface("direct")
    expect(notified).toHaveLength(0)
  })

  // The write still happens whatever the hint does: `setTypingSurface` is the
  // writer, and the toast is a side effect of the first switch only.
  it("still writes the choice", async () => {
    const mem = installStorage()
    vi.resetModules()
    const m = await load()
    m.switchTypingSurface("direct")
    expect(mem.get(m.TYPING_SURFACE_KEY)).toBe("direct")
  })
})
