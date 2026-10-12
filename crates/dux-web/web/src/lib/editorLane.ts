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
