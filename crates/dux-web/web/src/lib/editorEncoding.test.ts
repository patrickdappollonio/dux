import { afterEach, describe, expect, it } from "vitest"

import {
  REOPEN_CHOICES,
  SAVE_CHOICES,
  dismissFallbackNotice,
  encodingAfterSave,
  encodingLabel,
  fallbackNoticeDismissed,
  resetFallbackNotices,
  showsEncoding,
  showsFallbackNotice,
} from "./editorEncoding"

describe("encodingLabel", () => {
  it("names the encodings the way the status bar shows them", () => {
    expect(encodingLabel("UTF-8", false)).toBe("UTF-8")
    expect(encodingLabel("UTF-8", true)).toBe("UTF-8 with BOM")
    expect(encodingLabel("UTF-16LE", true)).toBe("UTF-16 LE")
    expect(encodingLabel("UTF-16BE", true)).toBe("UTF-16 BE")
    expect(encodingLabel("windows-1252", false)).toBe("Windows-1252")
    expect(encodingLabel("Shift_JIS", false)).toBe("Shift_JIS")
    expect(encodingLabel("gb18030", false)).toBe("GB18030")
  })

  it("falls back to the server's own name for one it does not list", () => {
    expect(encodingLabel("x-mac-cyrillic", false)).toBe("x-mac-cyrillic")
  })
})

describe("showsEncoding", () => {
  it("stays quiet for plain UTF-8 only", () => {
    expect(showsEncoding("UTF-8", false)).toBe(false)
    expect(showsEncoding("UTF-8", true)).toBe(true)
    expect(showsEncoding("windows-1252", false)).toBe(true)
    expect(showsEncoding("UTF-16LE", true)).toBe(true)
  })
})

describe("the menus", () => {
  it("offer UTF-8 first in both, and UTF-8 with and without a mark to save", () => {
    expect(REOPEN_CHOICES[0].encoding).toBe("UTF-8")
    expect(SAVE_CHOICES.slice(0, 2)).toEqual([
      { encoding: "UTF-8", bom: false, label: "UTF-8" },
      { encoding: "UTF-8", bom: true, label: "UTF-8 with BOM" },
    ])
  })

  it("always write UTF-16 with its mark, so the next open recognises it", () => {
    for (const choice of SAVE_CHOICES.filter((c) => c.encoding.startsWith("UTF-16"))) {
      expect(choice.bom).toBe(true)
    }
    for (const choice of SAVE_CHOICES.filter(
      (c) => !c.encoding.startsWith("UTF-"),
    )) {
      expect(choice.bom).toBe(false)
    }
  })

  it("list each encoding once", () => {
    const names = REOPEN_CHOICES.map((c) => c.encoding)
    expect(new Set(names).size).toBe(names.length)
  })
})

describe("encodingAfterSave", () => {
  const fallback = {
    encoding: "windows-1252",
    bom: false,
    encodingFallback: true,
    encodingRequested: null,
  }

  it("leaves an ordinary save in the same encoding alone, notice and all", () => {
    expect(encodingAfterSave(fallback, { encoding: "windows-1252", bom: false })).toBe(
      fallback,
    )
  })

  it("moves a conversion to the new encoding, no longer a guess and read by name", () => {
    expect(encodingAfterSave(fallback, { encoding: "UTF-8", bom: false })).toEqual({
      encoding: "UTF-8",
      bom: false,
      encodingFallback: false,
      encodingRequested: "UTF-8",
    })
  })
})

describe("the fallback notice", () => {
  afterEach(resetFallbackNotices)

  it("shows only for a fallback that has not been dismissed", () => {
    expect(showsFallbackNotice({ encodingFallback: true }, false)).toBe(true)
    expect(showsFallbackNotice({ encodingFallback: true }, true)).toBe(false)
    expect(showsFallbackNotice({ encodingFallback: false }, false)).toBe(false)
    expect(showsFallbackNotice(undefined, false)).toBe(false)
  })

  it("is dismissed per file and per root", () => {
    dismissFallbackNotice("agent:1", "a.txt")
    expect(fallbackNoticeDismissed("agent:1", "a.txt")).toBe(true)
    expect(fallbackNoticeDismissed("agent:1", "b.txt")).toBe(false)
    expect(fallbackNoticeDismissed("agent:2", "a.txt")).toBe(false)
  })
})
