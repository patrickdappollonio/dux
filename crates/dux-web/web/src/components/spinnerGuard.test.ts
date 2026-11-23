import { readdirSync, readFileSync, statSync } from "node:fs"
import { fileURLToPath } from "node:url"
import { join } from "node:path"
import { describe, expect, it } from "vitest"

// The one-spinner rule, read from source: a second spinner is a second place
// to forget reduced motion. Each allowance names why it is not a copy.
describe("guard: one loading indicator", () => {
  const root = fileURLToPath(new URL(".", import.meta.url))
  const files: string[] = []
  const walk = (dir: string) => {
    for (const name of readdirSync(dir)) {
      const full = join(dir, name)
      if (statSync(full).isDirectory()) walk(full)
      else if (/\.tsx?$/.test(name) && !/\.test\./.test(name)) files.push(full)
    }
  }
  walk(join(root, ".."))

  // `<Loader2` uses allowed outside Spinner.tsx.
  const ALLOWED: Record<string, number> = {
    // The toast library's busy spinner is the library's to draw.
    "components/ui/sonner.tsx": 0,
    // The ellipsis menu trigger's busy state, which moves to its own shared
    // trigger component.
    "components/ChangedFiles.tsx": 1,
  }

  it("draws a ring only through Spinner", () => {
    for (const file of files) {
      const rel = file.slice(join(root, "..").length + 1)
      if (rel === "components/Spinner.tsx") continue
      const uses = (readFileSync(file, "utf8").match(/<Loader2\b/g) ?? []).length
      expect(uses, `${rel} draws its own ring`).toBe(ALLOWED[rel] ?? 0)
    }
  })
})
