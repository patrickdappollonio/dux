import { describe, expect, it } from "vitest"

import { matchCharRanges, subsequenceMatchRanges } from "@/lib/searchMatch"

describe("matchCharRanges", () => {
  it("finds every occurrence of every needle, case-insensitively, merged", () => {
    expect(matchCharRanges("Tailscale on your tailnet", ["tail"])).toEqual([
      { start: 0, end: 4 },
      { start: 18, end: 22 },
    ])
    // Overlapping hits from two needles fold into one range.
    expect(matchCharRanges("attention", ["atten", "tion"])).toEqual([{ start: 0, end: 9 }])
  })

  it("lights nothing for an empty needle or a miss", () => {
    expect(matchCharRanges("anything", [""])).toEqual([])
    expect(matchCharRanges("anything", [])).toEqual([])
    expect(matchCharRanges("anything", ["zz"])).toEqual([])
  })

  it("counts code points, so a hit after an emoji lands on the right letters", () => {
    expect(matchCharRanges("🦆 duck", ["duck"])).toEqual([{ start: 2, end: 6 }])
  })
})

describe("subsequenceMatchRanges", () => {
  it("prefers word starts the way cmdk scores them", () => {
    // g, then the c that starts "checkout" rather than the one inside it, then its o.
    expect(subsequenceMatchRanges("git checkout", "gco")).toEqual([
      { start: 0, end: 1 },
      { start: 4, end: 5 },
      { start: 9, end: 10 },
    ])
    // The leftmost "co" sits inside "deco"; the one cmdk prefers starts a word.
    expect(subsequenceMatchRanges("deco commit", "co")).toEqual([{ start: 5, end: 7 }])
  })

  it("lights a run of adjacent letters as one range, ignoring case", () => {
    expect(subsequenceMatchRanges("Run Tests", "TES")).toEqual([{ start: 4, end: 7 }])
  })

  it("lights nothing when the query is not a subsequence of the name", () => {
    expect(subsequenceMatchRanges("git checkout", "ogc")).toEqual([])
    expect(subsequenceMatchRanges("git checkout", "")).toEqual([])
  })
})
