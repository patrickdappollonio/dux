import { describe, expect, it } from "vitest"

import {
  currentRunStamp,
  noteServerRunProbe,
  onServerRunChanged,
  onServerRunUnconfirmed,
  runIdentityConfirmedAs,
} from "./serverRun"

describe("the run-identity fan-out", () => {
  it("says nothing at all when the run is the same", () => {
    const seen: string[] = []
    const offA = onServerRunChanged(() => seen.push("changed"))
    const offB = onServerRunUnconfirmed(() => seen.push("unconfirmed"))
    noteServerRunProbe("same")
    offA()
    offB()
    expect(seen).toEqual([])
  })

  it("tells both audiences about a confirmed change", () => {
    const seen: string[] = []
    const offA = onServerRunChanged(() => seen.push("changed"))
    const offB = onServerRunUnconfirmed(() => seen.push("unconfirmed"))
    noteServerRunProbe("changed")
    offA()
    offB()
    expect(seen).toEqual(["changed", "unconfirmed"])
  })

  // The asymmetry the module exists for: an unproven answer must not cost a
  // returning driver its ghosts, but it must retire a replay high-water mark.
  it("tells only the unconfirmed audience about a probe that could not answer", () => {
    const seen: string[] = []
    const offA = onServerRunChanged(() => seen.push("changed"))
    const offB = onServerRunUnconfirmed(() => seen.push("unconfirmed"))
    noteServerRunProbe("unknown")
    offA()
    offB()
    expect(seen).toEqual(["unconfirmed"])
  })

  it("stops calling a listener that unsubscribed", () => {
    let calls = 0
    const off = onServerRunUnconfirmed(() => {
      calls++
    })
    off()
    noteServerRunProbe("changed")
    expect(calls).toBe(0)
  })
})

// THE STAMP, and the question it answers: may a memory learned under one run of
// the server still be acted on now? Only a CONFIRMED sameness says yes.
describe("the run-identity stamp", () => {
  it("confirms a stamp taken under the run the probe says is the same one", () => {
    noteServerRunProbe("same")
    const stamp = currentRunStamp()
    noteServerRunProbe("same")
    expect(runIdentityConfirmedAs(stamp)).toBe(true)
  })

  it("refuses a stamp while the probe could not answer", () => {
    noteServerRunProbe("same")
    const stamp = currentRunStamp()
    noteServerRunProbe("unknown")
    expect(runIdentityConfirmedAs(stamp)).toBe(false)
  })

  // A later answer of "same" compares against the boot baseline, so it proves
  // the run never moved and the older stamp is good again.
  it("confirms the same stamp again once a probe answers", () => {
    noteServerRunProbe("same")
    const stamp = currentRunStamp()
    noteServerRunProbe("unknown")
    noteServerRunProbe("same")
    expect(runIdentityConfirmedAs(stamp)).toBe(true)
  })

  it("refuses a stamp from before a confirmed change, forever", () => {
    noteServerRunProbe("same")
    const stamp = currentRunStamp()
    noteServerRunProbe("changed")
    expect(runIdentityConfirmedAs(stamp)).toBe(false)
    noteServerRunProbe("same")
    expect(runIdentityConfirmedAs(stamp)).toBe(false)
    expect(runIdentityConfirmedAs(currentRunStamp())).toBe(true)
  })
})
