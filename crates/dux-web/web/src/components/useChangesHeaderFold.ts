import { useCallback, useLayoutEffect, useRef, useState } from "react"

type Part = "header" | "title" | "recap" | "trigger"

// How many icon squares the header's action cell holds when nothing is folded:
// the editor button, the New terminal button and the `⋯`.
const UNFOLDED_BUTTONS = 3

function px(value: string): number {
  const parsed = Number.parseFloat(value)
  return Number.isFinite(parsed) ? parsed : 0
}

// Whether the title, the recap and all three buttons fail to fit side by side.
// Every width read here is one folding cannot move: the title's and the
// recap's full text (`scrollWidth`, which ignores their own clipping), and the
// three buttons counted from the `⋯`, which never leaves. So the answer is the
// same before and after the fold it causes, and a fold cannot undo itself.
// `null` when the header has no width to judge by (not laid out, or hidden).
function tooNarrow(nodes: Record<Part, HTMLElement | null>): boolean | null {
  const { header, title, recap, trigger } = nodes
  if (!header || !title || !trigger) return null
  const style = getComputedStyle(header)
  const room = header.clientWidth - px(style.paddingLeft) - px(style.paddingRight)
  if (room <= 0) return null
  const recapWidth = recap?.scrollWidth ?? 0
  const titleGap = recapWidth > 0 && title.parentElement
    ? px(getComputedStyle(title.parentElement).columnGap)
    : 0
  const buttonGap = trigger.parentElement
    ? px(getComputedStyle(trigger.parentElement).columnGap)
    : 0
  const needed =
    title.scrollWidth +
    titleGap +
    recapWidth +
    px(style.columnGap) +
    UNFOLDED_BUTTONS * trigger.offsetWidth +
    (UNFOLDED_BUTTONS - 1) * buttonGap
  // `scrollWidth` rounds to whole pixels, so one pixel of slack keeps a
  // rounded-down title from touching the first button.
  return needed + 1 > room
}

/** Whether the Changes pane header folds its editor and New terminal buttons
 * into its `⋯`, with the refs the header hangs on the parts it measures. It is
 * measured when a part mounts and whenever one of them changes size (a divider
 * drag, the recap's figures changing, a pointer change resizing the buttons),
 * and at no other time, so scrolling and typing elsewhere cost nothing. A
 * missing `ResizeObserver` (jsdom) leaves the mount-time answer standing. */
export function useChangesHeaderFold() {
  const [folded, setFolded] = useState(false)
  const nodes = useRef<Record<Part, HTMLElement | null>>({
    header: null,
    title: null,
    recap: null,
    trigger: null,
  })
  const observer = useRef<ResizeObserver | null>(null)

  const measure = useCallback(() => {
    const next = tooNarrow(nodes.current)
    if (next !== null) setFolded(next)
  }, [])

  const track = useCallback(
    (part: Part, node: HTMLElement | null) => {
      const previous = nodes.current[part]
      if (previous === node) return
      if (previous) observer.current?.unobserve(previous)
      nodes.current[part] = node
      if (node) observer.current?.observe(node)
      measure()
    },
    [measure],
  )
  const headerRef = useCallback((node: HTMLElement | null) => track("header", node), [track])
  const titleRef = useCallback((node: HTMLElement | null) => track("title", node), [track])
  const recapRef = useCallback((node: HTMLElement | null) => track("recap", node), [track])
  const triggerRef = useCallback((node: HTMLElement | null) => track("trigger", node), [track])

  useLayoutEffect(() => {
    measure()
    if (typeof ResizeObserver === "undefined") return
    const resize = new ResizeObserver(measure)
    observer.current = resize
    for (const node of Object.values(nodes.current)) {
      if (node) resize.observe(node)
    }
    return () => {
      resize.disconnect()
      observer.current = null
    }
  }, [measure])

  return { folded, headerRef, titleRef, recapRef, triggerRef }
}
