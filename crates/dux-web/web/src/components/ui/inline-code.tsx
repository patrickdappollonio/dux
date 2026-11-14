import { useCallback, useLayoutEffect, useRef, useState } from "react"

import { cn } from "@/lib/utils"

// The one chip for a name inside prose: a branch, a path, a file, a command, an
// agent, project or terminal name, a pull request number. The chip is what
// tells the name apart from the sentence around it, so callers drop the quotes
// they used to wrap it in. Sized relative to its surroundings, so it sits in a
// dialog title as comfortably as in body text. To a screen reader it is plain
// inline text.
//
// A chip that fits on a row never splits: its spaces are kept and it offers no
// break inside (`pre`), so one that does not fit the rest of a row moves to the
// next one whole, hyphen, spaces and all, as the terminal UI's name chip does.
// Otherwise a hyphen or a space inside it is a break point like any other,
// which drew `demo-web` as two pills, `demo-` at the end of one row and `web`
// at the start of the next. It stays an inline box rather than an inline
// block: an inline block is a break point on both sides whatever comes after
// it, so a sentence ending in a chip could start a row with its lone `.` or
// `?`, and in a row that truncates with an ellipsis an inline block that does
// not fit is swallowed whole by the ellipsis, name and all, where inline text
// is cut mid-name and stays readable.
//
// A chip longer than a whole row cannot move anywhere whole. That chip, and
// only that one, flows like the words around it, breaking where they may
// (it keeps its spaces but takes whether to wrap at all from its row, so in a
// row that truncates it stays on one line and is cut by the ellipsis), and
// anywhere as a last resort (`anywhere` rather than `break-word`, because it also shrinks the
// chip's minimum width, so a long path cannot stretch a flex or grid cell, or
// a toast, past the screen). Every row it spans gets a background of its own
// that hugs that row's text (`box-decoration-clone`), and the sentence carries
// on right after it. Which of the two a chip is depends on how wide it is
// against the row it sits in, which only layout knows, so it is measured, and
// measured again whenever that row changes width.
//
// Nothing is substituted for its spaces in either mode, so a copied chip is
// byte for byte the value.
const CHIP_CLASSES = "inline rounded bg-muted px-1.5 py-0.5 font-mono text-[0.85em] box-decoration-clone"
const WHOLE_CLASSES = "whitespace-pre"
const FLOW_CLASSES = "[white-space-collapse:preserve] wrap-anywhere"

function InlineCode({ className, ref, children, ...props }: React.ComponentProps<"code">) {
  const own = useRef<HTMLElement>(null)
  const longerThanARow = useLongerThanARow(own, children)
  const setRef = useCallback(
    (node: HTMLElement | null) => {
      own.current = node
      if (typeof ref === "function") ref(node)
      else if (ref) ref.current = node
    },
    [ref],
  )
  return (
    <code
      ref={setRef}
      data-slot="inline-code"
      data-wrap={longerThanARow ? "flow" : "whole"}
      className={cn(CHIP_CLASSES, longerThanARow ? FLOW_CLASSES : WHOLE_CLASSES, className)}
      {...props}
    >
      {children}
    </code>
  )
}

// Whether the chip, laid out on one line, is wider than the rows of the block
// it sits in, less whatever it cannot be parted from after it (a `.` or a `?`
// that has to share its row). Re-measured when its content changes, when that
// block changes width, and once the page's fonts have loaded, since a fallback
// font measures differently.
function useLongerThanARow(chip: React.RefObject<HTMLElement | null>, content: React.ReactNode): boolean {
  const [longer, setLonger] = useState(false)
  useLayoutEffect(() => {
    const el = chip.current
    const block = el ? rowsOf(el) : null
    if (!el || !block) return
    const measure = () => setLonger(measureLonger(el, block))
    measure()
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure)
    observer?.observe(block)
    let live = true
    void document.fonts?.ready.then(() => {
      if (live) measure()
    })
    return () => {
      live = false
      observer?.disconnect()
    }
  }, [chip, content])
  return longer
}

// The nearest ancestor that lays out rows of text, past any inline wrapper the
// chip sits in (a `strong`, a highlight).
function rowsOf(el: HTMLElement): HTMLElement | null {
  for (let node = el.parentElement; node; node = node.parentElement) {
    const display = getComputedStyle(node).display
    if (display !== "inline" && display !== "contents") return node
  }
  return null
}

function rowWidth(block: HTMLElement): number {
  const style = getComputedStyle(block)
  return block.clientWidth - (parseFloat(style.paddingLeft) || 0) - (parseFloat(style.paddingRight) || 0)
}

// Two synchronous reads inside the layout effect, so neither probe is ever
// painted. The row is read with the chip flowing, because a chip that is kept
// whole sets the minimum width of a block sized by its content (a flex item):
// read with the chip whole, a block the chip has stretched past its container
// would always report room for it and the chip would never flow. The chip is
// then laid out at its widest for its one-line width, padding included.
function measureLonger(el: HTMLElement, block: HTMLElement): boolean {
  const { display, width, maxWidth, whiteSpace, overflowWrap } = el.style
  el.style.whiteSpace = "pre-wrap"
  el.style.overflowWrap = "anywhere"
  const row = rowWidth(block) - trailingWidth(el)
  el.style.display = "inline-block"
  el.style.width = "max-content"
  el.style.maxWidth = "none"
  const oneLine = el.getBoundingClientRect().width
  el.style.display = display
  el.style.width = width
  el.style.maxWidth = maxWidth
  el.style.whiteSpace = whiteSpace
  el.style.overflowWrap = overflowWrap
  return oneLine > row + 0.5
}

// The width of the text glued to the chip's end: the run of the following text
// up to its first space, which a row cannot break away from the chip.
function trailingWidth(el: HTMLElement): number {
  const next = el.nextSibling
  if (!(next instanceof Text)) return 0
  const glued = /^\S+/.exec(next.data)
  if (!glued) return 0
  const range = document.createRange()
  range.setStart(next, 0)
  range.setEnd(next, glued[0].length)
  // jsdom's ranges cannot be measured.
  return typeof range.getBoundingClientRect === "function" ? range.getBoundingClientRect().width : 0
}

export { InlineCode }
