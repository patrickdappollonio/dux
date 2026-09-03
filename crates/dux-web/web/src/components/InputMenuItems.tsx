import {
  Keyboard,
  KeyboardOff,
  MessageSquare,
  Minimize2,
  Paperclip,
  SquareTerminal,
} from "lucide-react"

import { DropdownMenuItem } from "@/components/ui/dropdown-menu"
import {
  exitTheater,
  mobileAccessoryBarVisible,
  setAccessoryBarVisibility,
  useDux,
} from "@/lib/store"
import type { InputMenuGates } from "@/lib/inputMenu"
import { hideTerminalKeysHint, switchTypingSurface } from "@/lib/typingSurface"

// THE INPUT ITEMS, shared by every menu that carries any of them: the input
// `⋯` inside the virtual input (see `InputMenu`) and the INPUT group at the top
// of whichever menu the surface always has (see `PaneInputGroup`, which the
// phone's merged pane menu, the phone's agentless terminal header, the sidebar
// row menus and the floating pill all render). One component so the labels,
// icons and store writes can never drift between them.
//
// VISIBILITY IS THE CALLER'S, deliberately: this component self-gates nothing.
// The same item belongs on different predicates depending on where the menu is
// anchored (the header menus are inside the phone shell, the input menu also
// serves a coarse-pointer tablet in the desktop shell), and a component that
// answered that for itself is exactly how the widened keys gate would have been
// silently undone. Callers compute an `InputMenuGates` and pass it.
//
// Neutral color and no trailing ellipsis on the view toggles: they act
// immediately (an optimistic override plus the generic settings PATCH), no
// dialog and nothing destructive. "Attach a file…" does carry one, because it
// opens the operating system's picker dialog.

export function InputMenuItems({
  gates = { surfaceSwitch: false, keysToggle: false },
  attach = false,
  theaterExit = false,
  onAttach,
  composeSurface = false,
  directLeavesNothingBelow = true,
  keysHideLeavesNothingBelow = false,
}: {
  /// The two rows either menu can carry. Defaulted off for the callers whose
  /// menu carries neither, which is every caller of the theater exit.
  gates?: InputMenuGates
  /// "Attach a file…", which lives in the top menu's INPUT group and nowhere
  /// else. Off when uploads are switched off server-side
  /// (`file_drop_max_bytes = 0`) and for anyone who does not own the input: a
  /// non-owner cannot paste the saved path afterwards, so the file would
  /// strand. It is a prop rather than a gate because the caller that sets it is
  /// also the one holding `onAttach`, and the two are the same fact.
  attach?: boolean
  /// "Leave theater mode". A way BACK and not a way there: entering is the
  /// header's expand button, and in theater that header is exactly what is not
  /// on screen. Only the top menus pass it, for the same reason: the bottom
  /// `⋯` lives inside the virtual input, so it is nobody's guaranteed exit.
  theaterExit?: boolean
  /// Opens the file picker. Called synchronously from the item's click, so the
  /// browser's user activation still covers the `.click()` on the hidden input.
  onAttach?: () => void
  /// Which typing surface is live right now: `true` while the buffered message
  /// box is up, `false` while keystrokes go straight to the terminal. Only read
  /// when `gates.surfaceSwitch` is set.
  composeSurface?: boolean
  /// Would switching to direct typing leave NOTHING under the terminal? It
  /// decides whether the one-time "here is the way back" hint fires, so only
  /// the menu that can flip that way has to answer it. Defaults to the loud
  /// answer for the top menu, which offers the opposite direction only and can
  /// therefore never reach the hint at all.
  directLeavesNothingBelow?: boolean
  /// Would HIDING THE TERMINAL KEYS leave nothing under the terminal? The other
  /// door out of the virtual input, and the same one-time hint: from direct
  /// typing with only the key row down, hiding it takes the bottom `⋯` with it.
  /// Defaults to the quiet answer for the top menu, whose keys item can only
  /// ever read "Show terminal keys" (it is offered exactly while no bottom row
  /// is up, which is exactly when the keys are already hidden).
  keysHideLeavesNothingBelow?: boolean
}) {
  const duxState = useDux()
  const accessoryBarVisible = mobileAccessoryBarVisible(duxState)
  return (
    <>
      {attach ? (
        <DropdownMenuItem onClick={() => onAttach?.()}>
          <Paperclip />
          Attach a file…
        </DropdownMenuItem>
      ) : null}
      {/* NAMED FOR WHAT IT DOES, not for the state it is in. A menu row is a
          sentence, and "Use virtual input" says what tapping it gets you. The
          bottom `⋯` carries BOTH directions, because it exists for as long as
          any row under the terminal does; the top menu carries the way back
          alone, for the pane that has no row left to hold one. Both write
          through the one `switchTypingSurface`, so they cannot drift. */}
      {gates.surfaceSwitch ? (
        <DropdownMenuItem
          onClick={() =>
            switchTypingSurface(
              composeSurface ? "direct" : "compose",
              directLeavesNothingBelow,
            )
          }
        >
          {composeSurface ? <SquareTerminal /> : <MessageSquare />}
          {composeSurface ? "Type directly in the terminal" : "Use virtual input"}
        </DropdownMenuItem>
      ) : null}
      {gates.keysToggle ? (
        <DropdownMenuItem
          onClick={() => {
            void setAccessoryBarVisibility(!accessoryBarVisible)
            // HIDING THE KEYS IS THE OTHER WAY OUT of the virtual input, so it
            // owes the same one-time signpost the surface switch does: from
            // direct typing this row is the whole bottom bar, and the `⋯` that
            // carries the way back hangs off it. Only ever raised on the way
            // down, and only where nothing is left below.
            if (accessoryBarVisible) {
              hideTerminalKeysHint(keysHideLeavesNothingBelow)
            }
          }}
        >
          {accessoryBarVisible ? <KeyboardOff /> : <Keyboard />}
          {accessoryBarVisible ? "Hide terminal keys" : "Show terminal keys"}
        </DropdownMenuItem>
      ) : null}
      {/* The guaranteed way out of theater. It is a way BACK only, so there is
          no matching "Enter theater mode": entering is the header's expand
          button, and this menu exists precisely for the state where that header
          is not on screen. Same two-arrow glyph as the button it undoes. */}
      {theaterExit ? (
        <DropdownMenuItem onClick={() => exitTheater()}>
          <Minimize2 />
          Leave theater mode
        </DropdownMenuItem>
      ) : null}
    </>
  )
}
