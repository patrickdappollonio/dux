import { useEffect, useRef, useState } from "react"
import type {
  GroupProps,
  PanelImperativeHandle,
  PanelProps,
} from "react-resizable-panels"

import { DIVIDER_DRAG_THRESHOLD_PX } from "@/lib/paneDivider"
import {
  CHANGES_PANE_COLLAPSE_EPSILON,
  CHANGES_PANE_DEFAULT_PERCENT,
  CHANGES_PANE_MIN_PERCENT,
  changesPaneCollapseStep,
  changesPaneMountPercent,
  collapseChangesPaneFromDrag,
  persistChangesPanePercent,
  setChangesPanePercent,
} from "@/lib/store"

// How many animation frames a re-shown Changes pane may wait for the panel
// library to publish its layout. In 4.11.2 the normal answer arrives on the
// first frame; the rest is bounded headroom.
export const CHANGES_PANE_HEAL_FRAMES = 5

interface GestureState {
  pointerDown: boolean
  collapseArmed: boolean
  pointerMoved: boolean
  cancelled: boolean
  origin: { x: number; y: number } | null
  persistPending: number | null
  keyboardStep: boolean
  lastOpenPercent: number
}

/**
 * Own the timing-sensitive interaction state around react-resizable-panels'
 * Changes pane. The hook deliberately keeps the library-facing event targets,
 * capture phases and scheduling primitives in one place: window capture for
 * gesture observation, requestAnimationFrame for re-show healing, a microtask
 * for the synchronous keyboard report, and a macrotask for collapse commit.
 */
