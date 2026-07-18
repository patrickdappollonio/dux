import { describe, expect, it } from "vitest"

import { relativeTime } from "@/lib/relativeTime"

const NOW = Date.parse("2026-07-17T12:00:00Z")
const ago = (ms: number) => new Date(NOW - ms).toISOString()

describe("relativeTime", () => {
  it("returns 'now' for a sub-45-second delta", () => {
    expect(relativeTime(ago(0), NOW)).toBe("now")
    expect(relativeTime(ago(44_000), NOW)).toBe("now")
  })

  it("returns 'now' for a future timestamp rather than a negative value", () => {
    expect(relativeTime(new Date(NOW + 60_000).toISOString(), NOW)).toBe("now")
  })

  it("renders whole minutes below an hour", () => {
    expect(relativeTime(ago(60_000), NOW)).toBe("1m")
    expect(relativeTime(ago(59 * 60_000), NOW)).toBe("59m")
  })

  it("renders whole hours below a day", () => {
    expect(relativeTime(ago(60 * 60_000), NOW)).toBe("1h")
    expect(relativeTime(ago(23 * 3_600_000), NOW)).toBe("23h")
  })

  it("renders whole days below a week", () => {
    expect(relativeTime(ago(24 * 3_600_000), NOW)).toBe("1d")
    expect(relativeTime(ago(6 * 86_400_000), NOW)).toBe("6d")
  })

  it("renders weeks as the largest unit", () => {
    expect(relativeTime(ago(7 * 86_400_000), NOW)).toBe("1w")
    expect(relativeTime(ago(21 * 86_400_000), NOW)).toBe("3w")
  })

  it("returns an empty string for an unparseable timestamp", () => {
    expect(relativeTime("not-a-date", NOW)).toBe("")
    expect(relativeTime("", NOW)).toBe("")
  })
})
