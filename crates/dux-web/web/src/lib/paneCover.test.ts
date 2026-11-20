// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest"
import { renderHook } from "@testing-library/react"

import {
  agentRowKey,
  registerPaneCover,
  registerRowClaim,
  resetPaneCovers,
  rowClaimSpent,
  rowDrivenElsewhereFor,
  rowWatchedFor,
  usePaneCoverOwned,
} from "./paneCover"

afterEach(() => resetPaneCovers())

describe("the pane cover registry", () => {
  it("answers per pty id, and reads uncovered for one nobody published", () => {
    registerPaneCover("tab-a", "box")
    registerPaneCover("tab-b", "spinner")
    expect(renderHook(() => usePaneCoverOwned("tab-a")).result.current).toBe(true)
    expect(renderHook(() => usePaneCoverOwned("tab-b")).result.current).toBe(false)
    expect(renderHook(() => usePaneCoverOwned("tab-c")).result.current).toBe(false)
    expect(renderHook(() => usePaneCoverOwned(null)).result.current).toBe(false)
  })

  it("re-renders a reader when the pane's verdict changes", () => {
    const retire = registerPaneCover("tab-a", "spinner")
    const { result, rerender } = renderHook(() => usePaneCoverOwned("tab-a"))
    expect(result.current).toBe(false)
    retire()
    registerPaneCover("tab-a", "box")
    rerender()
    expect(result.current).toBe(true)
  })

  it("keeps a replacement pane's verdict through the outgoing pane's cleanup", () => {
    // React does not order an old cleanup before a new effect, so a late
    // retirement must not clear the entry the successor has already written.
    // Same verdict on both sides deliberately: a registration is told apart by
    // its own identity, never by the value it happens to carry.
    const stale = registerPaneCover("tab-a", "box")
    registerPaneCover("tab-a", "box")
    stale()
    expect(renderHook(() => usePaneCoverOwned("tab-a")).result.current).toBe(true)
  })
})

describe("a row's answer", () => {
  const row = agentRowKey("s1")

  it("is watched on a card the server confirmed, and not on a pane's own guess", () => {
    // A pane mounted while the page was hidden guesses it does not drive the
    // pty and shows a card before any handshake; that guess is its own.
    const guess = registerPaneCover("s1", "card", { row, driver: null })
    registerPaneCover("t1", "none", { row, driver: "this" })
    expect(rowWatchedFor(row)).toBe(false)
    expect(rowDrivenElsewhereFor(row)).toBe(false)
    guess()
    registerPaneCover("s1", "card", { row, driver: "other" })
    expect(rowWatchedFor(row)).toBe(true)
    expect(rowDrivenElsewhereFor(row)).toBe(true)
  })

  it("says nobody drives it under the card's Running in the background", () => {
    registerPaneCover("s1", "card", { row, driver: "nobody" })
    expect(rowWatchedFor(row)).toBe(true)
    expect(rowDrivenElsewhereFor(row)).toBe(false)
  })

  it("leaves the question to the spine while no pane of it has heard the server", () => {
    registerPaneCover("s1", "spinner", { row, driver: null })
    expect(rowDrivenElsewhereFor(row)).toBeUndefined()
    expect(rowDrivenElsewhereFor(agentRowKey("s2"))).toBeUndefined()
  })

  it("is driven from the press until the other part hears the server", () => {
    registerPaneCover("s1", "card", { row })
    registerPaneCover("t1", "card", { row })
    registerRowClaim(row, "t1")
    // The pressed pane bounces its socket and believes it drives the pty.
    registerPaneCover("t1", "spinner", { row, driver: "this" })
    expect(rowWatchedFor(row)).toBe(false)
    expect(rowDrivenElsewhereFor(row)).toBe(false)
    expect(rowClaimSpent(row)).toBe(false)
    registerPaneCover("s1", "none", { row, driver: "this" })
    expect(rowClaimSpent(row)).toBe(true)
  })

  it.each([
    ["back under the card", "card"],
    ["lost behind the Reconnect box", "box"],
  ] as const)("lets the claim go when the pressed pane is %s", (_case, kind) => {
    registerPaneCover("s1", "card", { row })
    registerRowClaim(row, "t1")
    registerPaneCover("t1", kind, { row })
    expect(rowClaimSpent(row)).toBe(true)
    expect(rowWatchedFor(row)).toBe(true)
  })

  it("lets the claim go when the pressed pane is gone", () => {
    registerPaneCover("s1", "card", { row })
    const pressed = registerPaneCover("t1", "spinner", { row, driver: "this" })
    registerRowClaim(row, "t1")
    expect(rowClaimSpent(row)).toBe(false)
    pressed()
    expect(rowClaimSpent(row)).toBe(true)
    expect(rowWatchedFor(row)).toBe(true)
  })

  it("covers a part of the row that has no pane, such as a dormant tab", () => {
    registerPaneCover("t1", "card", { row })
    expect(renderHook(() => usePaneCoverOwned("dormant-tab", row)).result.current).toBe(true)
    expect(renderHook(() => usePaneCoverOwned("dormant-tab")).result.current).toBe(false)
  })
})
