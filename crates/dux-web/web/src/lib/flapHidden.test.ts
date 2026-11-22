// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

type FlapHidden = typeof import("./flapHidden")

// Each import is a fresh page: the module keeps the page's own answer.
async function freshPage(): Promise<FlapHidden> {
  vi.resetModules()
  return import("./flapHidden")
}

beforeEach(() => localStorage.clear())

afterEach(() => {
  vi.restoreAllMocks()
  localStorage.clear()
})

describe("the flap's show/hide on this device", () => {
  it("reads shown when nothing is stored", async () => {
    const flap = await freshPage()
    expect(flap.readFlapHidden()).toBe(false)
  })

  it("keeps a fold across a reload", async () => {
    const first = await freshPage()
    first.setFlapHidden(true)
    const reloaded = await freshPage()
    expect(reloaded.readFlapHidden()).toBe(true)
  })

  it("works for the page when storage is blocked", async () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new DOMException("blocked", "SecurityError")
    })
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new DOMException("blocked", "SecurityError")
    })
    const flap = await freshPage()
    expect(flap.readFlapHidden()).toBe(false)
    flap.setFlapHidden(true)
    expect(flap.readFlapHidden()).toBe(true)
  })

  it("follows the press when the write fails over an older stored value", async () => {
    const flap = await freshPage()
    flap.setFlapHidden(true)
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new DOMException("full", "QuotaExceededError")
    })
    flap.setFlapHidden(false)
    expect(flap.readFlapHidden()).toBe(false)
  })

  it("follows a change another tab makes, when it makes it", async () => {
    const flap = await freshPage()
    const listener = vi.fn()
    const unsubscribe = flap.subscribeFlapHidden(listener)
    expect(flap.readFlapHidden()).toBe(false)

    // The other tab's write lands in storage and fires `storage` here.
    localStorage.setItem(flap.FLAP_HIDDEN_KEY, "1")
    window.dispatchEvent(
      new StorageEvent("storage", {
        key: flap.FLAP_HIDDEN_KEY,
        newValue: "1",
        storageArea: localStorage,
      }),
    )
    expect(listener).toHaveBeenCalled()
    expect(flap.readFlapHidden()).toBe(true)
    unsubscribe()
  })

  it("does not change at a later read when another tab wrote with no event", async () => {
    const flap = await freshPage()
    expect(flap.readFlapHidden()).toBe(false)
    localStorage.setItem(flap.FLAP_HIDDEN_KEY, "1")
    expect(flap.readFlapHidden()).toBe(false)
  })
})
