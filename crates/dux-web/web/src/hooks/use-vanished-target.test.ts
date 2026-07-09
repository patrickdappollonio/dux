// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest"
import { renderHook } from "@testing-library/react"

import { useVanishedTargetGuard } from "./use-vanished-target"

describe("useVanishedTargetGuard", () => {
  it("returns true and never calls close while present", () => {
    const close = vi.fn()
    const { result } = renderHook(() => useVanishedTargetGuard(true, true, close))
    expect(result.current).toBe(true)
    expect(close).not.toHaveBeenCalled()
  })

  it("returns false and calls close once when target set but not present", () => {
    const close = vi.fn()
    const { result } = renderHook(() =>
      useVanishedTargetGuard(true, false, close),
    )
    expect(result.current).toBe(false)
    expect(close).toHaveBeenCalledTimes(1)
  })

  it("does not call close when no target is set regardless of present", () => {
    const close = vi.fn()
    const { result: notPresent } = renderHook(() =>
      useVanishedTargetGuard(false, false, close),
    )
    expect(notPresent.current).toBe(false)
    expect(close).not.toHaveBeenCalled()

    const { result: present } = renderHook(() =>
      useVanishedTargetGuard(false, true, close),
    )
    expect(present.current).toBe(false)
    expect(close).not.toHaveBeenCalled()
  })

  it("close fires when present flips from true to false across a rerender", () => {
    const close = vi.fn()
    const { rerender } = renderHook(
      ({ present }) => useVanishedTargetGuard(true, present, close),
      { initialProps: { present: true } },
    )
    expect(close).not.toHaveBeenCalled()
    rerender({ present: false })
    expect(close).toHaveBeenCalledTimes(1)
  })
})
