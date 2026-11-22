import { readFileSync } from "node:fs"
import { fileURLToPath } from "node:url"
import { compile } from "@tailwindcss/node"
import { describe, expect, it } from "vitest"

// Compiles the shipped stylesheet. Order matters because two classes that set
// the same property tie on specificity and the later one in the stylesheet
// wins; `md:` and `max-md:` must sit where they sat while they were media
// queries, or every class list that pairs them with another variant changes
// meaning without a line of it changing.
const indexCss = fileURLToPath(new URL("./index.css", import.meta.url))

async function build(candidates: string[]): Promise<string> {
  const compiler = await compile(readFileSync(indexCss, "utf8"), {
    base: fileURLToPath(new URL(".", import.meta.url)),
    from: indexCss,
    onDependency: () => {},
  })
  return compiler.build(candidates)
}

function position(css: string, candidate: string): number {
  const selector = "." + candidate.replace(/[:[\]=]/g, (c) => `\\${c}`) + " {"
  const at = css.indexOf(selector)
  expect(at, candidate).toBeGreaterThanOrEqual(0)
  return at
}

// The order Tailwind gave these variants when `md:` and `max-md:` were its
// own width breakpoints.
const BREAKPOINT_ORDER = [
  "aria-expanded:p-1",
  "data-[state=open]:p-1",
  "max-lg:p-1",
  "max-md:p-1",
  "max-sm:p-1",
  "sm:p-1",
  "md:p-1",
  "lg:p-1",
  "pointer-coarse:p-1",
  "pointer-fine:p-1",
  "data-open:p-1",
]

describe("the layout variants", () => {
  it("sort where the width breakpoints they replace sorted", async () => {
    const css = await build(BREAKPOINT_ORDER)
    const sorted = [...BREAKPOINT_ORDER].sort(
      (a, b) => position(css, a) - position(css, b),
    )
    expect(sorted).toEqual(BREAKPOINT_ORDER)
  })

  it("key on the layout attribute, and leave the other max-* as widths", async () => {
    const css = await build(["md:p-1", "max-md:p-2", "max-sm:p-3"])
    expect(css).toMatch(/\.md\\:p-1 \{\s*&:where\(:root\[data-layout="computer"\] \*\)/)
    expect(css).toMatch(/\.max-md\\:p-2 \{\s*&:where\(:root\[data-layout="phone"\] \*\)/)
    expect(css).toMatch(/\.max-sm\\:p-3 \{\s*@media \(width < 40rem\)/)
  })
})
