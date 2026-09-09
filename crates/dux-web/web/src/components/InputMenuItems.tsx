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

// The input items, shared by every menu that carries any of them, so the labels,
// icons and store writes cannot drift. VISIBILITY IS THE CALLER'S: the same item
// belongs on different predicates depending on where the menu is anchored, so
// callers compute an `InputMenuGates` and this component self-gates nothing. The
// view toggles act immediately, so they are neutral and carry no ellipsis; "Attach
// a file…" carries one, because it opens the operating system's picker.

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
  /// "Attach a file…", in the top menu's INPUT group and nowhere else. Off when
  /// uploads are off server-side, and for a non-owner, who could not paste the
  /// saved path afterwards. A prop rather than a gate: it is `onAttach`'s own fact.
  attach?: boolean
  /// "Leave theater mode": a way back, never a way there. Only the top menus pass
  /// it, because the bottom `⋯` lives inside the virtual input and can leave.
  theaterExit?: boolean
  /// Opens the file picker. Called synchronously from the item's click, so the
  /// browser's user activation still covers the `.click()` on the hidden input.
  onAttach?: () => void
  /// Which typing surface is live: `true` while the buffered message box is up.
  /// Only read when `gates.surfaceSwitch` is set.
  composeSurface?: boolean
  /// Would switching to direct typing leave NOTHING under the terminal? It decides
  /// whether the one-time way-back hint fires, so only the menu that can flip that
  /// way answers it; the top menu offers the opposite direction and never reaches it.
  directLeavesNothingBelow?: boolean
  /// Would hiding the terminal keys leave nothing under the terminal? The other
  /// door out of the virtual input, owing the same one-time hint. Defaults to the
  /// quiet answer, which is the only one the top menu can reach.
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
            // Hiding the keys is the other way out of the virtual input, so it owes
            // the same one-time signpost, raised only on the way down and only where
            // nothing is left below.
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
