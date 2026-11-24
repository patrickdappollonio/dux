import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react"

import { isTruncated } from "@/hooks/use-truncated"

/// How much room beyond an exact fit the names need beside a standing control.
/// It absorbs the sub-pixel gap between a box's width and the whole-pixel
/// truncation test, which could otherwise flip a control at the boundary width.
export const FOLD_SLACK_PX = 8

export type FoldMeasure = {
  /// Some name's own text is cut off right now.
  truncated: boolean
  /// The names' natural width; meaningful only while none of them is cut off.
  namesWidth: number
  /// The width the names' row has now.
  rowWidth: number
  /// Each control's width with the gap before it, in fold order, as last seen
  /// on screen; null for one never seen.
  widths: (number | null)[]
}

/// How many controls, counted in fold order, are folded after this measurement.
/// A cut-off name folds one more. Otherwise the answer is the fewest folded
/// that leave the names, the standing controls and the slack inside the row the
/// names would have with every control folded, which is the same whatever is
/// folded now, so each width has one resting state.
export function nextFoldCount(count: number, m: FoldMeasure): number {
  const n = m.widths.length
  if (m.truncated) return Math.min(count + 1, n)
  // A row with no width is not laid out (hidden, or a page with no layout).
  if (m.rowWidth <= 0) return count
  const width = (i: number) => m.widths[i] ?? Infinity
  let room = m.rowWidth
  for (let i = count; i < n; i++) room += width(i)
  for (let c = 0; c < n; c++) {
    let need = m.namesWidth + FOLD_SLACK_PX
    for (let i = c; i < n; i++) need += width(i)
    if (need <= room) return c
  }
  return n
}

// Folds a row's controls away one at a time, in the order of `items`, whenever
// the names beside them would be cut off. The row's children are the names'
// boxes, and inside them the elements marked `data-fold-name` are the ones whose
// own text truncates. Each control is measured through the element `itemRef`
// hands it. The row is re-measured when a control folds or unfolds, when
// `watch` changes (pass the names' text), when the row is resized, which covers
// the window, a dragged divider and a side panel opening or closing, and when
// web fonts finish loading; nothing is measured on an ordinary re-render.
//
// `count` is null until the first measurement, while every control is on
// screen so its width can be read; the measurement runs before the browser
// paints, so the row is never seen in that state. A new set of `items` goes
// back to null, because the remembered widths are no longer theirs.
export function useFoldToFit({
  enabled,
  items,
  watch,
  focusOnFold,
}: {
  /// Off where there is nowhere to fold into; nothing is then folded.
  enabled: boolean
  /// The controls that can fold, the first to fold first.
  items: readonly string[]
  watch: string
  /// Where focus goes when a control folds while it holds it.
  focusOnFold?: React.RefObject<HTMLElement | null>
}): {
  /// Whether `item` is folded now; false for every item until it has been
  /// measured, so a control is on screen for its first measurement.
  isFolded: (item: string) => boolean
  /// Measured at least once, so what stands will stay standing at this width.
  settled: boolean
  rowRef: React.RefObject<HTMLDivElement | null>
  itemRef: (item: string) => (el: HTMLElement | null) => void
} {
  const layout = items.join("\u0000")
  const [state, setState] = useState<{ layout: string; count: number | null }>(
    { layout, count: null },
  )
  if (state.layout !== layout) setState({ layout, count: null })
  const rowRef = useRef<HTMLDivElement | null>(null)
  const elements = useRef(new Map<string, HTMLElement>())
  const widths = useRef(new Map<string, number>())
  const focused = useRef<string | null>(null)

  const refs = useRef(new Map<string, (el: HTMLElement | null) => void>())
  const itemRef = useCallback((item: string) => {
    let ref = refs.current.get(item)
    if (!ref) {
      ref = (el) => {
        if (el) elements.current.set(item, el)
        else elements.current.delete(item)
      }
      refs.current.set(item, ref)
    }
    return ref
  }, [])

  // Forget remembered widths when the set of controls changes.
  useLayoutEffect(() => {
    widths.current.clear()
  }, [layout])

  const measure = useCallback(() => {
    const row = rowRef.current
    if (!row || !enabled) return
    const gap = parseFloat(getComputedStyle(row.parentElement ?? row).columnGap)
    focused.current = null
    for (const [item, el] of elements.current) {
      widths.current.set(
        item,
        el.getBoundingClientRect().width + (Number.isFinite(gap) ? gap : 0),
      )
      if (el.contains(document.activeElement)) focused.current = item
    }
    const names = [...row.querySelectorAll<HTMLElement>("[data-fold-name]")]
    const boxes = [...row.children].map((el) => el.getBoundingClientRect())
    const m: FoldMeasure = {
      truncated: names.some(isTruncated),
      namesWidth:
        boxes.length === 0 ? 0 : boxes[boxes.length - 1].right - boxes[0].left,
      rowWidth: row.clientWidth,
      widths: (layout === "" ? [] : layout.split("\u0000")).map(
        (item) => widths.current.get(item) ?? null,
      ),
    }
    setState((s) => {
      const count = nextFoldCount(s.count ?? 0, m)
      return s.count === count ? s : { ...s, count }
    })
  }, [enabled, layout])

  const count = enabled ? (state.count ?? 0) : 0
  const isFolded = useCallback(
    (item: string) => {
      const index = items.indexOf(item)
      return index !== -1 && index < count
    },
    [items, count],
  )

  // Ahead of the re-measure below, which forgets where focus was once the
  // control holding it is gone.
  useLayoutEffect(() => {
    if (focused.current !== null && isFolded(focused.current)) {
      focused.current = null
      focusOnFold?.current?.focus()
    }
  }, [isFolded, focusOnFold])

  useLayoutEffect(() => measure(), [measure, watch, state.count, state.layout])

  useEffect(() => {
    const row = rowRef.current
    if (!row || typeof ResizeObserver === "undefined") return
    const observer = new ResizeObserver(measure)
    observer.observe(row)
    return () => observer.disconnect()
  }, [measure])

  // A web font arriving changes the names' and the controls' widths without
  // resizing the row.
  useEffect(() => {
    const fonts = typeof document === "undefined" ? undefined : document.fonts
    if (!fonts?.addEventListener) return
    fonts.addEventListener("loadingdone", measure)
    return () => fonts.removeEventListener("loadingdone", measure)
  }, [measure])

  return {
    isFolded,
    settled: !enabled || state.count !== null,
    rowRef,
    itemRef,
  }
}
