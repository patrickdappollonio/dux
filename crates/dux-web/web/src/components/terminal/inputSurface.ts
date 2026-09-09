// Everything that puts a byte into the PTY on the user's behalf, and everything
// that decides where the caret goes afterwards. Pane-adjacent rather than part
// of the lifecycle: none of it is tied to the terminal's lifetime.
//
// The routing rule is a pair of standalone functions rather than methods,
// because the pane needs it before this hook has run. Every refocus goes through
// `focusTypingSurfaceIn` and every keyboard-state question through
// `typingSurfaceHasFocusIn`; there is no second implementation of either.
import { useEffect, useMemo, useRef, useState } from "react"
import type { Terminal } from "@xterm/xterm"

import {
  COMPOSE_SUBMIT_DELAY_MS,
  composeSendTooLarge,
  composeSendWrites,
  insertIntoComposeDraft,
} from "@/lib/composebar"
import { notifyError } from "@/lib/notify"
import {
  composeDraft,
  peekComposeDraft,
  setComposeDraft,
  useDux,
} from "@/lib/store"
import { pasteClipboardText, pasteIntoTerm } from "@/lib/termClipboard"
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

// Hand a full-screen app one page of scrolling in the shape it asked for: wheel
// events while it tracks the mouse, the PgUp/PgDn key otherwise. The alt-screen
// has no scrollback of its own, so this is what a page key means there.
export function forwardPageToApp(
  term: Terminal,
  pty: PtySocket | null,
  up: boolean,
): void {
  if (term.modes.mouseTrackingMode === "none") {
    pty?.sendInput(new TextEncoder().encode(pageKeySeq(up ? "up" : "down")))
    return
  }
  // Replayed as real wheel events so xterm encodes them the way the app asked
  // (see `lib/termmouse.ts`); with no finger to take a point from, the
  // terminal's centre stands in for one.
  const element = term.element
  if (!element) return
  const lines = Math.max(1, term.rows - 1)
  const { clientX, clientY } = rectCenter(element.getBoundingClientRect())
  dispatchMouseReplay(
    element,
    wheelReplaySteps(up ? -lines : lines),
    clientX,
    clientY,
  )
}

// Where typing focus belongs right now: the compose textarea while the compose
// bar is up, xterm's hidden textarea otherwise.
export function focusTypingSurfaceIn(refs: TypingSurfaceRefs): void {
  if (refs.live.current.composeActive && refs.composeInputRef.current) {
    refs.composeInputRef.current.focus()
  } else {
    refs.termRef.current?.focus()
  }
}

// Whether the active typing surface holds focus right now. Accessory-key
// handlers read it at tap time and refocus only when it was already focused: a
// key tap must never change the soft keyboard's state.
export function typingSurfaceHasFocusIn(refs: TypingSurfaceRefs): boolean {
  const active = document.activeElement
  if (active === null) return false
  if (refs.live.current.composeActive && refs.composeInputRef.current !== null) {
    return active === refs.composeInputRef.current
  }
  return active === (refs.termRef.current?.textarea ?? null)
}

/// May an automatic focus move happen right now? Focusing summons the soft
/// keyboard, so every condition waits for the pane to have reconciled:
///
///   - `ownershipConfirmed` is the server's answer; `isOwner` alone is the
///     foreground guess taken before the handshake.
///   - the replay for the current attach epoch must be on screen, or the
///     keyboard covers a placeholder.
///   - an IME composition in flight is never interrupted, or the half-typed
///     text and its candidate popup are destroyed.
///
/// Automatic moves only: a user tapping the box focuses it themselves.
export function typingFocusAllowed(ctx: {
  isOwner: boolean
  ownershipConfirmed: boolean
  replayApplied: boolean
  composing: boolean
}): boolean {
  if (!ctx.isOwner) return false
  if (!ctx.ownershipConfirmed) return false
  if (!ctx.replayApplied) return false
  return !ctx.composing
}

