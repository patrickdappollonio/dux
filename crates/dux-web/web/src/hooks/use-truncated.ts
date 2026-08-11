import { useCallback, useEffect, useRef, useState } from "react"

// Is this element's text actually cut off by its own `truncate`?
//
// The header's chips reveal their full value on hover, but ONLY when there is
// something to reveal: a tooltip that repeats text the user can already read is
// noise. "Cut off" is not a guess, it is a measurement (`scrollWidth` against
// `clientWidth`), and it has to be re-taken whenever the element's box changes,
// which on this surface happens constantly: the terminal/Changes split is
// draggable, the window resizes, and a chip's siblings grow and shrink around
// it.
//
// So the measurement is re-taken on three events: the ref attaching, the watched
// value changing, and a ResizeObserver firing on the element itself. There is
// deliberately no polling. `ResizeObserver` is absent under jsdom, so its
// absence is a supported state rather than a crash: the hook simply reports the
// mount-time answer, which in a layout-free test environment is "not truncated".
export function useIsTruncated<T extends HTMLElement = HTMLElement>(
  // Re-measure when this changes. Pass the rendered text: a longer value can
  // overflow a box whose own size never moved, so the observer alone misses it.
  watch?: unknown,
): { ref: (node: T | null) => void; truncated: boolean } {
  const [truncated, setTruncated] = useState(false)
  const nodeRef = useRef<T | null>(null)

  const measure = useCallback(() => {
    const el = nodeRef.current
    // A sub-pixel layout can leave scrollWidth one larger than clientWidth on
    // text that is not actually clipped, so compare with a 1px tolerance rather
    // than strictly: a tooltip that fires on a glyph's worth of rounding is the
    // same noise this hook exists to avoid.
    setTruncated(el ? el.scrollWidth - el.clientWidth > 1 : false)
  }, [])

  const ref = useCallback(
    (node: T | null) => {
      nodeRef.current = node
      measure()
    },
    [measure],
  )

  useEffect(() => {
    measure()
    const el = nodeRef.current
    if (!el || typeof ResizeObserver === "undefined") return
    const observer = new ResizeObserver(measure)
    observer.observe(el)
    return () => observer.disconnect()
  }, [measure, watch])

  return { ref, truncated }
}
