import { readdirSync, readFileSync } from "node:fs"
import { join } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

// A hand-built `⋯` trigger drifts from the others in size, border, reveal and
// touch floor, so the dots glyph is imported by the shared trigger alone.
describe("no hand-built ⋯ trigger (guard)", () => {
  const ALLOWED = new Set(["EllipsisMenuTrigger.tsx"])
  const walk = (dir: string): string[] =>
    readdirSync(dir, { withFileTypes: true }).flatMap((e) =>
      e.isDirectory()
        ? walk(join(dir, e.name))
        : /\.(tsx|ts)$/.test(e.name) && !/\.test\./.test(e.name)
          ? [join(dir, e.name)]
          : [],
    )
  it("imports the ellipsis glyph in no file but the shared trigger", () => {
    const root = fileURLToPath(new URL("../", import.meta.url))
    const offenders = walk(root).filter((f) => {
      if (ALLOWED.has(f.slice(f.lastIndexOf("/") + 1))) return false
      const names = [...readFileSync(f, "utf8").matchAll(/import\s*\{([^}]*)\}\s*from "lucide-react"/g)]
        .flatMap((m) => m[1].split(","))
        .map((n) => n.trim().split(/\s+as\s+/)[0])
      return names.some((n) => /^(Ellipsis|EllipsisVertical|MoreHorizontal|MoreVertical)$/.test(n))
    })
    expect(offenders, "build a ⋯ menu trigger with EllipsisMenuTrigger").toEqual([])
  })
})
