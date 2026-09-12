import { Loader2, RefreshCw } from "lucide-react"
import { useEffect, useState } from "react"
import { createPortal } from "react-dom"

import { Button } from "@/components/ui/button"
import type { ReconnectPlanEvent } from "@/lib/reconnectingSocket"
import { reconnect, useDux } from "@/lib/store"

// The same ASCII "dux" wordmark `public/offline.html` shows, so the modal and the
// service-worker page read as one. Left-aligned in an inline-block, or centering
// shears each line independently.
const DUX_ART = `       ░██
       ░██
 ░████████ ░██    ░██ ░██    ░██
░██    ░██ ░██    ░██  ░██  ░██
░██    ░██ ░██    ░██   ░█████
░██   ░███ ░██   ░███  ░██  ░██
 ░█████░██  ░█████░██ ░██    ░██ `

/// How often the countdown's clock is re-read. See the effect that uses it.
const COUNTDOWN_SAMPLE_MS = 250

// Whole seconds until the next attempt, floored at zero. Wall clock, not a tick
// count, so the number is right however the render cadence behaves; ceiling, so
// the last visible second is "1 s" rather than a "0 s" that lingers.
function secondsUntil(at: number, now: number): number {
  return Math.max(0, Math.ceil((at - now) / 1000))
}

/// Seconds as the overlay says them: a number and a unit, never "a few".
function secs(ms: number): string {
  return `${Math.round(ms / 1000)} s`
}

/// The attempt count in words, because a budget of one is a real config.
function attemptsWord(n: number): string {
  return n === 1 ? "1 attempt" : `${n} attempts`
}

/// What the overlay says, derived from the socket's own plan. Three faces: one
/// for the gap between attempts, one for an attempt in flight, and one for a
/// loop that has ended. The button is in all three and is always instant, so
/// there is no face without a way out.
function copyFor(
  plan: ReconnectPlanEvent | null,
  now: number,
): { title: string; spinner: boolean; lines: string[]; button: string } {
  if (plan?.phase === "given_up") {
    return {
      title: "Not connected to dux",
      spinner: false,
      lines: [
        `dux stopped trying after ${attemptsWord(plan.attempt)}. Reconnect when the server is back, or check that this device is online.`,
      ],
      button: "Reconnect",
    }
  }
  const title = "Reconnecting to dux…"
  if (plan?.phase === "connecting") {
    return {
      title,
      spinner: true,
      lines: [
        plan.budget > 0
          ? `Attempt ${plan.attempt} of ${plan.budget}, connecting…`
          : `Attempt ${plan.attempt}, connecting…`,
        `If the server is unreachable this can take up to ${secs(plan.attemptTimeoutMs)}.`,
      ],
      button: "Reconnect now",
    }
  }
  // Waiting, and the pre-plan window: after the first drop but before the socket
  // has published anything there is no attempt to count, so the overlay says the
  // half it does know rather than inventing a number.
  const countable = plan !== null && plan.attempt >= 1 && plan.nextAttemptAt !== null
  const wait = countable
    ? `Retrying in ${secondsUntil(plan.nextAttemptAt as number, now)} s.`
    : ""
  const attempt = !countable
    ? null
    : plan.budget > 0
      ? `Attempt ${plan.attempt} of ${plan.budget} failed. ${wait}`
      : `Attempt ${plan.attempt} failed. ${wait}`
  return {
    title,
    spinner: true,
    lines: [
      ...(attempt === null ? [] : [attempt]),
      "The server may be down or this device may be offline.",
    ],
    button: "Reconnect now",
  }
}

// The sticky offline state avoids flicker between retries. Reconnection is
// budgeted while visible and parked while hidden; the button resets the budget
// and attempts at once, in every face.
export function OfflineOverlay() {
  const { offline, reconnectPlan } = useDux()
  const waiting =
    offline &&
    reconnectPlan !== null &&
    reconnectPlan.phase === "waiting" &&
    reconnectPlan.nextAttemptAt !== null
  // The clock behind the countdown, sampled only while a countdown is on screen:
  // the other two faces have no number that moves, and an interval behind them
  // would be a timer nobody reads. The DIGIT changes once a second; the sampling
  // is finer than that on purpose, because the clock cannot be read during a
  // render, so a plan landing between two samples would otherwise start its
  // countdown from a moment already past.
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!waiting) return
    const id = setInterval(() => setNow(Date.now()), COUNTDOWN_SAMPLE_MS)
    return () => clearInterval(id)
  }, [waiting])

  if (!offline) return null
  const copy = copyFor(reconnectPlan, now)

  return createPortal(
    <div
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="offline-overlay-title"
      aria-describedby="offline-overlay-desc"
      className="fixed inset-0 z-[100] flex items-center justify-center bg-background/40 p-6 backdrop-grayscale supports-backdrop-filter:backdrop-blur-sm"
    >
      <div className="w-full max-w-md rounded-xl border bg-card p-6 text-center text-card-foreground shadow-xl">
        <pre
          aria-hidden
          className="mx-auto mb-6 inline-block text-left font-blocks text-[11px] leading-[1.15] text-muted-foreground"
        >
          {DUX_ART}
        </pre>
        <h1
          id="offline-overlay-title"
          className="mb-1.5 flex items-center justify-center gap-2 text-lg font-semibold"
        >
          {copy.spinner ? (
            <Loader2
              className="size-4 animate-spin text-muted-foreground"
              aria-hidden
            />
          ) : null}
          {copy.title}
        </h1>
        <div
          id="offline-overlay-desc"
          aria-live="polite"
          className="mb-6 space-y-1 text-sm leading-relaxed text-muted-foreground"
        >
          {copy.lines.map((line) => (
            <p key={line}>{line}</p>
          ))}
        </div>
        <Button onClick={reconnect}>
          <RefreshCw aria-hidden />
          {copy.button}
        </Button>
      </div>
    </div>,
    document.body,
  )
}
