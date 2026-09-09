import { Maximize2, Minimize2 } from "lucide-react"
import { useRef } from "react"

import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
import { useTheaterToggleFocus } from "@/hooks/use-theater"
import { toggleTheater, useDux } from "@/lib/store"

// The primary trigger for theater mode, and the only way IN. Each size takes the
// shared height token of the header it sits in; the desktop one says the word
// "Theater", the name every other surface uses, and its label does not flip with a
// state that is never on screen here, though `aria-label` still carries it. It
// takes focus back when an exit it did not press brings it back on screen.
export function TheaterToggle({
  size = "desktop",
}: {
  size?: "desktop" | "mobile"
}) {
  const { theater, selectedTarget } = useDux()
  const ref = useRef<HTMLButtonElement | null>(null)
  useTheaterToggleFocus(ref, theater)
  if (!selectedTarget) return null
  const label = theater ? "Leave theater mode" : "Theater mode"
  const mobile = size === "mobile"
  return (
    <SimpleTooltip content={label}>
      <Button
        ref={ref}
        variant="outline"
        // `default` rather than `icon` on a computer: the same height as the
        // icon-only controls beside it, so the label changes only the width.
        size={mobile ? "lg" : "default"}
        className={mobile ? "min-w-11 shrink-0" : "shrink-0"}
        aria-label={label}
        aria-pressed={theater}
        onClick={() => toggleTheater()}
      >
        {theater ? <Minimize2 /> : <Maximize2 />}
        {mobile ? null : "Theater"}
      </Button>
    </SimpleTooltip>
  )
}
