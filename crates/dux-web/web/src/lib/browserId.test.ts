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

// Two tabs opened at once can both find storage empty and both mint. Whichever
// write lands last is the browser's id, and both tabs must end up presenting it.
describe("browserId when two tabs mint at once", () => {
  const OTHER = "b".repeat(32)

  function otherTabWrites(value: string, previous: string | null) {
    localStorage.setItem(BROWSER_ID_KEY, value)
    window.dispatchEvent(
      new StorageEvent("storage", {
        key: BROWSER_ID_KEY,
        oldValue: previous,
        newValue: value,
      }),
    )
  }

  it("adopts the other tab's id when its write lands right after this tab's", () => {
    const setItem = Storage.prototype.setItem
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(function (
      this: Storage,
      key: string,
      value: string,
    ) {
      setItem.call(this, key, value)
      setItem.call(this, key, OTHER)
    })
    expect(browserId()).toBe(OTHER)
  })

  it("adopts the other tab's id when its write lands later", () => {
    const mine = browserId()
    otherTabWrites(OTHER, mine)
    expect(browserId()).toBe(OTHER)
  })

  it("keeps the stored id when a stale write's event arrives after a newer one", () => {
    // This tab wrote last, so storage holds its id; the event for the other
    // tab's earlier, losing write is delivered afterwards.
    const mine = browserId()
    window.dispatchEvent(
      new StorageEvent("storage", { key: BROWSER_ID_KEY, oldValue: null, newValue: OTHER }),
    )
    expect(browserId()).toBe(mine)
  })

  it("ignores another key's change and a value the server would refuse", () => {
    const mine = browserId()
    otherTabWrites("not valid!", mine)
    expect(browserId()).toBe(mine)
    window.dispatchEvent(new StorageEvent("storage", { key: "dux:other", newValue: OTHER }))
    expect(browserId()).toBe(mine)
  })
})
