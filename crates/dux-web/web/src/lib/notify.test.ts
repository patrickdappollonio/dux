import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// `lib/notify.ts` is the only module in the app allowed to import sonner, so it
// is also the only module a sonner spy has to be installed under. Everything
// else in the web UI reaches sonner THROUGH here, which is what
// `notifyBoundary.test.ts` pins.
vi.mock("sonner", () => {
  const toast = Object.assign(vi.fn(), {
    success: vi.fn(),
    info: vi.fn(),
    error: vi.fn(),
    warning: vi.fn(),
    loading: vi.fn(),
    dismiss: vi.fn(),
  })
  return { toast }
})

const { toast } = await import("sonner")

const {
  BUSY_TOAST_MAX_MS,
  DEFAULT_STATUS_CLEAR_SECONDS,
  dismissNotification,
  notifyBusy,
  notifyError,
  notifyInfo,
  notifyStatus,
  notifySuccess,
  notifyWarning,
  setStatusClearSeconds,
  statusToastDuration,
} = await import("./notify")

beforeEach(() => {
  vi.clearAllMocks()
  // Every test states the window it expects; the module keeps it across tests.
  setStatusClearSeconds(undefined)
})

describe("statusToastDuration", () => {
  it("gives info/success the plain configured window", () => {
    expect(statusToastDuration("info", 6)).toBe(6000)
    expect(statusToastDuration("success", 6)).toBe(6000)
  })

  it("gives a warning three times the info window so it survives a glance away", () => {
    // The same factor the status line uses: `WARNING_CLEAR_FACTOR` in
    // `crates/dux-core/src/statusline.rs`.
    expect(statusToastDuration("warning", 6)).toBe(18000)
  })

  it("gives an error four times the info window, the longest final", () => {
    expect(statusToastDuration("error", 6)).toBe(24000)
  })

  it("orders the finals info < warning < error at any configured window", () => {
    for (const secs of [1, 6, 10, 30]) {
      const info = statusToastDuration("info", secs)
      const warning = statusToastDuration("warning", secs)
      const error = statusToastDuration("error", secs)
      expect(info).toBeLessThan(warning)
      expect(warning).toBeLessThan(error)
    }
  })

  it("scales every final off the user's status_clear_seconds", () => {
    expect(statusToastDuration("info", 10)).toBe(10000)
    expect(statusToastDuration("warning", 10)).toBe(30000)
    expect(statusToastDuration("error", 10)).toBe(40000)
  })

  it("falls back to the 6s default before the bootstrap document lands", () => {
    expect(DEFAULT_STATUS_CLEAR_SECONDS).toBe(6)
    expect(statusToastDuration("info", null)).toBe(6000)
    expect(statusToastDuration("info", undefined)).toBe(6000)
    expect(statusToastDuration("error", null)).toBe(24000)
  })

  it("treats status_clear_seconds of 0 as the user opting every final out of auto-clear", () => {
    expect(statusToastDuration("info", 0)).toBe(Infinity)
    expect(statusToastDuration("warning", 0)).toBe(Infinity)
    expect(statusToastDuration("error", 0)).toBe(Infinity)
  })

  it("caps a busy toast at the leak guard, never at the configured window", () => {
    // The guard is a safety net for a dropped socket, not a readability window,
    // so it is independent of status_clear_seconds and survives the 0 opt-out.
    expect(statusToastDuration("busy", 6)).toBe(BUSY_TOAST_MAX_MS)
    expect(statusToastDuration("busy", 30)).toBe(BUSY_TOAST_MAX_MS)
    expect(statusToastDuration("busy", 0)).toBe(BUSY_TOAST_MAX_MS)
  })

  it("keeps the busy guard well clear of the engine's 20s BUSY_TIMEOUT", () => {
    // dux_core::statusline::BUSY_TIMEOUT upgrades a stranded keyed Busy to a
    // Warning after 20s, and that upgrade replaces the toast in place. The
    // client guard must never fire first or a live operation would appear to
    // stop on its own.
    expect(BUSY_TOAST_MAX_MS).toBeGreaterThan(20_000 * 2)
  })

  it("never returns a zero or negative duration, which sonner reads as 'use the default'", () => {
    for (const tone of ["info", "success", "warning", "error", "busy"]) {
      for (const secs of [0, 1, 6, 60]) {
        expect(statusToastDuration(tone, secs)).toBeGreaterThan(0)
      }
    }
  })
})

