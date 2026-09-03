import { Ellipsis, Settings } from "lucide-react"

import { AppMenuBody } from "@/components/AppMenu"
import { AgentActionsMenu } from "@/components/FlatAgentList"
import { InputMenuItems } from "@/components/InputMenuItems"
import { PaneInputGroup } from "@/components/PaneInputGroup"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { useDux } from "@/lib/store"
import type { SessionView } from "@/lib/types"

/// The one name the control answers to, on both of the surfaces it is painted
/// on. The flap and the pill are the same `⋯` in two places, so a screen reader
/// and a voice command must not have to learn which one is on screen.
export const MOBILE_PANE_MENU_LABEL = "Session actions"

// THE PHONE'S ONE PANE MENU, opened from the docked flap and from the floating
// pill.
//
// The two used to answer to different menus: the flap opened the agent's
// actions, the pill opened the app menu. That made theater on a phone the one
// state in which renaming, deleting, the tab and project submenus and every
// other per-agent action were simply unreachable, and it made the flight a lie:
// the cluster flies across the screen as one object precisely because it IS one
// object, and one of its four buttons changing what it does on arrival is the
// thing the animation says cannot happen.
//
// So there is one body, in this order: the pane's INPUT group, then the agent's
// own actions, then the way out of theater while the mode is on, then the app
// menu as a drill named for the cog it stands in for. A phone renders every one
// of these menus as a bottom sheet with its own stacked sub-sheets, so a
// submenu is the sheet's own idiom rather than a flyout squeezed against an
// edge.
//
// THE INPUT GROUP IS FIRST, above the agent's own actions, because this sheet
// is the only permanent home the virtual input's controls have: typing directly
// in the terminal takes the whole bottom bar away, and the way back has to be
// somewhere that never leaves. It is a thumb's reach from the `⋯` that opened
// the sheet, which is where the hand already is.
//
// There is no "Changes ±N" row: the flap and the pill both carry a real count
// BUTTON beside this trigger, keyboard-reachable and labelled for a screen
// reader, and a second copy of it in the menu was two places for the same
// number to be printed.
export function MobilePaneMenu({
  session,
  side = "bottom",
}: {
  session: SessionView
  /// Which way the menu opens on a surface wide enough to anchor it: the flap
  /// hangs from the band at the top of the pane, the pill floats at the bottom.
  /// A phone ignores it and renders a sheet.
  side?: "top" | "bottom"
}) {
  return (
    <DropdownMenu>
      <SimpleTooltip content={MOBILE_PANE_MENU_LABEL}>
        <DropdownMenuTrigger
          render={
            <Button
              variant="ghost"
              size="icon"
              className="size-10 shrink-0 rounded-full"
              aria-label={MOBILE_PANE_MENU_LABEL}
            />
          }
        >
          <Ellipsis />
        </DropdownMenuTrigger>
      </SimpleTooltip>
      <DropdownMenuContent align="end" side={side}>
        <MobilePaneMenuBody session={session} />
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

/// Everything the phone's pane menu carries, ready to drop into any content.
export function MobilePaneMenuBody({ session }: { session: SessionView }) {
  const duxState = useDux()
  const theater = duxState.theater
  return (
    <>
      {/* Whichever of this agent's panes is mounted and owns its input answers
          for the whole group, exactly as the row menus borrow the attach: an
          upload travels through that pane's own gated connection and lands in
          its own sink, and the surface items are that pane's own state. */}
      <PaneInputGroup ptyIds={[session.id, ...session.tabs.map((t) => t.id)]} />
      <AgentActionsMenu session={session} context="terminal" />
      {theater ? (
        <>
          <DropdownMenuSeparator />
          {/* The way back, from the surface the mode leaves on screen. */}
          <InputMenuItems theaterExit />
        </>
      ) : null}
      <DropdownMenuSeparator />
      {/* NAMED FOR THE CONTROL IT STANDS IN FOR. Theater takes the phone's top
          bar, and with it the cog; the flap's own header is gone in both modes.
          A user looking for the app's own actions should find them under the
          name they know, and this is the one trigger on screen. */}
      <DropdownMenuSub>
        <DropdownMenuSubTrigger>
          <Settings />
          Settings
        </DropdownMenuSubTrigger>
        <DropdownMenuSubContent side="left">
          <AppMenuBody />
        </DropdownMenuSubContent>
      </DropdownMenuSub>
    </>
  )
}
