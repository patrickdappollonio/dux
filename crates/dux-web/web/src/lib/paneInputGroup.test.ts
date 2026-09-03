import { afterEach, describe, expect, it } from "vitest"

import {
  paneInputGroupFor,
  paneInputGroupHasItems,
  registerPaneInputGroup,
  resetPaneInputGroups,
} from "./paneInputGroup"

// The registry's rules, without a React tree: what a menu is handed, in what
// order it looks, and what a late cleanup may not take away.

afterEach(() => resetPaneInputGroups())

describe("paneInputGroupHasItems", () => {
  // The caller ORs its own attach in, so a group with neither of the pane's own
  // rows is nothing to put a label over.
  it("is false for an absent group and for one with no rows", () => {
    expect(paneInputGroupHasItems(null)).toBe(false)
    expect(
      paneInputGroupHasItems({ surfaceSwitch: false, keysToggle: false }),
    ).toBe(false)
    expect(
      paneInputGroupHasItems({ surfaceSwitch: true, keysToggle: false }),
    ).toBe(true)
    expect(
      paneInputGroupHasItems({ surfaceSwitch: false, keysToggle: true }),
    ).toBe(true)
  })
})

describe("the pane input-group registry", () => {
  const gates = (surfaceSwitch: boolean) => ({
    surfaceSwitch,
    keysToggle: false,
  })

  it("answers for the pane the menu asks about, and nobody else's", () => {
    registerPaneInputGroup("other", gates(true))
    expect(paneInputGroupFor(["mine"])).toBeNull()
    registerPaneInputGroup("mine", gates(false))
    expect(paneInputGroupFor(["mine"])).toEqual(gates(false))
  })

  // An agent asks with its slot id first and then every tab id, the same scan
  // and the same order the attach capability uses, so the group and the act
  // behind its attach item can never come from different panes.
  it("takes the first published pty in the order asked", () => {
    registerPaneInputGroup("tab-2", gates(true))
    expect(paneInputGroupFor(["s1", "tab-1", "tab-2"])).toEqual(gates(true))
    registerPaneInputGroup("s1", gates(false))
    expect(paneInputGroupFor(["s1", "tab-1", "tab-2"])).toEqual(gates(false))
  })

  it("retires only its own registration, so a replacement pane survives", () => {
    const first = gates(true)
    const retireFirst = registerPaneInputGroup("p", first)
    const second = gates(false)
    registerPaneInputGroup("p", second)
    // The outgoing pane's cleanup runs after the incoming pane registered.
    retireFirst()
    expect(paneInputGroupFor(["p"])).toBe(second)
  })

  it("removes the entry when the live registration retires", () => {
    const retire = registerPaneInputGroup("p", gates(true))
    retire()
    expect(paneInputGroupFor(["p"])).toBeNull()
  })
})
