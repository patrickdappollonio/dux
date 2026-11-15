import { ResizableHandle } from "@/components/ui/resizable"

// The divider between an agent and its terminal dock: an 8px grab band that
// paints nothing itself, carrying a 1px line and a centred grip pill. With the
// strips hidden it is all the dock shows of itself while folded away.
//
// The band takes the shared grab behaviour (stacking, cursor, touch handling,
// focus ring) from the Changes divider's handle, but not its paint: a filled
// 8px band read as a grey slab rather than a border, and its first column
// covered the sidebar's edge line. So the element is transparent in every state
// and the line and grip light up on hover and while held instead.
export function DockDivider({ disabled }: { disabled: boolean }) {
  return (
    <ResizableHandle
      disabled={disabled}
      className="group/divider bg-transparent hover:bg-transparent data-[dux-held=true]:bg-transparent aria-[orientation=horizontal]:h-2"
    >
      {/* Starts one pixel in: the sidebar's edge line stands in the center
        * pane's first column (it is pinned 1px past the sidebar), so a line
        * from the very edge would paint over it. */}
      <span
        aria-hidden
        data-testid="dock-divider-line"
        className="pointer-events-none absolute top-1/2 right-0 left-px h-px bg-border group-hover/divider:bg-ring group-data-[dux-held=true]/divider:bg-ring"
      />
      {/* 2px to 7px of the 8px band, so its middle is the line's middle. */}
      <span
        aria-hidden
        data-testid="dock-divider-grip"
        className="pointer-events-none absolute top-0.5 left-1/2 h-[5px] w-8 -translate-x-1/2 rounded-full bg-muted-foreground/50 group-hover/divider:bg-muted-foreground group-data-[dux-held=true]/divider:bg-muted-foreground"
      />
    </ResizableHandle>
  )
}
