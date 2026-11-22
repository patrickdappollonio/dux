// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"

import {
  LAYOUT_CHOICE_KEY,
  currentShellLayout,
  installShellLayoutAttribute,
  isPhoneDevice,
  readLayoutChoice,
  resolveShellLayout,
  setLayoutChoice,
  subscribeShellLayout,
} from "@/lib/shellLayout"
import { stubCoarsePointer } from "@/test/matchMedia"

// Real device numbers in CSS px. A landscape phone's screen keeps its short
// side whatever the browser chrome and the keyboard take from the viewport.
const PORTRAIT_PHONE = { width: 390, screenShortSide: 390, coarse: true }
const LANDSCAPE_PHONE = { width: 851, screenShortSide: 393, coarse: true }
const LANDSCAPE_IPHONE_MAX = { width: 932, screenShortSide: 430, coarse: true }
const TABLET_PORTRAIT = { width: 820, screenShortSide: 820, coarse: true }
const TABLET_LANDSCAPE = { width: 1180, screenShortSide: 820, coarse: true }
const SMALL_TABLET_LANDSCAPE = { width: 960, screenShortSide: 600, coarse: true }
const NARROW_COMPUTER = { width: 700, screenShortSide: 1080, coarse: false }
const SHORT_COMPUTER_WINDOW = { width: 1440, screenShortSide: 1080, coarse: false }

describe("resolveShellLayout", () => {
  it.each([
    ["a portrait phone", PORTRAIT_PHONE, "phone"],
    ["a landscape phone", LANDSCAPE_PHONE, "phone"],
    ["the largest landscape phone", LANDSCAPE_IPHONE_MAX, "phone"],
    ["a tablet in portrait", TABLET_PORTRAIT, "computer"],
    ["a tablet in landscape", TABLET_LANDSCAPE, "computer"],
    ["a small tablet in landscape", SMALL_TABLET_LANDSCAPE, "computer"],
    ["a narrow computer window", NARROW_COMPUTER, "phone"],
    ["a short computer window with a mouse", SHORT_COMPUTER_WINDOW, "computer"],
  ] as const)("chooses by itself for %s", (_name, device, expected) => {
    expect(resolveShellLayout({ ...device, choice: null })).toBe(expected)
  })

  it("reads an unknown screen size as not short", () => {
    expect(
      resolveShellLayout({ width: 1024, screenShortSide: 0, coarse: true, choice: null }),
    ).toBe("computer")
  })

  it.each([
    ["a landscape phone", LANDSCAPE_PHONE],
    ["a portrait phone", PORTRAIT_PHONE],
    ["a narrow computer window", NARROW_COMPUTER],
  ])("lets a person choose the computer layout on %s", (_name, device) => {
    expect(resolveShellLayout({ ...device, choice: "computer" })).toBe("computer")
  })

  it.each([
    ["a tablet in landscape", TABLET_LANDSCAPE],
    ["a computer", SHORT_COMPUTER_WINDOW],
  ])("lets a person choose the phone layout on %s", (_name, device) => {
    expect(resolveShellLayout({ ...device, choice: "phone" })).toBe("phone")
  })
})

