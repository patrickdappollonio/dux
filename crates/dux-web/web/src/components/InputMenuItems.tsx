import {
  Keyboard,
  KeyboardOff,
  MessageSquare,
  Minimize2,
  Paperclip,
  SquareTerminal,
} from "lucide-react"

import {
  DropdownMenuItem,
  DropdownMenuSeparator,
} from "@/components/ui/dropdown-menu"
import {
  exitTheater,
  mobileAccessoryBarVisible,
  setAccessoryBarVisibility,
  useDux,
} from "@/lib/store"
import {
  inputMenuHasItems,
  type InputMenuGates,
} from "@/lib/inputMenu"
import { switchTypingSurface } from "@/lib/typingSurface"

// THE INPUT ITEMS, shared by every menu that carries any of them: the input
// `⋯` inside the virtual input (see `InputMenu`) and the INPUT group at the top
// of whichever menu the surface always has (see `PaneInputGroup`, which the
// phone's merged pane menu, the phone's agentless terminal header, the desktop
// pane header's menu and the floating pill all render). One component so the
// labels, icons and store writes can never drift between them.
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
  gates,
  onAttach,
  composeSurface = false,
  trailingSeparator = false,
}: {
  gates: InputMenuGates
  /// Opens the file picker. Called synchronously from the item's click, so the
  /// browser's user activation still covers the `.click()` on the hidden input.
  onAttach?: () => void
  /// Which typing surface is live right now: `true` while the buffered message
  /// box is up, `false` while keystrokes go straight to the terminal. Only read
  /// when `gates.surfaceSwitch` is set.
  composeSurface?: boolean
  /// A separator AFTER the items, for the header menus that continue with their
  /// own entries below.
  trailingSeparator?: boolean
}) {
  const duxState = useDux()
  const accessoryBarVisible = mobileAccessoryBarVisible(duxState)
  return (
    <>
      {gates.attach ? (
        <DropdownMenuItem onClick={() => onAttach?.()}>
          <Paperclip />
          Attach a file…
        </DropdownMenuItem>
      ) : null}
      {/* NAMED FOR WHAT IT DOES, not for the state it is in, unlike the key
          row's "Box"/"Direct" cap (which has one cell of width and reads as a
          status light beside its neighbours). A menu row is a sentence, and
          "Use virtual input" says what tapping it gets you. The two directions
          live in DIFFERENT menus (the way out inside the virtual input, the way
          back in the top menu that outlives it), and both write through the one
          `switchTypingSurface`, so they cannot drift. */}
      {gates.surfaceSwitch ? (
        <DropdownMenuItem
          onClick={() =>
            switchTypingSurface(composeSurface ? "direct" : "compose")
          }
        >
          {composeSurface ? <SquareTerminal /> : <MessageSquare />}
          {composeSurface ? "Type directly in the terminal" : "Use virtual input"}
        </DropdownMenuItem>
      ) : null}
      {gates.keysToggle ? (
        <DropdownMenuItem
          onClick={() =>
            void setAccessoryBarVisibility(!accessoryBarVisible)
          }
        >
          {accessoryBarVisible ? <KeyboardOff /> : <Keyboard />}
          {accessoryBarVisible ? "Hide terminal keys" : "Show terminal keys"}
        </DropdownMenuItem>
      ) : null}
      {/* The guaranteed way out of theater. It is a way BACK only, so there is
          no matching "Enter theater mode": entering is the header's expand
          button, and this menu exists precisely for the state where that header
          is not on screen. Same two-arrow glyph as the button it undoes. */}
      {gates.theaterExit ? (
        <DropdownMenuItem onClick={() => exitTheater()}>
          <Minimize2 />
          Leave theater mode
        </DropdownMenuItem>
      ) : null}
      {trailingSeparator && inputMenuHasItems(gates) ? (
        <DropdownMenuSeparator />
      ) : null}
    </>
  )
}
