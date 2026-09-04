import * as React from "react"
import { useEffect, useRef } from "react"
import { CornerDownLeft } from "lucide-react"

import { Button } from "@/components/ui/button"
import { composeHardwareKeyForwards } from "@/lib/termkeys"

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
// buffer (native textarea behavior). Only Send delivers-and-submits. The one
// class of physical key that IS intercepted is the keys a textarea has no
// meaning for, Escape and F1-F12, which forward to the PTY through
// `onForwardKey` (see the prop and `composeHardwareKeyForwards`).

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
  // What the empty box asks for. The bar sits under two different surfaces and
  // they want two different things typed into them: an agent pane is a
  // conversation, every other PTY surface is a shell. The parent knows which
  // it is, so it says; the default is the shell wording, which is what a bar
  // rendered without an opinion is sitting under.
  placeholder?: string
  // Forward the bytes of a physical key the textarea has no meaning for
  // (Escape and F1-F12, decided by the pure `composeHardwareKeyForwards` in
  // lib/termkeys) to the PTY. A tablet with a keyboard case keeps the compose
  // bar up, and its hardware Esc must interrupt a running agent the way the
  // accessory bar's Esc key does; the parent routes this through the SAME
  // write helper as that key (`sendSeq`), which owns the ownership gate and
  // the modifier latch. Optional and presentational like everything else
  // here: without it every keystroke keeps native textarea behavior.
  onForwardKey?: (seq: string) => void
  // The control in the row's LEADING slot, opposite Send. In practice this is
  // always the input ⋯ menu, but the bar takes it as a node rather than naming
  // it: the compose bar is presentational and the anchor matrix (which of the
  // three input rows carries the menu) is the parent's decision, not this
  // component's. Absent where the menu would render empty.
  leading?: React.ReactNode
}

// The textarea grows with its content from one line up to this many, then
// scrolls internally. Three lines: with the soft keyboard up the terminal is
// already down to a handful of rows, and device testing showed a taller box
// left too little PTY visible; three still shows enough of a draft to review,
// and the box scrolls for anything longer.
const MAX_ROWS = 3

// The default hint: what the bar asks for when nobody says otherwise, and what
// every terminal surface (companion, project and standalone alike) asks for.
// Exported so the agent-pane caller's opposite number can sit beside it.
export const TERMINAL_PLACEHOLDER = "Type a command…"

// The agent-pane hint. An agent session is a conversation with a CLI, not a
// shell prompt, and asking for a command there described the wrong activity to
// exactly the users who type the longest into this box.
export const AGENT_PLACEHOLDER = "Write a message to the agent…"

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
  // AN EMPTY BUFFER IS ONE ROW BY DEFINITION, so it is not measured at all:
  // the inline sizing is dropped and the class-level `min-h-10` owns the rest
  // height, exactly as it does before the box has ever grown. This is the
  // reported bug's fix (type a long message, Send, the text clears and the box
  // stays tall) and it is deliberately a short circuit rather than a better
  // measurement: the measured path could not be made to fail in a test, so
  // rather than guess at which browser-side reflow quirk produced the stale
  // read, the one state where the answer needs no reading stops reading.
  if (el.value === "") {
    el.style.height = ""
    el.style.overflowY = ""
    return
  }
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
  onForwardKey,
  placeholder = TERMINAL_PLACEHOLDER,
  leading,
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

  // The physical-keyboard forward: consult the pure rule and, on a match,
  // consume the event and hand the bytes to the parent. preventDefault is all
  // the consumption needed; the draft and the focus are untouched, so the box
  // keeps composing right through an Esc that interrupts the agent. The rule
  // itself refuses anything modified or mid-IME-composition (Escape while
  // composing keeps its native cancel-composition meaning), so a bail here is
  // the browser's key exactly as before. `isComposing` lives on the native
  // event, not React's synthetic one.
  const onKeyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (!onForwardKey) return
    const seq = composeHardwareKeyForwards({
      type: event.type,
      key: event.key,
      ctrlKey: event.ctrlKey,
      shiftKey: event.shiftKey,
      altKey: event.altKey,
      metaKey: event.metaKey,
      isComposing: event.nativeEvent.isComposing,
      keyCode: event.keyCode,
    })
    if (seq === null) return
    event.preventDefault()
    onForwardKey(seq)
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
      {/* The input ⋯ menu, in the leading slot: its placement idiom mirrors
          Send's on the opposite edge (bottom-aligned beside a grown multi-row
          textarea, `size-10 shrink-0 self-end`, which also keeps the 40px
          touch-target floor). It is always here while this bar is up, not only
          while something is hidden, which is what makes the hidden-bars dead
          end unreachable. */}
      {leading}
      <textarea
        ref={taRef}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={onKeyDown}
        rows={1}
        placeholder={placeholder}
        aria-label="Message"
        // Native keyboard assistance ON, deliberately the opposite of xterm's
        // hidden textarea (which forces all of these off because a PTY stream
        // has no buffer for them to fix). This buffer is exactly what they are
        // for, and enabling them is the reason the compose bar exists.
        autoComplete="off"
        autoCorrect="on"
        autoCapitalize="sentences"
        spellCheck={true}
        // text-sm (14px) matches the xterm canvas next door in SIZE only
        // (Terminal option fontSize: 14); the browser-default 16px visibly
        // towered over the terminal text on a phone. The FACE is deliberately
        // the app's sans, not the bundled terminal stack: this is a message
        // box a person composes prose in, with autocorrect and an IME working
        // on it, not a view of terminal content. An input font under 16px
        // normally trips
        // iOS Safari's auto-zoom-on-focus; index.html's viewport
        // `maximum-scale=1` disables that zoom (see the comment there).
        // leading-5 pins the line-height to a parseable 20px so `autosize`'s
        // computed-style read never falls back to its jsdom-only default.
        className="min-h-10 min-w-0 flex-1 resize-none rounded-md border bg-background px-3 py-2 text-sm leading-5 text-foreground placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
      />
      {/* Enabled even when the buffer is empty: an empty Send is a bare Enter
          (confirming TUI menus/prompts), not a no-op. size-10 keeps the 40px
          touch-target floor; self-end pins it to the bar's bottom edge as the
          textarea grows. The glyph is the return-key arrow (CornerDownLeft),
          because Send IS the Enter press; an up-arrow read as "scroll up". */}
      <Button
        variant="secondary"
        aria-label="Send"
        onPointerDown={onSendPointerDown}
        onClick={onSendClick}
        className="size-10 shrink-0"
      >
        <CornerDownLeft />
      </Button>
    </div>
  )
}
