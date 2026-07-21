import { describe, expect, it } from "vitest"

import { agentRowVisual, statusDotColorClass } from "./agentRow"

describe("agentRowVisual", () => {
  it("shimmers an active agent that is streaming output", () => {
    expect(agentRowVisual("active", true)).toEqual({
      shimmer: true,
      dimmed: false,
      attention: false,
      typing: false,
    })
  })

  it("does not shimmer (or dim) an idle active agent", () => {
    expect(agentRowVisual("active", false)).toEqual({
      shimmer: false,
      dimmed: false,
      attention: false,
      typing: false,
    })
  })

  it("dims a detached agent and never shimmers it", () => {
    expect(agentRowVisual("detached", false)).toEqual({
      shimmer: false,
      dimmed: true,
      attention: false,
      typing: false,
    })
    // Even if a non-active agent somehow reports working, it stays dimmed and
    // unshimmered — shimmer is gated on the active status.
    expect(agentRowVisual("detached", true)).toEqual({
      shimmer: false,
      dimmed: true,
      attention: false,
      typing: false,
    })
  })

  it("dims an exited agent", () => {
    expect(agentRowVisual("exited", false)).toEqual({
      shimmer: false,
      dimmed: true,
      attention: false,
      typing: false,
    })
  })

  it("flags attention independently of shimmer and dimmed", () => {
    // A flagged agent may still be streaming its permission prompt.
    expect(agentRowVisual("active", true, true)).toEqual({
      shimmer: true,
      dimmed: false,
      attention: true,
      typing: false,
    })
    // Attention without streaming.
    expect(agentRowVisual("active", false, true)).toEqual({
      shimmer: false,
      dimmed: false,
      attention: true,
      typing: false,
    })
  })

  it("exposes typing for an active typing agent and keeps the working cue OFF", () => {
    // Typing alone: caret cue (typing=true), no bob/shimmer (shimmer=false).
    expect(agentRowVisual("active", false, false, true)).toEqual({
      shimmer: false,
      dimmed: false,
      attention: false,
      typing: true,
    })
  })

  it("suppresses the working cue while typing so the two states stay distinct", () => {
    // Both flags set: typing wins the visual, shimmer is suppressed.
    expect(agentRowVisual("active", true, false, true)).toEqual({
      shimmer: false,
      dimmed: false,
      attention: false,
      typing: true,
    })
  })

  it("keeps the working cue ON when working but not typing", () => {
    expect(agentRowVisual("active", true, false, false)).toEqual({
      shimmer: true,
      dimmed: false,
      attention: false,
      typing: false,
    })
  })

  it("never reports typing for a non-active agent", () => {
    expect(agentRowVisual("detached", false, false, true).typing).toBe(false)
    expect(agentRowVisual("exited", false, false, true).typing).toBe(false)
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
