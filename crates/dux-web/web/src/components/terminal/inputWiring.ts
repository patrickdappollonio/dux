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
 * While the message box is up, xterm does not hold the keyboard: a focus landing
 * anywhere in the pane is redirected into the box, so a mouse cannot click into
 * the terminal and type past the buffer. One rule at the focus edge rather than a
 * per-key filter, so selection, copy, links and mouse gestures are untouched.
 * Listens on the pane's CONTAINER, which is the element this pane owns.
 */
export function registerComposeFocusGuard(options: {
  container: HTMLElement
  composeActive: () => boolean
  focusTypingSurface: () => void
}): Disposable {
  let disposed = false
  const onFocusIn = () => {
    if (!options.composeActive()) return
    // Deferred by a microtask, which is load-bearing on Linux: xterm publishes to
    // the X11 PRIMARY selection with a synchronous `focus()` then `select()`, and
    // moving focus between them breaks middle-click paste. Re-asked on the way out,
    // since the surface can switch and the pane can be disposed in between.
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
 * Take xterm's hidden textarea out of the tab order while the message box is the
 * typing surface, and return the restore. The focus guard would otherwise bounce
 * a Shift-Tab out of the box straight back, and the edge cannot tell that event
 * from a click into the terminal: same target, same `relatedTarget`. The tab order
 * can, because a pointer does not consult it.
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
