// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("sonner", () => ({ toast: { info: vi.fn() } }))

import { toast } from "sonner"

import { applyFavicon } from "./favicon"

const toastInfo = vi.mocked(toast.info)

function iconLinks(): HTMLLinkElement[] {
  return Array.from(document.querySelectorAll("link[rel='icon']"))
}

afterEach(() => {
  document.head.innerHTML = ""
})

describe("applyFavicon", () => {
  it("points the icon link at the bundled png for the default", () => {
    applyFavicon("")
    const links = iconLinks()
    expect(links).toHaveLength(1)
    expect(links[0].getAttribute("href")).toBe("/favicon.png")
    // the PNG carries a concrete MIME (matches the static <link> in index.html)
    expect(links[0].getAttribute("type")).toBe("image/png")
  })

  it("leaves the existing link untouched when the default already matches (no churn)", () => {
    // Reproduce the static <link> shipped in index.html, then apply the default
    // favicon: the resolved target is identical, so the element must be reused,
    // not torn down and recreated (otherwise every page load flashes the icon).
    const existing = document.createElement("link")
    existing.setAttribute("rel", "icon")
    existing.setAttribute("type", "image/png")
    existing.setAttribute("href", "/favicon.png")
    document.head.appendChild(existing)

    applyFavicon("")

    const links = iconLinks()
    expect(links).toHaveLength(1)
    expect(links[0]).toBe(existing) // same node — not replaced
  })

  it("replaces an existing icon link rather than stacking them", () => {
    const old = document.createElement("link")
    old.setAttribute("rel", "icon")
    old.setAttribute("href", "/favicon.png")
    document.head.appendChild(old)

    applyFavicon("violet")

    const links = iconLinks()
    expect(links).toHaveLength(1)
    expect(links[0].getAttribute("href")?.startsWith("data:image/svg+xml,")).toBe(
      true,
    )
    expect(links[0].getAttribute("type")).toBe("image/svg+xml")
  })

  it("renders a curated colour as a tinted duck data uri with that colour", () => {
    applyFavicon("violet")
    const href = iconLinks()[0].getAttribute("href") ?? ""
    expect(href.startsWith("data:image/svg+xml,")).toBe(true)
    const decoded = decodeURIComponent(href.replace("data:image/svg+xml,", ""))
    expect(decoded).toContain('fill="#863bff"')
    expect(decoded).toContain('fill-rule="evenodd"')
  })

  it("degrades a legacy value to the bundled png", () => {
    applyFavicon("https://x.test/a.png")
    const links = iconLinks()
    expect(links).toHaveLength(1)
    expect(links[0].getAttribute("href")).toBe("/favicon.png")
    expect(links[0].getAttribute("type")).toBe("image/png")
  })
})

describe("applyFavicon legacy migration notice", () => {
  beforeEach(() => {
    // Clear the module-level re-arm latch (a curated/empty value resets it) and the
    // toast spy so each test starts from a known state.
    applyFavicon("")
    toastInfo.mockClear()
  })

  it("notifies once for a repeated legacy value", () => {
    applyFavicon("#863bff")
    applyFavicon("#863bff")
    expect(toastInfo).toHaveBeenCalledTimes(1)
  })

  it("re-notifies when a DIFFERENT legacy value appears after a curated one", () => {
    applyFavicon("#863bff") // legacy → notice
    applyFavicon("blue") // curated → clears the latch, no notice
    applyFavicon("bogus") // a different legacy value → notice again
    expect(toastInfo).toHaveBeenCalledTimes(2)
  })

  it("never notifies for curated or empty values", () => {
    applyFavicon("")
    applyFavicon("violet")
    applyFavicon("rose")
    expect(toastInfo).not.toHaveBeenCalled()
  })
})
