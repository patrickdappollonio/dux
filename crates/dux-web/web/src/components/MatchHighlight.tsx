import type { CharRange } from "@/lib/searchMatch"

// The one look for "this is what your search matched", on every web search.
// The caller computes `ranges` with the helper that mirrors its own filter
// (`lib/searchMatch.ts`), so the lit text is always why the row is shown; this
// component only paints them. Ranges are code points, sorted and disjoint, and
// anything past the end of the text is ignored. Array.from keeps CJK and emoji
// whole.
export function MatchHighlight({
  text,
  ranges,
}: {
  text: string
  ranges: readonly CharRange[]
}) {
  if (ranges.length === 0) return <>{text}</>
  const chars = Array.from(text)
  const parts: React.ReactNode[] = []
  let cursor = 0
  for (const { start, end } of ranges) {
    const from = Math.max(start, cursor)
    const to = Math.min(end, chars.length)
    if (from >= to) continue
    if (from > cursor) parts.push(chars.slice(cursor, from).join(""))
    parts.push(
      <mark
        key={from}
        className="rounded-[2px] bg-dux-search-match text-dux-search-match-foreground"
      >
        {chars.slice(from, to).join("")}
      </mark>,
    )
    cursor = to
  }
  if (cursor < chars.length) parts.push(chars.slice(cursor).join(""))
  return <>{parts}</>
}
