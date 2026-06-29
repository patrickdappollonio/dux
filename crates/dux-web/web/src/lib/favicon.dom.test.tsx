// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest"

import { applyFavicon } from "./favicon"

function iconLinks(): HTMLLinkElement[] {
  return Array.from(document.querySelectorAll("link[rel='icon']"))
}

afterEach(() => {
  document.head.innerHTML = ""
})

describe("applyFavicon", () => {
  it("points the icon link at the bundled svg for the default", () => {
    applyFavicon("")
    const links = iconLinks()
    expect(links).toHaveLength(1)
    expect(links[0].getAttribute("href")).toBe("/favicon.svg")
    expect(links[0].getAttribute("type")).toBe("image/svg+xml")
  })

  it("replaces an existing icon link rather than stacking them", () => {
    const old = document.createElement("link")
    old.setAttribute("rel", "icon")
    old.setAttribute("href", "/favicon.svg")
    document.head.appendChild(old)

    applyFavicon("#863bff")

    const links = iconLinks()
    expect(links).toHaveLength(1)
    expect(links[0].getAttribute("href")?.startsWith("data:image/svg+xml,")).toBe(
      true,
    )
    expect(links[0].getAttribute("type")).toBe("image/svg+xml")
  })

  it("uses a custom url verbatim without forcing the svg type", () => {
    applyFavicon("https://x.test/a.png")
    const links = iconLinks()
    expect(links).toHaveLength(1)
    expect(links[0].getAttribute("href")).toBe("https://x.test/a.png")
    expect(links[0].getAttribute("type")).toBeNull()
  })

  it("renders a named colour as an outline data uri with that colour", () => {
    applyFavicon("violet")
    const href = iconLinks()[0].getAttribute("href") ?? ""
    expect(href.startsWith("data:image/svg+xml,")).toBe(true)
    const decoded = decodeURIComponent(href.replace("data:image/svg+xml,", ""))
    expect(decoded).toContain('stroke="#863bff"')
    expect(decoded).toContain('fill="none"')
  })
})
