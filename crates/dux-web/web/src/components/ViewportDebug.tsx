import { useCallback, useEffect, useRef, useState, type CSSProperties } from "react"

import {
  findCut,
  pushSizeEvent,
  type ClipCandidate,
  type SizeEvent,
  type SizeEventType,
} from "@/lib/viewportDebug"

// The phone shell's viewport readout behind `?debug=viewport`. Its colours are
// fixed rather than themed so a screenshot reads the same under any theme.

export interface ViewportDebugProps {
  /** What `useVisualViewportHeight()` returned on the shell's last render. */
  hookHeight: number | null
  /** The shell is pinned to the visual viewport (the terminal screen). */
  constrained: boolean
  /** The shell judged the keyboard open and dropped its bottom inset. */
  keyboardOpen: boolean
}

// Top to bottom in the order the shell nests them.
const BOXES = [
  "shell",
  "shell-inner",
  "pane-screen",
  "stack",
  "stack-terminal-area",
  "xterm-host",
  "key-rows",
  "compose-row",
  "compose-textarea",
  "send-button",
] as const

const BANDS = [
  "red",
  "orange",
  "yellow",
  "lime",
  "cyan",
  "blue",
  "violet",
  "white",
  "grey",
  "hotpink",
] as const

const BAND_PX = 4
const POLL_MS = 400
// The panel keeps clear of the strip the readout is there to explain.
const BOTTOM_KEEPOUT_PX = 80
const Z = 2147483000

type Position = "top" | "mid"

const r1 = (n: number) => String(Math.round(n * 10) / 10)

function labelOf(el: Element): string {
  const named =
    el.getAttribute("data-debug-box") ??
    el.getAttribute("data-slot") ??
    el.getAttribute("data-testid")
  if (named) return named
  const cls = typeof el.className === "string" ? el.className.split(" ")[0] : ""
  return el.tagName.toLowerCase() + (cls ? `.${cls}` : "")
}

function clips(style: CSSStyleDeclaration): boolean {
  return (
    style.overflowX !== "visible" ||
    style.overflowY !== "visible" ||
    (style.clipPath !== "" && style.clipPath !== "none")
  )
}

function ancestorsOf(el: Element): ClipCandidate[] {
  const out: ClipCandidate[] = []
  for (let a = el.parentElement; a; a = a.parentElement) {
    out.push({
      label: labelOf(a),
      rect: a.getBoundingClientRect(),
      clips: clips(getComputedStyle(a)),
    })
  }
  return out
}

function isTransparent(color: string): boolean {
  return color === "" || color === "transparent" || /rgba\(.*,\s*0\)$/.test(color)
}

// The element the browser hit-tests at a point, and the first painted
// background at or above it, which is what shows there.
function describePoint(x: number, y: number): string {
  if (typeof document.elementFromPoint !== "function") return "unsupported"
  const hit = document.elementFromPoint(x, y)
  if (!hit) return "nothing"
  const named = hit.closest("[data-debug-box],[data-slot]")
  let bg = "none"
  for (let a: Element | null = hit; a; a = a.parentElement) {
    const color = getComputedStyle(a).backgroundColor
    if (!isTransparent(color)) {
      bg = `${color} @${labelOf(a)}`
      break
    }
  }
  return `${hit.tagName.toLowerCase()} [${named ? labelOf(named) : "-"}] bg ${bg}`
}

function rectLine(name: string, el: Element | null): string {
  if (!el) return `${name} absent`
  const r = el.getBoundingClientRect()
  return `${name} t${r1(r.top)} b${r1(r.bottom)} h${r1(r.height)}`
}

const pad = (s: string, n: number) => (s.length >= n ? s : s + " ".repeat(n - s.length))

interface Probes {
  dvh: HTMLDivElement | null
  svh: HTMLDivElement | null
  lvh: HTMLDivElement | null
  safe: HTMLDivElement | null
}

