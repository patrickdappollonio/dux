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

// THE INPUT `⋯`: the menu that belongs to the virtual input, and lives and dies
// with it. It is input-LOCAL, and that is the whole of it: the way out of the
// virtual input, and the terminal-keys toggle.
//
// IT IS NOT A PERMANENT SURFACE, deliberately. It used to render in every bar
// state, minimal row of its own included, so that it could be the one menu
// always within reach; asking to type directly in the terminal now takes the
// whole bottom bar, and a row kept alive purely to hold an `⋯` is exactly the
// chrome that choice is asking to be rid of. What must always be reachable
// (attaching a file, the way back to the virtual input) moved up into the top
// menu every surface already has, through `PaneInputGroup`.
//
// It renders at the leading edge of the bottom-most input row that exists: the
// compose row's leading slot when the message box is up, the accessory bar's
// row-one trailing slot when only the keys are up. Exactly one instance ever
// renders; the anchors are mutually exclusive by construction and a test pins
// the state that could produce two.
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
