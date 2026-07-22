import { readFileSync } from "node:fs"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

// Pins the shipped viewport meta in index.html. It carries two load-bearing,
// easy-to-"clean-up" directives whose loss only shows on a real phone:
//  - interactive-widget=resizes-content: the soft keyboard shrinks the LAYOUT
//    viewport so the mobile shell's accessory/compose rows sit on the keyboard
//    (see the comment block in index.html).
//  - maximum-scale=1: the compose textarea is 14px type, and any input font
//    under 16px trips iOS Safari's auto-zoom-on-focus. maximum-scale=1 is the
//    standard, accessibility-preserving fix: Chromium/Android ignores it
//    entirely, and since iOS 10 Safari ignores it for USER pinch-zoom (so
//    accessibility zoom keeps working) while it still disables the
//    focus-triggered auto-zoom.
const html = readFileSync(
  fileURLToPath(new URL("../../index.html", import.meta.url)),
  "utf8",
)

describe("index.html viewport meta", () => {
  const meta = html
    .split("\n")
    .find((l) => l.includes('name="viewport"'))

  it("ships exactly one viewport meta line", () => {
    expect(meta).toBeTruthy()
  })

  it("keeps the keyboard-resize directive", () => {
    expect(meta).toContain("interactive-widget=resizes-content")
  })

  it("disables the iOS input-focus auto-zoom via maximum-scale=1", () => {
    expect(meta).toContain("maximum-scale=1")
  })
})