describe("the configured window is read at RAISE time, not captured by the caller", () => {
  // The trap this removes is documented in CLAUDE.md's clipboard-paste tenet: a
  // raise registered in a mount effect that captures `status_clear_seconds`
  // out of the render closure pins every later toast from that component to
  // the pre-bootstrap default. With the window living
  // here and read on the way past, a caller has nothing to capture and nothing
  // to get wrong.
  it("uses the default until the bootstrap document lands", () => {
    notifyError("nope")
    expect(toast.error).toHaveBeenCalledWith("nope", { duration: 24000 })
  })

  it("follows a later setStatusClearSeconds with no caller involvement", () => {
    setStatusClearSeconds(10)
    notifySuccess("fine")
    expect(toast.success).toHaveBeenCalledWith("fine", { duration: 10000 })

    setStatusClearSeconds(0)
    notifySuccess("fine again")
    expect(toast.success).toHaveBeenLastCalledWith("fine again", {
      duration: Infinity,
    })
  })

  it("honours the documented 0 opt-out for every client-raised final", () => {
    setStatusClearSeconds(0)
    notifyWarning("careful")
    expect(toast.warning).toHaveBeenCalledWith("careful", { duration: Infinity })
  })
})

describe("an id means REPLACEMENT, so a repeat-prone notification carries none", () => {
  // sonner only resets a toast's remaining time when its DURATION changes, and
  // its close-timer effect re-runs on every re-raise. Re-raising the same
  // message on a FIXED id therefore restarts the countdown: copying text more
  // often than the window is long pinned "Copied to clipboard" open
  // indefinitely (measured at 90 seconds across 30 re-raises). Leaving the id
  // off makes every raise its own event on its own clock.
  it("passes no id when the caller gives none, so repeats never share a clock", () => {
    notifySuccess("Copied to clipboard")
    notifySuccess("Copied to clipboard")
    notifySuccess("Copied to clipboard")

    expect(toast.success).toHaveBeenCalledTimes(3)
    for (const call of vi.mocked(toast.success).mock.calls) {
      expect(call[1]).not.toHaveProperty("id")
    }
  })

  it("passes the id straight through when the caller means to replace", () => {
    notifyError("boom", { id: "drop-report" })
    expect(toast.error).toHaveBeenCalledWith("boom", {
      id: "drop-report",
      duration: 24000,
    })
  })
})

describe("sticky", () => {
  // A sticky notification is one the user must ACT on outside the toast to
  // recover from, or one where something may have been lost. It waits for them
  // instead of for a clock.
  it("gives any tone an infinite duration", () => {
    notifyError("saved but not delivered", { sticky: true })
    expect(toast.error).toHaveBeenCalledWith("saved but not delivered", {
      duration: Infinity,
    })
  })

  it("outranks the configured window, however short", () => {
    setStatusClearSeconds(1)
    notifyWarning("half done", { sticky: true })
    expect(toast.warning).toHaveBeenCalledWith("half done", {
      duration: Infinity,
    })
  })

  it("is off by default", () => {
    setStatusClearSeconds(6)
    notifySuccess("all good")
    expect(toast.success).toHaveBeenCalledWith("all good", { duration: 6000 })
  })

  it("rides an engine status too, and an absent flag means not sticky", () => {
    setStatusClearSeconds(6)
    notifyStatus("error", "server says stop", { id: "k", sticky: true })
    expect(toast.error).toHaveBeenCalledWith("server says stop", {
      id: "k",
      duration: Infinity,
    })

    notifyStatus("error", "ordinary", { id: "k2" })
    expect(toast.error).toHaveBeenLastCalledWith("ordinary", {
      id: "k2",
      duration: 24000,
    })
  })

  it("does not apply to a busy toast, which is not a final state", () => {
    // A spinner never waits for the user: it is replaced by its final, and the
    // leak guard is what retires it if that final never comes.
    notifyStatus("busy", "working", { id: "b", sticky: true })
    expect(toast.loading).toHaveBeenCalledWith("working", {
      id: "b",
      duration: BUSY_TOAST_MAX_MS,
    })
  })
})

