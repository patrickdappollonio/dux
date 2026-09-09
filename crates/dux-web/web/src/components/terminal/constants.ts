// Measured constants and the one shared input writer, in a module of their own
// because more than one of the rebuilt units reads each of them.
import type { Terminal } from "@xterm/xterm"

import type { PtySocket } from "@/lib/ptySocket"
import { LF } from "@/lib/termkeys"

/// The pointer must move at least this many CSS px between mousedown and
/// mouseup to count as a drag (a selection attempt) rather than a click.
/// Guards the mouse-capture hint from firing on a plain click into a
/// mouse-reporting app, and decides whether a link gesture travelled.
export const DRAG_THRESHOLD_PX = 4

/// Desktop wheel speed for local scrollback scrolling: xterm's
/// `scrollSensitivity` multiplier, matching the TUI's MOUSE_WHEEL_LINES. It
/// affects local viewport scrolling only, which xterm disables entirely while
/// an app in the PTY captures the wheel; the wheel-report path to a
/// mouse-tracking app stays one report per event whatever this says.
export const WHEEL_SCROLL_SENSITIVITY = 3

/// How often the pane re-reads its replay-wait clock while a cover is up with no
/// screen behind it. A poll rather than a `setTimeout` because the quantity is
/// accumulated visible time (see `lib/visibleClock.ts`), which a timer cannot
/// measure: a hidden tab throttles it and a suspended page resumes with it
/// already fired. The poll exists only while there is something to wait for.
export const REPLAY_WAIT_POLL_MS = 1000

/// How long the container must hold still before its new size goes to the PTY.
/// A PTY resize is a SIGWINCH, a full child repaint, so it is debounced to one
/// send with the final dimensions, and the SAME delay is reused when a
/// touch-scroll gesture ends with a held resize to flush: the flush is a settle
/// window like any other, giving the keyboard/URL-bar animation that held it
/// time to finish collapsing.
export const RESIZE_SEND_DEBOUNCE_MS = 200

/// How long the PTY's grid must hold still before a diverged viewer bounces its
/// socket to heal (see `viewerGrid.ts`). Longer than both things that make an
/// applied grid arrive in bursts, the owner's send debounce above and the first
/// open's width jiggle, so one desktop gesture cannot reconnect a watching phone
/// several times.
export const VIEWER_HEAL_DEBOUNCE_MS = 500

/// The xterm scrollbar's width in CSS pixels, from the `--xterm-scrollbar-width`
/// variable index.css also reserves the button overlay's gutter from. Read here
/// rather than per call site so the scrollbar option and the watcher view's
/// available-width arithmetic cannot disagree. The fallback applies only to a
/// missing or unparsable value: an explicit 0 is a real answer, so the check is
/// for NaN and never for falsiness.
export function xtermScrollbarWidth(): number {
  const parsed = parseInt(
    getComputedStyle(document.documentElement).getPropertyValue(
      "--xterm-scrollbar-width",
    ),
    10,
  )
  return Number.isNaN(parsed) ? 8 : parsed
}

/// Bytes written straight to the PTY, bypassing xterm's data pipeline, plus the
/// view side effects a typed key would get through it: snap to the live edge and
/// drop a stale selection, so the user sees where the input landed. Shared so
/// every direct-write entry point lands identically. Latch handling stays with
/// each caller, which decide it from different rules.
export function writeInputWithLandingEffects(
  term: Terminal | null,
  pty: PtySocket | null,
  bytes: Uint8Array,
): void {
  term?.scrollToBottom()
  term?.clearSelection()
  pty?.sendInput(bytes)
}

/// A soft newline (LF / Ctrl-j): the shared landing-effects write with the one
/// fixed LF byte, kept as its own named helper so the two soft-newline entry
/// points (physical Shift-Enter and the accessory bar's ⇧↵ key) stay in step.
const LF_BYTES = new TextEncoder().encode(LF)
export function writeSoftNewline(
  term: Terminal | null,
  pty: PtySocket | null,
): void {
  writeInputWithLandingEffects(term, pty, LF_BYTES)
}
