import { readdirSync, readFileSync, statSync } from "node:fs"
import { join, relative } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

// Safari before 16.4 cannot parse a regular-expression lookbehind, and a
// lookbehind literal there is a syntax error for the whole chunk it is in, not
// just the one expression. This fails on `(?<=` or `(?<!` anywhere in a
// production source file under src/.

const srcDir = fileURLToPath(new URL("../", import.meta.url))

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry)
    return statSync(path).isDirectory() ? walk(path) : [path]
  })
}

describe("regular expressions", () => {
  it("use no lookbehind", () => {
    const offenders = walk(srcDir)
      .filter((p) => /\.tsx?$/.test(p) && !p.includes(".test.") && !p.endsWith(".d.ts"))
      .filter((p) => /\(\?<[=!]/.test(readFileSync(p, "utf8")))
      .map((p) => relative(srcDir, p))
    expect(offenders).toEqual([])
  })
})
