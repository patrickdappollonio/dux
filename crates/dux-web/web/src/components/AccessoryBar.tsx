import type * as React from "react"
import { ArrowDown, ArrowLeft, ArrowRight, ArrowUp } from "lucide-react"

import { Button } from "@/components/ui/button"
import { DUX_TERMINAL_FONT_STACK } from "@/lib/terminalFont"
import { cn } from "@/lib/utils"

// A viewport page-scroll intent from the PgUp/PgDn keys, driving xterm's scroll
// API directly: the mobile scrollbar is a slim target that jumps a long way.
export type ScrollDir = "pageUp" | "pageDown"

// Two rows of touch targets for the keys a soft keyboard cannot produce at all.
// Presentational only: it decides layout and emits intents, while every behavior
// behind them lives in TerminalPane and lib/termkeys.

interface AccessoryBarProps {
  // Fire-and-forget key intents. The parent maps these to PTY byte sequences,
  // applying any latched Alt prefix and consulting cursor-key mode for arrows.
  onEsc: () => void
  onTab: () => void
  // Insert a soft newline (LF / Ctrl-j) — the touch equivalent of Shift-Enter,
  // which no soft keyboard can produce.
  onNewline: () => void
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
  // The input ⋯ menu, when THIS bar is the bottom-most input row. The parent owns
  // the anchor matrix and hands over a node, absent when another row carries it.
  inputMenu?: React.ReactNode
}

// Every bar button preventDefaults on pointerdown, so the press never shifts focus
// off the typing surface and a tap preserves the soft-keyboard state as it was.
// That contract is the input keys'; the page-scroll keys share the handler and then
// blur deliberately, to dismiss the keyboard for reading.
function keyDown(handler: () => void) {
  return (event: React.PointerEvent) => {
    event.preventDefault()
    handler()
  }
}

// Keyboard activation: Enter or Space fires a click with `detail === 0`. A click
// following a real tap carries `detail >= 1` and is ignored, or the key fires twice.
function keyClick(handler: () => void) {
  return (event: React.MouseEvent) => {
    if (event.detail === 0) handler()
  }
}

// The soft-newline key's glyph, drawn rather than typed: a bare "⇧↵" label renders
// unevenly across platforms. Stroked in currentColor to match the lucide icons.
function ShiftEnterIcon() {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {/* shift: an up arrow (closed outline) */}
      <path d="M6 3 L2 8 H4 V13 H8 V8 H10 Z" />
      {/* return: shaft dropping down and hooking left, with an arrowhead */}
      <path d="M22 5 V12 H13" />
      <path d="M16 9 L13 12 L16 15" />
    </svg>
  )
}

// One key cell, at the 40px thumb-target floor. Text labels take the BUNDLED
// terminal stack, deliberately not the user's configured family: these caps are
// dux's own chrome rather than terminal content.
function KeyButton({
  label,
  ariaLabel,
  pressed,
  onActivate,
  children,
}: {
  label?: string
  ariaLabel?: string
  pressed?: boolean
  // The key's intent, fired once per activation: on pointerdown for a real press,
  // and on a detail-0 click for keyboard activation.
  onActivate: () => void
  children?: React.ReactNode
}) {
  return (
    <Button
      variant="secondary"
      aria-label={ariaLabel ?? label}
      aria-pressed={pressed}
      onPointerDown={keyDown(onActivate)}
      onClick={keyClick(onActivate)}
      style={{ fontFamily: DUX_TERMINAL_FONT_STACK }}
      className={cn(
        "h-10 min-w-0 flex-1",
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
  onNewline,
  onArrow,
  onScroll,
  ctrl,
  alt,
  onToggleCtrl,
  onToggleAlt,
  inputMenu,
}: AccessoryBarProps) {
  // Two stacked rows, gapped so a fat-finger tap on one does not catch the other.
  // Safe-area insets are NOT applied here: the mobile root pads its own bottom.
  return (
    <div className="flex shrink-0 flex-col gap-1.5 border-t bg-background px-1 py-1">
      {/* Row one — modifier / special keys sent to the program. */}
      <div className="flex items-center gap-1">
        <KeyButton label="Esc" onActivate={onEsc} />
        <KeyButton label="Tab" onActivate={onTab} />
        <KeyButton
          label="Ctrl"
          pressed={ctrl}
          onActivate={onToggleCtrl}
        />
        <KeyButton
          label="Alt"
          pressed={alt}
          onActivate={onToggleAlt}
        />
        <KeyButton ariaLabel="Insert newline" onActivate={onNewline}>
          <ShiftEnterIcon />
        </KeyButton>
        {/* THE INPUT ⋯, when this row is the bottom-most input row. It sits
            behind its own divider on the misclick-safe-spacing rule: opening a
            menu out from under a thumb aiming for ⇧↵ is a different kind of
            surprise from a mistyped key, and the divider's own margins are the
            clear space that makes the two cells hard to confuse.

            TOUCH FLOOR, per axis: the trigger keeps `size-10` on BOTH axes
            (40px square), so nothing here is an exemption. Nothing in the row
            carries a min-width floor any more: the only cell that ever needed
            one was the "Box"/"Direct" typing-surface cap, whose label did not
            fit an even flex split, and that control is gone from the row (the
            same switch lives in this very menu, which says what it does). The
            five keys that remain are short mono labels and icons sharing what
            the trigger leaves, floored at the 40px touch target by `h-10` on
            the cross axis. */}
        {inputMenu ? (
          <>
            <div
              aria-hidden="true"
              className="mx-1.5 w-px shrink-0 self-stretch bg-border"
            />
            {inputMenu}
          </>
        ) : null}
      </div>
      {/* Row two — navigation. The four cursor arrows (sent to the program, keep
          focus) and PgUp/PgDn (scroll the xterm viewport, blur to dismiss the
          keyboard; see onScroll) do OPPOSITE things to focus, so a divider with
          breathing room separates the two clusters — a mistap on PgUp while
          aiming for → would otherwise yank the keyboard away (misclick-safe
          spacing, per the CLAUDE.md tenet). */}
      <div className="flex items-center gap-1">
        <KeyButton ariaLabel="Left" onActivate={() => onArrow("left")}>
          <ArrowLeft />
        </KeyButton>
        <KeyButton ariaLabel="Down" onActivate={() => onArrow("down")}>
          <ArrowDown />
        </KeyButton>
        <KeyButton ariaLabel="Up" onActivate={() => onArrow("up")}>
          <ArrowUp />
        </KeyButton>
        <KeyButton
          ariaLabel="Right"
          onActivate={() => onArrow("right")}
        >
          <ArrowRight />
        </KeyButton>
        <div
          aria-hidden="true"
          className="mx-1.5 w-px shrink-0 self-stretch bg-border"
        />
        <KeyButton
          label="PgUp"
          ariaLabel="Page up"
          onActivate={() => onScroll("pageUp")}
        />
        <KeyButton
          label="PgDn"
          ariaLabel="Page down"
          onActivate={() => onScroll("pageDown")}
        />
      </div>
    </div>
  )
}
