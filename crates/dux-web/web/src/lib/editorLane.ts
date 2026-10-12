// One operation per file at a time.
//
// A save, a conversion, a reopen in another encoding and a reload from disk all
// change what a buffer holds, or what its labels say, when their answers land.
// Run two of them on one file at once and the later answer is about bytes the
// earlier one has already replaced; every race these have produced was that
// shape. So they share one serial lane per file: an operation takes the lane
// before it starts and gives it back when its answer has been applied or
// dropped, and while it is held the others are offered as unavailable (or, for
// a freshness reload nobody asked for, simply not started).
//
// The lanes live at page scope, not in the editor component, because an
// operation outlives the component that started it: closing the editor while a
// save is out and opening it again must still find that file busy.

import { useSyncExternalStore } from "react"

export type LaneOp = "save" | "convert" | "reopen" | "reload" | "freshness"

const lanes = new Map<string, { op: LaneOp; id: number }>()
const listeners = new Set<() => void>()
let nextId = 0

function changed(): void {
  for (const listener of listeners) listener()
}

// A file's lane key: the editor root (a namespaced key) and the path.
export function laneKey(rootKey: string, path: string): string {
  return `${rootKey}\u0000${path}`
}

// Take the file's lane for `op`. Returns the release, or null when another
// operation holds it. The release is single-use and only ever frees the lane
// this call took, so a late duplicate release cannot free a later holder.
export function acquireLane(key: string, op: LaneOp): (() => void) | null {
  if (lanes.has(key)) return null
  nextId += 1
  const id = nextId
  lanes.set(key, { op, id })
  changed()
  let released = false
  return () => {
    if (released) return
    released = true
    if (lanes.get(key)?.id === id) {
      lanes.delete(key)
      changed()
    }
  }
}

// The operation holding the file's lane, if any.
export function laneBusy(key: string): LaneOp | undefined {
  return lanes.get(key)?.op
}

export function subscribeLanes(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

// The operation holding a file's lane, for rendering: re-renders when it
// changes, so the controls that would start a second operation are disabled.
export function useLaneBusy(key: string | null): LaneOp | undefined {
  return useSyncExternalStore(subscribeLanes, () =>
    key === null ? undefined : laneBusy(key),
  )
}

// For tests only: the lanes are page-lifetime state.
export function resetLanes(): void {
  lanes.clear()
  changed()
}

// How the busy operation is named in words.
export function laneOpWords(op: LaneOp): string {
  switch (op) {
    case "save":
      return "a save"
    case "convert":
      return "a save in another encoding"
    case "reopen":
      return "a reopen in another encoding"
    case "reload":
    case "freshness":
      return "a reload from disk"
  }
}

// How long an operation may hold its file's lane waiting for the server. A
// request can hang with nothing ever coming back (the path swapped for a
// FIFO, a server wedged mid-answer), and a lane held forever leaves the file
// with no save, reload or reopen for as long as the page lives, closing the
// editor included. Generous on purpose: a real save or read of an editable
// file never comes near it.
export const LANE_DEADLINE_MS = 60_000

// An operation the server never answered within the deadline. A save's text
// may still have been written; a read's answer is simply never applied.
export class LaneDeadlineError extends Error {
  readonly op: LaneOp
  constructor(op: LaneOp, ms: number) {
    const seconds = Math.round(ms / 1000)
    super(
      op === "save" || op === "convert"
        ? `the server didn't answer within ${seconds} seconds, so dux stopped waiting; the save may or may not have been written. Reload the file from disk to see what it holds`
        : `the server didn't answer within ${seconds} seconds, so dux stopped waiting and applied nothing`,
    )
    this.name = "LaneDeadlineError"
    this.op = op
  }
}

// `request`, or a LaneDeadlineError once `ms` passes without an answer. An
// answer that arrives after that is never seen by whoever awaited this: the
// operation has already ended, and its lane been given back.
export function withLaneDeadline<T>(
  request: Promise<T>,
  op: LaneOp,
  ms: number = LANE_DEADLINE_MS,
): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => reject(new LaneDeadlineError(op, ms)), ms)
    request.then(
      (value) => {
        clearTimeout(timer)
        resolve(value)
      },
      (error: unknown) => {
        clearTimeout(timer)
        reject(error)
      },
    )
  })
}
