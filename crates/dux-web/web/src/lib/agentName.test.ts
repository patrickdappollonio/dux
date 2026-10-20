import { readFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

import { isValidAgentName, sanitizeAgentName } from "./agentName"

// The other half of the pin in dux-core's `git.rs`, which also puts every case
// to git itself: the browser accepts exactly the names git takes as branches.
const fixture = JSON.parse(
  readFileSync(
    join(
      dirname(fileURLToPath(import.meta.url)),
      "../../../../dux-core/tests/fixtures/agent_name_cross_language.json",
    ),
    "utf8",
  ),
) as { cases: { name: string; valid: boolean }[] }

describe("isValidAgentName", () => {
  for (const { name, valid } of fixture.cases) {
    it(`${valid ? "accepts" : "refuses"} ${JSON.stringify(name)}`, () => {
      expect(isValidAgentName(name)).toBe(valid)
    })
  }
})

describe("sanitizeAgentName", () => {
  it("turns a space into a dash and drops what git never takes", () => {
    expect(sanitizeAgentName("my feature")).toBe("my-feature")
    expect(sanitizeAgentName("a~b^c:d?e*f[g\\h\ti")).toBe("abcdefghi")
    expect(sanitizeAgentName("dependabot/cargo/tray-icon-0.26.1")).toBe(
      "dependabot/cargo/tray-icon-0.26.1",
    )
  })
})
