// Every tab strip in the web UI is built from `TabStrip` and `TabPill`, so the
// agent's provider tabs, an agent's terminal dock and the editor's file tabs
// look the same wherever they are. This guard reads the source and fails on a
// tab pill or a strip container written anywhere else.
//
// It reads text, not the program, so it cannot see a role or a class string
// built at run time, and it skips test files.
import { readdirSync, readFileSync, statSync } from "node:fs"
import { join, relative } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

const srcDir = fileURLToPath(new URL("../", import.meta.url))

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry)
    return statSync(path).isDirectory() ? walk(path) : [path]
  })
}

const SHARED = "components/TabStrip.tsx"

/** Files that may write a tab pill or a strip by hand, each with its reason. */
const ALLOWED: Record<string, string> = {}

const sources = walk(srcDir)
  .filter((path) => /\.tsx?$/.test(path) && !/\.test\.tsx?$/.test(path))
  .map((path) => ({ file: relative(srcDir, path), text: readFileSync(path, "utf8") }))

describe("tab strips are built from the shared TabStrip and TabPill", () => {
  it("writes the tab role only in the shared pill", () => {
    const offenders = sources
      .filter(({ file }) => file !== SHARED && !(file in ALLOWED))
      // A JSX attribute, not a selector such as `[role="tab"]`.
      .filter(({ text }) => /\srole=(?:"tab"|\{\s*["']tab["']\s*\})/.test(text))
      .map(({ file }) => file)
    expect(offenders).toEqual([])
  })

  it("writes the strip container's classes only in the shared strip", () => {
    const offenders = sources
      .filter(({ file }) => file !== SHARED && !(file in ALLOWED))
      .filter(({ text }) => text.includes("bg-muted/30 px-2 py-1.25"))
      .map(({ file }) => file)
    expect(offenders).toEqual([])
  })
})
