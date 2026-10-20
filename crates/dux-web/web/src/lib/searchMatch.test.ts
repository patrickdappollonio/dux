import { describe, expect, it } from "vitest"

import { cmdkMatchRanges, matchCharRanges } from "@/lib/searchMatch"

describe("matchCharRanges", () => {
  it("finds every occurrence of every needle, case-insensitively, merged", () => {
    expect(matchCharRanges("Tailscale on your tailnet", ["tail"])).toEqual([
      { start: 0, end: 4 },
      { start: 18, end: 22 },
    ])
    // Overlapping hits from two needles fold into one range.
    expect(matchCharRanges("attention", ["atten", "tion"])).toEqual([{ start: 0, end: 9 }])
  })

  it("lights overlapping occurrences of one needle", () => {
    expect(matchCharRanges("banana", ["ana"])).toEqual([{ start: 1, end: 6 }])
  })

  it("folds case the way the filter's whole-string lowercase does", () => {
    // A final capital sigma lowers to "ς" only in context.
    expect(matchCharRanges("ΟΔΟΣ", ["ΟΔΟΣ".toLowerCase()])).toEqual([{ start: 0, end: 4 }])
    // "İ" lowers to two code points; the hit after it still lands on "stan".
    expect(matchCharRanges("İstanbul", ["stan"])).toEqual([{ start: 1, end: 5 }])
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

describe("cmdkMatchRanges", () => {
  it("prefers word starts the way cmdk scores them", () => {
    // g, then the c that starts "checkout" rather than the one inside it, then its o.
    expect(cmdkMatchRanges("git checkout", "gco")).toEqual([
      { start: 0, end: 1 },
      { start: 4, end: 5 },
      { start: 9, end: 10 },
    ])
    // The leftmost "co" sits inside "deco"; the one cmdk prefers starts a word.
    expect(cmdkMatchRanges("deco commit", "co")).toEqual([{ start: 5, end: 7 }])
  })

  it("lights a swapped pair cmdk forgives", () => {
    expect(cmdkMatchRanges("Run Tests", "rnu")).toEqual([{ start: 0, end: 3 }])
  })

  it("lights the shown letter a lowercase that expands came from", () => {
    expect(cmdkMatchRanges("İstanbul", "i")).toEqual([{ start: 0, end: 1 }])
  })

  it("lights nothing when cmdk would not keep the row", () => {
    expect(cmdkMatchRanges("git checkout", "zz")).toEqual([])
    expect(cmdkMatchRanges("git checkout", "")).toEqual([])
  })
})
