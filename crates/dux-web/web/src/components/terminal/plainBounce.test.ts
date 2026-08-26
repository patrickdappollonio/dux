import { describe, expect, it } from "vitest"

import type { PtySocket } from "@/lib/ptySocket"

import type { TakeoverIntent } from "./channels"
import { plainBounce } from "./plainBounce"

function intent(): TakeoverIntent & { armedWith: () => string | undefined } {
  let armed = false
  let expected: string | undefined = undefined
  return {
    read: () => armed,
    expectedOwner: () => expected,
    arm: (expectedOwner?: string) => {
      armed = true
      expected = expectedOwner
    },
    clear: () => {
      armed = false
      expected = undefined
    },
    armedWith: () => expected,
  }
}

describe("a plain bounce", () => {
  it("reopens the socket", () => {
    let connects = 0
    const pty = { connect: () => connects++ } as unknown as PtySocket
    plainBounce(pty, intent())
    expect(connects).toBe(1)
  })

  // A press take-over names nobody, so a surviving flag would be granted
  // unconditionally: the reopen would take the pty from whoever holds it.
  it("spends a pressed take-over before reopening", () => {
    const takeover = intent()
    takeover.arm()
    const pty = { connect: () => {} } as unknown as PtySocket
    plainBounce(pty, takeover)
    expect(takeover.read()).toBe(false)
  })

  // A self-succession names the ghost it expects to displace. Carried into a
  // later bounce the name is stale, the server refuses the transfer, and the
  // pane is left at a geometry the pty never applied.
  it("spends a self-succession, expected owner and all", () => {
    const takeover = intent()
    takeover.arm("conn-7")
    const pty = { connect: () => {} } as unknown as PtySocket
    plainBounce(pty, takeover)
    expect(takeover.read()).toBe(false)
    expect(takeover.armedWith()).toBeUndefined()
  })

  it("still spends the intent when there is no socket to reopen", () => {
    const takeover = intent()
    takeover.arm()
    plainBounce(null, takeover)
    expect(takeover.read()).toBe(false)
  })
})
