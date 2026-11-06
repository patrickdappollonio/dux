import { readFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

import { cloneDirName, GITHUB_COM_ONLY, type GithubHosts } from "./cloneDirName"

// The other half of the pin in dux-core's `clone_project.rs`: both read this
// one file, so the browser fills in the folder the clone itself will use.
const fixture = JSON.parse(
  readFileSync(
    join(
      dirname(fileURLToPath(import.meta.url)),
      "../../../../dux-core/tests/fixtures/clone_dir_name_cross_language.json",
    ),
    "utf8",
  ),
) as {
  cases: { what: string; address: string; github_hosts?: GithubHosts; name: string | null }[]
}

describe("cloneDirName", () => {
  for (const { what, address, github_hosts, name } of fixture.cases) {
    it(`names the folder for ${what}`, () => {
      expect(cloneDirName(address, github_hosts ?? GITHUB_COM_ONLY)).toBe(name)
    })
  }
})
