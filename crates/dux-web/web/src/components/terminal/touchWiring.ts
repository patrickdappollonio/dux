import type { RefObject } from "react"
import type { Terminal } from "@xterm/xterm"

import { copyTermSelection } from "@/lib/termClipboard"
import { copyOnSelectAction } from "@/lib/termkeys"
import { dragScrollLines, dragWheelReport } from "@/lib/viewport"
import {
  dispatchMouseReplay,
  tapReplaySteps,
  wheelReplaySteps,
} from "@/lib/termmouse"
import {
  activateLinkAtPoint,
  linkifierElement,
  terminalTapAction,
} from "@/lib/termlink"

import type { LiveSettings } from "./liveValues"
import type { LinkPress } from "./linkPress"
import { mouseCaptureHintShown } from "./pageSessionHints"
import type { ResizeCoordinator } from "./resizeCoordinator"
import { createSelectionDrag } from "./selectionDrag"
import {
  createTouchGesture,
  type TouchGestureOutcome,
} from "./touchGesture"

type TerminalTouchWiringOptions = {
  term: Terminal
  host: HTMLDivElement
  container: HTMLDivElement
  composeInputRef: RefObject<HTMLTextAreaElement | null>
  live: LiveSettings
  ownership: { read: () => boolean }
  resize: ResizeCoordinator
  links: LinkPress
}

type Disposable = { dispose: () => void }

function forwardsWheel(options: TerminalTouchWiringOptions): boolean {
  return (
    options.term.buffer.active.type !== "normal" &&
    options.ownership.read() &&
    options.term.modes.mouseTrackingMode !== "none"
  )
}

function scrollAllowed(options: TerminalTouchWiringOptions): boolean {
  if (
    options.live.current.viewerOverflow &&
    options.host.scrollHeight > options.host.clientHeight
  ) {
    return false
  }
  return options.term.buffer.active.type === "normal" || forwardsWheel(options)
}

function renderedRowHeight(options: TerminalTouchWiringOptions): number {
  const screen = options.term.element?.querySelector(".xterm-screen")
  const height =
    screen instanceof HTMLElement && screen.clientHeight > 0
      ? screen.clientHeight
      : options.container.clientHeight
  return height / options.term.rows
}

function moveScroll(
  options: TerminalTouchWiringOptions,
  accumulatedPixels: number,
  touch: Touch,
): number {
  const rowHeight = renderedRowHeight(options)
  const { scrollLines, remainderPx } = dragScrollLines(
    accumulatedPixels,
    rowHeight,
  )
  if (scrollLines === 0) return accumulatedPixels
  if (forwardsWheel(options)) {
    const { notch } = dragWheelReport(accumulatedPixels, rowHeight)
    dispatchMouseReplay(
      options.term.element,
      wheelReplaySteps(notch),
      touch.clientX,
      touch.clientY,
    )
  } else {
    options.term.scrollLines(scrollLines)
  }
  return remainderPx
}

function copyTouchSelection(options: TerminalTouchWiringOptions): void {
  const action = copyOnSelectAction({
    copyOnSelect: options.live.current.copyOnSelect,
    selection: options.term.getSelection(),
    dragged: true,
    mouseTrackingMode: options.term.modes.mouseTrackingMode,
    hintShown: mouseCaptureHintShown(),
    gesture: "long-press",
  })
  if (action === "copy") copyTermSelection(options.term, () => {})
}

function activateTouchLink(
  options: TerminalTouchWiringOptions,
  touch: Touch | undefined,
): boolean {
  if (!touch) return false
  return activateLinkAtPoint(
    linkifierElement(options.term.element),
    touch.clientX,
    touch.clientY,
    options.links.activations,
  )
}

function replayTouchClick(
  options: TerminalTouchWiringOptions,
  touch: Touch,
  focusCompose: boolean,
): void {
  const focusedBefore = document.activeElement
  dispatchMouseReplay(
    options.term.element,
    tapReplaySteps(),
    touch.clientX,
    touch.clientY,
  )
  if (!focusCompose && focusedBefore instanceof HTMLElement) {
    focusedBefore.focus()
  }
}

function handleTap(
  options: TerminalTouchWiringOptions,
  event: TouchEvent,
): void {
  if (options.term.hasSelection()) options.term.clearSelection()
  if (!options.live.current.composeActive || !options.ownership.read()) return
  const compose = options.composeInputRef.current
  if (!compose) return

  event.preventDefault()
  const touch = event.changedTouches[0]
  const action = terminalTapAction({
    linkActivated: activateTouchLink(options, touch),
    mouseTracking: options.term.modes.mouseTrackingMode !== "none",
  })
  if (touch && action.forwardClick) {
    replayTouchClick(options, touch, action.focusCompose)
  }
  if (action.focusCompose) compose.focus()
}

function handleLift(
  options: TerminalTouchWiringOptions,
  outcome: TouchGestureOutcome,
  event: TouchEvent,
): void {
  if (outcome.wasSelecting) {
    event.preventDefault()
    copyTouchSelection(options)
    return
  }
  if (outcome.wasTap) handleTap(options, event)
}

export function registerTerminalTouchWiring(
  options: TerminalTouchWiringOptions,
): Disposable {
  const selection = createSelectionDrag(options.term)
  const gesture = createTouchGesture({
    scrollAllowed: () => scrollAllowed(options),
    onGestureReset: () => {
      selection.end()
      options.resize.setHolding(false)
    },
    onGestureFinished: () => options.resize.flushHeld(),
    onLongPress: (touch) => selection.begin(touch),
    onSelectMove: (touch) => selection.extend(touch),
    onScrollStart: () => {
      options.resize.setHolding(true)
      options.term.textarea?.blur()
      options.composeInputRef.current?.blur()
    },
    onScrollMove: (pixels, touch) => moveScroll(options, pixels, touch),
    onLift: (outcome, event) => handleLift(options, outcome, event),
  })
  gesture.attach(options.container)
  return { dispose: () => gesture.dispose() }
}
