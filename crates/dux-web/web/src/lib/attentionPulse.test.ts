import { readFileSync } from "node:fs"

import { describe, expect, it } from "vitest"

import {
  ATTENTION_PULSE_FLOOR,
  ATTENTION_PULSE_KEYFRAMES,
  ATTENTION_PULSE_PERIOD_MS,
  attentionFrameAt,
  type AttentionFrame,
} from "./attentionPulse"

// Walk one mode's schedule from 0 to `until` ms by following each answer's
// `msUntilChange`, returning the [start, frame] runs it produced.
function runs(mode: "rhythm" | "steady", until: number): Array<[number, AttentionFrame]> {
  const out: Array<[number, AttentionFrame]> = []
  let t = 0
  while (t < until) {
    const { frame, msUntilChange } = attentionFrameAt(t, mode)
    expect(msUntilChange).toBeGreaterThan(0)
    if (out.length === 0 || out[out.length - 1][1] !== frame) out.push([t, frame])
    t += msUntilChange
  }
  return out
}

describe("the shared attention pulse", () => {
  // The row dot is a CSS animation and the favicon is driven from TS, so the
  // stylesheet is pinned to the TS constants: every expected value below is
  // DERIVED from them, so moving the rhythm in TS moves what this demands of
  // the stylesheet instead of quietly passing.
  it("keeps the stylesheet's row-dot animation equal to the shared constants", () => {
    const css = readFileSync(`${process.cwd()}/src/index.css`, "utf8")
    expect(css).toContain(
      `--animate-attention-pulse: attention-pulse ${ATTENTION_PULSE_PERIOD_MS / 1000}s ease-in-out infinite;`,
    )

    const body = /@keyframes attention-pulse\s*\{([\s\S]*?)\n\}/.exec(css)
    expect(body).toBeTruthy()
    const parsed = new Map<number, number>()
    for (const [, stops, opacity] of body![1].matchAll(
      /([\d%,\s]+)\{\s*opacity:\s*([\d.]+);\s*\}/g,
    )) {
      for (const stop of stops.split(",")) {
        parsed.set(Number(stop.trim().replace("%", "")), Number(opacity))
      }
    }
    const expected = new Map(
      ATTENTION_PULSE_KEYFRAMES.map((k) => [k.percent, k.opacity] as const),
    )
    expect([...parsed.entries()].sort((a, b) => a[0] - b[0])).toEqual(
      [...expected.entries()].sort((a, b) => a[0] - b[0]),
    )
    // The dips go down to the floor and nowhere else.
    expect(Math.min(...expected.values())).toBe(ATTENTION_PULSE_FLOOR)
  })

  it("plays the row dot's two quick dips then a hold, flipping where the dip crosses halfway", () => {
    const p = ATTENTION_PULSE_PERIOD_MS
    // Each flip sits at the midpoint of a keyframe segment whose opacity
    // changes, which is where an ease-in-out tween crosses halfway.
    const flips: number[] = []
    for (let i = 1; i < ATTENTION_PULSE_KEYFRAMES.length; i += 1) {
      const a = ATTENTION_PULSE_KEYFRAMES[i - 1]
      const b = ATTENTION_PULSE_KEYFRAMES[i]
      if (a.opacity !== b.opacity) flips.push(((a.percent + b.percent) / 200) * p)
    }
    expect(flips).toHaveLength(4)
    expect(runs("rhythm", p)).toEqual([
      [0, "on"],
      [flips[0], "dim"],
      [flips[1], "on"],
      [flips[2], "dim"],
      [flips[3], "on"],
    ])
    // Two dips, and the hold after them is the long majority of the period.
    expect(p - flips[3]).toBeGreaterThan(p / 2)
  })

  it("repeats on the same period, whatever point in a cycle it is asked about", () => {
    const p = ATTENTION_PULSE_PERIOD_MS
    for (const t of [0, 1, 130, 500, 999, 1500]) {
      expect(attentionFrameAt(t + 3 * p, "rhythm")).toEqual(attentionFrameAt(t, "rhythm"))
      expect(attentionFrameAt(t + 3 * p, "steady")).toEqual(attentionFrameAt(t, "steady"))
    }
  })

  it("degrades to an even on/off blink over the same period for a throttled tab", () => {
    const p = ATTENTION_PULSE_PERIOD_MS
    expect(runs("steady", 2 * p)).toEqual([
      [0, "on"],
      [p / 2, "dim"],
      [p, "on"],
      [1.5 * p, "dim"],
    ])
    // A timer that is only allowed to wake once a second still lands on
    // alternating frames, which is the point of the steady mode.
    expect(p / 2).toBeGreaterThanOrEqual(1000)
    const sampled = [0, 1000, 2000, 3000].map((t) => attentionFrameAt(t, "steady").frame)
    expect(sampled).toEqual(["on", "dim", "on", "dim"])
  })
})
