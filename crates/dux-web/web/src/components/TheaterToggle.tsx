import { Maximize2, Minimize2 } from "lucide-react"
import { useRef } from "react"

import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
import { useTheaterToggleFocus } from "@/hooks/use-theater"
import { toggleTheater, useDux } from "@/lib/store"

// THE PRIMARY TRIGGER for theater mode: an icon button in the pane header,
// beside the macros trigger, flipping between lucide `maximize-2` and
// `minimize-2`. It is the one control that is visible BEFORE you need it,
// which is what a mode you have never used requires; the pill's exit and the
// input `⋯` item are ways back, not ways there.
//
// Deliberately not a double tap on the terminal (that gesture is already spoken
// for four times over: focusing the compose box, a forwarded mouse report, a
// long-press selection, and an OSC 8 link), and deliberately not a keyboard
// shortcut (the web has no palette and no global hotkeys, and any chord would
// have to survive being forwarded to the PTY).
//
// TWO SIZES, one component. On the desktop header it takes the cluster's shared
// `h-8` token, because a control's height is set with its neighbours rather
// than on its own; on the phone header it takes the same `lg` (36px tall) plus
// `min-w-11` treatment every other control in that header has, whose per-axis
// justification lives in MobileShell's header.
//
// It also takes focus back when an exit that was not its own press brings it
// back on screen (the pill's button, or Escape), so a keyboard is left on the
// control that undoes what just happened rather than on the document body.
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
  return (
    <SimpleTooltip content={label}>
      <Button
        ref={ref}
        variant="outline"
        size={size === "mobile" ? "lg" : "icon"}
        className={size === "mobile" ? "min-w-11 shrink-0" : "shrink-0"}
        aria-label={label}
        aria-pressed={theater}
        onClick={() => toggleTheater()}
      >
        {theater ? <Minimize2 /> : <Maximize2 />}
      </Button>
    </SimpleTooltip>
  )
}
