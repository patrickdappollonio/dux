import * as React from "react"
import { RotateCw, TriangleAlert } from "lucide-react"
import type { Button as ButtonPrimitive } from "@base-ui/react/button"

import { ErrorLine } from "@/components/ErrorLine"
import { Button } from "@/components/ui/button"
import { ListLine } from "@/components/ui/empty"

/**
 * The size is the only look a caller chooses, and each one has its place:
 * - `default`: a pane or a list whose load failed.
 * - `row`: an outlined button inside a list row.
 * - `quiet`: a short text-only button among a row's other controls.
 * - `fill`: the button is the whole row (a fixed-height tree row), so nothing
 *   of it can reach past the row's own height.
 * - `page`: a full-screen gate page, where it is the one control.
 */
export type RetryButtonSize = "default" | "row" | "quiet" | "fill" | "page"

const SIZE: Record<RetryButtonSize, { variant: "outline" | "ghost"; className: string }> = {
  default: { variant: "outline", className: "max-md:min-h-10 pointer-coarse:min-h-10" },
  // 44px on a phone and under a finger, like every control in a list row.
  row: { variant: "outline", className: "h-8 max-md:h-11 pointer-coarse:min-h-11" },
  quiet: {
    variant: "ghost",
    className: "h-7 px-2 text-xs text-muted-foreground max-md:min-h-11 pointer-coarse:min-h-11",
  },
  // Takes its height from the row it fills, which is the tree's own (32px
  // under a finger), so it has no touch floor of its own to outgrow it.
  fill: {
    variant: "ghost",
    className: "w-full justify-start gap-1 rounded px-1 font-normal text-muted-foreground [&_svg:not([class*='size-'])]:size-3.5",
  },
  page: { variant: "outline", className: "h-10" },
}

export interface RetryButtonProps
  extends Omit<ButtonPrimitive.Props, "children" | "className" | "type" | "render" | "nativeButton"> {
  onRetry: (event: React.MouseEvent<HTMLButtonElement>) => void
  size?: RetryButtonSize
  /** What the button says; "Retry" unless the place words it otherwise. */
  label?: string
  /** The warning sign in place of the retry icon, for a retry that is the only
   * sign something failed. */
  warning?: boolean
}

/**
 * The one control that repeats a failed load, with the one retry icon.
 *
 * It passes on everything a wrapper hands its child (a tooltip's trigger, a
 * menu's, a disabled-reason wrapper) and its ref, so it works inside any
 * `render=` slot.
 */
export function RetryButton({
  onRetry,
  size = "default",
  label = "Retry",
  warning = false,
  onClick,
  ...buttonProps
}: RetryButtonProps) {
  const { variant, className } = SIZE[size]
  const Icon = warning ? TriangleAlert : RotateCw
  return (
    <Button
      {...buttonProps}
      type="button"
      variant={variant}
      className={className}
      onClick={(event) => {
        onClick?.(event)
        onRetry(event)
      }}
    >
      <Icon aria-hidden />
      {label}
    </Button>
  )
}

/**
 * A failed load: its sentence, then the button that repeats it. `list` sits
 * where a list's empty and loading lines sit (`ListLine`), `center` fills a
 * pane. The sentence is the caller's, the server's or the deadline's; it is the
 * dialog error line, so it is the only red here and a screen reader hears it
 * once, when the load fails.
 */
export function FailedLoad({
  message,
  onRetry,
  layout,
}: {
  message: React.ReactNode
  onRetry: RetryButtonProps["onRetry"]
  layout: "list" | "center"
}) {
  const body = (
    <>
      <ErrorLine>{message}</ErrorLine>
      <RetryButton onRetry={onRetry} />
    </>
  )
  return layout === "list" ? (
    <ListLine>{body}</ListLine>
  ) : (
    <div className="flex h-full flex-col items-center justify-center gap-2 px-4 text-center">
      {body}
    </div>
  )
}
