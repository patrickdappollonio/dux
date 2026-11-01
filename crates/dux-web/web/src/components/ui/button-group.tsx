// Vendored from shadcn/ui (button-group). The child selectors below square the
// inner corners and collapse the shared border. Three deviations, all forced by
// base-ui rather than radix: the Separator import path, ButtonGroupText using a
// plain div, and the orientation selectors counting siblings rather than using
// :first-child and :last-child. While a base-ui menu or popover is open, its
// trigger and its portal render hidden spans in place (focus guards on both
// sides of the trigger, and the portal's own guards and its aria-owns span), so
// upstream's selectors would square a trigger's outer corners for as long as its
// popup is open. "Not first" is therefore "follows a child that is not one of
// those spans" and "not last" is "is followed by a child that is not one".
import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"

import { cn } from "@/lib/utils"
import { Separator } from "@/components/ui/separator"

const buttonGroupVariants = cva(
  "flex w-fit items-stretch has-[>[data-slot=button-group]]:gap-2 [&>*]:focus-visible:relative [&>*]:focus-visible:z-10 has-[select[aria-hidden=true]:last-child]:[&>[data-slot=select-trigger]:last-of-type]:rounded-r-md [&>[data-slot=select-trigger]:not([class*='w-'])]:w-fit [&>input]:flex-1",
  {
    variants: {
      orientation: {
        horizontal:
          "[&>:not([data-base-ui-focus-guard]):not(span[aria-owns])~*]:rounded-l-none [&>:not([data-base-ui-focus-guard]):not(span[aria-owns])~*]:border-l-0 [&>*:has(~:not([data-base-ui-focus-guard]):not(span[aria-owns]))]:rounded-r-none",
        vertical:
          "flex-col [&>:not([data-base-ui-focus-guard]):not(span[aria-owns])~*]:rounded-t-none [&>:not([data-base-ui-focus-guard]):not(span[aria-owns])~*]:border-t-0 [&>*:has(~:not([data-base-ui-focus-guard]):not(span[aria-owns]))]:rounded-b-none",
      },
    },
    defaultVariants: {
      orientation: "horizontal",
    },
  },
)

function ButtonGroup({
  className,
  orientation,
  ...props
}: React.ComponentProps<"div"> & VariantProps<typeof buttonGroupVariants>) {
  return (
    <div
      role="group"
      data-slot="button-group"
      data-orientation={orientation}
      className={cn(buttonGroupVariants({ orientation }), className)}
      {...props}
    />
  )
}

function ButtonGroupText({
  className,
  ...props
}: React.ComponentProps<"div">) {
  return (
    <div
      className={cn(
        "flex items-center gap-2 rounded-md border bg-muted px-4 text-sm font-medium shadow-xs [&_svg]:pointer-events-none [&_svg:not([class*='size-'])]:size-4",
        className,
      )}
      {...props}
    />
  )
}

function ButtonGroupSeparator({
  className,
  orientation = "vertical",
  ...props
}: React.ComponentProps<typeof Separator>) {
  return (
    <Separator
      data-slot="button-group-separator"
      orientation={orientation}
      className={cn(
        "relative m-0! self-stretch bg-input data-[orientation=vertical]:h-auto",
        className,
      )}
      {...props}
    />
  )
}

export {
  ButtonGroup,
  ButtonGroupSeparator,
  ButtonGroupText,
  buttonGroupVariants,
}
