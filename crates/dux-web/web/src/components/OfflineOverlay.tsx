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

/// How often the countdown re-reads the clock. One second, because that is the
/// resolution it shows and the sampling starts from the moment the countdown
/// itself does.
const COUNTDOWN_SAMPLE_MS = 1000

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

/// The line that carries the countdown, mounted fresh for every attempt.
///
/// KEYED ON `at` BY ITS CALLER, and that key is the whole design. The clock
/// cannot be read during a render, so the moment a countdown starts has to come
/// from somewhere; a clock ticking in the parent is stale by however long the
/// previous face was on screen, which after a connecting face that sat out its
/// ten second deadline meant a first frame reading "Retrying in 11 s". A new
/// attempt is a new key, so this remounts and reads the clock in its own state
/// initializer, at the moment it is actually showing.
function CountdownLine({ prefix, at }: { prefix: string; at: number }) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), COUNTDOWN_SAMPLE_MS)
    return () => clearInterval(id)
  }, [])
  return <p>{`${prefix} Retrying in ${secondsUntil(at, now)} s.`}</p>
}

/// What the overlay says, derived from the socket's own plan. Three faces: one
/// for the gap between attempts, one for an attempt in flight, and one for a
/// loop that has ended. The button is in all three and is always instant, so
/// there is no face without a way out.
///
/// `counter` is the line that moves, and `at` on it is when the next attempt is
/// due, which is what the countdown counts to; `stable` is the sentence that
/// stays put for the whole face.
type OverlayFace = {
  title: string
  spinner: boolean
  counter: { text: string; at: number | null } | null
  stable: string
  button: string
}

function copyFor(plan: ReconnectPlanEvent | null): OverlayFace {
  if (plan?.phase === "given_up") {
    return {
      title: "Not connected to dux",
      spinner: false,
      counter: null,
      stable: `dux stopped trying after ${attemptsWord(plan.attempt)}. Reconnect when the server is back, or check that this device is online.`,
      button: "Reconnect",
    }
  }
  const title = "Reconnecting to dux…"
  const button = "Reconnect now"
  if (plan?.phase === "connecting") {
    return {
      title,
      spinner: true,
      counter: {
        text:
          plan.budget > 0
            ? `Attempt ${plan.attempt} of ${plan.budget}, connecting…`
            : `Attempt ${plan.attempt}, connecting…`,
        at: null,
      },
      stable: `If the server is unreachable this can take up to ${secs(plan.attemptTimeoutMs)}.`,
      button,
    }
  }
  // Waiting, and the pre-plan window: after the first drop but before the socket
  // has published anything there is no attempt to count, so the overlay says the
  // half it does know rather than inventing a number.
  const countable = plan !== null && plan.attempt >= 1 && plan.nextAttemptAt !== null
  return {
    title,
    spinner: true,
    counter: countable
      ? {
          text:
            plan.budget > 0
              ? `Attempt ${plan.attempt} of ${plan.budget} failed.`
              : `Attempt ${plan.attempt} failed.`,
          at: plan.nextAttemptAt,
        }
      : null,
    stable: "The server may be down or this device may be offline.",
    button,
  }
}

// The sticky offline state avoids flicker between retries. Reconnection is
// budgeted while visible and parked while hidden; the button resets the budget
// and attempts at once, in every face.
export function OfflineOverlay() {
  const { offline, reconnectPlan } = useDux()
  if (!offline) return null
  const face = copyFor(reconnectPlan)

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
          {face.spinner ? (
            <Loader2
              className="size-4 animate-spin text-muted-foreground"
              aria-hidden
            />
          ) : null}
          {face.title}
        </h1>
        <div
          id="offline-overlay-desc"
          aria-live="polite"
          className="mb-6 space-y-1 text-sm leading-relaxed text-muted-foreground"
        >
          {face.counter === null ? null : face.counter.at === null ? (
            <p>{face.counter.text}</p>
          ) : (
            <CountdownLine
              key={face.counter.at}
              prefix={face.counter.text}
              at={face.counter.at}
            />
          )}
          <p>{face.stable}</p>
        </div>
        <Button onClick={reconnect}>
          <RefreshCw aria-hidden />
          {face.button}
        </Button>
      </div>
    </div>,
    document.body,
  )
}