/// Once per attach, not once per epoch: `typingFocusAllowed` is a permission,
/// not an occasion, and its `replayApplied` input flips on every socket reopen.
/// `attach` names what a move is owed for (the target, plus which typing surface
/// is up), so a reconnect onto the same one is silent.
///
/// Two rules on top of that:
///
///   - Losing ownership forgets, so a take-over after a demotion still focuses.
///   - A refusal is not a consumption: a move blocked by an in-flight IME
///     composition leaves the memory alone, or the attach loses its one move.
export function nextTypingFocus(ctx: {
  /// `typingFocusAllowed` for this same commit.
  allowed: boolean
  isOwner: boolean
  /// What a move would be owed FOR, as an opaque key.
  attach: string
  /// What the pane last moved the keyboard for, or null.
  focusedFor: string | null
}): { focus: boolean; focusedFor: string | null } {
  if (!ctx.allowed) {
    return { focus: false, focusedFor: ctx.isOwner ? ctx.focusedFor : null }
  }
  if (ctx.focusedFor === ctx.attach) {
    return { focus: false, focusedFor: ctx.focusedFor }
  }
  return { focus: true, focusedFor: ctx.attach }
}

export type InputSurfaceDeps = TypingSurfaceRefs & {
  ptyRef: { current: PtySocket | null }
  ownership: OwnershipVerdict
  /// The pane target this draft belongs to: an agent tab id or a terminal id.
  /// Drafts are keyed by it, so switching agents keeps each pane's own text.
  targetId: string
}

