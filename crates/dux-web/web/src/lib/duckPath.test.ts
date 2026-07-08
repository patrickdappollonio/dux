import { readFileSync } from "node:fs"
import { fileURLToPath } from "node:url"
import { dirname, join } from "node:path"
import { describe, expect, it } from "vitest"

// Drift guard for the traced duck favicon path. The committed DUCK_PATH in
// favicon.ts is produced by `scripts/gen-duck-favicon.mjs`. That is a MANUAL,
// ad-hoc regeneration step (potrace output is nondeterministic, and jimp/potrace
// are not devDependencies) — nothing in CI re-runs it, and the maintainer
// validates the duck visually before committing. This test only guards the
// committed value's shape so it can never silently become empty/truncated/
// malformed (e.g. a bad merge, a botched hand-edit, or a wrong-image trace).

const here = dirname(fileURLToPath(import.meta.url))
const faviconSrc = readFileSync(join(here, "favicon.ts"), "utf8")

function committedDuckPath(): string {
  const match = faviconSrc.match(/const DUCK_PATH =\n {2}"([^"]*)"/)
  if (!match) throw new Error("DUCK_PATH constant not found in favicon.ts")
  return match[1]
}

describe("committed DUCK_PATH", () => {
  it("is a non-empty SVG path starting with a moveto", () => {
    const path = committedDuckPath()
    expect(path.length).toBeGreaterThan(100)
    expect(path.startsWith("M")).toBe(true)
  })

  it("only contains SVG-path characters (no attribute-breakout)", () => {
    const path = committedDuckPath()
    // path data is commands + numbers + whitespace/commas/signs/dots only
    expect(path).toMatch(/^[MLCZmlcz0-9.,\s-]+$/)
    expect(path).not.toContain('"')
    expect(path).not.toContain("<")
  })

  // Geometric CORRUPTION guard (NOT a staleness/exact-match guard): potrace output
  // is nondeterministic, so we never pin the exact path. Instead we assert the
  // committed duck still has its multiple cutout sub-shapes, is built from many
  // bezier curves, and spans the full 512 viewBox — so a truncated, collapsed, or
  // degenerate (straight-line / repeated-segment) path fails the build.
  it("keeps its multiple cutout sub-shapes", () => {
    const path = committedDuckPath()
    // The duck is built from several `M ` subpaths (outer body, head, eyes, beak,
    // bowtie, baton, wing). A gross truncation would drop most of them.
    const subpaths = path.match(/M /g) ?? []
    expect(subpaths.length).toBeGreaterThanOrEqual(6)
  })

  it("is a substantial, curve-rich trace (not a degenerate stub)", () => {
    const path = committedDuckPath()
    // The real trace is thousands of chars of cubic beziers. A short or all-
    // straight-line path (e.g. a corrupted stub or a hand-typed placeholder) has
    // few/no `C` commands and would fail here even if it started with `M` and
    // touched both viewBox edges.
    expect(path.length).toBeGreaterThan(2000)
    const curves = path.match(/C /g) ?? []
    expect(curves.length).toBeGreaterThanOrEqual(20)
  })

  it("spans the full 512 viewBox on both axes", () => {
    const path = committedDuckPath()
    // Parse coordinate PAIRS (x y) and assert BOTH axes reach near both edges of
    // the 0..512 canvas — a collapsed/clipped trace, or one that only varies on a
    // single axis, would sit in a narrow band.
    const nums = (path.match(/-?\d+(?:\.\d+)?/g) ?? []).map(Number)
    const xs: number[] = []
    const ys: number[] = []
    for (let i = 0; i + 1 < nums.length; i += 2) {
      xs.push(nums[i])
      ys.push(nums[i + 1])
    }
    expect(Math.min(...xs)).toBeLessThan(40)
    expect(Math.max(...xs)).toBeGreaterThan(480)
    expect(Math.min(...ys)).toBeLessThan(40)
    expect(Math.max(...ys)).toBeGreaterThan(480)
  })
})
