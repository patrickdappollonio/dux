import { readdirSync, readFileSync } from "node:fs"
import { join } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

// A warning, danger or info notice is the `Notice` component. This guard reads
// every component source and fails on a class string that paints the notice
// frame by hand: a warning or danger border together with the tint of the same
// tone. It reads class text only, so a frame built at runtime gets past it.

const dir = fileURLToPath(new URL("./", import.meta.url))

const FRAME =
  /(?:border-dux-warning|border-destructive)[^"'`\n]*(?:bg-dux-warning|bg-destructive)\/\d+|(?:bg-dux-warning|bg-destructive)\/\d+[^"'`\n]*(?:border-dux-warning|border-destructive)/

// File, with why its tinted frame is not a notice.
const ALLOWED: Record<string, string> = {
  "Notice.tsx": "the component itself",
  "DeleteSessionDialog.tsx": "a branch-check pill: a rounded-full status chip in a row, not a sentence-and-buttons notice",
}

describe("no hand-built notice frame", () => {
  it("keeps every warning and danger frame inside Notice", () => {
    const hits = readdirSync(dir)
      .filter((f) => f.endsWith(".tsx") && !f.includes(".test."))
      .filter((f) => !(f in ALLOWED))
      .filter((f) => FRAME.test(readFileSync(join(dir, f), "utf8")))
    expect(hits, "build the notice with <Notice>, or allow-list the file with a reason").toEqual([])
  })
})
