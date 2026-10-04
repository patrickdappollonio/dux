// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// The sign-in gate's layer as the hidden app's own listeners see it. The app
// stays mounted under the gate, and a few of its listeners sit on the document
// in the CAPTURE phase, where they see an event before the gate does. Each one
// asks `isInGateLayer` (through `outsideGate`) and leaves the gate's events
// alone; and gesture state a release on the gate would leave half-done is
// reset the moment the gate goes up (`onGateUp`).

type Mod = typeof import("./gateLayer")

let host: HTMLDivElement
let field: HTMLInputElement
let outside: HTMLDivElement
let signedIn = true

beforeEach(() => {
  host = document.createElement("div")
  host.setAttribute("data-auth-gate-layer", "")
  field = document.createElement("input")
  host.appendChild(field)
  document.body.appendChild(host)
  outside = document.createElement("div")
  document.body.appendChild(outside)
  signedIn = true
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string) =>
      String(input).endsWith("/auth/login")
        ? new Response(null, { status: 204 })
        : new Response(
            JSON.stringify({ password_set: true, required_here: true, signed_in: signedIn }),
          ),
    ),
  )
  vi.resetModules()
})

afterEach(() => {
  host.remove()
  outside.remove()
  vi.unstubAllGlobals()
})

async function load(): Promise<Mod> {
  return import("./gateLayer")
}

describe("isInGateLayer", () => {
  it("knows the gate's own elements and nothing else", async () => {
    const m = await load()
    expect(m.isInGateLayer(field)).toBe(true)
    expect(m.isInGateLayer(outside)).toBe(false)
    expect(m.isInGateLayer(null)).toBe(false)
  })
})

describe("outsideGate", () => {
  it("runs the listener for the app's events and skips the gate's", async () => {
    const m = await load()
    const seen = vi.fn()
    const listener = m.outsideGate(seen)
    document.addEventListener("compositionstart", listener, true)
    try {
      field.dispatchEvent(new Event("compositionstart", { bubbles: true }))
      expect(seen).not.toHaveBeenCalled()
      outside.dispatchEvent(new Event("compositionstart", { bubbles: true }))
      expect(seen).toHaveBeenCalledTimes(1)
    } finally {
      document.removeEventListener("compositionstart", listener, true)
    }
  })

  it("does not stop the event: the gate's own handlers still get it", async () => {
    const m = await load()
    const listener = m.outsideGate(() => {})
    const onField = vi.fn()
    field.addEventListener("pointerdown", onField)
    document.addEventListener("pointerdown", listener, true)
    try {
      field.dispatchEvent(new Event("pointerdown", { bubbles: true }))
      expect(onField).toHaveBeenCalledTimes(1)
    } finally {
      document.removeEventListener("pointerdown", listener, true)
    }
  })
})

describe("onGateUp", () => {
  it("runs the reset when the page goes to a gate page, and not when it comes back", async () => {
    const m = await load()
    const gate = await import("./authGate")
    await gate.initAuthGate()
    const reset = vi.fn()
    const stop = m.onGateUp(reset)
    signedIn = false
    gate.reportUnauthorized()
    expect(reset).toHaveBeenCalledTimes(1)
    signedIn = true
    await gate.signIn("pw")
    expect(reset).toHaveBeenCalledTimes(1)
    stop()
  })

  it("stops when asked", async () => {
    const m = await load()
    const gate = await import("./authGate")
    await gate.initAuthGate()
    const reset = vi.fn()
    m.onGateUp(reset)()
    signedIn = false
    gate.reportUnauthorized()
    expect(reset).not.toHaveBeenCalled()
  })
})
