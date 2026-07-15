// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("sonner", () => ({ toast: { info: vi.fn() } }))

import { toast } from "sonner"

import { applyAttentionFavicon, applyFavicon } from "./favicon"

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

  it("points the toast at the Preferences dialog, not the removed command palette", () => {
    applyFavicon("#863bff")
    const message = toastInfo.mock.calls[0][0] as string
    expect(message).toContain("Preferences dialog")
    expect(message).toContain("cog menu")
    expect(message).not.toMatch(/command palette/i)
    expect(message).not.toContain("—")
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

describe("applyAttentionFavicon", () => {
  afterEach(() => {
    document.head.innerHTML = ""
    vi.restoreAllMocks()
    vi.unstubAllGlobals()
  })

  it("restores the clean base icon when there is no attention", () => {
    applyAttentionFavicon("", false)
    const links = iconLinks()
    expect(links).toHaveLength(1)
    expect(links[0].getAttribute("href")).toBe("/favicon.png")
  })

  it("keeps the clean base icon when compositing cannot run (jsdom canvas)", async () => {
    // jsdom has no real <canvas> 2d context, so `composeFaviconWithDot` fails and
    // resolves/rejects to a no-op. The meaningful guarantee: the icon stays the
    // clean base PNG, with no half-composed or dotted data-URL icon ever applied.
    vi.spyOn(console, "warn").mockImplementation(() => {})
    applyAttentionFavicon("", false)
    applyAttentionFavicon("", true)
    // Flush the compose promise chain.
    await Promise.resolve()
    await Promise.resolve()
    const links = iconLinks()
    expect(links).toHaveLength(1)
    expect(links[0].getAttribute("href")).toBe("/favicon.png")
  })

  it("does not apply a stale composed icon after attention clears mid-compose", async () => {
    // The out-of-order guard (`wantedDotBase`): if attention clears while a
    // compose is still in flight, the compose must NOT stomp the restored clean
    // icon when it finally resolves.
    vi.spyOn(console, "warn").mockImplementation(() => {})

    // A minimal fake 2D context so `composeFaviconWithDot` runs to completion.
    const fakeCtx = {
      clearRect: () => {},
      drawImage: () => {},
      beginPath: () => {},
      arc: () => {},
      fill: () => {},
      fillStyle: "",
    } as unknown as CanvasRenderingContext2D
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(fakeCtx)
    vi.spyOn(HTMLCanvasElement.prototype, "toDataURL").mockReturnValue(
      "data:image/png;base64,COMPOSED",
    )

    // Stub Image so we control exactly when onload fires (the compose resolves).
    const createdImages: Array<{ onload?: () => void; onerror?: () => void }> = []
    vi.stubGlobal(
      "Image",
      class {
        onload?: () => void
        onerror?: () => void
        set src(_v: string) {}
        constructor() {
          createdImages.push(this)
        }
      },
    )

    applyAttentionFavicon("", false) // clean base established
    applyAttentionFavicon("", true) // request a dot: compose starts, in flight
    applyAttentionFavicon("", false) // attention clears BEFORE the compose resolves

    // Now let the in-flight compose finish. It resolves the composed data URL, but
    // the dot is no longer wanted, so it must be dropped.
    createdImages[0]?.onload?.()
    await Promise.resolve()
    await Promise.resolve()

    const links = iconLinks()
    expect(links).toHaveLength(1)
    expect(links[0].getAttribute("href")).toBe("/favicon.png")
    expect(links[0].getAttribute("href")).not.toContain("COMPOSED")
  })

  it("restores the base after attention clears", () => {
    applyAttentionFavicon("", true) // no-op compose under jsdom
    applyAttentionFavicon("", false)
    const links = iconLinks()
    expect(links).toHaveLength(1)
    expect(links[0].getAttribute("href")).toBe("/favicon.png")
  })
})
