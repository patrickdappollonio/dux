import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react"

import { isTruncated } from "@/hooks/use-truncated"

/// How much room beyond an exact fit the names need before the folded controls
/// come back. Folding and unfolding each change the room the names have, so
/// without a margin a sub-pixel difference could flip the header on every
/// measurement at the boundary width.
export const FOLD_SLACK_PX = 8

export type FoldMeasure = {
  /// Some name's own text is cut off right now.
  truncated: boolean
  /// The names' natural width, read while none of them is cut off.
  namesWidth: number
  /// The width the names' row has now.
  rowWidth: number
  /// The width the controls took, gap included, the last time they were on
  /// screen; null before they ever were.
  clusterWidth: number | null
}

/// Whether the controls should be folded after this measurement. Folding
/// happens the moment a name is cut off. Unfolding waits until the names, the
/// controls and the slack all fit in the row the names have while folded, which
/// is the room they would share once the controls are back.
export function nextFolded(folded: boolean, m: FoldMeasure): boolean {
  if (!folded) return m.truncated
  if (m.truncated || m.clusterWidth === null) return true
  return m.namesWidth + m.clusterWidth + FOLD_SLACK_PX > m.rowWidth
}

// Folds a row's controls away whenever the names beside them would be cut off.
// The row's children are the names' boxes, and inside them the elements marked
// `data-fold-name` are the ones whose own text truncates. The row is re-measured when the controls fold or unfold,
// when `watch` changes (pass the names' text), and when the row is resized,
// which covers the window, a dragged divider and a side panel opening or
// closing; nothing is measured on an ordinary re-render.
//
// `folded` is null until the first measurement, while the controls are on
// screen so their width can be read; the measurement runs before the browser
// paints, so the header is never seen in that state. A new `layout` (pass what
// decides which controls exist) goes back to null, because the remembered
// controls width is no longer theirs.
export function useFoldToFit({
  enabled,
  layout,
  watch,
  focusOnFold,
}: {
  /// Off where there is nowhere to fold into; `folded` is then false.
  enabled: boolean
  layout: string
  watch: string
  /// Where focus goes when the controls fold while one of them holds it.
  focusOnFold?: React.RefObject<HTMLElement | null>
}): {
  folded: boolean | null
  rowRef: React.RefObject<HTMLDivElement | null>
  clusterRef: React.RefObject<HTMLDivElement | null>
} {
  const [state, setState] = useState<{ layout: string; folded: boolean | null }>(
    { layout, folded: null },
  )
  if (state.layout !== layout) setState({ layout, folded: null })
  const rowRef = useRef<HTMLDivElement | null>(null)
  const clusterRef = useRef<HTMLDivElement | null>(null)
  const clusterWidth = useRef<number | null>(null)
  const focusInCluster = useRef(false)

  const measure = useCallback(() => {
    const row = rowRef.current
    if (!row || !enabled) return
    const cluster = clusterRef.current
    focusInCluster.current = cluster?.contains(document.activeElement) ?? false
    if (cluster) {
      const gap = parseFloat(getComputedStyle(cluster.parentElement ?? cluster).columnGap)
      clusterWidth.current =
        cluster.getBoundingClientRect().width + (Number.isFinite(gap) ? gap : 0)
    }
    const names = [...row.querySelectorAll<HTMLElement>("[data-fold-name]")]
    const truncated = names.some(isTruncated)
    const boxes = [...row.children].map((el) => el.getBoundingClientRect())
    const namesWidth =
      boxes.length === 0 ? 0 : boxes[boxes.length - 1].right - boxes[0].left
    const m: FoldMeasure = {
      truncated,
      namesWidth,
      rowWidth: row.clientWidth,
      clusterWidth: clusterWidth.current,
    }
    setState((s) => {
      const folded = nextFolded(s.folded ?? false, m)
      return s.folded === folded ? s : { ...s, folded }
    })
  }, [enabled])

  // Ahead of the re-measure below, which forgets where focus was once the
  // controls are gone.
  useLayoutEffect(() => {
    if (state.folded === true && focusInCluster.current) focusOnFold?.current?.focus()
  }, [state.folded, focusOnFold])

  useLayoutEffect(() => measure(), [measure, watch, state.folded, state.layout])

  useEffect(() => {
    const row = rowRef.current
    if (!row || typeof ResizeObserver === "undefined") return
    const observer = new ResizeObserver(measure)
    observer.observe(row)
    return () => observer.disconnect()
  }, [measure])

  return {
    folded: enabled ? state.folded : false,
    rowRef,
    clusterRef,
  }
}
