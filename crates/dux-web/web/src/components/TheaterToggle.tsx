import { Maximize2, Minimize2 } from "lucide-react"
import { useRef } from "react"

import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
import { useTheaterToggleFocusWhen } from "@/hooks/use-theater"
import { toggleTheater, useDux } from "@/lib/store"

// The computer pane header's trigger for theater mode, and its way IN while the
// names leave it room. It says the word "Theater", the name every other surface
// uses, and its label does not flip with a state that is never on screen here,
// though `aria-label` still carries it. It takes focus back when an exit it did
// not press brings it back on screen, once the header has `settled` on keeping
// it there: a toggle that is about to fold into the menu must not take the focus
// only to drop it.
export function TheaterToggle({ settled = true }: { settled?: boolean }) {
  const { theater, selectedTarget } = useDux()
  const ref = useRef<HTMLButtonElement | null>(null)
  useTheaterToggleFocusWhen(ref, !theater && settled)
  if (!selectedTarget) return null
  const label = theater ? "Leave theater mode" : "Theater mode"
  return (
    <SimpleTooltip content={label}>
      <Button
        ref={ref}
        variant="outline"
        // `default` rather than `icon`: the same height as the icon-only
        // controls beside it, so the label changes only the width. A finger
        // gets the 40px touch height, as does the rest of the header's cluster.
        size="default"
        className="shrink-0 pointer-coarse:h-10"
        aria-label={label}
        aria-pressed={theater}
        onClick={() => toggleTheater()}
      >
        {theater ? <Minimize2 /> : <Maximize2 />}
        Theater
      </Button>
    </SimpleTooltip>
  )
}