describe("the device's layout choice", () => {
  afterEach(() => {
    setLayoutChoice(null)
    localStorage.clear()
    vi.unstubAllGlobals()
  })

  it("is kept in this browser and read back as a fresh read would", () => {
    setLayoutChoice("computer")
    expect(localStorage.getItem(LAYOUT_CHOICE_KEY)).toBe("computer")
    expect(readLayoutChoice()).toBe("computer")
    setLayoutChoice(null)
    expect(localStorage.getItem(LAYOUT_CHOICE_KEY)).toBeNull()
    expect(readLayoutChoice()).toBeNull()
  })

  it("forgets a choice another tab of this browser cleared", () => {
    setLayoutChoice("computer")
    localStorage.removeItem(LAYOUT_CHOICE_KEY)
    expect(readLayoutChoice()).toBeNull()
  })

  it("keeps the choice in memory when storage refuses to write", () => {
    const setItem = vi
      .spyOn(Storage.prototype, "setItem")
      .mockImplementation(() => {
        throw new Error("QuotaExceededError")
      })
    try {
      setLayoutChoice("phone")
      expect(readLayoutChoice()).toBe("phone")
    } finally {
      setItem.mockRestore()
    }
  })

  it("ignores a stored value it never wrote", () => {
    localStorage.setItem(LAYOUT_CHOICE_KEY, "tablet")
    expect(readLayoutChoice()).toBeNull()
  })

  it("wins over the window it is read in", () => {
    vi.stubGlobal("innerWidth", 390)
    expect(currentShellLayout()).toBe("phone")
    setLayoutChoice("computer")
    expect(currentShellLayout()).toBe("computer")
  })

  it("reads the coarse pointer and the screen for the automatic answer", () => {
    const media = stubCoarsePointer(true)
    try {
      vi.stubGlobal("innerWidth", 851)
      vi.stubGlobal("screen", { width: 851, height: 393 })
      expect(currentShellLayout()).toBe("phone")
      media.set("(pointer: coarse)", false)
      expect(currentShellLayout()).toBe("computer")
    } finally {
      media.restore()
    }
  })

  it("tells subscribers when the choice, the window or the pointer moves the answer", () => {
    const media = stubCoarsePointer(false)
    vi.stubGlobal("innerWidth", 851)
    vi.stubGlobal("screen", { width: 851, height: 393 })
    const seen = vi.fn()
    const stop = subscribeShellLayout(seen)
    try {
      setLayoutChoice("phone")
      setLayoutChoice(null)
      vi.stubGlobal("innerWidth", 390)
      window.dispatchEvent(new Event("resize"))
      vi.stubGlobal("innerWidth", 851)
      window.dispatchEvent(new Event("resize"))
      media.set("(pointer: coarse)", true)
      expect(seen).toHaveBeenCalledTimes(5)
    } finally {
      stop()
      media.restore()
    }
    setLayoutChoice("phone")
    expect(seen).toHaveBeenCalledTimes(5)
  })

  it("keeps a tablet on the computer layout while the keyboard takes the viewport's height", () => {
    const media = stubCoarsePointer(true)
    vi.stubGlobal("innerWidth", 1180)
    vi.stubGlobal("innerHeight", 760)
    vi.stubGlobal("screen", { width: 1180, height: 820 })
    const seen = vi.fn()
    const stop = subscribeShellLayout(seen)
    try {
      vi.stubGlobal("innerHeight", 400)
      window.dispatchEvent(new Event("resize"))
      expect(currentShellLayout()).toBe("computer")
      expect(seen).not.toHaveBeenCalled()
    } finally {
      stop()
      media.restore()
    }
  })

  it("listens once and answers from memory however many subscribe", () => {
    const media = stubCoarsePointer(false)
    const addListener = vi.spyOn(window, "addEventListener")
    const stops = Array.from({ length: 40 }, () => subscribeShellLayout(() => {}))
    const getItem = vi.spyOn(Storage.prototype, "getItem")
    try {
      for (let i = 0; i < 40; i++) currentShellLayout()
      window.dispatchEvent(new Event("resize"))
      expect(addListener.mock.calls.filter(([type]) => type === "resize")).toHaveLength(1)
      expect(getItem).toHaveBeenCalledTimes(1)
    } finally {
      for (const stop of stops) stop()
      addListener.mockRestore()
      getItem.mockRestore()
      media.restore()
    }
  })

  it("follows a choice another tab of this browser stored", () => {
    const seen = vi.fn()
    const stop = subscribeShellLayout(seen)
    try {
      localStorage.setItem(LAYOUT_CHOICE_KEY, "phone")
      window.dispatchEvent(new StorageEvent("storage", { key: "something-else" }))
      expect(seen).not.toHaveBeenCalled()
      window.dispatchEvent(new StorageEvent("storage", { key: LAYOUT_CHOICE_KEY }))
      expect(seen).toHaveBeenCalledTimes(1)
    } finally {
      stop()
    }
  })

  it.each([
    ["a computer in the phone layout", 1440, { width: 2560, height: 1440 }, false, "phone", false],
    ["a narrow computer window", 600, { width: 2560, height: 1440 }, false, null, false],
    ["a phone in the computer layout", 390, { width: 390, height: 844 }, true, "computer", true],
    ["a phone turned sideways", 851, { width: 851, height: 393 }, true, null, true],
    ["a tablet", 1180, { width: 1180, height: 820 }, true, null, false],
  ] as const)("tells a phone by its hardware, not its layout, on %s", (_n, width, screen, coarse, choice, phone) => {
    const media = stubCoarsePointer(coarse)
    try {
      vi.stubGlobal("innerWidth", width)
      vi.stubGlobal("screen", screen)
      setLayoutChoice(choice)
      expect(isPhoneDevice()).toBe(phone)
    } finally {
      media.restore()
    }
  })

  it("marks the document with the layout the classes style for", () => {
    vi.stubGlobal("innerWidth", 1440)
    const stop = installShellLayoutAttribute(document)
    try {
      expect(document.documentElement.dataset.layout).toBe("computer")
      setLayoutChoice("phone")
      expect(document.documentElement.dataset.layout).toBe("phone")
    } finally {
      stop()
      delete document.documentElement.dataset.layout
    }
  })
})
