import { Ellipsis } from "lucide-react"

import { InputMenuItems } from "@/components/InputMenuItems"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { inputMenuHasItems, type InputMenuGates } from "@/lib/inputMenu"
import { cn } from "@/lib/utils"

// The menu that belongs to the virtual input and lives and dies with it. It is
// input-local, and that is the whole of it: the way out of the virtual input,
// and the terminal-keys toggle. It is deliberately not a permanent surface, so
// what must always be reachable lives in the top menu instead, through
// `PaneInputGroup`.
//
// It renders at the leading edge of the bottom-most input row that exists: the
// compose row's leading slot when the message box is up, the accessory bar's
// row-one trailing slot when only the keys are. The anchors are mutually
// exclusive by construction, so exactly one instance ever renders.
//
// It never renders empty, the empty state being reachable, so every anchor asks
// `inputMenuHasItems` first and this component asks again for those that do not.
export function InputMenu({
  gates,
  composeSurface,
  directLeavesNothingBelow,
  keysHideLeavesNothingBelow,
  className,
}: {
  gates: InputMenuGates
  composeSurface?: boolean
  /// Whether a switch to direct typing would leave nothing under the terminal,
  /// which is the only case the one-time way-back hint fires in. The pane knows
  /// (its key row may well survive the switch) and passes the answer down.
  directLeavesNothingBelow?: boolean
  /// The same question for the other flip in this menu: whether hiding the
  /// terminal keys would leave nothing under the terminal.
  keysHideLeavesNothingBelow?: boolean
  className?: string
}) {
  if (!inputMenuHasItems(gates)) return null
  return (
    <DropdownMenu>
      <SimpleTooltip content="Input options">
        <DropdownMenuTrigger
          render={
            <Button
              variant="ghost"
              aria-label="Input options"
              // `size-10` keeps the 40px touch-target floor on both axes;
              // `self-end` bottom-aligns it beside a grown multi-row textarea
              // exactly as Send does (inert in the single-child fallback row).
              className={cn("size-10 shrink-0 self-end", className)}
            />
          }
        >
          <Ellipsis />
        </DropdownMenuTrigger>
      </SimpleTooltip>
      {/* Anchored ABOVE its trigger: the trigger sits on the bottom edge of the
          window, where a downward popup has nowhere to go. On a phone the
          primitive renders every menu as a bottom sheet and ignores this. */}
      <DropdownMenuContent side="top" align="start">
        <InputMenuItems
          gates={gates}
          composeSurface={composeSurface}
          directLeavesNothingBelow={directLeavesNothingBelow}
          keysHideLeavesNothingBelow={keysHideLeavesNothingBelow}
        />
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
