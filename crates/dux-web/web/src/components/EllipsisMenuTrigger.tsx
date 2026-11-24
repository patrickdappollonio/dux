import type * as React from "react"
import { Ellipsis } from "lucide-react"

import { Spinner } from "@/components/Spinner"
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
  HEADER_BOX,
  isPillPlacement,
  REVEAL_CAP,
  ROW_BOX,
  type PillPlacement,
  type RowFit,
} from "./ellipsisPlacements"

type TriggerProps = Omit<
  React.ComponentProps<typeof DropdownMenuTrigger>,
  "render" | "className" | "style" | "children"
>

type Reveal = "hover" | "hover-or-focus"

/**
 * What a placement may ask for, one shape per placement so that a choice that
 * means nothing there is a compile error (`ellipsisTypeContract.tsx` pins it).
 * `reveal` is for a trigger inside a row or pill whose ancestor carries
 * `REVEAL_ROW`, and only those two place it so; `fit` is a row's, see
 * `ROW_BOX`; `phoneOnly` is a header's.
 */
type Placement =
  | { placement: "row"; reveal?: Reveal; fit?: RowFit }
  | { placement: PillPlacement; reveal?: Reveal }
  | { placement: "header"; phoneOnly?: boolean }
  | { placement: "toolbar" | "banner" | "floating" }

// A union member that does not name a key still has to refuse it, or a caller
// could pass it to a placement that has no such choice.
type Refused<T> = T extends unknown
  ? T & { [K in Exclude<"reveal" | "fit" | "phoneOnly", keyof T>]?: never }
  : never

/**
 * The one `⋯` that opens a menu. It goes inside a `DropdownMenu`, where a
 * `DropdownMenuTrigger` would, and may itself be the child of a tooltip.
 *
 * Without `reveal` it is painted. With one of the two hover reveals it takes no
 * room while idle, opens on hover or keyboard focus in the row, stays open
 * while its menu is, and is always there under a finger. `hover-or-focus` also
 * opens on mouse focus anywhere in the row; `hover` does not, for a row whose
 * menu hands focus back to its trigger and would strand it open.
 *
 * `phoneOnly` leaves a header's out of the computer layout, for a fold that
 * exists only where the controls beside it do not fit.
 *
 * `busy` swaps the dots for a spinner, takes no press, and holds a hover reveal
 * open so the spinner outlives the menu.
 */
export function EllipsisMenuTrigger({
  busy = false,
  "aria-label": ariaLabel,
  onClick,
  ...rest
}: TriggerProps &
  Refused<Placement> & {
    busy?: boolean
    "aria-label": string
  }) {
  const { placement, reveal, fit, phoneOnly, ...props } = rest as typeof rest & {
    reveal?: Reveal
    fit?: RowFit
    phoneOnly?: boolean
  }
  const pill = isPillPlacement(placement)
  const iconClass = pill ? "size-3.5" : undefined
  const handleClick = pill
    ? (e: React.MouseEvent<HTMLElement>) => {
        // The pill's own click selects the tab; this one only opens the menu.
        e.stopPropagation()
        ;(onClick as ((ev: React.MouseEvent<HTMLElement>) => void) | undefined)?.(e)
      }
    : onClick
  const icon = busy ? (
    <Spinner kind="ring" size={pill ? "sm" : "md"} />
  ) : (
    <Ellipsis className={iconClass} />
  )
  const box = pill
    ? BOX[placement]
    : placement === "row"
      ? ROW_BOX[fit ?? "standard"]
      : placement === "header"
        ? cn(HEADER_BOX, phoneOnly && "md:hidden")
        : BOX[placement]
  const trigger = (
    <DropdownMenuTrigger
      {...props}
      onClick={handleClick}
      disabled={busy || props.disabled}
      aria-busy={busy || undefined}
      aria-label={ariaLabel}
      render={
        pill ? (
          <button type="button" className={box} />
        ) : (
          <Button
            variant={placement === "header" ? "outline" : "ghost"}
            size={placement === "toolbar" ? "icon-sm" : "icon"}
            className={box}
          />
        )
      }
    >
      {icon}
    </DropdownMenuTrigger>
  )
  if (!reveal || !(pill || placement === "row")) return trigger
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
