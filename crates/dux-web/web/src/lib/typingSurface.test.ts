// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// The device-local typing-surface choice. It is transient UI state, not
// configuration: `ui.compose_bar` stays the only configuration surface, and
// this only remembers where the user last left the toggle ON THIS DEVICE so a
// reload does not snap the surface back under them.
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
