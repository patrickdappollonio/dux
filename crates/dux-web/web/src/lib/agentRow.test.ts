import { describe, expect, it } from "vitest"

import { agentRowVisual } from "./agentRow"

describe("agentRowVisual", () => {
  it("shimmers an active agent that is streaming output", () => {
    expect(agentRowVisual("active", true)).toEqual({
      shimmer: true,
      dimmed: false,
    })
  })

  it("does not shimmer (or dim) an idle active agent", () => {
    expect(agentRowVisual("active", false)).toEqual({
      shimmer: false,
      dimmed: false,
    })
  })

  it("dims a detached agent and never shimmers it", () => {
    expect(agentRowVisual("detached", false)).toEqual({
      shimmer: false,
      dimmed: true,
    })
    // Even if a non-active agent somehow reports working, it stays dimmed and
    // unshimmered — shimmer is gated on the active status.
    expect(agentRowVisual("detached", true)).toEqual({
      shimmer: false,
      dimmed: true,
    })
  })

  it("dims an exited agent", () => {
    expect(agentRowVisual("exited", false)).toEqual({
      shimmer: false,
      dimmed: true,
    })
  })
})
