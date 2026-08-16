// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"

import {
  attachCapabilityFor,
  registerAttachCapability,
  resetAttachCapabilities,
} from "./attachRegistry"

afterEach(() => resetAttachCapabilities())

describe("attachRegistry", () => {
  it("publishes and retires a capability by pty id", () => {
    const attach = vi.fn()
    expect(attachCapabilityFor("s1")).toBeNull()
    const retire = registerAttachCapability("s1", attach)
    attachCapabilityFor("s1")!()
    expect(attach).toHaveBeenCalledTimes(1)
    retire()
    expect(attachCapabilityFor("s1")).toBeNull()
  })

  it("keeps ids apart", () => {
    const a = vi.fn()
    const b = vi.fn()
    registerAttachCapability("s1", a)
    registerAttachCapability("t1", b)
    attachCapabilityFor("t1")!()
    expect(a).not.toHaveBeenCalled()
    expect(b).toHaveBeenCalledTimes(1)
  })

  // CROSS-COMMIT REPLACEMENT: a second pane can register the same pty id
  // before the first pane unmounts, so the first pane's late cleanup must not
  // retire the live registration; an unconditional delete would, and the row
  // menu's item would vanish. (StrictMode's own order is setup, cleanup,
  // setup, which is naturally compatible with the guard.)
  it("a stale retirement does not remove the live registration", () => {
    const first = vi.fn()
    const second = vi.fn()
    const retireFirst = registerAttachCapability("s1", first)
    registerAttachCapability("s1", second)
    retireFirst()
    expect(attachCapabilityFor("s1")).not.toBeNull()
    attachCapabilityFor("s1")!()
    expect(second).toHaveBeenCalledTimes(1)
    expect(first).not.toHaveBeenCalled()
  })

  it("last write wins for one id", () => {
    const first = vi.fn()
    const second = vi.fn()
    registerAttachCapability("s1", first)
    registerAttachCapability("s1", second)
    attachCapabilityFor("s1")!()
    expect(second).toHaveBeenCalledTimes(1)
    expect(first).not.toHaveBeenCalled()
  })
})
