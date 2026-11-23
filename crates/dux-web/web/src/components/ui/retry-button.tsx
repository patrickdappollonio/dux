import * as React from "react"
import { RotateCw } from "lucide-react"

import { Button } from "@/components/ui/button"

/**
 * The size is the only choice a caller makes, and each one has its place:
 * `default` for a pane or a list whose load failed, `row` for a button that
 * sits inside a list row (a quieter, shorter one), `page` for a full-screen
 * gate page, where it is the one control.
 */
export type RetryButtonSize = "default" | "row" | "page"

const SIZE: Record<RetryButtonSize, { variant: "outline" | "ghost"; className: string }> = {
  default: { variant: "outline", className: "max-md:min-h-10 pointer-coarse:min-h-10" },
  // 44px on a phone and under a finger, like every control in a list row.
  row: {
    variant: "ghost",
    className: "h-7 px-2 text-xs text-muted-foreground max-md:min-h-11 pointer-coarse:min-h-11",
  },
  page: { variant: "outline", className: "h-10" },
}

export interface RetryButtonProps {
  onRetry: (event: React.MouseEvent<HTMLButtonElement>) => void
  size?: RetryButtonSize
  /** What the button says; "Retry" unless the place words it otherwise. */
  label?: string
  disabled?: boolean
  tabIndex?: number
}

/** The one control that repeats a failed load, with the one retry icon. */
export function RetryButton({
  onRetry,
  size = "default",
  label = "Retry",
  disabled,
  tabIndex,
}: RetryButtonProps) {
  const { variant, className } = SIZE[size]
  return (
    <Button variant={variant} className={className} disabled={disabled} tabIndex={tabIndex} onClick={onRetry}>
      <RotateCw aria-hidden />
      {label}
    </Button>
  )
}

/**
 * A failed load: its sentence, then the button that repeats it. `start` sits
 * at the top left of a list or a tree, `center` fills a pane. The sentence is
 * the caller's: the server's, or the deadline's.
 */
export function FailedLoad({
  message,
  onRetry,
  layout = "start",
}: {
  message: React.ReactNode
  onRetry: RetryButtonProps["onRetry"]
  layout?: "start" | "center"
}) {
  return (
    <div
      className={
        layout === "center"
          ? "flex h-full flex-col items-center justify-center gap-2 px-4 text-center text-sm text-destructive"
          : "flex flex-col items-start gap-2 px-2 py-2 text-sm text-destructive"
      }
    >
      <p>{message}</p>
      <RetryButton onRetry={onRetry} />
    </div>
  )
}
