import { describe, expect, it } from "vitest"

import {
  BUSY_TOAST_MAX_MS,
  DEFAULT_STATUS_CLEAR_SECONDS,
  statusToastDuration,
} from "./statusToast"

describe("statusToastDuration", () => {
  it("gives info/success the plain configured window", () => {
    expect(statusToastDuration("info", 6)).toBe(6000)
    expect(statusToastDuration("success", 6)).toBe(6000)
  })

  it("gives a warning twice the info window so it survives a glance away", () => {
    expect(statusToastDuration("warning", 6)).toBe(12000)
  })

  it("gives an error four times the info window, the longest final", () => {
    expect(statusToastDuration("error", 6)).toBe(24000)
  })

  it("orders the finals info < warning < error at any configured window", () => {
    for (const secs of [1, 6, 10, 30]) {
      const info = statusToastDuration("info", secs)
      const warning = statusToastDuration("warning", secs)
      const error = statusToastDuration("error", secs)
      expect(info).toBeLessThan(warning)
      expect(warning).toBeLessThan(error)
    }
  })

  it("scales every final off the user's status_clear_seconds", () => {
    expect(statusToastDuration("info", 10)).toBe(10000)
    expect(statusToastDuration("warning", 10)).toBe(20000)
    expect(statusToastDuration("error", 10)).toBe(40000)
  })

  it("falls back to the 6s default before the bootstrap document lands", () => {
    expect(DEFAULT_STATUS_CLEAR_SECONDS).toBe(6)
    expect(statusToastDuration("info", null)).toBe(6000)
    expect(statusToastDuration("info", undefined)).toBe(6000)
    expect(statusToastDuration("error", null)).toBe(24000)
  })

  it("treats status_clear_seconds of 0 as the user opting every final out of auto-clear", () => {
    expect(statusToastDuration("info", 0)).toBe(Infinity)
    expect(statusToastDuration("warning", 0)).toBe(Infinity)
    expect(statusToastDuration("error", 0)).toBe(Infinity)
  })

  it("caps a busy toast at the leak guard, never at the configured window", () => {
    // The guard is a safety net for a dropped socket, not a readability window,
    // so it is independent of status_clear_seconds and survives the 0 opt-out.
    expect(statusToastDuration("busy", 6)).toBe(BUSY_TOAST_MAX_MS)
    expect(statusToastDuration("busy", 30)).toBe(BUSY_TOAST_MAX_MS)
    expect(statusToastDuration("busy", 0)).toBe(BUSY_TOAST_MAX_MS)
  })

  it("keeps the busy guard well clear of the engine's 20s BUSY_TIMEOUT", () => {
    // dux_core::statusline::BUSY_TIMEOUT upgrades a stranded keyed Busy to a
    // Warning after 20s, and that upgrade replaces the toast in place. The
    // client guard must never fire first or a live operation would appear to
    // stop on its own.
    expect(BUSY_TOAST_MAX_MS).toBeGreaterThan(20_000 * 2)
  })

  it("never returns a zero or negative duration, which sonner reads as 'use the default'", () => {
    for (const tone of ["info", "success", "warning", "error", "busy"]) {
      for (const secs of [0, 1, 6, 60]) {
        expect(statusToastDuration(tone, secs)).toBeGreaterThan(0)
      }
    }
  })
})
