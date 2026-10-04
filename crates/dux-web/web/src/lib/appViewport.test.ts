import { describe, expect, it } from "vitest"

import { SHELL_HEIGHT_CLASS, SHELL_SAFE_TOP, shellHeight } from "./appViewport"

// The shells give up the height the auth banners take, so a banner pushes the
// app down instead of covering its header.
describe("shell height", () => {
  it("is the viewport minus the banner stack", () => {
    expect(shellHeight(null)).toBe("calc(100svh - var(--dux-app-top, 0px))")
  })

  it("is the measured visual viewport minus the banner stack while the keyboard is up", () => {
    expect(shellHeight(512)).toBe("calc(512px - var(--dux-app-top, 0px))")
  })

  it("has a class twin for the desktop shell", () => {
    expect(SHELL_HEIGHT_CLASS).toBe("h-[calc(100svh-var(--dux-app-top,0px))]")
  })

  it("lets the banner stack take over the notch inset", () => {
    expect(SHELL_SAFE_TOP).toBe("var(--dux-app-safe-top, env(safe-area-inset-top))")
  })
})