describe("tone routing", () => {
  it("sends each client tone to its own sonner tone", () => {
    notifyInfo("i")
    notifySuccess("s")
    notifyWarning("w")
    notifyError("e")
    expect(toast.info).toHaveBeenCalledWith("i", { duration: 6000 })
    expect(toast.success).toHaveBeenCalledWith("s", { duration: 6000 })
    expect(toast.warning).toHaveBeenCalledWith("w", { duration: 18000 })
    expect(toast.error).toHaveBeenCalledWith("e", { duration: 24000 })
  })

  it("shows an engine `info` with the success icon, because the engine has no success tone", () => {
    // `dux_core`'s status line reports a finished operation as Info: "Pulled.",
    // "Changes committed successfully." Showing those with the informational
    // icon would demote every good outcome the engine reports. A client that
    // means "informational" still gets the informational icon, because it had
    // `notifySuccess` available and did not pick it.
    notifyStatus("info", "Pulled.", { id: "pull" })
    expect(toast.success).toHaveBeenCalledWith("Pulled.", {
      id: "pull",
      duration: 6000,
    })
    expect(toast.info).not.toHaveBeenCalled()
  })

  it("drops an empty message rather than raising a blank toast", () => {
    notifyError("")
    notifyStatus("info", "", { id: "x" }) // an engine info, shown as a success
    expect(toast.error).not.toHaveBeenCalled()
    expect(toast.info).not.toHaveBeenCalled()
    expect(toast.success).not.toHaveBeenCalled()
  })

  it("treats an unknown engine tone as the plain success window", () => {
    // The wire tone is a string the server picks; a tone this build has never
    // heard of must still be readable rather than crashing or vanishing.
    notifyStatus("brand-new-tone", "hello", { id: "n" })
    expect(toast.success).toHaveBeenCalledWith("hello", {
      id: "n",
      duration: 6000,
    })
  })
})

describe("the busy leak guard", () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it("retires a spinner whose final never arrives", () => {
    notifyBusy("Saving shot.png", { id: "drop-1" })
    vi.advanceTimersByTime(BUSY_TOAST_MAX_MS - 1)
    expect(toast.dismiss).not.toHaveBeenCalled()
    vi.advanceTimersByTime(1)
    expect(toast.dismiss).toHaveBeenCalledWith("drop-1")
  })

  // The consequence is concrete and is the reason the cancellation exists: with
  // `ui.status_clear_seconds = 0` a final toast is given an infinite duration,
  // so nothing else will ever take it off the screen. A guard left armed from
  // the spinner it replaced would fire 60s later and dismiss the user's
  // permanent report out from under them, and a drop report is often the error
  // listing which files were refused.
  it("is disarmed by the final that replaces the spinner, so a permanent report survives", () => {
    setStatusClearSeconds(0)
    notifyBusy("Saving shot.png", { id: "drop-1" })
    notifyError("Could not save shot.png", { id: "drop-1" })

    expect(toast.error).toHaveBeenCalledWith("Could not save shot.png", {
      id: "drop-1",
      duration: Infinity,
    })

    // Well past the guard's window. Nothing may dismiss the final.
    vi.advanceTimersByTime(BUSY_TOAST_MAX_MS * 2)
    expect(toast.dismiss).not.toHaveBeenCalled()
  })

  it("is disarmed for an ordinary auto-clearing report too", () => {
    setStatusClearSeconds(6)
    notifyBusy("Saving shot.png", { id: "drop-2" })
    notifySuccess("Saved shot.png and sent its path.", { id: "drop-2" })

    vi.advanceTimersByTime(BUSY_TOAST_MAX_MS * 2)
    expect(toast.dismiss).not.toHaveBeenCalled()
  })

  it("leaves a spinner on another id guarded, so the cancellation is never a blanket clear", () => {
    notifyBusy("Saving a.png", { id: "drop-a" })
    notifyBusy("Saving b.png", { id: "drop-b" })
    notifySuccess("Saved a.png.", { id: "drop-a" })

    vi.advanceTimersByTime(BUSY_TOAST_MAX_MS * 2)
    expect(toast.dismiss).toHaveBeenCalledTimes(1)
    expect(toast.dismiss).toHaveBeenCalledWith("drop-b")
  })

  it("re-arms on a replacing spinner, so only the newest one is ever dismissed", () => {
    notifyBusy("step one", { id: "s" })
    vi.advanceTimersByTime(BUSY_TOAST_MAX_MS - 5)
    notifyBusy("step two", { id: "s" })
    vi.advanceTimersByTime(10)
    // The first guard would have fired by now had it not been disarmed.
    expect(toast.dismiss).not.toHaveBeenCalled()
    vi.advanceTimersByTime(BUSY_TOAST_MAX_MS)
    expect(toast.dismiss).toHaveBeenCalledTimes(1)
  })

  it("disarms the guard when the caller dismisses the notification itself", () => {
    notifyBusy("working", { id: "d" })
    dismissNotification("d")
    expect(toast.dismiss).toHaveBeenCalledWith("d")
    vi.advanceTimersByTime(BUSY_TOAST_MAX_MS * 2)
    // Exactly the caller's dismissal, never a second one from a stale guard.
    expect(toast.dismiss).toHaveBeenCalledTimes(1)
  })
})
