// A developer readout of the phone's viewport geometry (`ViewportDebug`). The
// switch is the page's own query, before the `#`, which no in-app move rewrites.

export const VIEWPORT_DEBUG_PARAM = "debug"
export const VIEWPORT_DEBUG_VALUE = "viewport"

export function viewportDebugEnabled(search: string): boolean {
  return new URLSearchParams(search).get(VIEWPORT_DEBUG_PARAM) === VIEWPORT_DEBUG_VALUE
}

export interface EdgeRect {
  top: number
  bottom: number
}

export interface ClipCandidate {
  label: string
  rect: EdgeRect
  clips: boolean
}

// Rounding in the browser's own layout leaves boxes a fraction of a pixel past
// their container; that is not a cut anyone can see.
const CUT_SLACK_PX = 0.5

/** The nearest clipping ancestor (ancestors ordered nearest first) that hides part of the box's bottom. */
export function findCut(
  box: EdgeRect,
  ancestors: readonly ClipCandidate[],
): { by: string; px: number } | null {
  for (const ancestor of ancestors) {
    if (!ancestor.clips) continue
    const px = box.bottom - ancestor.rect.bottom
    if (px > CUT_SLACK_PX) return { by: ancestor.label, px: Math.round(px * 10) / 10 }
  }
  return null
}

export type SizeEventType = "vv-resize" | "vv-scroll" | "win-resize"

export interface SizeEvent {
  /** Milliseconds since the readout mounted. */
  t: number
  type: SizeEventType
  height: number
}

export const SIZE_EVENT_LOG_LENGTH = 8

export function pushSizeEvent(log: readonly SizeEvent[], event: SizeEvent): SizeEvent[] {
  return [event, ...log].slice(0, SIZE_EVENT_LOG_LENGTH)
}
