import type { ReactNode } from "react"

import { TerminalStack, type TerminalPartProps } from "@/components/TerminalStack"

export { TerminalStack } from "@/components/TerminalStack"
export type { TerminalPartProps } from "@/components/TerminalStack"

type TerminalPaneProps = TerminalPartProps & {
  // Painted over the stack's box, which is the only positioning context that
  // excludes the compose row and the key rows sitting under the terminals.
  overlay?: ReactNode
}

/// One terminal screen: today's single terminal, run as a stack of one.
export function TerminalPane(props: TerminalPaneProps) {
  const { overlay, ...part } = props
  return <TerminalStack parts={[part]} overlay={overlay} />
}
