import { Loader2 } from "lucide-react"
import type { ReactNode } from "react"

import { cn } from "@/lib/utils"

// The one loading indicator in the web UI: a ring that turns. It turns through
// the `motion-safe:` variant, so under reduced motion it stays as a still ring
// and the page keeps saying "loading" without moving.
//
// `size` is named: "sm" and "md" and "lg" are fixed boxes, and "fit" leaves the
// size to the host, for a slot that already sizes its icon (a button's icon
// slot, an Empty's icon tile). `tone` is "muted" for a quiet pane or row,
// "current" to follow the surrounding text (a button face, a coloured pill),
// "primary" where the spinner is the thing the eye should rest on.
//
// Decorative: whatever the spinner stands for is said in words beside it.
const SIZES = {
  sm: "size-3.5",
  md: "size-4",
  lg: "size-5",
  fit: "",
} as const

const TONES = {
  muted: "text-muted-foreground",
  current: "",
  primary: "text-primary",
} as const

export type SpinnerSize = keyof typeof SIZES

export function Spinner({
  size = "md",
  tone = "muted",
}: {
  size?: SpinnerSize
  tone?: keyof typeof TONES
}) {
  return (
    <Loader2
      aria-hidden="true"
      className={cn(
        "shrink-0 motion-safe:animate-spin",
        SIZES[size],
        TONES[tone],
      )}
    />
  )
}

// A spinner with the sentence that says what is loading, for the places that
// show one. Where the row sits (centred, padded, in a list) is the caller's;
// `announce` makes the sentence a live region for the rows that already were.
export function LoadingRow({
  size = "md",
  announce = false,
  children,
}: {
  size?: "sm" | "md"
  announce?: boolean
  children: ReactNode
}) {
  return (
    <span
      className={cn(
        "inline-flex items-center gap-2 text-muted-foreground",
        size === "sm" ? "text-xs" : "text-sm",
      )}
    >
      <Spinner size={size === "sm" ? "sm" : "md"} />
      <span role={announce ? "status" : undefined}>{children}</span>
    </span>
  )
}
