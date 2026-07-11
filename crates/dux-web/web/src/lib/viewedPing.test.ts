import { describe, expect, it } from "vitest"
import { VIEWED_PING_INTERVAL_MS, shouldSendViewed } from "./viewedPing"

describe("shouldSendViewed", () => {
  it("pings only when this device is the owner AND visible", () => {
    expect(shouldSendViewed({ isOwner: true, visible: true })).toBe(true)
    // A read-only observer must not suppress attention for everyone.
    expect(shouldSendViewed({ isOwner: false, visible: true })).toBe(false)
    // A backgrounded owner's open socket must not read as "watching".
    expect(shouldSendViewed({ isOwner: true, visible: false })).toBe(false)
    expect(shouldSendViewed({ isOwner: false, visible: false })).toBe(false)
  })

  it("pings comfortably faster than the 3s engagement window", () => {
    expect(VIEWED_PING_INTERVAL_MS).toBeLessThan(3000)
  })
})
