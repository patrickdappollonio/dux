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

// THE INPUT `⋯`: the one menu attached to the virtual input area, and the
// reason the hidden-bars dead end cannot come back. It replaces the old
// conditional "show hidden bars" button, which only existed while something was
// hidden and could therefore only ever be a way back, never a way there.
//
// It renders in EVERY bar state, at the leading edge of the bottom-most input
// row that exists (see the anchor matrix in TerminalPane): the compose row's
// leading slot when the message box is up, the accessory bar's row-one trailing
// slot when only the keys are up, and its own minimal row when neither is.
// Exactly one instance ever renders; the anchors are mutually exclusive by
// construction and a test pins the state that used to produce two.
//
// It NEVER renders empty: an `⋯` that opens nothing is worse than no `⋯`, and
// the empty state is reachable, so every anchor asks `inputMenuHasItems` first
// and this component asks again for the callers that do not.
export function InputMenu({
  gates,
  onAttach,
  composeSurface,
  className,
}: {
  gates: InputMenuGates
  onAttach?: () => void
  composeSurface?: boolean
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
          onAttach={onAttach}
          composeSurface={composeSurface}
        />
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
