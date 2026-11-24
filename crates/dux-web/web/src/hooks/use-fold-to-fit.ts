import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react"

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

/// How a control tells the row that something it opened is open, so the fold
/// stays as it is until that closes.
export type FoldHold = { acquire: () => () => void }

export const FoldHoldContext = createContext<FoldHold | null>(null)

/// Holds the fold of the header this is inside for as long as `open`, and
/// after it until the returned function is called, which the popup calls from
/// its "close finished" callback so the fold waits for the focus hand-back (the
/// hold ends a task after that call).
/// Outside such a header it does nothing. It lets go when its component
/// unmounts, so a popup that is torn down cannot leave the fold held.
export function useFoldHold(open: boolean): () => void {
  const hold = useContext(FoldHoldContext)
  const [lingering, setLingering] = useState(false)
  if (open && !lingering) setLingering(true)
  const held = open || lingering
  useLayoutEffect(() => (hold && held ? hold.acquire() : undefined), [hold, held])
  // A task later, not at once: a popup hands focus back to the control it
  // opened from as it finishes closing, and a control folded before that
  // would take the focus down with it.
  return useCallback(() => {
    setTimeout(() => setLingering(false), 0)
  }, [])
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
// While something a control opened is open (see `FoldHoldContext`) the fold does
// not change: a control is never taken from under its own popover or dialog, nor
// a row from under an open menu. The widths are still read, and the fold settles
// to the layout's one resting state once the last hold lets go.
//
// A folded control's width is remembered from when it was last on screen, and
// the width can change while it is away: a web font arriving, browser zoom. So
// either of those forgets every remembered width and puts every control back for
// one measurement, before paint, which is the one time all of them can be read.
//
// `count` is null until the first measurement, while every control is on
// screen so its width can be read; the measurement runs before the browser
// paints, so the row is never seen in that state. A new set of `items`, or
// `enabled` turning on or off, goes back to null, because the remembered widths
// are no longer theirs; while a hold is up that waits for it to end.
export function useFoldToFit({
  enabled: wantEnabled,
  items: wantItems,
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
  /// Provide this to the header's controls, for `useFoldHold`.
  hold: FoldHold
} {
  const holds = useRef(0)
  // Whether a hold is up, as state so that rendering can ask.
  const [holding, setHolding] = useState(false)
  // The set of controls and the on/off flag the fold works with. They follow
  // what the caller passes, except while a hold is up: then the fold is
  // measured and drawn for the set it had, so a control that joins stands and
  // one that leaves is gone, and the rest keep their place until the last hold
  // lets go and the new set is measured from scratch.
  const [applied, setApplied] = useState({
    items: wantItems,
    enabled: wantEnabled,
  })
  const wantLayout = wantItems.join("\u0000")
  const [state, setState] = useState<{ layout: string; count: number | null }>(
    { layout: wantLayout, count: null },
  )
  if (
    !holding &&
    (applied.items.join("\u0000") !== wantLayout ||
      applied.enabled !== wantEnabled)
  ) {
    setApplied({ items: wantItems, enabled: wantEnabled })
    setState({ layout: wantLayout, count: null })
  }
  const items = applied.items
  const enabled = applied.enabled
  const layout = items.join("\u0000")
  const rowRef = useRef<HTMLDivElement | null>(null)
  const elements = useRef(new Map<string, HTMLElement>())
  const widths = useRef(new Map<string, number>())
  const focused = useRef<string | null>(null)
  const [released, setReleased] = useState(0)
  const stale = useRef(false)
  const hold = useMemo<FoldHold>(
    () => ({
      acquire: () => {
        holds.current++
        setHolding(true)
        let live = true
        return () => {
          if (!live) return
          live = false
          holds.current--
          setHolding(holds.current > 0)
          setReleased((n) => n + 1)
        }
      },
    }),
    [],
  )

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
    // A hold keeps the fold where it is; the widths above are still read.
    if (holds.current > 0) return
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

  // Every control back on screen for one measurement, since a folded control's
  // remembered width may no longer be its width.
  const remeasureAll = useCallback(() => {
    if (holds.current > 0) {
      stale.current = true
      return
    }
    stale.current = false
    widths.current.clear()
    setState((s) => (s.count === null ? s : { ...s, count: null }))
  }, [])

  // A hold ending is a measurement, after any font or zoom change it deferred.
  useLayoutEffect(() => {
    if (released > 0 && holds.current === 0 && stale.current) remeasureAll()
  }, [released, remeasureAll])

  useLayoutEffect(
    () => measure(),
    [measure, watch, state.count, state.layout, released],
  )

  useEffect(() => {
    const row = rowRef.current
    if (!row || typeof ResizeObserver === "undefined") return
    const observer = new ResizeObserver(measure)
    observer.observe(row)
    return () => observer.disconnect()
  }, [measure])

  // Browser zoom changes every width at once, and the row's own resize is
  // already measuring, so it is the pixel ratio that says the widths moved.
  useEffect(() => {
    let ratio = window.devicePixelRatio
    const onResize = () => {
      if (window.devicePixelRatio === ratio) return
      ratio = window.devicePixelRatio
      remeasureAll()
    }
    window.addEventListener("resize", onResize)
    return () => window.removeEventListener("resize", onResize)
  }, [remeasureAll])

  // A web font arriving changes the names' and the controls' widths without
  // resizing the row.
  useEffect(() => {
    const fonts = typeof document === "undefined" ? undefined : document.fonts
    if (!fonts?.addEventListener) return
    fonts.addEventListener("loadingdone", remeasureAll)
    return () => fonts.removeEventListener("loadingdone", remeasureAll)
  }, [remeasureAll])

  return {
    isFolded,
    settled: !enabled || state.count !== null,
    rowRef,
    itemRef,
    hold,
  }
}
