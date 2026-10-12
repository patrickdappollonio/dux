import { afterEach, describe, expect, it } from "vitest"

import {
  REOPEN_CHOICES,
  SAVE_CHOICES,
  dismissNotice,
  encodingNotices,
  encodingAfterSave,
  encodingLabel,
  noticeDismissed,
  resetEncodingNotices,
  type NoticeFacts,
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


describe("the encoding notices", () => {
  afterEach(resetEncodingNotices)

  const plain: NoticeFacts = {
    encoding: "UTF-8",
    bom: false,
    encodingFallback: false,
    unwritable: null,
    mixedEol: false,
    eol: "LF",
    looksLikeUtf8: false,
  }
  const kinds = (facts: Partial<NoticeFacts>, dismissed = () => false) =>
    encodingNotices("a.txt", { ...plain, ...facts }, dismissed).map((n) => n.kind)

  it("says nothing about a plain file", () => {
    expect(kinds({})).toEqual([])
  })

  it("raises each fact that decides what a save would do", () => {
    expect(kinds({ encoding: "windows-1252", encodingFallback: true })).toEqual(["fallback"])
    expect(kinds({ unwritable: "round_trip" })).toEqual(["unwritable"])
    expect(kinds({ unwritable: "doubled_mark" })).toEqual(["unwritable"])
    expect(kinds({ looksLikeUtf8: true })).toEqual(["looks_utf8"])
    expect(kinds({ mixedEol: true })).toEqual(["mixed_eol"])
  })

  it("leaves out the line endings of a file that cannot be saved anyway", () => {
    expect(kinds({ mixedEol: true, unwritable: "round_trip" })).toEqual(["unwritable"])
  })

  it("names the file as a chip and the style a save writes", () => {
    const [notice] = encodingNotices(
      "a.txt",
      { ...plain, mixedEol: true, eol: "CRLF" },
      () => false,
    )
    expect(notice.message[0]).toEqual({ name: "a.txt", quoted: false })
    expect(notice.message.filter((s) => typeof s === "string").join("")).toContain(
      "as CRLF",
    )
  })

  it("can dismiss everything but the read-only reason", () => {
    expect(kinds({ unwritable: "round_trip", looksLikeUtf8: true }, () => true)).toEqual([
      "unwritable",
    ])
  })

  it("is dismissed per file, per root and per kind", () => {
    dismissNotice("agent:1", "a.txt", "fallback")
    expect(noticeDismissed("agent:1", "a.txt", "fallback")).toBe(true)
    expect(noticeDismissed("agent:1", "a.txt", "mixed_eol")).toBe(false)
    expect(noticeDismissed("agent:1", "b.txt", "fallback")).toBe(false)
    expect(noticeDismissed("agent:2", "a.txt", "fallback")).toBe(false)
  })
})
