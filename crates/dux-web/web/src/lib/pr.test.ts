import { describe, expect, it } from "vitest"

import {
  prAriaLabel,
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
    expect(prIconClass("open")).toContain("text-dux-pr-open")
    expect(prIconClass("merged")).toContain("text-dux-pr-merged")
    expect(prIconClass("closed")).toContain("text-dux-pr-closed")
  })

  it("uses a neutral tint for unknown states", () => {
    expect(prIconClass("draft")).toContain("text-muted-foreground")
  })
})

describe("prIconHoverClass", () => {
  it("provides an explicit same-hue hover for every known state", () => {
    expect(prIconHoverClass("open")).toContain("hover:text-dux-pr-open-hover")
    expect(prIconHoverClass("open")).toContain("hover:bg-dux-pr-open-tint/15")
    expect(prIconHoverClass("merged")).toContain("hover:text-dux-pr-merged-hover")
    expect(prIconHoverClass("closed")).toContain("hover:text-dux-pr-closed-hover")
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
    expect(prBannerClass("open")).toContain("bg-dux-pr-open-tint/10")
    expect(prBannerClass("open")).toContain("border-dux-pr-open-tint/30")
    expect(prBannerClass("merged")).toContain("bg-dux-pr-merged-tint/10")
    expect(prBannerClass("closed")).toContain("bg-dux-pr-closed-tint/10")
  })

  it("falls back to a neutral strip for unknown states", () => {
    expect(prBannerClass("draft")).toContain("bg-muted")
  })
})

describe("prAriaLabel", () => {
  it("spells the words out and speaks the state as a clause", () => {
    expect(prAriaLabel(123, "open")).toBe("Pull request #123, open")
    expect(prAriaLabel(7, "merged")).toBe("Pull request #7, merged")
    expect(prAriaLabel(7, "closed")).toBe("Pull request #7, closed")
  })

  it("says nothing about a state it does not know", () => {
    // "Pull request #7, unknown" would be worse than silence: it announces a
    // word the listener has to decide is meaningless.
    expect(prAriaLabel(7, "draft")).toBe("Pull request #7")
  })
})
