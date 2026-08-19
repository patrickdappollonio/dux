// THE INPUT SURFACE.
//
// Everything that puts a byte into the PTY on the user's behalf, and everything
// that decides where the caret goes afterwards: the compose bar's Send, the
// accessory bar's key rows, the sticky modifier latches, the draft splice, and
// the one focus-routing rule.
//
// It is a pane-adjacent unit rather than part of the lifecycle because none of
// it is tied to the terminal's lifetime: these are handlers the render hands to
// two child components, and they read the live terminal and socket through the
// refs the lifecycle fills in.
//
// THE ONE ROUTING RULE lives here as a pair of standalone functions rather than
// as methods, because the pane needs it before this hook has run (the ownership
// machine's take-over refocuses, and the lifecycle focuses on mount). Every
// refocus in the pane goes through `focusTypingSurfaceIn`, and every
// keyboard-state question through `typingSurfaceHasFocusIn`; there is no second
// implementation of either.
import { useEffect, useMemo, useRef, useState } from "react"
import type { Terminal } from "@xterm/xterm"

import {
  COMPOSE_SUBMIT_DELAY_MS,
  composeSendTooLarge,
  composeSendWrites,
  insertIntoComposeDraft,
} from "@/lib/composebar"
import { notifyError } from "@/lib/notify"
import { pasteIntoTerm } from "@/lib/termClipboard"
import type { PtySocket } from "@/lib/ptySocket"
import { arrowSeq, ESC, pageKeySeq } from "@/lib/termkeys"
import {
  dispatchMouseReplay,
  rectCenter,
  wheelReplaySteps,
} from "@/lib/termmouse"
import type { ScrollDir } from "@/components/AccessoryBar"

import type { LiveSettings } from "./liveValues"
import type { ModifierLatch, OwnershipVerdict } from "./channels"
import { writeInputWithLandingEffects, writeSoftNewline } from "./constants"

/// The three things "where does typing go" is answered from.
export type TypingSurfaceRefs = {
  live: LiveSettings
  composeInputRef: { current: HTMLTextAreaElement | null }
  termRef: { current: Terminal | null }
}

// Where typing focus belongs right now: the compose textarea while the
// mobile compose bar is up (so the soft keyboard keeps typing into the
// buffer), xterm's hidden textarea otherwise. Every handler that used to
// refocus the terminal after acting routes through this, keeping the
// accessory-bar contract (a bar key never steals focus from the active
// typing surface) intact for both surfaces.
export function focusTypingSurfaceIn(refs: TypingSurfaceRefs): void {
  if (refs.live.current.composeActive && refs.composeInputRef.current) {
    refs.composeInputRef.current.focus()
  } else {
    refs.termRef.current?.focus()
  }
}

// Whether the active typing surface (the compose textarea while the bar is
// up, xterm's hidden textarea otherwise) holds focus RIGHT NOW. The
// accessory-key handlers read this at tap time to preserve the soft-keyboard
// state: a key tap must never CHANGE that state, so they refocus only when
// the surface had focus when the tap landed (the bar's buttons preventDefault
// their pointerdown, so the tap itself never moves focus; the conditional
// refocus is insurance for browsers where that suppression is incomplete).
// An unconditional focusTypingSurface() here was the soft-keyboard-pop bug:
// a user paging through output with the keyboard closed had it summoned by
// every key tap.
export function typingSurfaceHasFocusIn(refs: TypingSurfaceRefs): boolean {
  const active = document.activeElement
  if (active === null) return false
  if (refs.live.current.composeActive && refs.composeInputRef.current !== null) {
    return active === refs.composeInputRef.current
  }
  return active === (refs.termRef.current?.textarea ?? null)
}

// Accessory-bar key sends. Esc/Tab/arrows are full sequences, not single
// chars, so they bypass `applyModifiers` (which only transforms single-char
// input). We still honor a latched Alt by prefixing ESC, and we clear any
// latch one-shot afterward. Ctrl on a non-char key has no meaning here, so
// it's simply consumed. Sends go through the same socket path as typed input.

