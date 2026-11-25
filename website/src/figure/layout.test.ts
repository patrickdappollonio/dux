import { readFileSync } from "node:fs"
import { describe, expect, it } from "vitest"

import { FIGURE_LAYOUT } from "./layout"

const page = readFileSync(new URL("../pages/figure/web-ui.astro", import.meta.url), "utf8")
const figure = readFileSync(new URL("./figure.html", import.meta.url), "utf8")

// The sidebar's container is `hidden … md:*`, and the app's `md:` variants hold
// only under `[data-layout=computer]` on <html>. A figure page that does not
// declare that layout serves the sidebar at display:none.
describe("web UI figure layout", () => {
  it("draws the sidebar's container behind the computer layout's variant", () => {
    expect(figure).toMatch(/data-slot="sidebar-wrapper"[\s\S]*?class="[^"]*\bhidden\b[^"]*\bmd:(block|flex)\b/)
  })

  it("is the computer layout the app resolves for a wide screen", () => {
    expect(FIGURE_LAYOUT).toBe("computer")
  })

  it("is declared on the served page's <html> element", () => {
    const html = page.match(/<html\b[^>]*>/)?.[0] ?? ""
    expect(html).toContain("data-layout={FIGURE_LAYOUT}")
  })
})
