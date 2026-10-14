// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { BROWSER_ID_KEY, browserId, resetBrowserIdForTests } from "./browserId"

beforeEach(() => {
  localStorage.clear()
  resetBrowserIdForTests()
})

afterEach(() => {
  vi.restoreAllMocks()
  localStorage.clear()
  resetBrowserIdForTests()
})

describe("browserId", () => {
  it("mints a random id once and keeps it in storage for every tab of this browser", () => {
    const id = browserId()
    expect(id).toMatch(/^[0-9a-f]{32}$/)
    expect(localStorage.getItem(BROWSER_ID_KEY)).toBe(id)

    // Another tab of the same browser starts with an empty page cache.
    resetBrowserIdForTests()
    expect(browserId()).toBe(id)
  })

  it("replaces a stored value the server would refuse", () => {
    localStorage.setItem(BROWSER_ID_KEY, "not valid!")
    const id = browserId()
    expect(id).toMatch(/^[0-9a-f]{32}$/)
    expect(localStorage.getItem(BROWSER_ID_KEY)).toBe(id)
  })

  it("falls back to an id for this page alone when storage throws", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new DOMException("blocked", "SecurityError")
    })
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new DOMException("blocked", "SecurityError")
    })
    const id = browserId()
    expect(id).toMatch(/^[0-9a-f]{32}$/)
    expect(browserId()).toBe(id)

    // Another page, still without storage, counts as a browser of its own.
    resetBrowserIdForTests()
    expect(browserId()).not.toBe(id)
  })
})
