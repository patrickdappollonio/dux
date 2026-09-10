import { describe, expect, it } from "vitest"
import { diffHeadBanner } from "./diffHeadBanner"
import type { FileDiffHead } from "./fileApi"

function head(overrides: Partial<FileDiffHead> = {}): FileDiffHead {
  return {
    text: "",
    shown_lines: 4000,
    total_lines: 12345,
    truncated: true,
    total_is_at_least: false,
    binary: false,
    ...overrides,
  }
}

describe("diffHeadBanner", () => {
  it("names the shown and total line counts", () => {
    expect(diffHeadBanner(head())).toBe(
      "Diff cut here: showing the first 4000 of 12345 lines. Open the file in " +
        "your editor or run git diff to see the rest.",
    )
  })

  it("says nothing when nothing was cut", () => {
    expect(diffHeadBanner(head({ truncated: false }))).toBe("")
  })

  it("says more than when the count stopped short", () => {
    expect(diffHeadBanner(head({ total_is_at_least: true }))).toContain(
      "of more than 12345 lines.",
    )
  })

  it("counts one line as one line", () => {
    expect(
      diffHeadBanner(head({ shown_lines: 0, total_lines: 1 })),
    ).toContain("of 1 line.")
  })
})
