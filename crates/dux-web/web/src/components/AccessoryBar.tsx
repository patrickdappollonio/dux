import type * as React from "react"
import { ArrowDown, ArrowLeft, ArrowRight, ArrowUp } from "lucide-react"

import { Button } from "@/components/ui/button"
import { cn } from "@/lib/utils"

// The mobile soft keyboard can't produce Esc, Tab, Ctrl-chords, or arrow keys,
// which a terminal needs constantly. The accessory bar supplies them as a fixed
// row of touch targets that sit directly above the on-screen keyboard.
//
// Presentational only: this component decides layout and emits intents. All
// behavior (which byte sequence to send, cursor-key mode, one-shot latch
// clearing) lives in TerminalPane + lib/termkeys.

interface AccessoryBarProps {
  // Fire-and-forget key intents. The parent maps these to PTY byte sequences,
  // applying any latched Alt prefix and consulting cursor-key mode for arrows.
  onEsc: () => void
  onTab: () => void
  onArrow: (dir: "up" | "down" | "left" | "right") => void
  // Sticky modifier latches and their toggles. The bar reflects the latched
  // state; the parent owns it and clears it one-shot after the next keystroke.
  ctrl: boolean
  alt: boolean
  onToggleCtrl: () => void
  onToggleAlt: () => void
}

// CRITICAL: every bar button must call preventDefault() on pointerdown so the
// xterm hidden textarea keeps focus and the soft keyboard stays open. We also
// fire the action on pointerdown (not click) for snappy, latency-free feel.
function keyDown(handler: () => void) {
  return (event: React.PointerEvent) => {
    event.preventDefault()
    handler()
  }
}

// One key cell. flex-1 makes the cells evenly fill the row; h-11 (44px) meets
// the touch-target minimum. Text labels are font-mono so Esc/Tab/Ctrl/Alt read
// like keycaps; arrow cells pass an icon child instead.
function KeyButton({
  label,
  ariaLabel,
  pressed,
  onPointerDown,
  children,
}: {
  label?: string
  ariaLabel?: string
  pressed?: boolean
  onPointerDown: (event: React.PointerEvent) => void
  children?: React.ReactNode
}) {
  return (
    <Button
      variant="secondary"
      aria-label={ariaLabel ?? label}
      aria-pressed={pressed}
      onPointerDown={onPointerDown}
      className={cn(
        "h-11 min-w-0 flex-1 font-mono",
        // Latched modifiers get an accent fill so the active state is
        // unmistakable on a glance — accent tokens, never raw colors.
        pressed && "bg-primary text-primary-foreground hover:bg-primary/80",
      )}
    >
      {children ?? label}
    </Button>
  )
}

export function AccessoryBar({
  onEsc,
  onTab,
  onArrow,
  ctrl,
  alt,
  onToggleCtrl,
  onToggleAlt,
}: AccessoryBarProps) {
  // A plain flex row. Keeping it a single flexible strip means future keys can
  // extend it into a horizontally scrollable bar without restructuring.
  return (
    <div className="flex shrink-0 items-center gap-1 border-t bg-background px-1 py-1">
      <KeyButton label="Esc" onPointerDown={keyDown(onEsc)} />
      <KeyButton label="Tab" onPointerDown={keyDown(onTab)} />
      <KeyButton
        label="Ctrl"
        pressed={ctrl}
        onPointerDown={keyDown(onToggleCtrl)}
      />
      <KeyButton label="Alt" pressed={alt} onPointerDown={keyDown(onToggleAlt)} />
      <KeyButton ariaLabel="Left" onPointerDown={keyDown(() => onArrow("left"))}>
        <ArrowLeft />
      </KeyButton>
      <KeyButton ariaLabel="Down" onPointerDown={keyDown(() => onArrow("down"))}>
        <ArrowDown />
      </KeyButton>
      <KeyButton ariaLabel="Up" onPointerDown={keyDown(() => onArrow("up"))}>
        <ArrowUp />
      </KeyButton>
      <KeyButton
        ariaLabel="Right"
        onPointerDown={keyDown(() => onArrow("right"))}
      >
        <ArrowRight />
      </KeyButton>
    </div>
  )
}
