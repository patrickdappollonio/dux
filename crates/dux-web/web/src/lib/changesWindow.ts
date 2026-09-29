// React-free windowing for the Changes pane. The pane lists every changed file,
// which can be tens of thousands of rows (an unignored dependency folder), so it
// mounts only the rows near the viewport. The list is flat: a heading per
// section, that section's rows while it is open, and a separator between the
// two sections when both have rows. An expanded folder's contents are flattened
// into it right under the folder, a level deeper, so an expansion of hundreds
// of rows is windowed like everything else.

import {
  NO_EXPANSIONS,
  folderKey,
  isExpandable,
  type Expansions,
} from "./changesTree"
import type { ChangedFileView } from "./types"

export type ChangesSection = "staged" | "unstaged"

export type ChangesListItem =
  | { kind: "header"; key: string; section: ChangesSection }
  | {
      kind: "row"
      key: string
      section: ChangesSection
      file: ChangedFileView
      // 0 for a row of the listing itself, one more per expanded folder above.
      depth: number
      // A folded folder whose contents are showing under it.
      expanded: boolean
    }
  // An expanded folder whose contents are being asked for.
  | { kind: "loading"; key: string; section: ChangesSection; dir: string; depth: number }
  // An expanded folder whose contents could not be listed.
  | {
      kind: "failed"
      key: string
      section: ChangesSection
      dir: string
      depth: number
      message: string
    }
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
// both have rows to show. Under each expanded folder come its contents, or a
// loading row until they land, or a failure row when they could not be listed.
export function buildChangesItems(sections: {
  staged: ChangesSectionInput
  unstaged: ChangesSectionInput
  expansions?: Expansions
}): ChangesListItem[] {
  const expansions = sections.expansions ?? NO_EXPANSIONS
  const items: ChangesListItem[] = []
  const pushRows = (
    section: ChangesSection,
    files: readonly ChangedFileView[],
    depth: number,
  ) => {
    for (const file of files) {
      const node = isExpandable(file)
        ? expansions.get(folderKey(section, file.path))
        : undefined
      items.push({
        kind: "row",
        key: `${section}:${file.path}`,
        section,
        file,
        depth,
        expanded: node !== undefined,
      })
      if (!node) continue
      if (node.children !== null) {
        pushRows(section, node.children, depth + 1)
      } else if (node.error !== null) {
        items.push({
          kind: "failed",
          key: `${section}:${file.path}:failed`,
          section,
          dir: file.path,
          depth: depth + 1,
          message: node.error,
        })
      } else {
        items.push({
          kind: "loading",
          key: `${section}:${file.path}:loading`,
          section,
          dir: file.path,
          depth: depth + 1,
        })
      }
    }
  }
  const push = (section: ChangesSection, input: ChangesSectionInput) => {
    if (input.files.length === 0) return
    items.push({ kind: "header", key: `header:${section}`, section })
    if (!input.open) return
    pushRows(section, input.files, 0)
  }
  push("staged", sections.staged)
  if (sections.staged.files.length > 0 && sections.unstaged.files.length > 0) {
    items.push({ kind: "separator", key: "separator" })
  }
  push("unstaged", sections.unstaged)
  return items
}

// The list's outline in reading order: a heading or the separator by its item
// index, and each open section's rows as one contiguous range, so the pane can
// hold a section's rows in one container its heading points at.
export type ChangesListPart =
  | { kind: "header" | "separator"; index: number }
  | { kind: "rows"; section: ChangesSection; first: number; last: number }

export function changesListStructure(items: ChangesListItem[]): ChangesListPart[] {
  const parts: ChangesListPart[] = []
  for (let index = 0; index < items.length; index += 1) {
    const item = items[index]!
    if (item.kind === "header" || item.kind === "separator") {
      parts.push({ kind: item.kind, index })
      continue
    }
    const previous = parts.at(-1)
    if (previous?.kind === "rows" && previous.section === item.section) {
      previous.last = index
    } else {
      parts.push({ kind: "rows", section: item.section, first: index, last: index })
    }
  }
  return parts
}

// A section's rows as a tree: each expanded folder's contents (its rows, and
// its loading or failure row) as one contiguous range right after the folder's
// own row, nested the way the folders are. The pane gives each range a
// container of its own, which is what a folder's toggle points at. Rows
// between containers come as ranges, never one part per row, so walking the
// tree costs the number of expanded folders, not the number of rows.
export type ChangesRowPart =
  | { kind: "items"; first: number; last: number }
  | { kind: "folder"; key: string; first: number; last: number; parts: ChangesRowPart[] }

export function changesRowTree(
  items: ChangesListItem[],
  first: number,
  last: number,
): ChangesRowPart[] {
  const depthOf = (index: number): number => {
    const item = items[index]!
    return item.kind === "header" || item.kind === "separator" ? 0 : item.depth
  }
  const build = (from: number, to: number): ChangesRowPart[] => {
    const parts: ChangesRowPart[] = []
    let index = from
    while (index <= to) {
      const previous = parts.at(-1)
      if (previous?.kind === "items" && previous.last === index - 1) previous.last = index
      else parts.push({ kind: "items", first: index, last: index })
      const item = items[index]!
      index += 1
      if (item.kind !== "row" || !item.expanded) continue
      // The folder's contents run while the depth stays below the folder's.
      const start = index
      while (index <= to && depthOf(index) > item.depth) index += 1
      if (index > start) {
        parts.push({
          kind: "folder",
          key: item.key,
          first: start,
          last: index - 1,
          parts: build(start, index - 1),
        })
      }
    }
    return parts
  }
  return build(first, last)
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