export function useChangesPaneController(showChanges: boolean) {
  // `defaultSize` is consumed in the render that mounts the panel. Re-read the
  // remembered value during the hidden -> shown render, not one effect later.
  const [mountPercent, setMountPercent] = useState(changesPaneMountPercent)
  const [mountedShowing, setMountedShowing] = useState(showChanges)
  if (mountedShowing !== showChanges) {
    setMountedShowing(showChanges)
    if (showChanges) setMountPercent(changesPaneMountPercent())
  }

  const panelRef = useRef<PanelImperativeHandle | null>(null)
  const wasShowingRef = useRef(showChanges)
  const reshowPendingRef = useRef(false)
  const collapseCommitRef = useRef<number | null>(null)
  const gestureRef = useRef<GestureState>({
    pointerDown: false,
    collapseArmed: false,
    pointerMoved: false,
    cancelled: false,
    origin: null,
    persistPending: null,
    keyboardStep: false,
    lastOpenPercent: mountPercent,
  })

  // The panel library can retain a cached zero-width layout after the panel is
  // hidden. Its imperative handle is not reliably registered until a later
  // frame and throws while missing, so retry for a bounded number of frames.
  useEffect(() => {
    const reshown = showChanges && !wasShowingRef.current
    wasShowingRef.current = showChanges
    if (!reshown) return

    reshowPendingRef.current = true
    let frames = 0
    let scheduled = requestAnimationFrame(function attempt() {
      const handle = panelRef.current
      if (!handle) {
        reshowPendingRef.current = false
        return
      }

      let percent: number
      try {
        percent = handle.getSize().asPercentage
      } catch (err) {
        frames += 1
        if (frames < CHANGES_PANE_HEAL_FRAMES) {
          scheduled = requestAnimationFrame(attempt)
          return
        }
        reshowPendingRef.current = false
        console.warn(
          "[dux] the Changes panel never published a layout; it reopens at whatever width the panel library kept. Hide and show it again to retry.",
          err,
        )
        return
      }

      reshowPendingRef.current = false
      if (percent >= CHANGES_PANE_COLLAPSE_EPSILON) return
      try {
        handle.resize(`${mountPercent}%`)
      } catch (err) {
        console.warn(
          "[dux] the Changes panel refused to resize back to its default width; it reopens at nothing. Hide and show it again to retry.",
          err,
        )
        return
      }
      setChangesPanePercent(mountPercent)
    })

    return () => {
      cancelAnimationFrame(scheduled)
      reshowPendingRef.current = false
    }
  }, [showChanges, mountPercent])

  // Committing from pointerup itself would unmount the separator before the
  // panel library's document-capture listener finishes. A macrotask preserves
  // the original listener order and lets the library finish first.
  const scheduleCollapseCommit = () => {
    if (collapseCommitRef.current !== null) return
    collapseCommitRef.current = window.setTimeout(() => {
      collapseCommitRef.current = null
      collapseChangesPaneFromDrag()
    }, 0)
  }

  useEffect(
    () => () => {
      if (collapseCommitRef.current !== null) {
        clearTimeout(collapseCommitRef.current)
      }
    },
    [],
  )

  const restoreSplit = () => {
    const handle = panelRef.current
    if (!handle) return
    const percent = Math.max(
      gestureRef.current.lastOpenPercent,
      CHANGES_PANE_MIN_PERCENT,
    )
    try {
      handle.resize(`${percent}%`)
    } catch (err) {
      console.warn(
        "[dux] the Changes panel refused to go back to the width it was at before a cancelled gesture. Use the header's Changes button to reopen it.",
        err,
      )
      return
    }
    setChangesPanePercent(percent)
  }

  // Observe the same global events, on the same target and in the same capture
  // phase, as the previous inline controller. The verdict intentionally
  // survives pointerup/cancel because the library's bad zero-width report can
  // arrive later; the next pointerdown resets it.
  useEffect(() => {
    const onDown = (event: Event) => {
      const gesture = gestureRef.current
      gesture.pointerDown = true
      gesture.pointerMoved = false
      gesture.cancelled = false
      const point = event as PointerEvent
      gesture.origin =
        typeof point.clientX === "number"
          ? { x: point.clientX, y: point.clientY }
          : null
    }

    const onMove = (event: Event) => {
      const gesture = gestureRef.current
      if (!gesture.pointerDown || gesture.pointerMoved) return
      const point = event as PointerEvent
      if (gesture.origin === null || typeof point.clientX !== "number") {
        gesture.pointerMoved = true
        return
      }
      const dx = Math.abs(point.clientX - gesture.origin.x)
      const dy = Math.abs(point.clientY - gesture.origin.y)
      if (Math.max(dx, dy) >= DIVIDER_DRAG_THRESHOLD_PX) {
        gesture.pointerMoved = true
      }
    }

    const onUp = () => {
      const gesture = gestureRef.current
      gesture.pointerDown = false
      const pending = gesture.persistPending
      gesture.persistPending = null
      if (pending !== null) persistChangesPanePercent(pending)
      if (!gesture.collapseArmed) return
      gesture.collapseArmed = false
      scheduleCollapseCommit()
    }

    const onCancel = () => {
      const gesture = gestureRef.current
      gesture.pointerDown = false
      gesture.cancelled = true
      gesture.persistPending = null
      gesture.collapseArmed = false
    }

    const onKeyDown = () => {
      gestureRef.current.keyboardStep = true
      queueMicrotask(() => {
        gestureRef.current.keyboardStep = false
      })
    }

    window.addEventListener("pointerdown", onDown, true)
    window.addEventListener("pointermove", onMove, true)
    window.addEventListener("pointerup", onUp, true)
    window.addEventListener("pointercancel", onCancel, true)
    window.addEventListener("keydown", onKeyDown, true)
    return () => {
      window.removeEventListener("pointerdown", onDown, true)
      window.removeEventListener("pointermove", onMove, true)
      window.removeEventListener("pointerup", onUp, true)
      window.removeEventListener("pointercancel", onCancel, true)
      window.removeEventListener("keydown", onKeyDown, true)
    }
  }, [])

  const onLayoutChange: NonNullable<GroupProps["onLayoutChange"]> = (layout) => {
    const percent = layout["changes-pane"]
    setChangesPanePercent(percent ?? CHANGES_PANE_DEFAULT_PERCENT)
    if (percent === undefined) return
    const gesture = gestureRef.current
    if (gesture.pointerDown) {
      gesture.persistPending = percent
    } else if (gesture.keyboardStep) {
      persistChangesPanePercent(percent)
    }
  }

  const onResize: NonNullable<PanelProps["onResize"]> = (
    size,
    _id,
    prevSize,
  ) => {
    const gesture = gestureRef.current
    const percent = size.asPercentage
    const step = changesPaneCollapseStep({
      percent,
      prevPercent: prevSize?.asPercentage,
      pointerDown: gesture.pointerDown,
      armed: gesture.collapseArmed,
      reshowPending: reshowPendingRef.current,
      pointerMoved: gesture.pointerMoved,
      cancelled: gesture.cancelled,
      keyboardStep: gesture.keyboardStep,
    })

    if (percent >= CHANGES_PANE_COLLAPSE_EPSILON) {
      gesture.lastOpenPercent = percent
    }
    if (step === "arm") {
      gesture.collapseArmed = true
    } else if (step === "disarm") {
      gesture.collapseArmed = false
    } else if (step === "commit") {
      gesture.collapseArmed = false
      scheduleCollapseCommit()
    } else if (step === "restore") {
      gesture.collapseArmed = false
      gesture.persistPending = null
      restoreSplit()
    }
  }

  return { mountPercent, panelRef, onLayoutChange, onResize }
}
