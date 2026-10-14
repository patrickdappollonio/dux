// @vitest-environment jsdom
import { describe, expect, it } from "vitest"

import { tokenHex } from "@/lib/tokenHex"

describe("tokenHex", () => {
  it("falls back when the browser has no canvas to paint the token on", () => {
    document.documentElement.style.setProperty(
      "--dux-added",
      "oklch(72.3% 0.219 149.579)",
    )
    expect(tokenHex("--dux-added", "00C950")).toBe("00C950")
  })
})
