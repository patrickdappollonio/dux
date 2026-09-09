import { describe, expect, it } from "vitest"

import { formatRegularCount } from "./formatRegularCount"

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
