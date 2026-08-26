import { describe, expect, it } from "vitest"

import {
  noteServerRunProbe,
  onServerRunChanged,
  onServerRunUnconfirmed,
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
