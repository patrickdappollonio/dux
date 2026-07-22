import * as React from "react"
import { useEffect, useRef } from "react"
import { ArrowUp } from "lucide-react"

import { Button } from "@/components/ui/button"

// Typing straight into xterm's hidden textarea is hostile on a phone: the
// soft keyboard's autocorrect/swipe/IME fight an input that must stay raw, and
// there is no local editing before bytes hit the PTY. The compose bar is the
// phone's typing surface instead: a real textarea (native keyboard assistance
// ON, the whole point) that buffers the message locally, plus a Send button
// that delivers it in one write. It renders as the third row of the mobile
// shell, below the accessory bar's two key rows.
//
// Presentational and thin, like AccessoryBar: this component owns only the
// textarea's autosizing, and emits `onSend(text)`. The buffer itself is
// CONTROLLED (value/onChange) and lives in TerminalPane, so unmounting the bar
// (a preference flip, a rotation to desktop width) never destroys in-progress
// text. All behavior (payload encoding, bracketed paste, ownership gating,
// PTY writes, scroll/selection side effects) lives in TerminalPane +
// lib/composebar.
//
// Enter inside the textarea is NOT intercepted: it inserts a newline in the
// buffer (native textarea behavior). Only Send delivers-and-submits.

interface ComposeBarProps {
  // The buffered message text, owned by the parent (controlled input).
  value: string
  // Buffer edits, and the post-send clear: a successful send routes through
  // `onChange("")` rather than mutating the DOM value, or the controlled
  // value would desync and the cleared text would reappear on the next
  // parent re-render.
  onChange: (text: string) => void
  // Fire the buffered text (possibly empty: an empty Send means "press
  // Enter", how the user confirms a TUI menu/prompt without focusing xterm).
  // The parent encodes the payload and performs the PTY write. Returns
  // whether the send actually happened: on true the buffer is cleared, on
  // false (not the owner, socket down, oversized message) it is KEPT so the
  // user can retry, with the parent toasting the reason.
  onSend: (text: string) => boolean
  // The parent's handle on the textarea, so the tap-to-focus redirect (and
  // any focus bookkeeping) can target it without reaching into the DOM. A
  // plain RefObject (not a callback ref) attached directly to the textarea;
  // when absent the component falls back to its own internal ref.
  inputRef?: React.RefObject<HTMLTextAreaElement | null>
}

// The textarea grows with its content from one line up to this many, then
// scrolls internally. Three lines: with the soft keyboard up the terminal is
// already down to a handful of rows, and device testing showed a taller box
// left too little PTY visible; three still shows enough of a draft to review,
// and the box scrolls for anything longer.
const MAX_ROWS = 3

// Autosize by measurement, not CSS: `field-sizing: content` is unsupported on
// OLDER iOS Safari (it shipped in 26.2, Dec 2025), so the JS measurement keeps
// those devices working. We reset the height and read back scrollHeight (the
// content's natural height), capping it at MAX_ROWS' worth of pixels. The
// `|| 20` fallback covers environments whose computed line-height is not a
// parseable pixel value ("normal", or empty under jsdom); the explicit
// `leading-5` class below makes it parseable (20px) in real browsers.
//
// BORDER-BOX, load-bearing: Tailwind preflight sets `box-sizing: border-box`,
// so the height style must cover content + padding + BORDER, while
// `scrollHeight` is content + padding only. Setting height = scrollHeight
// left the content area short by the border width and, with overflow-y
// hidden, clipped the bottom of the last line on device. The border delta is
// measured as `offsetHeight - clientHeight` (both include/exclude exactly the
// border) and added to the height AND to the cap; the cap likewise adds the
// vertical padding so it means "MAX_ROWS lines of CONTENT", not "MAX_ROWS
// lines minus the box chrome".
function autosize(el: HTMLTextAreaElement): void {
  el.style.height = "auto"
  const style = getComputedStyle(el)
  const line = parseFloat(style.lineHeight) || 20
  const padding =
    (parseFloat(style.paddingTop) || 0) + (parseFloat(style.paddingBottom) || 0)
  const border = el.offsetHeight - el.clientHeight
  const max = Math.ceil(line * MAX_ROWS + padding + border)
  const needed = el.scrollHeight + border
  el.style.height = `${Math.min(needed, max)}px`
  el.style.overflowY = needed > max ? "auto" : "hidden"
}

