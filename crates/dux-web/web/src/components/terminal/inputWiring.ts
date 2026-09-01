import type { Terminal } from "@xterm/xterm"

import { copyTermSelection } from "@/lib/termClipboard"
import {
  applyModifiers,
  classifyClipboardKey,
  copyOnSelectAction,
  forcesTextPaste,
  softNewlineAction,
} from "@/lib/termkeys"
import { isFocusReport } from "@/lib/suppressViewerReports"
import type { PtySocket } from "@/lib/ptySocket"
import { latin1Bytes } from "@/lib/termmouse"

import type { LiveSettings } from "./liveValues"
import type { ModifierLatch, OwnershipVerdict } from "./channels"
import { DRAG_THRESHOLD_PX, writeSoftNewline } from "./constants"
import {
  mouseCaptureHintShown,
  raiseMouseCaptureHint,
} from "./pageSessionHints"

type TerminalInputWiringOptions = {
  term: Terminal
  pty: PtySocket
  container: HTMLDivElement
  isMac: boolean
  live: LiveSettings
  mods: ModifierLatch
  ownership: OwnershipVerdict
  pointerTypeRef: { current: string }
  replayInFlight: () => boolean
  focusTypingSurface: () => void
  onClipboardPaste: (event: ClipboardEvent) => void
  armForcedTextPaste: () => void
}

type Disposable = { dispose: () => void }

function registerTerminalStreams(
  options: TerminalInputWiringOptions,
): Disposable {
  const encoder = new TextEncoder()
  const dataSub = options.term.onData((input) => {
    if (!options.ownership.read()) return
    if (options.replayInFlight() && isFocusReport(input)) return
    const latch = options.mods.read()
    const modified =
      latch.ctrl || latch.alt ? applyModifiers(input, latch) : input
    if (latch.ctrl || latch.alt) {
      options.mods.write({ ctrl: false, alt: false })
    }
    options.pty.sendInput(encoder.encode(modified))
  })
  const binarySub = options.term.onBinary((input) => {
    if (options.ownership.read()) options.pty.sendInput(latin1Bytes(input))
  })
  return {
    dispose: () => {
      dataSub.dispose()
      binarySub.dispose()
    },
  }
}

function terminalKeyAction(
  event: KeyboardEvent,
  options: TerminalInputWiringOptions,
): boolean {
  const softNewline = softNewlineAction(event, {
    isOwner: options.ownership.read(),
    ctrlLatched: options.mods.read().ctrl,
    altLatched: options.mods.read().alt,
  })
  if (softNewline.handled) {
    event.preventDefault()
    event.stopPropagation()
    if (softNewline.send !== null) {
      if (softNewline.clearLatch) {
        options.mods.write({ ctrl: false, alt: false })
      }
      writeSoftNewline(options.term, options.pty)
    }
    return false
  }
  if (event.type !== "keydown") return true

  const chord = {
    ctrlKey: event.ctrlKey,
    shiftKey: event.shiftKey,
    altKey: event.altKey,
    metaKey: event.metaKey,
    code: event.code,
    keyCode: event.keyCode,
    isMac: options.isMac,
  }
  if (forcesTextPaste(chord)) options.armForcedTextPaste()
  const clipboard = classifyClipboardKey(chord)
  if (clipboard === "passthrough") return true
  if (clipboard === "paste") return false

  void copyTermSelection(options.term, options.focusTypingSurface)
  event.preventDefault()
  return false
}

function registerMouseCopy(
  options: TerminalInputWiringOptions,
): Disposable {
  let mouseDown: { x: number; y: number } | null = null
  const onMouseDown = (event: MouseEvent) => {
    if (event.button === 0) {
      mouseDown = { x: event.clientX, y: event.clientY }
    }
  }
  const onMouseUp = (event: MouseEvent) => {
    const down = mouseDown
    mouseDown = null
    const dragged =
      down !== null &&
      Math.hypot(event.clientX - down.x, event.clientY - down.y) >=
        DRAG_THRESHOLD_PX
    const action = copyOnSelectAction({
      copyOnSelect: options.live.current.copyOnSelect,
      selection: options.term.getSelection(),
      dragged,
      mouseTrackingMode: options.term.modes.mouseTrackingMode,
      hintShown: mouseCaptureHintShown(),
      gesture: "mouse-drag",
    })
    if (action === "copy") {
      void copyTermSelection(options.term, options.focusTypingSurface)
    } else if (action === "hint") {
      raiseMouseCaptureHint(options.isMac)
    }
  }
  options.container.addEventListener("mousedown", onMouseDown)
  options.container.addEventListener("mouseup", onMouseUp)
  return {
    dispose: () => {
      options.container.removeEventListener("mousedown", onMouseDown)
      options.container.removeEventListener("mouseup", onMouseUp)
    },
  }
}

