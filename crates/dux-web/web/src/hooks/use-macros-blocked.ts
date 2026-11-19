import { targetPtyId, targetTabDormant } from "@/components/terminalAreaModel"
import { usePaneMacroTarget } from "@/lib/paneMacroTarget"
import { useDuxSelector, type SelectedTarget } from "@/lib/store"

/// Why the macro picker cannot open for this target right now, or null when it
/// can. While the take-over card covers the pane this tab is not driving the
/// pty, and a dormant tab has no pane to carry a macro at all.
export function useMacrosBlockedHint(target: SelectedTarget): string | null {
  const dormant = useDuxSelector((s) => targetTabDormant(target, s))
  const pane = usePaneMacroTarget(targetPtyId(target))
  if (dormant) return "Start the tab to send macros"
  if (pane?.takeoverCard) return "Take over to send macros"
  return null
}
