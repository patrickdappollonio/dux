// A card saying an agent or one of its tabs is not running is the shared
// `NotRunningCard`, which owns its layout, the resume docs link and the Start
// button's touch size. Every such card links to how resume works, so a file
// other than the card that reaches for that link is building a copy of it.
import { readdirSync, readFileSync, statSync } from "node:fs"
import { join, relative } from "node:path"
import { fileURLToPath } from "node:url"
import { expect, it } from "vitest"

const srcDir = fileURLToPath(new URL("../", import.meta.url))

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry)
    return statSync(path).isDirectory() ? walk(path) : [path]
  })
}

it("links to how resume works only from the shared not-running card", () => {
  const users = walk(join(srcDir, "components"))
    .map((path) => relative(srcDir, path))
    .filter((name) => /\.tsx?$/.test(name) && !name.includes(".test."))
    .filter((name) =>
      readFileSync(join(srcDir, name), "utf-8").includes("DOCS_AGENT_TABS_RESUME"),
    )
  expect(users).toEqual(["components/NotRunningCard.tsx"])
})
