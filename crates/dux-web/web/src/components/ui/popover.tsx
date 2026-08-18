import * as React from "react"
import { Popover as PopoverPrimitive } from "@base-ui/react/popover"

import { cn } from "@/lib/utils"
import { useIsMobile } from "@/hooks/use-mobile"
import {
  SHEET_BACKDROP_CLASS,
  SHEET_POPUP_CLASS,
  SHEET_POSITIONER_STYLE,
} from "@/components/ui/popupSheet"

function Popover({ ...props }: PopoverPrimitive.Root.Props) {
  return <PopoverPrimitive.Root data-slot="popover" {...props} />
}

function PopoverTrigger({ ...props }: PopoverPrimitive.Trigger.Props) {
  return <PopoverPrimitive.Trigger data-slot="popover-trigger" {...props} />
}

function PopoverContent({
  className,
  align = "center",
  alignOffset = 0,
  side = "bottom",
  sideOffset = 4,
  ...props
}: PopoverPrimitive.Popup.Props &
  Pick<
    PopoverPrimitive.Positioner.Props,
    "align" | "alignOffset" | "side" | "sideOffset"
  >) {
  const isMobile = useIsMobile()
  // Sheet mode hands initial focus to the popup itself rather than base-ui's
  // default first-tabbable element: a menu sheet never opens the soft
  // keyboard, so a popover sheet containing an input must not either. The
  // input is one tap away, exactly like tapping into any menu row.
  const sheetPopupRef = React.useRef<HTMLDivElement | null>(null)
  if (isMobile) {
    // The bottom-sheet presentation, shared with the dropdown menus (see
    // ui/popupSheet.ts): on a phone every popup primitive presents the same
    // way. Same Portal/Positioner/Popup parts as the desktop branch, so the
    // popover contract (finalFocus, dismissal, Escape) rides through
    // unchanged; only geometry and animation differ. The anchored align/side
    // props are accepted and ignored: the positioner still receives them, but
    // SHEET_POSITIONER_STYLE overrides the computed placement.
    return (
      <PopoverPrimitive.Portal>
        <PopoverPrimitive.Backdrop
          data-slot="popover-backdrop"
          className={SHEET_BACKDROP_CLASS}
        />
        <PopoverPrimitive.Positioner
          align={align}
          alignOffset={alignOffset}
          side={side}
          sideOffset={sideOffset}
          className="isolate z-50"
          style={SHEET_POSITIONER_STYLE}
        >
          <PopoverPrimitive.Popup
            data-slot="popover-content"
            ref={sheetPopupRef}
            initialFocus={() => sheetPopupRef.current}
            className={cn(SHEET_POPUP_CLASS, className)}
            {...props}
          />
        </PopoverPrimitive.Positioner>
      </PopoverPrimitive.Portal>
    )
  }
  return (
    <PopoverPrimitive.Portal>
      <PopoverPrimitive.Positioner
        align={align}
        alignOffset={alignOffset}
        side={side}
        sideOffset={sideOffset}
        className="isolate z-50"
      >
        <PopoverPrimitive.Popup
          data-slot="popover-content"
          // max-h-(--available-height): the positioner publishes how much
          // viewport is left on the chosen side, and capping the popup there
          // matches the dropdown menus, so a tall popover scrolls inside
          // itself (children need min-h-0 chains) instead of running off
          // screen. Short popovers are unaffected.
          className={cn(
            "z-50 flex max-h-(--available-height) w-72 origin-(--transform-origin) flex-col gap-2.5 rounded-lg bg-popover p-2.5 text-sm text-popover-foreground shadow-md ring-1 ring-foreground/10 outline-hidden duration-100 data-[side=bottom]:slide-in-from-top-2 data-[side=inline-end]:slide-in-from-left-2 data-[side=inline-start]:slide-in-from-right-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 data-open:animate-in data-open:fade-in-0 data-open:zoom-in-95 data-closed:animate-out data-closed:fade-out-0 data-closed:zoom-out-95",
            className
          )}
          {...props}
        />
      </PopoverPrimitive.Positioner>
    </PopoverPrimitive.Portal>
  )
}

function PopoverHeader({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="popover-header"
      className={cn("flex flex-col gap-0.5 text-sm", className)}
      {...props}
    />
  )
}

function PopoverTitle({ className, ...props }: PopoverPrimitive.Title.Props) {
  return (
    <PopoverPrimitive.Title
      data-slot="popover-title"
      className={cn("font-medium", className)}
      {...props}
    />
  )
}

function PopoverDescription({
  className,
  ...props
}: PopoverPrimitive.Description.Props) {
  return (
    <PopoverPrimitive.Description
      data-slot="popover-description"
      className={cn("text-muted-foreground", className)}
      {...props}
    />
  )
}

export {
  Popover,
  PopoverContent,
  PopoverDescription,
  PopoverHeader,
  PopoverTitle,
  PopoverTrigger,
}
