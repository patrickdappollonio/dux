import { describe, expect, it } from "vitest"

import { agentRowVisual, statusDotColorClass } from "./agentRow"

describe("agentRowVisual", () => {
  it("shimmers an active agent that is streaming output", () => {
    expect(agentRowVisual("active", true)).toEqual({
      shimmer: true,
      dimmed: false,
      attention: false,
    })
  })

  it("does not shimmer (or dim) an idle active agent", () => {
    expect(agentRowVisual("active", false)).toEqual({
      shimmer: false,
      dimmed: false,
      attention: false,
    })
  })

  it("dims a detached agent and never shimmers it", () => {
    expect(agentRowVisual("detached", false)).toEqual({
      shimmer: false,
      dimmed: true,
      attention: false,
    })
    // Even if a non-active agent somehow reports working, it stays dimmed and
    // unshimmered — shimmer is gated on the active status.
    expect(agentRowVisual("detached", true)).toEqual({
      shimmer: false,
      dimmed: true,
      attention: false,
    })
  })

  it("dims an exited agent", () => {
    expect(agentRowVisual("exited", false)).toEqual({
      shimmer: false,
      dimmed: true,
      attention: false,
    })
  })

  it("flags attention independently of shimmer and dimmed", () => {
    // A flagged agent may still be streaming its permission prompt.
    expect(agentRowVisual("active", true, true)).toEqual({
      shimmer: true,
      dimmed: false,
      attention: true,
    })
    // Attention without streaming.
    expect(agentRowVisual("active", false, true)).toEqual({
      shimmer: false,
      dimmed: false,
      attention: true,
    })
  })
})

describe("statusDotColorClass", () => {
  it("uses the cyan-frost attention tint regardless of status when flagged", () => {
    expect(statusDotColorClass("active", true)).toBe("text-cyan-100")
    expect(statusDotColorClass("detached", true)).toBe("text-cyan-100")
    expect(statusDotColorClass("exited", true)).toBe("text-cyan-100")
  })

  it("falls back to the per-status color when not flagged for attention", () => {
    expect(statusDotColorClass("active", false)).toBe("text-green-500")
    expect(statusDotColorClass("detached", false)).toBe("text-amber-500")
    expect(statusDotColorClass("exited", false)).toBe("text-muted-foreground")
  })
})
