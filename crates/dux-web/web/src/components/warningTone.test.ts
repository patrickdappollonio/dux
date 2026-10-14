// A warning is painted in the theme's `dux-warning` token, never in a Tailwind
// palette amber or yellow, so the tone lives in one place in index.css. The
// amber STATE colors (a detached agent, a connection still being made, a
// modified file) are not warnings, and each has a semantic token of its own for
// the same reason. This guard reads every source file under components/ and
// lib/ and fails on a palette amber or yellow utility outside the allowlist
// below, which carries a reason for every entry. The green and red state
// colors (working, active, added and deleted lines, a pull request's state,
// success and failure) are tokens for the same reason, so a palette green, red,
// emerald or rose utility class written out in any source file under src/
// fails too, with no exceptions. Both checks match literal palette class names
// only: a hex or arbitrary value, or a class assembled at runtime, gets past
// them. Each state token must also be defined in index.css and used where it
// belongs, because Tailwind emits nothing for a class naming an undefined
// token and the color would vanish without an error.
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
const PALETTE_GREEN_RED = /\b[a-z-]+-(?:green|red|emerald|rose)-\d{2,3}\b/g

// File and exact class, with why it cannot be a token.
const ALLOWED: Record<string, { classes: string[]; why: string }> = {}

// The state tokens, each with the class that paints it in each file that must
// use it.
const STATE_TOKENS: { token: string; uses: Record<string, string> }[] = [
  {
    token: "dux-detached",
    uses: {
      "components/StatusBadge.tsx": "text-dux-detached",
      "lib/agentRow.ts": "text-dux-detached",
      "lib/flatList.ts": "text-dux-detached",
    },
  },
  {
    token: "dux-connecting",
    uses: { "components/ConnDot.tsx": "bg-dux-connecting" },
  },
  {
    token: "dux-git-modified",
    uses: { "components/FileStatusIcon.tsx": "text-dux-git-modified" },
  },
  {
    token: "dux-working",
    uses: {
      "lib/flatList.ts": "text-dux-working",
      "lib/flatTerminals.ts": "text-dux-working",
    },
  },
  {
    token: "dux-active",
    uses: {
      "components/StatusBadge.tsx": "text-dux-active",
      "lib/agentRow.ts": "text-dux-active",
    },
  },
  {
    token: "dux-added",
    uses: {
      "components/ChangedFiles.tsx": "text-dux-added",
      "components/FileStatusIcon.tsx": "text-dux-added",
    },
  },
  {
    token: "dux-deleted",
    uses: {
      "components/ChangedFiles.tsx": "text-dux-deleted",
      "components/FileStatusIcon.tsx": "text-dux-deleted",
    },
  },
  {
    token: "dux-success",
    uses: {
      "components/ConnDot.tsx": "bg-dux-success",
      "components/ui/sonner.tsx": "text-dux-success",
    },
  },
  {
    token: "dux-error",
    uses: { "components/ConnDot.tsx": "bg-dux-error" },
  },
  { token: "dux-pr-open", uses: { "lib/pr.ts": "text-dux-pr-open" } },
  { token: "dux-pr-open-hover", uses: { "lib/pr.ts": "hover:text-dux-pr-open-hover" } },
  { token: "dux-pr-open-tint", uses: { "lib/pr.ts": "bg-dux-pr-open-tint/10" } },
  { token: "dux-pr-closed", uses: { "lib/pr.ts": "text-dux-pr-closed" } },
  { token: "dux-pr-closed-hover", uses: { "lib/pr.ts": "hover:text-dux-pr-closed-hover" } },
  { token: "dux-pr-closed-tint", uses: { "lib/pr.ts": "bg-dux-pr-closed-tint/10" } },
]

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

  it("paints green and red states with tokens, never a palette class", () => {
    const offenders: string[] = []
    for (const path of walk(srcDir)) {
      const name = relative(srcDir, path)
      if (!/\.tsx?$/.test(name) || name.includes(".test.")) continue
      const source = readFileSync(path, "utf-8")
      for (const match of source.matchAll(PALETTE_GREEN_RED)) {
        offenders.push(`${name}: ${match[0]}`)
      }
    }
    expect(offenders).toEqual([])
  })

  it.each(STATE_TOKENS)("defines the $token state token in the theme", ({ token }) => {
    const css = readFileSync(join(srcDir, "index.css"), "utf-8")
    expect(css).toMatch(new RegExp(`--color-${token}:\\s*var\\(--${token}\\)`))
    expect(css.match(new RegExp(`--${token}:`, "g"))?.length).toBe(2)
  })

  it.each(STATE_TOKENS)("paints the $token state with its token", ({ uses }) => {
    for (const [name, cls] of Object.entries(uses)) {
      const source = readFileSync(join(srcDir, name), "utf-8")
      expect(source, name).toContain(cls)
    }
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
