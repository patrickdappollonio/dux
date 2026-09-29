// Drives the favicon's attention blink: which frame to show and when, on the
// rhythm the sidebar row's dot plays (`attentionPulse.ts`).
//
// WHY A WORKER. The favicon matters most in a tab you are NOT looking at, and
// that is exactly where browsers throttle a page's timers:
// - Chrome aligns a hidden page's timers to one wake per second, and after five
//   minutes hidden (with a timer chain five deep and no audio) to one wake per
//   minute ("intensive throttling", Chrome 88):
//   https://developer.chrome.com/blog/timer-throttling-in-chrome-88
// - MDN, setTimeout "Reasons for delays longer than specified": Firefox desktop
//   holds inactive tabs to a 1 s minimum and Chrome applies the tiers above:
//   https://developer.mozilla.org/en-US/docs/Web/API/Window/setTimeout
// Those policies are written for the page's (Window) timers; neither page says
// anything about dedicated workers, and the Chromium intent that shipped
// intensive throttling scopes it to timers in Windows
// (https://groups.google.com/a/chromium.org/g/blink-dev/c/8En_5DqV_fU). In
// practice a dedicated worker's timers keep their cadence in a hidden tab (the
// behaviour the `worker-timers` library is built on), and the tick they post
// back arrives as a message, which timer throttling does not delay. So the clock
// runs in a worker, with the page's own timers as the fallback when a worker
// cannot be made.
//
// WHY TWO MODES. None of that is a guarantee, so a hidden page does not bet the
// visible result on it: while hidden the favicon plays the `steady` on/off blink
// over the same period, which still alternates on every wake of a clock held to
// one per second, where the two quick dips would be sampled away. On a visible
// page (whose favicon is in the tab strip too) it plays the full rhythm.

import {
  attentionFrameAt,
  type AttentionFrame,
} from "./attentionPulse"

/** One pending wake at a time; `set` replaces whatever was pending. */
export interface WakeTimer {
  set(delayMs: number, fire: () => void): void
  clear(): void
  /** Release the timer for good (terminates a worker). */
  dispose(): void
}

/** A wake timer on the page's own `setTimeout`. */
export function windowWakeTimer(): WakeTimer {
  let handle: ReturnType<typeof setTimeout> | undefined
  const clear = () => {
    if (handle !== undefined) clearTimeout(handle)
    handle = undefined
  }
  return {
    set(delayMs, fire) {
      clear()
      handle = setTimeout(() => {
        handle = undefined
        fire()
      }, delayMs)
    },
    clear,
    dispose: clear,
  }
}

/** A wake timer whose clock runs in a dedicated worker, or `null` when this
 * browser cannot make one. */
export function workerWakeTimer(): WakeTimer | null {
  if (typeof Worker === "undefined") return null
  let worker: Worker
  try {
    worker = new Worker(new URL("./faviconBlink.worker.ts", import.meta.url), {
      type: "module",
    })
  } catch {
    return null
  }
  // Each request gets a fresh id and only the latest one's tick fires, so a
  // tick already in flight when the page moved on is dropped.
  let seq = 0
  let wanted: { id: number; due: number; fire: () => void } | null = null
  let disposed = false
  // Set once the worker has failed; from then on every wake runs here.
  let fallback: WakeTimer | null = null
  worker.onmessage = (ev: MessageEvent<{ type: string; id: number }>) => {
    if (disposed || fallback || ev.data?.type !== "tick") return
    if (!wanted || ev.data.id !== wanted.id) return
    const { fire } = wanted
    wanted = null
    fire()
  }
  // A script that cannot load (a 404, the network, a worker-src policy) fails
  // after construction, asynchronously. Drop the worker and carry the pending
  // wake over to the page's timers at its original due time, so the schedule
  // continues and nothing the dead worker still delivers fires it twice.
  const failOver = () => {
    if (disposed || fallback) return
    worker.terminate()
    fallback = windowWakeTimer()
    const pending = wanted
    wanted = null
    if (pending) fallback.set(Math.max(0, pending.due - Date.now()), pending.fire)
  }
  worker.onerror = failOver
  worker.onmessageerror = failOver
  return {
    set(delayMs, fire) {
      if (disposed) return
      if (fallback) {
        fallback.set(delayMs, fire)
        return
      }
      seq += 1
      wanted = { id: seq, due: Date.now() + delayMs, fire }
      worker.postMessage({ type: "set", id: seq, delay: delayMs })
    },
    clear() {
      if (disposed) return
      if (fallback) {
        fallback.clear()
        return
      }
      wanted = null
      worker.postMessage({ type: "clear" })
    },
    dispose() {
      if (disposed) return
      disposed = true
      wanted = null
      if (fallback) fallback.dispose()
      else worker.terminate()
    },
  }
}

/** The worker clock where one can be made, the page's timers otherwise. */
export function createWakeTimer(): WakeTimer {
  return workerWakeTimer() ?? windowWakeTimer()
}

export interface AttentionBlink {
  /** Re-read `hidden` and reschedule from the current moment. */
  resync(): void
  /** Stop for good and dispose the timer. No frame is shown after this. */
  stop(): void
}

/**
 * Start blinking: shows the `on` frame at once, then each frame change on the
 * shared schedule. Frames are chosen from elapsed wall-clock time, so a late
 * wake shows the frame that is due now rather than replaying missed ones.
 */
export function startAttentionBlink(opts: {
  timer: WakeTimer
  show: (frame: AttentionFrame) => void
  hidden: () => boolean
  now?: () => number
}): AttentionBlink {
  const now = opts.now ?? (() => Date.now())
  const started = now()
  let last: AttentionFrame | null = null
  let stopped = false

  const tick = () => {
    if (stopped) return
    const { frame, msUntilChange } = attentionFrameAt(
      now() - started,
      opts.hidden() ? "steady" : "rhythm",
    )
    if (frame !== last) {
      last = frame
      opts.show(frame)
    }
    opts.timer.set(msUntilChange, tick)
  }
  tick()

  return {
    resync() {
      if (stopped) return
      opts.timer.clear()
      tick()
    },
    stop() {
      if (stopped) return
      stopped = true
      opts.timer.dispose()
    },
  }
}
