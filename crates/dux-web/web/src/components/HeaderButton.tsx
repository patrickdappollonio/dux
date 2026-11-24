import type * as React from "react"

import { Button } from "@/components/ui/button"
import { cn } from "@/lib/utils"

import { HEADER_BOX } from "./ellipsisPlacements"

// The one format of a button in a pane, bar or form header: the Changes pane's,
// the centre pane's, the editor's. Outline, `h-8` (40px on a phone and under a
// finger), a 16px icon, a 6px gap between the icon and its word, and 2px of
// padding less than a plain button when the word is gone, so a lone icon is a
// square. The headers stay separate components; only their buttons share this.
// The header's `⋯` is the same box through `HEADER_BOX`, so the icon-only
// button and the `⋯` cannot drift apart.

/** The gap between the buttons of one header: the misclick spacing between
 * adjacent controls. A header lays its buttons out in a row wearing this. */
export const HEADER_BUTTON_ROW = "flex shrink-0 items-center gap-2"

// Where the word gives way to the icon alone, keeping the accessible name:
// `always` never does, `wide` below the xl breakpoint (a header whose path
// needs the room), `computer` on a phone (a phone prefers icon-only, and the
// tooltip names the control). The padding follows, so a hidden word leaves a
// square, 32px or 40px for a finger or a phone.
const WORDS = {
  always: { span: undefined, button: undefined },
  wide: {
    span: "max-xl:sr-only",
    button: "max-xl:px-2 pointer-coarse:max-xl:px-3 max-md:px-3",
  },
  computer: {
    span: "max-md:sr-only",
    button: "max-md:px-3",
  },
} as const

export type HeaderButtonWords = keyof typeof WORDS

type HeaderButtonProps = Omit<
  React.ComponentProps<typeof Button>,
  "variant" | "size" | "children" | "toggle"
> & {
  icon: React.ReactNode
  /** The word beside the icon. Without it the button is an icon-only square
   * and `aria-label` must name it. */
  label?: string
  /** Where the word is shown; `always` unless named. */
  words?: HeaderButtonWords
  /** The header's one filled primary action (Save), beside its outline peers. */
  primary?: boolean
  /** A two-state control that says which way it is set. */
  toggle?: boolean
}

export function HeaderButton({
  icon,
  label,
  words = "always",
  primary = false,
  toggle = false,
  className,
  ...props
}: HeaderButtonProps) {
  const labelled = label !== undefined
  return (
    <Button
      variant={primary ? "default" : "outline"}
      size={labelled ? "default" : "icon"}
      toggle={toggle}
      className={cn(
        labelled ? "h-8 shrink-0 max-md:h-10 pointer-coarse:h-10" : HEADER_BOX,
        labelled && WORDS[words].button,
        className,
      )}
      {...props}
    >
      {icon}
      {labelled ? <span className={WORDS[words].span}>{label}</span> : null}
    </Button>
  )
}
