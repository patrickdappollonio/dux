import { InputMenuItems } from "@/components/InputMenuItems"
import {
  DropdownMenuGroup,
  DropdownMenuLabel,
  DropdownMenuSeparator,
} from "@/components/ui/dropdown-menu"
import { useAttachCapability } from "@/lib/attachRegistry"
import {
  paneInputGroupHasItems,
  usePaneInputGroup,
} from "@/lib/paneInputGroup"

/// The label every surface prints over the group. One constant so the phone
/// sheet, the desktop pane menu and the floating pill cannot name it three
/// different things, and so a test can pin it.
export const PANE_INPUT_GROUP_LABEL = "Input"

// THE INPUT GROUP, AT THE TOP OF WHATEVER MENU THE SURFACE ALREADY HAS.
//
// "Type directly in the terminal" removes the whole bottom bar, compose row,
// key row and the input `⋯` with them, so the way back cannot live down there:
// a control that only exists while you do not need it is not a way back at all.
// It lives here instead, in the menu the surface always has for the pane in
// front of it (the flap's `⋯` on a phone, the pane's own row `⋯` in the sidebar
// on a computer, the floating pill's in theater), and "Attach a file…" joins it
// because an upload is an input act and had no other permanent home either.
//
// A COMPUTER PUTS IT IN THE SIDEBAR ROW rather than in the header's cog, which
// is deliberate: the cog's menu is the app's, and none of these rows is about
// the app. The row menu is the per-agent (and per-terminal) surface that
// already exists, and it already carried the attach item.
//
// The GROUP LABEL stays even with one item in it. It is the only labelled group
// in these menus, and that is the point: these rows are about the pane's typing
// surface rather than about the agent, and an unlabelled pair of them at the
// top of an agent's actions reads as two more agent actions.
//
// WHAT IS IN IT IS THE PANE'S ANSWER, not this component's: the pane knows
// whether it owns the input and which surfaces are up, and publishes through
// `paneInputGroup`. The attach act is borrowed from the same pane's own
// capability, so the file travels through its already-gated socket and lands in
// its own sink; both halves have to be there.
export function PaneInputGroup({
  ptyIds,
  /// A separator AFTER the group, for a menu that continues below it. Every
  /// current caller wants one; it is a prop so a menu that ends here does not
  /// have to grow a trailing rule.
  trailingSeparator = true,
}: {
  /// Every pty this surface could be about: an agent passes its session-slot id
  /// and every tab id, a terminal its single id.
  ptyIds: string[]
  trailingSeparator?: boolean
}) {
  const gates = usePaneInputGroup(ptyIds)
  const attachToPane = useAttachCapability(ptyIds)
  const attach = attachToPane !== null
  if (!attach && !paneInputGroupHasItems(gates)) return null
  return (
    <>
      {/* A REAL GROUP, not a label with rows under it: the primitive's label
          part reads its group from context and throws outside one, and the
          grouping is also what a screen reader announces the label as. */}
      <DropdownMenuGroup>
        <DropdownMenuLabel>{PANE_INPUT_GROUP_LABEL}</DropdownMenuLabel>
        <InputMenuItems
          attach={attach}
          gates={{
            surfaceSwitch: gates?.surfaceSwitch ?? false,
            keysToggle: gates?.keysToggle ?? false,
          }}
          // The top menu only ever offers the way BACK to the virtual input:
          // the other direction is the bottom `⋯`, which exists exactly while
          // the virtual input does.
          composeSurface={false}
          onAttach={() => attachToPane?.()}
        />
      </DropdownMenuGroup>
      {trailingSeparator ? <DropdownMenuSeparator /> : null}
    </>
  )
}
