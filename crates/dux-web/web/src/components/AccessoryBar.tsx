import type * as React from "react"
import { ArrowDown, ArrowLeft, ArrowRight, ArrowUp } from "lucide-react"

import { Button } from "@/components/ui/button"
import { cn } from "@/lib/utils"

// A viewport page-scroll intent emitted by the second accessory row's PgUp/PgDn
// keys. The mobile scrollbar is a slim touch target and a small drag jumps a long
// way when there is a lot of scrollback, so these buttons drive xterm's scroll
// API directly (see TerminalPane.onScroll).
export type ScrollDir = "pageUp" | "pageDown"

// The mobile soft keyboard can't produce Esc, Tab, Ctrl-chords, a Shift-Enter
// soft newline, cursor arrows, or a usable way to page through output, which a
// terminal needs constantly. The accessory bar supplies them as two fixed rows of
// touch targets directly above the on-screen keyboard: row one is the modifier /
// special keys (Esc, Tab, Ctrl, Alt, and the ⇧↵ newline), row two is navigation
// (the four cursor arrows plus PgUp/PgDn page scrolling).
//
// Presentational only: this component decides layout and emits intents. All
// behavior (which byte sequence to send, cursor-key mode, one-shot latch
// clearing, viewport scrolling) lives in TerminalPane + lib/termkeys.

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
  // THE TYPING-SURFACE QUICK TOGGLE. A person with a keyboard case attached
  // wants to type straight into the terminal; the same tablet without it wants
  // the buffered message box, and the browser cannot tell the two apart
  // (measured), so the user swaps it. It sits in THIS row because this row is
  // where the thumb already is in both states, and it is one tap rather than
  // two. It is not the guaranteed way back any more: the input ⋯ menu is, and
  // that menu renders in every bar state including bars-all-hidden. Both write
  // through the same `setTypingSurface` helper, so they cannot drift. Absent
  // (undefined) where the toggle would change nothing, which is every case
  // except the `auto` setting on a touch device.
  composeSurface?: boolean
  onToggleSurface?: () => void
  // The input ⋯ menu, when THIS bar is the bottom-most input row (the message
  // box is off, so the compose row that normally carries it is not there).
  // Presentational like everything else here: the parent owns the anchor
  // matrix and simply hands over a node, absent whenever another row carries
  // the menu or the menu would be empty.
  inputMenu?: React.ReactNode
}

// CRITICAL: every bar button calls preventDefault() on pointerdown so the press
// can't shift focus off the active typing surface before the handler runs, and
// we fire on pointerdown (not click) for a snappy, latency-free feel. Because
// the press never takes focus, a tap PRESERVES the soft-keyboard state,
// whichever it was: a user typing keeps the keyboard (and their focus), and a
// user paging through output with the keyboard closed does not have it pop
// open — the parent's handlers only refocus when the typing surface had focus
// at tap time. The PgUp/PgDn page-scroll keys reuse the same handler for that
// focus/sequencing guarantee but then deliberately blur in
// TerminalPane.onScroll to dismiss the keyboard for reading. So "preserves the
// keyboard" is the input-key contract, not a universal one.
function keyDown(handler: () => void) {
  return (event: React.PointerEvent) => {
    event.preventDefault()
    handler()
  }
}

// Keyboard/AT activation, the same pattern as the compose bar's Send button:
// Enter or Space on the focused button fires a `click` with `detail === 0` (no
// pointer press). A click that FOLLOWS a real pointer tap carries `detail >= 1`
// and is ignored, because the pointerdown handler above already fired; without
// the detail gate every tap would fire the key twice.
function keyClick(handler: () => void) {
  return (event: React.MouseEvent) => {
    if (event.detail === 0) handler()
  }
}

// The soft-newline key's glyph. A bare "⇧↵" text label rendered unevenly across
// fonts/platforms, so we draw it: a filled shift up-arrow next to a return arrow,
// stroked in currentColor to match the lucide icons on the neighboring keys.
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

