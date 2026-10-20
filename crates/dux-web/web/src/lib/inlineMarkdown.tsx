import type { ReactNode } from "react"

import { MatchHighlight } from "@/components/MatchHighlight"
import { InlineCode } from "@/components/ui/inline-code"
import { matchCharRanges, type CharRange } from "@/lib/searchMatch"

interface InlineSegment {
  text: string
  code: boolean
}

// Splits single-backtick-delimited spans out of the text. Parts alternate
// plain/code when every opening backtick found a partner. A dangling last
// backtick and everything after it stay literal text rather than being dropped.
function inlineSegments(text: string): InlineSegment[] {
  const parts = text.split("`")
  const total = parts.length
  const hasDanglingBacktick = total % 2 === 0
  const segments: InlineSegment[] = []
  parts.forEach((part, index) => {
    const isLastPart = index === total - 1
    if (index % 2 === 1 && !(hasDanglingBacktick && isLastPart)) {
      segments.push({ text: part, code: true })
      return
    }
    // Dangling trailing backtick: restore the literal backtick character that
    // `split` consumed, so the plain text renders exactly as written.
    const literal = hasDanglingBacktick && isLastPart ? `\`${part}` : part
    if (literal.length > 0) segments.push({ text: literal, code: false })
  })
  return segments
}

/** The text as `renderInlineCode` shows it, with the delimiting backticks gone:
 * what a search over that prose has to match, so a hit is something on screen. */
export function inlineCodeDisplayText(text: string): string {
  return inlineSegments(text)
    .map((segment) => segment.text)
    .join("")
}

// Renders single-backtick-delimited spans as <code> elements. Deliberately not a markdown
// parser: no bold, no italic, no links, and no dependency for it. Setting descriptions and
// dialog copy are adapted from config.toml comments that use backticks for inline code, which
// as plain text left the literal backtick characters visible in the UI.
//
// With `needles`, every occurrence of each one in the shown text is lit through the shared
// `MatchHighlight`, a chip's text included; a hit that runs across a chip's edge is lit on
// both sides of it.
export function renderInlineCode(text: string, needles: readonly string[] = []): ReactNode[] {
  const segments = inlineSegments(text)
  if (segments.length === 0) return [text]
  const ranges = needles.length > 0 ? matchCharRanges(inlineCodeDisplayText(text), needles) : []

  let offset = 0
  return segments.map((segment, index) => {
    const length = Array.from(segment.text).length
    const local: CharRange[] = ranges
      .filter((r) => r.end > offset && r.start < offset + length)
      .map((r) => ({
        start: Math.max(r.start, offset) - offset,
        end: Math.min(r.end, offset + length) - offset,
      }))
    offset += length
    if (segment.code) {
      return (
        <InlineCode key={index}>
          {local.length > 0 ? <MatchHighlight text={segment.text} ranges={local} /> : segment.text}
        </InlineCode>
      )
    }
    return local.length > 0 ? (
      <MatchHighlight key={index} text={segment.text} ranges={local} />
    ) : (
      segment.text
    )
  })
}
