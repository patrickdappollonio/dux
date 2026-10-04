// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, renderHook } from "@testing-library/react"

// The Changes pane's gesture tracking across a sign-out. A drag the sign-in
// gate interrupts can lose its release (it lands on the gate, or never comes),
// and the width it was carrying must not be saved by the next release after
// the user signs back in.

const persistChangesPanePercent = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    changesPaneMountPercent: () => 30,
    persistChangesPanePercent: (p: number) => persistChangesPanePercent(p),
    setChangesPanePercent: () => {},
  }
})

let signedIn = true

beforeEach(() => {
  signedIn = true
  persistChangesPanePercent.mockClear()
  const mem = new Map<string, string>()
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => mem.get(k) ?? null,
    setItem: (k: string, v: string) => void mem.set(k, String(v)),
    removeItem: (k: string) => void mem.delete(k),
  })
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
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("a Changes pane drag the gate interrupts", () => {
  it("is forgotten, so a release after sign-in saves nothing", async () => {
    const gate = await import("@/lib/authGate")
    await gate.initAuthGate()
    const { useChangesPaneController } = await import("./use-changes-pane-controller")
    const { result } = renderHook(() => useChangesPaneController(true))

    // A drag on the divider is under way.
    window.dispatchEvent(new Event("pointerdown"))
    act(() => {
      result.current.onLayoutChange({ "terminal-pane": 60, "changes-pane": 40 })
    })

    // The session ends before the release arrives.
    signedIn = false
    gate.reportUnauthorized()
    signedIn = true
    await gate.signIn("pw")

    // The next release anywhere belongs to no drag.
    window.dispatchEvent(new Event("pointerup"))
    expect(persistChangesPanePercent).not.toHaveBeenCalled()
  })

  it("still saves a drag that ends normally", async () => {
    const gate = await import("@/lib/authGate")
    await gate.initAuthGate()
    const { useChangesPaneController } = await import("./use-changes-pane-controller")
    const { result } = renderHook(() => useChangesPaneController(true))
    window.dispatchEvent(new Event("pointerdown"))
    act(() => {
      result.current.onLayoutChange({ "terminal-pane": 60, "changes-pane": 40 })
    })
    window.dispatchEvent(new Event("pointerup"))
    expect(persistChangesPanePercent).toHaveBeenCalledWith(40)
  })
})
