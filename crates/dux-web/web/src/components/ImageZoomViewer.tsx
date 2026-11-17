import { useEffect, useLayoutEffect, useRef, useState } from "react"
import { MoveHorizontal, Shrink, ZoomIn, ZoomOut } from "lucide-react"

import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
import { ButtonGroup, ButtonGroupText } from "@/components/ui/button-group"
import {
  anchoredScroll,
  canPan,
  CANVAS_PAD,
  canvasLayout,
  clampZoom,
  imagePointAt,
  MAX_ZOOM,
  MIN_ZOOM,
  resolveScale,
  stepZoom,
  wheelZoomFactor,
  zoomPercent,
  type CanvasLayout,
  type Point,
  type Size,
  type ZoomMode,
} from "@/lib/imageZoom"
import { cn } from "@/lib/utils"

// Raster pictures switch to nearest-neighbour sampling from this scale up, so
// a zoomed screenshot shows crisp pixels instead of a blur. Below it the
// browser's smoothing reads better (and a downscale needs it).
const PIXELATED_FROM = 2

// Two taps closer than this in time and distance are a double tap.
const DOUBLE_TAP_MS = 300
const DOUBLE_TAP_SLOP = 30
// A touch that travels further than this is a pan, not a tap.
const TAP_MOVE_SLOP = 10

// One height token for the whole cluster. Every control meets the 40px floor
// on a phone or a coarse pointer, on both axes, so no exemption is needed.
const ZOOM_CONTROL =
  "h-7 min-w-7 px-2 text-xs max-md:h-10 max-md:min-w-10 pointer-coarse:h-10 pointer-coarse:min-w-10"

type Gesture =
  | { kind: "pan"; last: Point }
  | { kind: "pinch"; startDist: number; startScale: number; imagePoint: Point }

interface Live {
  natural: Size | null
  viewport: Size
  scale: number
  layout: CanvasLayout | null
  mode: ZoomMode
}

/**
 * The editor's picture viewer, shared by the raster image tab and the SVG
 * preview. It opens in Fit (the whole picture, never enlarged), offers Fit
 * width, 100% and stepped zoom from a corner toolbar, zooms around the pointer
 * on Ctrl/Cmd+wheel and on a pinch (a trackpad pinch arrives as Ctrl+wheel),
 * pans by drag when the picture is bigger than the pane, and toggles Fit and
 * 100% on a double click or double tap. Nothing animates, so reduced motion
 * needs no special case. Zoom is component state only: the caller keys the
 * viewer by path, so every file opens in Fit.
 */
