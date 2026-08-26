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

/// Desktop wheel speed for LOCAL scrollback scrolling: xterm's
/// `scrollSensitivity` multiplier, set to 3 so one wheel notch moves three
/// lines' worth instead of one (matching the TUI's MOUSE_WHEEL_LINES).
/// Verified against the installed xterm 6 source: the Viewport passes this
/// option to its scrollable element as `mouseWheelScrollSensitivity` (LOCAL
/// viewport scrolling only), and that local wheel handling is DISABLED entirely
/// while an app in the PTY captures the wheel; the wheel-REPORT path to a
/// mouse-tracking app sends one report per wheel event regardless of this
/// value, so app forwarding stays 1:1 per tick. The touch drag path
/// (`dragScrollLines`) is finger-proportional and unaffected.
export const WHEEL_SCROLL_SENSITIVITY = 3

/// How often the pane re-reads its replay-wait clock while a cover is up with
/// no screen behind it. A poll rather than a `setTimeout` because the quantity
/// being waited on is ACCUMULATED VISIBLE TIME (see `lib/visibleClock.ts`),
/// which a timer cannot measure: a hidden tab throttles it and a suspended page
/// resumes with it already fired. One second, so the Reconnect box appears
/// within a second of the configured wait, and the poll exists only while there
/// is something to wait for.
export const REPLAY_WAIT_POLL_MS = 1000

/// How long the container must hold still before its new size goes to the PTY.
/// A PTY resize is a SIGWINCH, a full child repaint, so it is debounced to one
/// send with the final dimensions, and the SAME delay is reused when a
/// touch-scroll gesture ends with a held resize to flush: the flush is a settle
/// window like any other, giving the keyboard/URL-bar animation that held it
/// time to finish collapsing.
export const RESIZE_SEND_DEBOUNCE_MS = 200

/// How long the PTY's grid must hold still before a diverged VIEWER bounces its
/// socket to heal (see `viewerGrid.ts`). Deliberately longer than the two things
/// that make an applied grid arrive in bursts: the owner's own send debounce
/// above (200ms, so a divider drag lands one grid per settle) and the first
/// open's width jiggle (two grids 60ms apart, in `lib/firstFrameResize.ts`). A
/// window shorter than either would reconnect a watching phone several times
/// for one gesture on the desktop; this collapses a burst into exactly one
/// reconnect, at the cost of the badge standing for half a second longer than
/// it strictly must.
export const VIEWER_HEAL_DEBOUNCE_MS = 500

/// The xterm scrollbar's width in CSS pixels, from the one
/// `--xterm-scrollbar-width` CSS variable index.css also reserves the button
/// overlay's gutter from. Read here rather than at each call site so the
/// terminal's own scrollbar option and the watcher view's available-width
/// arithmetic cannot disagree about how much room it takes. Falls back to 8,
/// the value in index.css, ONLY when the variable is missing or unparsable:
/// an explicit 0 is a real answer (a scrollbar deliberately hidden), so the
/// check is for NaN, never for falsiness.
export function xtermScrollbarWidth(): number {
  const parsed = parseInt(
    getComputedStyle(document.documentElement).getPropertyValue(
      "--xterm-scrollbar-width",
    ),
    10,
  )
  return Number.isNaN(parsed) ? 8 : parsed
}

/// Bytes written straight to the PTY (bypassing xterm's data pipeline), plus
/// the view side effects a typed key would get through that pipeline: snap to
/// the live edge and drop any stale selection so the user sees where the input
/// landed. Shared so every entry point that writes input directly (the physical
/// Shift-Enter handler, the accessory bar's ⇧↵ key, and the mobile compose
/// bar's Send) lands identically and cannot drift apart. Latch handling is left
/// to each caller, because they decide it from different rules.
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
