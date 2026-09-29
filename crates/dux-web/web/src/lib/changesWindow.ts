// React-free windowing for the Changes pane. The pane lists every changed file,
// which can be tens of thousands of rows (an unignored dependency folder), so it
// mounts only the rows near the viewport. The list is flat: a heading per
// section, that section's rows while it is open, and a separator between the
// two sections when both have rows.

import type { ChangedFileView } from "./types"

export type ChangesSection = "staged" | "unstaged"

export type ChangesListItem =
  | { kind: "header"; key: string; section: ChangesSection }
  | { kind: "row"; key: string; section: ChangesSection; file: ChangedFileView }
  | { kind: "separator"; key: string }

export type ChangesItemKind = ChangesListItem["kind"]

// The height each kind of item occupies, its spacing included. Measured from
// the DOM once one of each is mounted, since the rows grow on a phone and under
// a coarse pointer through CSS alone.
export type ChangesItemHeights = Record<ChangesItemKind, number>

export interface ChangesSectionInput {
  files: ChangedFileView[]
  open: boolean
}

// The flat list. A section with no (filtered) files renders nothing at all,
// heading included, and the separator appears only between two sections that
// both have rows to show.
export function buildChangesItems(sections: {
  staged: ChangesSectionInput
  unstaged: ChangesSectionInput
}): ChangesListItem[] {
  const items: ChangesListItem[] = []
  const push = (section: ChangesSection, input: ChangesSectionInput) => {
    if (input.files.length === 0) return
    items.push({ kind: "header", key: `header:${section}`, section })
    if (!input.open) return
    for (const file of input.files) {
      items.push({ kind: "row", key: `${section}:${file.path}`, section, file })
    }
  }
  push("staged", sections.staged)
  if (sections.staged.files.length > 0 && sections.unstaged.files.length > 0) {
    items.push({ kind: "separator", key: "separator" })
  }
  push("unstaged", sections.unstaged)
  return items
}

// Each item's top offset, plus one trailing entry holding the total height.
export function layoutChangesItems(
  items: ChangesListItem[],
  heights: ChangesItemHeights,
): number[] {
  const offsets = new Array<number>(items.length + 1)
  let top = 0
  for (let index = 0; index < items.length; index += 1) {
    offsets[index] = top
    top += heights[items[index]!.kind]
  }
  offsets[items.length] = top
  return offsets
}

// The first index whose item ends below `y`, by binary search over the offsets.
function firstEndingBelow(offsets: number[], y: number): number {
  let low = 0
  let high = offsets.length - 1
  while (low < high) {
    const mid = (low + high) >> 1
    if (offsets[mid + 1]! <= y) low = mid + 1
    else high = mid
  }
  return low
}

// The indices to mount: everything intersecting the viewport, `overscan` more
// on each side, and the pinned index (the row holding keyboard focus) wherever
// it is, so scrolling a focused row away never unmounts it and drops focus to
// the page. Always ascending, so the DOM order, and with it the Tab order,
// matches the visual order.
export function visibleChangesIndices(
  offsets: number[],
  scrollTop: number,
  viewportHeight: number,
  overscan: number,
  pinned: number | null = null,
): number[] {
  const count = offsets.length - 1
  if (count <= 0) return []
  const top = Math.max(0, scrollTop)
  const first = Math.max(
    0,
    Math.min(count - 1, firstEndingBelow(offsets, top)) - overscan,
  )
  const last = Math.min(
    count - 1,
    firstEndingBelow(offsets, top + Math.max(0, viewportHeight)) + overscan,
  )
  const indices: number[] = []
  if (pinned !== null && pinned >= 0 && pinned < first) indices.push(pinned)
  for (let index = first; index <= last; index += 1) indices.push(index)
  if (pinned !== null && pinned > last && pinned < count) indices.push(pinned)
  return indices
}
