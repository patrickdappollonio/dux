import type { ReactNode } from "react"

// Renders single-backtick-delimited spans (e.g. "the `gh` CLI") as <code>
// elements. This is deliberately NOT a markdown parser: no bold, no italic, no
// links, and no dependency is added for it. It exists because setting
// descriptions and dialog copy are adapted from config.toml comments that use
// backticks for inline code, and rendering them as plain text left the literal
// backtick characters visible in the UI.
//
// Pure and side-effect free so it is unit-testable without mounting anything
// beyond the returned React nodes.
export function renderInlineCode(text: string): ReactNode[] {
  const parts = text.split("`")

  // An even number of backticks means every opening backtick found a partner,
  // so parts alternate plain/code/plain/code/... starting and ending on a
  // plain-text segment. An odd count means the last backtick is dangling (no
  // closing partner): treat it, and everything after it, as literal text by
  // rejoining the tail back onto the plain-text stream instead of dropping it.
  const isCodeSegment = (index: number) => index % 2 === 1

  const nodes: ReactNode[] = []
  const total = parts.length
  const hasDanglingBacktick = total % 2 === 0

  parts.forEach((part, index) => {
    const isLastPart = index === total - 1
    const treatAsCode = isCodeSegment(index) && !(hasDanglingBacktick && isLastPart)

    if (treatAsCode) {
      nodes.push(
        <code
          key={index}
          className="rounded bg-muted px-1.5 py-0.5 font-mono text-[0.85em]"
        >
          {part}
        </code>,
      )
      return
    }

    // Dangling trailing backtick: restore the literal backtick character that
    // `split` consumed, so the plain text renders exactly as written.
    const literal = hasDanglingBacktick && isLastPart ? `\`${part}` : part
    if (literal.length > 0) {
      nodes.push(literal)
    }
  })

  return nodes.length > 0 ? nodes : [text]
}
