import { ResizableHandle } from "@/components/ui/resizable"

// The divider between an agent and its terminal dock: the shared 1px handle,
// which is itself the line like the Changes pane's and the sidebar's, with a
// grip pill over its middle. With the strips hidden it is all the dock shows of
// itself while folded away.
//
// The grab band is not painted. It is the handle's `::after`, grown to the
// shared minimums (10px, 20px on a coarse pointer), so the hit area is no
// smaller than when the handle was 8px tall; an 8px handle left the page
// background showing between the two terminals as a band of its own.
export function DockDivider({ disabled }: { disabled: boolean }) {
  return (
    <ResizableHandle
      disabled={disabled}
      // Starts one pixel in: the sidebar's edge line stands in the center
      // pane's first column (it is pinned 1px past the sidebar), so a line
      // from the very edge would paint over it.
      className="group/divider ml-px aria-[orientation=horizontal]:w-[calc(100%-1px)] aria-[orientation=horizontal]:after:h-2.5 pointer-coarse:aria-[orientation=horizontal]:after:h-5"
    >
      <span
        aria-hidden
        data-testid="dock-divider-grip"
        className="pointer-events-none absolute top-1/2 left-1/2 h-[5px] w-8 -translate-x-1/2 -translate-y-1/2 rounded-full bg-muted-foreground/50 group-hover/divider:bg-muted-foreground group-data-[dux-held=true]/divider:bg-muted-foreground"
      />
    </ResizableHandle>
  )
}
