import { describe, expect, it } from "vitest"

import { DEFAULT_INSTANCE_TITLE, resolveInstanceTitle } from "./instanceTitle"

describe("resolveInstanceTitle", () => {
  it("returns a configured title verbatim", () => {
    expect(resolveInstanceTitle("dux #1")).toBe("dux #1")
  })

  it("trims surrounding whitespace", () => {
    expect(resolveInstanceTitle("  dux (prod)  ")).toBe("dux (prod)")
  })

  it("falls back to the product name when missing", () => {
    expect(resolveInstanceTitle(undefined)).toBe(DEFAULT_INSTANCE_TITLE)
    expect(resolveInstanceTitle(null)).toBe(DEFAULT_INSTANCE_TITLE)
  })

  it("falls back when empty or whitespace only", () => {
    expect(resolveInstanceTitle("")).toBe("dux")
    expect(resolveInstanceTitle("   ")).toBe("dux")
  })

  it("collapses internal newlines so the tab and wordmark stay identical", () => {
    // A hand-edited config can contain a TOML newline escape; browsers truncate a
    // tab title at the first newline while a nowrap span would show a space.
    expect(resolveInstanceTitle("dux\nlab")).toBe("dux lab")
  })
})
