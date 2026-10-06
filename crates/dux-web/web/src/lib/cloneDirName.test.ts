import { readFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

import { cloneDirName } from "./cloneDirName"

// The other half of the pin in dux-core's `clone_project.rs`: both read this
// one file, so the browser fills in the folder git itself would pick.
const fixture = JSON.parse(
  readFileSync(
    join(
      dirname(fileURLToPath(import.meta.url)),
      "../../../../dux-core/tests/fixtures/clone_dir_name_cross_language.json",
    ),
    "utf8",
  ),
) as { cases: { what: string; address: string; name: string | null }[] }

describe("cloneDirName", () => {
  for (const { what, address, name } of fixture.cases) {
    it(`names the folder for ${what}`, () => {
      expect(cloneDirName(address)).toBe(name)
    })
  }
})