export function ComposeBar({
  value,
  onChange,
  onSend,
  inputRef,
}: ComposeBarProps) {
  // The textarea handle used for the autosize re-measure: the parent's ref
  // when provided (so the parent and this component share ONE handle rather
  // than merging two), the component's own otherwise.
  const ownRef = useRef<HTMLTextAreaElement | null>(null)
  const taRef = inputRef ?? ownRef

  // Re-measure whenever the rendered value changes, AFTER the commit: the
  // measurement reads the DOM (scrollHeight follows the textarea's actual
  // value), so it must run once the DOM reflects `value`. This one effect
  // covers typing, the post-send clear, and a parent-driven rewrite alike.
  useEffect(() => {
    const el = taRef.current
    if (el) autosize(el)
  }, [value, taRef])

  // One shared send routine for both activation paths below: deliver the
  // buffer, and clear it ONLY when the parent reports the send happened (a
  // refused send keeps the draft for a retry; the parent toasts why).
  const trySend = () => {
    if (onSend(value)) onChange("")
  }

  // Fire on pointerdown with preventDefault, the same trick every accessory-bar
  // key uses (see AccessoryBar's `keyDown`): the press must never shift focus
  // off the compose textarea, or the soft keyboard would dismiss on every Send.
  const onSendPointerDown = (event: React.PointerEvent) => {
    event.preventDefault()
    trySend()
  }

  // Keyboard/AT activation: Enter or Space on the focused button fires a
  // `click` with `detail === 0` (no pointer press). A click that FOLLOWS a
  // real pointer tap carries `detail >= 1` and is ignored here, because the
  // pointerdown handler above already sent; without the detail gate every tap
  // would double-send.
  const onSendClick = (event: React.MouseEvent) => {
    if (event.detail === 0) trySend()
  }

  return (
    <div className="flex shrink-0 items-end gap-1.5 border-t bg-background px-1 py-1">
      <textarea
        ref={taRef}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        rows={1}
        placeholder="Type a command…"
        aria-label="Message"
        // Native keyboard assistance ON, deliberately the opposite of xterm's
        // hidden textarea (which forces all of these off because a PTY stream
        // has no buffer for them to fix). This buffer is exactly what they are
        // for, and enabling them is the reason the compose bar exists.
        autoComplete="off"
        autoCorrect="on"
        autoCapitalize="sentences"
        spellCheck={true}
        // text-sm (14px) matches the xterm canvas next door (Terminal option
        // fontSize: 14); the browser-default 16px visibly towered over the
        // terminal text on a phone. An input font under 16px normally trips
        // iOS Safari's auto-zoom-on-focus; index.html's viewport
        // `maximum-scale=1` disables that zoom (see the comment there).
        // leading-5 pins the line-height to a parseable 20px so `autosize`'s
        // computed-style read never falls back to its jsdom-only default.
        className="min-h-10 min-w-0 flex-1 resize-none rounded-md border bg-background px-3 py-2 text-sm leading-5 text-foreground placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
      />
      {/* Enabled even when the buffer is empty: an empty Send is a bare Enter
          (confirming TUI menus/prompts), not a no-op. size-10 keeps the 40px
          touch-target floor; self-end pins it to the bar's bottom edge as the
          textarea grows. */}
      <Button
        variant="secondary"
        aria-label="Send"
        onPointerDown={onSendPointerDown}
        onClick={onSendClick}
        className="size-10 shrink-0"
      >
        <ArrowUp />
      </Button>
    </div>
  )
}
