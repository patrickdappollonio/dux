import { describe, expect, it } from "vitest"

import { deviceLabel } from "./deviceLabel"

// Representative real-world UA strings for each browser/OS pair the label names.
const UA = {
  chromeMac:
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
  chromeLinux:
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
  chromeWindows:
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
  safariMac:
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.1 Safari/605.1.15",
  firefoxLinux:
    "Mozilla/5.0 (X11; Linux x86_64; rv:121.0) Gecko/20100101 Firefox/121.0",
  edgeWindows:
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36 Edg/120.0.0.0",
  chromeAndroid:
    "Mozilla/5.0 (Linux; Android 13; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Mobile Safari/537.36",
  safariIphone:
    "Mozilla/5.0 (iPhone; CPU iPhone OS 17_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.1 Mobile/15E148 Safari/604.1",
  // Mobile Edge uses platform-specific tokens ("EdgA/" on Android, "EdgiOS/" on
  // iOS) that do NOT contain the bare desktop "Edg/", so they must be matched
  // explicitly or they fall through to Chrome.
  edgeAndroid:
    "Mozilla/5.0 (Linux; Android 13; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Mobile Safari/537.36 EdgA/120.0.0.0",
  edgeIphone:
    "Mozilla/5.0 (iPhone; CPU iPhone OS 17_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.1 EdgiOS/120.0.0.0 Mobile/15E148 Safari/605.1.15",
  // A Windows UA whose browser token is not one we recognize.
  unknownBrowserWindows: "SomeCrawler/2.0 (Windows NT 10.0; Win64; x64)",
}

describe("deviceLabel", () => {
  it("names browser + OS for the common pairs", () => {
    expect(deviceLabel(UA.chromeMac)).toBe("Chrome on macOS")
    expect(deviceLabel(UA.chromeLinux)).toBe("Chrome on Linux")
    expect(deviceLabel(UA.chromeWindows)).toBe("Chrome on Windows")
    expect(deviceLabel(UA.safariMac)).toBe("Safari on macOS")
    expect(deviceLabel(UA.firefoxLinux)).toBe("Firefox on Linux")
    expect(deviceLabel(UA.edgeWindows)).toBe("Edge on Windows")
    expect(deviceLabel(UA.chromeAndroid)).toBe("Chrome on Android")
    expect(deviceLabel(UA.safariIphone)).toBe("Safari on iOS")
  })

  it("names mobile Edge (Android/iOS) as Edge, not Chrome", () => {
    // The bare "Edg/" desktop check would miss these; the broadened pattern must
    // match "EdgA/" and "EdgiOS/" and win over the Chrome fallback.
    expect(deviceLabel(UA.edgeAndroid)).toBe("Edge on Android")
    expect(deviceLabel(UA.edgeIphone)).toBe("Edge on iOS")
  })

  it("returns null for unknown, empty, or missing input", () => {
    expect(deviceLabel("totally unrecognizable string")).toBeNull()
    expect(deviceLabel("")).toBeNull()
    expect(deviceLabel("   ")).toBeNull()
    expect(deviceLabel(null)).toBeNull()
    expect(deviceLabel(undefined)).toBeNull()
  })

  it("falls back to OS-only when the OS is known but the browser is not", () => {
    expect(deviceLabel(UA.unknownBrowserWindows)).toBe("Windows")
  })

  it("orders OS checks so Android beats Linux and iOS beats macOS", () => {
    // Android UA also contains "Linux".
    expect(deviceLabel(UA.chromeAndroid)).toContain("Android")
    // iPhone UA also contains "like Mac OS X".
    expect(deviceLabel(UA.safariIphone)).toContain("iOS")
  })
})
