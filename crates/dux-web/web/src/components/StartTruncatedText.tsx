import { SimpleTooltip } from "@/components/SimpleTooltip"
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
 * row gives no other way to read what was clipped.
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
  const box = (
    <span
      data-truncate="start"
      className={cn("min-w-0 truncate text-left [direction:rtl]", className)}
    >
      <bdi dir="ltr">{text}</bdi>
    </span>
  )
  return tooltip ? <SimpleTooltip content={text}>{box}</SimpleTooltip> : box
}
