import { describe, expect, it } from "vitest"

import type { SessionStatus, SessionView } from "@/lib/types"

import { agentRowVisual, statusDotColorClass } from "./agentRow"
import { stateWord } from "./flatList"

describe("agentRowVisual", () => {
  it("marks an active agent that is streaming output as working", () => {
    expect(agentRowVisual("active", true)).toEqual({
      working: true,
      dimmed: false,
      attention: false,
      typing: false,
    })
  })

  it("neither works nor dims an idle active agent", () => {
    expect(agentRowVisual("active", false)).toEqual({
      working: false,
      dimmed: false,
      attention: false,
      typing: false,
    })
  })

  it("dims a detached agent and never marks it working", () => {
    expect(agentRowVisual("detached", false)).toEqual({
      working: false,
      dimmed: true,
      attention: false,
      typing: false,
    })
    // Even if a non-active agent somehow reports working, it stays dimmed and
    // not working; the working cue is gated on the active status.
    expect(agentRowVisual("detached", true)).toEqual({
      working: false,
      dimmed: true,
      attention: false,
      typing: false,
    })
  })

  it("dims an exited agent", () => {
    expect(agentRowVisual("exited", false)).toEqual({
      working: false,
      dimmed: true,
      attention: false,
      typing: false,
    })
  })

  it("turns the working cue OFF under attention, which outranks it", () => {
    // A flagged agent is usually still streaming its permission prompt, and the
    // word it shows is "Needs you". The cue must agree with that word: pulsing
    // the glyph here would also nest an opacity animation inside the attention
    // blink and multiply the two into a much deeper dip.
    expect(agentRowVisual("active", true, true)).toEqual({
      working: false,
      dimmed: false,
      attention: true,
      typing: false,
    })
    // Attention without streaming.
    expect(agentRowVisual("active", false, true)).toEqual({
      working: false,
      dimmed: false,
      attention: true,
      typing: false,
    })
  })

  it("exposes typing for an active typing agent and keeps the working cue OFF", () => {
    // Typing alone: caret cue (typing=true), no pulse (working=false).
    expect(agentRowVisual("active", false, false, true)).toEqual({
      working: false,
      dimmed: false,
      attention: false,
      typing: true,
    })
  })

  it("suppresses the working cue while typing so the two states stay distinct", () => {
    // Both flags set: typing wins the visual, the working cue is suppressed.
    expect(agentRowVisual("active", true, false, true)).toEqual({
      working: false,
      dimmed: false,
      attention: false,
      typing: true,
    })
  })

  it("keeps the working cue ON when working but not typing", () => {
    expect(agentRowVisual("active", true, false, false)).toEqual({
      working: true,
      dimmed: false,
      attention: false,
      typing: false,
    })
  })

  it("never reports typing for a non-active agent", () => {
    expect(agentRowVisual("detached", false, false, true).typing).toBe(false)
    expect(agentRowVisual("exited", false, false, true).typing).toBe(false)
  })

  // The cue and the word are two readings of the same row, and a row that
  // pulses while saying something other than "Working" is the bug this pins.
  // Walked over the whole input space rather than sampled, because the ladder
  // has four inputs and the disagreement lived in one corner of it.
  it("fires exactly when the row's state word is the busy one", () => {
    const statuses: SessionStatus[] = ["active", "detached", "exited"]
    for (const status of statuses) {
      for (const working of [false, true]) {
        for (const needsAttention of [false, true]) {
          for (const typing of [false, true]) {
            const visual = agentRowVisual(status, working, needsAttention, typing)
            const word = stateWord({
              status,
              working,
              needs_attention: needsAttention,
              typing,
            } as SessionView)
            expect(visual.working).toBe(word.label === "Working")
          }
        }
      }
    }
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
