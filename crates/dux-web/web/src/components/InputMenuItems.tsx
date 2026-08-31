import {
  Keyboard,
  KeyboardOff,
  MessageSquare,
  Minimize2,
  PanelTopClose,
  PanelTopOpen,
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
  mobileTopBarVisible,
  setMobileBarVisibility,
  useDux,
} from "@/lib/store"
import {
  inputMenuHasItems,
  type InputMenuGates,
} from "@/lib/inputMenu"
import { setTypingSurface } from "@/lib/typingSurface"

// THE INPUT MENU'S ITEMS, shared by every menu that carries any of them: the
// always-present input `⋯` below the terminal (see `InputMenu`), the mobile
// agent screen's header menu (AgentActionsMenu, context="terminal") and the
// agentless project/standalone screens' header menu (MobileShell). One
// component so the labels, icons and store writes can never drift between them.
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
  const topBarVisible = mobileTopBarVisible(duxState)
  const accessoryBarVisible = mobileAccessoryBarVisible(duxState)
  // The separator between "Attach a file…" (an action on a file) and the view
  // toggles below it (preferences about this screen's chrome) only earns its
  // place when both sides exist.
  const viewItems = gates.surfaceSwitch || gates.keysToggle || gates.topBarToggle
  return (
    <>
      {gates.attach ? (
        <DropdownMenuItem onClick={() => onAttach?.()}>
          <Paperclip />
          Attach a file…
        </DropdownMenuItem>
      ) : null}
      {gates.attach && viewItems ? <DropdownMenuSeparator /> : null}
      {/* NAMED FOR WHAT IT DOES, not for the state it is in, unlike the key
          row's "Box"/"Direct" cap (which has one cell of width and reads as a
          status light beside its neighbours). A menu row is a sentence, and
          "Use the message box" says what tapping it gets you. Both write
          through the same `setTypingSurface`, so the two cannot drift. */}
      {gates.surfaceSwitch ? (
        <DropdownMenuItem
          onClick={() => setTypingSurface(composeSurface ? "direct" : "compose")}
        >
          {composeSurface ? <SquareTerminal /> : <MessageSquare />}
          {composeSurface ? "Type directly in the terminal" : "Use the message box"}
        </DropdownMenuItem>
      ) : null}
      {gates.keysToggle ? (
        <DropdownMenuItem
          onClick={() =>
            void setMobileBarVisibility("accessory", !accessoryBarVisible)
          }
        >
          {accessoryBarVisible ? <KeyboardOff /> : <Keyboard />}
          {accessoryBarVisible ? "Hide terminal keys" : "Show terminal keys"}
        </DropdownMenuItem>
      ) : null}
      {gates.topBarToggle ? (
        <DropdownMenuItem
          onClick={() => void setMobileBarVisibility("top", !topBarVisible)}
        >
          {topBarVisible ? <PanelTopClose /> : <PanelTopOpen />}
          {topBarVisible ? "Hide top bar" : "Show top bar"}
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
