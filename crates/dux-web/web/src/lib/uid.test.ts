import { afterEach, describe, expect, it, vi } from "vitest"
import { newClientId } from "./uid"

// dux frequently runs over plain HTTP on a LAN/Tailscale address, where
// `crypto.randomUUID` is undefined (secure-context-only) but `crypto` itself
// and `crypto.getRandomValues` are still present. `newClientId` must not
// throw there. See lib/sw.ts's isSecureContext gating for the same
// deployment reality this guards against.
describe("newClientId", () => {
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it("uses crypto.randomUUID when available", () => {
    const id = newClientId()
    expect(id).toMatch(/^[0-9a-f-]{36}$/)
  })

  it("falls back to crypto.getRandomValues when randomUUID is undefined (insecure context)", () => {
    const original = crypto.randomUUID
    // Simulate a plain-HTTP context: randomUUID is gone, getRandomValues stays.
    // @ts-expect-error -- deliberately deleting a required method for the test
    delete crypto.randomUUID
    try {
      const id = newClientId()
      expect(id).toMatch(
        /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
      )
      // Distinct calls must not collide.
      expect(newClientId()).not.toBe(id)
    } finally {
      crypto.randomUUID = original
    }
  })

  it("falls back to Math.random when crypto is entirely unavailable", () => {
    vi.stubGlobal("crypto", undefined)
    const id = newClientId()
    expect(typeof id).toBe("string")
    expect(id.length).toBeGreaterThan(0)
  })
})