export type InputSurfaceDeps = TypingSurfaceRefs & {
  ptyRef: { current: PtySocket | null }
  ownership: OwnershipVerdict
}

export type InputSurface = {
  /// The latch's visible state, for the accessory bar's highlight.
  ctrl: boolean
  alt: boolean
  /// The latch channel, whose only writer is this unit.
  mods: ModifierLatch
  /// The compose draft. It lives HERE and not in `ComposeBar` precisely so the
  /// bar can unmount (a preference flip, a rotation past the mobile
  /// breakpoint) without destroying in-progress text.
  composeText: string
  setComposeText: (value: string) => void
  focusTypingSurface: () => void
  typingSurfaceHasFocus: () => boolean
  insertComposeText: (text: string) => void
  sendCompose: (text: string) => boolean
  sendSeq: (seq: string) => void
  onArrow: (dir: "up" | "down" | "left" | "right") => void
  sendNewline: () => void
  toggleCtrl: () => void
  toggleAlt: () => void
  onScroll: (dir: ScrollDir) => void
  onRightClickPaste: () => void
}

export function useInputSurface(deps: InputSurfaceDeps): InputSurface {
  const { live, composeInputRef, termRef, ptyRef, ownership } = deps
  const refs: TypingSurfaceRefs = { live, composeInputRef, termRef }
  const focusTypingSurface = () => focusTypingSurfaceIn(refs)
  const typingSurfaceHasFocus = () => typingSurfaceHasFocusIn(refs)

  // Sticky (one-shot latched) soft-keyboard modifiers for the accessory bar.
  // The state drives the latch's visual highlight; the ref mirrors it so the
  // value is readable inside the lifecycle's stable `onData` closure, which
  // would otherwise capture a stale `ctrl`/`alt`. The CHANNEL writes both
  // together, so they can never diverge, and it is the only writer.
  const [ctrl, setCtrl] = useState(false)
  const [alt, setAlt] = useState(false)
  const modsRef = useRef({ ctrl: false, alt: false })
  const mods = useMemo<ModifierLatch>(
    () => ({
      read: () => modsRef.current,
      write: (next) => {
        modsRef.current = next
        setCtrl(next.ctrl)
        setAlt(next.alt)
      },
    }),
    [],
  )

  const [composeText, setComposeText] = useState("")
  // Where the caret should land after a programmatic draft splice (a picked
  // macro inserting into the draft). A controlled textarea re-renders on the
  // value change and the browser parks the caret at the end of the new value,
  // so the splice records its intended caret here and this effect applies it in
  // the same commit the new draft text reaches the DOM. Null means "no pending
  // placement": ordinary typing never goes through this.
  const pendingComposeCaretRef = useRef<number | null>(null)
  useEffect(() => {
    const caret = pendingComposeCaretRef.current
    if (caret === null) return
    pendingComposeCaretRef.current = null
    composeInputRef.current?.setSelectionRange(caret, caret)
  }, [composeText, composeInputRef])

  // Right-click pastes the browser clipboard (classic terminal: selecting
  // copies via copy-on-select, right-click pastes). Gated on ownership (a
  // read-only viewer cannot drive input). Needs a secure context for
  // `readText`; `pasteIntoTerm` toasts a "use Ctrl+v" hint when the clipboard
  // cannot be read (plain HTTP).
  function onRightClickPaste() {
    const term = termRef.current
    if (term && ownership.read()) void pasteIntoTerm(term, focusTypingSurface)
  }

  // Splice text into the mobile compose bar's DRAFT at the caret. Shared by
  // the two things that put text there without typing it: a picked macro (via
  // the module-scope `composeInsert` sink) and the path of an image pasted
  // while the bar is the typing surface. One implementation, so the caret
  // handling and the refocus cannot drift between them.
  function insertComposeText(text: string) {
    // The textarea's selection is read up front, once: the functional updater
    // below may run more than once (StrictMode), and it must splice the same
    // way each time. A missing element or selection falls back to appending
    // (insertIntoComposeDraft treats null as "append").
    const el = composeInputRef.current
    const selectionStart = el === null ? null : el.selectionStart
    const selectionEnd = el === null ? null : el.selectionEnd
    setComposeText((prev) => {
      const { next, caret } = insertIntoComposeDraft(
        prev,
        selectionStart,
        selectionEnd,
        text,
      )
      // A ref write inside the updater is idempotent: the same inputs yield
      // the same caret on a re-run. The caret-placement effect applies it once
      // the new draft value reaches the DOM.
      pendingComposeCaretRef.current = caret
      return next
    })
    // The draft the text just joined is where editing continues; the active
    // typing surface here IS the compose textarea.
    focusTypingSurface()
  }

  const encoder = new TextEncoder()

  // The compose bar's Send: deliver the buffered message, then submit it.
  // The write plan lives in the pure `composeSendWrites`: the MACRO keystroke
  // convention (newlines are Alt+Enter, ESC CR, exactly like
  // `macroPayloadBytes`) as the body write, and the submitting bare CR as a
  // SEPARATE write the timeout below delivers COMPOSE_SUBMIT_DELAY_MS later.
  // Deliberately NOT bracketed paste, and no read of `bracketedPasteMode`;
  // and the Enter travels alone because Claude Code merges stdin chunks into
  // one paste through a measured 50ms debounce that would swallow a
  // same-window CR into the paste as a newline (see COMPOSE_SUBMIT_DELAY_MS).
  // An empty buffer is a single immediate bare CR, a lone Enter keystroke.
  // The shared landing-effects writer replays the scroll-to-live-edge and
  // selection-drop a typed key would get, ONCE, with the first write. Focus
  // stays in the compose textarea (the Send button preventDefaults its
  // pointerdown, so it never left).
  //
  // Returns whether the send happened; the bar clears its buffer only on
  // true. A composed message can be minutes of typing, so unlike a keystroke
  // (cheap to re-type, silently droppable) every refused send KEEPS the buffer
  // and toasts the reason: not the input owner (like every write path; take
  // over to reclaim), socket not open (the sendInput readyState guard would
  // silently drop the bytes), or payload over the client-side cap (an
  // oversized frame would make the server abort the whole socket, see
  // MAX_COMPOSE_SEND_BYTES).
  //
  // Deliberately does NOT consume the one-shot Ctrl/Alt accessory latches: a
  // latch arms the next direct KEY, and a composed message is not a key; a
  // user who tapped Ctrl intending Ctrl-c should not lose the latch to an
  // unrelated Send.
  //
  // All three refusals KEEP the fixed `compose-send` id, which is the opposite
  // of what the terminal copy and paste notifications now do (see
  // `lib/termClipboard.ts` for why theirs went away). Send is one deliberate
  // press producing one of three fixed sentences, and a user who presses it
  // three times against a dead socket wants one "not connected", not three
  // identical copies of it stacked up. The id is doing real work here: it also
  // means the reason REPLACES itself when it changes, so a viewer who takes
  // over and then hits the size cap sees the new reason rather than two
  // contradictory ones.
  //
  // The hazard is real and is accepted: repeating a failing Send restarts the
  // 24s error countdown each time, so the toast lingers for a full window after
  // the LAST attempt rather than the first. That is the correct end of the
  // trade for a message that is still true while the user keeps trying, and it
  // is bounded, unlike the copy-on-select case where an incidental gesture the
  // user never thought of as raising a toast could pin one open indefinitely.
  function sendCompose(text: string): boolean {
    if (!ownership.read()) {
      notifyError("Another device is driving this terminal. Take over to send.", {
        id: "compose-send",
      })
      return false
    }
    if (!(ptyRef.current?.isOpen ?? false)) {
      notifyError("Not connected right now. Your message was kept.", {
        id: "compose-send",
      })
      return false
    }
    const writes = composeSendWrites(text)
    const totalBytes = writes.reduce((n, w) => n + w.byteLength, 0)
    if (composeSendTooLarge(totalBytes)) {
      notifyError("Message too large to send. Trim it down and try again.", {
        id: "compose-send",
      })
      return false
    }
    writeInputWithLandingEffects(termRef.current, ptyRef.current, writes[0])
    // A two-write plan: the submitting CR follows after the measured-safe gap
    // (see composeSendWrites). The send is committed at this point, hence
    // `true` below; the delayed CR is a bare PTY write with no further side
    // effects. Guards: the pane may unmount (its cleanup nulls `ptyRef`, so
    // the identity check fails) or the socket may drop (`isOpen`) before the
    // timer fires; in either case the orphaned CR is skipped rather than
    // delivered to a socket this pane no longer drives.
    if (writes.length > 1) {
      const pty = ptyRef.current
      const rest = writes.slice(1)
      setTimeout(() => {
        if (pty === null || ptyRef.current !== pty || !pty.isOpen) return
        for (const w of rest) pty.sendInput(w)
      }, COMPOSE_SUBMIT_DELAY_MS)
    }
    return true
  }

  function sendSeq(seq: string) {
    // Read-only when not the owner: the accessory-bar keys (Esc/Tab/arrows) are
    // input too, so a secondary viewer's taps are dropped just like typed input.
    if (!ownership.read()) return
    // Captured BEFORE acting: a key tap preserves the keyboard state, so the
    // refocus below runs only when the typing surface had focus at tap time
    // (see typingSurfaceHasFocus).
    const keepFocus = typingSurfaceHasFocus()
    const latch = mods.read()
    const out = latch.alt ? ESC + seq : seq
    if (latch.ctrl || latch.alt) {
      mods.write({ ctrl: false, alt: false })
    }
    ptyRef.current?.sendInput(encoder.encode(out))
    if (keepFocus) focusTypingSurface()
  }

  function onArrow(dir: "up" | "down" | "left" | "right") {
    const app = termRef.current?.modes.applicationCursorKeysMode ?? false
    sendSeq(arrowSeq(dir, app))
  }

  // The accessory bar's ⇧↵ key, the touch equivalent of Shift-Enter, since a
  // soft keyboard can't produce that chord. Owner-gated like every accessory
  // send; consumes any armed Ctrl/Alt latch (a raw newline doesn't combine with
  // them, so unlike `sendSeq` it never routes through `applyModifiers`) and keeps
  // focus so the user keeps typing. Shares `writeSoftNewline` with the physical
  // Shift-Enter handler so both land input identically.
  function sendNewline() {
    if (!ownership.read()) return
    const keepFocus = typingSurfaceHasFocus()
    if (mods.read().ctrl || mods.read().alt) {
      mods.write({ ctrl: false, alt: false })
    }
    writeSoftNewline(termRef.current, ptyRef.current)
    if (keepFocus) focusTypingSurface()
  }

  function toggleCtrl() {
    const keepFocus = typingSurfaceHasFocus()
    mods.write({ ctrl: !mods.read().ctrl, alt: mods.read().alt })
    if (keepFocus) focusTypingSurface()
  }

  function toggleAlt() {
    const keepFocus = typingSurfaceHasFocus()
    mods.write({ ctrl: mods.read().ctrl, alt: !mods.read().alt })
    if (keepFocus) focusTypingSurface()
  }

  // Scroll the xterm viewport from the accessory bar's second row. On the normal
  // buffer these drive xterm's own scrollback (the history that accumulates as
  // the agent streams output), giving a reliable touch target the slim scrollbar
  // can't.
  //
  // On the ALT-SCREEN (a full-screen TUI) xterm has no scrollback, so PgUp/PgDn
  // forward a page to the app itself, mirroring the TUI's forward-scroll: a
  // mouse-tracking app (Claude, Codex, ...) gets a screenful of wheel events; a
  // keyboard-only app gets the PgUp/PgDn keys. Jump-to-top/bottom has no clean
  // wheel equivalent, so those two stay scrollback-only and are a no-op on the
  // alt-screen; the cursor-arrow row drives fine-grained movement there.
  //
  // Scrolling is a READ gesture, so it drops the hidden textarea's focus: that
  // slides the soft keyboard away to free the whole screen for reading back and,
  // crucially, stops a scroll-button tap from re-summoning it. On iOS the
  // textarea stays the focused element after the user swipes the keyboard down,
  // so any later tap on a focus-retaining (preventDefault) button pops it right
  // back up; blurring here is what keeps it down. Tapping the terminal refocuses
  // to resume typing. (The input keys, Esc/Tab/Ctrl/Alt/newline and the cursor
  // arrows, instead KEEP focus; only PgUp/PgDn blur. It's an input vs
  // page-scroll split, not a row split.)
  function onScroll(dir: ScrollDir) {
    const term = termRef.current
    if (!term) return
    const altScreen = term.buffer.active.type !== "normal"
    // On the alt-screen, a Page button forwards to the full-screen app (input,
    // so only when we own the PTY); top/bottom have no wheel equivalent and fall
    // through to the local scroll, which is a no-op there.
    if (
      altScreen &&
      ownership.read() &&
      (dir === "pageUp" || dir === "pageDown")
    ) {
      const up = dir === "pageUp"
      if (term.modes.mouseTrackingMode !== "none") {
        // A screenful of wheel notches toward older (up) or newer (down) output.
        // The exact distance depends on the app's per-notch step; one row-height
        // shy of a full screen is a reasonable page. Replayed as real wheel
        // events at the middle of the terminal so xterm encodes them the way the
        // app asked (see `lib/termmouse.ts`); there is no finger to take a point
        // from here, so the centre stands in for one.
        const lines = Math.max(1, term.rows - 1)
        const element = term.element
        if (element) {
          const { clientX, clientY } = rectCenter(
            element.getBoundingClientRect(),
          )
          dispatchMouseReplay(
            element,
            wheelReplaySteps(up ? -lines : lines),
            clientX,
            clientY,
          )
        }
      } else {
        // Keyboard-only full-screen app: send the actual PgUp/PgDn key.
        ptyRef.current?.sendInput(encoder.encode(pageKeySeq(up ? "up" : "down")))
      }
      if (navigator.maxTouchPoints > 0) {
        term.textarea?.blur()
        // The compose textarea holds the keyboard when the compose bar is up;
        // a page-scroll is a reading gesture on either surface, so let it go.
        composeInputRef.current?.blur()
      }
      return
    }
    switch (dir) {
      case "pageUp":
        term.scrollPages(-1)
        break
      case "pageDown":
        term.scrollPages(1)
        break
    }
    // Only a touch device has a soft keyboard to dismiss. Gating on touch
    // capability stops a narrow-window mouse user (who also gets this mobile bar)
    // from silently losing terminal focus when paging through output. The
    // compose textarea can be the keyboard's holder too, so both surfaces let go.
    if (navigator.maxTouchPoints > 0) {
      term.textarea?.blur()
      composeInputRef.current?.blur()
    }
  }

  return {
    ctrl,
    alt,
    mods,
    composeText,
    setComposeText,
    focusTypingSurface,
    typingSurfaceHasFocus,
    insertComposeText,
    sendCompose,
    sendSeq,
    onArrow,
    sendNewline,
    toggleCtrl,
    toggleAlt,
    onScroll,
    onRightClickPaste,
  }
}
