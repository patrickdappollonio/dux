// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("sonner", () => ({ toast: { info: vi.fn() } }))

import { toast } from "sonner"

import { ATTENTION_PULSE_PERIOD_MS } from "./attentionPulse"
import { applyAttentionFavicon, applyFavicon } from "./favicon"
import { toastText } from "@/test/toastText"

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
    expect(links[0]).toBe(existing) // same node, not replaced
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
    const message = toastText(toastInfo.mock.calls[0][0])
    expect(message).toContain("Preferences dialog")
    expect(message).toContain("cog menu")
    expect(message).not.toMatch(/command palette/i)
    expect(message).not.toContain("\u2014")
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

describe("applyAttentionFavicon blink", () => {
  // A 2D context that records the dot's alpha, and a toDataURL that names the
  // frame it was asked for from that alpha, so a test can tell the frames apart.
  let composes = 0
  let lastAlpha = 1
  let images: Array<{ onload?: () => void; onerror?: () => void; src?: string }> = []

  function installCanvas() {
    const ctx = {
      clearRect: () => {},
      drawImage: () => {},
      beginPath: () => {},
      arc: () => {},
      fill: () => {
        lastAlpha = ctx.globalAlpha
      },
      fillStyle: "",
      globalAlpha: 1,
    }
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(
      ctx as unknown as CanvasRenderingContext2D,
    )
    vi.spyOn(HTMLCanvasElement.prototype, "toDataURL").mockImplementation(() => {
      composes += 1
      return `data:image/png;base64,${lastAlpha === 1 ? "ON" : "DIM"}-${composes}`
    })
    vi.stubGlobal(
      "Image",
      class {
        onload?: () => void
        onerror?: () => void
        src?: string
        constructor() {
          images.push(this)
        }
      },
    )
  }

  function reducedMotion(on: boolean) {
    vi.stubGlobal("matchMedia", (q: string) => ({
      matches: on && q.includes("reduce"),
      media: q,
      addEventListener: () => {},
      removeEventListener: () => {},
    }))
  }

  async function loadImages() {
    for (const img of images.splice(0)) img.onload?.()
    await Promise.resolve()
    await Promise.resolve()
  }

  function shownHref(): string {
    return iconLinks()[0]?.getAttribute("href") ?? ""
  }

  // A fresh module per test: the frame cache is module state, and the earlier
  // suites leave their own composed icons in it.
  let applyAttentionFavicon: typeof import("./favicon").applyAttentionFavicon

  beforeEach(async () => {
    vi.resetModules()
    ;({ applyAttentionFavicon } = await import("./favicon"))
    vi.useFakeTimers()
    composes = 0
    images = []
    installCanvas()
    reducedMotion(false)
    applyAttentionFavicon("", false)
  })

  afterEach(() => {
    applyAttentionFavicon("", false)
    vi.useRealTimers()
    vi.restoreAllMocks()
    vi.unstubAllGlobals()
    document.head.innerHTML = ""
  })

  it("blinks between the dotted and dimmed frames on the shared rhythm while attention holds", async () => {
    applyAttentionFavicon("", true)
    await loadImages()
    expect(shownHref()).toContain("ON")

    const seen = new Set<string>()
    for (let t = 0; t < ATTENTION_PULSE_PERIOD_MS; t += 10) {
      vi.advanceTimersByTime(10)
      seen.add(shownHref().includes("DIM") ? "dim" : "on")
    }
    expect(seen).toEqual(new Set(["on", "dim"]))
  })

  it("stops exactly when attention clears, restoring the configured favicon and leaving no timer", async () => {
    applyAttentionFavicon("violet", true)
    await loadImages()
    expect(vi.getTimerCount()).toBeGreaterThan(0)

    applyAttentionFavicon("violet", false)
    expect(shownHref().startsWith("data:image/svg+xml,")).toBe(true)
    expect(iconLinks()[0].getAttribute("type")).toBe("image/svg+xml")
    expect(vi.getTimerCount()).toBe(0)
    vi.advanceTimersByTime(ATTENTION_PULSE_PERIOD_MS * 3)
    expect(shownHref().startsWith("data:image/svg+xml,")).toBe(true)
  })

  it("holds a steady dot under reduced motion, with no timer running", async () => {
    reducedMotion(true)
    applyAttentionFavicon("", true)
    await loadImages()
    expect(shownHref()).toContain("ON")
    expect(vi.getTimerCount()).toBe(0)
    vi.advanceTimersByTime(ATTENTION_PULSE_PERIOD_MS * 2)
    expect(shownHref()).toContain("ON")
  })

  it("pre-renders both frames once per favicon and never re-encodes per tick", async () => {
    applyAttentionFavicon("", true)
    await loadImages()
    expect(composes).toBe(2)
    vi.advanceTimersByTime(ATTENTION_PULSE_PERIOD_MS * 4)
    expect(composes).toBe(2)

    // Another favicon composes its own pair; coming back reuses the cache.
    applyAttentionFavicon("blue", true)
    await loadImages()
    expect(composes).toBe(4)
    applyAttentionFavicon("", false)
    applyAttentionFavicon("", true)
    await loadImages()
    expect(composes).toBe(4)
    expect(shownHref()).toContain("ON")
  })

  it("keeps blinking through the store's repeat calls while attention still holds", async () => {
    applyAttentionFavicon("", true)
    await loadImages()
    const frames: string[] = []
    for (let t = 0; t < ATTENTION_PULSE_PERIOD_MS; t += 10) {
      vi.advanceTimersByTime(10)
      // The store re-applies on every spine push.
      applyAttentionFavicon("", true)
      frames.push(shownHref().includes("DIM") ? "dim" : "on")
    }
    expect(frames).toContain("dim")
  })
})
