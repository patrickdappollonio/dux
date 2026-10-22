// The "needs attention" pulse both attention dots share: the sidebar row's
// `AttentionDot` plays it as the `attention-pulse` CSS animation and the favicon
// plays the same period and two-dip shape by swapping pre-rendered frames, with
// longer dips of its own (`FAVICON_RHYTHM_FLIPS_MS`). The stylesheet's
// `--animate-attention-pulse` and `@keyframes attention-pulse` are pinned to the
// constants below by `attentionPulse.test.ts`, so the two cannot drift.

/** One full cycle of the pulse: two quick dips, then a hold. */
export const ATTENTION_PULSE_PERIOD_MS = 2000

/** The opacity at the bottom of each dip. */
export const ATTENTION_PULSE_FLOOR = 0.25

/** The pulse's keyframes, as percentages of the period, in order. */
export const ATTENTION_PULSE_KEYFRAMES: ReadonlyArray<{
  percent: number
  opacity: number
}> = [
  { percent: 0, opacity: 1 },
  { percent: 9, opacity: ATTENTION_PULSE_FLOOR },
  { percent: 18, opacity: 1 },
  { percent: 27, opacity: ATTENTION_PULSE_FLOOR },
  { percent: 36, opacity: 1 },
  { percent: 100, opacity: 1 },
]

export type AttentionFrame = "on" | "dim"

/**
 * `rhythm` is the row dot's shape, two dips and a hold (with the favicon's
 * longer dips), for a page whose timers run on time. `steady` is an even on/off
 * blink over the same period, for a hidden page whose timers the browser only
 * wakes about once a second: sampled at that rate
 * the rhythm's dips (a few hundred milliseconds each) would almost never be
 * seen, while a half-period square wave still alternates on every wake.
 */
export type AttentionPulseMode = "rhythm" | "steady"

// The favicon's frame flips in one `rhythm` period, in ms from its start. The
// row dot's dips last 180 ms, and Chrome does not paint a favicon change shorter
// than about 200 ms, so the favicon starts its first dip where the row dot's does
// and lengthens each step to 300 ms, which every dip survives.
const FAVICON_RHYTHM_FLIPS_MS: readonly number[] = [90, 390, 690, 990]

const STEADY_FLIPS_MS: readonly number[] = [ATTENTION_PULSE_PERIOD_MS / 2]

/**
 * The frame to show `elapsedMs` after the pulse started, and how long until it
 * next changes. Every period starts on the `on` frame.
 */
export function attentionFrameAt(
  elapsedMs: number,
  mode: AttentionPulseMode,
): { frame: AttentionFrame; msUntilChange: number } {
  const flips = mode === "rhythm" ? FAVICON_RHYTHM_FLIPS_MS : STEADY_FLIPS_MS
  const phase =
    ((elapsedMs % ATTENTION_PULSE_PERIOD_MS) + ATTENTION_PULSE_PERIOD_MS) %
    ATTENTION_PULSE_PERIOD_MS
  let passed = 0
  while (passed < flips.length && flips[passed] <= phase) passed += 1
  const next = passed < flips.length ? flips[passed] : ATTENTION_PULSE_PERIOD_MS
  return {
    frame: passed % 2 === 0 ? "on" : "dim",
    msUntilChange: next - phase,
  }
}
