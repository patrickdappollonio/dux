import * as React from "react"
import { useEffect, useRef } from "react"
import { CornerDownLeft } from "lucide-react"

import { Button } from "@/components/ui/button"
import { composeHardwareKeyForwards } from "@/lib/termkeys"

// The phone's typing surface: a real textarea with native keyboard assistance
// on, buffering the message locally, plus a Send that delivers it in one write.
// Typing straight into xterm's hidden textarea is hostile on a phone, where
// autocorrect, swipe and IMEs fight an input that must stay raw.
//
// Presentational and thin: this owns only the textarea's autosizing and emits
// `onSend(text)`. The buffer is controlled and lives in TerminalPane, so
// unmounting the bar never destroys in-progress text, and every behavior
// (payload encoding, ownership gating, PTY writes) lives there and in
// `lib/composebar`.
//
// Enter inserts a newline; only Send delivers. The one class of physical key
// intercepted is the keys a textarea has no meaning for, Escape and F1-F12,
// forwarded through `onForwardKey`.

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
  // (Escape and F1-F12, decided by `composeHardwareKeyForwards`) to the PTY: a
  // tablet with a keyboard case keeps the compose bar up, and its hardware Esc
  // must interrupt a running agent the way the accessory Esc key does. The
  // parent routes it through that key's own write helper, which owns the
  // ownership gate. Without it every keystroke keeps native textarea behavior.
  onForwardKey?: (seq: string) => void
  // The control in the row's leading slot, opposite Send: a node rather than a
  // named menu, because which input row carries the `⋯` is the parent's
  // decision. Absent where the menu would render empty.
  leading?: React.ReactNode
}

// The textarea grows with its content up to this many lines, then scrolls
// internally. Three, because with the soft keyboard up the terminal is already
// down to a handful of rows and a taller box leaves too little PTY visible.
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
// older iOS Safari. The height is reset, `scrollHeight` read back, and capped
// at MAX_ROWS' worth of pixels. The `|| 20` fallback covers a computed
// line-height that is not a parseable pixel value ("normal", or empty under
// jsdom); the `leading-5` class makes it parseable in real browsers.
//
// Border-box is load-bearing: Tailwind preflight sets it, so the height style
// must cover content plus padding plus border while `scrollHeight` covers only
// the first two, and the shortfall clips the last line under overflow-y hidden.
// The delta is `offsetHeight - clientHeight`, added to the height and to the
// cap, which also adds the vertical padding so it means MAX_ROWS lines of
// content rather than MAX_ROWS lines minus the box chrome.
function autosize(el: HTMLTextAreaElement): void {
  // An empty buffer is one row by definition, so it is not measured: the
  // inline sizing is dropped and the class-level `min-h-10` owns the rest
  // height. Short-circuited rather than measured, because the measured read can
  // come back stale after a send and leave the box tall with no text in it.
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

  // The physical-keyboard forward. preventDefault is all the consumption
  // needed: the draft and the focus are untouched, so the box keeps composing
  // through an Esc that interrupts the agent. The rule refuses anything
  // modified or mid-IME-composition, where Escape keeps its native
  // cancel-composition meaning. `isComposing` lives on the native event.
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
        // text-sm matches the xterm canvas next door in size only; the face is
        // the app's sans, because this is prose a person composes with
        // autocorrect and an IME, not a view of terminal content. An input font
        // under 16px normally trips iOS Safari's auto-zoom-on-focus, which
        // index.html's viewport `maximum-scale=1` disables. leading-5 pins the
        // line-height to a parseable 20px for `autosize`'s computed-style read.
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
