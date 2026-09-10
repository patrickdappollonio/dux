import { describe, expect, it } from "vitest"

import { formatCount, formatRegularCount } from "./formatRegularCount"

describe("formatRegularCount", () => {
  it("pluralizes a zero count", () => {
    expect(formatRegularCount(0, "file")).toBe("0 files")
  })

  it("keeps a count of one singular", () => {
    expect(formatRegularCount(1, "file")).toBe("1 file")
  })

  it("pluralizes a count above one", () => {
    expect(formatRegularCount(4, "agent")).toBe("4 agents")
  })
})

describe("formatCount", () => {
  it("keeps a count of one singular", () => {
    expect(formatCount(1, "process", "processes")).toBe("1 process")
  })

  it("uses the spelled-out plural everywhere else", () => {
    expect(formatCount(0, "process", "processes")).toBe("0 processes")
    expect(formatCount(2, "process", "processes")).toBe("2 processes")
  })
})
