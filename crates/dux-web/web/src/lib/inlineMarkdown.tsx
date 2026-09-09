import type { ReactNode } from "react"

// Renders single-backtick-delimited spans as <code> elements. Deliberately not a markdown
// parser: no bold, no italic, no links, and no dependency for it. Setting descriptions and
// dialog copy are adapted from config.toml comments that use backticks for inline code, which
// as plain text left the literal backtick characters visible in the UI.
export function renderInlineCode(text: string): ReactNode[] {
  const parts = text.split("`")

  // Parts alternate plain/code when every opening backtick found a partner. A dangling last
  // backtick and everything after it stay literal text rather than being dropped.
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
