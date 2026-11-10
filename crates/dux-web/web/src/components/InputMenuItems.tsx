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
import { keysToggleLabel } from "@/lib/paneInputGroup"
import { switchTypingSurface } from "@/lib/typingSurface"

// The surface-switch row, named for what tapping it gets you rather than for the
// state it is in. Glyph, word and destination are decided together, so the row
// can never offer one surface and travel to the other.
function surfaceSwitchRow(composeSurface: boolean) {
  return composeSurface
    ? {
        icon: <SquareTerminal />,
        label: "Type directly in the terminal",
        target: "direct" as const,
      }
    : {
        icon: <MessageSquare />,
        label: "Use virtual input",
        target: "compose" as const,
      }
}

// The input items the top menu's Input group carries, so the labels, icons and
// store writes live in one place. VISIBILITY IS THE CALLER'S: each row is on a
// predicate the pane published, and this component self-gates nothing. The view
// toggles act immediately, so they are neutral and carry no ellipsis; "Attach a
// file…" carries one, because it opens the operating system's picker.

export function InputMenuItems({
  attach = false,
  surfaceSwitch = false,
  keysToggle = false,
  theaterExit = false,
  onAttach,
  composeSurface = false,
}: {
  /// "Attach a file…". Off when uploads are off server-side, and for a
  /// non-owner, who could not paste the saved path afterwards. A prop rather
  /// than a gate: it is `onAttach`'s own fact.
  attach?: boolean
  /// The typing-surface switch, in the direction `composeSurface` implies.
  surfaceSwitch?: boolean
  /// Hide/Show terminal keys (`ui.mobile_accessory_bar`).
  keysToggle?: boolean
  /// "Leave theater mode": a way back, never a way there.
  theaterExit?: boolean
  /// Opens the file picker. Called synchronously from the item's click, so the
  /// browser's user activation still covers the `.click()` on the hidden input.
  onAttach?: () => void
  /// Which typing surface is live: `true` while the buffered message box is up.
  composeSurface?: boolean
}) {
  const duxState = useDux()
  const accessoryBarVisible = mobileAccessoryBarVisible(duxState)
  const surfaceRow = surfaceSwitchRow(composeSurface)
  return (
    <>
      {attach ? (
        <DropdownMenuItem onClick={() => onAttach?.()}>
          <Paperclip />
          Attach a file…
        </DropdownMenuItem>
      ) : null}
      {surfaceSwitch ? (
        <DropdownMenuItem onClick={() => switchTypingSurface(surfaceRow.target)}>
          {surfaceRow.icon}
          {surfaceRow.label}
        </DropdownMenuItem>
      ) : null}
      {keysToggle ? (
        <DropdownMenuItem
          onClick={() => void setAccessoryBarVisibility(!accessoryBarVisible)}
        >
          {accessoryBarVisible ? <KeyboardOff /> : <Keyboard />}
          {keysToggleLabel(accessoryBarVisible)}
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
