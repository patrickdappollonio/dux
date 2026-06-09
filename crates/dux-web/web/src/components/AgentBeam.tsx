import { useLayoutEffect, useRef, useState } from "react"

// A faint constant-length light that crawls the LEFT edge of a working agent row
// and races the other three sides, fading in and out each lap. The styling
// (geometry, stroke, dash, keyframes) lives in `.agent-beam` in index.css;
// `pathLength={100}` keeps the comet a fixed fraction of the perimeter so it
// never grows or shrinks as it rounds the corners.
//
// An SVG `<rect>`'s outline is drawn top → right → bottom → LEFT, so the left
// edge is the final slice of the perimeter. Its size as a fraction of the whole
// depends on the row's aspect ratio (a wide row has a tiny left edge), which CSS
// can't know — so we measure the row and hand the slice's start to the keyframes
// via `--beam-left-start`. The travel animation then spends half the lap crossing
// that slice (slow) and half on the rest (fast). Render inside a `relative` row,
// only while the agent is working.
export function AgentBeam() {
  const ref = useRef<SVGSVGElement>(null)
  // Where the left-edge slice begins, as a percent of the perimeter. The default
  // is the square-row value ((2w+h)/2(w+h) at w=h = 75%); the measure below
  // refines it to the real row before the first paint settles.
  const [leftStart, setLeftStart] = useState(75)

  // Layout effect (not useEffect) so the measured value is applied before the
  // first paint — otherwise a wide row's first lap animates with the square-row
  // default (75) and the "slow" zone briefly lands on the wrong edge.
  useLayoutEffect(() => {
    const svg = ref.current
    const row = svg?.parentElement
    if (!row) return
    const measure = () => {
      const { width, height } = row.getBoundingClientRect()
      if (width <= 0 || height <= 0) return
      // Perimeter order is top(w) → right(h) → bottom(w) → left(h); the left
      // slice starts after the first three sides.
      const start = ((2 * width + height) / (2 * (width + height))) * 100
      setLeftStart(start)
    }
    measure()
    const ro = new ResizeObserver(measure)
    ro.observe(row)
    return () => ro.disconnect()
  }, [])

  return (
    <svg
      ref={ref}
      aria-hidden
      className="agent-beam"
      style={{ "--beam-left-start": leftStart } as React.CSSProperties}
    >
      <rect x="0" y="0" width="100%" height="100%" rx="6" pathLength={100} />
    </svg>
  )
}