function measure(
  props: ViewportDebugProps,
  probes: Probes,
  log: readonly SizeEvent[],
): { text: string; vvTop: number; vvBottom: number } {
  const vv = window.visualViewport
  const vvTop = vv?.offsetTop ?? 0
  const vvHeight = vv?.height ?? window.innerHeight
  const vvBottom = vvTop + vvHeight
  const live = vv ? Math.round(vv.height) : null
  const root = document.documentElement
  const safe = probes.safe ? getComputedStyle(probes.safe) : null
  const h = (el: HTMLDivElement | null) => (el ? r1(el.getBoundingClientRect().height) : "?")
  const shellEl = document.querySelector("[data-debug-box='shell']")
  const vars = getComputedStyle(shellEl ?? root)
  const lines = [
    `vv ${vv ? `${r1(vv.width)}x${r1(vv.height)} oT${r1(vv.offsetTop)} pT${r1(vv.pageTop)} s${r1(vv.scale)}` : "absent"}`,
    `inner ${window.innerWidth}x${window.innerHeight} client ${root.clientHeight}  vvBottom ${r1(vvBottom)}`,
    `dvh ${h(probes.dvh)} svh ${h(probes.svh)} lvh ${h(probes.lvh)}`,
    `safe t${safe?.paddingTop ?? "?"} r${safe?.paddingRight ?? "?"} b${safe?.paddingBottom ?? "?"} l${safe?.paddingLeft ?? "?"}`,
    `screen ${screen.width}x${screen.height} dpr ${Math.round(window.devicePixelRatio * 10000) / 10000}`,
    `scrollY ${r1(window.scrollY)} scrollingEl.top ${r1(document.scrollingElement?.scrollTop ?? 0)}`,
    rectLine("html", root),
    rectLine("body", document.body),
    `hook ${props.hookHeight ?? "null"} live ${live ?? "null"} diff ${props.hookHeight !== null && live !== null ? live - props.hookHeight : "-"}`,
    `kbdOpen ${props.keyboardOpen ? "yes" : "no"} constrained ${props.constrained ? "yes" : "no"}`,
    `--dux-app-top "${vars.getPropertyValue("--dux-app-top").trim()}" --dux-app-safe-top "${vars.getPropertyValue("--dux-app-safe-top").trim()}"`,
    "box                  top    bot      h  pad-b  css-h   ovf  gap  cut",
  ]
  for (const name of BOXES) {
    const els = Array.from(document.querySelectorAll(`[data-debug-box='${name}']`)).filter(
      (el) => getComputedStyle(el).visibility !== "hidden",
    )
    if (els.length === 0) {
      lines.push(`${pad(name, 19)} absent`)
      continue
    }
    els.forEach((el, i) => {
      const r = el.getBoundingClientRect()
      const s = getComputedStyle(el)
      const cut = findCut(r, ancestorsOf(el))
      const below = r.bottom - vvBottom > 0.5 ? ` BELOW-VV ${r1(r.bottom - vvBottom)}` : ""
      lines.push(
        [
          pad(els.length > 1 ? `${name}#${i}` : name, 19),
          pad(r1(r.top), 6),
          pad(r1(r.bottom), 6),
          pad(r1(r.height), 6),
          pad(s.paddingBottom, 6),
          pad(s.height, 7),
          pad(s.overflowY, 6),
          pad(r1(vvBottom - r.bottom), 5),
          cut ? `CUT ${cut.px} by ${cut.by}` : "-",
        ].join(" ") + below,
      )
    })
  }
  const x = (vv?.offsetLeft ?? 0) + (vv?.width ?? window.innerWidth) / 2
  for (const up of [1, 8, 20]) {
    lines.push(`pt -${up}: ${describePoint(x, vvBottom - up)}`)
  }
  lines.push("events (newest first, ms since mount):")
  for (let i = 0; i < log.length; i += 2) {
    lines.push(
      log
        .slice(i, i + 2)
        .map((e) => `${pad(`+${Math.round(e.t)}`, 7)} ${pad(e.type, 10)} ${r1(e.height)}`)
        .join(" | "),
    )
  }
  return { text: lines.join("\n"), vvTop, vvBottom }
}

