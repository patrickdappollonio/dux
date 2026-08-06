import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { showBusyToast } from "./busyToast"
import { showFinalToast } from "./finalToast"
import { BUSY_TOAST_MAX_MS } from "./statusToast"

// The busy raiser and the final raiser are two modules that have to agree about
// one leak guard, and the only caller that exercises the handover between them
// is the file-drop report: it raises a spinner per file on one id and then
// raises its final on that same id, without knowing the guard exists. The store
// cannot stand in for it, because `showStatusToast` cancels the guard itself on
// the way past, so the whole 1740-test suite stayed green with the
// `cancelBusyToastGuard` call deleted from `showFinalToast`.
vi.mock("sonner", () => {
  const toast = Object.assign(vi.fn(), {
    success: vi.fn(),
    error: vi.fn(),
    warning: vi.fn(),
    loading: vi.fn(),
    dismiss: vi.fn(),
  })
  return { toast }
})

const { toast } = await import("sonner")

describe("a final toast raised over a spinner on the same id", () => {
  beforeEach(() => {
    vi.useFakeTimers()
    vi.clearAllMocks()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  // The consequence is concrete and is the reason the line exists: with
  // `ui.status_clear_seconds = 0` a final toast is given an infinite duration,
  // so nothing else will ever take it off the screen. A guard left armed from
  // the spinner it replaced would fire 60s later and dismiss the user's
  // permanent report out from under them, and a drop report is often the error
  // listing which files were refused.
  it("disarms the spinner's leak guard, so a permanent report is never dismissed", () => {
    showBusyToast("Saving shot.png", { id: "drop-1" })
    showFinalToast("error", "Could not save shot.png", {
      id: "drop-1",
      statusClearSeconds: 0,
    })

    expect(toast.error).toHaveBeenCalledWith("Could not save shot.png", {
      id: "drop-1",
      duration: Infinity,
    })

    // Well past the guard's window. Nothing may dismiss the final.
    vi.advanceTimersByTime(BUSY_TOAST_MAX_MS * 2)
    expect(toast.dismiss).not.toHaveBeenCalled()
  })

  // The same handover with the default window. The final still owns its own
  // dismissal; a stale guard firing early would cut the report short.
  it("disarms it for an ordinary auto-clearing report too", () => {
    showBusyToast("Saving shot.png", { id: "drop-2" })
    showFinalToast("success", "Saved shot.png and sent its path.", {
      id: "drop-2",
      statusClearSeconds: 6,
    })

    vi.advanceTimersByTime(BUSY_TOAST_MAX_MS * 2)
    expect(toast.dismiss).not.toHaveBeenCalled()
  })

  // The guard is per id, so a spinner still running on a DIFFERENT id keeps its
  // own guard: the cancellation must not be a blanket clear.
  it("leaves a spinner on another id guarded", () => {
    showBusyToast("Saving a.png", { id: "drop-a" })
    showBusyToast("Saving b.png", { id: "drop-b" })
    showFinalToast("success", "Saved a.png.", {
      id: "drop-a",
      statusClearSeconds: 6,
    })

    vi.advanceTimersByTime(BUSY_TOAST_MAX_MS * 2)
    expect(toast.dismiss).toHaveBeenCalledTimes(1)
    expect(toast.dismiss).toHaveBeenCalledWith("drop-b")
  })
})
