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
    // A fresh page, so "unchosen" is the module's own answer rather than an
    // in-memory choice a previous case left lying around.
    installStorage({ "dux:typing-surface": "sideways" })
    vi.resetModules()
    const m = await load()
    expect(m.readTypingSurface()).toBeNull()
  })

  // A GARBAGE VALUE IS NOT A DECISION EITHER, and a storage that cannot be
  // written over it must not pin the toggle. This is the shape a browser that
  // allows reads and refuses writes lands in: the stored string is whatever was
  // there, and the choice the user just made lives only in memory. Reading the
  // unrecognized string as "nothing chosen" and then ignoring the in-memory
  // answer left the switch looking dead for the rest of the page.
  it("falls back to the live choice when the stored value makes no sense", async () => {
    vi.resetModules()
    const mem = new Map([["dux:typing-surface", "sideways"]])
    vi.stubGlobal("localStorage", {
      getItem: (k: string) => mem.get(k) ?? null,
      setItem: () => {
        throw new Error("denied")
      },
      removeItem: () => {
        throw new Error("denied")
      },
    })
    const m = await load()
    expect(m.readTypingSurface()).toBeNull()
    m.setTypingSurface("direct")
    expect(m.readTypingSurface()).toBe("direct")
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

// THE ONE-TIME HINT, for the switch that leaves nothing under the terminal.
//
// Choosing to type directly takes the message box away, and on a pane whose key
// row is down too it takes the last row with it: no `⋯`, nothing. The first
// time THAT happens on a device, dux says where the way back went. Once, and
// never again on this device. A switch that leaves a key row behind says
// nothing, because the bottom `⋯` is still on screen carrying the way back.
describe("the direct-typing hint", () => {
  beforeEach(() => void notified.splice(0))

  it("fires on the first switch to direct, and never again", async () => {
    installStorage()
    vi.resetModules()
    const m = await load()
    m.switchTypingSurface("direct", true)
    expect(notified).toHaveLength(1)
    // It says where the way back is, which is the whole reason it exists.
    expect(notified[0]).toContain("Use virtual input")

    m.switchTypingSurface("compose", true)
    m.switchTypingSurface("direct", true)
    expect(notified).toHaveLength(1)
  })

  it("never fires for the way back", async () => {
    installStorage()
    vi.resetModules()
    const m = await load()
    m.switchTypingSurface("compose", true)
    expect(notified).toHaveLength(0)
  })

  it("stays quiet on a device that has already been told", async () => {
    const m = await load()
    installStorage({ [m.DIRECT_INPUT_HINT_KEY]: "shown" })
    vi.resetModules()
    const fresh = await load()
    fresh.switchTypingSurface("direct", true)
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
    m.switchTypingSurface("direct", true)
    expect(notified).toHaveLength(0)
  })

  // THE SENTENCE HAS TO NAME A CONTROL THAT IS THERE. The way back lives in
  // different places on the two shells, and a hint naming the other one sends
  // the reader hunting for something that is not on the screen in front of them.
  it("names the cluster over the terminal on a phone", async () => {
    const m = await load()
    const text = m.directHintMessage("phone")
    expect(text).toContain("over the terminal")
    expect(text).not.toContain("sidebar")
    expect(text).toContain("Use virtual input")
  })

  it("names the pane's own row menu on a computer", async () => {
    const m = await load()
    const text = m.directHintMessage("computer")
    expect(text).toContain("sidebar")
    expect(text).toContain("Use virtual input")
  })

  it("picks the sentence by the shell the switch happened on", async () => {
    installStorage()
    vi.resetModules()
    const m = await load()
    const wide = window.innerWidth
    Object.defineProperty(window, "innerWidth", {
      configurable: true,
      value: 390,
    })
    try {
      m.switchTypingSurface("direct", true)
    } finally {
      Object.defineProperty(window, "innerWidth", {
        configurable: true,
        value: wide,
      })
    }
    expect(notified).toEqual([m.directHintMessage("phone")])
  })

  // A KEY ROW LEFT BEHIND IS THE WAY BACK, visibly, in the bottom `⋯` that
  // hangs off it. Sending the reader to a different menu would point at the
  // wrong control, so the switch that keeps a row says nothing at all, and it
  // does not spend the device's one hint either.
  it("stays quiet while a row is left under the terminal", async () => {
    const mem = installStorage()
    vi.resetModules()
    const m = await load()
    m.switchTypingSurface("direct", false)
    expect(notified).toHaveLength(0)
    expect(mem.get(m.DIRECT_INPUT_HINT_KEY)).toBeUndefined()
    // And the hint is still owed, for the day nothing is left below.
    m.switchTypingSurface("compose", false)
    m.switchTypingSurface("direct", true)
    expect(notified).toHaveLength(1)
  })

  // The write still happens whatever the hint does: `setTypingSurface` is the
  // writer, and the toast is a side effect of the first switch only.
  it("still writes the choice", async () => {
    const mem = installStorage()
    vi.resetModules()
    const m = await load()
    m.switchTypingSurface("direct", true)
    expect(mem.get(m.TYPING_SURFACE_KEY)).toBe("direct")
  })
})
