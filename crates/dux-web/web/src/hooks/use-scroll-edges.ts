import * as React from "react"

/** The three numbers that say where a scroll container is. */
export interface ScrollMetrics {
  scrollTop: number
  scrollHeight: number
  clientHeight: number
}

/** Which edges of a scroll container have content hidden past them, and how
 * wide a scrollbar it shows (0 for none, or one that floats over the content). */
export interface ScrollEdges {
  moreAbove: boolean
  moreBelow: boolean
  scrollbar: number
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

const NO_EDGES: ScrollEdges = { moreAbove: false, moreBelow: false, scrollbar: 0 }

// What the box gives up to a scrollbar is what its border box has beyond its
// padding box and borders; a scrollbar that floats takes nothing.
function readEdges(el: HTMLElement): ScrollEdges {
  return {
    moreAbove: hasMoreAbove(el),
    moreBelow: hasMoreBelow(el),
    scrollbar: Math.max(0, Math.round(el.offsetWidth - el.clientWidth - el.clientLeft * 2)),
  }
}

// Keeps the previous object while nothing changed, so a scroll that leaves both
// answers as they were does not re-render the list.
function keepIfSame(prev: ScrollEdges, next: ScrollEdges): ScrollEdges {
  return prev.moreAbove === next.moreAbove &&
    prev.moreBelow === next.moreBelow &&
    prev.scrollbar === next.scrollbar
    ? prev
    : next
}

/**
 * Tracks whether `node`, a scroll container, has content hidden above and
 * below it, and the width of the scrollbar it shows.
 *
 * The answer is read before the first paint with `node` in place, and
 * re-measured on a scroll, on the container changing size, and on any of its
 * direct children changing size or being added or removed, which is how rows
 * arriving or a section opening grows the content without the container itself
 * resizing. There is no polling. A missing `ResizeObserver` (jsdom) is
 * supported: the hook then answers from scrolls and child-list changes alone.
 */
export function useScrollEdgesOf(node: HTMLElement | null): ScrollEdges {
  const [edges, setEdges] = React.useState<ScrollEdges>(NO_EDGES)

  React.useLayoutEffect(() => {
    if (node === null) return
    const el = node
    const measure = () => setEdges((prev) => keepIfSame(prev, readEdges(el)))
    measure()

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

  return node === null ? NO_EDGES : edges
}
