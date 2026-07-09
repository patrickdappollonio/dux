import { readFileSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

import { DOCS_AGENT_TABS_CLOSING, DOCS_AGENT_TABS_RESUME } from "./docs"

// Guard against silent anchor drift: these constants deep-link to headings in
// website/docs/agent-tabs.md (a separate package), and the site slugs those
// headings with rehype-slug. If someone renames a heading, this fails so they
// update the constant here too, rather than shipping a link that lands mid-page.
const here = dirname(fileURLToPath(import.meta.url))
const doc = readFileSync(
  resolve(here, "../../../../../website/docs/agent-tabs.md"),
  "utf8",
)

describe("agent-tabs docs anchors", () => {
  it("the exported links point at the intended anchors", () => {
    expect(DOCS_AGENT_TABS_RESUME).toBe(
      "https://getdux.app/docs/agent-tabs#how-resume-works",
    )
    expect(DOCS_AGENT_TABS_CLOSING).toBe(
      "https://getdux.app/docs/agent-tabs#closing-a-tab-is-one-way",
    )
  })

  it("the headings those anchors resolve to still exist in the docs page", () => {
    // `#how-resume-works`
    expect(doc).toMatch(/^## How resume works$/m)
    // `#closing-a-tab-is-one-way`
    expect(doc).toMatch(/^### Closing a tab is one-way$/m)
  })
})
