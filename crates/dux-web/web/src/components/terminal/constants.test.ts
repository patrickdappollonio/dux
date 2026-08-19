// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"

import { xtermScrollbarWidth } from "./constants"

// The one behavior worth pinning here is the fallback boundary: 8 is for a
// variable that is MISSING or unparsable, never for one that says 0. The old
// `|| 8` read an explicit 0 (a deliberately hidden scrollbar) as unset and
// re-reserved a gutter nothing was drawn in.
describe("xtermScrollbarWidth", () => {
  const stubVar = (value: string) => {
    vi.stubGlobal(
      "getComputedStyle",
      vi.fn(() => ({ getPropertyValue: () => value })),
    )
  }
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it("honors an explicit 0 instead of falling back to 8", () => {
    stubVar("0")
    expect(xtermScrollbarWidth()).toBe(0)
  })

  it("reads an ordinary pixel value", () => {
    stubVar("12px")
    expect(xtermScrollbarWidth()).toBe(12)
  })

  it("falls back to 8 only when the variable is missing or unparsable", () => {
    stubVar("")
    expect(xtermScrollbarWidth()).toBe(8)
    stubVar("thin")
    expect(xtermScrollbarWidth()).toBe(8)
  })
})
