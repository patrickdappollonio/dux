import * as React from "react"

/** The three numbers that say where a scroll container is. */
export interface ScrollMetrics {
  scrollTop: number
  scrollHeight: number
  clientHeight: number
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

/**
 * Tracks whether a scroll container has content hidden below it.
 *
 * The callback ref goes on the scroll container. The answer is re-measured on
 * a scroll, on the container changing size, and on any of its direct children
 * changing size or being added or removed, which is how rows arriving or a
 * section opening grows the content without the container itself resizing.
 * There is no polling. A missing `ResizeObserver` (jsdom) is supported: the
 * hook then answers from scrolls and child-list changes alone.
 */
export function useMoreBelow<T extends HTMLElement = HTMLElement>(): {
  ref: (node: T | null) => void
  moreBelow: boolean
} {
  const [node, setNode] = React.useState<T | null>(null)
  const [moreBelow, setMoreBelow] = React.useState(false)

  // The first answer is read as the node attaches, when its children are
  // already in place; the effect below only subscribes to changes.
  const ref = React.useCallback((next: T | null) => {
    setNode(next)
    setMoreBelow(next === null ? false : hasMoreBelow(next))
  }, [])

  React.useEffect(() => {
    if (node === null) return
    const el = node
    const measure = () => setMoreBelow(hasMoreBelow(el))

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

  return { ref, moreBelow }
}
