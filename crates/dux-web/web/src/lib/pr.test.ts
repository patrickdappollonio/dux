import { describe, expect, it } from "vitest"

import {
  prBannerClass,
  prIconClass,
  prIconHoverClass,
  prStateLabel,
} from "@/lib/pr"

describe("prStateLabel", () => {
  it("maps known states to their TUI lowercase words", () => {
    expect(prStateLabel("open")).toBe("open")
    expect(prStateLabel("merged")).toBe("merged")
    expect(prStateLabel("closed")).toBe("closed")
  })

  it("falls back to 'unknown' for any unrecognized state", () => {
    expect(prStateLabel("draft")).toBe("unknown")
    expect(prStateLabel("")).toBe("unknown")
  })
})

describe("prIconClass", () => {
  it("tints the icon green/purple/red per state", () => {
    expect(prIconClass("open")).toContain("text-green-500")
    expect(prIconClass("merged")).toContain("text-purple-400")
    expect(prIconClass("closed")).toContain("text-red-400")
  })

  it("uses a neutral tint for unknown states", () => {
    expect(prIconClass("draft")).toContain("text-muted-foreground")
  })
})

describe("prIconHoverClass", () => {
  it("provides an explicit same-hue hover for every known state", () => {
    expect(prIconHoverClass("open")).toContain("hover:text-green-400")
    expect(prIconHoverClass("open")).toContain("hover:bg-green-600/15")
    expect(prIconHoverClass("merged")).toContain("hover:text-purple-300")
    expect(prIconHoverClass("closed")).toContain("hover:text-red-300")
  })

  it("never emits a near-white hover surface that would wash the icon out", () => {
    for (const state of ["open", "merged", "closed", "draft"]) {
      const cls = prIconHoverClass(state)
      expect(cls).not.toContain("bg-white")
      expect(cls).not.toContain("bg-background")
    }
  })
})

describe("prBannerClass", () => {
  it("produces a soft state-colored strip per state", () => {
    expect(prBannerClass("open")).toContain("bg-green-600/10")
    expect(prBannerClass("open")).toContain("border-green-600/30")
    expect(prBannerClass("merged")).toContain("bg-purple-600/10")
    expect(prBannerClass("closed")).toContain("bg-red-600/10")
  })

  it("falls back to a neutral strip for unknown states", () => {
    expect(prBannerClass("draft")).toContain("bg-muted")
  })
})
