"use client"

import * as ResizablePrimitive from "react-resizable-panels"

import { DIVIDER_CHROME, DIVIDER_TARGET_MIN } from "@/lib/paneDivider"
import { cn } from "@/lib/utils"

function ResizablePanelGroup({
  className,
  ...props
}: ResizablePrimitive.GroupProps) {
  return (
    <ResizablePrimitive.Group
      data-slot="resizable-panel-group"
      // Handed the same grab-band minimums the sidebar's own divider reads, so
      // one number moves both dividers. See lib/paneDivider.ts.
      resizeTargetMinimumSize={DIVIDER_TARGET_MIN}
      className={cn(
        "flex h-full w-full aria-[orientation=vertical]:flex-col",
        className
      )}
      {...props}
    />
  )
}

function ResizablePanel({ ...props }: ResizablePrimitive.PanelProps) {
  return <ResizablePrimitive.Panel data-slot="resizable-panel" {...props} />
}

function ResizableHandle({
  withHandle,
  className,
  ...props
}: ResizablePrimitive.SeparatorProps & {
  withHandle?: boolean
}) {
  return (
    <ResizablePrimitive.Separator
      data-slot="resizable-handle"
      className={cn(
        "relative flex w-px items-center justify-center bg-border ring-offset-background",
        // The grab band, the resize cursor and the touch-action suppression
        // come from the shared divider chrome; the sidebar's edge wears the
        // same string. The band matches the hit region the library already
        // claims in the capture phase, so nothing new is taken from the
        // neighbouring panes; what it adds is `touch-action: none` across the
        // whole of that region instead of only over the painted line.
        DIVIDER_CHROME,
        "aria-[orientation=horizontal]:h-px aria-[orientation=horizontal]:w-full aria-[orientation=horizontal]:cursor-row-resize aria-[orientation=horizontal]:after:left-0 aria-[orientation=horizontal]:after:h-1 aria-[orientation=horizontal]:after:w-full aria-[orientation=horizontal]:after:translate-x-0 aria-[orientation=horizontal]:after:-translate-y-1/2 [&[aria-orientation=horizontal]>div]:rotate-90",
        className
      )}
      {...props}
    >
      {withHandle && (
        <div className="z-10 flex h-6 w-1 shrink-0 rounded-lg bg-border" />
      )}
    </ResizablePrimitive.Separator>
  )
}

export { ResizableHandle, ResizablePanel, ResizablePanelGroup }