export function ViewportDebug(props: ViewportDebugProps) {
  const probes = useRef<Probes>({ dvh: null, svh: null, lvh: null, safe: null })
  const preRef = useRef<HTMLPreElement | null>(null)
  const propsRef = useRef(props)
  const logRef = useRef<SizeEvent[]>([])
  const [snapshot, setSnapshot] = useState({ text: "", vvTop: 0, vvBottom: 0 })
  const [position, setPosition] = useState<Position>("top")
  const [copied, setCopied] = useState("")

  const remeasure = useCallback(() => {
    let next: { text: string; vvTop: number; vvBottom: number }
    // A readout that throws must not take the app under it down with it.
    try {
      next = measure(propsRef.current, probes.current, logRef.current)
    } catch (error) {
      next = { text: `measure failed: ${String(error)}`, vvTop: 0, vvBottom: window.innerHeight }
    }
    setSnapshot((prev) =>
      prev.text === next.text && prev.vvTop === next.vvTop && prev.vvBottom === next.vvBottom
        ? prev
        : next,
    )
  }, [])

  useEffect(() => {
    propsRef.current = props
    remeasure()
  })

  useEffect(() => {
    const mounted = performance.now()
    const vv = window.visualViewport
    let frame = 0
    const on = (type: SizeEventType) => () => {
      const height = type === "win-resize" ? window.innerHeight : (vv?.height ?? 0)
      logRef.current = pushSizeEvent(logRef.current, {
        t: performance.now() - mounted,
        type,
        height,
      })
      remeasure()
      // Once more after the shell has rendered the height this event carried.
      cancelAnimationFrame(frame)
      frame = requestAnimationFrame(remeasure)
    }
    const vvResize = on("vv-resize")
    const vvScroll = on("vv-scroll")
    const winResize = on("win-resize")
    vv?.addEventListener("resize", vvResize)
    vv?.addEventListener("scroll", vvScroll)
    window.addEventListener("resize", winResize)
    const poll = setInterval(remeasure, POLL_MS)
    return () => {
      vv?.removeEventListener("resize", vvResize)
      vv?.removeEventListener("scroll", vvScroll)
      window.removeEventListener("resize", winResize)
      clearInterval(poll)
      cancelAnimationFrame(frame)
    }
  }, [remeasure])

  const copy = () => {
    const text = snapshot.text
    const selectAll = () => {
      if (preRef.current) window.getSelection()?.selectAllChildren(preRef.current)
      setCopied("selected")
    }
    if (!navigator.clipboard) return selectAll()
    navigator.clipboard.writeText(text).then(() => setCopied("copied"), selectAll)
  }

  const vvHeight = snapshot.vvBottom - snapshot.vvTop
  const top = snapshot.vvTop + (position === "top" ? 4 : Math.round(vvHeight * 0.3))
  const maxHeight = Math.max(60, snapshot.vvBottom - BOTTOM_KEEPOUT_PX - top)
  // Keeps the soft keyboard up: a press on the panel must not move focus.
  const keepFocus = (e: React.PointerEvent) => e.preventDefault()
  const button: CSSProperties = {
    font: "inherit",
    color: "#fff",
    background: "#333",
    border: "1px solid #888",
    padding: "4px 10px",
    minHeight: 28,
  }

  return (
    <>
      <div
        aria-hidden
        style={{ position: "fixed", top: 0, left: 0, width: 0, height: 0, visibility: "hidden", pointerEvents: "none" }}
      >
        <div ref={(el) => void (probes.current.dvh = el)} style={{ position: "absolute", width: 0, height: "100dvh" }} />
        <div ref={(el) => void (probes.current.svh = el)} style={{ position: "absolute", width: 0, height: "100svh" }} />
        <div ref={(el) => void (probes.current.lvh = el)} style={{ position: "absolute", width: 0, height: "100lvh" }} />
        <div
          ref={(el) => void (probes.current.safe = el)}
          style={{
            position: "absolute",
            paddingTop: "env(safe-area-inset-top)",
            paddingRight: "env(safe-area-inset-right)",
            paddingBottom: "env(safe-area-inset-bottom)",
            paddingLeft: "env(safe-area-inset-left)",
          }}
        />
      </div>
      <div
        aria-hidden
        data-viewport-debug="bands"
        style={{
          position: "fixed",
          left: 0,
          bottom: 0,
          width: 24,
          display: "flex",
          flexDirection: "column-reverse",
          pointerEvents: "none",
          zIndex: Z,
        }}
      >
        {BANDS.map((color) => (
          <div key={color} style={{ height: BAND_PX, background: color }} />
        ))}
      </div>
      <div
        aria-hidden
        data-viewport-debug="vv-bottom"
        style={{
          position: "fixed",
          left: 0,
          right: 0,
          top: snapshot.vvBottom - 1,
          height: 1,
          background: "magenta",
          pointerEvents: "none",
          zIndex: Z,
        }}
      />
      <div
        data-viewport-debug="panel"
        style={{
          position: "fixed",
          left: 28,
          right: 4,
          top,
          maxHeight,
          display: "flex",
          flexDirection: "column",
          background: "rgba(0,0,0,0.88)",
          color: "#fff",
          border: "1px solid #ff0",
          font: "9px/11px ui-monospace, Menlo, monospace",
          zIndex: Z,
        }}
      >
        <div style={{ display: "flex", gap: 6, padding: 2 }}>
          <button
            type="button"
            style={button}
            onPointerDown={keepFocus}
            onClick={() => setPosition((p) => (p === "top" ? "mid" : "top"))}
          >
            {position === "top" ? "to middle" : "to top"}
          </button>
          <button type="button" style={button} onPointerDown={keepFocus} onClick={copy}>
            copy{copied ? ` (${copied})` : ""}
          </button>
        </div>
        <pre
          ref={preRef}
          style={{
            margin: 0,
            padding: 2,
            overflow: "auto",
            whiteSpace: "pre",
            userSelect: "text",
            WebkitUserSelect: "text",
            touchAction: "pan-x pan-y",
          }}
        >
          {snapshot.text}
        </pre>
      </div>
    </>
  )
}
