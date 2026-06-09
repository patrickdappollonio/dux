import type * as React from "react"
import {
  ArrowDown,
  ArrowLeft,
  ArrowRight,
  ArrowUp,
  ChevronsDown,
  ChevronsUp,
} from "lucide-react"

import { Button } from "@/components/ui/button"
import { cn } from "@/lib/utils"

// A viewport scroll intent emitted by the second accessory row. The mobile
// scrollbar is a slim touch target and a small drag jumps a long way when there
// is a lot of scrollback, so these buttons drive xterm's scroll API directly
// (see TerminalPane.onScroll).
export type ScrollDir = "top" | "pageUp" | "pageDown" | "bottom"

// The mobile soft keyboard can't produce Esc, Tab, Ctrl-chords, arrow keys, or
// a usable way to scroll back through output, which a terminal needs
// constantly. The accessory bar supplies them as fixed rows of touch targets
// that sit directly above the on-screen keyboard: row one is keys, row two is
// scroll controls.
//
// Presentational only: this component decides layout and emits intents. All
// behavior (which byte sequence to send, cursor-key mode, one-shot latch
// clearing, viewport scrolling) lives in TerminalPane + lib/termkeys.

interface AccessoryBarProps {
  // Fire-and-forget key intents. The parent maps these to PTY byte sequences,
  // applying any latched Alt prefix and consulting cursor-key mode for arrows.
  onEsc: () => void
  onTab: () => void
  onArrow: (dir: "up" | "down" | "left" | "right") => void
  // Viewport scroll intents for the second row. The parent scrolls the xterm
  // viewport (not the PTY) so the user can read back without the scrollbar.
  onScroll: (dir: ScrollDir) => void
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
  onScroll,
  ctrl,
  alt,
  onToggleCtrl,
  onToggleAlt,
}: AccessoryBarProps) {
  // Two flex rows stacked: keys on top, scroll controls below; gap-1.5 between
  // the rows so a fat-finger tap on a key row doesn't catch the scroll row
  // directly beneath it. Safe-area insets are NOT applied here: in normal layout
  // the status bar sits below this bar (so it isn't the screen's bottom edge),
  // and in fullscreen the enclosing column pads its own bottom — both handled by
  // ancestors (see App.tsx mobile root and TerminalPane's fullscreen column).
  return (
    <div className="flex shrink-0 flex-col gap-1.5 border-t bg-background px-1 py-1">
      <div className="flex items-center gap-1">
        <KeyButton label="Esc" onPointerDown={keyDown(onEsc)} />
        <KeyButton label="Tab" onPointerDown={keyDown(onTab)} />
        <KeyButton
          label="Ctrl"
          pressed={ctrl}
          onPointerDown={keyDown(onToggleCtrl)}
        />
        <KeyButton
          label="Alt"
          pressed={alt}
          onPointerDown={keyDown(onToggleAlt)}
        />
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
      {/* Scroll controls: jump to top, page up/down, jump to the latest output.
          Extremes are icons; page steps are keycap labels mirroring PgUp/PgDn. */}
      <div className="flex items-center gap-1">
        <KeyButton
          ariaLabel="Scroll to top"
          onPointerDown={keyDown(() => onScroll("top"))}
        >
          <ChevronsUp />
        </KeyButton>
        <KeyButton
          label="PgUp"
          ariaLabel="Page up"
          onPointerDown={keyDown(() => onScroll("pageUp"))}
        />
        <KeyButton
          label="PgDn"
          ariaLabel="Page down"
          onPointerDown={keyDown(() => onScroll("pageDown"))}
        />
        <KeyButton
          ariaLabel="Scroll to latest"
          onPointerDown={keyDown(() => onScroll("bottom"))}
        >
          <ChevronsDown />
        </KeyButton>
      </div>
    </div>
  )
}
