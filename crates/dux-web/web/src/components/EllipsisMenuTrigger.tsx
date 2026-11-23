import type * as React from "react"
import { Ellipsis, Loader2 } from "lucide-react"

import { Button } from "@/components/ui/button"
import { DropdownMenuTrigger } from "@/components/ui/dropdown-menu"
import {
  ALWAYS_REVEALED_ON_PHONE,
  HOVER_REVEAL,
  REVEALED_ON_ANY_FOCUS,
} from "@/lib/hoverReveal"
import { cn } from "@/lib/utils"

import {
  BOX,
  IS_PILL,
  REVEAL_CAP,
  type EllipsisPlacement,
} from "./ellipsisPlacements"

type TriggerProps = Omit<
  React.ComponentProps<typeof DropdownMenuTrigger>,
  "render" | "className" | "style" | "children"
>

/**
 * The one `⋯` that opens a menu. It goes inside a `DropdownMenu`, where a
 * `DropdownMenuTrigger` would, and may itself be the child of a tooltip.
 *
 * `reveal` is `always` (painted, the default) or, for a trigger inside a row or
 * pill whose ancestor carries `REVEAL_ROW`, one of two hover reveals: both take
 * no room while idle, open on hover or keyboard focus in the row, stay open
 * while their menu is, and are always there under a finger. `hover-or-focus`
 * also opens on mouse focus anywhere in the row; `hover` does not, for a row
 * whose menu hands focus back to its trigger and would strand it open.
 *
 * `roomy` keeps a phone's 44px box for a pane whose neighbours already have it,
 * so the controls of one cluster share a height. `slim` narrows a row's `⋯` on a
 * phone under a finger, keeping the 40px floor in height, for a row whose title
 * needs the width: it sits between the row's text and the screen's edge, and
 * above and below are the rows' own full-height controls.
 *
 * `phoneOnly` leaves it out of the computer layout, for a fold that exists only
 * where the controls beside it do not fit.
 *
 * `busy` swaps the dots for a spinner, takes no press, and holds a hover reveal
 * open so the spinner outlives the menu.
 */
export function EllipsisMenuTrigger({
  placement,
  reveal = "always",
  busy = false,
  phoneOnly = false,
  roomy = false,
  slim = false,
  "aria-label": ariaLabel,
  onClick,
  ...props
}: TriggerProps & {
  placement: EllipsisPlacement
  reveal?: "always" | "hover" | "hover-or-focus"
  roomy?: boolean
  slim?: boolean
  busy?: boolean
  phoneOnly?: boolean
  "aria-label": string
}) {
  const pill = IS_PILL(placement)
  const iconClass = pill ? "size-3.5" : undefined
  const handleClick = pill
    ? (e: React.MouseEvent<HTMLElement>) => {
        // The pill's own click selects the tab; this one only opens the menu.
        e.stopPropagation()
        ;(onClick as ((ev: React.MouseEvent<HTMLElement>) => void) | undefined)?.(e)
      }
    : onClick
  const icon = busy ? (
    <Loader2 className={cn("motion-safe:animate-spin", iconClass)} />
  ) : (
    <Ellipsis className={iconClass} />
  )
  const trigger = (
    <DropdownMenuTrigger
      {...props}
      onClick={handleClick}
      disabled={busy || props.disabled}
      aria-busy={busy || undefined}
      aria-label={ariaLabel}
      render={
        pill ? (
          <button type="button" className={BOX[placement]} />
        ) : (
          <Button
            variant={placement === "header" ? "outline" : "ghost"}
            size={placement === "toolbar" ? "icon-sm" : "icon"}
            className={cn(
              BOX[placement],
              roomy && "max-md:size-11 max-md:pointer-coarse:size-11",
              slim && "max-md:pointer-coarse:w-7",
              phoneOnly && "md:hidden",
            )}
          />
        )
      }
    >
      {icon}
    </DropdownMenuTrigger>
  )
  if (reveal === "always") return trigger
  return (
    <div
      className={cn(
        HOVER_REVEAL,
        REVEAL_CAP[placement],
        "has-[[aria-busy=true]]:max-w-(--reveal-cap) has-[[aria-busy=true]]:opacity-100",
        reveal === "hover-or-focus" && REVEALED_ON_ANY_FOCUS,
        !pill && ALWAYS_REVEALED_ON_PHONE,
      )}
    >
      {trigger}
    </div>
  )
}
