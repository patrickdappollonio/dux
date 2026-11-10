import { ChevronDown, ChevronUp, Maximize2, Minimize2 } from "lucide-react"
import type { LucideIcon } from "lucide-react"

import type { ComputerLayout, LayoutAction } from "@/lib/layoutModel"

/** One layout control: what it does, its icon and the words it is named by. */
export interface LayoutControl {
  action: LayoutAction
  icon: LucideIcon
  label: string
}

/// The layout controls a computer offers for each layout: hide or maximize the
/// terminals from the split, and the way back from either maximized layout.
export function layoutControls(layout: ComputerLayout): LayoutControl[] {
  switch (layout) {
    case "split":
      return [
        { action: "hideTerminals", icon: ChevronDown, label: "Hide terminals" },
        { action: "maximizeTerminal", icon: Maximize2, label: "Maximize terminal" },
      ]
    case "agent":
      return [{ action: "restore", icon: ChevronUp, label: "Show terminals" }]
    case "terminal":
      return [{ action: "restore", icon: Minimize2, label: "Restore the split" }]
  }
}