// One key cell. flex-1 makes the cells evenly fill the row; h-10 (40px) is a
// comfortable thumb target while keeping the two-row bar from eating the phone's
// scarce vertical space. Text labels are font-mono so Esc/Tab/Ctrl/Alt read like
// keycaps; arrow and newline cells pass an icon child instead.
function KeyButton({
  label,
  ariaLabel,
  pressed,
  onActivate,
  children,
  className,
}: {
  label?: string
  ariaLabel?: string
  pressed?: boolean
  // The key's intent, fired once per activation: on pointerdown for a real
  // press (focus-preserving, see `keyDown`) and on a detail-0 click for
  // keyboard/AT activation (see `keyClick`).
  onActivate: () => void
  children?: React.ReactNode
  // Extra classes merged last so a caller can loosen the min-w-0 floor (the
  // surface toggle needs a wider one; see its call site).
  className?: string
}) {
  return (
    <Button
      variant="secondary"
      aria-label={ariaLabel ?? label}
      aria-pressed={pressed}
      onPointerDown={keyDown(onActivate)}
      onClick={keyClick(onActivate)}
      className={cn(
        "h-10 min-w-0 flex-1 font-mono",
        className,
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
  composeSurface,
  onToggleSurface,
  inputMenu,
}: AccessoryBarProps) {
  // Two flex rows stacked: modifier/special keys on top, navigation (arrows +
  // page scroll) below; gap-1.5 between the rows so a fat-finger tap on the top
  // row doesn't catch the row directly beneath it. Safe-area insets are NOT
  // applied here: the mobile root pads its own bottom (clearing the home
  // indicator), handled by an ancestor (see App.tsx mobile root).
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
        {/* The typing-surface toggle (see the prop). It NAMES the state it is
            in, "Box" while the buffered message box is the typing surface and
            "Direct" while keystrokes go straight to the terminal, with the
            full sentence on the aria-label for anyone who cannot see the word.
            TEXT ONLY, and on the SHORTER row, both for room rather than taste:
            MEASURED at 390px, an eighth cell on the navigation row pushed the
            key off the screen edge, and an icon beside the word costs another
            22px the cell does not have. It sits behind its own divider,
            because changing the typing surface out from under you is a far
            bigger consequence than a mistap on a cursor key, and that is the
            misclick-safe spacing the navigation row already uses. */}
        {onToggleSurface ? (
          <>
            <div
              aria-hidden="true"
              className="mx-1.5 w-px shrink-0 self-stretch bg-border"
            />
            <KeyButton
              label={composeSurface ? "Box" : "Direct"}
              ariaLabel={
                composeSurface
                  ? "Typing surface: message box. Switch to typing directly."
                  : "Typing surface: direct. Switch to the message box."
              }
              onActivate={onToggleSurface}
              // min-w-16: with the input ⋯ cell in the row, an even flex split
              // at 390px gives every cell 47px, and the "Direct" label needs 54
              // (measured in the preview container; it visibly clipped). The
              // floor holds this cell at 64 and the five keys settle at 44,
              // still above the 40px touch floor on both axes.
              className="min-w-16"
            />

          </>
        ) : null}
        {/* THE INPUT ⋯, when this row is the bottom-most input row. It sits
            behind its own divider for the same misclick reason the surface
            toggle does: opening a menu out from under a thumb aiming for ⇧↵
            is a different kind of surprise from a mistyped key.

            TOUCH FLOOR, per axis: the trigger keeps `size-10` on BOTH axes
            (40px square), so nothing here is an exemption. WIDTH BUDGET: row
            one is budgeted for 390px; MEASURED in the preview container at
            that width with the "Direct" label up (the tightest state the row
            has): the label clipped under an even flex split, so the surface
            toggle carries a min-width floor (see it above) and every cell
            stays at or above the 40px touch floor. */}
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
