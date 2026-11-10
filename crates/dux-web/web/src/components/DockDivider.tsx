import { ResizableHandle } from "@/components/ui/resizable"

// The divider between an agent and its terminal dock: the app's grip handle
// turned horizontal, an 8px band, wearing the shared divider chrome so it
// grabs and paints like the other dividers. With the strips hidden it is all
// the dock shows of itself while folded away.
export function DockDivider({ disabled }: { disabled: boolean }) {
  return (
    <ResizableHandle
      withHandle
      disabled={disabled}
      className="aria-[orientation=horizontal]:h-2"
    />
  )
}
