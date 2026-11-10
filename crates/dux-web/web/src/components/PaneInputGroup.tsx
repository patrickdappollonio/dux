import { InputMenuItems } from "@/components/InputMenuItems"
import {
  DropdownMenuGroup,
  DropdownMenuLabel,
  DropdownMenuSeparator,
} from "@/components/ui/dropdown-menu"
import { useAttachCapability } from "@/lib/attachRegistry"
import { useIsMobile } from "@/hooks/use-mobile"
import { usePaneInputGroup } from "@/lib/paneInputGroup"

/// The label every surface prints over the group. One constant so the phone
/// sheet, the desktop pane menu and the floating pill cannot name it three
/// different things, and so a test can pin it.
export const PANE_INPUT_GROUP_LABEL = "Input"

// The input group, at the top of whatever menu the surface already has: the
// flap's `⋯` on a phone, the pane header's on a computer, the floating pill's in
// theater, the sidebar row's as a fourth anchor on the same body.
//
// It is the one permanent home of the virtual input's controls: the typing
// surface switch in both directions, and on a computer the terminal-keys
// toggle (a phone has that as a button in the action cluster). "Attach a file…"
// joins it because an upload is an input act with no other permanent home. A
// computer puts this on the pane header rather than the cog beside it, because
// the cog's menu is the app's and none of these rows is about the app.
//
// What is in it is the pane's answer, published through `paneInputGroup`: only
// the pane knows whether it owns the input and which surfaces are on offer. The
// attach act is borrowed from that same pane's capability, so the file travels
// through its already-gated socket; both halves have to be there.
export function PaneInputGroup({
  ptyIds,
  /// A separator AFTER the group, for a menu that continues below it. Every
  /// current caller wants one; it is a prop so a menu that ends here does not
  /// have to grow a trailing rule.
  trailingSeparator = true,
}: {
  /// The ptys to read the group under. A surface painted over a pane passes
  /// that pane's single id; a sidebar row, which is over no pane, passes every
  /// pty its subject could be mounted as.
  ptyIds: string[]
  trailingSeparator?: boolean
}) {
  const gates = usePaneInputGroup(ptyIds)
  const isMobile = useIsMobile()
  const attachToPane = useAttachCapability(ptyIds)
  const attach = attachToPane !== null
  // A phone's keys toggle is a button in the action cluster, so the row would be
  // the same control in two places.
  const keysToggle = (gates?.keysToggle ?? false) && !isMobile
  const surfaceSwitch = gates?.surfaceSwitch ?? false
  if (!attach && !surfaceSwitch && !keysToggle) return null
  return (
    <>
      {/* A REAL GROUP, not a label with rows under it: the primitive's label
          part reads its group from context and throws outside one, and the
          grouping is also what a screen reader announces the label as. */}
      <DropdownMenuGroup>
        <DropdownMenuLabel>{PANE_INPUT_GROUP_LABEL}</DropdownMenuLabel>
        <InputMenuItems
          attach={attach}
          surfaceSwitch={surfaceSwitch}
          composeSurface={gates?.composeSurface ?? false}
          keysToggle={keysToggle}
          onAttach={() => attachToPane?.()}
        />
      </DropdownMenuGroup>
      {trailingSeparator ? <DropdownMenuSeparator /> : null}
    </>
  )
}
