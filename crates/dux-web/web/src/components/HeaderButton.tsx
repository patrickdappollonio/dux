import type * as React from "react"

import { Button } from "@/components/ui/button"
import { cn } from "@/lib/utils"

import { HEADER_BOX } from "./ellipsisPlacements"

// The one format of a button in a pane, bar or form header: the Changes pane's,
// the centre pane's, the editor's. Outline, `h-8` (40px on a phone and under a
// finger), a 16px icon and a 6px gap between the icon and its word. A button
// that shows no word is exactly the header's `⋯` box (`HEADER_BOX`), whatever
// size it is. A caller names what it needs (`words`, `hide`, `tone`, `primary`,
// `toggle`) and never a class or a style, and a button with no visible word
// cannot be written without a name. The headers stay separate components; only
// their buttons share this, and `headerButtonGuard.test.ts` fails on a header
// that builds one by hand.

/** The gap between the buttons of one header: the misclick spacing between
 * adjacent controls. A header lays its buttons out in a row wearing this. */
export const HEADER_BUTTON_ROW = "flex shrink-0 items-center gap-2"

// Where the word gives way to the icon alone, keeping the accessible name:
// `always` never does, `wide` below the xl breakpoint (a header whose path
// needs the room), `computer` on a phone (a phone prefers icon-only, and the
// tooltip names the control). Where the word is gone the button takes the
// `⋯`'s box: the width is `HEADER_BOX`'s size at each of its sizes (the test
// reads them from it) and the padding goes, so the icon sits in the middle.
const WORDS = {
  always: { span: undefined, button: undefined },
  wide: {
    span: "max-xl:sr-only",
    button: "max-xl:w-8 max-xl:px-0 pointer-coarse:max-xl:w-10 max-md:w-10",
  },
  computer: {
    span: "max-md:sr-only",
    button: "max-md:w-10 max-md:px-0",
  },
} as const

export type HeaderButtonWords = keyof typeof WORDS

// Where a button is not shown at all, for a control whose job a phone does
// elsewhere.
const HIDE = { phone: "max-md:hidden" } as const

// A button's tone, for the one that warns.
const TONE = { warning: "text-dux-warning" } as const

type HeaderButtonBase = Omit<
  React.ComponentProps<typeof Button>,
  "variant" | "size" | "children" | "toggle" | "className" | "style" | "aria-label"
> & {
  icon: React.ReactNode
  /** Where the word is shown; `always` unless named. */
  words?: HeaderButtonWords
  /** Where the button is not shown at all. */
  hide?: keyof typeof HIDE
  /** The button's tone; neutral unless named. */
  tone?: keyof typeof TONE
  /** The header's one filled primary action (Save), beside its outline peers. */
  primary?: boolean
  /** A two-state control that says which way it is set. */
  toggle?: boolean
}

// The word beside the icon, or an `aria-label` that names the button when it
// has none: one of the two is always there, so a screen reader is never handed
// a nameless icon. `label` may be undefined at run time when a caller's word is
// optional, provided it names the button.
type HeaderButtonName =
  | { label: string; "aria-label"?: string }
  | { label?: string; "aria-label": string }

export type HeaderButtonProps = HeaderButtonBase & HeaderButtonName

export function HeaderButton({
  icon,
  label,
  words = "always",
  hide,
  tone,
  primary = false,
  toggle = false,
  ...props
}: HeaderButtonProps) {
  const labelled = label !== undefined && label !== ""
  return (
    <Button
      {...props}
      variant={primary ? "default" : "outline"}
      size={labelled ? "default" : "icon"}
      toggle={toggle}
      className={cn(
        labelled ? "h-8 shrink-0 max-md:h-10 pointer-coarse:h-10" : HEADER_BOX,
        labelled && WORDS[words].button,
        hide && HIDE[hide],
        tone && TONE[tone],
      )}
    >
      {icon}
      {/* Tabular figures, so a count in the word does not move the button as it
        * changes. */}
      {labelled ? (
        <span className={cn("tabular-nums", WORDS[words].span)}>{label}</span>
      ) : null}
    </Button>
  )
}

/**
 * Takes a header's button out of sight and out of reach while its job is done
 * elsewhere (the Changes pane's editor button, folded into its `⋯`), but keeps
 * it laid out so the header can still measure the width it would need to come
 * back. Folded, it is out of the flow and anchored to the row's trailing edge,
 * so it reaches back into the header and never past its edge; it is hidden, so
 * the keyboard, a pointer and a screen reader all skip it. Unfolded it adds no
 * box of its own. Wrap the control together with whatever wraps it (a tooltip's
 * or a disabled hint's trigger), so the wrapper goes with it.
 */
export function FoldedAway({
  folded,
  children,
}: {
  folded: boolean
  children: React.ReactNode
}) {
  return (
    <span className={folded ? "invisible absolute right-0" : "contents"}>{children}</span>
  )
}
