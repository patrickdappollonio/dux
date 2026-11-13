import * as React from "react"

/** The three numbers that say where a scroll container is. */
export interface ScrollMetrics {
  scrollTop: number
  scrollHeight: number
  clientHeight: number
}

/** Which edges of a scroll container have content hidden past them. */
export interface ScrollEdges {
  moreAbove: boolean
  moreBelow: boolean
}

/**
 * Is part of the content hidden above the container's top edge? The 1px
 * tolerance matches `hasMoreBelow`, so a sub-pixel scroll offset left behind by
 * a fractional device pixel ratio does not read as scrolled.
 */
export function hasMoreAbove(m: ScrollMetrics): boolean {
  return m.scrollTop > 1
}

/**
 * Is part of the content hidden below the container's bottom edge? A
 * fractional device pixel ratio can leave the fully scrolled position a pixel
 * short of `scrollHeight - clientHeight`, so the comparison has a 1px
 * tolerance, which also keeps a list that fits from reading as scrollable.
 */
export function hasMoreBelow(m: ScrollMetrics): boolean {
  return m.scrollHeight - m.clientHeight - m.scrollTop > 1
}

const NO_EDGES: ScrollEdges = { moreAbove: false, moreBelow: false }

function readEdges(m: ScrollMetrics): ScrollEdges {
  return { moreAbove: hasMoreAbove(m), moreBelow: hasMoreBelow(m) }
}

// Keeps the previous object while nothing changed, so a scroll that leaves both
// answers as they were does not re-render the list.
function keepIfSame(prev: ScrollEdges, next: ScrollEdges): ScrollEdges {
  return prev.moreAbove === next.moreAbove && prev.moreBelow === next.moreBelow
    ? prev
    : next
}

/**
 * Tracks whether a scroll container has content hidden above and below it.
 *
 * The callback ref goes on the scroll container. The answer is re-measured on
 * a scroll, on the container changing size, and on any of its direct children
 * changing size or being added or removed, which is how rows arriving or a
 * section opening grows the content without the container itself resizing.
 * There is no polling. A missing `ResizeObserver` (jsdom) is supported: the
 * hook then answers from scrolls and child-list changes alone.
 */
export function useScrollEdges<T extends HTMLElement = HTMLElement>(): ScrollEdges & {
  ref: (node: T | null) => void
} {
  const [node, setNode] = React.useState<T | null>(null)
  const [edges, setEdges] = React.useState<ScrollEdges>(NO_EDGES)

  // The first answer is read as the node attaches, when its children are
  // already in place; the effect below only subscribes to changes.
  const ref = React.useCallback((next: T | null) => {
    setNode(next)
    setEdges((prev) => keepIfSame(prev, next === null ? NO_EDGES : readEdges(next)))
  }, [])

  React.useEffect(() => {
    if (node === null) return
    const el = node
    const measure = () => setEdges((prev) => keepIfSame(prev, readEdges(el)))

    const resize =
      typeof ResizeObserver === "function" ? new ResizeObserver(measure) : null
    const observeChildren = () => {
      if (resize === null) return
      resize.disconnect()
      resize.observe(el)
      for (const child of el.children) resize.observe(child)
    }
    observeChildren()
    const mutations = new MutationObserver(() => {
      observeChildren()
      measure()
    })
    mutations.observe(el, { childList: true })
    el.addEventListener("scroll", measure, { passive: true })
    return () => {
      el.removeEventListener("scroll", measure)
      mutations.disconnect()
      resize?.disconnect()
    }
  }, [node])

  return { ref, ...edges }
}