export function ImageZoomViewer({
  src,
  alt,
  raster,
  declaredSize = null,
  onNaturalSize,
  onError,
}: {
  src: string
  alt: string
  /** Raster pictures get crisp pixels when enlarged; an SVG redraws itself. */
  raster: boolean
  /** The size the picture declares for itself, preferred over what the browser
   * reports (an SVG with only a viewBox is reported 150px tall). */
  declaredSize?: Size | null
  onNaturalSize?: (size: Size) => void
  onError?: () => void
}) {
  const scrollRef = useRef<HTMLDivElement>(null)
  const canvasRef = useRef<HTMLDivElement>(null)
  const [viewport, setViewport] = useState<Size>({ width: 0, height: 0 })
  const [natural, setNatural] = useState<Size | null>(null)
  const [mode, setMode] = useState<ZoomMode>("fit")
  const [freeScale, setFreeScale] = useState(1)
  const [panning, setPanning] = useState(false)
  const pendingScroll = useRef<Point | null>(null)

  const scale = natural ? resolveScale(mode, freeScale, natural, viewport) : 1
  const layout = natural ? canvasLayout(natural, scale, viewport) : null
  const live = useRef<Live>({ natural, viewport, scale, layout, mode })
  // Handlers and the native wheel listener read the latest render's values.
  useLayoutEffect(() => {
    live.current = { natural, viewport, scale, layout, mode }
  })

  // Fit and Fit width follow the pane: the viewport size is state, so a resize
  // re-derives their scale on the next render.
  useLayoutEffect(() => {
    const el = scrollRef.current
    if (!el) return
    const measure = () =>
      setViewport((prev) =>
        prev.width === el.clientWidth && prev.height === el.clientHeight
          ? prev
          : { width: el.clientWidth, height: el.clientHeight },
      )
    measure()
    if (typeof ResizeObserver === "undefined") return
    const observer = new ResizeObserver(measure)
    observer.observe(el)
    return () => observer.disconnect()
  }, [])

  // A zoom decides its scroll offset with the new layout; it is applied once
  // that layout is in the DOM, before paint, so the anchor never jumps.
  useLayoutEffect(() => {
    const el = scrollRef.current
    const target = pendingScroll.current
    if (!el || !target) return
    pendingScroll.current = null
    el.scrollLeft = target.x
    el.scrollTop = target.y
  })

  function viewportPointOf(clientX: number, clientY: number): Point {
    const el = scrollRef.current
    const canvas = canvasRef.current
    if (!el || !canvas) return { x: 0, y: 0 }
    // Measured against the canvas, so a reserved scrollbar gutter cancels out.
    const rect = canvas.getBoundingClientRect()
    return {
      x: clientX - rect.left - el.scrollLeft,
      y: clientY - rect.top - el.scrollTop,
    }
  }

  function centre(): Point {
    const v = live.current.viewport
    return { x: v.width / 2, y: v.height / 2 }
  }

  // Zoom to `next`, keeping the picture pixel at `from` (a viewport point)
  // and placing it at `to` (the same point unless a double click recentres).
  function zoomTo(next: number, from: Point = centre(), to: Point = from) {
    const el = scrollRef.current
    const { natural: n, layout: current, viewport: v } = live.current
    if (!el || !n || !current) return
    const imagePoint = imagePointAt(current, {
      x: from.x + el.scrollLeft,
      y: from.y + el.scrollTop,
    })
    const nextScale = clampZoom(next)
    pendingScroll.current = anchoredScroll(
      canvasLayout(n, nextScale, v),
      v,
      imagePoint,
      to,
    )
    setMode("free")
    setFreeScale(nextScale)
  }

  function chooseMode(next: "fit" | "width") {
    // Fit width from Fit starts at the top, where reading a tall picture starts.
    if (next === "fit" || live.current.mode === "fit") {
      pendingScroll.current = { x: 0, y: 0 }
      setMode(next)
      return
    }
    const el = scrollRef.current
    const { natural: n, layout: current, viewport: v } = live.current
    if (el && n && current) {
      const c = centre()
      const imagePoint = imagePointAt(current, {
        x: c.x + el.scrollLeft,
        y: c.y + el.scrollTop,
      })
      pendingScroll.current = anchoredScroll(
        canvasLayout(n, resolveScale(next, 1, n, v), v),
        v,
        imagePoint,
        c,
      )
    }
    setMode(next)
  }

  function toggleFitAt(point: Point) {
    if (live.current.mode === "fit") zoomTo(1, point, centre())
    else chooseMode("fit")
  }

  // Ctrl/Cmd+wheel zooms; the listener is native because React's wheel
  // handler is passive and could not stop the browser zooming the page.
  useEffect(() => {
    const el = scrollRef.current
    if (!el) return
    const onWheel = (e: WheelEvent) => {
      if (!(e.ctrlKey || e.metaKey)) return
      e.preventDefault()
      const point = viewportPointOf(e.clientX, e.clientY)
      zoomTo(live.current.scale * wheelZoomFactor(e.deltaY, e.deltaMode), point)
    }
    el.addEventListener("wheel", onWheel, { passive: false })
    return () => el.removeEventListener("wheel", onWheel)
    // zoomTo and viewportPointOf read only refs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const pointers = useRef(new Map<number, Point>())
  const gesture = useRef<Gesture | null>(null)
  const touchDown = useRef<{ x: number; y: number; time: number; moved: boolean } | null>(null)
  const lastTap = useRef<{ x: number; y: number; time: number } | null>(null)
  // A double tap also produces a dblclick on some browsers; it must not toggle twice.
  const ignoreDblClickUntil = useRef(0)

  function startPinch() {
    const [a, b] = [...pointers.current.values()]
    const { layout: current } = live.current
    const el = scrollRef.current
    if (!a || !b || !current || !el) return
    const mid = viewportPointOf((a.x + b.x) / 2, (a.y + b.y) / 2)
    gesture.current = {
      kind: "pinch",
      startDist: Math.max(1, Math.hypot(a.x - b.x, a.y - b.y)),
      startScale: live.current.scale,
      imagePoint: imagePointAt(current, {
        x: mid.x + el.scrollLeft,
        y: mid.y + el.scrollTop,
      }),
    }
    setPanning(false)
  }

  function startPan(at: Point) {
    const { layout: current, viewport: v } = live.current
    if (!current || !canPan(current, v)) {
      gesture.current = null
      return
    }
    gesture.current = { kind: "pan", last: at }
    setPanning(true)
  }

  function onPointerDown(e: React.PointerEvent<HTMLDivElement>) {
    if (e.pointerType === "mouse" && e.button !== 0) return
    const at = { x: e.clientX, y: e.clientY }
    pointers.current.set(e.pointerId, at)
    try {
      e.currentTarget.setPointerCapture(e.pointerId)
    } catch {
      // Capture is a nicety (a drag that leaves the pane keeps panning).
    }
    if (pointers.current.size === 2) {
      touchDown.current = null
      startPinch()
    } else if (pointers.current.size === 1) {
      if (e.pointerType === "touch") {
        touchDown.current = { ...at, time: e.timeStamp, moved: false }
      }
      startPan(at)
    }
  }

  function onPointerMove(e: React.PointerEvent<HTMLDivElement>) {
    if (!pointers.current.has(e.pointerId)) return
    const at = { x: e.clientX, y: e.clientY }
    pointers.current.set(e.pointerId, at)
    const down = touchDown.current
    if (down && Math.hypot(at.x - down.x, at.y - down.y) > TAP_MOVE_SLOP) {
      down.moved = true
    }
    const g = gesture.current
    const el = scrollRef.current
    if (!g || !el) return
    if (g.kind === "pan") {
      el.scrollLeft -= at.x - g.last.x
      el.scrollTop -= at.y - g.last.y
      g.last = at
      return
    }
    const [a, b] = [...pointers.current.values()]
    const { natural: n, viewport: v } = live.current
    if (!a || !b || !n) return
    const dist = Math.hypot(a.x - b.x, a.y - b.y)
    const nextScale = clampZoom((g.startScale * dist) / g.startDist)
    const mid = viewportPointOf((a.x + b.x) / 2, (a.y + b.y) / 2)
    pendingScroll.current = anchoredScroll(
      canvasLayout(n, nextScale, v),
      v,
      g.imagePoint,
      mid,
    )
    setMode("free")
    setFreeScale(nextScale)
  }

  function onPointerEnd(e: React.PointerEvent<HTMLDivElement>) {
    if (!pointers.current.delete(e.pointerId)) return
    const wasPinch = gesture.current?.kind === "pinch"
    const remaining = [...pointers.current.values()]
    if (remaining.length === 1) {
      // Lifting one finger of a pinch carries on as a pan with the other.
      startPan(remaining[0])
      return
    }
    if (remaining.length === 0) {
      gesture.current = null
      setPanning(false)
    }
    const down = touchDown.current
    touchDown.current = null
    if (
      e.type !== "pointerup" ||
      e.pointerType !== "touch" ||
      wasPinch ||
      !down ||
      down.moved
    ) {
      return
    }
    const prev = lastTap.current
    if (
      prev &&
      e.timeStamp - prev.time < DOUBLE_TAP_MS &&
      Math.hypot(e.clientX - prev.x, e.clientY - prev.y) < DOUBLE_TAP_SLOP
    ) {
      lastTap.current = null
      ignoreDblClickUntil.current = e.timeStamp + 600
      toggleFitAt(viewportPointOf(e.clientX, e.clientY))
    } else {
      lastTap.current = { x: e.clientX, y: e.clientY, time: e.timeStamp }
    }
  }

  function onDoubleClick(e: React.MouseEvent<HTMLDivElement>) {
    if (e.timeStamp < ignoreDblClickUntil.current) return
    toggleFitAt(viewportPointOf(e.clientX, e.clientY))
  }

  const pannable = layout !== null && canPan(layout, viewport)
  const actualSize = mode === "free" && Math.abs(scale - 1) < 1e-6

  return (
    <div className="relative h-full min-h-0 bg-muted/30">
      <div
        ref={scrollRef}
        data-testid="image-zoom-viewport"
        className={cn(
          "absolute inset-0 touch-none overflow-auto select-none [scrollbar-gutter:stable_both-edges]",
          pannable && (panning ? "cursor-grabbing" : "cursor-grab"),
        )}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerEnd}
        onPointerCancel={onPointerEnd}
        onDoubleClick={onDoubleClick}
      >
        <div
          ref={canvasRef}
          className="relative"
          style={
            layout
              ? { width: layout.canvas.width, height: layout.canvas.height }
              : { width: "100%", height: "100%" }
          }
        >
          {src !== "" && (
            <img
              src={src}
              alt={alt}
              draggable={false}
              className="absolute max-w-none"
              style={
                layout
                  ? {
                      left: layout.left,
                      top: layout.top,
                      width: layout.width,
                      height: layout.height,
                      imageRendering:
                        raster && scale >= PIXELATED_FROM ? "pixelated" : undefined,
                    }
                  : // Unsized until it loads; a picture that never decodes still shows
                    // the browser's broken-image mark here.
                    { left: CANVAS_PAD, top: CANVAS_PAD }
              }
              onLoad={(e) => {
                const img = e.currentTarget
                const size =
                  declaredSize ??
                  (img.naturalWidth > 0 && img.naturalHeight > 0
                    ? { width: img.naturalWidth, height: img.naturalHeight }
                    : { width: 300, height: 150 })
                setNatural((prev) =>
                  prev && prev.width === size.width && prev.height === size.height
                    ? prev
                    : size,
                )
                onNaturalSize?.(size)
              }}
              onError={onError}
            />
          )}
        </div>
      </div>
      <ButtonGroup
        aria-label="Zoom"
        className="absolute right-5 bottom-5 z-10 rounded-lg bg-background shadow-md"
      >
        <SimpleTooltip content="Fit the whole picture">
          <Button
            variant="outline"
            className={ZOOM_CONTROL}
            toggle
            aria-pressed={mode === "fit"}
            onClick={() => chooseMode("fit")}
          >
            <Shrink />
            <span className="max-md:sr-only">Fit</span>
          </Button>
        </SimpleTooltip>
        <SimpleTooltip content="Fit the width and scroll down">
          <Button
            variant="outline"
            className={ZOOM_CONTROL}
            toggle
            aria-pressed={mode === "width"}
            onClick={() => chooseMode("width")}
          >
            <MoveHorizontal />
            <span className="max-md:sr-only">Fit width</span>
          </Button>
        </SimpleTooltip>
        <SimpleTooltip content="Actual pixels">
          <Button
            variant="outline"
            className={ZOOM_CONTROL}
            toggle
            aria-pressed={actualSize}
            aria-label="Actual size, 100%"
            onClick={() => zoomTo(1)}
          >
            100%
          </Button>
        </SimpleTooltip>
        <SimpleTooltip content="Zoom out">
          <Button
            variant="outline"
            className={ZOOM_CONTROL}
            aria-label="Zoom out"
            disabled={scale <= MIN_ZOOM + 1e-6}
            onClick={() => zoomTo(stepZoom(scale, -1))}
          >
            <ZoomOut />
          </Button>
        </SimpleTooltip>
        <ButtonGroupText
          data-testid="image-zoom-level"
          className="h-7 w-14 justify-center bg-background px-0 text-xs tabular-nums shadow-none max-md:h-10 pointer-coarse:h-10 dark:bg-input/30"
        >
          {zoomPercent(scale)}
        </ButtonGroupText>
        <SimpleTooltip content="Zoom in">
          <Button
            variant="outline"
            className={ZOOM_CONTROL}
            aria-label="Zoom in"
            disabled={scale >= MAX_ZOOM - 1e-6}
            onClick={() => zoomTo(stepZoom(scale, 1))}
          >
            <ZoomIn />
          </Button>
        </SimpleTooltip>
      </ButtonGroup>
    </div>
  )
}
