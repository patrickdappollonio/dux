// A warning is painted in the theme's `dux-warning` token, never in a Tailwind
// palette amber or yellow, so the tone lives in one place in index.css. This
// guard reads every source file under components/ and lib/ and fails on a
// palette amber or yellow utility outside the allowlist below, whose entries
// are STATE colors rather than warnings and keep their own hue on purpose.
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

const PALETTE_AMBER = /\b[a-z-]+-(?:amber|yellow)-\d{2,3}\b/g

// File and exact class, with why it is not a warning.
const ALLOWED: Record<string, { classes: string[]; why: string }> = {
  "components/StatusBadge.tsx": {
    classes: ["text-amber-500"],
    why: "the detached session state, not a warning",
  },
  "lib/agentRow.ts": {
    classes: ["text-amber-500"],
    why: "the detached state word's color, not a warning",
  },
  "lib/flatList.ts": {
    classes: ["text-amber-500"],
    why: "the detached state word's color, not a warning",
  },
  "components/ConnDot.tsx": {
    classes: ["bg-amber-500"],
    why: "the connecting/reconnecting connection state dot",
  },
  "components/FileStatusIcon.tsx": {
    classes: ["text-amber-500"],
    why: "the git status hue for a modified or type-changed file",
  },
}

describe("warning tone", () => {
  it("defines the dux-warning token in the theme", () => {
    const css = readFileSync(join(srcDir, "index.css"), "utf-8")
    expect(css).toMatch(/--color-dux-warning:\s*var\(--dux-warning\)/)
    expect(css.match(/--dux-warning:/g)?.length).toBe(2)
  })

  it("paints warnings with the token, never a palette amber or yellow", () => {
    const offenders: string[] = []
    for (const path of [
      ...walk(join(srcDir, "components")),
      ...walk(join(srcDir, "lib")),
    ]) {
      const name = relative(srcDir, path)
      if (!/\.tsx?$/.test(name) || name.includes(".test.")) continue
      const source = readFileSync(path, "utf-8")
      const allowed = ALLOWED[name]?.classes ?? []
      for (const match of source.matchAll(PALETTE_AMBER)) {
        if (!allowed.includes(match[0])) offenders.push(`${name}: ${match[0]}`)
      }
    }
    expect(offenders).toEqual([])
  })

  it.each([
    "components/RenameSessionDialog.tsx",
    "components/AddProjectDialog.tsx",
    "components/AgentInfoDialog.tsx",
  ])("%s paints its warning with the token", (name) => {
    const source = readFileSync(join(srcDir, name), "utf-8")
    expect(source).toContain("text-dux-warning")
  })
})
