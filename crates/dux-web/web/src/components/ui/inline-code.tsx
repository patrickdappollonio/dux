import { useCallback, useLayoutEffect, useRef, useState } from "react"

import { cn } from "@/lib/utils"

// The one chip for a name inside prose: a branch, a path, a file, a command, an
// agent, project or terminal name, a pull request number. The chip is what
// tells the name apart from the sentence around it, so callers drop the quotes
// they used to wrap it in. Sized relative to its surroundings, so it sits in a
// dialog title as comfortably as in body text. To a screen reader it is plain
// inline text.
//
// A chip that fits on a row is one inline block, so it never splits: one that
// does not fit the rest of a row moves to the next one whole, hyphen, spaces
// and all, as the terminal UI's name chip does. Inside an inline box a hyphen
// or a space is a break point like any other, which drew `demo-web` as two
// pills, `demo-` at the end of one row and `web` at the start of the next.
//
// A chip longer than a whole row cannot move anywhere whole, and as an inline
// block it would fill every row it spans and push the rest of the sentence
// under it. So that chip, and only that one, is an inline box instead: it
// flows like the words around it, every row it spans gets a background of its
// own that hugs that row's text (`box-decoration-clone`), and the sentence
// carries on right after it. Which of the two a chip is depends on how wide it
// is against the row it sits in, which only layout knows, so it is measured,
// and measured again whenever that row changes width.
//
// Its spaces are kept (`pre-wrap`), because in a command they are part of the
// value, and nothing is substituted for them, so a copied chip is byte for byte
// the value. `anywhere` rather than `break-word` because it also shrinks the
// chip's minimum width, so a long path cannot stretch a flex or grid cell, or a
// toast, past the screen.
const CHIP_CLASSES =
  "max-w-full rounded bg-muted px-1.5 py-0.5 font-mono text-[0.85em] box-decoration-clone whitespace-pre-wrap wrap-anywhere"

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
      className={cn(CHIP_CLASSES, longerThanARow ? "inline" : "inline-block", className)}
      {...props}
    >
      {children}
    </code>
  )
}

// Whether the chip, laid out on one line, is wider than the rows of the block
// it sits in. Re-measured when its content changes, when that block changes
// width, and once the page's fonts have loaded, since a fallback font measures
// differently.
function useLongerThanARow(chip: React.RefObject<HTMLElement | null>, content: React.ReactNode): boolean {
  const [longer, setLonger] = useState(false)
  useLayoutEffect(() => {
    const el = chip.current
    const block = el ? rowsOf(el) : null
    if (!el || !block) return
    const measure = () => setLonger(oneLineWidth(el) > rowWidth(block) + 0.5)
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

// The chip's width on one line, padding included, read by laying it out at its
// widest for one synchronous measurement inside the layout effect, so the
// probe is never painted.
function oneLineWidth(el: HTMLElement): number {
  const { display, width, maxWidth } = el.style
  el.style.display = "inline-block"
  el.style.width = "max-content"
  el.style.maxWidth = "none"
  const measured = el.getBoundingClientRect().width
  el.style.display = display
  el.style.width = width
  el.style.maxWidth = maxWidth
  return measured
}

export { InlineCode }
