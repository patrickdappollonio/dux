import { useCallback, useEffect, useRef, useState } from "react"

// Is this element's text actually cut off by its own `truncate`? A tooltip repeating text the
// user can already read is noise, so this is measured (`scrollWidth` against `clientWidth`)
// and re-measured on the ref attaching, the watched value changing, and a ResizeObserver
// firing on the element; there is deliberately no polling. A missing `ResizeObserver` (jsdom)
// is a supported state: the hook then reports the mount-time answer.
export function useIsTruncated<T extends HTMLElement = HTMLElement>(
  // Re-measure when this changes. Pass the rendered text: a longer value can
  // overflow a box whose own size never moved, so the observer alone misses it.
  watch?: unknown,
): { ref: (node: T | null) => void; truncated: boolean } {
  const [truncated, setTruncated] = useState(false)
  const nodeRef = useRef<T | null>(null)

  const measure = useCallback(() => {
    const el = nodeRef.current
    // A sub-pixel layout can leave scrollWidth one larger than clientWidth on text that is
    // not actually clipped, so compare with a 1px tolerance rather than strictly.
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