function registerPasteGuards(
  options: TerminalInputWiringOptions,
): Disposable {
  const onContextMenu = () => {
    if (!options.term.textarea) return
    options.term.textarea.value = ""
    if (options.pointerTypeRef.current === "touch") {
      options.term.textarea.blur()
    }
  }
  const onPaste = (event: ClipboardEvent) => options.onClipboardPaste(event)
  options.container.addEventListener("contextmenu", onContextMenu)
  options.container.addEventListener("paste", onPaste, true)
  return {
    dispose: () => {
      options.container.removeEventListener("contextmenu", onContextMenu)
      options.container.removeEventListener("paste", onPaste, true)
    },
  }
}

/**
 * WHILE THE MESSAGE BOX IS UP, XTERM DOES NOT HOLD THE KEYBOARD.
 *
 * The compose bar is THE typing surface when it is on, and everything else in
 * the pane already assumes it: the caret style says "focused" permanently
 * (`inactiveCursorStyle`), a tap on the terminal hands focus back to the box
 * (`touchWiring`), and every automatic focus move routes through
 * `focusTypingSurfaceIn`. A finger has no other way in, so this never mattered
 * until the box could be turned on for a MOUSE, which can click straight into
 * the terminal and start typing past the buffer.
 *
 * One rule at the focus edge rather than a filter on each key: a swallowed
 * keystroke is a keystroke the user has to type again somewhere else, while a
 * redirected focus puts the very next character in the box they asked for.
 * Selecting, copying and the link and mouse gestures are untouched, none of
 * them needs xterm to be focused. The way back is the Direct toggle.
 *
 * Listens on the pane's own CONTAINER for the bubbling `focusin` rather than on
 * xterm's hidden textarea: the container is the element this pane owns, the
 * compose box lives outside it, and nothing else inside it takes focus.
 */
export function registerComposeFocusGuard(options: {
  container: HTMLElement
  composeActive: () => boolean
  focusTypingSurface: () => void
}): Disposable {
  let disposed = false
  const onFocusIn = () => {
    if (!options.composeActive()) return
    // DEFERRED BY A MICROTASK, and the delay is load-bearing on Linux. xterm
    // publishes a terminal selection to the X11 PRIMARY selection by stuffing
    // it into its hidden textarea and calling `focus()` then `select()` back to
    // back, synchronously. Moving focus BETWEEN the two takes the selection
    // away from the element being selected, and middle-click paste into other
    // applications silently stops working for as long as the box is up. A
    // microtask runs after that whole synchronous sequence and still long
    // before the user can type, so nothing else about the redirect changes.
    //
    // Asked AGAIN on the way out: the surface can be switched to Direct, and
    // the pane can be disposed, between the focus and the deferred move.
    queueMicrotask(() => {
      if (disposed || !options.composeActive()) return
      options.focusTypingSurface()
    })
  }
  options.container.addEventListener("focusin", onFocusIn)
  return {
    dispose: () => {
      disposed = true
      options.container.removeEventListener("focusin", onFocusIn)
    },
  }
}

/**
 * TAKE XTERM'S HIDDEN TEXTAREA OUT OF THE TAB ORDER while the message box is
 * the typing surface. Returns the restore.
 *
 * The focus guard above sends every focus landing inside the pane to the
 * message box, which makes xterm's `tabindex="0"` helper textarea a keyboard
 * TRAP: Shift-Tab out of the box lands on it and is bounced straight back, so
 * there is no way to navigate backwards out of the pane at all.
 *
 * The trap cannot be answered at the focus edge, because a Shift-Tab out of the
 * box and a CLICK into the terminal produce the identical `focusin`: same
 * target, same `relatedTarget` (the box, in both cases), and one of the two
 * must still be redirected. So the keyboard answer is the tab ORDER, which a
 * pointer does not consult: a click still focuses a `tabindex="-1"` element and
 * still lands in the box.
 *
 * Reversible rather than permanent: whatever xterm had is put back the moment
 * the box goes away, so the terminal is an ordinary tab stop again for everyone
 * typing straight into it.
 */
export function suspendTerminalTabStop(
  textarea: { tabIndex: number } | null | undefined,
): () => void {
  if (!textarea) return () => {}
  const previous = textarea.tabIndex
  textarea.tabIndex = -1
  return () => {
    textarea.tabIndex = previous
  }
}

export function registerTerminalInputWiring(
  options: TerminalInputWiringOptions,
): Disposable {
  const streams = registerTerminalStreams(options)
  const mouseCopy = registerMouseCopy(options)
  const pasteGuards = registerPasteGuards(options)
  const composeFocus = registerComposeFocusGuard({
    container: options.container,
    composeActive: () => options.live.current.composeActive,
    focusTypingSurface: options.focusTypingSurface,
  })
  options.term.attachCustomKeyEventHandler((event) =>
    terminalKeyAction(event, options),
  )
  return {
    dispose: () => {
      streams.dispose()
      mouseCopy.dispose()
      pasteGuards.dispose()
      composeFocus.dispose()
    },
  }
}