export type InputSurface = {
  /// The latch's visible state, for the accessory bar's highlight.
  ctrl: boolean
  alt: boolean
  /// The latch channel, whose only writer is this unit.
  mods: ModifierLatch
  /// The compose draft. It lives in the store, keyed by target id, so it
  /// survives the bar unmounting, the pane remounting on a reconnect, and a
  /// `pagehide`/`pageshow` round trip.
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
  const { live, composeInputRef, termRef, ptyRef, ownership, targetId } = deps
  const refs: TypingSurfaceRefs = { live, composeInputRef, termRef }
  const focusTypingSurface = () => focusTypingSurfaceIn(refs)
  const typingSurfaceHasFocus = () => typingSurfaceHasFocusIn(refs)

  // Sticky (one-shot latched) soft-keyboard modifiers. The state drives the
  // highlight; the ref mirrors it for the lifecycle's stable `onData` closure,
  // which would capture a stale value. The channel is the only writer of both.
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

  const composeText = composeDraft(useDux(), targetId)
  const setComposeText = (value: string) => setComposeDraft(targetId, value)
  // Where the caret should land after a programmatic draft splice: a controlled
  // textarea re-renders and the browser parks the caret at the end of the new
  // value. Null means no pending placement; ordinary typing never sets it.
  const pendingComposeCaretRef = useRef<number | null>(null)
  useEffect(() => {
    const caret = pendingComposeCaretRef.current
    if (caret === null) return
    pendingComposeCaretRef.current = null
    composeInputRef.current?.setSelectionRange(caret, caret)
  }, [composeText, composeInputRef])

  // Right-click pastes the browser clipboard, gated on ownership. `readText`
  // needs a secure context; the shared reader toasts a hint when it is refused.
  //
  // The destination is the typing surface, not always the terminal: while the
  // message box is up a right-click joins the draft, or the clipboard goes on
  // the wire behind an unsent draft.
  function onRightClickPaste() {
    if (!ownership.read()) return
    if (live.current.composeActive && composeInputRef.current !== null) {
      void pasteClipboardText(insertComposeText, focusTypingSurface)
      return
    }
    const term = termRef.current
    if (term) void pasteIntoTerm(term, focusTypingSurface)
  }

  // Splice text into the compose draft at the caret. Shared by everything that
  // puts text there without typing it (the `composeInsert` sink, a pasted
  // image's path), so the caret handling and the refocus cannot drift.
  function insertComposeText(text: string) {
    // Read up front, once: the splice below may run more than once and must
    // splice the same way each time. `insertIntoComposeDraft` appends on null.
    const el = composeInputRef.current
    const selectionStart = el === null ? null : el.selectionStart
    const selectionEnd = el === null ? null : el.selectionEnd
    const { next, caret } = insertIntoComposeDraft(
      // Read at CALL time, not off this render's closure: the sink is registered
      // once and outlives every keystroke after it.
      peekComposeDraft(targetId),
      selectionStart,
      selectionEnd,
      text,
    )
    // Applied by the caret-placement effect once the new value reaches the DOM.
    // The store write is idempotent, so a double invoke splices once.
    pendingComposeCaretRef.current = caret
    setComposeText(next)
    // The draft the text just joined is where editing continues.
    focusTypingSurface()
  }

  const encoder = new TextEncoder()

  // The compose bar's Send. `composeSendWrites` holds the plan: the macro
  // keystroke convention as the body write, and the submitting bare CR as a
  // separate write delivered `COMPOSE_SUBMIT_DELAY_MS` later, deliberately not
  // bracketed paste, because a receiving CLI's stdin debounce would swallow a
  // same-window CR into the paste as a newline. An empty buffer sends one bare CR.
  //
  // Returns whether the send happened; the bar clears its buffer only on true.
  // A composed message can be minutes of typing, so every refusal keeps the
  // buffer and toasts the reason: not the owner, socket not open, or over
  // `MAX_COMPOSE_SEND_BYTES` (an oversized frame aborts the whole socket).
  //
  // It does not consume the one-shot Ctrl/Alt latches: a latch arms the next
  // direct key, and a composed message is not a key.
  //
  // The refusals share the fixed `compose-send` id, so one press repeated
  // against a dead socket raises one toast and a changed reason replaces the
  // old one. The cost, accepted: each attempt restarts the error's countdown.
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
    // The send is committed once the body is written, so the delayed CR is a
    // bare PTY write with no further effects. It is skipped when the pane has
    // unmounted (its cleanup nulls `ptyRef`) or the socket has dropped, rather
    // than delivered to a socket this pane no longer drives.
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
    // Full sequences rather than single chars, so they bypass `applyModifiers`;
    // a latched Alt still prefixes ESC, and Ctrl on a non-char key is consumed.
    // Owner-gated: a bar key is input like any other.
    if (!ownership.read()) return
    // Captured before acting: the refocus below must run only when the typing
    // surface had focus at tap time.
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

  // The accessory bar's soft-newline key, since a soft keyboard cannot produce
  // Shift-Enter. Owner-gated, and it consumes any armed latch because a raw
  // newline does not combine with one. Shares `writeSoftNewline` with the
  // physical Shift-Enter handler so both land input identically.
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

  // Scroll the xterm viewport from the accessory bar. On the normal buffer these
  // drive xterm's own scrollback; on the alt-screen there is none, so PgUp/PgDn
  // forward a page to the app itself: wheel events while it tracks the mouse,
  // the keys otherwise.
  //
  // Scrolling is a read gesture, so it drops focus and lets the soft keyboard
  // go: on iOS the textarea stays focused after a keyboard swipe-down, so a
  // later tap on a focus-retaining button pops it back up. The input keys keep
  // focus instead; the split is input versus page-scroll, not row versus row.
  function onScroll(dir: ScrollDir) {
    const term = termRef.current
    if (!term) return
    const up = dir === "pageUp"
    const altScreen = term.buffer.active.type !== "normal"
    // Forwarding is input, so it is owner-gated; a watcher falls through to the
    // local scroll, which is a no-op on the alt-screen.
    if (altScreen && ownership.read()) {
      forwardPageToApp(term, ptyRef.current, up)
    } else {
      term.scrollPages(up ? -1 : 1)
    }
    // Only a touch device has a soft keyboard to dismiss; without the gate a
    // narrow-window mouse user silently loses terminal focus when paging. A
    // page-scroll is a reading gesture on either typing surface.
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
