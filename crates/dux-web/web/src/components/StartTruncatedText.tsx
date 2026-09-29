import { useEffect, useRef, useState } from "react"

import { SimpleTooltip } from "@/components/SimpleTooltip"
import { isTruncated } from "@/hooks/use-truncated"
import { cn } from "@/lib/utils"

/**
 * One line of text that ellipsizes at its START, so the end stays visible:
 * "…/patrickdappollonio/dux" rather than "/home/patrick/Golang/src/gith…". For
 * paths, whose meaningful part is the tail (a file's name, a project's folder).
 *
 * The box is `direction: rtl`, which moves both the clipping and the ellipsis to
 * the left edge, and `text-left`, so a text that fits sits where an ordinary one
 * would. The text itself is one `<bdi dir="ltr">` isolate: without it the rtl
 * box reorders bidi-neutral characters at the edges, so a leading "/" or the
 * "." of a dotfile path would be drawn at the far end. Inside the isolate the
 * text lays out exactly as written, and the whole of it stays in the DOM, so a
 * screen reader always hears all of it; the clipping is visual only.
 *
 * `tooltip` puts the whole text in the shared tooltip, for a surface where the
 * row gives no other way to read what was clipped. See `ClippedHint`.
 */
export function StartTruncatedText({
  text,
  className,
  tooltip = false,
}: {
  text: string
  className?: string
  tooltip?: boolean
}) {
  return tooltip ? (
    <ClippedHint text={text} className={className} />
  ) : (
    startTruncatedBox(text, className)
  )
}

// A plain element rather than a component, so the tooltip trigger that renders
// it can hand the span its own props and ref directly.
function startTruncatedBox(
  text: string,
  className?: string,
  boxRef?: React.Ref<HTMLSpanElement>,
) {
  return (
    <span
      ref={boxRef}
      data-truncate="start"
      className={cn("min-w-0 truncate text-left [direction:rtl]", className)}
    >
      <bdi dir="ltr">{text}</bdi>
    </span>
  )
}

// The hint says what the ellipsis hid, so it opens only while the text is
// actually clipped, measured at the gesture itself (a hover, or a focus) rather
// than by an observer per row. The box is plain text and takes no tab stop: a
// keyboard user reaches the hint by focusing the control the text sits in (a
// list row's button), whose focus opens it and whose blur closes it, so the
// row's keyboard model is unchanged.
function ClippedHint({ text, className }: { text: string; className?: string }) {
  const boxRef = useRef<HTMLSpanElement>(null)
  const [open, setOpen] = useState(false)

  useEffect(() => {
    const box = boxRef.current
    const host = box?.parentElement?.closest<HTMLElement>(
      "button, a[href], [tabindex]",
    )
    if (!box || !host) return
    const onFocus = () => {
      if (focusIsVisible(host) && isTruncated(box)) setOpen(true)
    }
    const onBlur = () => setOpen(false)
    host.addEventListener("focus", onFocus)
    host.addEventListener("blur", onBlur)
    return () => {
      host.removeEventListener("focus", onFocus)
      host.removeEventListener("blur", onBlur)
    }
  }, [])

  return (
    <SimpleTooltip
      content={text}
      open={open}
      onOpenChange={(next) => {
        const box = boxRef.current
        setOpen(next && box !== null && isTruncated(box))
      }}
    >
      {startTruncatedBox(text, className, boxRef)}
    </SimpleTooltip>
  )
}

// A pointer press focuses the row's button too, and a hint flashing up on every
// click is noise, so only a keyboard-style focus opens it. A browser that
// cannot answer the selector is treated as a keyboard focus.
function focusIsVisible(el: HTMLElement): boolean {
  try {
    return el.matches(":focus-visible")
  } catch {
    return true
  }
}
