// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest"
import { renderHook } from "@testing-library/react"

import { registerPaneCover, resetPaneCovers, usePaneCoverOwned } from "./paneCover"

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
