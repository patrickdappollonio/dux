import { foldForDivider, type ComputerLayout } from "@/lib/layoutModel"

// The smallest share either part keeps in the split before a drag folds it.
export const STACK_PART_MIN_PERCENT = 15

/// The class a part's box wears when it is the one typing does not go to.
export const NOT_TYPING_CLASS = "grayscale opacity-80"

/// The panel sizes, in percent of the stack, for a layout: the split at its
/// share, or the folded part down to its strip (nothing with the strips
/// hidden) and the other part taking the rest.
export function panelSizes(
  layout: ComputerLayout,
  hasDock: boolean,
  splitPercent: number,
  px: { group: number; agentHeader: number; dockHeader: number },
): { agent: number } {
  if (!hasDock) return { agent: 100 }
  const share = (headerPx: number) =>
    px.group > 0 ? Math.min(100, (headerPx / px.group) * 100) : 0
  switch (layout) {
    case "split":
      return { agent: splitPercent }
    case "agent":
      return { agent: 100 - share(px.dockHeader) }
    case "terminal":
      return { agent: share(px.agentHeader) }
  }
}

/// What a divider release in the split means: a fold into the other part's
/// maximized layout when a part was dragged past its minimum, or the agent's
/// new share to remember. A fold is never remembered as the split: the next
/// restore would come back at a size that folds again.
export function dividerRelease(
  agentShare: number,
  dockShare: number,
): { fold: Exclude<ComputerLayout, "split"> } | { persist: number } {
  const fold = foldForDivider({
    agentShare,
    dockShare,
    agentMin: STACK_PART_MIN_PERCENT,
    dockMin: STACK_PART_MIN_PERCENT,
  })
  return fold === null ? { persist: agentShare } : { fold }
}
